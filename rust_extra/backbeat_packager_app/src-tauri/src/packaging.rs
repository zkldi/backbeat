use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use backbeat_core::AssetId;
use backbeat_packager::SeenCache;
use serde::Serialize;
use walkdir::WalkDir;

#[derive(Clone, Default)]
pub(crate) struct BatchState {
	cache: SeenCache,
	imported_assets: Arc<Mutex<HashSet<AssetId>>>,
}

#[derive(Serialize)]
pub(crate) struct Discovery {
	paths: Vec<String>,
	errors: Vec<String>,
}

pub(crate) fn discover(inputs: &[String]) -> Discovery {
	let mut paths = BTreeSet::new();
	let mut errors = Vec::new();

	for input in inputs {
		let root = Path::new(input);
		if root.is_file() && !backbeat_packager::should_be_packaged(root) {
			errors.push(format!("Unsupported chart: {input}"));
			continue;
		}

		for entry in WalkDir::new(root).follow_links(false) {
			let entry = match entry {
				Ok(entry) => entry,
				Err(error) => {
					errors.push(error.to_string());
					continue;
				}
			};

			if !entry.file_type().is_file() || !backbeat_packager::should_be_packaged(entry.path())
			{
				continue;
			}

			match canonical_chart_path(entry.path()) {
				Ok(path) => {
					paths.insert(path);
				}
				Err(error) => errors.push(error),
			}
		}
	}

	Discovery {
		paths: paths.into_iter().collect(),
		errors,
	}
}

fn canonical_chart_path(path: &Path) -> Result<String, String> {
	let canonical = path
		.canonicalize()
		.map_err(|error| format!("{}: {error}", path.display()))?;

	canonical
		.into_os_string()
		.into_string()
		.map_err(|path| format!("Path is not Unicode: {}", path.to_string_lossy()))
}

#[derive(Serialize)]
pub(crate) struct Packaged {
	output: String,
	charts: usize,
	missing: Vec<String>,
}

pub(crate) fn package_chart(
	path: &str,
	output_dir: &str,
	batch: &BatchState,
) -> Result<Packaged, String> {
	package_with_cache(Path::new(path), Path::new(output_dir), &batch.cache)
		.map_err(|error| error.to_string())
}

pub(crate) fn import_chart(
	path: &Path,
	store: &backbeat_sdk::Backbeat,
	batch: &BatchState,
) -> Result<Packaged, String> {
	let (packages, missing) = package_for_batch(path, &batch.cache)?;
	let chart_dir = path.parent().unwrap_or_else(|| Path::new("."));

	// Import assets first so a failed asset read does not publish a new bundle.
	for package in &packages {
		for (asset_path, id) in &package.assets {
			if batch
				.imported_assets
				.lock()
				.map_err(|error| error.to_string())?
				.contains(id)
			{
				continue;
			}

			if store.get_asset(*id).is_err() {
				store
					.import_asset(chart_dir.join(asset_path.as_path()))
					.map_err(|error| format!("Could not import asset {asset_path}: {error}"))?;
			}

			batch
				.imported_assets
				.lock()
				.map_err(|error| error.to_string())?
				.insert(*id);
		}
	}

	for package in &packages {
		store
			.import_bundle(package)
			.map_err(|error| error.to_string())?;
	}

	Ok(Packaged {
		output: store.config().store.path.to_string_lossy().into_owned(),
		charts: packages.len(),
		missing: missing.into_iter().map(|path| path.to_string()).collect(),
	})
}

fn package_for_batch(
	path: &Path,
	cache: &SeenCache,
) -> Result<
	(
		Vec<backbeat_core::BackbeatFile>,
		Vec<backbeat_core::AssetPath>,
	),
	String,
> {
	let chart_cache = cache.with_isolated_missing_assets();
	let packages = backbeat_packager::package_with_cache(path, &chart_cache)
		.map_err(|error| error.to_string())?;
	let missing = chart_cache.missing_asset_paths();

	Ok((packages, missing))
}

fn package_with_cache(
	path: &Path,
	output_dir: &Path,
	cache: &SeenCache,
) -> Result<Packaged, Box<dyn std::error::Error>> {
	if !output_dir.is_dir() {
		return Err("Choose an existing output folder".into());
	}

	let (packages, missing) = package_for_batch(path, cache)?;

	// Build fully before publishing; no partial archives or overwritten files.
	let mut temporary = tempfile::Builder::new()
		.suffix(".bbzip")
		.tempfile_in(output_dir)?;

	backbeat_packager::write_bbzip(path, &packages, temporary.as_file_mut())?;
	temporary.as_file().sync_all()?;

	let description = packages
		.first()
		.ok_or("Chart has no playable charts")?
		.desc
		.as_str();
	let filename = backbeat_packager::sanitise_filename(description);

	let mut number = 0_u64;
	let output: PathBuf = loop {
		let name = if number == 0 {
			format!("{filename}.bbzip")
		} else {
			format!("{filename} ({number}).bbzip")
		};

		let output = output_dir.join(name);

		match temporary.persist_noclobber(&output) {
			Ok(_) => break output,
			Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
				temporary = error.file;
				number += 1;
			}
			Err(error) => return Err(error.error.into()),
		}
	};

	Ok(Packaged {
		output: output.to_string_lossy().into_owned(),
		charts: packages.len(),
		missing: missing.into_iter().map(|path| path.to_string()).collect(),
	})
}

#[cfg(test)]
fn package(path: &Path, output_dir: &Path) -> Result<Packaged, Box<dyn std::error::Error>> {
	package_with_cache(path, output_dir, &SeenCache::new())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
	use super::*;
	use std::fs;

	#[test]
	fn recursive_discovery_deduplicates_and_reports_bad_inputs() {
		let dir = tempfile::tempdir().unwrap();
		fs::create_dir(dir.path().join("nested")).unwrap();
		let chart = dir.path().join("nested/song.SM");
		fs::write(&chart, "").unwrap();
		let audio = dir.path().join("song.ogg");
		fs::write(&audio, "").unwrap();
		let result = discover(&[
			dir.path().display().to_string(),
			chart.display().to_string(),
			audio.display().to_string(),
			dir.path().join("absent").display().to_string(),
		]);
		assert_eq!(result.paths.len(), 1);
		assert_eq!(result.errors.len(), 2);
	}

	#[test]
	fn batch_cache_keeps_missing_assets_separate_for_each_chart() {
		let dir = tempfile::tempdir().unwrap();
		let first = dir.path().join("first.sm");
		let second = dir.path().join("second.sm");
		fs::write(
			&first,
			"#TITLE:First;\n#MUSIC:first.ogg;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();
		fs::write(
			&second,
			"#TITLE:Second;\n#MUSIC:second.ogg;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();

		let cache = SeenCache::new();
		let (_, first_missing) = package_for_batch(&first, &cache).unwrap();
		let (_, second_missing) = package_for_batch(&second, &cache).unwrap();

		assert_eq!(
			first_missing,
			vec![backbeat_core::AssetPath::from_path("first.ogg").unwrap()]
		);
		assert_eq!(
			second_missing,
			vec![backbeat_core::AssetPath::from_path("second.ogg").unwrap()]
		);
	}

	#[test]
	fn archives_are_readable_and_never_overwritten_or_repackaged_as_assets() {
		let dir = tempfile::tempdir().unwrap();
		let chart = dir.path().join("song.sm");
		fs::write(
			&chart,
			"#TITLE:Test;\n#MUSIC:missing.ogg;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();
		let first = package(&chart, dir.path()).unwrap();
		let packages = backbeat_packager::package(&chart).unwrap();
		let filename = backbeat_packager::sanitise_filename(packages[0].desc.as_str());
		assert_eq!(
			Path::new(&first.output).file_name().unwrap(),
			format!("{filename}.bbzip").as_str()
		);
		assert_eq!(first.charts, 1);
		assert_eq!(first.missing, vec!["missing.ogg"]);
		let bytes = fs::read(&first.output).unwrap();
		let second = package(&chart, dir.path()).unwrap();
		assert_eq!(
			Path::new(&second.output).file_name().unwrap(),
			format!("{filename} (1).bbzip").as_str()
		);
		assert_ne!(first.output, second.output);
		assert_eq!(fs::read(&first.output).unwrap(), bytes);
		let mut archive =
			backbeat_core::BbZipReader::new(fs::File::open(second.output).unwrap()).unwrap();
		assert_eq!(archive.len(), 1);
		assert!(archive.get_entry(0).unwrap().is_some());
	}

	#[test]
	fn direct_import_stores_charts_and_assets_without_archives() {
		let dir = tempfile::tempdir().unwrap();
		let source = dir.path().join("source");
		fs::create_dir(&source).unwrap();
		let chart = source.join("song.sm");
		fs::write(&chart, "#TITLE:Test;\n#MUSIC:song.ogg;\n#BANNER:missing.png;\n#NOTES:dance-single:A:Easy:1:0:0000;\n#NOTES:dance-single:B:Hard:9:0:0000;\n").unwrap();
		fs::write(source.join("song.ogg"), b"test audio").unwrap();

		let mut config = backbeat_store_config::BackbeatConfig::default();
		config.store.path = dir.path().join("store");
		config.write_to_dir(&dir.path().join("config")).unwrap();
		let store =
			backbeat_sdk::Backbeat::open_with_overridden_config_dir(dir.path().join("config"))
				.unwrap();
		let mut batch = BatchState::default();

		let result = import_chart(&chart, &store, &mut batch).unwrap();
		assert_eq!(result.charts, 2);
		assert_eq!(result.output, config.store.path.to_string_lossy());
		assert_eq!(result.missing, vec!["missing.png"]);
		assert_eq!(store.stats().unwrap().charts, 2);
		assert_eq!(store.stats().unwrap().asset_count, 1);
		assert!(
			store
				.get_asset(backbeat_core::Sha256::checksum_bytes(b"test audio").into())
				.is_ok()
		);

		// Retrying an import is idempotent, including assets shared by charts.
		import_chart(&chart, &store, &mut batch).unwrap();
		assert_eq!(store.stats().unwrap().charts, 2);
		assert_eq!(store.stats().unwrap().asset_count, 1);
		assert_eq!(fs::read_dir(&source).unwrap().count(), 2);
	}

	#[test]
	fn concurrent_imports_share_batch_state_and_assets() {
		let dir = tempfile::tempdir().unwrap();
		let source = dir.path().join("source");
		fs::create_dir(&source).unwrap();
		let paths: Vec<_> = ["first", "second"]
			.into_iter()
			.map(|name| {
				let path = source.join(format!("{name}.sm"));
				fs::write(
					&path,
					format!(
						"#TITLE:{name};\n#MUSIC:song.ogg;\n#NOTES:dance-single:A:Easy:1:0:0000;\n"
					),
				)
				.unwrap();
				path
			})
			.collect();
		fs::write(source.join("song.ogg"), b"shared audio").unwrap();
		let mut config = backbeat_store_config::BackbeatConfig::default();
		config.store.path = dir.path().join("store");
		config.write_to_dir(&dir.path().join("config")).unwrap();
		let store =
			backbeat_sdk::Backbeat::open_with_overridden_config_dir(dir.path().join("config"))
				.unwrap();
		let batch = BatchState::default();
		let barrier = std::sync::Barrier::new(paths.len());
		std::thread::scope(|scope| {
			let handles: Vec<_> = paths
				.iter()
				.map(|path| {
					let store = store.clone();
					let batch = batch.clone();
					let barrier = &barrier;
					scope.spawn(move || {
						barrier.wait();
						import_chart(path, &store, &batch).unwrap()
					})
				})
				.collect();
			for handle in handles {
				assert_eq!(handle.join().unwrap().charts, 1);
			}
		});
		assert_eq!(store.stats().unwrap().charts, 2);
		assert_eq!(store.stats().unwrap().asset_count, 1);
		assert_eq!(batch.imported_assets.lock().unwrap().len(), 1);
	}

	#[test]
	fn invalid_chart_leaves_no_archive() {
		let dir = tempfile::tempdir().unwrap();
		let chart = dir.path().join("empty.sm");
		fs::write(&chart, "#TITLE:No charts;").unwrap();
		assert!(package(&chart, dir.path()).is_err());
		assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
	}

	#[test]
	fn multiple_playable_charts_share_one_copy_of_each_asset() {
		use std::io::Read;
		let dir = tempfile::tempdir().unwrap();
		let chart = dir.path().join("song.sm");
		fs::write(&chart, "#TITLE:Test;\n#MUSIC:song.ogg;\n#NOTES:dance-single:A:Easy:1:0:0000;\n#NOTES:dance-single:B:Hard:9:0:0000;\n").unwrap();
		fs::write(dir.path().join("song.ogg"), b"test audio").unwrap();
		let result = package(&chart, dir.path()).unwrap();
		assert_eq!(result.charts, 2);
		assert!(result.missing.is_empty());
		let mut archive =
			backbeat_core::BbZipReader::new(fs::File::open(result.output).unwrap()).unwrap();
		assert_eq!(archive.len(), 3);
		let mut charts = 0;
		let mut assets = 0;
		for index in 0..archive.len() {
			match archive.get_entry(index).unwrap().unwrap() {
				backbeat_core::BbZipEntry::Bb(chart) => {
					assert_eq!(chart.assets.len(), 1);
					charts += 1;
				}
				backbeat_core::BbZipEntry::Asset(mut asset) => {
					let mut bytes = Vec::new();
					asset.read_to_end(&mut bytes).unwrap();
					assert_eq!(bytes, b"test audio");
					assets += 1;
				}
			}
		}
		assert_eq!((charts, assets), (2, 1));
	}
}

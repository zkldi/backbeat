#![cfg_attr(test, allow(unreachable_pub, clippy::all, clippy::restriction))]
#![cfg_attr(test, allow(clippy::pedantic, clippy::nursery, clippy::cargo))]
#![forbid(unsafe_code)]
//! Packaging. Take rhythm game charts and produce backbeat manifests from them.

mod base;
mod blacklist;
// mod clone_hero_chart;
mod deps;
// mod dtx;
mod error;
mod format;
pub mod fracturing;
mod seen_cache;
// mod tja;
mod util;

pub use error::PackageError;
pub use format::sm_enrich;
pub use fracturing::{MeldError, meld, meld_chart_bytes};
pub use seen_cache::SeenCache;
pub use util::sanitise_filename;

use std::collections::{HashMap, HashSet};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use backbeat_core::{AssetId, AssetPath, BackbeatFile, BbZipWriter};

use crate::error::FromFileError;
use crate::format::Format;

/// Package the chart file at `path` into one [`BackbeatFile`] per playable chart.
pub fn package(path: impl AsRef<Path>) -> Result<Vec<BackbeatFile>, PackageError> {
	package_with_cache(path, &SeenCache::new())
}

/// Whether `path` is a a chart that should be packaged. Backbeat doesn't have any constraints
/// on what you can package technically, but in practice you don't want to recursively package "mp3s"
/// as if _they're_ the chart.
pub fn should_be_packaged(path: impl AsRef<Path>) -> bool {
	Format::from_path(path).is_some()
}

/// Whether charts with this filename can be combined into one source file.
pub fn supports_melding(filename: &backbeat_core::ChartFilename) -> bool {
	Format::from_path(filename.as_str()).is_some_and(Format::can_meld)
}

/// Package a chart and return the referenced asset paths that could not be
/// found on disk.
pub fn package_with_missing_assets(
	path: impl AsRef<Path>,
) -> Result<(Vec<BackbeatFile>, Vec<AssetPath>), PackageError> {
	let cache = SeenCache::new();
	let packages = package_with_cache(path, &cache)?;

	let missing = cache.missing_asset_paths();
	Ok((packages, missing))
}

/// Like [`package`] but with a [`SeenCache`] so files seen multiple times
/// don't get read from disk twice.
pub fn package_with_cache(
	path: impl AsRef<Path>,
	cache: &SeenCache,
) -> Result<Vec<BackbeatFile>, PackageError> {
	let path = path.as_ref();
	let mut bb = package_base(path, cache)?;

	let Some(ext) = Format::from_path(bb.filename.as_str()) else {
		return Err(FromFileError::UnrecognisedFiletype(bb.filename.to_string()).into());
	};

	if !ext.should_fracture() {
		bb.desc = backbeat_inspector::inspect(&bb.chart.decompress(), &bb.filename)
			.map_err(FromFileError::Inspect)?
			.description;
		return Ok(vec![bb]);
	}

	let bytes = bb.chart.decompress();

	let fractured = ext.fracture_bytes(&bytes)?;

	Ok(fractured
		.into_iter()
		.map(|chart| {
			let mut fractured = bb.clone();
			fractured.desc = backbeat_inspector::inspect(&chart, &fractured.filename)
				.map_err(FromFileError::Inspect)?
				.description;
			fractured.chart = backbeat_core::ChartData::compress(&chart)
				.map_err(crate::error::FromFileError::Build)?;
			Ok(fractured)
		})
		.collect::<Result<_, crate::error::FromFileError>>()?)
}

/// Write packaged charts and their referenced assets to a `.bbzip` archive.
///
/// Archive entry names are generated because they have no meaning to Backbeat.
/// Assets are deduplicated by their content hash.
pub fn write_bbzip<W>(
	chart_path: impl AsRef<Path>,
	packages: &[BackbeatFile],
	output: W,
) -> Result<W, PackageError>
where
	W: Write + Seek,
{
	let chart_dir = chart_path.as_ref().parent().unwrap_or(Path::new("."));
	let mut archive = BbZipWriter::new(output);

	let mut seen = HashSet::new();

	for bb in packages {
		let base_desc = sanitise_filename(bb.desc.as_str());
		let mut desc = base_desc.clone();

		let mut i = 1;
		while !seen.insert(desc.clone()) {
			desc = format!("{base_desc} ({i})");
			i += 1;
		}

		let json = bb.to_json();
		archive.add_file(format!("{desc}.bb"), json.as_slice())?;
	}

	let mut all_assets = HashMap::<AssetId, PathBuf>::new();
	for bb in packages {
		for (path, id) in &bb.assets {
			all_assets
				.entry(*id)
				.or_insert_with(|| chart_dir.join(path.as_path()));
		}
	}
	let all_assets = all_assets;

	for (id, path) in all_assets {
		let mut file = std::fs::File::open(path)?;
		archive.add_file(id.to_string(), &mut file)?;
	}

	Ok(archive.finish()?)
}

fn package_base(path: &Path, cache: &SeenCache) -> Result<BackbeatFile, PackageError> {
	let mut bb = base::from_file(path, cache)?;

	let extension = if let Some(ext) = Format::from_path(path) {
		ext.resolve_assets(&mut bb, path, cache)?;
		Some(ext)
	} else {
		None
	};

	if let Some(extension) = extension {
		deps::assert_not_pathing_up_too_much(&bb, extension.max_allowed_up_pathing())?;
	}

	Ok(bb)
}

#[cfg(test)]
mod tests {
	use crate::error::FromFileError;
	use crate::fracturing::FractureError;

	use super::*;
	use backbeat_core::AssetPath;
	use std::fs;
	use std::path::Path;
	use tempfile::TempDir;

	#[test]
	fn chart_filename_extracts_basename() {
		let source = Path::new("fixtures/sm/0x1311/0x1311.sm");
		assert_eq!(base::chart_filename(source).unwrap().as_str(), "0x1311.sm");
	}

	#[test]
	fn unknown_extension_is_rejected_with_its_full_filename() {
		let tmp = TempDir::new().unwrap();
		let dir = tmp.path();

		fs::write(dir.join("notes.xyz"), b"opaque chart bytes").unwrap();
		fs::write(dir.join("readme.txt"), b"hello").unwrap();

		let error = package(dir.join("notes.xyz")).unwrap_err();
		assert!(matches!(
			error,
			PackageError::FromFile(FromFileError::UnrecognisedFiletype(filename))
				if filename == "notes.xyz"
		));
	}

	#[test]
	fn folder_scan_includes_non_referenced_siblings() {
		let tmp = TempDir::new().unwrap();
		let dir = tmp.path();

		fs::write(dir.join("song.bms"), b"#TITLE Test;\n#WAV01 kick\n").unwrap();
		fs::write(dir.join("kick.wav"), b"audio").unwrap();
		fs::write(dir.join("readme.txt"), b"hello").unwrap();

		let bbs = package(dir.join("song.bms")).expect("package");
		assert_eq!(bbs.len(), 1);
		let bb = &bbs[0];

		assert!(
			bb.assets
				.contains_key(&AssetPath::from_path("kick.wav").unwrap())
		);
		assert!(
			bb.assets
				.contains_key(&AssetPath::from_path("readme.txt").unwrap())
		);
	}

	#[test]
	fn package_reports_missing_asset_references() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.sm");
		fs::write(
			&path,
			b"#TITLE:Test;\n#MUSIC:missing.ogg;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();

		let (packages, missing) = package_with_missing_assets(&path).unwrap();
		assert_eq!(packages.len(), 1);
		assert_eq!(missing, vec![AssetPath::from_path("missing.ogg").unwrap()]);
	}

	#[test]
	fn package_fractures_sm_charts() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.sm");
		fs::write(
			&path,
			b"#TITLE:Test;\n#NOTES:dance-single:A:Easy:1:0:0000;\n#NOTES:dance-single:B:Hard:9:0:0000;\n",
		)
		.unwrap();

		let packages = package(&path).unwrap();
		assert_eq!(packages.len(), 2);
		for package in packages {
			assert_eq!(package.filename.as_str(), "song.sm");
			let chart = package.chart.decompress();
			assert_eq!(
				rg_formats::sm_msd::from_bytes(&chart)
					.all_with_tag("NOTES")
					.len(),
				1
			);
		}
	}

	#[test]
	fn package_fractures_ssc_charts() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.ssc");
		fs::write(
			&path,
			b"#TITLE:Test;\n\
			 #NOTEDATA:;\n#STEPSTYPE:dance-single;\n#DIFFICULTY:Easy;\n#METER:1;\n#NOTES:\n0000\n;\n\
			 #NOTEDATA:;\n#STEPSTYPE:dance-single;\n#DIFFICULTY:Hard;\n#METER:9;\n#NOTES:\n0000\n;\n",
		)
		.unwrap();

		let packages = package(&path).unwrap();
		assert_eq!(packages.len(), 2);
		for package in packages {
			assert_eq!(package.filename.as_str(), "song.ssc");
			let chart = package.chart.decompress();
			assert_eq!(
				rg_formats::sm_msd::from_bytes(&chart)
					.all_with_tag("NOTEDATA")
					.len(),
				1
			);
		}
	}

	#[test]
	fn package_fractures_dwi_charts() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.dwi");
		fs::write(
			&path,
			b"#TITLE:Test;\n#ARTIST:A;\n#SINGLE:BASIC:3:44;\n#DOUBLE:MANIAC:9:88;\n",
		)
		.unwrap();

		let packages = package(&path).unwrap();
		assert_eq!(packages.len(), 2);
		for package in packages {
			assert_eq!(package.filename.as_str(), "song.dwi");
			assert!(backbeat_inspector::inspect_bundle(&package).is_ok());
		}
	}

	#[test]
	fn package_rejects_sm_with_zero_charts() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.sm");
		fs::write(&path, b"#TITLE:Test;\n#ARTIST:A;\n").unwrap();

		let err = package(&path).unwrap_err();
		std::assert_matches!(
			err,
			PackageError::Fracture(crate::fracturing::FractureError::NoCharts)
		);
	}

	#[test]
	fn package_rejects_ssc_with_zero_charts() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.ssc");
		fs::write(&path, b"#TITLE:Test;\n#ARTIST:A;\n").unwrap();

		let err = package(&path).unwrap_err();
		std::assert_matches!(
			err,
			PackageError::Fracture(crate::fracturing::FractureError::NoCharts)
		);
	}

	#[test]
	fn fracture_emits_raw_sm_charts() {
		let charts = Format::Sm.fracture_bytes(
			b"#TITLE:Test;\n#NOTES:dance-single:A:Easy:1:0:0000;\n#NOTES:dance-single:B:Hard:9:0:0000;\n",
		)
		.unwrap();
		assert_eq!(charts.len(), 2);
		for chart in charts {
			assert_eq!(
				rg_formats::sm_msd::from_bytes(&chart)
					.all_with_tag("NOTES")
					.len(),
				1
			);
		}
	}

	#[test]
	fn fracture_rejects_non_fracturable_format() {
		let err = Format::Bms.fracture_bytes(b"#TITLE Test;\n").unwrap_err();
		std::assert_matches!(err, FractureError::UnsupportedFormat(format) if format == "bms");
	}

	#[test]
	fn package_accepts_parent_traversing_asset_ref() {
		let tmp = TempDir::new().unwrap();
		let root = tmp.path();
		let chart_dir = root.join("chart");
		fs::create_dir(&chart_dir).unwrap();
		fs::write(root.join("bg.png"), b"png").unwrap();
		fs::write(
			chart_dir.join("song.sm"),
			b"#TITLE:Test;\n#BANNER:../bg.png;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();

		let packages = package(chart_dir.join("song.sm")).unwrap();
		let asset_id = backbeat_core::Sha256::checksum_bytes(b"png").into();
		assert_eq!(
			packages[0]
				.assets
				.get(&AssetPath::from_path("../bg.png").unwrap()),
			Some(&asset_id)
		);
	}

	#[test]
	fn package_rejects_stepmania_assets_more_than_one_level_up() {
		let tmp = TempDir::new().unwrap();
		let root = tmp.path();
		let chart_dir = root.join("chart/nested");
		fs::create_dir_all(&chart_dir).unwrap();
		fs::write(root.join("bg.png"), b"png").unwrap();
		fs::write(
			chart_dir.join("song.sm"),
			b"#TITLE:Test;\n#BANNER:../../bg.png;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();

		let err = package(chart_dir.join("song.sm")).unwrap_err();
		std::assert_matches!(
			err,
			PackageError::FromFile(FromFileError::AssetPathTooDeep { path, max_depth: 1 }) if path == "../../bg.png"
		);
	}

	#[test]
	fn package_returns_one_sm_bundle_for_single_chart() {
		let tmp = TempDir::new().unwrap();
		let path = tmp.path().join("song.sm");
		fs::write(
			&path,
			b"#TITLE:Test;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
		)
		.unwrap();

		let bbs = package(&path).unwrap();
		assert_eq!(bbs.len(), 1);
		assert_eq!(
			rg_formats::sm_msd::from_bytes(&bbs[0].chart.decompress())
				.all_with_tag("NOTES")
				.len(),
			1
		);
	}
}

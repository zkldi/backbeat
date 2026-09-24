//! The backbeat store itself. This is the interface for a user's backbeat store.

mod export;
pub mod get;
mod internal;
mod schema;
pub mod stats;
mod tamper_seal;

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use backbeat_core::asset_id::AssetId;
use backbeat_core::bbzip::{BbZipEntry, BbZipReader};
use backbeat_core::{
	BackbeatFile, BundleId, ChartData, ChartDesc, ChartId, CollectionHeader, CollectionKind,
	Course, IdAlgorithm, Pack, Sha256, Table, TableChart, TableFolder, TableLevel, TableTags,
	ValidGamemodeIdentifier,
};
use backbeat_server_client::{BackbeatServerClient, RemoteError};
use backbeat_store_config::{BackbeatConfig, ConfigError, ServerConfig, default_config_dir};
use futures::{StreamExt, stream};
use parking_lot::RwLock;
use reqwest::Client;
use sqlx::SqlitePool;
use sqlx::sqlite::{
	SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};

use crate::asset_store::AssetStore;
use crate::assets::{AssetDependent, AssetDetail};
use crate::bundle::detail::{BundleAssetDep, BundleDetail, collection_appearances};
use crate::bundle::search::BundleSearchResult;
use crate::collections::{
	CollectionClientError, CollectionDownloadDataReport, CollectionDownloadProgress,
	CollectionMetadata, CollectionUpsertResult, CollectionUpsertStatus, CourseContents,
	CourseContentsChart, DownloadCollection, PackContents, PackContentsBundle, TableContents,
	TableContentsChart, TableContentsFolder, TableContentsLevel, plan_download_tasks,
	run_download_task,
};
use crate::download_manager::{
	DownloadListResult, DownloadManager, DownloadOverview, DownloadProgress, DownloadSnapshot,
};
use crate::maintenance::check::CorruptionReport;
use crate::remote::create_many_clients;
use crate::store::stats::StoreStats;
use crate::util::{BLOCK, decode_json, parse_iso8601};
use crate::{AssetData, DataId, bundle};

/// This is _the_ interface for speaking to backbeat.
///
/// This provides everything - querying, storing, server networking,
/// and so on.
///
/// Open the backbeat store with [`Backbeat::open`], and then
/// use whatever functions you want.
///
/// All methods are thread safe.
///
/// # On SQLite
///
/// The SQLite database is _part of the backbeat API_ and you
/// are fully permitted to read from it - it is a 100% stable interface.
///
/// You can use the [`Backbeat::sqlite_attach_command`] function to connect to the backbeat database
/// and execute arbitrary read-only queries.
#[derive(Clone)]
pub struct Backbeat {
	pub(crate) pool: SqlitePool,
	pub(crate) assets: AssetStore,
	pub(crate) config: Arc<RwLock<BackbeatConfig>>,
	pub(crate) config_dir: PathBuf,
	pub(crate) store_dir: PathBuf,
	pub(crate) servers: Arc<RwLock<Vec<BackbeatServerClient>>>,
	pub(crate) download_mgr: DownloadManager,
	pub(crate) general_http_client: Client,
}

/// Errors that can occur while opening or operating on a [`Backbeat`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
	#[error("database error: {0}")]
	Db(#[from] sqlx::Error),

	#[error("corrupt database: {0}")]
	Corrupt(String),

	#[error("migration error: {0}")]
	Migrate(String),

	#[error("I/O error: {0}")]
	Io(#[from] std::io::Error),

	#[error("config error: {0}")]
	Config(#[from] ConfigError),

	#[error("JSON error: {0}")]
	Json(#[from] serde_json::Error),

	#[error("header.json was invalid; {url} is not a valid Backbeat collection")]
	InvalidCollectionHeader {
		url: String,
		#[source]
		source: serde_json::Error,
	},

	#[error("collection client error: {0}")]
	CollectionClient(#[from] CollectionClientError),

	#[error("invalid .bb file: {0}")]
	Bundle(#[from] backbeat_core::BackbeatFileError),

	#[error("invalid .bbzip file: {0}")]
	BbZip(#[from] backbeat_core::BbZipReadError),

	#[error("could not write .bbzip file: {0}")]
	BbZipWrite(#[from] backbeat_core::BbZipWriteError),

	#[error("parse error: {0}")]
	Parse(String),

	#[error("not found: {0}")]
	NotFound(String),

	#[error("downloaded content does not match expected sha256")]
	HashMismatch,

	#[error("invalid precombined-assets archive: {0}")]
	PrecombinedAssets(#[from] backbeat_core::precombined_assets::PrecombinedAssetsError),

	#[error("downloaded bundle ID mismatch: expected {expected}, got {actual}")]
	BundleIdMismatch {
		expected: BundleId,
		actual: BundleId,
	},

	#[error("downloaded chart ID mismatch: expected {expected}, got {actual}")]
	ChartIdMismatch { expected: ChartId, actual: ChartId },

	#[error("failed to reach data server error: {0}")]
	Remote(#[from] backbeat_server_client::RemoteError),

	#[error("download task got aborted or panicked. This should never happen: {0}")]
	DownloadTaskFailed(String),
}

impl StoreError {
	pub(crate) fn shared(err: impl Into<Self>) -> Arc<Self> {
		Arc::new(err.into())
	}

	pub fn into_io_error(self) -> io::Error {
		match self {
			Self::Io(err) => err,
			Self::NotFound(message) => io::Error::new(io::ErrorKind::NotFound, message),
			err => io::Error::other(err),
		}
	}
}

pub type Result<T> = std::result::Result<T, StoreError>;

// this impl block is _the whole_ public api for the store.
impl Backbeat {
	/// This file is created next to the backbeat store to prevent macOS (and maybe linux) from trying
	/// to index the (really big) `assets/` dir
	const METADATA_NEVER_INDEX_FILE: &str = ".metadata_never_index";

	/// SQLite database filename inside the store directory.
	pub const DB_FILENAME: &str = "backbeat.db";

	/// Where backbeat stores large assets - these are files that charts make reference to, so backgrounds,
	/// audio files, etc.
	pub const ASSETS_DIR: &str = ".assets";

	/// Where backbeat keeps track of ongoing downloads.
	pub const DOWNLOADING_DIR: &str = ".downloading";

	/// Where backbeat saves logs.
	pub const LOGS_DIR: &str = "logs";

	/// Open the backbeat store. This will create it if it does not yet exist.
	pub fn open() -> Result<Self> {
		Self::open_with_overridden_config_dir(default_config_dir())
	}

	/// Open a backbeat store using this dir as the config dir.
	///
	/// This is likely not the function you want - normal backbeat usage should use `::open`,
	/// and this function is primarily useful for tests.
	#[doc(hidden)]
	pub fn open_with_overridden_config_dir(config_dir: impl AsRef<Path>) -> Result<Self> {
		let config_dir = config_dir.as_ref();

		let config = BackbeatConfig::load_with_overridden_dir(config_dir)?;

		let servers = create_many_clients(&config.servers);

		let store_dir = config.store.path.clone();
		crate::fs::create_dir_all(&store_dir)?;

		let marker = store_dir.join(Self::METADATA_NEVER_INDEX_FILE);
		if !marker.exists() {
			crate::fs::write(&marker, [])?;
		}

		let assets_dir = store_dir.join(Self::ASSETS_DIR);
		crate::fs::create_dir_all(&assets_dir)?;

		let downloads_dir = store_dir.join(Self::DOWNLOADING_DIR);
		crate::fs::create_dir_all(&downloads_dir)?;

		let logs_dir = store_dir.join(Self::LOGS_DIR);
		crate::fs::create_dir_all(&logs_dir)?;

		let db_path = store_dir.join(Self::DB_FILENAME);

		let opts = SqliteConnectOptions::new()
			.filename(&db_path)
			.create_if_missing(true)
			.page_size(8192)
			.auto_vacuum(SqliteAutoVacuum::Incremental)
			.journal_mode(SqliteJournalMode::Wal)
			.synchronous(SqliteSynchronous::Normal)
			.foreign_keys(true)
			.busy_timeout(Duration::from_secs(30))
			.pragma("cache_size", "-20000")
			.pragma("temp_store", "MEMORY")
			.pragma("mmap_size", "2147483648");

		let pool = BLOCK(
			SqlitePoolOptions::new()
				.max_connections(16)
				.connect_with(opts),
		)?;
		BLOCK(schema::apply(&pool))?;
		tamper_seal::validate(&pool)?;

		let downloads = DownloadManager::new(
			config.downloads.concurrency as usize,
			config.downloads.stream.0,
		);
		let general_http_client = Client::builder()
			.timeout(Duration::from_secs(15))
			.build()
			.map_err(CollectionClientError::Request)?;

		let store = Self {
			pool,
			assets: AssetStore::new(assets_dir),
			servers: Arc::new(RwLock::new(servers)),
			download_mgr: downloads,
			general_http_client,
			config: Arc::new(RwLock::new(config)),
			config_dir: config_dir.to_owned(),
			store_dir,
		};
		Ok(store)
	}

	/// Where the user has configured their store to be.
	///
	/// Note that you probably don't need to care about this - much less do anything with this information.
	pub fn store_dir(&self) -> &Path {
		&self.store_dir
	}

	/// The path to the logs dir. This is where backbeat logs get written to.
	pub fn logs_dir(&self) -> PathBuf {
		self.store_dir().join(Self::LOGS_DIR)
	}

	/// The path to the config dir. This contains a backbeat.toml file.
	pub fn config_dir(&self) -> &Path {
		&self.config_dir
	}

	/// Get a snapshot of the user's backbeat configuration.
	pub fn config(&self) -> BackbeatConfig {
		self.config.read().clone()
	}

	/// Add a data server to the config and live server list.
	pub fn server_add(&self, server: &ServerConfig) -> Result<()> {
		let url = url::Url::parse(&server.url).map_err(RemoteError::InvalidUrl)?;
		let mut updated = self.config.read().clone();

		if !updated.servers.iter().any(|configured| {
			url::Url::parse(&configured.url)
				.is_ok_and(|configured_url| configured_url.as_str() == url.as_str())
		}) {
			updated.servers.push(server.clone());
			updated.write_to_dir(&self.config_dir)?;
		}

		*self.servers.write() = create_many_clients(&updated.servers);
		*self.config.write() = updated;
		Ok(())
	}

	/// Remove a data server from the config and live server list.
	pub fn server_rm(&self, server: &ServerConfig) -> Result<()> {
		let url = url::Url::parse(&server.url).map_err(RemoteError::InvalidUrl)?;
		let mut updated = self.config.read().clone();
		let previous_len = updated.servers.len();

		updated.servers.retain(|configured| {
			url::Url::parse(&configured.url).map_or(true, |configured_url| {
				configured_url.as_str() != url.as_str()
			})
		});
		if updated.servers.len() != previous_len {
			updated.write_to_dir(&self.config_dir)?;
		}

		*self.servers.write() = create_many_clients(&updated.servers);
		*self.config.write() = updated;
		Ok(())
	}

	/// Does this store have absolutely zero data servers set up?
	///
	/// This is probably the weirdest public backbeat function, but it's useful
	/// to be able to quickly indicate "this user has literally no way of installing content".
	pub fn has_zero_data_servers(&self) -> bool {
		self.servers.read().is_empty()
	}

	/// Get the SQLite connection URL for attaching the store read-only.
	///
	/// This returns `file:///path/to/backbeat.db?mode=ro`.
	pub fn sqlite_connection_url(&self) -> String {
		let db_location = self.store_dir.join(Self::DB_FILENAME);
		let canon =
			std::fs::canonicalize(db_location).expect("backbeat.db was deleted while being used!");
		let mut url =
			url::Url::from_file_path(canon).expect("the Backbeat database path should be absolute");
		url.set_query(Some("mode=ro"));
		url.into()
	}

	/// Get a SQL statement that will attach the backbeat database read-only as `backbeat`.
	///
	/// Evaluate this string in sqlite to get a database `backbeat` attached to your database.
	///
	/// More specifically, this function returns `ATTACH DATABASE 'path_to_backbeat.db' AS backbeat;`.
	pub fn sqlite_attach_command(&self) -> String {
		let url = self.sqlite_connection_url();
		let escaped_url = url.replace('\'', "''");

		format!("ATTACH DATABASE '{escaped_url}' AS backbeat;")
	}

	/// Get a SQL statement that will detach 'backbeat' from your database.
	pub fn sqlite_detach_command(&self) -> String {
		// self, deliberate here in case of future
		"DETACH DATABASE backbeat;".into()
	}

	/// Add a backbeat file to the store. This does not queue up any downloads. Call [`Backbeat::bundle_download_assets`] after this to queue up downloads.
	pub fn import_bundle(&self, bb: &BackbeatFile) -> Result<BundleId> {
		BLOCK(self.import_bb_async(bb))
	}

	/// Import a `.bbzip` file into your store.
	pub fn import_bbzip(&self, path: impl AsRef<Path>) -> Result<()> {
		let f = fs_err::File::open(path.as_ref().to_path_buf())?;
		let mut bbzip = BbZipReader::new(f)?;

		for idx in 0..bbzip.len() {
			let Some(entry) = bbzip.get_entry(idx)? else {
				continue;
			};

			match entry {
				BbZipEntry::Bb(bb) => {
					self.import_bundle(&bb)?;
				}
				BbZipEntry::Asset(mut reader) => {
					// has to be buffered in memory
					// i can't think of a clever way
					// of doing this, maybe with tempfiles.

					let mut bytes = vec![];
					reader.read_to_end(&mut bytes)?;
					let bytes = bytes;

					let asset_id = AssetId(Sha256::checksum_bytes(&bytes));
					let size = bytes.len();

					if size as u64 >= self.config.read().store.inline.0 {
						self.assets.store(asset_id, &bytes)?;
						BLOCK(self.import_file_asset(asset_id, size as i64))?;
					} else {
						BLOCK(self.import_inline_asset(asset_id, size as i64, bytes))?;
					}
				}
			}
		}

		Ok(())
	}

	/// Take a path and add it to the store as an asset.
	///
	/// This API is honestly just provided to flesh out the API - it's very rare you'll need this,
	/// but if this didn't exist, you'd have to like, make a temp `.bbzip` or something.
	pub fn import_asset(&self, path: impl AsRef<Path>) -> Result<()> {
		let path = path.as_ref();
		let size = fs_err::metadata(path)?.len();

		let file = fs_err::File::open(path)?;

		let asset_id = AssetId(Sha256::checksum_data(file)?);

		// sucks that this is in two places
		if size >= self.config.read().store.inline.0 {
			self.assets.copy_into(asset_id, path)?;
			BLOCK(self.import_file_asset(asset_id, size as i64))?;
		} else {
			let data = fs_err::read(path)?;

			BLOCK(self.import_inline_asset(asset_id, size as i64, data))?;
		}

		Ok(())
	}

	/// Queue up downloads for all missing assets in this bundle. This bundle has to be installed
	/// in your store.
	///
	/// This is a useful function for repairing bundles with incomplete assets, or bundles that have
	/// just been manually added with [`Backbeat::import_bundle`].
	///
	// NOTE(zk): This function will _never_ use the precombined-assets optimisation. Ah well.
	pub fn bundle_download_assets(&self, bundle_id: BundleId) -> Result<()> {
		let bundle = self.get_bundle(bundle_id)?;

		for asset_id in bundle.assets.into_values() {
			if !self.has_asset(asset_id)? {
				self.download_mgr.queue_asset(self, asset_id);
			}
		}

		Ok(())
	}

	/// Search bundles by `query`, optionally restricted to file extensions.
	/// An empty extension list does not filter results.
	///
	/// Results are listed alphabetically if query is empty (that is, None or blank).
	pub fn search_bundles(
		&self,
		query: Option<&str>,
		offset: u64,
		limit: u32,
		extensions: &[&str],
	) -> Result<BundleSearchResult> {
		const CHART_SEARCH_MAX_LIMIT: u32 = 100;

		let limit = limit.clamp(1, CHART_SEARCH_MAX_LIMIT);
		let extensions = bundle::search::normalise_extensions(extensions);

		let Some(match_query) = query.and_then(bundle::search::build_fts_query) else {
			return bundle::search::list_bundles_internal(self, offset, limit, &extensions);
		};

		let limit = i64::from(limit);
		let offset = offset as i64;

		if !extensions.is_empty() {
			let extensions = serde_json::to_string(&extensions).expect("must ser");
			let rows = BLOCK(
				sqlx::query!(
					r#"
					SELECT
						b.id AS "id: backbeat_core::BundleId",
						b.description AS description,
						b.extension AS "extension?: String"
					FROM
						bundle_fts f
					JOIN
						bundle b ON b.id = f.bundle_id
					WHERE
						bundle_fts MATCH ?1
						AND b.extension IN (SELECT value FROM json_each(?2))
					ORDER BY
						rank
					LIMIT
						?3
					OFFSET
						?4
					"#,
					match_query,
					&extensions,
					limit,
					offset,
				)
				.fetch_all(&self.pool),
			)?;
			let total: i64 = BLOCK(
				sqlx::query_scalar!(
					"SELECT COUNT(*) FROM bundle_fts f \
					 JOIN bundle b ON b.id = f.bundle_id \
					 WHERE bundle_fts MATCH ?1 \
					 AND b.extension IN (SELECT value FROM json_each(?2))",
					match_query,
					&extensions,
				)
				.fetch_one(&self.pool),
			)?;
			let charts: Vec<bundle::search::ChartEntry> = rows
				.into_iter()
				.map(|row| bundle::search::ChartEntry {
					bundle_id: row.id,
					description: row.description,
					extension: row.extension,
				})
				.collect();
			return Ok(BundleSearchResult {
				has_more: offset + (charts.len() as i64) < total,
				charts,
				total: total.max(0).cast_unsigned(),
			});
		}

		let rows = BLOCK(
			sqlx::query!(
				"SELECT b.id AS \"id: backbeat_core::BundleId\", b.description AS description, \
				 b.extension AS \"extension?: String\" \
				 FROM bundle_fts f \
				 JOIN bundle b ON b.id = f.bundle_id \
				 WHERE bundle_fts MATCH ?1 \
				 ORDER BY rank \
				 LIMIT ?2 OFFSET ?3",
				match_query,
				limit,
				offset,
			)
			.fetch_all(&self.pool),
		)?;
		let total: i64 = BLOCK(
			sqlx::query_scalar!(
				"SELECT COUNT(*) FROM bundle_fts WHERE bundle_fts MATCH ?1",
				match_query,
			)
			.fetch_one(&self.pool),
		)?;
		let charts: Vec<bundle::search::ChartEntry> = rows
			.into_iter()
			.map(|row| bundle::search::ChartEntry {
				bundle_id: row.id,
				description: row.description,
				extension: row.extension,
			})
			.collect();
		Ok(BundleSearchResult {
			has_more: offset + (charts.len() as i64) < total,
			charts,
			total: total.max(0).cast_unsigned(),
		})
	}

	/// Collect statistics about the store.
	pub fn stats(&self) -> Result<StoreStats> {
		let charts: i64 =
			BLOCK(sqlx::query_scalar!("SELECT COUNT(*) FROM bundle").fetch_one(&self.pool))?;
		let charts = charts.max(0).cast_unsigned();
		let collections = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					(SELECT COUNT(*) FROM difftable) AS "tables!: i64",
					(SELECT COUNT(*) FROM course) AS "courses!: i64",
					(SELECT COUNT(*) FROM pack) AS "packs!: i64"
				"#,
			)
			.fetch_one(&self.pool),
		)?;

		let asset_row = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					COUNT(*) as count,
					COALESCE(SUM(size), 0) as bytes
				FROM
					downloaded_asset
				"#,
			)
			.fetch_one(&self.pool),
		)?;
		let asset_count = asset_row.count.cast_unsigned();
		let asset_bytes = asset_row.bytes.cast_unsigned();

		let db_path = self.store_dir.join(Self::DB_FILENAME);
		let db_bytes = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

		Ok(StoreStats {
			charts,
			tables: collections.tables.max(0).cast_unsigned(),
			courses: collections.courses.max(0).cast_unsigned(),
			packs: collections.packs.max(0).cast_unsigned(),
			asset_count,
			asset_bytes,
			db_bytes,
		})
	}

	/// Check for store corruption.
	pub fn corruption_check(&self) -> Result<CorruptionReport> {
		Ok(self.corruption_scan()?.report)
	}

	/// Repair some store corruption.
	///
	/// - For all charts, re-inspect it, and save the new inspected metadata.
	/// - Delete corrupt or missing assets so they can be downloaded again.
	/// - Delete charts whose contents are corrupt or cannot be inspected.
	pub fn corruption_repair(&self) -> Result<()> {
		self.corruption_repair_inner()
	}

	/// Delete all assets that are not referenced by a chart or collection.
	///
	/// Returns the amount of things that were removed, or will be removed if `dry_run` is true.
	pub fn asset_prune(&self, dry_run: bool) -> Result<u64> {
		let count = BLOCK(
			sqlx::query_scalar!(
				r#"
				SELECT COUNT(*)
				FROM downloaded_asset
				WHERE NOT EXISTS (
					SELECT 1
					FROM asset_map
					JOIN bundle USING (combined_assets_id)
					WHERE asset_map.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM pack_asset
					WHERE pack_asset.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM course_asset
					WHERE course_asset.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM difftable_asset
					WHERE difftable_asset.sha256 = downloaded_asset.sha256
				)
				"#,
			)
			.fetch_one(&self.pool),
		)?;

		if dry_run || count == 0 {
			return Ok(count.max(0).cast_unsigned());
		}

		let removed = BLOCK(async {
			let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
			let removed = sqlx::query_scalar!(
				r#"
				DELETE FROM downloaded_asset
				WHERE NOT EXISTS (
					SELECT 1
					FROM asset_map
					JOIN bundle USING (combined_assets_id)
					WHERE asset_map.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM pack_asset
					WHERE pack_asset.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM course_asset
					WHERE course_asset.sha256 = downloaded_asset.sha256
				)
				AND NOT EXISTS (
					SELECT 1 FROM difftable_asset
					WHERE difftable_asset.sha256 = downloaded_asset.sha256
				)
				RETURNING sha256 AS "asset_id!: backbeat_core::AssetId"
				"#,
			)
			.fetch_all(&mut *tx)
			.await?;

			if !removed.is_empty() {
				Self::increment_refresh(&mut tx).await?;
			}
			tx.commit().await?;
			Ok::<_, StoreError>(removed)
		})?;

		for asset_id in &removed {
			let path = self.assets.asset_path(*asset_id);
			if path.is_file() {
				crate::fs::remove_file(path)?;
			}
		}

		Ok(removed.len() as u64)
	}

	/// Delete any assets that are stored on disk, but don't exist in the store.
	/// This also wipes files in `.downloading` that have not been touched in the past hour.
	/// Under normal circumstances, this should never happen.
	///
	/// Returns the amount of things that were removed, or will be removed if `dry_run` is true.
	pub fn disk_prune(&self, dry_run: bool) -> Result<u64> {
		let known: HashSet<AssetId> = BLOCK(
			sqlx::query_scalar!(
				"SELECT sha256 AS \"asset_id!: backbeat_core::AssetId\" FROM downloaded_asset"
			)
			.fetch_all(&self.pool),
		)?
		.into_iter()
		.collect();
		let assets_dir = self.store_dir.join(Self::ASSETS_DIR);
		let mut removed = Self::disk_prune_assets(&assets_dir, &assets_dir, &known, dry_run)?;

		let stale_before = std::time::SystemTime::now() - Duration::from_hours(1);
		let downloads_dir = self.store_dir.join(Self::DOWNLOADING_DIR);
		for entry in crate::fs::read_dir(downloads_dir)? {
			let entry = entry?;
			let metadata = entry.metadata()?;
			if !metadata.is_file() || metadata.modified()? >= stale_before {
				continue;
			}

			removed += 1;
			if !dry_run {
				crate::fs::remove_file(entry.path())?;
			}
		}

		Ok(removed)
	}

	/// Check whether an asset is present on your configured data servers.
	pub async fn server_has_asset(&self, asset_id: AssetId) -> Result<bool> {
		self.run(move |store| async move {
			if store.has_asset_async(asset_id).await? {
				return Ok(true);
			}

			if store.has_zero_data_servers() {
				return Err(RemoteError::NoServers.into());
			}

			match store
				.for_all_remotes(|remote| async move {
					match remote.has_asset(asset_id).await {
						Ok(true) => Ok(()),
						Ok(false) => Err(RemoteError::NotFound),
						Err(err) => Err(err),
					}
				})
				.await
			{
				Ok(()) => Ok(true),
				Err(RemoteError::NotFound) => Ok(false),
				Err(err) => Err(err.into()),
			}
		})
		.await
	}

	/// Check whether a chart is available on your configured data servers without downloading it.
	pub async fn server_has_chart(&self, chart_id: &ChartId) -> Result<bool> {
		let chart_id = chart_id.clone();
		self.run(move |store| async move {
			let chart_id = &chart_id;
			if store.has_chart_async(chart_id).await? {
				return Ok(true);
			}

			if store.has_zero_data_servers() {
				return Err(RemoteError::NoServers.into());
			}

			match store
				.for_all_remotes(|remote| {
					let chart_id = chart_id.clone();
					async move {
						match remote.has_chart(&chart_id).await {
							Ok(true) => Ok(()),
							Ok(false) => Err(RemoteError::NotFound),
							Err(err) => Err(err),
						}
					}
				})
				.await
			{
				Ok(()) => Ok(true),
				Err(RemoteError::NotFound) => Ok(false),
				Err(err) => Err(err.into()),
			}
		})
		.await
	}

	/// Check whether a bundle is available on your configured data servers without downloading it.
	pub async fn server_has_bundle(&self, bundle_id: BundleId) -> Result<bool> {
		self.run(move |store| async move {
			if store.has_bundle_async(bundle_id).await? {
				return Ok(true);
			}

			if store.has_zero_data_servers() {
				return Err(RemoteError::NoServers.into());
			}

			match store
				.for_all_remotes(|remote| async move {
					match remote.has_bundle(bundle_id).await {
						Ok(true) => Ok(()),
						Ok(false) => Err(RemoteError::NotFound),
						Err(err) => Err(err),
					}
				})
				.await
			{
				Ok(()) => Ok(true),
				Err(RemoteError::NotFound) => Ok(false),
				Err(err) => Err(err.into()),
			}
		})
		.await
	}

	/// Download an asset from your configured data servers and install it into the store.
	///
	/// No-op if the asset is already present locally. Concurrent calls for the
	/// same asset are coalesced onto a single network fetch by the download
	/// manager.
	pub async fn server_download_asset(
		&self,
		asset_id: AssetId,
	) -> std::result::Result<(), Arc<StoreError>> {
		self.download_mgr.install_asset(self, asset_id).await
	}

	/// Download a bundle from your configured data servers and install it.
	/// This will also download all of the assets referenced by this chart, too.
	///
	/// No-op if the asset is already present locally. Concurrent calls for the
	/// same bundle are coalesced onto a single network fetch by the download
	/// manager.
	pub async fn server_download_bundle(
		&self,
		bundle_id: BundleId,
	) -> std::result::Result<(), Arc<StoreError>> {
		self.download_mgr.install_bundle(self, bundle_id).await
	}

	/// Download a chart from your configured data servers and install it.
	/// This will also download all of the assets referenced by this bundle, too.
	///
	/// No-op if the chart is already present locally. Concurrent calls for the
	/// same chart are coalesced onto a single network fetch by the download
	/// manager.
	pub async fn server_download_chart(
		&self,
		id: &ChartId,
	) -> std::result::Result<(), Arc<StoreError>> {
		self.download_mgr.install_chart(self, id).await
	}

	/// Get this pack from your database by URL.
	///
	/// This function does not do a network call, collections are just identified by URL.
	/// This fetches information from your store.
	pub fn get_pack(&self, url: &str) -> Result<PackContents> {
		let row = BLOCK(
			sqlx::query!(
			"SELECT name, updated, gamemode AS \"gamemode!: backbeat_core::ValidGamemodeIdentifier\", tags FROM pack WHERE url = ?1",
				url
			)
			.fetch_optional(&self.pool),
		)?
		.ok_or_else(|| StoreError::NotFound(url.to_owned()))?;
		let bundles = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					pe.bundle_id,
					pe.desc,
					pe.tags,
					EXISTS(SELECT 1 FROM bundle b WHERE b.id = pe.bundle_id) AS "installed!: bool"
				FROM pack_entry pe
				WHERE pe.url = ?1
				ORDER BY pe.entry
				"#,
				url
			)
			.fetch_all(&self.pool),
		)?
		.into_iter()
		.map(|bundle| {
			Ok(PackContentsBundle {
				id: bundle
					.bundle_id
					.parse()
					.map_err(|err| StoreError::Parse(format!("invalid bundle id: {err}")))?,
				desc: bundle.desc,
				tags: decode_json(&bundle.tags)?,
				installed: bundle.installed,
			})
		})
		.collect::<Result<Vec<_>>>()?;
		Ok(PackContents {
			name: row.name,
			gamemode: row.gamemode,
			updated: parse_iso8601(&row.updated)?,
			tags: decode_json(&row.tags)?,
			assets: get::load_collection_assets(&self.pool, CollectionKind::Pack, url)?,
			bundles,
		})
	}

	/// Get this table from your database by URL.
	///
	/// This function does not do a network call - this fetches information from your store.
	pub fn get_table(&self, url: &str) -> Result<TableContents> {
		let row = BLOCK(
			sqlx::query!(
			"SELECT name, symbol, updated, gamemode AS \"gamemode!: backbeat_core::ValidGamemodeIdentifier\", tags FROM difftable WHERE url = ?1",
				url
			)
			.fetch_optional(&self.pool),
		)?
		.ok_or_else(|| StoreError::NotFound(url.to_owned()))?;

		let levels = BLOCK(
			sqlx::query!(
				"SELECT level, level_order, tags FROM difftable_level WHERE url = ?1 ORDER BY level_order",
				url
			)
			.fetch_all(&self.pool),
		)?;

		let charts = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					tc.level,
							tc.id AS "id!: backbeat_core::ChartId",
					tc.desc,
					tc.tags,
					CASE
							WHEN tc.id GLOB 'sha256/*' THEN (
							SELECT b.id
							FROM bundle b
								WHERE b.chart_sha256 = substr(tc.id, 8)
							LIMIT 1
						)
						ELSE (
							SELECT b.id
							FROM chart_id ci
							JOIN bundle b ON b.chart_sha256 = ci.chart_sha256
								WHERE ci.id = tc.id
							LIMIT 1
						)
					END AS "bundle_id?: backbeat_core::BundleId"
				FROM difftable_chart tc
				JOIN difftable_level tl ON tl.url = tc.url AND tl.level = tc.level
				WHERE tc.url = ?1
				ORDER BY tl.level_order, tc.chart_order
				"#,
				url
			)
			.fetch_all(&self.pool),
		)?;

		let mut chart_map: HashMap<String, Vec<(TableChart, Option<BundleId>)>> = HashMap::new();
		for chart in charts {
			chart_map.entry(chart.level).or_default().push((
				TableChart {
					id: chart.id,
					desc: chart.desc,
					tags: decode_json(&chart.tags)?,
				},
				chart.bundle_id,
			));
		}

		let mut definition_levels = Vec::with_capacity(levels.len());
		let mut bundle_ids: Vec<Vec<Option<BundleId>>> = Vec::with_capacity(levels.len());
		for level in levels {
			let entries = chart_map.remove(&level.level).unwrap_or_default();
			let (charts, bundles) = entries.into_iter().unzip();
			definition_levels.push(TableLevel {
				level: level.level,
				tags: decode_json(&level.tags)?,
				charts,
			});
			bundle_ids.push(bundles);
		}

		let folder_definitions = BLOCK(
			sqlx::query!(
				"SELECT name, query, tags FROM difftable_folder WHERE url = ?1 ORDER BY folder_order",
				url
			)
			.fetch_all(&self.pool),
		)?
		.into_iter()
		.map(|folder| {
			Ok(TableFolder {
				name: folder.name,
				query: folder.query,
				tags: decode_json(&folder.tags)?,
			})
		})
		.collect::<Result<Vec<_>>>()?;

		let assets = get::load_collection_assets(&self.pool, CollectionKind::Table, url)?;
		let definition = Table {
			name: row.name,
			symbol: row.symbol,
			gamemode: row.gamemode,
			updated: parse_iso8601(&row.updated)?,
			tags: decode_json::<TableTags>(&row.tags)?,
			assets,
			levels: definition_levels,
			folders: folder_definitions,
		};

		let bundles_by_chart = definition
			.levels
			.iter()
			.zip(&bundle_ids)
			.flat_map(|(level, bundles)| level.charts.iter().zip(bundles))
			.map(|(chart, bundle_id)| (chart.id.clone(), *bundle_id))
			.collect::<HashMap<_, _>>();
		let folder_charts = definition
			.folders
			.iter()
			.map(|folder| {
				definition
					.evaluate_folder_expr(&folder.query)
					.into_iter()
					.map(|(_, chart)| TableContentsChart {
						id: chart.id.clone(),
						desc: chart.desc.clone(),
						tags: chart.tags.clone(),
						bundle_id: bundles_by_chart.get(&chart.id).copied().flatten(),
					})
					.collect::<Vec<_>>()
			})
			.collect::<Vec<_>>();

		let levels = definition
			.levels
			.into_iter()
			.zip(bundle_ids)
			.map(|(level, bundles)| TableContentsLevel {
				level: level.level,
				tags: level.tags,
				charts: level
					.charts
					.into_iter()
					.zip(bundles)
					.map(|(chart, bundle_id)| TableContentsChart {
						id: chart.id,
						desc: chart.desc,
						tags: chart.tags,
						bundle_id,
					})
					.collect(),
			})
			.collect();
		let folders = definition
			.folders
			.into_iter()
			.zip(folder_charts)
			.map(|(folder, charts)| TableContentsFolder {
				name: folder.name,
				query: folder.query,
				tags: folder.tags,
				charts,
			})
			.collect();

		Ok(TableContents {
			name: definition.name,
			symbol: definition.symbol,
			gamemode: definition.gamemode,
			updated: definition.updated,
			tags: definition.tags,
			assets: definition.assets,
			levels,
			folders,
		})
	}

	/// Get this course from your database by URL.
	///
	/// This function does not do a network call - this fetches information from your store.
	pub fn get_course(&self, url: &str) -> Result<CourseContents> {
		let row = BLOCK(
			sqlx::query!(
			"SELECT name, updated, gamemode AS \"gamemode!: backbeat_core::ValidGamemodeIdentifier\", tags FROM course WHERE url = ?1",
				url
			)
			.fetch_optional(&self.pool),
		)?
		.ok_or_else(|| StoreError::NotFound(url.to_owned()))?;
		let charts = BLOCK(
			sqlx::query!(
				r#"
				SELECT
							cc.id AS "id!: backbeat_core::ChartId",
					cc.desc,
					cc.tags,
					CASE
							WHEN cc.id GLOB 'sha256/*' THEN (
							SELECT b.id
							FROM bundle b
								WHERE b.chart_sha256 = substr(cc.id, 8)
							LIMIT 1
						)
						ELSE (
							SELECT b.id
							FROM chart_id ci
							JOIN bundle b ON b.chart_sha256 = ci.chart_sha256
								WHERE ci.id = cc.id
							LIMIT 1
						)
					END AS "bundle_id?: backbeat_core::BundleId"
				FROM course_chart cc
				WHERE cc.url = ?1
				ORDER BY cc.entry
				"#,
				url
			)
			.fetch_all(&self.pool),
		)?
		.into_iter()
		.map(|chart| {
			Ok(CourseContentsChart {
				id: chart.id,
				desc: chart.desc,
				tags: decode_json(&chart.tags)?,
				bundle_id: chart.bundle_id,
			})
		})
		.collect::<Result<Vec<_>>>()?;
		Ok(CourseContents {
			name: row.name,
			updated: parse_iso8601(&row.updated)?,
			gamemode: row.gamemode,
			tags: decode_json(&row.tags)?,
			assets: get::load_collection_assets(&self.pool, CollectionKind::Course, url)?,
			charts,
		})
	}

	/// Given a chart ID, get the chart file bytes.
	///
	/// To get the [`BackbeatFile`] for a chart, use [`Backbeat::get_chart`].
	pub fn get_chart_data(&self, chart_id: &ChartId) -> Result<Vec<u8>> {
		let chart_data = match &chart_id.alg {
			IdAlgorithm::Sha256 => {
				let sha256: Sha256 = chart_id.val.parse().map_err(|err| {
					StoreError::Parse(format!("invalid sha256 chart ID {chart_id}: {err}"))
				})?;
				BLOCK(
					sqlx::query!(
						r#"
						SELECT
							gzip_data AS "data!: Vec<u8>"
						FROM
							chart_data
						WHERE
							sha256 = ?1
						LIMIT 1
						"#,
						sha256,
					)
					.fetch_optional(&self.pool),
				)?
				.map(|e| e.data)
			}
			IdAlgorithm::Custom(_) => BLOCK(
				sqlx::query!(
					r#"
						SELECT
							chart_data.gzip_data AS "data!: Vec<u8>"
						FROM
							chart_data
						JOIN
							chart_id ON chart_id.chart_sha256 = chart_data.sha256
						WHERE
							chart_id.id = ?1
						LIMIT 1
						"#,
					chart_id.to_string(),
				)
				.fetch_optional(&self.pool),
			)?
			.map(|e| e.data),
		}
		.ok_or_else(|| StoreError::NotFound(chart_id.to_string()))?;

		Ok(ChartData::from_compressed(chart_data)?.decompress())
	}

	/// Get the backbeat file for this chart ID. If this request is ambiguous -- there are
	/// two bundles with the same chart_sha256 -- an unspecified bundle with the correct chart ID
	/// will be returned.
	pub fn get_chart(&self, chart_id: &ChartId) -> Result<BackbeatFile> {
		let chart_row: get::SomeChartInfo = match &chart_id.alg {
			IdAlgorithm::Sha256 => {
				let sha256: Sha256 = chart_id.val.parse().map_err(|err| {
					StoreError::Parse(format!("invalid sha256 chart ID {chart_id}: {err}"))
				})?;

				BLOCK(
					sqlx::query_as!(
						get::SomeChartInfo,
						r#"
						SELECT
							bundle.id AS "bundle_id: BundleId",
							bundle.description AS "desc: ChartDesc",
							bundle.filename AS "filename: backbeat_core::ChartFilename",
							chart_data.gzip_data AS "data!: Vec<u8>"
						FROM
							bundle
						JOIN
							chart_data ON chart_data.sha256 = bundle.chart_sha256
						WHERE
							bundle.chart_sha256 = ?1
						LIMIT 1
						"#,
						sha256,
					)
					.fetch_optional(&self.pool),
				)
			}
			IdAlgorithm::Custom(_) => {
				let id = chart_id.to_string();

				BLOCK(
					sqlx::query_as!(
						get::SomeChartInfo,
						r#"
						SELECT
							bundle.id AS "bundle_id: BundleId",
							bundle.description AS "desc: ChartDesc",
							bundle.filename AS "filename: backbeat_core::ChartFilename",
							chart_data.gzip_data AS "data!: Vec<u8>"
						FROM
							bundle
						JOIN
							chart_data ON chart_data.sha256 = bundle.chart_sha256
						JOIN
							chart_id ON chart_id.chart_sha256 = bundle.chart_sha256
						WHERE
							chart_id.id = ?1
						LIMIT 1
						"#,
						id,
					)
					.fetch_optional(&self.pool),
				)
			}
		}?
		.ok_or_else(|| StoreError::NotFound(chart_id.to_string()))?;

		get::assemble_bundle(self, chart_row)
	}

	/// Get a bundle by its bundle ID.
	pub fn get_bundle(&self, bundle_id: BundleId) -> Result<BackbeatFile> {
		let bundle_id_str = bundle_id.to_string();

		let some_chart_info = BLOCK(
			sqlx::query_as!(
				get::SomeChartInfo,
				r#"
				SELECT
					bundle.id AS "bundle_id: BundleId",
					bundle.description AS "desc: ChartDesc",
					bundle.filename AS "filename: backbeat_core::ChartFilename",
					chart_data.gzip_data AS "data!: Vec<u8>"
				FROM
					bundle
				JOIN
					chart_data ON chart_data.sha256 = bundle.chart_sha256
				WHERE
					bundle.id = ?1
				LIMIT 1"#,
				bundle_id_str,
			)
			.fetch_optional(&self.pool),
		)?
		.ok_or_else(|| StoreError::NotFound(bundle_id_str.clone()))?;

		get::assemble_bundle(self, some_chart_info)
	}

	/// Export a chart and all its assets to a folder or `.bbzip` file.
	pub fn export_chart(
		&self,
		chart_id: &ChartId,
		output: impl AsRef<Path>,
		bbzip: bool,
	) -> Result<()> {
		let bb = self.get_chart(chart_id)?;
		export::write(self, &bb, output.as_ref(), bbzip)
	}

	/// Export a bundle and all its assets to a folder or `.bbzip` file.
	pub fn export_bundle(
		&self,
		bundle_id: BundleId,
		output: impl AsRef<Path>,
		bbzip: bool,
	) -> Result<()> {
		let bb = self.get_bundle(bundle_id)?;
		export::write(self, &bb, output.as_ref(), bbzip)
	}

	/// Resolve asset data for a bundle + path.
	///
	/// This is like doing [`BackbeatFile::resolve_path`], but easy.
	pub fn resolve_asset(&self, bundle_id: BundleId, filepath: &str) -> Result<AssetData> {
		let asset = BLOCK(
			sqlx::query_scalar!(
				r#"
			SELECT
				asset_map.sha256 AS "sha256: backbeat_core::AssetId"
			FROM
				bundle
			JOIN
				asset_map ON bundle.combined_assets_id = asset_map.combined_assets_id
			WHERE
				bundle.id = ?1
				AND LOWER(path) = LOWER(?2)
			"#,
				bundle_id,
				filepath
			)
			.fetch_optional(&self.pool),
		)?;

		let asset = asset.ok_or_else(|| StoreError::NotFound(format!("No asset {filepath}")))?;

		self.get_asset(asset)
	}

	/// Given an asset ID, get the actual data for this asset.
	///
	/// This returns [`AssetData`], which either contains the entire file already in a Vec (if small)
	/// or a reader to get the file bytes off disk. Take a look at that type for more functionality and
	/// how to work with the bytes.
	pub fn get_asset(&self, asset: AssetId) -> Result<AssetData> {
		let fs_path = self.assets.asset_path(asset);
		if fs_path.is_file() {
			return Ok(AssetData::File(fs_path));
		}

		// Fall back to SQLite for small inline assets.
		let row = BLOCK(
			sqlx::query!(
				r#"SELECT inline_data AS "data!" FROM downloaded_asset WHERE sha256 = ?1 AND inline_data IS NOT NULL"#,
				asset,
			)
			.fetch_optional(&self.pool),
		)?;

		match row {
			Some(row) => Ok(AssetData::Bytes(row.data)),
			None => Err(StoreError::NotFound(format!("Could not find {asset}"))),
		}
	}

	/// Return `true` if this piece of data is installed.
	pub fn has_data(&self, data_id: &DataId) -> Result<bool> {
		match data_id {
			DataId::Chart(x) => self.has_chart(x),
			DataId::Bundle(x) => self.has_bundle(*x),
			DataId::Asset(x) => self.has_asset(*x),
		}
	}

	/// Return `true` if this asset is installed.
	pub fn has_asset(&self, asset_id: AssetId) -> Result<bool> {
		BLOCK(self.has_asset_async(asset_id))
	}

	/// Return `true` if a chart with this ID is installed.
	pub fn has_chart(&self, chart_id: &ChartId) -> Result<bool> {
		BLOCK(self.has_chart_async(chart_id))
	}

	/// Return `true` if this bundle is installed.
	pub fn has_bundle(&self, bundle_id: BundleId) -> Result<bool> {
		BLOCK(self.has_bundle_async(bundle_id))
	}

	/// Return `true` if a collection with this url is installed.
	pub fn has_collection(&self, url: &str) -> Result<bool> {
		Ok(BLOCK(
			sqlx::query_scalar!(
				r#"
				SELECT
					1 AS "exists!: i64"
				FROM
					pack
				WHERE
					url = ?1
				UNION ALL
				SELECT
					1 AS "exists!: i64"
				FROM
					difftable
				WHERE
					url = ?1
				UNION ALL
				SELECT
					1 AS "exists!: i64"
				FROM
					course
				WHERE
					url = ?1
				LIMIT 1
				"#,
				url,
			)
			.fetch_optional(&self.pool),
		)?
		.is_some())
	}

	/// Return `true` if a pack with this url is installed.
	pub fn has_pack(&self, url: &str) -> Result<bool> {
		Ok(BLOCK(
			sqlx::query_scalar!(
				r#"
				SELECT
					1 AS "exists!: i64"
				FROM
					pack
				WHERE
					url = ?1
				LIMIT 1
				"#,
				url,
			)
			.fetch_optional(&self.pool),
		)?
		.is_some())
	}

	/// Return `true` if a table with this url is installed.
	pub fn has_table(&self, url: &str) -> Result<bool> {
		Ok(BLOCK(
			sqlx::query_scalar!(
				r#"
				SELECT
					1 AS "exists!: i64"
				FROM
					difftable
				WHERE
					url = ?1
				LIMIT 1
				"#,
				url,
			)
			.fetch_optional(&self.pool),
		)?
		.is_some())
	}

	/// Return `true` if a course with this url is installed.
	pub fn has_course(&self, url: &str) -> Result<bool> {
		Ok(BLOCK(
			sqlx::query_scalar!(
				r#"
				SELECT
					1 AS "exists!: i64"
				FROM
					course
				WHERE
					url = ?1
				LIMIT 1
				"#,
				url,
			)
			.fetch_optional(&self.pool),
		)?
		.is_some())
	}

	/// Get the progress of a download, if any.
	pub fn download_progress(&self, key: &DataId) -> Option<DownloadProgress> {
		self.download_mgr.progress(key)
	}

	/// Return whether the store changed since `last_rev`, along with its current revision.
	pub fn should_refresh(&self, last_rev: i64) -> Result<(bool, i64)> {
		let revision = BLOCK(
			sqlx::query_scalar!("SELECT revision AS \"revision!: i64\" FROM refresh WHERE id = 1")
				.fetch_one(&self.pool),
		)?;
		Ok((revision != last_rev, revision))
	}

	/// What's the current state of all downloads?
	pub fn download_all_progress(&self) -> Vec<DownloadSnapshot> {
		self.download_mgr.all_progress()
	}

	/// Get an overview of current download stats.
	pub fn download_overview(&self) -> DownloadOverview {
		self.download_mgr.overview()
	}

	/// Paginated download list for the GUI.
	pub fn download_list(&self, offset: u64, limit: u32) -> DownloadListResult {
		self.download_mgr.list_progress(offset, limit)
	}

	/// Paginated chart and bundle downloads for the Downloads view.
	pub fn collection_download_list(&self, offset: u64, limit: u32) -> DownloadListResult {
		self.download_mgr.list_collection_progress(offset, limit)
	}

	/// Remove every terminal download from the download manager.
	pub fn download_clear_finished(&self) -> usize {
		self.download_mgr.clear_finished()
	}

	/// Cancel an in-flight download.
	pub fn download_cancel(&self, data_id: DataId) -> bool {
		self.download_mgr.cancel_download(data_id)
	}

	/// Cancel an in-flight asset download.
	pub fn download_asset_cancel(&self, asset_id: AssetId) -> bool {
		self.download_mgr.cancel_download(asset_id)
	}

	/// Cancel an in-flight chart download.
	pub fn download_chart_cancel(&self, chart_id: &ChartId) -> bool {
		self.download_mgr.cancel_download(chart_id.to_owned())
	}

	/// Cancel an in-flight bundle download.
	pub fn download_bundle_cancel(&self, bundle_id: BundleId) -> bool {
		self.download_mgr.cancel_download(bundle_id)
	}

	/// List installed tables, optionally restricted to gamemodes.
	pub fn list_tables(
		&self,
		gamemodes: &[ValidGamemodeIdentifier],
	) -> Result<Vec<CollectionMetadata>> {
		self.list_collections(CollectionKind::Table, gamemodes)
	}

	/// List installed courses, optionally restricted to gamemodes.
	pub fn list_courses(
		&self,
		gamemodes: &[ValidGamemodeIdentifier],
	) -> Result<Vec<CollectionMetadata>> {
		self.list_collections(CollectionKind::Course, gamemodes)
	}

	/// List installed packs, optionally restricted to gamemodes.
	pub fn list_packs(
		&self,
		gamemodes: &[ValidGamemodeIdentifier],
	) -> Result<Vec<CollectionMetadata>> {
		self.list_collections(CollectionKind::Pack, gamemodes)
	}

	fn list_collections(
		&self,
		kind: CollectionKind,
		gamemodes: &[ValidGamemodeIdentifier],
	) -> Result<Vec<CollectionMetadata>> {
		struct CollectionMetadataRow {
			url: String,
			name: String,
			gamemode: ValidGamemodeIdentifier,
			updated: String,
			installed: i64,
			total: i64,
		}

		let include_all_gamemodes = gamemodes.is_empty();
		let gamemodes = serde_json::to_string(
			&gamemodes
				.iter()
				.map(ValidGamemodeIdentifier::as_str)
				.collect::<Vec<_>>(),
		)?;
		let rows = match kind {
			CollectionKind::Table => BLOCK(
				sqlx::query_as!(
					CollectionMetadataRow,
					r#"
					SELECT
						t.url,
						t.name,
						t.gamemode AS "gamemode!: backbeat_core::ValidGamemodeIdentifier",
						t.updated,
						CAST(COALESCE(SUM(
							CASE WHEN tc.url IS NOT NULL AND (
								CASE WHEN tc.id GLOB 'sha256/*' THEN EXISTS (
									SELECT 1 FROM bundle b WHERE b.chart_sha256 = substr(tc.id, 8)
								) ELSE EXISTS (
									SELECT 1
									FROM chart_id ci
									JOIN bundle b ON b.chart_sha256 = ci.chart_sha256
									WHERE ci.id = tc.id
								) END
							) THEN 1 ELSE 0 END
						), 0) AS INTEGER) AS "installed!: i64",
						CAST(COUNT(tc.url) AS INTEGER) AS "total!: i64"
					FROM difftable t
					LEFT JOIN difftable_chart tc ON tc.url = t.url
					WHERE ?1 OR t.gamemode IN (SELECT value FROM json_each(?2))
					GROUP BY t.url
					ORDER BY t.name, t.url
					"#,
					include_all_gamemodes,
					gamemodes,
				)
				.fetch_all(&self.pool),
			)?,
			CollectionKind::Course => BLOCK(
				sqlx::query_as!(
					CollectionMetadataRow,
					r#"
					SELECT
						c.url,
						c.name,
						c.gamemode AS "gamemode!: backbeat_core::ValidGamemodeIdentifier",
						c.updated,
						CAST(COALESCE(SUM(
							CASE WHEN cc.url IS NOT NULL AND (
								CASE WHEN cc.id GLOB 'sha256/*' THEN EXISTS (
									SELECT 1 FROM bundle b WHERE b.chart_sha256 = substr(cc.id, 8)
								) ELSE EXISTS (
									SELECT 1
									FROM chart_id ci
									JOIN bundle b ON b.chart_sha256 = ci.chart_sha256
									WHERE ci.id = cc.id
								) END
							) THEN 1 ELSE 0 END
						), 0) AS INTEGER) AS "installed!: i64",
						CAST(COUNT(cc.url) AS INTEGER) AS "total!: i64"
					FROM course c
					LEFT JOIN course_chart cc ON cc.url = c.url
					WHERE ?1 OR c.gamemode IN (SELECT value FROM json_each(?2))
					GROUP BY c.url
					ORDER BY c.name, c.url
					"#,
					include_all_gamemodes,
					gamemodes,
				)
				.fetch_all(&self.pool),
			)?,
			CollectionKind::Pack => BLOCK(
				sqlx::query_as!(
					CollectionMetadataRow,
					r#"
					SELECT
						p.url,
						p.name,
						p.gamemode AS "gamemode!: backbeat_core::ValidGamemodeIdentifier",
						p.updated,
						CAST(COUNT(b.id) AS INTEGER) AS "installed!: i64",
						CAST(COUNT(pe.url) AS INTEGER) AS "total!: i64"
					FROM pack p
					LEFT JOIN pack_entry pe ON pe.url = p.url
					LEFT JOIN bundle b ON b.id = pe.bundle_id
					WHERE ?1 OR p.gamemode IN (SELECT value FROM json_each(?2))
					GROUP BY p.url
					ORDER BY p.name, p.url
					"#,
					include_all_gamemodes,
					gamemodes,
				)
				.fetch_all(&self.pool),
			)?,
		};
		rows.into_iter()
			.map(|row| {
				Ok(CollectionMetadata {
					url: row.url,
					name: row.name,
					gamemode: row.gamemode,
					updated: parse_iso8601(&row.updated)?,
					installed: row.installed.max(0).cast_unsigned(),
					total: row.total.max(0).cast_unsigned(),
				})
			})
			.collect()
	}

	/// Fetch only the header for this collection. Doubles up as a way of checking
	/// whether a URL is a valid collection.
	pub async fn collection_fetch_header(&self, url: &str) -> Result<CollectionHeader> {
		let url = url.to_owned();
		self.run(move |store| async move {
			let url = &url;
			let header_url = format!("{url}/header.json");
			let bytes = store.http_fetch_bytes(&header_url).await?;
			serde_json::from_slice(&bytes).map_err(|source| StoreError::InvalidCollectionHeader {
				url: url.to_owned(),
				source,
			})
		})
		.await
	}

	/// Fetch and insert this collection to your backbeat store.
	///
	/// Updates it if already installed.
	///
	/// To install the contents _inside_ this collection, use [`Backbeat::collection_fetch_download_data`].
	pub async fn collection_fetch_upsert(&self, url: &str) -> Result<CollectionUpsertResult> {
		let url = url.to_owned();
		self.run(move |store| async move {
			let url = &url;
			let header = store.collection_fetch_header(url).await?;
			let body_url = header.where_to_look(url);
			let body = store.http_fetch_bytes(&body_url).await?;
			let kind = header.kind;

			match kind {
				CollectionKind::Table => {
					let table = Table::from_json(&body).map_err(StoreError::Io)?;
					if table.updated != header.timestamp {
						return Err(StoreError::Parse(
							"collection timestamp changed while downloading; please try again"
								.to_owned(),
						));
					}
					let status = store
						.collection_upsert_status(url, kind, table.updated)
						.await?;
					if matches!(status, CollectionUpsertStatus::TimestampUnchanged) {
						return Ok(CollectionUpsertResult { kind, status });
					}
					store.table_put(url, &table).await?;
					Ok(CollectionUpsertResult { kind, status })
				}
				CollectionKind::Course => {
					let course = Course::from_json(&body).map_err(StoreError::Io)?;
					if course.updated != header.timestamp {
						return Err(StoreError::Parse(
							"collection timestamp changed while downloading; please try again"
								.to_owned(),
						));
					}
					let status = store
						.collection_upsert_status(url, kind, course.updated)
						.await?;
					if matches!(status, CollectionUpsertStatus::TimestampUnchanged) {
						return Ok(CollectionUpsertResult { kind, status });
					}
					store.course_put(url, &course).await?;
					Ok(CollectionUpsertResult { kind, status })
				}
				CollectionKind::Pack => {
					let pack = Pack::from_json(&body).map_err(StoreError::Io)?;
					if pack.updated != header.timestamp {
						return Err(StoreError::Parse(
							"collection timestamp changed while downloading; please try again"
								.to_owned(),
						));
					}

					let status = store
						.collection_upsert_status(url, kind, pack.updated)
						.await?;

					if matches!(status, CollectionUpsertStatus::TimestampUnchanged) {
						return Ok(CollectionUpsertResult { kind, status });
					}

					store.pack_put(url, &pack).await?;
					Ok(CollectionUpsertResult { kind, status })
				}
			}
		})
		.await
	}

	/// Remove a collection from your store. Optionally, decide whether you want to remove its
	/// charts, too.
	///
	/// Charts will only be removed if there isn't another collection referencing the chart.
	pub fn collection_rm(&self, url: &str, remove_charts_too: bool) -> Result<CollectionKind> {
		let kind = self.collection_kind_for_url(url)?;
		let bundles = if remove_charts_too {
			match kind {
				CollectionKind::Table => self
					.get_table(url)?
					.charts()
					.filter_map(|(_, chart)| chart.bundle_id)
					.collect::<HashSet<_>>(),
				CollectionKind::Course => self
					.get_course(url)?
					.charts
					.into_iter()
					.filter_map(|chart| chart.bundle_id)
					.collect(),
				CollectionKind::Pack => self
					.get_pack(url)?
					.bundles
					.into_iter()
					.filter(|bundle| bundle.installed)
					.map(|bundle| bundle.id)
					.collect(),
			}
		} else {
			HashSet::new()
		};
		BLOCK(async {
			let mut tx = self.pool.begin().await?;
			let affected = match kind {
				CollectionKind::Table => {
					sqlx::query!("DELETE FROM difftable WHERE url = ?1", url)
						.execute(&mut *tx)
						.await?
				}
				CollectionKind::Course => {
					sqlx::query!("DELETE FROM course WHERE url = ?1", url)
						.execute(&mut *tx)
						.await?
				}
				CollectionKind::Pack => {
					sqlx::query!("DELETE FROM pack WHERE url = ?1", url)
						.execute(&mut *tx)
						.await?
				}
			};
			if affected.rows_affected() == 0 {
				return Err(StoreError::NotFound(url.to_owned()));
			}
			Self::increment_refresh(&mut tx).await?;
			tx.commit().await?;
			Ok(())
		})?;

		// this is needlessly slow, could be one fn.
		if remove_charts_too {
			for bundle_id in bundles {
				if self.bundle_detail(bundle_id)?.appearances.is_empty() {
					self.bundle_rm(bundle_id)?;
				}
			}
			self.asset_prune(false)?;
		}

		Ok(kind)
	}

	/// Download charts, bundles, and assets referenced by an installed collection.
	///
	/// The collection at `url` must already be stored locally.
	pub async fn collection_fetch_download_data(
		&self,
		url: &str,
		mut progress: impl FnMut(CollectionDownloadProgress),
	) -> Result<CollectionDownloadDataReport> {
		let url = url.to_owned();
		let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
		let download = self.run(move |store| async move {
			store
				.collection_fetch_download_data_inner(&url, move |update| {
					let _ = sender.send(update);
				})
				.await
		});
		let updates = async {
			while let Some(update) = receiver.recv().await {
				progress(update);
			}
		};
		let (result, ()) = futures::future::join(download, updates).await;
		result
	}

	async fn collection_fetch_download_data_inner(
		&self,
		url: &str,
		mut progress: impl FnMut(CollectionDownloadProgress),
	) -> Result<CollectionDownloadDataReport> {
		if !self.has_collection(url)? {
			let _ = self.collection_fetch_upsert(url).await?;
		}

		let collection = match self.collection_kind_for_url(url)? {
			CollectionKind::Table => DownloadCollection::Table(self.get_table(url)?),
			CollectionKind::Course => DownloadCollection::Course(self.get_course(url)?),
			CollectionKind::Pack => DownloadCollection::Pack(self.get_pack(url)?),
		};
		let tasks = plan_download_tasks(collection);
		let total = tasks.len() as u64;
		let mut report = CollectionDownloadDataReport::default();
		let concurrency = self.config.read().downloads.concurrency.max(1);
		let mut downloads = stream::iter(tasks)
			.map(|item| {
				let store = self.clone();
				async move {
					let mut task_report = CollectionDownloadDataReport::default();
					run_download_task(&store, item.clone(), &mut task_report).await?;
					Ok::<_, StoreError>((item, task_report))
				}
			})
			.buffer_unordered(concurrency as usize);
		let mut completed = 0;

		while let Some(result) = downloads.next().await {
			let (item, task_report) = result?;
			report.merge(task_report);
			completed += 1;
			progress(CollectionDownloadProgress {
				current: completed,
				total,
				item,
			});
		}

		Ok(report)
	}

	/// Get some information about this stored asset.
	pub fn asset_detail(&self, asset_id: AssetId) -> Result<AssetDetail> {
		let size: Option<i64> = BLOCK(
			sqlx::query_scalar!(
				"SELECT size FROM downloaded_asset WHERE sha256 = ?1",
				asset_id,
			)
			.fetch_optional(&self.pool),
		)?;

		let dependent_rows = BLOCK(
			sqlx::query!(
				"SELECT b.id AS bundle_id, b.description AS description, am.path AS path \
				 FROM asset_map am \
				 JOIN bundle b ON b.combined_assets_id = am.combined_assets_id \
				 WHERE am.sha256 = ?1 \
				 ORDER BY b.description COLLATE NOCASE, b.id",
				asset_id,
			)
			.fetch_all(&self.pool),
		)?;

		if size.is_none() && dependent_rows.is_empty() {
			return Err(StoreError::NotFound(format!("No such asset {asset_id}")));
		}

		let dependents = dependent_rows
			.into_iter()
			.map(|row| {
				let bundle_id: BundleId = row.bundle_id.parse().map_err(|_| {
					StoreError::Corrupt(format!("invalid bundle id: {:?}", row.bundle_id))
				})?;
				Ok(AssetDependent {
					bundle_id,
					description: row.description,
					path: row.path,
				})
			})
			.collect::<Result<Vec<_>>>()?;

		Ok(AssetDetail {
			id: asset_id,
			size: size.map(|s| s.max(0).cast_unsigned()),
			dependents,
		})
	}

	/// Fetch details for a bundle.
	pub fn bundle_detail(&self, bundle_id: BundleId) -> Result<BundleDetail> {
		let Some(bundle_row) = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					bundle.filename AS "filename: backbeat_core::ChartFilename",
					bundle.description AS description,
					bundle.chart_sha256 AS "chart_sha256: backbeat_core::Sha256",
					chart_data.uncompressed_size AS "uncompressed_size!: i64"
				FROM
					bundle
				JOIN
					chart_data ON chart_data.sha256 = bundle.chart_sha256
				WHERE
					bundle.id = ?1
				LIMIT
					1
				"#,
				bundle_id,
			)
			.fetch_optional(&self.pool),
		)?
		else {
			return Err(StoreError::NotFound(format!("No such bundle {bundle_id}")));
		};

		let chart_id_rows = BLOCK(
			sqlx::query!(
				"SELECT id AS \"chart_id!: backbeat_core::ChartId\" FROM chart_id WHERE chart_sha256 = ?1 ORDER BY id",
				bundle_row.chart_sha256,
			)
			.fetch_all(&self.pool),
		)?;
		let chart_ids = chart_id_rows.into_iter().map(|row| row.chart_id).collect();

		let dependency_rows = BLOCK(
			sqlx::query!(
				r#"
				SELECT
					asset_map.path AS "path: backbeat_core::AssetPath",
					asset_map.sha256 AS "id!: backbeat_core::AssetId",
					downloaded_asset.size AS "size?: i64"
				FROM
					asset_map
				LEFT JOIN
					downloaded_asset ON downloaded_asset.sha256 = asset_map.sha256
				WHERE
					asset_map.combined_assets_id = (
						SELECT combined_assets_id FROM bundle WHERE id = ?1
					)
				ORDER BY
					asset_map.path
				"#,
				bundle_id,
			)
			.fetch_all(&self.pool),
		)?;
		let assets = dependency_rows
			.into_iter()
			.map(|row| BundleAssetDep {
				path: row.path,
				asset_id: row.id,
				size: row.size.map(|s| s.max(0).cast_unsigned()),
			})
			.collect();

		let appearances = collection_appearances(self, bundle_id, bundle_row.chart_sha256)?;

		Ok(BundleDetail {
			bundle_id,
			filename: bundle_row.filename,
			description: bundle_row.description,
			chart_sha256: bundle_row.chart_sha256,
			uncompressed_size: bundle_row.uncompressed_size.max(0).cast_unsigned(),
			chart_ids,
			assets,
			appearances,
		})
	}

	/// Remove this bundle from your store.
	pub fn bundle_rm(&self, bundle_id: BundleId) -> Result<()> {
		BLOCK(async {
			let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
			let Some(bundle) = sqlx::query!(
				r#"
				SELECT
					chart_sha256 AS "chart_sha256!: backbeat_core::Sha256",
					combined_assets_id
				FROM
					bundle
				WHERE
					id = ?1
				"#,
				bundle_id,
			)
			.fetch_optional(&mut *tx)
			.await?
			else {
				return Err(StoreError::NotFound(format!("No such bundle {bundle_id}")));
			};

			sqlx::query!("DELETE FROM bundle WHERE id = ?1", bundle_id)
				.execute(&mut *tx)
				.await?;

			sqlx::query!(
				"DELETE FROM asset_map WHERE combined_assets_id = ?1 AND NOT EXISTS (SELECT 1 FROM bundle WHERE combined_assets_id = ?1)",
				bundle.combined_assets_id,
			)
			.execute(&mut *tx)
			.await?;

			sqlx::query!(
				"DELETE FROM chart_data WHERE sha256 = ?1 AND NOT EXISTS (SELECT 1 FROM bundle WHERE chart_sha256 = ?1)",
				bundle.chart_sha256,
			)
			.execute(&mut *tx)
			.await?;

			Self::increment_refresh(&mut tx).await?;
			tx.commit().await?;
			Ok(())
		})
	}
}

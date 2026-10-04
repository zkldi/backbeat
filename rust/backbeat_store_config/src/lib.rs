#![cfg_attr(test, allow(unreachable_pub, clippy::all, clippy::restriction))]
#![cfg_attr(test, allow(clippy::pedantic, clippy::nursery, clippy::cargo))]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]
//! `backbeat.toml` configuration file.
//!
//! ## Example
//!
//! ```toml
//! [store]
//! path = "/Users/me/.local/share/backbeat"
//! inline = "16Ki"
//!
//! [downloads]
//! concurrency = 32
//!
//! [[server]]
//! url = "https://data.backbeat.ac"
//! ```

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod byte_size;

pub use self::byte_size::{ByteSize, ParseByteSizeError};

pub const CONFIG_FOLDER_NAME: &str = "backbeat";
pub const CONFIG_FILENAME: &str = "backbeat.toml";
pub const MAX_DOWNLOAD_CONCURRENCY: u32 = 1_000;

/// Where backbeat will default to writing configuration to. This is intended to be a "fixed point"
/// for loading backbeat from, and - although you can override your STORE_DIR - you cannot override
/// the CONFIG_DIR. This allows games to link into backbeat by just doing `bkb_open` without needing
/// to know where _you've_ specifically put your config file.
///
/// | Platform | Value
/// | -------  | -------------------------------------------------------
/// | Linux    | `$XDG_CONFIG_HOME`/backbeat or `$HOME`/.config/backbeat
/// | macOS    | `$HOME`/.config/backbeat
/// | Windows  | `{FOLDERID_RoamingAppData}`/backbeat
pub fn default_config_dir() -> PathBuf {
	let dirs = directories::BaseDirs::new().unwrap_or_else(|| {
		panic!(
			"Unknown system! We have no idea where your config dir should go. This should never happen; we support Linux+MacOS+Windows."
		)
	});

	#[cfg(target_os = "macos")]
	{
		dirs.home_dir().join(".config").join(CONFIG_FOLDER_NAME)
	}

	#[cfg(not(target_os = "macos"))]
	{
		dirs.config_dir().join(CONFIG_FOLDER_NAME)
	}
}

/// The top-level `backbeat.toml` configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct BackbeatConfig {
	/// `[store]`: Where to store data, and other settings.
	#[serde(default)]
	pub store: StoreConfig,

	/// `[downloads]`: How to download from data servers.
	#[serde(default)]
	pub downloads: DownloadsConfig,

	/// `[[server]]`: Backbeat Data Servers to fetch data from.
	#[serde(default = "default_servers", rename = "server")]
	#[serde(
		deserialize_with = "deserialize_servers",
		serialize_with = "serialize_servers"
	)]
	pub servers: Vec<ServerConfig>,

	/// `[info]`. What to serve on `/backbeat/info`, this is used as
	/// part of the backbeat_server module if you wish to use that.
	#[serde(default)]
	#[serde(skip_serializing_if = "Option::is_none")]
	pub info: Option<BackbeatServerInfo>,
}

fn default_servers() -> Vec<ServerConfig> {
	vec![ServerConfig {
		url: "https://data.makiba.ac".into(),
	}]
}

impl BackbeatConfig {
	/// Loads the backbeat config this user has. If this is the first time this user
	/// is interacting with backbeat, the config file is created with default values.
	pub fn load() -> Result<Self, ConfigError> {
		Self::load_with_overridden_dir(&default_config_dir())
	}

	/// Load a backbeat config file from an overridden directory i.e. aim at a very specific dir.
	///
	/// Do not use this publically. It's for tests. Please do not be an asshole.
	///
	/// This is likely not what you want. Use [`Self::load`] instead.
	#[doc(hidden)]
	pub fn load_with_overridden_dir(config_dir: &Path) -> Result<Self, ConfigError> {
		let path = config_dir.join(CONFIG_FILENAME);
		if !path.is_file() {
			let default = Self::default();
			default.write_to_dir(config_dir)?;
			return Ok(default);
		}

		let contents = fs_err::read_to_string(path)?;
		let mut config: Self = toml::from_str(&contents)?;
		// silently clamp this to 1-1000
		config.downloads.concurrency = config
			.downloads
			.concurrency
			.clamp(1, MAX_DOWNLOAD_CONCURRENCY);

		Ok(config)
	}

	/// Write this config to `dir/backbeat.toml`, creating `dir` if needed.
	pub fn write_to_dir(&self, dir: &Path) -> Result<PathBuf, ConfigError> {
		assert!(
			!dir.ends_with(CONFIG_FILENAME),
			"Tried to write_to_dir with a path name, not a filename. Refusing to create a **FOLDER** called backbeat.toml"
		);

		fs_err::create_dir_all(dir)?;
		let path = dir.join(CONFIG_FILENAME);
		fs_err::write(&path, self.to_toml()?)?;
		Ok(path)
	}

	/// Serialize to a TOML string.
	pub fn to_toml(&self) -> Result<String, ConfigError> {
		Ok(toml::to_string_pretty(self)?)
	}
}

/// `[store]` configuration section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct StoreConfig {
	/// Location of the data directory: where `backbeat.db` and `.assets/` live
	#[serde(default = "default_store_dir")]
	pub path: PathBuf,

	/// Maximum size in bytes for assets stored inline in SQLite rather than in `.assets/`. Assets strictly larger than this
	/// threshold are always written to the filesystem.
	///
	/// Defaults to `"128Ki"`. Set to `"0"` to disable inline storage
	/// entirely.
	#[serde(default = "default_inline_threshold")]
	pub inline: ByteSize,
}

impl Default for StoreConfig {
	fn default() -> Self {
		Self {
			path: default_store_dir(),
			inline: default_inline_threshold(),
		}
	}
}

/// Returns the path to the user's data directory.
///
/// | Platform | Value                                     | Example                                           |
/// | -------- | ----------------------------------------- | ------------------------------------------------- |
/// | Linux    | `$XDG_DATA_HOME` or `$HOME`/.local/share  | /home/alice/.local/share/backbeat                 |
/// | macOS    | `$HOME`/.local/share                      | /Users/Alice/.local/share/backbeat                |
/// | Windows  | `{FOLDERID_LocalAppData}`                 | C:\Users\Alice\AppData\Local\backbeat             |
fn default_store_dir() -> PathBuf {
	let dirs = directories::BaseDirs::new().unwrap_or_else(|| {
		panic!(
			"Unknown system! We have no idea where your data dir should go. This should never happen; we support Linux+MacOS+Windows."
		)
	});

	#[cfg(target_os = "macos")]
	{
		dirs.home_dir()
			.join(".local")
			.join("share")
			.join(CONFIG_FOLDER_NAME)
	}

	#[cfg(target_os = "windows")]
	{
		dirs.data_local_dir().join(CONFIG_FOLDER_NAME)
	}

	#[cfg(not(any(target_os = "macos", target_os = "windows")))]
	{
		dirs.data_dir().join(CONFIG_FOLDER_NAME)
	}
}

const fn default_inline_threshold() -> ByteSize {
	ByteSize(128 * 1024) // 128KiB
}

/// `[downloads]` configuration section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DownloadsConfig {
	/// Maximum number of data server requests the download manager runs in parallel.
	///
	/// Defaults to 32.
	#[serde(default = "default_download_concurrency")]
	pub concurrency: u32,

	/// Maximum size of an asset before they get streamed to disk, instead of buffered.
	///
	/// Defaults to "32Mi".
	#[serde(default = "default_download_stream")]
	pub stream: ByteSize,
}

impl Default for DownloadsConfig {
	fn default() -> Self {
		Self {
			concurrency: default_download_concurrency(),
			stream: default_download_stream(),
		}
	}
}

const fn default_download_concurrency() -> u32 {
	32
}

const fn default_download_stream() -> ByteSize {
	ByteSize(32 * 1024 * 1024)
}

/// A server entry under `[[server]]`.
///
/// Pretty much just a URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ServerConfig {
	/// The server base url, e.g. `https://data.backbeat.ac`.
	pub url: String,
}

fn dedupe_servers(servers: &mut Vec<ServerConfig>) {
	let mut urls = HashSet::with_capacity(servers.len());
	servers.retain(|server| urls.insert(server.url.clone()));
}

fn deserialize_servers<'de, D>(deserializer: D) -> Result<Vec<ServerConfig>, D::Error>
where
	D: Deserializer<'de>,
{
	let mut servers = Vec::<ServerConfig>::deserialize(deserializer)?;
	dedupe_servers(&mut servers);
	Ok(servers)
}

fn serialize_servers<S>(servers: &[ServerConfig], serializer: S) -> Result<S::Ok, S::Error>
where
	S: Serializer,
{
	let mut servers = servers.to_vec();
	dedupe_servers(&mut servers);
	servers.serialize(serializer)
}

/// What to show on `/backbeat/info` when running this store as a data server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BackbeatServerInfo {
	/// Human-readable server display name advertised in `/backbeat/info`.
	pub name: String,

	/// Optional contact string advertised in `/backbeat/info`.
	#[serde(default)]
	#[serde(skip_serializing_if = "Option::is_none")]
	pub contact: Option<String>,
}

/// Errors that can occur when loading or parsing `backbeat.toml`.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
	#[error("failed to read config file: {0}")]
	Io(#[from] io::Error),

	#[error("failed to parse config file: {0}")]
	Toml(#[from] toml::de::Error),

	#[error("failed to serialize config: {0}")]
	TomlSer(#[from] toml::ser::Error),
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn defaults_when_empty() {
		let cfg: BackbeatConfig = toml::from_str("").unwrap();
		assert_eq!(cfg.store.inline, ByteSize(128 * 1024));
		assert_eq!(cfg.store.path, default_store_dir());
		assert_eq!(cfg.downloads.concurrency, 32);
		assert_eq!(cfg.downloads.stream, ByteSize(32 * 1024 * 1024));
		assert_eq!(cfg.servers.len(), 1);
		assert!(cfg.info.is_none());
	}

	#[test]
	fn default_config_dir_ends_in_backbeat() {
		let dir = default_config_dir();
		assert!(
			dir.ends_with("backbeat"),
			"default_config_dir() should end in `backbeat`, got {}",
			dir.display()
		);
	}

	#[test]
	fn default_store_path_ends_in_backbeat() {
		let path = default_store_dir();
		assert!(
			path.ends_with("backbeat"),
			"default_store_path() should end in `backbeat`, got {}",
			path.display()
		);
	}

	#[cfg(target_os = "macos")]
	#[test]
	fn macos_defaults_separate_config_and_data() {
		let home = directories::BaseDirs::new().unwrap().home_dir().to_owned();
		assert_eq!(default_config_dir(), home.join(".config/backbeat"));
		assert_eq!(default_store_dir(), home.join(".local/share/backbeat"));
	}

	#[cfg(target_os = "windows")]
	#[test]
	fn windows_defaults_separate_roaming_config_and_local_data() {
		let dirs = directories::BaseDirs::new().unwrap();
		assert_eq!(
			default_config_dir(),
			dirs.config_dir().join(CONFIG_FOLDER_NAME)
		);
		assert_eq!(
			default_store_dir(),
			dirs.data_local_dir().join(CONFIG_FOLDER_NAME)
		);
	}

	#[test]
	fn store_path_defaults_when_omitted() {
		let cfg: BackbeatConfig = toml::from_str("[store]\ninline = \"0\"\n").unwrap();
		assert_eq!(cfg.store.path, default_store_dir());
		assert_eq!(cfg.store.inline, ByteSize::ZERO);
	}

	#[test]
	fn store_path_roundtrips() {
		let cfg: BackbeatConfig = toml::from_str(
			r#"[store]
path = "/var/lib/backbeat"
inline = "8Ki"
"#,
		)
		.unwrap();
		assert_eq!(cfg.store.path, PathBuf::from("/var/lib/backbeat"));
		let s = cfg.to_toml().unwrap();
		let reparsed: BackbeatConfig = toml::from_str(&s).unwrap();
		assert_eq!(cfg, reparsed);
	}

	#[test]
	fn parses_full_example() {
		let toml = r#"
[store]
inline = "0"

[downloads]
concurrency = 4

[[server]]
url = "https://backbeat.example.com"

[info]
name = "Example Backbeat"
contact = "ops@example.com"
"#;
		let cfg: BackbeatConfig = toml::from_str(toml).unwrap();
		assert_eq!(cfg.store.inline, ByteSize::ZERO);
		assert_eq!(cfg.downloads.concurrency, 4);
		assert_eq!(cfg.servers.len(), 1);
		assert_eq!(cfg.servers[0].url, "https://backbeat.example.com");
		let info = cfg.info.as_ref().expect("info config");
		assert_eq!(info.name, "Example Backbeat");
		assert_eq!(info.contact.as_deref(), Some("ops@example.com"));
	}

	#[test]
	fn downloads_stream_parses() {
		let toml = r#"
[downloads]
stream = "64Mi"
"#;
		let cfg: BackbeatConfig = toml::from_str(toml).unwrap();
		assert_eq!(cfg.downloads.stream, ByteSize(64 * 1024 * 1024));
	}

	#[test]
	fn partial_config_fills_defaults() {
		let cfg: BackbeatConfig = toml::from_str(
			r#"[store]
inline = "8Ki""#,
		)
		.unwrap();
		assert_eq!(cfg.store.inline, ByteSize(8 * 1024));
		assert!(cfg.info.is_none());
		assert_eq!(cfg.servers.len(), 1);
	}

	#[test]
	fn rejects_integer_inline() {
		let err = toml::from_str::<BackbeatConfig>("[store]\ninline = 16384").unwrap_err();
		assert!(err.to_string().contains("must be strings"));
	}

	#[test]
	fn roundtrip_serialization() {
		let cfg = BackbeatConfig::default();
		let toml_str = cfg.to_toml().unwrap();
		let parsed: BackbeatConfig = toml::from_str(&toml_str).unwrap();
		assert_eq!(cfg, parsed);
	}

	#[test]
	fn empty_data_servers_roundtrip() {
		let cfg = BackbeatConfig {
			servers: Vec::new(),
			..BackbeatConfig::default()
		};
		let toml_str = cfg.to_toml().unwrap();
		assert!(toml_str.contains("server = []"), "{toml_str:?}");
		assert!(!toml_str.contains("virtual_folders"));

		let reparsed: BackbeatConfig = toml::from_str(&toml_str).unwrap();
		assert!(reparsed.servers.is_empty());
	}

	#[test]
	fn info_section_roundtrips() {
		let cfg = BackbeatConfig {
			info: Some(BackbeatServerInfo {
				name: "hi".into(),
				contact: Some("hi@example.com".into()),
			}),
			..BackbeatConfig::default()
		};
		let toml_str = cfg.to_toml().unwrap();
		assert!(toml_str.contains("server = []"));
		let reparsed: BackbeatConfig = toml::from_str(&toml_str).unwrap();
		assert_eq!(cfg, reparsed);
	}

	#[test]
	fn parses_multiple_data_servers() {
		let toml = r#"
[[server]]
url = "https://a.example.com"

[[server]]
url = "https://b.example.com"
"#;
		let cfg: BackbeatConfig = toml::from_str(toml).unwrap();
		assert_eq!(cfg.servers.len(), 2);
		assert_eq!(cfg.servers[0].url, "https://a.example.com");
		assert_eq!(cfg.servers[1].url, "https://b.example.com");
	}

	#[test]
	fn deduplicates_data_servers_by_url_when_parsing() {
		let toml = r#"
[[server]]
url = "https://a.example.com"

[[server]]
url = "https://b.example.com"

[[server]]
url = "https://a.example.com"
"#;
		let cfg: BackbeatConfig = toml::from_str(toml).unwrap();
		assert_eq!(
			cfg.servers,
			vec![
				ServerConfig {
					url: "https://a.example.com".into()
				},
				ServerConfig {
					url: "https://b.example.com".into()
				}
			]
		);
	}

	#[test]
	fn deduplicates_data_servers_by_url_when_serializing() {
		let cfg = BackbeatConfig {
			servers: vec![
				ServerConfig {
					url: "https://a.example.com".into(),
				},
				ServerConfig {
					url: "https://b.example.com".into(),
				},
				ServerConfig {
					url: "https://a.example.com".into(),
				},
			],
			..BackbeatConfig::default()
		};

		let toml = cfg.to_toml().unwrap();
		let reparsed: toml::Value = toml::from_str(&toml).unwrap();
		let servers = reparsed["server"].as_array().unwrap();

		assert_eq!(servers.len(), 2);
		assert_eq!(servers[0]["url"].as_str(), Some("https://a.example.com"));
		assert_eq!(servers[1]["url"].as_str(), Some("https://b.example.com"));
	}
}

use backbeat_core::{AssetId, BackbeatFile, BundleId, ChartId, CombinedAssetsId};
use backbeat_store_config::BackbeatServerInfo;
use futures::StreamExt as _;
use reqwest::{Client, Method};
use tokio::io::AsyncRead;
use url::Url;

use crate::ServerPaths;

/// Errors that can occur when communicating with a Backbeat Server.
#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
	/// No data servers are configured.
	#[error("no data servers are configured")]
	NoServers,

	/// The base URL string in the config could not be parsed.
	#[error("invalid URL: {0}")]
	InvalidUrl(#[from] url::ParseError),

	/// A network-level error from the underlying HTTP client.
	#[error("network error: {0}")]
	Network(#[from] reqwest::Error),

	/// The server returned a response body that could not be deserialized.
	#[error("json error: {0}")]
	Json(#[from] serde_json::Error),

	/// The requested resource was not found (HTTP 404).
	#[error("not found")]
	NotFound,

	/// The requested `idAlgorithm` is not recognised by the server (HTTP 400).
	#[error("unknown id algorithm")]
	UnknownAlgorithm,

	/// The in-flight download was cancelled via the download manager.
	#[error("download cancelled")]
	Cancelled,

	/// The server returned a 5xx error.
	#[error("server error {0}")]
	Server(u16),

	/// The server returned an unexpected non-2xx status code.
	#[error("unexpected status {0}")]
	UnexpectedStatus(u16),
}

pub(crate) type Result<T> = std::result::Result<T, RemoteError>;

/// HTTP client for one Backbeat Data Server.
#[derive(Clone)]
pub struct BackbeatServerClient {
	client: Client,
	base_url: Url,
}

/// Assets can be huge, so we can't just return Vec<u8>.
///
/// This is a streaming resolver.
pub struct RemoteAssetReturn {
	pub reader: Box<dyn AsyncRead + Unpin + Send + 'static>,
	pub content_length: Option<u64>,
}

/// Exactly the same as [`RemoteAssetReturn`], but semantically
/// for pre-combined assets, which are `.tar.gz` amalgamations
/// of bytes.
pub struct RemotePrecombinedAssetsReturn {
	pub reader: Box<dyn AsyncRead + Unpin + Send + 'static>,
	pub content_length: Option<u64>,
}

impl BackbeatServerClient {
	/// Create a new [`BackbeatServerClient`] aimed at this server.
	pub fn new(base_url: &Url) -> Self {
		Self {
			client: Client::builder()
				.user_agent(format!("Backbeat/{}", backbeat_core::VERSION))
				.build()
				.expect("build Backbeat server HTTP client"),
			base_url: base_url.to_owned(),
		}
	}

	/// Get the base_url for this server.
	pub fn base_url(&self) -> &Url {
		&self.base_url
	}

	/// Check whether this server is up, getting no additional information.
	pub async fn check_up(&self) -> Result<()> {
		let resp = self
			.request(Method::GET, ServerPaths::backbeat_up())
			.send()
			.await?;
		check_status(resp.status().as_u16())?;
		Ok(())
	}

	/// Get the server-reported name and contact information.
	///
	/// This doubles up as a way of checking whether a server is up or not.
	pub async fn get_info(&self) -> Result<BackbeatServerInfo> {
		let resp = self
			.request(Method::GET, ServerPaths::backbeat_info())
			.send()
			.await?;
		check_status(resp.status().as_u16())?;
		Ok(resp.json().await?)
	}

	/// Check whether this asset is present on this server without downloading it.
	pub async fn has_asset(&self, asset_id: AssetId) -> Result<bool> {
		let path = ServerPaths::asset(asset_id);
		let resp = self.request(Method::HEAD, &path).send().await?;
		match resp.status().as_u16() {
			200 => Ok(true),
			404 => Ok(false),
			code => Err(status_to_error(code)),
		}
	}

	/// Check whether this bundle is present on this server without downloading it.
	pub async fn has_bundle(&self, bundle_id: BundleId) -> Result<bool> {
		let path = ServerPaths::bundle(bundle_id);
		let resp = self.request(Method::HEAD, &path).send().await?;
		match resp.status().as_u16() {
			200 => Ok(true),
			404 => Ok(false),
			code => Err(status_to_error(code)),
		}
	}

	/// Check whether this chart is present on this server without downloading it.
	pub async fn has_chart(&self, chart_id: &ChartId) -> Result<bool> {
		let path = ServerPaths::chart(chart_id);

		let resp = self.request(Method::HEAD, &path).send().await?;
		match resp.status().as_u16() {
			200 => Ok(true),
			400 => Err(RemoteError::UnknownAlgorithm),
			404 => Ok(false),
			code => Err(status_to_error(code)),
		}
	}

	/// Download an asset, returning a streaming reader and `Content-Length` when known.
	pub async fn get_asset(&self, asset_id: AssetId) -> Result<RemoteAssetReturn> {
		let path = ServerPaths::asset(asset_id);
		let resp = self.request(Method::GET, &path).send().await?;
		check_status(resp.status().as_u16())?;

		let content_length = resp.content_length();
		let reader = tokio_util::io::StreamReader::new(
			resp.bytes_stream()
				.map(|r| r.map_err(std::io::Error::other)),
		);

		Ok(RemoteAssetReturn {
			reader: Box::new(reader),
			content_length,
		})
	}

	/// Download precombined-assets. If they're available.
	///
	/// This is a niche optimisation for BMS and other games that have lots and lots of little files.
	///
	/// If this fails with a NotFound error, implementations should continue to just fetching files one
	/// by one.
	pub async fn get_precombined_assets(
		&self,
		combined_assets_id: CombinedAssetsId,
	) -> Result<RemotePrecombinedAssetsReturn> {
		let path = ServerPaths::precombined_assets(combined_assets_id);
		let resp = self.request(Method::GET, &path).send().await?;
		check_status(resp.status().as_u16())?;

		let content_length = resp.content_length();
		let reader = tokio_util::io::StreamReader::new(
			resp.bytes_stream()
				.map(|result| result.map_err(std::io::Error::other)),
		);

		Ok(RemotePrecombinedAssetsReturn {
			reader: Box::new(reader),
			content_length,
		})
	}

	/// Fetch a bundle.
	pub async fn get_bundle(&self, bundle_id: BundleId) -> Result<BackbeatFile> {
		let path = ServerPaths::bundle(bundle_id);
		let resp = self.request(Method::GET, &path).send().await?;
		check_status(resp.status().as_u16())?;
		Ok(resp.json().await?)
	}

	/// Download a bundle using a chart ID.
	pub async fn get_chart(&self, chart_id: &ChartId) -> Result<BackbeatFile> {
		let path = ServerPaths::chart(chart_id);
		let resp = self.request(Method::GET, &path).send().await?;

		let status = resp.status().as_u16();
		if status == 400 {
			return Err(RemoteError::UnknownAlgorithm);
		}
		check_status(status)?;

		let file: BackbeatFile = resp.json().await?;

		Ok(file)
	}

	fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
		let base = self.base_url.as_str().trim_end_matches('/');
		let url = Url::parse(&format!("{base}{path}")).expect("BCS path must produce a valid URL");
		self.client.request(method, url)
	}
}

fn check_status(code: u16) -> Result<()> {
	if (200..300).contains(&code) {
		return Ok(());
	}
	Err(status_to_error(code))
}

const fn status_to_error(code: u16) -> RemoteError {
	match code {
		400 => RemoteError::UnknownAlgorithm,
		404 => RemoteError::NotFound,
		500..=599 => RemoteError::Server(code),
		_ => RemoteError::UnexpectedStatus(code),
	}
}

//! Parse and dispatch `backbeat://` deep links.
//!
//! Three custom urls are supported:
//!
//! - `backbeat://charts/<alg>/<id>` -> [`Backbeat::server_download_chart`]
//! - `backbeat://bundle/<id>`      -> [`Backbeat::server_download_bundle`]
//! - `backbeat://collection/<uri-encoded-url>` -> collection installation

use backbeat_core::{BundleId, ChartId, IdAlgorithm};
use backbeat_sdk::{Backbeat, DataId};
use percent_encoding::percent_decode_str;
use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, State};
use url::Url;

use crate::AppState;

/// The `backbeat://` scheme. Configured in `tauri.conf.json` and registered
/// at runtime on Windows/Linux via the deep-link plugin.
pub const SCHEME: &str = "backbeat";

/// A parsed deep-link action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepLinkAction {
	/// Install a chart by chart ID.
	Chart { chart_id: ChartId },
	/// Install a bundle by its ID.
	Bundle { bundle_id: BundleId },
	/// Install or update a collection document.
	Collection { url: String },
}

impl DeepLinkAction {
	/// `(kind, target)` for event payloads and logs.
	fn describe(&self) -> (DownloadEventKind, String) {
		match self {
			Self::Chart { chart_id } => (DownloadEventKind::Chart, chart_id.to_string()),
			Self::Bundle { bundle_id } => (DownloadEventKind::Bundle, bundle_id.to_string()),
			Self::Collection { url } => (DownloadEventKind::Collection, url.clone()),
		}
	}

	fn data_id(&self) -> Option<DataId> {
		match self {
			Self::Chart { chart_id } => Some(DataId::Chart(chart_id.clone())),
			Self::Bundle { bundle_id } => Some(DataId::Bundle(*bundle_id)),
			Self::Collection { .. } => None,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadEventKind {
	Chart,
	Bundle,
	Collection,
	Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadEventStatus {
	Started,
	Ok,
	Failed,
}

/// One update emitted to the frontend's download toast.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadEvent {
	/// `"chart"`, `"bundle"`, `"collection"`, or `"unknown"` for a malformed link.
	pub kind: DownloadEventKind,
	/// Stable identifier for matching updates and retries (bundle id, chart id, URL, …).
	pub target: String,
	/// Human-readable label for the toast when available (e.g. a bundle description).
	#[serde(skip_serializing_if = "Option::is_none")]
	pub label: Option<String>,
	/// `"started"`, `"ok"`, or `"failed"`.
	pub status: DownloadEventStatus,
	/// Set when `status == "failed"`.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub error: Option<String>,
}

struct DeepLinkEventState {
	frontend_ready: bool,
	pending: Vec<DownloadEvent>,
}

pub struct DeepLinkEvents(Mutex<DeepLinkEventState>);

impl DeepLinkEvents {
	pub fn new() -> Self {
		Self(Mutex::new(DeepLinkEventState {
			frontend_ready: false,
			pending: Vec::new(),
		}))
	}

	fn record(&self, event: DownloadEvent) {
		let Ok(mut state) = self.0.lock() else {
			tracing::warn!("deep-link event state lock poisoned");
			return;
		};
		if !state.frontend_ready {
			state.pending.push(event);
		}
	}

	fn take_pending(&self) -> Vec<DownloadEvent> {
		let Ok(mut state) = self.0.lock() else {
			tracing::warn!("deep-link event state lock poisoned");
			return Vec::new();
		};
		state.frontend_ready = true;
		std::mem::take(&mut state.pending)
	}
}

fn emit_download_event(app: &AppHandle, event: DownloadEvent) {
	app.state::<AppState>()
		.deep_link_events
		.record(event.clone());
	let _ = app.emit("download", event);
}

#[tauri::command]
pub fn take_pending_events(state: State<'_, AppState>) -> Vec<DownloadEvent> {
	state.deep_link_events.take_pending()
}

/// Parse a `backbeat://` URL into a [`DeepLinkAction`].
pub fn parse(url: &Url) -> Result<DeepLinkAction, String> {
	if url.scheme() != SCHEME {
		return Err(format!(
			"expected a `{SCHEME}://` URL, got {}",
			url.scheme()
		));
	}

	let mut segs = segments(url);
	match segs.next().map(str::to_ascii_lowercase).as_deref() {
		Some("charts") => {
			let algorithm = segs
				.next()
				.ok_or_else(|| "backbeat://charts/<alg>/<id>: missing algorithm".to_string())
				.and_then(|s| {
					s.parse::<IdAlgorithm>()
						.map_err(|err| format!("invalid algorithm `{s}`: {err}"))
				})?;

			let id = segs
				.next()
				.ok_or_else(|| "backbeat://charts/<alg>/<id>: missing id".to_string())?
				.to_string();

			if segs.next().is_some() {
				return Err("backbeat://charts/<alg>/<id>: too many path segments".to_string());
			}
			Ok(DeepLinkAction::Chart {
				chart_id: ChartId {
					alg: algorithm,
					val: id,
				},
			})
		}
		Some("bundle") => {
			let bundle_id = segs
				.next()
				.ok_or_else(|| "backbeat://bundle/<id>: missing bundle id".to_string())
				.and_then(|s| {
					s.parse::<BundleId>()
						.map_err(|_| format!("invalid bundle id `{s}` (expected `b-<sha256>`)"))
				})?;
			if segs.next().is_some() {
				return Err("backbeat://bundle/<id>: too many path segments".to_string());
			}
			Ok(DeepLinkAction::Bundle { bundle_id })
		}
		Some("collection") => {
			parse_collection_url(url).map(|url| DeepLinkAction::Collection { url })
		}
		Some(other) => Err(format!(
			"unknown deep-link kind `{other}` (expected chart, bundle, or collection)"
		)),
		None => Err("empty deep-link path (expected chart, bundle, or collection)".to_string()),
	}
}

/// Parse and handle a single URL argument (e.g. from `argv` or a string event).
pub fn handle_url_str(app: &AppHandle, raw: &str) {
	let url = match raw.parse::<Url>() {
		Ok(url) => url,
		Err(err) => {
			tracing::warn!(%raw, ?err, "deep-link argument is not a valid URL");
			emit_download_event(
				app,
				DownloadEvent {
					kind: DownloadEventKind::Unknown,
					target: raw.to_string(),
					label: None,
					status: DownloadEventStatus::Failed,
					error: Some(format!("invalid URL: {err}")),
				},
			);
			return;
		}
	};
	handle(app, &url);
}

/// Parse a [`Url`] and kick off the matching install, emitting `download`
/// events for the frontend toast. Network work is spawned on a tokio task so
/// the caller (a deep-link callback) never blocks on it.
pub fn handle(app: &AppHandle, url: &Url) {
	let action = match parse(url) {
		Ok(action) => action,
		Err(err) => {
			tracing::warn!(%url, ?err, "rejected deep link");
			emit_download_event(
				app,
				DownloadEvent {
					kind: DownloadEventKind::Unknown,
					target: url.to_string(),
					label: None,
					status: DownloadEventStatus::Failed,
					error: Some(err),
				},
			);
			return;
		}
	};

	let (kind, target) = action.describe();
	tracing::info!(?kind, %target, "handling deep link");

	let store = match app.state::<AppState>().store.get() {
		Ok(store) => store,
		Err(err) => {
			emit_download_event(
				app,
				DownloadEvent {
					kind,
					target,
					label: None,
					status: DownloadEventStatus::Failed,
					error: Some(err),
				},
			);
			return;
		}
	};

	let app = app.clone();
	tauri::async_runtime::spawn(async move {
		let label = match action.data_id() {
			Some(item) => crate::downloads::collection_description(&store, &item)
				.await
				.or_else(|| installed_description(&store, &action)),
			None => None,
		};
		emit_download_event(
			&app,
			DownloadEvent {
				kind,
				target: target.clone(),
				label: label.clone(),
				status: DownloadEventStatus::Started,
				error: None,
			},
		);

		let result = run_action(&app, &store, &action).await;
		let label = label.or_else(|| installed_description(&store, &action));
		let event = match result {
			Ok(()) => DownloadEvent {
				kind,
				target,
				label,
				status: DownloadEventStatus::Ok,
				error: None,
			},
			Err(err) => {
				tracing::warn!(?kind, %target, ?err, "deep-link install failed");
				DownloadEvent {
					kind,
					target,
					label,
					status: DownloadEventStatus::Failed,
					error: Some(err),
				}
			}
		};
		emit_download_event(&app, event);
	});
}

/// Scan `argv` for a `backbeat://` URL and dispatch it. Used by the
/// single-instance plugin callback (Windows/Linux warm launch) and as a
/// fallback for cold launch.
pub fn handle_argv(app: &AppHandle, argv: &[String]) {
	for arg in argv {
		if arg.starts_with(&format!("{SCHEME}://")) {
			handle_url_str(app, arg);
		}
	}
}

fn installed_description(store: &Backbeat, action: &DeepLinkAction) -> Option<String> {
	let description = match action {
		DeepLinkAction::Chart { chart_id } => store.get_chart(chart_id).ok()?.desc.to_string(),
		DeepLinkAction::Bundle { bundle_id } => store.bundle_detail(*bundle_id).ok()?.description,
		DeepLinkAction::Collection { .. } => return None,
	};
	let description = description.trim();
	(!description.is_empty()).then(|| description.to_string())
}

async fn run_action(
	app: &AppHandle,
	store: &Backbeat,
	action: &DeepLinkAction,
) -> Result<(), String> {
	match action {
		DeepLinkAction::Chart { chart_id } => store
			.server_download_chart(chart_id)
			.await
			.map_err(|err| err.to_string()),
		DeepLinkAction::Bundle { bundle_id } => store
			.server_download_bundle(*bundle_id)
			.await
			.map_err(|err| err.to_string()),
		DeepLinkAction::Collection { url } => {
			crate::collections::install_or_update_collection(app, store, url).await
		}
	}
}

fn parse_collection_url(link: &Url) -> Result<String, String> {
	if link.query().is_some() || link.fragment().is_some() {
		return Err(
			"backbeat://collection/<uri-encoded-url>: URL must be path-encoded".to_string(),
		);
	}

	let encoded = match link.host_str() {
		Some("collection") => link.path().strip_prefix('/'),
		None => link.path().strip_prefix("/collection/"),
		_ => None,
	}
	.filter(|encoded| !encoded.is_empty() && !encoded.contains('/'))
	.ok_or_else(|| {
		"backbeat://collection/<uri-encoded-url>: missing or unencoded collection URL".to_string()
	})?;

	percent_decode_str(encoded)
		.decode_utf8()
		.map(|url| url.into_owned())
		.map_err(|err| {
			format!("backbeat://collection/<uri-encoded-url>: invalid URL encoding: {err}")
		})
}

/// Iterator over the deep-link's "path" segments, combining the URL host
/// (which carries the first segment for `backbeat://charts/...`) with the
/// actual path segments. Empty segments are skipped.
fn segments(url: &Url) -> impl Iterator<Item = &str> {
	let host = url.host_str().filter(|h| !h.is_empty());
	let path = url.path_segments().into_iter().flatten();
	host.into_iter().chain(path).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn url(s: &str) -> Url {
		s.parse().unwrap()
	}

	#[test]
	fn parses_chart_link_with_host_form() {
		let action = parse(&url("backbeat://charts/md5/abc123")).unwrap();
		assert_eq!(
			action,
			DeepLinkAction::Chart {
				chart_id: ChartId {
					alg: IdAlgorithm::Custom(backbeat_core::CustomIdAlgorithm::new("md5").unwrap()),
					val: "abc123".to_string(),
				},
			}
		);
	}

	#[test]
	fn parses_chart_link_with_empty_host_form() {
		let action = parse(&url("backbeat:///charts/sha256/deadbeef")).unwrap();
		assert_eq!(
			action,
			DeepLinkAction::Chart {
				chart_id: ChartId {
					alg: IdAlgorithm::Sha256,
					val: "deadbeef".to_string(),
				},
			}
		);
	}

	#[test]
	fn parses_chart_link_with_unknown_valid_algorithm() {
		let action = parse(&url("backbeat://charts/future-algorithm/abc123")).unwrap();
		assert_eq!(
			action,
			DeepLinkAction::Chart {
				chart_id: "future-algorithm/abc123".parse().unwrap(),
			}
		);
	}

	#[test]
	fn rejects_chart_link_with_invalid_algorithm() {
		assert!(parse(&url("backbeat://charts/future--algorithm/abc123")).is_err());
	}

	#[test]
	fn parses_bundle_link() {
		let id = "b-87428fc522803d31065e7bce3cf03fe475096631e5e07bbd7a0fde60c4cf25c7";
		let action = parse(&url(&format!("backbeat://bundle/{id}"))).unwrap();
		assert_eq!(
			action,
			DeepLinkAction::Bundle {
				bundle_id: id.parse().unwrap()
			}
		);
	}

	#[test]
	fn parses_collection_link() {
		let action = parse(&url(
			"backbeat://collection/https%3A%2F%2Fcollections.example.com%2Ftables%2Ffoo%3Fv%3D1",
		))
		.unwrap();
		assert_eq!(
			action,
			DeepLinkAction::Collection {
				url: "https://collections.example.com/tables/foo?v=1".to_string(),
			}
		);
	}

	#[test]
	fn rejects_wrong_scheme() {
		assert!(parse(&url("https://chart/md5/abc")).is_err());
	}

	#[test]
	fn rejects_unknown_kind() {
		assert!(parse(&url("backbeat://nope/abc")).is_err());
	}

	#[test]
	fn rejects_too_many_segments() {
		assert!(parse(&url("backbeat://charts/abc/md5/extra")).is_err());
		assert!(parse(&url("backbeat://bundle/b-x/extra")).is_err());
		assert!(parse(&url("backbeat://collection/https://example.com/foo")).is_err());
	}

	#[test]
	fn rejects_missing_segments() {
		assert!(parse(&url("backbeat://charts")).is_err());
		assert!(parse(&url("backbeat://charts/abc")).is_err());
		assert!(parse(&url("backbeat://bundle")).is_err());
		assert!(parse(&url("backbeat://collection")).is_err());
	}

	#[test]
	fn queues_events_until_the_frontend_registers() {
		let events = DeepLinkEvents::new();
		let event = DownloadEvent {
			kind: DownloadEventKind::Bundle,
			target: "b-example".to_string(),
			label: None,
			status: DownloadEventStatus::Started,
			error: None,
		};

		events.record(event.clone());
		assert_eq!(events.take_pending(), vec![event.clone()]);

		events.record(event);
		assert!(events.take_pending().is_empty());
	}
}

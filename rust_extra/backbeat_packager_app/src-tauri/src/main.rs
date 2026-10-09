#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod packaging;

#[cfg(target_os = "linux")]
fn apply_appimage_webkit_workarounds() {
	// https://v2.tauri.app/develop/debug/linux-graphics/
	if std::env::var_os("APPIMAGE").is_some()
		&& std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
	{
		// SAFETY: this is called at the start of `main`, before Tauri, WebKitGTK,
		// or any application threads have been initialized.
		unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
	}
}

#[tauri::command]
async fn open_result_folder(path: String) -> Result<(), String> {
	tauri::async_runtime::spawn_blocking(move || {
		let path = std::path::Path::new(&path);
		let folder = if path.is_dir() {
			path
		} else {
			path.parent()
				.ok_or("Could not determine containing folder")?
		};
		if !folder.is_dir() {
			return Err("Containing folder no longer exists".to_string());
		}
		open::that_detached(folder).map_err(|error| error.to_string())
	})
	.await
	.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn discover(paths: Vec<String>) -> Result<packaging::Discovery, String> {
	tauri::async_runtime::spawn_blocking(move || packaging::discover(&paths))
		.await
		.map_err(|error| error.to_string())
}

#[derive(Default)]
struct AppState {
	store: Option<backbeat_sdk::Backbeat>,
	batch: packaging::BatchState,
}

#[tauri::command]
fn packaging_concurrency() -> usize {
	std::thread::available_parallelism().map_or(4, |n| n.get())
}

type SharedState = std::sync::Arc<std::sync::Mutex<AppState>>;

#[tauri::command]
async fn reset_batch(state: tauri::State<'_, SharedState>) -> Result<(), String> {
	let state = state.inner().clone();
	tauri::async_runtime::spawn_blocking(move || {
		let mut state = state.lock().map_err(|error| error.to_string())?;
		state.batch = packaging::BatchState::default();
		Ok(())
	})
	.await
	.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn package_chart(
	path: String,
	output_dir: String,
	state: tauri::State<'_, SharedState>,
) -> Result<packaging::Packaged, String> {
	let state = state.inner().clone();
	tauri::async_runtime::spawn_blocking(move || {
		let batch = state
			.lock()
			.map_err(|error| error.to_string())?
			.batch
			.clone();
		packaging::package_chart(&path, &output_dir, &batch)
	})
	.await
	.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn store_destination() -> Result<String, String> {
	tauri::async_runtime::spawn_blocking(|| {
		backbeat_store_config::BackbeatConfig::load()
			.map(|config| config.store.path.to_string_lossy().into_owned())
			.map_err(|error| error.to_string())
	})
	.await
	.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn import_chart(
	path: String,
	store_path: String,
	state: tauri::State<'_, SharedState>,
) -> Result<packaging::Packaged, String> {
	let state = state.inner().clone();
	tauri::async_runtime::spawn_blocking(move || {
		let (store, batch) = {
			let mut state = state.lock().map_err(|error| error.to_string())?;
			if state.store.is_none() {
				state.store =
					Some(backbeat_sdk::Backbeat::open().map_err(|error| error.to_string())?);
			}
			let store = state.store.as_ref().ok_or("Store could not be opened")?;
			if store.config().store.path != std::path::Path::new(&store_path) {
				return Err(
					"Store location changed. Restart the Packager before importing.".into(),
				);
			}
			(store.clone(), state.batch.clone())
		};
		packaging::import_chart(std::path::Path::new(&path), &store, &batch)
	})
	.await
	.map_err(|error| error.to_string())?
}

fn main() {
	#[cfg(target_os = "linux")]
	apply_appimage_webkit_workarounds();

	tauri::Builder::default()
		.plugin(tauri_plugin_dialog::init())
		.manage(SharedState::default())
		.invoke_handler(tauri::generate_handler![
			open_result_folder,
			packaging_concurrency,
			discover,
			reset_batch,
			package_chart,
			store_destination,
			import_chart
		])
		.run(tauri::generate_context!())
		.expect("failed to run zk's Backbeat Packager");
}

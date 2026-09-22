#![cfg_attr(test, allow(unreachable_pub, clippy::all, clippy::restriction))]
#![cfg_attr(test, allow(clippy::pedantic, clippy::nursery, clippy::cargo))]
// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unreachable_pub)]
#![doc = include_str!("../../README.md")]

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

fn main() {
	#[cfg(target_os = "linux")]
	apply_appimage_webkit_workarounds();

	backbeat_app::run();
}

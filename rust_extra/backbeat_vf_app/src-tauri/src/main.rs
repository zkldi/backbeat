#![cfg_attr(test, allow(unreachable_pub, clippy::all, clippy::restriction))]
#![cfg_attr(test, allow(clippy::pedantic, clippy::nursery, clippy::cargo))]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unreachable_pub)]

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

	if std::env::args().nth(1).as_deref() == Some("--mount-daemon") {
		std::process::exit(backbeat_vf_mountd::run_process());
	}
	backbeat_vf_app::run();
}

alias app-packager := pkg-gui

# Run zk's Backbeat Packager with the native desktop bridge.
pkg-gui:
	cd rust_extra/backbeat_packager_app && bun run tauri dev

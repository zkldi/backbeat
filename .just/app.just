# Run the Backbeat app interactively in dev mode.
app:
	#!/usr/bin/env bash
	set -euo pipefail

	if ! command -v bun >/dev/null 2>&1; then
		echo "bun is required: https://bun.sh" >&2
		exit 1
	fi

	cd rust/backbeat_app

	if [ ! -d node_modules ]; then
		bun install
	fi

	exec bun run tauri dev

app-build-macos:
	#!/usr/bin/env bash
	set -euo pipefail

	if [ "$(uname -s)" != "Darwin" ]; then
		echo "app-build produces a macOS app bundle/dmg and must be run on macOS" >&2
		exit 1
	fi

	if ! command -v bun >/dev/null 2>&1; then
		echo "bun is required: https://bun.sh" >&2
		exit 1
	fi

	cd rust/backbeat_app

	if [ ! -d node_modules ]; then
		bun install
	fi

	bun tauri build --target universal-apple-darwin --bundles app,dmg

# Build a debug GUI .app and install it to /Applications (macOS deep links).
install-app:
	#!/usr/bin/env bash
	set -euo pipefail

	if [ "$(uname -s)" != "Darwin" ]; then
		echo "install-gui builds a macOS app bundle and must be run on macOS" >&2
		exit 1
	fi

	if ! command -v bun >/dev/null 2>&1; then
		echo "bun is required: https://bun.sh" >&2
		exit 1
	fi

	cd rust/backbeat_app

	if [ ! -d node_modules ]; then
		bun install
	fi

	bun tauri build --debug --bundles app

	app_bundle="../../target/debug/bundle/macos/Backbeat.app"
	if [ ! -d "$app_bundle" ]; then
		echo "expected app bundle at $app_bundle" >&2
		exit 1
	fi

	cp -R "$app_bundle" /Applications/
	echo "Installed /Applications/Backbeat.app"

app-typecheck:
	#!/usr/bin/env bash
	set -euo pipefail

	if ! command -v bun >/dev/null 2>&1; then
		echo "bun is required: https://bun.sh" >&2
		exit 1
	fi

	cd rust/backbeat_app

	if [ ! -d node_modules ]; then
		bun install
	fi

	exec bun run typecheck

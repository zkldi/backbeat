# zk's Backbeat Packager

The `zk-backbeat-packager` desktop app for turning rhythm game charts into `.bbzip` archives, using
`backbeat_packager`. Built with Tauri 2 and Solid, alongside the other Backbeat apps.

## Use

1. Drop charts or folders, or click **Choose folders**. Folders are scanned
   recursively. Nothing is converted yet.
2. Choose **Save .bbzip files** and select a folder, or choose **Import to store**.
   The store path is shown below the buttons and uses the main app's configuration.
3. The **Results** tab lists each file's outcome, output path, errors, and missing
   assets. The latest 100 results are shown first; **Show earlier results** reveals more.

Archive mode writes one file per source, named after the first generated chart's
description. Names are sanitised for Windows and Linux and limited in length.
Multiple playable charts become separate `.bb` entries in that archive. Existing
files are preserved with numeric suffixes. Archives are published only after the
write finishes.

Store mode imports charts and local assets through `backbeat_sdk`, without writing
archives. Repeated imports are deduplicated by the store. Imports are not a batch
transaction: already imported assets or charts remain if a later operation fails.
Missing references are listed in Results for both modes.

Processing runs concurrently in the background, using the same CPU-based worker
limit as the CLI (four workers if the CPU count is unavailable). Workers share
asset hash and file lookup caches. **Stop after running files** finishes active
files and leaves unstarted files ready in the Package tab. Dropping a new folder
replaces the pending files. Drops while processing or scanning are ignored. Results
remain across batches until the app closes. Symlink directories are not traversed.

## Recognised chart formats

Extensions are case-insensitive. Recognition follows the packager's
`should_be_packaged` function; a recognised extension still needs valid chart content.

| Family | Extensions |
| --- | --- |
| StepMania | `.sm`, `.ssc` |
| Dance With Intensity | `.dwi` |
| BMS family | `.bms`, `.bme`, `.bml`, `.pms` |
| BMSON | `.bmson` |
| K-Shoot Mania / KSON | `.ksh`, `.kson` |

`.osu`, `.tja`, `.dtx`, and Clone Hero `.chart` files are not supported.
ZIP/OSZ archives are not extracted; unpack them first. `.bb` and `.bbzip` are
Backbeat output formats, not chart inputs for this app.

Keep charts with their original assets and directory layout. The existing packager
scans the chart folder for assets and resolves format-specific references, including
supported parent-directory references. Non-chart files encountered during folder
search are ignored as inputs, but may be included as assets. Existing `.bb` and
`.bbzip` files are excluded from assets to avoid packaging generated output again.
Missing referenced assets are reported as warnings; the archive is still saved and
may be incomplete. Other errors fail that chart and processing continues.

## Development

From the repository root:

```sh
bun install
just pkg-gui
```

Or run `bun run tauri dev` from `rust_extra/backbeat_packager_app`.
Requires the repository's Rust toolchain, Bun, and the same native Tauri dependencies
as the existing desktop apps. The frontend uses port 1422.

```sh
bun run --cwd rust_extra/backbeat_packager_app typecheck
bun run --cwd rust_extra/backbeat_packager_app lint
bun run --cwd rust_extra/backbeat_packager_app build
cargo test -p zk-backbeat-packager -p backbeat_packager
```

To build a desktop bundle, run `bun run tauri build` in this directory.
`bun run dev` alone previews the UI; native file dialogs and packaging require Tauri.

## CI packages

In GitHub Actions, select **Build zk's Backbeat Packager (Windows)** or
**Build zk's Backbeat Packager (Linux)**, then **Run workflow** on the desired ref.
These builds run manually, like the Backbeat app builds.

Download the completed run's `zk-backbeat-packager-<platform>-x86-64-<commit>`
artifact. Windows includes MSI and NSIS installers; Linux includes DEB, RPM, and
AppImage packages. Both include `SHA256SUMS` and retain artifacts for 14 days.

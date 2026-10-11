# Backbeat SDK

Backbeat is a standard for sharing and storing rhythm game charts in an efficient, easy, distributed manner.

This crate is the core API for developing with Backbeat.

[Docs](https://docs.rs/backbeat_sdk)

[More info](https://backbeat.ac/docs)

If you are a game client written in Rust, you can add this:

```sh
cargo add backbeat_sdk
```

and you are off to the races!

If you're not written in Rust, you can use the C SDK instead,
see the [backbeat_c_sdk](https://github.com/zkldi/backbeat/tree/main/rust/backbeat_c_sdk).

## Usage

Generally speaking, take a look at the [`Backbeat`] struct. It's the centerpiece of this API, and is all you really need.

```sh
cargo add backbeat_sdk backbeat_core
cargo add tokio --features macros,rt-multi-thread
```

Here's a quick tour:

```rust,no_run
use backbeat_core::ChartId;
use backbeat_sdk::{AssetData, Backbeat};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
	// You just open the store - don't worry about configuring anything or creating it.
	let store = Backbeat::open()?;

	// basic stats
	let stats = store.stats()?;
	println!("{} charts, {} assets", stats.charts, stats.asset_count);

	// lets download a chart off the internet by sha256
	//
	// This automatically uses whatever the user has configured
	// to be their data servers.
	let chart_id: ChartId = "sha256/4842b24fecaa8a53dbcdc4f05f6fc919a2a2089d762d5f93144a4e1a97a1248a".parse()?;
	store.server_download_chart(&chart_id).await?;

	// get the downloaded bundle from your store
	let bundle = store.get_chart(&chart_id)?;
	let bundle_id = bundle.bundle_id();

	println!("chart has {} assets", bundle.assets.len());

	// Do asset lookups. When this chart asks for "You Suffer.mp3", call this function
	// to figure out what bytes you should use!
	let asset_data = store.resolve_asset(bundle_id, "You Suffer.mp3")?;

	// small assets are inlined into memory.
	//
	// Your game will need to handle both!
	match asset_data {
		AssetData::Bytes(b) => {
			println!("You Suffer.mp3 is an inlined file with {} bytes", b.len());
		},
		AssetData::File(path) => {
			let b = std::fs::read(path)?;
			println!("You Suffer.mp3 is stored on-disk, and is {} bytes", b.len());
		}
	}

	Ok(())
}
```

## Database API

The `backbeat.db` file is considered a stable part of the API for backbeat. You are allowed to do any read-only query from it.

Writing directly to the backbeat database is explicitly illegal. Do not do this, for any reason. You have to go very far out of your way to get a writable connection to the DB, so please don't.

The database API is as follows, you get read-only access to exactly these tables:

```sql
-- Backbeat will increment `refresh.revision` this whenever data has changed.
-- The value wraps around at 9_000_000_000_000_000_000, but that's fine.
--
-- This is how you listen for store changes. It's the simplest thing,
-- but it works so well, no process state or anything like that.
CREATE TABLE "refresh" (
	-- pointless column just to ensure we have one row of this
	id INTEGER PRIMARY KEY CHECK (id = 1),

	-- backbeat increments this when content is installed/removed/whatever.
	-- TL;DR this is the "should refresh" value.
	revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;
INSERT INTO refresh(id, revision) VALUES (1, 0);

-- Chart contents stored by sha256.
CREATE TABLE "chart_data" (
	sha256 TEXT PRIMARY KEY NOT NULL,
	-- The actual chart bytes, **gzip-compressed** for storage reasons.
	gzip_data BLOB NOT NULL,
	uncompressed_size INTEGER NOT NULL
) STRICT;

-- Extra chartIDs associated with a chart.
--
-- Backbeat enforces that `sha256` is calculated for every chart,
-- but also calculates some additional chart ID algorithms for
-- certain filetypes that are useful for external reasons.
--
-- The list of chart IDs supported by backbeat is baked into core.
-- To add support for a new one, update `backbeat_core`. At the moment,
-- only `md5` is supported as an extra algorithm, however, I anticipate there
-- to be more externally useful ones in the future.
--
-- When I was first designing this feature, we had support for "ksm-ir-hash",
-- "etterna-chartkey" and "groovestats-v3" hashes. However, all of them turned
-- out to be either unjustifiably buggy or practically unimplementable outside
-- of the games themselves. If you want to add a custom ID algorithm to backbeat
-- I think that's _awesome_ and I would love to merge it, but it has to be well
-- specified, and once merged, _cannot ever be patched again_.
CREATE TABLE "chart_id" (
	chart_sha256 TEXT NOT NULL REFERENCES chart_data(sha256) ON DELETE CASCADE,
	-- A string like "md5/2b00042f7481c7b056c4b410d28f33cf".
	id TEXT NOT NULL,

	PRIMARY KEY (chart_sha256, id)
) STRICT;
CREATE INDEX IF NOT EXISTS chart_id_id ON chart_id(id);
CREATE INDEX IF NOT EXISTS chart_id_sha256 ON chart_id(chart_sha256);

-- Assets are stored grouped up by "asset_map". This reduces storage costs
-- for say, many charts for the same bms chart. Without this layout, each new
-- chart would add N more dependencies, and it just doesn't scale.
CREATE TABLE "asset_map" (
	combined_assets_id TEXT NOT NULL,
	path TEXT NOT NULL,
	sha256 TEXT NOT NULL,
	PRIMARY KEY (combined_assets_id, path)
) STRICT;
CREATE INDEX IF NOT EXISTS asset_map_sha256 ON asset_map(sha256);
CREATE INDEX IF NOT EXISTS asset_map_combined_assets_id ON asset_map(combined_assets_id);

-- The guts of backbeat. These are all of the bundles you have installed.
CREATE TABLE "bundle" (
	id TEXT PRIMARY KEY NOT NULL,
	chart_sha256 TEXT NOT NULL REFERENCES chart_data(sha256),
	filename TEXT NOT NULL,
	-- This is everything after the last "." in the filename field above.
	-- This is pre-stored for you so that you get a fast indexed lookup
	-- on this extremely common operation. If you need finer filename
	-- lookups, use the `filename` field.
	extension TEXT COLLATE NOCASE,

	combined_assets_id TEXT NOT NULL,

	-- This is the only "mandatory" inspection field. Every
	-- chart should be able to be "described" with a human-friendly
	-- string. Usually in the form "artist - title [charter's diffname]".
	description TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS bundle_chart_sha256 ON bundle(chart_sha256);
CREATE INDEX IF NOT EXISTS bundle_extension_idx ON bundle(extension);

CREATE TABLE "downloaded_asset" (
	sha256 TEXT PRIMARY KEY NOT NULL,
	size INTEGER NOT NULL,
	inline_data BLOB
) STRICT;

-- Packs are groups of bundles.
CREATE TABLE "pack" (
	url TEXT PRIMARY KEY NOT NULL,
	name TEXT NOT NULL,
	gamemode TEXT NOT NULL,
	updated TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}'
) STRICT;

-- Packs may, optionally, contain filename->asset maps. These may be used for things
-- like banners, wallpapers, etc.
CREATE TABLE "pack_asset" (
	url TEXT NOT NULL REFERENCES pack(url) ON DELETE CASCADE,
	path TEXT NOT NULL,
	sha256 TEXT NOT NULL,

	PRIMARY KEY (url, path)
) STRICT;

-- A bundle entry in a pack.
CREATE TABLE "pack_entry" (
	url TEXT NOT NULL REFERENCES pack(url) ON DELETE CASCADE,
	entry INTEGER NOT NULL CHECK (entry > 0),
	bundle_id TEXT NOT NULL,
	desc TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}',

	PRIMARY KEY (url, entry),
	UNIQUE (url, entry)
) STRICT;

-- Courses are an ordered list of charts. They are most well known for their usage
-- in dan courses, but are also sometimes used more generally.
CREATE TABLE "course" (
	url TEXT PRIMARY KEY NOT NULL,
	name TEXT NOT NULL,
	gamemode TEXT NOT NULL,
	updated TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}'
) STRICT;

-- Courses may, optionally, contain filename->asset maps. These may be used for things
-- like banners, wallpapers, etc.
CREATE TABLE "course_asset" (
	url TEXT NOT NULL REFERENCES course(url) ON DELETE CASCADE,
	path TEXT NOT NULL,
	sha256 TEXT NOT NULL,

	PRIMARY KEY (url, path)
) STRICT;

-- A chart entry (and its index) in a course.
CREATE TABLE "course_chart" (
	url TEXT NOT NULL REFERENCES course(url) ON DELETE CASCADE,
	entry INTEGER NOT NULL CHECK (entry > 0),
	id TEXT NOT NULL,
	desc TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}',

	PRIMARY KEY (url, entry)
) STRICT;

-- A difficulty table is a mapping of chart ids to a difficulty value. These are intended to let users
-- define rating systems, and so on.
CREATE TABLE "difftable" (
	url TEXT PRIMARY KEY NOT NULL,
	name TEXT NOT NULL,
	symbol TEXT NOT NULL,
	updated TEXT NOT NULL,
	gamemode TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}'
) STRICT;

-- Tables may, optionally, contain filename->asset maps, for things like banners.
CREATE TABLE "difftable_asset" (
	url TEXT NOT NULL REFERENCES difftable(url) ON DELETE CASCADE,
	path TEXT NOT NULL,
	sha256 TEXT NOT NULL,

	PRIMARY KEY (url, path)
) STRICT;

-- A level in a table is a grouping of charts. Levels are not numbers - they are strings,
-- and the order of levels is kept in sync here with `level_order`.
CREATE TABLE "difftable_level" (
	url TEXT NOT NULL REFERENCES difftable(url) ON DELETE CASCADE,
	level TEXT NOT NULL,
	level_order INTEGER NOT NULL CHECK (level_order > 0),
	tags TEXT NOT NULL DEFAULT '{}',

	PRIMARY KEY (url, level),
	UNIQUE (url, level_order)
) STRICT;

-- An entry in a table. This maps a chart to a difficulty level in the table.
CREATE TABLE "difftable_chart" (
	url TEXT NOT NULL,
	level TEXT NOT NULL,
	chart_order INTEGER NOT NULL CHECK (chart_order > 0),
	id TEXT NOT NULL,
	desc TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}',

	PRIMARY KEY (url, level, chart_order),
	FOREIGN KEY (url, level) REFERENCES difftable_level(url, level) ON DELETE CASCADE
) STRICT;

-- Tables are allowed to define their own "folders". A folder query is defined in
-- [tinyfilter](https://github.com/zkldi/tinyfilter) syntax. For more details,
-- see the backbeat docs.
CREATE TABLE "difftable_folder" (
	url TEXT NOT NULL REFERENCES difftable(url) ON DELETE CASCADE,
	folder_order INTEGER NOT NULL CHECK (folder_order > 0),
	name TEXT NOT NULL,
	query TEXT NOT NULL,
	tags TEXT NOT NULL DEFAULT '{}',

	PRIMARY KEY (url, folder_order)
) STRICT;

-- Extra stuff. This allows for quick full text searches on bundle descriptions.
CREATE VIRTUAL TABLE bundle_fts USING fts5(
	bundle_id UNINDEXED,
	description,
	tokenize = 'unicode61 remove_diacritics 2'
);
```

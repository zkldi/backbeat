-- identifier for backbeat.db
PRAGMA application_id = 0x72700727;
-- set us to version 1
PRAGMA user_version = 1;

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
-- To add support for a new one, update `backbeat_inspector`. At the moment,
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
	description TEXT NOT NULL
) STRICT;
CREATE INDEX IF NOT EXISTS bundle_chart_sha256 ON bundle(chart_sha256);
CREATE INDEX IF NOT EXISTS bundle_extension_idx ON bundle(extension);

CREATE TABLE "downloaded_asset" (
	sha256 TEXT PRIMARY KEY NOT NULL,
	size INTEGER NOT NULL,
	inline_data BLOB
) STRICT;

-- collections! --

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

-- end collections! --

-- Extra stuff. This allows for quick full text searches on bundle descriptions.
CREATE VIRTUAL TABLE bundle_fts USING fts5(
	bundle_id UNINDEXED,
	description,
	tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER bundle_fts_ai AFTER INSERT ON bundle BEGIN
	INSERT INTO bundle_fts (bundle_id, description)
	VALUES (new.id, COALESCE(new.description, ''));
END;

CREATE TRIGGER bundle_fts_ad AFTER DELETE ON bundle BEGIN
	DELETE FROM bundle_fts WHERE bundle_id = old.id;
END;

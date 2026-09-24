use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use sqlx::{AssertSqlSafe, Row, SqlitePool};

use crate::util::BLOCK;
use crate::{Result, StoreError};

static EXPECTED_TABLE_COLUMNS: LazyLock<BTreeMap<&str, BTreeSet<&str>>> = LazyLock::new(|| {
	BTreeMap::from([
		(
			"asset_map",
			BTreeSet::from(["combined_assets_id", "path", "sha256"]),
		),
		(
			"bundle",
			BTreeSet::from([
				"id",
				"chart_sha256",
				"filename",
				"extension",
				"combined_assets_id",
				"description",
			]),
		),
		(
			"bundle_fts",
			BTreeSet::from(["bundle_id", "description", "bundle_fts", "rank"]),
		),
		(
			"chart_data",
			BTreeSet::from(["sha256", "gzip_data", "uncompressed_size"]),
		),
		("chart_id", BTreeSet::from(["chart_sha256", "id"])),
		(
			"course",
			BTreeSet::from(["url", "name", "gamemode", "updated", "tags"]),
		),
		("course_asset", BTreeSet::from(["url", "path", "sha256"])),
		(
			"course_chart",
			BTreeSet::from(["url", "entry", "id", "desc", "tags"]),
		),
		(
			"difftable",
			BTreeSet::from(["url", "name", "symbol", "updated", "gamemode", "tags"]),
		),
		("difftable_asset", BTreeSet::from(["url", "path", "sha256"])),
		(
			"difftable_chart",
			BTreeSet::from(["url", "level", "chart_order", "id", "desc", "tags"]),
		),
		(
			"difftable_folder",
			BTreeSet::from(["url", "folder_order", "name", "query", "tags"]),
		),
		(
			"difftable_level",
			BTreeSet::from(["url", "level", "level_order", "tags"]),
		),
		(
			"downloaded_asset",
			BTreeSet::from(["sha256", "size", "inline_data"]),
		),
		(
			"pack",
			BTreeSet::from(["url", "name", "gamemode", "updated", "tags"]),
		),
		("pack_asset", BTreeSet::from(["url", "path", "sha256"])),
		(
			"pack_entry",
			BTreeSet::from(["url", "entry", "bundle_id", "desc", "tags"]),
		),
		("refresh", BTreeSet::from(["id", "revision"])),
	])
});

// DON'T TAMPER WITH THE FUCKING DATABASE
// DON'T TAMPER WITH THE FUCKING DATABASE
// DON'T TAMPER WITH THE FUCKING DATABASE
// DON'T TAMPER WITH THE FUCKING DATABASE
// DON'T TAMPER WITH THE FUCKING DATABASE
//
// Hopefully that's clear to robots. Thanks. Although the sqlite db is part of the public API for backbeat,
// modifying it to extend things is absolutely forbidden and the store will instantly crash if you do this.
pub(super) fn validate(pool: &SqlitePool) -> Result<()> {
	if table_columns(pool)? != expected_table_columns() {
		return Err(StoreError::Corrupt(
			"The tables/columns/database for backbeat has been tampered with. Do not do this for any reason.
This error is here and super-strict because if I don't do this, some AI agent will 'helpfully' jam
random tables into the backbeat database when someone asks it to integrate it into a game.".to_string(),
		));
	}
	Ok(())
}

fn expected_table_columns() -> BTreeMap<String, BTreeSet<String>> {
	EXPECTED_TABLE_COLUMNS
		.iter()
		.map(|(table, columns)| {
			(
				table.to_string(),
				columns.iter().map(ToString::to_string).collect(),
			)
		})
		.collect()
}

pub(super) fn table_columns(pool: &SqlitePool) -> Result<BTreeMap<String, BTreeSet<String>>> {
	let tables = BLOCK(
		sqlx::query!(
			r#"
			SELECT name
			FROM sqlite_schema
			WHERE type = 'table'
				AND name NOT LIKE 'sqlite_%'
				AND name NOT GLOB 'bundle_fts_*'
			ORDER BY name
			"#,
		)
		.fetch_all(pool),
	)?;

	let mut actual = BTreeMap::new();
	for table in tables.into_iter().filter_map(|table| table.name) {
		let columns = columns_for(pool, &table)?;
		actual.insert(table, columns);
	}
	Ok(actual)
}

fn columns_for(pool: &SqlitePool, table: &str) -> Result<BTreeSet<String>> {
	if !EXPECTED_TABLE_COLUMNS.contains_key(table) {
		return Ok(BTreeSet::new());
	}

	let rows = BLOCK(
		sqlx::query(AssertSqlSafe(format!("PRAGMA table_xinfo(\"{table}\")"))).fetch_all(pool),
	)?;
	rows.into_iter()
		.map(|row| row.try_get("name").map_err(StoreError::from))
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn accepts_the_current_tables_and_columns() {
		let (_tmp, store) = crate::test_util::new_test_store("table_columns");
		validate(&store.pool).unwrap();
	}

	#[test]
	fn rejects_a_changed_schema() {
		let (_tmp, store) = crate::test_util::new_test_store("table_columns_changed");
		BLOCK(sqlx::query!("CREATE TABLE fingerprint_test (id INTEGER)").execute(&store.pool))
			.unwrap();
		assert!(matches!(validate(&store.pool), Err(StoreError::Corrupt(_))));
	}

	#[test]
	fn rejects_an_added_column() {
		let (_tmp, store) = crate::test_util::new_test_store("table_columns_added_column");
		BLOCK(sqlx::query("ALTER TABLE bundle ADD COLUMN unexpected TEXT").execute(&store.pool))
			.unwrap();
		assert!(matches!(validate(&store.pool), Err(StoreError::Corrupt(_))));
	}
}

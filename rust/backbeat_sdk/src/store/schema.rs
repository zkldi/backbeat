//! Store schema upgrades, keyed by SQLite's portable `user_version` value.

use sqlx::{SqliteConnection, SqlitePool};

use crate::store::{Result, StoreError};

const APPLICATION_ID: i64 = 0x7270_0727;
const CURRENT_VERSION: i64 = 1;

pub(super) async fn apply(pool: &SqlitePool) -> Result<()> {
	apply_inner(pool).await.map_err(|error| match error {
		StoreError::Db(database_error) => StoreError::Migrate(database_error.to_string()),
		other => other,
	})
}

async fn apply_inner(pool: &SqlitePool) -> Result<()> {
	let mut connection = pool.acquire().await?;
	sqlx::query("BEGIN IMMEDIATE")
		.execute(&mut *connection)
		.await?;

	let result = apply_locked(&mut connection).await;
	match result {
		Ok(()) => {
			sqlx::query("COMMIT").execute(&mut *connection).await?;
			Ok(())
		}
		Err(error) => {
			sqlx::query("ROLLBACK").execute(&mut *connection).await?;
			Err(error)
		}
	}
}

async fn apply_locked(connection: &mut SqliteConnection) -> Result<()> {
	let version: i64 = sqlx::query_scalar("PRAGMA user_version")
		.fetch_one(&mut *connection)
		.await?;
	let application_id: i64 = sqlx::query_scalar("PRAGMA application_id")
		.fetch_one(&mut *connection)
		.await?;

	if !(0..=CURRENT_VERSION).contains(&version) {
		return Err(StoreError::Migrate(format!(
			"unsupported database schema version {version}; this build supports up to {CURRENT_VERSION}"
		)));
	}
	let expected_application_id = if version == 0 { 0 } else { APPLICATION_ID };
	if application_id != expected_application_id {
		return Err(StoreError::Migrate(format!(
			"unexpected SQLite application ID {application_id:#x}"
		)));
	}

	// Add future schema revisions here in ascending order, gated by the version
	// read above. Each SQL file sets `user_version` to its own revision.
	if version < 1 {
		sqlx::raw_sql(include_str!("../../schema/v1.sql"))
			.execute(&mut *connection)
			.await?;
	}

	// Older releases used SQLx checksums, which differ across platforms. The
	// SQLite schema version is authoritative; this table is no longer needed.
	sqlx::query("DROP TABLE IF EXISTS _db_migrations")
		.execute(&mut *connection)
		.await?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::store::Backbeat;
	use crate::util::BLOCK;
	use backbeat_store_config::BackbeatConfig;
	use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

	#[test]
	fn opens_legacy_store_with_a_different_checksum() {
		let temp = tempfile::Builder::new()
			.prefix("legacy_migration_checksum")
			.tempdir()
			.expect("create legacy store directory");
		let config_dir = temp.path().join("config");
		let store_dir = temp.path().join("data");
		let mut config = BackbeatConfig::default();
		config.store.path = store_dir.clone();
		config.write_to_dir(&config_dir).expect("write test config");
		std::fs::create_dir_all(&store_dir).expect("create store directory");

		// Build the on-disk layout produced by the SQLx migration from the
		// previous release, before the new opener sees the database.
		let legacy_pool = BLOCK(
			SqlitePoolOptions::new().connect_with(
				SqliteConnectOptions::new()
					.filename(store_dir.join(Backbeat::DB_FILENAME))
					.create_if_missing(true),
			),
		)
		.expect("create legacy database");
		BLOCK(async {
			sqlx::raw_sql(include_str!("../../schema/v1.sql"))
				.execute(&legacy_pool)
				.await?;
			sqlx::query(
				"CREATE TABLE _db_migrations (
					version BIGINT PRIMARY KEY,
					description TEXT NOT NULL,
					installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
					success BOOLEAN NOT NULL,
					checksum BLOB NOT NULL,
					execution_time BIGINT NOT NULL
				)",
			)
			.execute(&legacy_pool)
			.await?;
			sqlx::query(
				"INSERT INTO _db_migrations (version, description, success, checksum, execution_time)
				 VALUES (20230312222545, 'zenithforever', TRUE, X'00', 1)",
			)
				.execute(&legacy_pool)
				.await?;
			sqlx::query("UPDATE refresh SET revision = 42 WHERE id = 1")
				.execute(&legacy_pool)
				.await?;
			Ok::<_, sqlx::Error>(())
		})
		.unwrap();
		drop(legacy_pool);

		let reopened = Backbeat::open_with_overridden_config_dir(&config_dir)
			.expect("reopen a store created by SQLx migrations");
		let (revision, legacy_table): (i64, i64) = BLOCK(async {
			let revision = sqlx::query_scalar("SELECT revision FROM refresh WHERE id = 1")
				.fetch_one(&reopened.pool)
				.await?;
			let legacy_table = sqlx::query_scalar(
				"SELECT count(*) FROM sqlite_schema WHERE name = '_db_migrations'",
			)
			.fetch_one(&reopened.pool)
			.await?;
			Ok::<_, sqlx::Error>((revision, legacy_table))
		})
		.unwrap();
		assert_eq!(revision, 42);
		assert_eq!(legacy_table, 0);

		// The migrated store remains writable after the legacy ledger is removed.
		BLOCK(sqlx::query("UPDATE refresh SET revision = 43 WHERE id = 1").execute(&reopened.pool))
			.unwrap();
		let revision: i64 = BLOCK(
			sqlx::query_scalar("SELECT revision FROM refresh WHERE id = 1")
				.fetch_one(&reopened.pool),
		)
		.unwrap();
		assert_eq!(revision, 43);
	}

	#[test]
	fn refuses_a_newer_schema_without_modifying_it() {
		let (_temp, store) = crate::test_util::new_test_store("future_schema_version");
		BLOCK(sqlx::query("PRAGMA user_version = 2").execute(&store.pool)).unwrap();
		assert!(matches!(
			BLOCK(apply(&store.pool)),
			Err(StoreError::Migrate(message)) if message.contains("version 2")
		));
		let version: i64 =
			BLOCK(sqlx::query_scalar("PRAGMA user_version").fetch_one(&store.pool)).unwrap();
		assert_eq!(version, 2);
	}
}

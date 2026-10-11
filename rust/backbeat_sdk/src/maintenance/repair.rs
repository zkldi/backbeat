use std::io::ErrorKind;

use crate::store::Backbeat;
use crate::util::BLOCK;
use crate::{Result, StoreError};

impl Backbeat {
	pub(crate) fn corruption_repair_inner(&self) -> Result<()> {
		let scan = self.corruption_scan()?;
		let mut bad_assets = scan.report.missing_assets;
		bad_assets.extend(scan.report.corrupt_assets);

		for asset_id in &bad_assets {
			match crate::fs::remove_file(self.assets.asset_path(*asset_id)) {
				Ok(()) => {}
				Err(err) if err.kind() == ErrorKind::NotFound => {}
				Err(err) => return Err(err.into()),
			}
		}

		BLOCK(async {
			let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
			let mut changed = false;

			for asset_id in bad_assets {
				let result =
					sqlx::query!("DELETE FROM downloaded_asset WHERE sha256 = ?1", asset_id)
						.execute(&mut *tx)
						.await?;
				changed |= result.rows_affected() > 0;
			}

			for chart_sha256 in scan.report.corrupt_charts {
				let asset_maps = sqlx::query_scalar!(
					r#"
					SELECT DISTINCT combined_assets_id AS "combined_assets_id!: backbeat_core::CombinedAssetsId"
					FROM bundle
					WHERE chart_sha256 = ?1
					"#,
					chart_sha256,
				)
				.fetch_all(&mut *tx)
				.await?;

				let bundles =
					sqlx::query!("DELETE FROM bundle WHERE chart_sha256 = ?1", chart_sha256)
						.execute(&mut *tx)
						.await?;
				changed |= bundles.rows_affected() > 0;

				for combined_assets_id in asset_maps {
					sqlx::query!(
						"DELETE FROM asset_map WHERE combined_assets_id = ?1 AND NOT EXISTS (SELECT 1 FROM bundle WHERE combined_assets_id = ?1)",
						combined_assets_id,
					)
					.execute(&mut *tx)
					.await?;
				}

				let chart = sqlx::query!("DELETE FROM chart_data WHERE sha256 = ?1", chart_sha256)
					.execute(&mut *tx)
					.await?;
				changed |= chart.rows_affected() > 0;
			}

			for bundle in scan.bundles {
				let updated = sqlx::query!(
					"UPDATE bundle SET description = ?1 WHERE id = ?2 AND description != ?1",
					bundle.description,
					bundle.bundle_id,
				)
				.execute(&mut *tx)
				.await?;
				if updated.rows_affected() > 0 {
					sqlx::query!(
						"UPDATE bundle_fts SET description = ?1 WHERE bundle_id = ?2",
						bundle.description,
						bundle.bundle_id,
					)
					.execute(&mut *tx)
					.await?;
					changed = true;
				}
			}

			for repair in scan.chart_ids {
				if !repair.needs_update {
					continue;
				}
				sqlx::query!(
					"DELETE FROM chart_id WHERE chart_sha256 = ?1",
					repair.chart_sha256,
				)
				.execute(&mut *tx)
				.await?;
				for chart_id in repair.chart_ids {
					sqlx::query!(
						"INSERT INTO chart_id (chart_sha256, id) VALUES (?1, ?2)",
						repair.chart_sha256,
						chart_id,
					)
					.execute(&mut *tx)
					.await?;
				}
				changed = true;
			}

			if changed {
				Self::increment_refresh(&mut tx).await?;
			}
			tx.commit().await?;
			Ok::<_, StoreError>(())
		})
	}
}

#[cfg(test)]
mod tests {
	use std::collections::HashMap;

	use backbeat_core::{
		AssetId, AssetPath, BackbeatFile, ChartData, ChartDesc, ChartFilename, Sha256,
	};
	use backbeat_store_config::{BackbeatConfig, ByteSize};

	use super::*;
	use crate::test_support::store_asset_unchecked;
	use crate::test_util::{new_test_store, new_test_store_with};

	fn bundle(filename: &str, chart: &[u8]) -> BackbeatFile {
		BackbeatFile {
			filename: ChartFilename::from_path(filename).unwrap(),
			assets: HashMap::new(),
			desc: ChartDesc::new("test").unwrap(),
			chart: ChartData::compress(chart).unwrap(),
		}
	}

	#[test]
	fn repairs_chart_ids() {
		let (_temp, store) = new_test_store("corruption_repair_chart_ids");
		let bb = bundle("chart.bms", b"#TITLE Test\n#ARTIST Artist\n#BPM 120\n");
		let chart_id = bb.additional_chart_ids().into_iter().next().unwrap();
		let bundle_id = store.import_bundle(&bb).unwrap();
		let chart_sha256 = bb.chart_sha256();
		let desired_chart_id = chart_id.to_string();
		let wrong_chart_id = format!("{}/wrong", chart_id.alg);

		BLOCK(async {
			sqlx::query!("DELETE FROM chart_id WHERE chart_sha256 = ?1", chart_sha256)
				.execute(&store.pool)
				.await?;
			sqlx::query!(
				"INSERT INTO chart_id (chart_sha256, id) VALUES (?1, ?2)",
				chart_sha256,
				wrong_chart_id,
			)
			.execute(&store.pool)
			.await?;
			Ok::<_, StoreError>(())
		})
		.unwrap();

		assert_eq!(store.corruption_check().unwrap().wrong_chart_ids.len(), 1);
		store.corruption_repair().unwrap();

		let detail = store.bundle_detail(bundle_id).unwrap();
		assert_eq!(
			detail
				.chart_ids
				.into_iter()
				.map(|id| id.to_string())
				.collect::<Vec<_>>(),
			vec![desired_chart_id]
		);
		assert!(store.corruption_check().unwrap().is_ok());
		store.corruption_repair().unwrap();
	}

	#[test]
	fn removes_corrupt_inline_asset_rows() {
		let (_temp, store) = new_test_store("corruption_repair_inline_asset");
		let asset_id = AssetId(Sha256::checksum_bytes(b"expected"));
		store_asset_unchecked(&store, asset_id, b"wrong").unwrap();

		let report = store.corruption_check().unwrap();
		assert_eq!(report.corrupt_assets, vec![asset_id]);
		store.corruption_repair().unwrap();
		assert!(!store.has_asset(asset_id).unwrap());
	}

	#[test]
	fn removes_missing_and_corrupt_disk_assets() {
		let mut config = BackbeatConfig::default();
		config.store.inline = ByteSize::ZERO;
		let (_temp, store) = new_test_store_with("corruption_repair_disk_assets", config);
		let corrupt_id = AssetId(Sha256::checksum_bytes(b"expected"));
		let missing_id = AssetId(Sha256::checksum_bytes(b"missing"));
		store_asset_unchecked(&store, corrupt_id, b"wrong").unwrap();
		store_asset_unchecked(&store, missing_id, b"missing").unwrap();
		crate::fs::remove_file(store.assets.asset_path(missing_id)).unwrap();

		let report = store.corruption_check().unwrap();
		assert_eq!(report.large_asset_count, 2);
		assert_eq!(report.missing_assets, vec![missing_id]);
		assert_eq!(report.corrupt_assets, vec![corrupt_id]);

		store.corruption_repair().unwrap();
		assert!(!store.has_asset(corrupt_id).unwrap());
		assert!(!store.has_asset(missing_id).unwrap());
		assert!(!store.assets.asset_path(corrupt_id).exists());
	}

	#[test]
	fn removes_every_bundle_for_a_corrupt_chart() {
		let (_temp, store) = new_test_store("corruption_repair_chart");
		let mut first = bundle("first.bin", b"original");
		let asset_id = AssetId(Sha256::checksum_bytes(b"asset"));
		first
			.assets
			.insert(AssetPath::from_path("asset.bin").unwrap(), asset_id);
		let mut second = first.clone();
		second.filename = ChartFilename::from_path("second.bin").unwrap();
		let first_id = store.import_bundle(&first).unwrap();
		let second_id = store.import_bundle(&second).unwrap();
		let chart_sha256 = first.chart_sha256();
		let replacement = ChartData::compress(b"changed").unwrap();
		let replacement = replacement.as_compressed();

		BLOCK(
			sqlx::query!(
				"UPDATE chart_data SET gzip_data = ?1 WHERE sha256 = ?2",
				replacement,
				chart_sha256,
			)
			.execute(&store.pool),
		)
		.unwrap();

		assert_eq!(
			store.corruption_check().unwrap().corrupt_charts,
			vec![chart_sha256]
		);
		store.corruption_repair().unwrap();
		assert!(!store.has_bundle(first_id).unwrap());
		assert!(!store.has_bundle(second_id).unwrap());
		let asset_map_count = BLOCK(
			sqlx::query_scalar!("SELECT COUNT(*) AS \"count!: i64\" FROM asset_map")
				.fetch_one(&store.pool),
		)
		.unwrap();
		assert_eq!(asset_map_count, 0);
	}

	#[test]
	fn preserves_charts_with_valid_storage_data() {
		let (_temp, store) = new_test_store("corruption_repair_invalid_chart");
		let bundle_id = store.import_bundle(&bundle("chart.bin", b"")).unwrap();
		BLOCK(
			sqlx::query!(
				"UPDATE bundle SET filename = 'chart.sm', extension = 'sm' WHERE id = ?1",
				bundle_id
			)
			.execute(&store.pool),
		)
		.unwrap();

		assert!(store.corruption_check().unwrap().corrupt_charts.is_empty());
		store.corruption_repair().unwrap();
		assert!(store.has_bundle(bundle_id).unwrap());
	}
}

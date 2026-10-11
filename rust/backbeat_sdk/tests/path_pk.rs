#![allow(unreachable_pub, clippy::all, clippy::restriction)]
#![allow(clippy::pedantic, clippy::nursery, clippy::cargo)]

//! Path/lookup edge cases for chart and asset storage.

use backbeat_core::asset_id::AssetId;
use backbeat_core::{AssetPath, ChartDesc, ChartFilename, ChartId, IdAlgorithm, Sha256};
use backbeat_core::{Assets, BackbeatFile, ChartData};

mod support;
use support::new_test_store;

fn sample_bb(filename: &str, chart_bytes: &[u8]) -> BackbeatFile {
	BackbeatFile {
		filename: ChartFilename::from_path(filename).unwrap(),
		assets: Assets::default(),
		desc: ChartDesc::new("test").unwrap(),
		chart: ChartData::compress(chart_bytes).unwrap(),
	}
}

fn chart_id(bb: &BackbeatFile, algorithm: IdAlgorithm) -> ChartId {
	match &algorithm {
		IdAlgorithm::Sha256 => ChartId {
			alg: algorithm,
			val: bb.chart_sha256().to_string(),
		},
		IdAlgorithm::Custom(_) => bb
			.additional_chart_ids()
			.into_iter()
			.find(|id| id.alg == algorithm)
			.unwrap(),
	}
}

#[test]
fn read_chart_bytes_skips_asset_lookup() {
	let (_tmp, store) = new_test_store("bb_path_pk_read_chart");
	let chart_bytes = b"#TITLE:read_chart_bytes;";
	let bb = sample_bb("song.bms", chart_bytes);
	store.import_bundle(&bb).expect("import");

	let exported = store
		.get_chart_data(&chart_id(&bb, IdAlgorithm::Sha256))
		.expect("read chart bytes");
	assert_eq!(exported, chart_bytes);
}

#[test]
fn zero_byte_chart_fixture_roundtrips_through_store() {
	let (_tmp, store) = new_test_store("bb_path_pk_zero_byte_chart");
	let fixture = concat!(
		env!("CARGO_MANIFEST_DIR"),
		"/../../fixtures/bb/zero-byte-chart.bb"
	);
	let bb = BackbeatFile::from_file(fixture).unwrap();
	let bundle_id = store.import_bundle(&bb).unwrap();
	let chart_id = chart_id(&bb, IdAlgorithm::Sha256);

	assert!(store.get_chart_data(&chart_id).unwrap().is_empty());
	assert!(
		store
			.get_chart(&chart_id)
			.unwrap()
			.chart
			.decompress()
			.is_empty()
	);
	assert!(
		store
			.get_bundle(bundle_id)
			.unwrap()
			.chart
			.decompress()
			.is_empty()
	);
}

#[test]
fn unknown_valid_chart_algorithm_is_a_normal_miss() {
	let (_tmp, store) = new_test_store("bb_path_pk_unknown_algorithm");
	let chart_id: ChartId = "future-algorithm/abc123".parse().unwrap();

	assert!(!store.has_chart(&chart_id).unwrap());
}

#[test]
fn parent_traversing_asset_path_is_allowed() {
	let (_tmp, store) = new_test_store("bb_path_pk");
	let asset_id = AssetId(Sha256::checksum_bytes(b"escape"));
	let mut bb = sample_bb(
		"baz.sm",
		b"#TITLE:escape;\n#BPMS:0=120;\n#NOTES:dance-single:A:Hard:9:0:0000;\n",
	);
	bb.assets.insert(
		AssetPath::from_path("../../../banner.png").unwrap(),
		asset_id,
	);
	store.import_bundle(&bb).expect("parent path allowed");
}

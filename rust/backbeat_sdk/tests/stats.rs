#![allow(unreachable_pub, clippy::all, clippy::restriction)]
#![allow(clippy::pedantic, clippy::nursery, clippy::cargo)]

//! Regression tests for `Backbeat::stats`.

use backbeat_core::ChartDesc;
use backbeat_core::ChartFilename;
use backbeat_core::{BackbeatFile, ChartData};
mod support;
use support::new_test_store;

#[test]
fn stats_count_bundle_rows() {
	let (_tmp, store) = new_test_store("bb_stats_bundle_rows");
	let chart_bytes = b"#TITLE:shared-content;";

	let bb_a = BackbeatFile {
		filename: ChartFilename::from_path("song.bms").unwrap(),
		assets: Default::default(),
		desc: ChartDesc::new("test").unwrap(),
		chart: ChartData::compress(chart_bytes).unwrap(),
	};
	let bundle_a = store.import_bundle(&bb_a).expect("import a");

	let bb_b = BackbeatFile {
		filename: ChartFilename::from_path("song1234.bms").unwrap(),
		assets: Default::default(),
		desc: ChartDesc::new("test").unwrap(),
		chart: ChartData::compress(chart_bytes).unwrap(),
	};
	let bundle_b = store.import_bundle(&bb_b).expect("import b");

	let sha256_a = store.get_bundle(bundle_a).unwrap().chart_sha256();
	let sha256_b = store.get_bundle(bundle_b).unwrap().chart_sha256();

	assert_eq!(
		sha256_a, sha256_b,
		"identical bytes must hash identically despite gzip"
	);

	let stats = store.stats().expect("stats");
	assert_eq!(stats.charts, 2);
}

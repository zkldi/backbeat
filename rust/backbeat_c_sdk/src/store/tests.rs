use std::ffi::{CStr, CString};
use std::fs;
use std::ptr;

use backbeat_sdk::Backbeat;

use super::*;
use crate::error::{BKB_ERR_INVALID_STRING, BKB_ERR_NULL_ARG, BKB_OK};
use crate::functions::bkb_string_free;

unsafe fn take_string(value: bkb_string) -> String {
	let result = unsafe { CStr::from_ptr(value.ptr) }
		.to_str()
		.unwrap()
		.to_owned();
	unsafe { bkb_string_free(value) };
	result
}

#[test]
fn open_rejects_a_null_output() {
	assert_eq!(unsafe { bkb_store_open(ptr::null_mut()) }, BKB_ERR_NULL_ARG);
}

#[test]
fn accessors_reject_a_null_store() {
	let mut out = bkb_string {
		ptr: ptr::dangling_mut(),
		len: usize::MAX,
	};
	assert_eq!(
		unsafe { bkb_store_dir(ptr::null(), &mut out) },
		BKB_ERR_NULL_ARG
	);
	assert_eq!(out.ptr, ptr::dangling_mut());
	assert_eq!(out.len, usize::MAX);
}

#[test]
fn free_accepts_null() {
	unsafe { bkb_store_free(ptr::null_mut()) };
}

#[test]
fn accessors_return_owned_values() {
	let root = tempfile::tempdir().unwrap();
	let config_dir = root.path().join("config");
	let store_dir = root.path().join("store");
	fs::create_dir_all(&config_dir).unwrap();
	fs::write(
		config_dir.join("backbeat.toml"),
		format!("[store]\npath = {:?}\n", store_dir.to_string_lossy()),
	)
	.unwrap();
	let inner = Backbeat::open_with_overridden_config_dir(&config_dir).unwrap();
	let store = Box::into_raw(Box::new(bkb_store { inner }));

	let mut string = bkb_string {
		ptr: ptr::null_mut(),
		len: 0,
	};
	assert_eq!(unsafe { bkb_store_dir(store, &mut string) }, BKB_OK);
	assert_eq!(unsafe { take_string(string) }, store_dir.to_string_lossy());

	assert_eq!(unsafe { bkb_store_logs_dir(store, &mut string) }, BKB_OK);
	assert_eq!(
		unsafe { take_string(string) },
		store_dir.join("logs").to_string_lossy()
	);

	assert_eq!(unsafe { bkb_store_config_dir(store, &mut string) }, BKB_OK);
	assert_eq!(unsafe { take_string(string) }, config_dir.to_string_lossy());

	let url = CString::new("https://data.makiba.ac").unwrap();
	let server = bkb_server_config { url: url.as_ptr() };
	assert_eq!(unsafe { bkb_store_server_rm(store, &server) }, BKB_OK);

	let mut has_zero_data_servers = false;
	assert_eq!(
		unsafe { bkb_store_has_zero_data_servers(store, &mut has_zero_data_servers) },
		BKB_OK
	);
	assert!(has_zero_data_servers);

	let url = CString::new("https://data.backbeat.example").unwrap();
	let server = bkb_server_config { url: url.as_ptr() };
	assert_eq!(unsafe { bkb_store_server_add(store, &server) }, BKB_OK);
	assert_eq!(
		unsafe { bkb_store_has_zero_data_servers(store, &mut has_zero_data_servers) },
		BKB_OK
	);
	assert!(!has_zero_data_servers);
	assert_eq!(unsafe { bkb_store_server_rm(store, &server) }, BKB_OK);
	assert_eq!(
		unsafe { bkb_store_has_zero_data_servers(store, &mut has_zero_data_servers) },
		BKB_OK
	);
	assert!(has_zero_data_servers);

	let invalid_url = CString::new("not a URL").unwrap();
	let invalid_server = bkb_server_config {
		url: invalid_url.as_ptr(),
	};
	assert_eq!(
		unsafe { bkb_store_server_add(store, &invalid_server) },
		crate::error::BKB_ERR_INVALID_URL
	);
	assert_eq!(
		unsafe { bkb_store_server_rm(store, &invalid_server) },
		crate::error::BKB_ERR_INVALID_URL
	);

	assert_eq!(
		unsafe { bkb_store_sqlite_connection_url(store, &mut string) },
		BKB_OK
	);
	let connection_url = unsafe { take_string(string) };
	assert!(connection_url.starts_with("file://"));
	assert!(connection_url.ends_with("/backbeat.db?mode=ro"));

	assert_eq!(
		unsafe { bkb_store_sqlite_attach_command(store, &mut string) },
		BKB_OK
	);
	assert_eq!(
		unsafe { take_string(string) },
		format!("ATTACH DATABASE '{connection_url}' AS backbeat;")
	);

	assert_eq!(
		unsafe { bkb_store_sqlite_detach_command(store, &mut string) },
		BKB_OK
	);
	assert_eq!(unsafe { take_string(string) }, "DETACH DATABASE backbeat;");

	unsafe { bkb_store_free(store) };
}

#[test]
fn server_arguments_are_checked() {
	assert_eq!(
		unsafe { bkb_store_server_add(ptr::null(), ptr::null()) },
		BKB_ERR_NULL_ARG
	);
	let invalid_url = [u8::MAX, 0];
	let server = bkb_server_config {
		url: invalid_url.as_ptr().cast(),
	};
	assert_eq!(
		unsafe { bkb_store_server_rm(ptr::null(), &server) },
		BKB_ERR_INVALID_STRING
	);
}

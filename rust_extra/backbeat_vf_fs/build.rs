use std::env;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let db_path = Path::new(&env::var("CARGO_MANIFEST_DIR")?)
		.join("../../rust/backbeat_sdk/template.backbeat.db");
	let db_url = format!("sqlite://{}?mode=ro", db_path.display());

	// Compile-time `sqlx::query!` checks use this URL (no `.env` required).
	println!("cargo:rustc-env=DATABASE_URL={db_url}");

	// Tell Cargo that if the given file changes, to rerun this build script.
	println!("cargo:rerun-if-changed={}", db_path.display());

	Ok(())
}

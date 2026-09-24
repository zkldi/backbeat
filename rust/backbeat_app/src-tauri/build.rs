use std::env;
use std::path::Path;

use vergen_git2::{Emitter, Git2Builder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
	let db_path =
		Path::new(&env::var("CARGO_MANIFEST_DIR")?).join("../../backbeat_sdk/template.backbeat.db");
	let db_url = format!("sqlite://{}?mode=ro", db_path.display());

	// Compile-time `sqlx::query!` checks use this URL (no `.env` required).
	println!("cargo:rustc-env=DATABASE_URL={db_url}");

	// Tell Cargo that if the given file changes, to rerun this build script.
	println!("cargo:rerun-if-changed={}", db_path.display());

	let git2 = Git2Builder::default().sha(true).build()?;
	Emitter::default().add_instructions(&git2)?.emit()?;

	tauri_build::build();

	Ok(())
}

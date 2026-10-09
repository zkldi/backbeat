//! Patterns for files to ignore during folder asset scanning.
//!
//! When importing a chart, backbeat scans the entire chart directory and
//! treats every file it finds as an asset. Chart files themselves (`.bms`,
//! `.ksh`, etc.) must be excluded from this scan so that sibling charts are
//! not recorded as assets of one another.

use std::sync::OnceLock;

use regex::RegexSet;

static PATTERNS: OnceLock<RegexSet> = OnceLock::new();

fn patterns() -> &'static RegexSet {
	PATTERNS.get_or_init(|| {
		RegexSet::new([
			// Never recursively include previously packaged output as an asset.
			r"(?i)\.(bb|bbzip)$",
			// Other charts shouldn't be included in the dependency list.
			r"(?i)\.(bms|bme|bml|pms)$",
			r"(?i)\.bmson$",
			r"(?i)\.(sm|ssc|dwi)$",
			r"(?i)\.osu$",
			r"(?i)\.(ksh|kson|kshl|oldksh)$",
			r"(?i)\.dtx$",
			r"(?i)\.tja$",
			r"(?i)\.chart$",
			r"(?i)\.old$",
			// OS noise
			r"(?i)^\.DS_Store$",
			r"(?i)^Thumbs\.db$",
			r"(?i)^desktop\.ini$",
			r"(?i)^\._.*",        // macOS AppleDouble resource fork files
			r"(?i)^__MACOSX$",    // junk directory created by macOS zip
			r"(?i)\.lnk$",        // Windows shortcuts
			r"(?i)\.exe$",        // Windows executables
			r"(?i)\.bat$",        // Windows batch scripts
			r"(?i)~$",            // editor backup files (Emacs, gedit, nano)
			r"(?i)^\.directory$", // KDE/Dolphin folder metadata
		])
		.expect("ignore patterns are valid regexes")
	})
}

pub(crate) fn should_ignore_file(filename: &str) -> bool {
	patterns().is_match(filename)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn chart_files_are_ignored() {
		assert!(should_ignore_file("song.bms"));
		assert!(should_ignore_file("song.BME"));
		assert!(should_ignore_file("song.Bml"));
		assert!(should_ignore_file("song.pms"));
		assert!(should_ignore_file("song.bmson"));
		assert!(should_ignore_file("song.sm"));
		assert!(should_ignore_file("song.ksh"));
		assert!(should_ignore_file("song.kson"));
		assert!(should_ignore_file("song.kshl"));
		assert!(should_ignore_file("song.oldksh"));
		assert!(should_ignore_file("beatmap.osu"));
	}

	#[test]
	fn os_noise_is_ignored() {
		assert!(should_ignore_file(".DS_Store"));
		assert!(should_ignore_file("Thumbs.db"));
		assert!(should_ignore_file("desktop.ini"));
		assert!(should_ignore_file("._kick.wav")); // macOS AppleDouble
		assert!(should_ignore_file("._")); // macOS AppleDouble
		assert!(should_ignore_file("__MACOSX"));
		assert!(should_ignore_file("shortcut.lnk"));
		assert!(should_ignore_file("setup.exe"));
		assert!(should_ignore_file("Setup.EXE"));
		assert!(should_ignore_file("sabunmaker.bat"));
		assert!(should_ignore_file("_decode_ogg.BAT"));
		assert!(should_ignore_file("notes.txt~"));
		assert!(should_ignore_file(".directory"));
	}

	#[test]
	fn sm_old_files_are_ignored() {
		assert!(should_ignore_file("song.sm.old"));
		assert!(should_ignore_file("song.SM.OLD"));
		assert!(should_ignore_file("chart.ssc.old"));
	}

	#[test]
	fn daw_and_editor_sidecars_are_not_ignored() {
		assert!(!should_ignore_file("song.reapeaks"));
		assert!(!should_ignore_file("song.reapindex"));
		assert!(!should_ignore_file("project.rpp"));
		assert!(!should_ignore_file("project.rpp-bak"));
		assert!(!should_ignore_file("clip.asd"));
		assert!(!should_ignore_file("song.sfk"));
		assert!(!should_ignore_file("chart.kco"));
		assert!(!should_ignore_file("photo.xmp"));
		assert!(!should_ignore_file(".vs"));
	}

	#[test]
	fn assets_are_not_ignored() {
		assert!(!should_ignore_file("kick.wav"));
		assert!(!should_ignore_file("bg.png"));
		assert!(!should_ignore_file("preview.ogg"));
		assert!(!should_ignore_file("info.txt"));
		assert!(!should_ignore_file("movie.mp4"));
	}
}

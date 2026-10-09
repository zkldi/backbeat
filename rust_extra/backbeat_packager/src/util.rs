use std::path::Path;

/// Rust's default `.extension()` is broken for files called e.g. `.foo`.
///
/// It seems to disagree that those files have an extension. I disagree. `.bms` is a bms
/// file with no name, not a file called `.bms` with no extension.
pub(crate) fn safe_extension(path: &Path) -> Option<&str> {
	if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
		return Some(ext);
	}
	let name = path.file_name()?.to_str()?;
	let rest = name.strip_prefix('.')?;
	if !rest.is_empty() && !rest.contains('.') {
		Some(rest)
	} else {
		None
	}
}

/// Make a portable filename stem, leaving room for an extension and collision suffix.
pub fn sanitise_filename(name: &str) -> String {
	let mut sanitised: String = name
		.chars()
		.map(|character| {
			if character.is_control()
				|| matches!(
					character,
					'/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
				) {
				'-'
			} else {
				character
			}
		})
		.collect();

	while sanitised.len() > 200 {
		sanitised.pop();
	}
	sanitised = sanitised.trim_end_matches([' ', '.']).to_owned();
	if sanitised.is_empty() || matches!(sanitised.as_str(), "." | "..") {
		return "something".to_owned();
	}

	let stem = sanitised
		.split('.')
		.next()
		.unwrap_or_default()
		.trim_end_matches([' ', '.']);
	if is_windows_device_name(stem) {
		let index = sanitised.find('.').unwrap_or(sanitised.len());
		sanitised.insert(index, '-');
	}

	sanitised
}

fn is_windows_device_name(stem: &str) -> bool {
	let uppercase = stem.to_ascii_uppercase();
	matches!(
		uppercase.as_str(),
		"CON"
			| "PRN" | "AUX"
			| "NUL" | "COM1"
			| "COM2" | "COM3"
			| "COM4" | "COM5"
			| "COM6" | "COM7"
			| "COM8" | "COM9"
			| "LPT1" | "LPT2"
			| "LPT3" | "LPT4"
			| "LPT5" | "LPT6"
			| "LPT7" | "LPT8"
			| "LPT9" | "CLOCK$"
			| "COM¹" | "COM²"
			| "COM³" | "LPT¹"
			| "LPT²" | "LPT³"
	)
}

#[cfg(test)]
mod tests {
	use super::sanitise_filename;

	#[test]
	fn sanitise_filename_makes_portable_file_names() {
		assert_eq!(sanitise_filename("A/B: C?"), "A-B- C-");
		assert_eq!(sanitise_filename("CON.txt"), "CON-.txt");
		assert_eq!(sanitise_filename("..."), "something");
		assert_eq!(sanitise_filename("../A\\B\n"), "..-A-B-");
		assert_eq!(sanitise_filename("NUL. "), "NUL-");
		assert_eq!(sanitise_filename(""), "something");
		assert_eq!(sanitise_filename(&"界".repeat(100)), "界".repeat(66));
	}
}

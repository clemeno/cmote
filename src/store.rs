// store.rs — how cmote's on-disk files are written (PLAN §110).
//
// Three files are written while the app runs: `targets.json` (§14), `settings.json` (§31) and the
// sealed `secrets.age` (§16). Only the vault was written SAFELY. It seals its blob into a temp file
// beside the real one and renames it over the top, so a crash halfway through cannot leave a
// half-written vault — which for that file would mean losing every stored secret at once.
//
// The other two used a plain `std::fs::write`, which truncates the file and then fills it. A crash,
// a full disk or a killed process between those two steps leaves a truncated file, and a truncated
// JSON file does not parse — so `Targets::load_from` would treat it as empty and the next save would
// write that emptiness back. The user's saved targets would be gone with no error anywhere.
//
// So the vault's own pattern moves here and all three writers use it. `rename` over an existing path
// is atomic on both targets: on unix by POSIX, and on Windows because `std::fs::rename` maps to
// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Write `bytes` to `path`, replacing whatever was there, atomically.
///
/// The temp file sits in the SAME directory as the target, because a rename is only atomic within
/// one filesystem — a temp in `%TEMP%` could be on another volume, where `rename` degrades to a
/// copy-then-delete and the crash window comes back. The parent directory is created first: the data
/// directory normally exists, but a test path or a fresh portable install may not have it yet.
pub fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)
			.with_context(|| format!("failed to create {}", parent.display()))?;
	}
	let temp = temp_beside(path);
	std::fs::write(&temp, bytes).with_context(|| format!("failed to write {}", temp.display()))?;
	std::fs::rename(&temp, path)
		.with_context(|| format!("failed to replace {}", path.display()))?;
	Ok(())
}

/// `<path>.tmp`, beside the target. `with_extension` would be wrong: it REPLACES the extension, so
/// `secrets.age` would become `secrets.tmp` and two different stores could collide on one temp name.
fn temp_beside(path: &Path) -> PathBuf {
	with_suffix(path, ".tmp")
}

/// `path` with `suffix` appended to the whole file name, extension included.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
	let mut name = path.file_name().map_or_else(OsString::new, OsString::from);
	name.push(suffix);
	path.with_file_name(name)
}

#[cfg(test)]
mod tests {
	use super::{with_suffix, write_atomically};

	#[test]
	fn the_temp_name_keeps_the_whole_file_name() {
		// `with_extension` would give `secrets.tmp` and `targets.tmp` — and `settings.tmp` too, so
		// two stores writing at once could fight over one temp path.
		let vault = std::path::Path::new("/data/secrets.age");
		assert_eq!(
			with_suffix(vault, ".tmp").file_name().unwrap(),
			"secrets.age.tmp"
		);
		let targets = std::path::Path::new("/data/targets.json");
		assert_eq!(
			with_suffix(targets, ".tmp").file_name().unwrap(),
			"targets.json.tmp"
		);
	}

	#[test]
	fn a_write_creates_the_directory_and_leaves_no_temp_behind() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("nested").join("targets.json");
		write_atomically(&path, b"[]").unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
		assert!(
			!with_suffix(&path, ".tmp").exists(),
			"the temp file must be renamed away, not left beside the store"
		);
	}

	#[test]
	fn a_second_write_replaces_the_first() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		write_atomically(&path, b"first").unwrap();
		write_atomically(&path, b"second").unwrap();
		assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
	}
}

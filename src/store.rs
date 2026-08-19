// store.rs — how cmote's on-disk files are written (PLAN §110).
//
// Four files are written while the app runs: `targets.json` (§14), `settings.json` (§31), the sealed
// `secrets.age` (§16) and `known_hosts` (§8). Only the vault was written SAFELY. It seals its blob
// into a temp file beside the real one and renames it over the top, so a crash halfway through
// cannot leave a half-written vault — which for that file would mean losing every stored secret at
// once.
//
// The other three used a plain `std::fs::write`, which truncates the file and then fills it. A
// crash, a full disk or a killed process between those two steps leaves a truncated file, and a
// truncated JSON file does not parse — so `Targets::load_from` would treat it as empty and the next
// save would write that emptiness back. The user's saved targets would be gone with no error
// anywhere. A truncated `known_hosts` is quieter and worse: the hosts it forgot verify as `Unknown`,
// which is the first-contact prompt rather than the refusal their pinned key would have earned.
//
// So the vault's own pattern moves here and all four writers use it. `rename` over an existing path
// is atomic on both targets: on unix by POSIX, and on Windows because `std::fs::rename` maps to
// `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING`.
//
// The editor's file-save path keeps its OWN atomic write (`local::fs::save_atomically`): it writes
// files the user browses, so it hides its temp behind a dotted name and deletes it if the rename
// fails, rather than working inside cmote's own data directory the way these four do.

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

/// Copy `path` to `path.bak`, once, before a migration rewrites it (§110).
///
/// "Once" is the whole point. A migration runs on the first save after an upgrade, and a backup
/// refreshed on every save would be overwritten by the migrated file on the next run — leaving no
/// copy of the ORIGINAL, which is the only thing this backup is for. So an existing `.bak` is left
/// alone. A missing source is not an error either: on a first run there is nothing to preserve.
pub fn back_up_once(path: &Path) -> Result<()> {
	let backup = with_suffix(path, ".bak");
	if backup.exists() || !path.exists() {
		return Ok(());
	}
	std::fs::copy(path, &backup)
		.with_context(|| format!("failed to back up {}", path.display()))?;
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
	use super::{back_up_once, with_suffix, write_atomically};

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

	#[test]
	fn the_backup_keeps_the_original_and_never_the_migrated_copy() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(&path, "the original").unwrap();
		back_up_once(&path).unwrap();
		// The migration writes the new shape, and the next save runs the backup again.
		write_atomically(&path, b"the migrated shape").unwrap();
		back_up_once(&path).unwrap();
		assert_eq!(
			std::fs::read_to_string(with_suffix(&path, ".bak")).unwrap(),
			"the original",
			"a refreshed backup would hold the migrated file and preserve nothing"
		);
	}

	#[test]
	fn backing_up_a_file_that_is_not_there_is_not_an_error() {
		// First run: no store yet, so nothing to preserve and no reason to fail.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		back_up_once(&path).unwrap();
		assert!(!with_suffix(&path, ".bak").exists());
	}
}

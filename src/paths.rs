// paths.rs — where cmote's on-disk data lives (PLAN §11, §14).
//
// cmote writes two files: `known_hosts` (the TOFU host-key store, §8) and
// `targets.json` (the saved connection profiles, §14). Both want the SAME
// directory, resolved the SAME way, so the resolution lives here once rather than
// being duplicated per file. `hostkey` and `profiles` each just join their file
// name onto `data_dir()`.
//
// The rule (§11): prefer a `cmote-data/` folder beside the executable — that keeps
// the whole data set travelling with the app (a USB stick stays self-contained).
// Only when the exe sits somewhere read-only (`Program Files`, `/Applications`) do
// we fall back to the per-user data directory.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// The directory cmote stores its data in (§11). Prefers the portable
/// `cmote-data/` beside the binary when that is writable; otherwise falls back to
/// the per-user data directory (`%LOCALAPPDATA%\cmote\` on Windows,
/// `~/Library/Application Support/cmote/` on macOS). The returned directory is
/// guaranteed to exist — callers can join a file name and read/write it.
pub fn data_dir() -> Result<PathBuf> {
	if let Some(dir) = writable_portable_dir() {
		return Ok(dir);
	}

	let dir = user_data_dir()?;
	std::fs::create_dir_all(&dir).context("failed to create the fallback data directory")?;
	Ok(dir)
}

// The per-user data directory is resolved with plain `std` per OS — no `dirs`
// crate (`ponytail:` §11). Only the two supported targets have a branch; any other
// target fails to compile with the message below rather than silently misbehaving.

/// Windows fallback: `%LOCALAPPDATA%\cmote` (e.g. `C:\Users\<user>\AppData\Local\cmote`).
#[cfg(windows)]
fn user_data_dir() -> Result<PathBuf> {
	let base = std::env::var_os("LOCALAPPDATA")
		.map(PathBuf::from)
		.context("no writable data directory (LOCALAPPDATA is not set)")?;
	Ok(base.join("cmote"))
}

/// macOS fallback: `~/Library/Application Support/cmote` — Apple's convention for
/// app-managed data, resolved from `$HOME`.
#[cfg(target_os = "macos")]
fn user_data_dir() -> Result<PathBuf> {
	let home = std::env::var_os("HOME")
		.map(PathBuf::from)
		.context("no writable data directory (HOME is not set)")?;
	Ok(home.join("Library/Application Support/cmote"))
}

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!(
	"cmote supports only Windows and macOS (PLAN §2); no data-directory fallback is defined for this target"
);

/// Return `cmote-data/` beside the exe if we can actually write there, else
/// `None`. `ponytail:` a create-dir + write-probe is enough to tell portable
/// (USB stick, any folder) from a read-only location like `Program Files`.
fn writable_portable_dir() -> Option<PathBuf> {
	let exe = std::env::current_exe().ok()?;
	let dir = exe.parent()?.join("cmote-data");
	std::fs::create_dir_all(&dir).ok()?;

	let probe = dir.join(".write-probe");
	std::fs::File::create(&probe).ok()?;
	let _ = std::fs::remove_file(&probe);
	Some(dir)
}

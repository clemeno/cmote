// local/path.rs — the one dialect translation a local session needs (PLAN §103).
//
// The file panes speak POSIX. That is not a choice made for them: SFTP puts `/`-separated paths on
// the wire whatever the server runs on, so `explorer` was written against one dialect and every
// path in the tree, the pane, the editor and the transfer queue is a `/`-rooted string
// (`explorer::ROOT`). Windows is not that. A local session therefore needs exactly one thing this
// module provides: a total, testable translation between the panes' dialect and the platform's.
//
// The mapping keeps the panes' single root and still shows every drive:
//
//   /                 the VIRTUAL root — no native path at all; listing it lists the drives
//   /C:               the root of drive C            ->  C:\
//   /C:/Users/cme     a folder on it                 ->  C:\Users\cme
//
// A drive is a directory named `C:` inside `/`, which costs nothing anywhere else: `explorer::join`
// composes it like any other name, `name()` reads it back, and the tree's arithmetic is untouched.
// The alternative — rooting the panes at one drive — would have hidden every other one, and there is
// no honest way for a machine with four drives to show one.
//
// On macOS the two dialects are already the same and every function here is the identity.
//
// [`to_native`] is the security boundary of the local file layer, and it is a boundary in one
// direction only: everything downstream of it (`fs`, `copy`) takes a real `PathBuf` and never a
// string, so a path that this function refuses cannot reach the filesystem at all. What it refuses:
//
//   * a `..` or `.` component — the panes never build one, so its presence means the path came from
//     somewhere else, and a traversal is not worth being tolerant about even among a user's own files;
//   * (Windows only) a `:` anywhere but in the drive — on NTFS that names an alternate data stream,
//     so `notes.txt` and `notes.txt:hidden` are different files and only one of them is the one on
//     screen;
//   * (Windows only) a `\` inside a component — it is a separator there, so a name carrying one
//     would silently address a different depth than the one that was clicked. Neither of these two
//     is a hazard on macOS, where both characters are ordinary in a file name, and refusing them
//     there made a listed file unopenable (§124);
//   * a first component that is not a drive — `/etc` is not a place on Windows, and answering as if
//     it were would put the panes somewhere they cannot be.

use std::path::{Path, PathBuf};

/// The native path for a pane path, or `None` when there is not one.
///
/// `None` is a real answer and not only an error: the virtual root `/` has no native path on
/// Windows, because "all the drives" is not a directory. Callers that list a directory treat it as
/// "ask [`drives`] instead"; callers that read or write a file treat it as a refusal.
#[cfg(windows)]
pub fn to_native(pane: &str) -> Option<PathBuf> {
	let mut parts = pane
		.trim_start_matches('/')
		.split('/')
		.filter(|part| !part.is_empty());
	// The first component must be a drive. Anything else is not a location on this platform, and
	// mapping it onto one would be an invention.
	let drive = parts.next()?;
	if !is_drive(drive) {
		return None;
	}
	// `C:` alone is the drive's ROOT and needs the separator: `C:` on its own means "the working
	// directory on C:" to Windows, which is a different folder and not one the user clicked.
	let mut native = PathBuf::from(format!("{drive}\\"));
	for part in parts {
		if !is_plain_component(part) {
			return None;
		}
		native.push(part);
	}
	Some(native)
}

/// The macOS translation: the dialects agree, so this only vets the components.
#[cfg(target_os = "macos")]
pub fn to_native(pane: &str) -> Option<PathBuf> {
	if !pane.starts_with('/') {
		return None;
	}
	let mut native = PathBuf::from("/");
	for part in pane.split('/').filter(|part| !part.is_empty()) {
		if !is_plain_component(part) {
			return None;
		}
		native.push(part);
	}
	Some(native)
}

/// The pane path for a native one — the inverse of [`to_native`], used to report back what the
/// filesystem answered (a resolved symlink, a walked tree's entries).
///
/// `None` for anything that is not a plain absolute path on a drive: a UNC share (`\\server\share`)
/// has no place in the `/C:` scheme, and a relative path has no place in the panes at all.
#[cfg(windows)]
pub fn to_posix(native: &Path) -> Option<String> {
	// Imported here, not at module scope: `Component` names the pieces of a WINDOWS path and this is
	// the only function that takes one apart, so at module scope it would be unused on macOS (§113).
	use std::path::Component;

	let mut components = native.components();
	// A drive-rooted absolute path is a `Prefix` (the `C:`) followed by a `RootDir` (the `\`).
	// Anything else — a UNC prefix, a relative path, a drive with no root — is refused.
	let Some(Component::Prefix(prefix)) = components.next() else {
		return None;
	};
	let drive = prefix.as_os_str().to_str()?;
	if !is_drive(drive) {
		return None;
	}
	if !matches!(components.next(), Some(Component::RootDir)) {
		return None;
	}
	let mut pane = format!("/{drive}");
	for component in components {
		let Component::Normal(part) = component else {
			// `.` and `..` are refused here for the same reason `to_native` refuses them: the panes
			// address folders, not routes to them.
			return None;
		};
		pane.push('/');
		pane.push_str(part.to_str()?);
	}
	Some(pane)
}

/// The macOS inverse: absolute already, so only the encoding is checked.
#[cfg(target_os = "macos")]
pub fn to_posix(native: &Path) -> Option<String> {
	if !native.is_absolute() {
		return None;
	}
	native.to_str().map(str::to_owned)
}

/// The drives to list for the virtual root, as pane names (`C:`, `D:`).
///
/// `GetLogicalDrives` rather than probing `A:\` through `Z:\` for a directory: a probe is 26 stat
/// calls, and two of them are historically the ones that hurt — asking whether `A:\` is a directory
/// spins up a floppy drive, and an empty optical drive can block for seconds. One bitmask answers
/// the same question without touching a device.
#[cfg(windows)]
pub fn drives() -> Vec<String> {
	// SAFETY: no arguments, no pointers, no allocation — the call reads the OS's own drive bitmask
	// and returns it by value. It cannot fail in a way that needs handling: a zero mask (which
	// cannot happen on a running Windows) simply lists nothing.
	let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
	(0..26u8)
		.filter(|bit| mask & (1 << bit) != 0)
		.map(|bit| format!("{}:", char::from(b'A' + bit)))
		.collect()
}

/// macOS has one root and no drives beside it, so the virtual root IS the root and lists nothing of
/// its own — `to_native("/")` answers `/` there, and the caller never asks this.
#[cfg(target_os = "macos")]
pub fn drives() -> Vec<String> {
	Vec::new()
}

/// [`to_native`] with the refusal already worded for a user (§103).
///
/// Lives here, beside the translation it wraps, because it was written twice — identically — in
/// `local::fs` and `local::copy`, which is how a boundary comes to be crossed two ways. Every
/// caller that must have a native path goes through this one, so the message a refusal shows and
/// the check that produced it can never drift apart.
pub fn native(pane: &str) -> Result<PathBuf, String> {
	to_native(pane).ok_or_else(|| format!("{pane} is not a path on this machine."))
}

/// Whether this pane path is the virtual root — the one path with no native equivalent on Windows,
/// and the one whose listing is the drive letters.
pub fn is_virtual_root(pane: &str) -> bool {
	cfg!(windows) && pane.trim_matches('/').is_empty()
}

/// Whether a pane component names a drive: one ASCII letter and a colon, nothing else.
#[cfg(windows)]
fn is_drive(part: &str) -> bool {
	let bytes = part.as_bytes();
	bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// Whether a component is a plain file name — safe to push onto a native path.
///
/// The refusals are the module note's list, and they split by platform because two of the four
/// hazards are Windows' own (§124). It is deliberately a whitelist of shape rather than a blacklist
/// of characters: everything the panes produce passes, and everything else is refused without cmote
/// having to have thought of it.
///
/// **The split is a fix, not a relaxation.** `\` and `:` are refused because on Windows one is a
/// separator and the other opens an alternate data stream — both stated in the module note as
/// Windows facts. On macOS neither is true: both are ordinary characters in a file name, and
/// refusing them made a file the panes had LISTED impossible to open, because `to_posix` does not
/// vet components and `to_native` did. A listed row that answers "not a path on this machine" is a
/// visible bug, and it was invisible here because it needed the macOS arm to show up.
///
/// `cfg!` rather than a `#[cfg]` pair on purpose: both arms then compile and are linted everywhere,
/// and the test below can assert the platform difference as `== cfg!(windows)` instead of asserting
/// one half and hoping about the other (§115).
fn is_plain_component(part: &str) -> bool {
	let ordinary = !part.is_empty() && part != "." && part != ".." && !part.contains('/');
	// The two Windows-only hazards. `/` above is refused on both, being the separator either way.
	let windows_safe = !cfg!(windows) || (!part.contains('\\') && !part.contains(':'));
	ordinary && windows_safe
}

/// Where a local session's file panes open: the user's own folder, as a pane path.
///
/// A remote session opens at `/` because that is the top of the server and there is nothing better
/// to say. Here there is: the user is standing in their own home directory the moment the shell
/// starts, and opening the panes at the drive list would mean two clicks before the first useful
/// folder. Falls back to the virtual root when the home directory cannot be resolved or does not
/// translate, which is the honest answer rather than a guess at another folder.
pub fn home() -> String {
	native_home()
		.as_deref()
		.and_then(to_posix)
		.unwrap_or_else(|| crate::explorer::ROOT.to_owned())
}

/// The user's home directory from the platform's own variables — no `dirs` crate, the same rule
/// `paths.rs` follows. `pub(super)` because the pty starts the shell there too (`local::pty`), and
/// the two must agree: a shell standing somewhere the panes are not showing is a session that opens
/// looking at two different places.
pub(super) fn native_home() -> Option<PathBuf> {
	// `USERPROFILE` on Windows, `HOME` on macOS. Checked in that order rather than by `cfg`, because
	// a Git Bash session sets `HOME` on Windows too and either answer is the user's own folder.
	["USERPROFILE", "HOME"]
		.iter()
		.filter_map(std::env::var_os)
		.map(PathBuf::from)
		.find(|path| path.is_dir())
}

#[cfg(test)]
mod tests {
	use std::path::{Path, PathBuf};

	use super::{drives, home, is_plain_component, is_virtual_root, to_native, to_posix};

	/// The pane paths THIS platform can hold, for the tests that must hold on both arms (§124).
	///
	/// The two dialects have nothing in common to test with — `/C:` is meaningless on macOS and
	/// `/etc` is not a place on Windows — so the shared tests below take their sample from here and
	/// assert the same PROPERTY of it either way. That is the difference between a test that covers
	/// both arms and two tests that each cover one: this one cannot be green on Windows while the
	/// macOS arm is broken, because there is only one of it (§115).
	fn examples() -> [&'static str; 4] {
		if cfg!(windows) {
			["/C:", "/C:/Users", "/C:/Users/cme/Documents", "/D:/data"]
		} else {
			["/", "/Users", "/Users/cme/Documents", "/etc"]
		}
	}

	#[cfg(windows)]
	#[test]
	fn a_drive_is_a_folder_inside_the_virtual_root() {
		// The whole scheme in three lines: `/` is nowhere, `/C:` is a drive's root, and everything
		// below it is an ordinary path. `C:` alone would mean "the working directory on C:" to
		// Windows, so the trailing separator is not cosmetic.
		assert_eq!(to_native("/"), None);
		assert_eq!(to_native("/C:"), Some(PathBuf::from(r"C:\")));
		assert_eq!(
			to_native("/C:/Users/cme"),
			Some(PathBuf::from(r"C:\Users\cme"))
		);
		assert!(is_virtual_root("/"));
		assert!(!is_virtual_root("/C:"));
	}

	/// Every path the panes hold came back from the filesystem through `to_posix`, and every path they
	/// send goes out through `to_native`. If the two disagreed anywhere, a folder would list under one
	/// name and refuse to open under it.
	///
	/// Runs on both arms since §124. It was `#[cfg(windows)]` and its sample was the `/C:` scheme
	/// spelled out, which left the macOS pair — the security boundary of the local file layer — with
	/// no round-trip test at all.
	#[test]
	fn the_translation_round_trips() {
		for pane in examples() {
			let native = to_native(pane).expect("translates");
			assert_eq!(to_posix(&native).as_deref(), Some(pane), "{pane}");
		}
	}

	/// A pane path that is not rooted is not a location at all, on either platform, and the refusal
	/// happens for a different reason on each: no drive can be its first component on Windows, no
	/// leading `/` on macOS. One assertion, both arms, so neither can rot unseen (§115).
	#[test]
	fn an_unrooted_pane_path_is_refused() {
		assert_eq!(to_native(""), None);
		assert_eq!(to_native("Users/cme"), None);
		assert_eq!(to_native("etc"), None);
	}

	/// A relative native path has no place in the panes either way — refused before the prefix check
	/// on Windows, by `is_absolute` on macOS.
	#[test]
	fn a_relative_native_path_has_no_pane_path() {
		assert_eq!(to_posix(Path::new("Users")), None);
		assert_eq!(to_posix(Path::new("")), None);
	}

	/// A name the platform will hand back but Rust cannot read as UTF-8 stops here rather than
	/// reaching the panes, which hold `String`s. Both arms build one the only way their own OS allows
	/// — an unpaired surrogate on Windows, a stray byte on macOS — and both must answer `None`, which
	/// is the `to_str()?` in each half of `to_posix` (§124).
	#[test]
	fn a_name_that_is_not_utf8_has_no_pane_path() {
		#[cfg(windows)]
		let native = {
			use std::os::windows::ffi::OsStringExt;
			// `C:\` followed by a lone high surrogate: a real Windows name, not valid UTF-16.
			PathBuf::from(std::ffi::OsString::from_wide(&[
				0x0043, 0x003A, 0x005C, 0xD800,
			]))
		};
		#[cfg(target_os = "macos")]
		let native = {
			use std::os::unix::ffi::OsStrExt;
			// `/` followed by a byte no UTF-8 sequence starts with: a real POSIX name.
			PathBuf::from(std::ffi::OsStr::from_bytes(&[b'/', 0xFF]))
		};
		assert_eq!(to_posix(&native), None);
	}

	/// The virtual root and its drive list are one decision seen from two sides: `/` is a place with
	/// no native path exactly where there are drives to list instead. Asserted as one equality so the
	/// two halves cannot drift apart on the platform nobody is looking at.
	#[test]
	fn the_virtual_root_exists_exactly_where_the_drives_do() {
		assert_eq!(is_virtual_root("/"), cfg!(windows));
		assert_eq!(to_native("/").is_none(), cfg!(windows));
		assert_eq!(!drives().is_empty(), cfg!(windows));
	}

	#[cfg(windows)]
	#[test]
	fn nothing_but_a_drive_can_be_the_first_component() {
		// `/etc` is not a place on this platform. Answering as if it were would put the panes
		// somewhere that cannot exist, so it is refused rather than mapped onto a drive.
		assert_eq!(to_native("/etc/passwd"), None);
		assert_eq!(to_native("/CC:/x"), None);
		assert_eq!(to_native("/1:/x"), None);
		assert_eq!(to_native(""), None);
	}

	#[cfg(windows)]
	#[test]
	fn a_unc_share_has_no_place_in_the_scheme() {
		// It has no drive letter, so there is nothing to be the first component. Refused rather than
		// half-translated — the panes would show a path that cannot be reopened.
		assert_eq!(to_posix(Path::new(r"\\server\share\file")), None);
	}

	#[test]
	fn a_traversal_never_reaches_the_filesystem() {
		// The panes never build a `..`, so one arriving means the path came from elsewhere. This is
		// the refusal that makes every function downstream able to take a real `PathBuf` and stop
		// thinking about it.
		assert!(!is_plain_component(".."));
		assert!(!is_plain_component("."));
		assert!(!is_plain_component(""));
		if cfg!(windows) {
			assert_eq!(to_native("/C:/Users/../Windows"), None);
			assert_eq!(to_native("/C:/Users/./cme"), None);
		} else {
			assert_eq!(to_native("/Users/../etc"), None);
		}
	}

	/// The two refusals that belong to Windows alone, asserted on both arms as one equality (§115,
	/// §124).
	///
	/// On NTFS `notes.txt` and `notes.txt:hidden` are two files, and only one of them is the row that
	/// was clicked; a backslash is a separator there, so a name carrying one addresses a different
	/// depth. On macOS both characters are ordinary in a file name, and refusing them made a file the
	/// panes had listed refuse to open — which is what the `== cfg!(windows)` form here would have
	/// caught the day it was written.
	#[test]
	fn the_windows_only_hazards_are_refused_only_on_windows() {
		assert_eq!(!is_plain_component("notes.txt:hidden"), cfg!(windows));
		assert_eq!(!is_plain_component(r"a\b"), cfg!(windows));
		// And on macOS such a name survives the whole round trip, which is the property that was
		// broken: a listed row has to be openable.
		if !cfg!(windows) {
			let pane = "/Users/cme/a:b\\c";
			let native = to_native(pane).expect("an ordinary macOS name translates");
			assert_eq!(to_posix(&native).as_deref(), Some(pane));
		}
	}

	#[test]
	fn a_name_with_a_separator_in_it_is_refused_everywhere() {
		// `/` is the separator on both platforms, so this refusal has no arms.
		assert!(!is_plain_component("a/b"));
		// Everything ordinary passes — including spaces, dots and non-ASCII, which are all legal.
		for name in ["notes.txt", "My Documents", ".gitconfig", "café", "a-b_c.1"] {
			assert!(is_plain_component(name), "{name} is an ordinary name");
		}
	}

	#[test]
	fn the_panes_open_somewhere_that_translates_back() {
		// Whatever `home` answers is fed straight into a listing, so it has to be a path `to_native`
		// accepts. The fallback is the root, which is why this holds even with no home directory.
		let start = home();
		assert!(start.starts_with('/'), "{start} is a pane path");
		assert!(
			is_virtual_root(&start) || to_native(&start).is_some(),
			"{start} must be listable"
		);
	}
}

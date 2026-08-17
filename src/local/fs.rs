// local/fs.rs — the file panes' answers for a local session (PLAN §103).
//
// The remote file layer has three backends behind one seam (§46): SFTP, shell commands, and a plain
// refusal. This is a fourth, and by far the simplest — the files are right here, so every question is
// one `std::fs` call. What matters is that it answers in exactly the same events as the other three
// (`DirListed`, `FilesChunk`, `MakeDirDone`, …), because that is what lets the tree, the pane, the
// details popup, the editor and the preview run unchanged over a local session.
//
// Three rules are worth stating, because each one is a decision and not a translation:
//
//   * **Every operation runs on a blocking task.** `std::fs` blocks, and the session loop must stay
//     free to move terminal bytes while a crowded directory is being read — the same reason the SFTP
//     listings are spawned (§18). So each function here hands its work to `spawn_blocking` and reports
//     through the event channel when it is done.
//   * **A listing does NOT follow symlinks, and the tree does.** That is not an inconsistency, it is
//     the remote behaviour copied exactly: the pane keeps a link's own kind because resolving every
//     one costs a call per link in a crowded folder (§19), and the tree resolves them because it has
//     to know whether a link is a branch it can open (§18).
//   * **What Windows cannot answer is left empty, never invented.** `Meta` has an owner, a group and
//     a `drwxr-xr-x` permission word, all of which are unix facts. Windows has an owner (a SID behind
//     a security descriptor) and nothing resembling the other two. Filling `mode` with something
//     derived from the read-only attribute would put a plausible, wrong sentence in the details popup,
//     so those fields stay `None` there and the popup simply shows the size and the time — which is
//     the same thing it does for a remote whose server volunteered no attributes (§20).

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use super::path;
use crate::bridge::SshEvent;
use crate::files::{self, Entry, Kind, Meta, Zone};

/// List the folders inside `pane`, for the explorer tree (§18) — the local twin of
/// `ssh::browse::list`.
///
/// The virtual root is the one path with no directory behind it: on Windows it lists the DRIVES, and
/// that is the whole reason the tree has a root to hang them off (`local::path`).
pub async fn list(events: &mpsc::Sender<SshEvent>, pane: String) {
	if path::is_virtual_root(&pane) {
		let _ = events
			.send(SshEvent::DirListed {
				path: pane,
				dirs: path::drives(),
			})
			.await;
		return;
	}
	let events = events.clone();
	spawn(async move {
		match blocking(&pane, read_dirs).await {
			Ok(dirs) => {
				let _ = events.send(SshEvent::DirListed { path: pane, dirs }).await;
			}
			Err(reason) => {
				eprintln!("listing {pane} failed: {reason}");
				let _ = events
					.send(SshEvent::DirFailed { path: pane, reason })
					.await;
			}
		}
	});
}

/// List every entry inside `pane`, for the files pane (§19) — the local twin of
/// `ssh::browse::list_all`, batches and all.
///
/// The drives are listed here too, as directory rows with no metadata: a drive has a size, but it is
/// the size of the volume rather than of a folder, and the pane's size column means "how big is this
/// thing" — so answering with the volume's capacity would be a different question's answer.
pub async fn list_all(events: &mpsc::Sender<SshEvent>, pane: String, request: u64) {
	if path::is_virtual_root(&pane) {
		let drives = path::drives().into_iter().map(drive_entry).collect();
		send_batches(events, request, drives).await;
		return;
	}
	let events = events.clone();
	spawn(async move {
		match blocking(&pane, read_entries).await {
			Ok(mut entries) => {
				files::sort(&mut entries);
				send_batches(&events, request, entries).await;
			}
			Err(reason) => {
				eprintln!("files listing failed: {reason}");
				let _ = events.send(SshEvent::FilesFailed { request, reason }).await;
			}
		}
	});
}

/// Create a folder (§18). The destination is checked FIRST, for the same reason the SFTP path checks
/// it: `create_dir` on an occupied path gives a terse OS error, and "already exists" is the reason
/// worth showing.
pub async fn make_dir(events: &mpsc::Sender<SshEvent>, pane: String) {
	let events = events.clone();
	spawn(async move {
		let event = match blocking(&pane, |native| {
			if native.symlink_metadata().is_ok() {
				return Err(format!(
					"{} already exists — nothing was created.",
					pane_of(native)
				));
			}
			std::fs::create_dir(native)
				.map_err(|error| format!("Could not create the folder: {error}"))
		})
		.await
		{
			Ok(()) => SshEvent::MakeDirDone(pane),
			Err(reason) => SshEvent::MakeDirFailed(reason),
		};
		let _ = events.send(event).await;
	});
}

/// Rename a folder (§18). Like the SFTP path, an occupied destination is refused rather than
/// replaced: `std::fs::rename` on Windows replaces a file without asking, and a folder quietly
/// replaced is not something the user can undo.
pub async fn rename(events: &mpsc::Sender<SshEvent>, from: String, to: String) {
	let events = events.clone();
	spawn(async move {
		let event = match rename_now(&from, &to).await {
			Ok(()) => SshEvent::RenameDone { from, to },
			Err(reason) => SshEvent::RenameFailed(reason),
		};
		let _ = events.send(event).await;
	});
}

/// The rename itself, split out so both paths are one expression and the guard cannot be skipped.
async fn rename_now(from: &str, to: &str) -> Result<(), String> {
	let source = native(from)?;
	let destination = native(to)?;
	let taken = to.to_owned();
	tokio::task::spawn_blocking(move || {
		if destination.symlink_metadata().is_ok() {
			return Err(format!("{taken} already exists — nothing was renamed."));
		}
		std::fs::rename(&source, &destination).map_err(|error| format!("Could not rename: {error}"))
	})
	.await
	.map_err(|_| joined())?
}

/// Delete entries, folders and their contents included (§18).
///
/// A symlink is unlinked and never followed, exactly as the SFTP walk does — following one would
/// delete whatever it points at, which is somewhere the user did not select. On Windows a directory
/// symlink and a junction both need `remove_dir` rather than `remove_file`, so the two are told apart
/// before the removal rather than by trying one and catching the other.
pub async fn remove(events: &mpsc::Sender<SshEvent>, panes: Vec<String>) {
	let events = events.clone();
	spawn(async move {
		let event = match remove_now(&panes).await {
			Ok(()) => SshEvent::DeleteDone(panes),
			Err(reason) => SshEvent::DeleteFailed(reason),
		};
		let _ = events.send(event).await;
	});
}

/// Remove each target in turn, stopping at the first failure and naming it. A delete that half
/// happened is worth being told about, and the panels re-list either way (§18).
async fn remove_now(panes: &[String]) -> Result<(), String> {
	let mut targets = Vec::with_capacity(panes.len());
	for pane in panes {
		targets.push((pane.clone(), native(pane)?));
	}
	tokio::task::spawn_blocking(move || {
		for (pane, target) in &targets {
			remove_tree(target).map_err(|error| format!("Could not delete {pane}: {error}"))?;
		}
		Ok(())
	})
	.await
	.map_err(|_| joined())?
}

/// Resolve one symlink for the details popup (§20). Reports nothing when it will not resolve, which
/// is what a broken link is — the popup simply has no target line.
pub async fn read_link(events: &mpsc::Sender<SshEvent>, pane: String) {
	let events = events.clone();
	spawn(async move {
		let Ok(target) = blocking(&pane, |native| {
			std::fs::read_link(native).map_err(|error| format!("{error}"))
		})
		.await
		else {
			return;
		};
		// The target is reported as the OS spells it, backslashes and all. It is a value to READ —
		// nothing reopens it as a pane path — and rewriting it into cmote's dialect would show the user
		// something other than what is stored in the link.
		let _ = events
			.send(SshEvent::LinkTarget {
				path: pane,
				target: target.to_string_lossy().into_owned(),
			})
			.await;
	});
}

/// This machine's timezone, for rendering the pane's mtimes (§20).
///
/// The remote path asks the server with `date +'%z %Z'`; there is nothing to ask here, and std can
/// tell the time but not the offset. The label is left EMPTY on purpose rather than filled with
/// Windows' own name for the zone: those are long ("Romance Daylight Time") where the pane's format
/// wants an abbreviation, and inventing "RDT" would be putting a name on screen that no clock uses.
/// An empty label makes `files::with_zone` render the offset alone — `+02:00`, which is unambiguous.
pub async fn report_zone(events: &mpsc::Sender<SshEvent>) {
	let _ = events.send(SshEvent::Zone(zone())).await;
}

/// Read a whole file for a viewer tab (§32, §53), refusing one over `limit` off its metadata before a
/// byte is read — the same order the remote path uses, so a huge file costs a stat and not a read.
pub async fn load(events: &mpsc::Sender<SshEvent>, viewer_id: u64, pane: String, limit: u64) {
	let events = events.clone();
	spawn(async move {
		let read = blocking(&pane, move |native| {
			let meta = native
				.metadata()
				.map_err(|error| format!("Could not read the file: {error}"))?;
			if meta.len() > limit {
				// The remote reader's own wording and its own formatter (`ssh::edit::human_size`), so a
				// file too big to open says the same sentence whichever machine it is on. A second
				// spelling of one refusal is a second thing to keep true.
				return Err(format!(
					"This file is {} — too large to open (limit {}).",
					crate::ssh::edit::human_size(meta.len()),
					crate::ssh::edit::human_size(limit)
				));
			}
			std::fs::read(native).map_err(|error| format!("Could not read the file: {error}"))
		})
		.await;
		let event = match read {
			Ok(bytes) => SshEvent::FileLoaded {
				viewer_id,
				path: pane,
				bytes,
			},
			Err(reason) => SshEvent::FileLoadFailed { viewer_id, reason },
		};
		let _ = events.send(event).await;
	});
}

/// Write the editor's buffer back (§32), atomically: a temp sibling then a rename over the target, so
/// a crash mid-write cannot truncate the user's file. The same shape as the remote save, for the same
/// reason.
pub async fn save(events: &mpsc::Sender<SshEvent>, viewer_id: u64, pane: String, bytes: Vec<u8>) {
	let events = events.clone();
	spawn(async move {
		let event = match blocking(&pane, move |native| write_atomically(native, &bytes)).await {
			Ok(()) => SshEvent::EditSaved {
				viewer_id,
				path: pane,
			},
			Err(reason) => SshEvent::EditSaveFailed { viewer_id, reason },
		};
		let _ = events.send(event).await;
	});
}

/// Write `bytes` to `target` without ever leaving it half-written.
///
/// The temp file is a HIDDEN sibling rather than a file in the OS temp directory, because a rename
/// only replaces atomically within one filesystem — across volumes it degrades to a copy, which is
/// exactly the truncation window this exists to close. Its name carries cmote's own marker so a leak
/// (a crash between the write and the rename) is identifiable rather than mysterious.
fn write_atomically(target: &Path, bytes: &[u8]) -> Result<(), String> {
	let name = target
		.file_name()
		.ok_or_else(|| "That path names no file.".to_owned())?;
	let temp = target.with_file_name(format!(".{}.cmote-save", name.to_string_lossy()));
	std::fs::write(&temp, bytes).map_err(|error| format!("Could not write the file: {error}"))?;
	std::fs::rename(&temp, target).map_err(|error| {
		// The temp file would otherwise be left beside the user's file forever.
		let _ = std::fs::remove_file(&temp);
		format!("Could not replace the file: {error}")
	})
}

/// The folder names inside a directory (§18), links to folders included.
///
/// A link's own type says nothing about what it points at, so each one is followed with a `metadata`
/// call and kept only if the target is a directory. `||` short-circuits, so a real directory costs
/// nothing extra and only a link pays — the same trade the SFTP listing makes.
fn read_dirs(directory: &Path) -> Result<Vec<String>, String> {
	let mut dirs = Vec::new();
	for entry in listing(directory)? {
		let Ok(kind) = entry.file_type() else {
			continue;
		};
		let is_dir = kind.is_dir()
			|| (kind.is_symlink() && entry.path().metadata().is_ok_and(|meta| meta.is_dir()));
		if is_dir {
			dirs.push(entry.file_name().to_string_lossy().into_owned());
		}
	}
	Ok(dirs)
}

/// Every entry inside a directory, with what the OS volunteered about it (§19, §20).
///
/// `DirEntry::metadata` is deliberate: it does NOT follow symlinks, which is what the pane wants (a
/// link keeps its own kind), and on Windows it is free — the data came back with the directory scan,
/// so a folder of ten thousand files costs one enumeration rather than ten thousand stats.
fn read_entries(directory: &Path) -> Result<Vec<Entry>, String> {
	let mut entries = Vec::new();
	for entry in listing(directory)? {
		let name = entry.file_name().to_string_lossy().into_owned();
		// `.` and `..` never appear in a std listing, but the model drops them at ingest anyway
		// (`explorer::is_dot_link`) — checked here too so both backends agree about what a listing
		// contains rather than relying on one platform's enumerator.
		if crate::explorer::is_dot_link(&name) {
			continue;
		}
		let meta = entry.metadata().ok();
		entries.push(Entry {
			kind: kind_of(meta.as_ref()),
			meta: meta.as_ref().map(meta_of).unwrap_or_default(),
			name,
		});
	}
	Ok(entries)
}

/// One directory's entries, or a reason. The reason is the OS's own words, which is what makes it
/// actionable — "Access is denied" is a different problem from "The system cannot find the path".
fn listing(directory: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
	let read = std::fs::read_dir(directory).map_err(|error| format!("{error}"))?;
	// Entries that fail mid-enumeration are skipped rather than failing the listing: one unreadable
	// name should not empty a folder of a thousand good ones.
	Ok(read.filter_map(Result::ok).collect())
}

/// What kind of thing an entry is, as the pane classifies it (§19). Metadata that could not be read
/// leaves it a plain file — the row still shows, with the name it has and nothing claimed about it.
fn kind_of(meta: Option<&std::fs::Metadata>) -> Kind {
	match meta {
		Some(meta) if meta.is_symlink() => Kind::Link,
		Some(meta) if meta.is_dir() => Kind::Dir,
		_ => Kind::File,
	}
}

/// What the OS said about an entry beyond its name and kind (§20).
///
/// The owner, the group and the permission word are unix facts. On macOS they are read from the mode
/// and the ids; on Windows they stay `None` (see the module note) — the popup then shows the size and
/// the time, exactly as it does for a server that volunteered no attributes.
fn meta_of(meta: &std::fs::Metadata) -> Meta {
	Meta {
		// A directory's "size" is its entry, not its contents, on both platforms — reported as the OS
		// reports it rather than suppressed, so the column says the same thing `ls -l` would.
		size: Some(meta.len()),
		mtime: mtime_of(meta),
		owner: owner_of(meta),
		group: group_of(meta),
		mode: mode_of(meta),
	}
}

/// Last modification as seconds since the epoch — the model's shape (SFTP v3's), so both backends
/// hand the pane the same kind of number and one formatter renders both.
///
/// Saturating rather than wrapping: the field is a `u32`, so a file stamped past 2106 (or one with a
/// corrupt time, which is how this actually happens) pins to the maximum rather than folding round to
/// 1970 and sorting to the top of the pane.
fn mtime_of(meta: &std::fs::Metadata) -> Option<u32> {
	let modified = meta.modified().ok()?;
	let since_epoch = modified
		.duration_since(std::time::UNIX_EPOCH)
		.ok()?
		.as_secs();
	Some(u32::try_from(since_epoch).unwrap_or(u32::MAX))
}

/// The unix permission word, `ls -l` style. macOS only: see the module note.
#[cfg(target_os = "macos")]
fn mode_of(meta: &std::fs::Metadata) -> Option<String> {
	use std::os::unix::fs::MetadataExt;
	Some(files::format_mode(meta.mode()))
}

/// Windows has no permission word to render, so none is claimed.
#[cfg(windows)]
fn mode_of(_meta: &std::fs::Metadata) -> Option<String> {
	None
}

/// The owning user id. A number rather than a name — resolving it needs the password database, which
/// is a lookup per entry, and the pane's remote path falls back to numeric ids for exactly the same
/// reason when a server sends no names.
#[cfg(target_os = "macos")]
fn owner_of(meta: &std::fs::Metadata) -> Option<String> {
	use std::os::unix::fs::MetadataExt;
	Some(meta.uid().to_string())
}

#[cfg(windows)]
fn owner_of(_meta: &std::fs::Metadata) -> Option<String> {
	None
}

#[cfg(target_os = "macos")]
fn group_of(meta: &std::fs::Metadata) -> Option<String> {
	use std::os::unix::fs::MetadataExt;
	Some(meta.gid().to_string())
}

#[cfg(windows)]
fn group_of(_meta: &std::fs::Metadata) -> Option<String> {
	None
}

/// A drive as a pane row: a folder with nothing claimed about it (see [`list_all`]).
fn drive_entry(name: String) -> Entry {
	Entry {
		name,
		kind: Kind::Dir,
		meta: Meta::default(),
	}
}

/// Remove one entry whatever it is — the local twin of the SFTP walk (§18).
///
/// The order of the three questions is the whole function. A SYMLINK is looked at with
/// `symlink_metadata`, so it is seen as itself and unlinked, never followed. Then it matters whether
/// the link pointed at a folder: on Windows a directory symlink and a junction are removed with
/// `remove_dir` and a file symlink with `remove_file`, and using the wrong one fails. Only a real
/// directory is descended into.
fn remove_tree(root: &Path) -> std::io::Result<()> {
	let meta = root.symlink_metadata()?;
	if meta.is_symlink() {
		// `remove_dir` on a directory link removes the LINK, not the target — that is what the OS call
		// does, and it is why the target is never walked.
		return if root.is_dir() {
			std::fs::remove_dir(root)
		} else {
			std::fs::remove_file(root)
		};
	}
	if meta.is_dir() {
		// Breadth-first into a heap-held frontier rather than a recursive call, so a deep tree costs
		// memory and not stack — the same shape as `ssh::browse::remove_subtree`.
		let mut dirs = vec![root.to_path_buf()];
		let mut files: Vec<PathBuf> = Vec::new();
		let mut frontier = vec![root.to_path_buf()];
		while let Some(dir) = frontier.pop() {
			for entry in std::fs::read_dir(&dir)?.filter_map(Result::ok) {
				let child = entry.path();
				let kind = entry.file_type()?;
				// A link to a folder is a FILE here — its own type is a link, so it is unlinked rather
				// than descended into. Getting this wrong would delete the target's contents.
				if kind.is_dir() {
					dirs.push(child.clone());
					frontier.push(child);
				} else {
					files.push(child);
				}
			}
		}
		for file in &files {
			remove_tree(file)?;
		}
		// Deepest first: `dirs` is in discovery order (a parent before its children), so reversing it
		// takes the children before the parent — which is what `remove_dir` needs.
		for dir in dirs.iter().rev() {
			std::fs::remove_dir(dir)?;
		}
		return Ok(());
	}
	std::fs::remove_file(root)
}

/// Send a listing as `FilesChunk` batches, the last one flagged `done` — the same contract the SFTP
/// path has, including the single empty batch for an empty directory, which is what tells the pane to
/// stop waiting.
async fn send_batches(events: &mpsc::Sender<SshEvent>, request: u64, entries: Vec<Entry>) {
	let total = entries.len();
	let mut sent = 0;
	loop {
		let batch = entries[sent..(sent + files::BATCH).min(total)].to_vec();
		sent += batch.len();
		let done = sent == total;
		let delivered = events
			.send(SshEvent::FilesChunk {
				request,
				entries: batch,
				done,
			})
			.await
			.is_ok();
		if done || !delivered {
			return;
		}
	}
}

/// Translate a pane path and run `work` on the real path, off the async runtime.
///
/// The two halves are inseparable on purpose: the translation is the boundary that refuses a
/// traversal or an alternate-data-stream name (`local::path`), and putting it in front of every
/// blocking call is what lets `work` take a `&Path` and stop thinking about where it came from.
async fn blocking<T, F>(pane: &str, work: F) -> Result<T, String>
where
	T: Send + 'static,
	F: FnOnce(&Path) -> Result<T, String> + Send + 'static,
{
	let native = native(pane)?;
	tokio::task::spawn_blocking(move || work(&native))
		.await
		.map_err(|_| joined())?
}

/// The native path for a pane path, or the one refusal this layer can give before touching the disk.
fn native(pane: &str) -> Result<PathBuf, String> {
	path::to_native(pane).ok_or_else(|| format!("{pane} is not a path on this machine."))
}

/// A pane path back out of a native one, for a message that names what it acted on. Falls back to the
/// OS spelling, which is still the right thing to show a user even if it is not cmote's dialect.
fn pane_of(native: &Path) -> String {
	path::to_posix(native).unwrap_or_else(|| native.to_string_lossy().into_owned())
}

/// What is said when a blocking task did not come back — the runtime is shutting down, or the task
/// panicked. Never blamed on the file: the operation genuinely has no result to report.
fn joined() -> String {
	"The operation did not finish.".to_owned()
}

/// Run one file operation detached, so the session loop keeps moving terminal bytes (§18).
fn spawn<F>(work: F)
where
	F: std::future::Future<Output = ()> + Send + 'static,
{
	tokio::spawn(work);
}

/// This machine's zone, read from Windows itself.
///
/// `Bias` is minutes WEST of UTC (Windows defines UTC = local + bias), and the model wants minutes
/// east, hence the negation. The daylight correction is chosen by the id the call returns rather than
/// computed from a date: the OS has already decided which rule is in force right now, and that
/// decision is the one the file times on screen were stamped under.
#[cfg(windows)]
fn zone() -> Zone {
	use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};

	// SAFETY: `GetTimeZoneInformation` fills a caller-owned struct of exactly this type and reads
	// nothing else. A zeroed value is a valid one to hand it, and the returned id says whether it
	// wrote anything worth reading.
	let mut info: TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
	let id = unsafe { GetTimeZoneInformation(&mut info) };
	const TIME_ZONE_ID_INVALID: u32 = u32::MAX;
	const TIME_ZONE_ID_DAYLIGHT: u32 = 2;
	if id == TIME_ZONE_ID_INVALID {
		// No zone to be had: the default renders as UTC, which is at least never wrong about the
		// instant, only about the wall clock (§20).
		return Zone::default();
	}
	let bias = if id == TIME_ZONE_ID_DAYLIGHT {
		info.Bias + info.DaylightBias
	} else {
		// Both the standard id and the "unknown rules" id take the standard bias: an OS that will not
		// say whether daylight time is in force has, by saying so, told us not to add an hour.
		info.Bias + info.StandardBias
	};
	Zone {
		offset: -bias,
		label: String::new(),
	}
}

/// macOS has no equivalent one-call answer in the bindings cmote already carries, so the zone is left
/// at UTC there. `ponytail:` the times are then right about the instant and wrong about the wall
/// clock, which is the same state a remote with no `date` leaves the pane in (§20) — but it is a
/// visible gap on the machine the user is sitting at, and it is the thinner half of §103.
#[cfg(target_os = "macos")]
fn zone() -> Zone {
	Zone::default()
}

#[cfg(test)]
mod tests {
	use super::{
		Kind, Meta, drive_entry, kind_of, meta_of, mtime_of, read_dirs, read_entries, remove_tree,
		write_atomically, zone,
	};
	use std::path::Path;

	/// A directory holding a file, a subfolder and a nested file — enough for every walk below.
	fn tree() -> tempfile::TempDir {
		let root = tempfile::tempdir().expect("a temp dir");
		std::fs::write(root.path().join("a.txt"), b"alpha").expect("a file");
		std::fs::create_dir(root.path().join("sub")).expect("a folder");
		std::fs::write(root.path().join("sub/b.txt"), b"beta").expect("a nested file");
		root
	}

	#[test]
	fn the_tree_lists_folders_and_the_pane_lists_everything() {
		// The two listings answer different questions off the same directory, exactly as the remote
		// pair do: the tree wants branches, the pane wants rows.
		let root = tree();
		assert_eq!(read_dirs(root.path()).expect("lists"), vec!["sub"]);
		let entries = read_entries(root.path()).expect("lists");
		let mut names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
		names.sort_unstable();
		assert_eq!(names, vec!["a.txt", "sub"]);
	}

	#[test]
	fn a_listing_carries_the_size_and_the_time_it_already_had() {
		// The extras ride along with the enumeration rather than being asked for per entry — that is
		// what keeps a crowded folder cheap (§20).
		let root = tree();
		let entries = read_entries(root.path()).expect("lists");
		let file = entries
			.iter()
			.find(|entry| entry.name == "a.txt")
			.expect("the file is listed");
		assert_eq!(file.kind, Kind::File);
		assert_eq!(file.meta.size, Some(5));
		assert!(file.meta.mtime.is_some(), "a real file has a real mtime");
	}

	#[test]
	fn an_unreadable_entry_leaves_the_row_rather_than_the_listing_empty() {
		// One name that will not stat must not empty a folder of a thousand good ones, and a row with
		// nothing claimed about it is still the honest answer for that name.
		assert_eq!(kind_of(None), Kind::File);
		let drive = drive_entry("C:".to_owned());
		assert_eq!(drive.kind, Kind::Dir);
		assert_eq!(drive.meta, Meta::default());
	}

	#[test]
	fn a_deep_tree_is_removed_from_the_inside_out() {
		// `remove_dir` needs an empty directory, so the walk has to reach the leaves first. Depth is
		// held on the heap, not the stack, so this holds for a tree deeper than a recursion would take.
		let root = tempfile::tempdir().expect("a temp dir");
		let mut deep = root.path().to_path_buf();
		for level in 0..40 {
			deep = deep.join(format!("level-{level}"));
		}
		std::fs::create_dir_all(&deep).expect("a deep tree");
		std::fs::write(deep.join("leaf.txt"), b"x").expect("a leaf");
		let target = root.path().join("level-0");
		remove_tree(&target).expect("removes");
		assert!(!target.exists(), "the whole tree went");
		assert!(root.path().exists(), "and nothing above it did");
	}

	#[test]
	fn a_save_that_fails_never_leaves_a_truncated_file() {
		// The file is replaced by a rename, so the user's file is either the old one or the new one and
		// never a half-written one. The temp sibling is what makes that true, so it must not survive.
		let root = tempfile::tempdir().expect("a temp dir");
		let target = root.path().join("notes.txt");
		std::fs::write(&target, b"before").expect("the original");
		write_atomically(&target, b"after").expect("writes");
		assert_eq!(std::fs::read(&target).expect("reads"), b"after");
		let leftovers: Vec<String> = std::fs::read_dir(root.path())
			.expect("lists")
			.filter_map(Result::ok)
			.map(|entry| entry.file_name().to_string_lossy().into_owned())
			.filter(|name| name.contains("cmote-save"))
			.collect();
		assert!(leftovers.is_empty(), "the temp sibling was cleaned up");
	}

	#[test]
	fn a_save_refuses_a_path_that_names_no_file() {
		// A root has no file name to build a temp sibling beside, so it is refused rather than written
		// somewhere improvised.
		let error = write_atomically(Path::new("/"), b"x").expect_err("refuses");
		assert!(error.contains("names no file"), "{error}");
	}

	#[test]
	fn a_corrupt_timestamp_pins_rather_than_folding_round_to_1970() {
		// The model's field is a `u32`. A saturating conversion keeps such a file at the END of a
		// time sort, where it is visible; wrapping would put it at the top, where it looks ordinary.
		let root = tempfile::tempdir().expect("a temp dir");
		let file = root.path().join("now.txt");
		std::fs::write(&file, b"x").expect("a file");
		let meta = file.metadata().expect("metadata");
		let stamp = mtime_of(&meta).expect("a real file has a real mtime");
		assert!(stamp > 1_700_000_000, "a plausible epoch second: {stamp}");
		// And the whole `Meta` is built from that one metadata read, never a second stat.
		assert_eq!(meta_of(&meta).mtime, Some(stamp));
	}

	#[test]
	fn the_zone_is_an_offset_and_never_an_invented_abbreviation() {
		// An empty label makes the pane render the offset alone (`files::with_zone`). A Windows zone
		// name is a long sentence, and shortening one to "RDT" would put a name on screen that no
		// clock uses.
		let zone = zone();
		assert!(zone.label.is_empty() || zone.label == "UTC");
		assert!(
			zone.offset > -24 * 60 && zone.offset < 24 * 60,
			"an offset in minutes east of UTC: {}",
			zone.offset
		);
	}
}

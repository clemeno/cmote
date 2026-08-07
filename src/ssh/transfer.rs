// ssh/transfer.rs — the shared spine of a recursive folder transfer (PLAN §17, §19).
//
// A tree upload (`upload::transfer_tree`) and a tree download (`download::fetch_tree`) are
// mirror images: walk one side's directory tree, recreate it on the other, and copy each
// file across. Two things are the SAME whichever way the bytes flow, so they live here
// rather than being written twice:
//
//   * the SHAPE of a walked tree (`TreePlan`) — the directories to create (parents first)
//     and the files to copy (with their sizes, so the progress bar has a real total); and
//   * the CONFLICT protocol — when a file's destination is already taken, the transfer
//     parks, asks the GUI (`SshEvent::TransferConflict`), and waits for the answer
//     (`SshCommand::ResolveConflict`, delivered here as a `ConflictChoice`). A sticky
//     "…all" answer is remembered so the rest of the tree is settled without asking again.
//
// The directory-walking itself is NOT here: one side reads the local filesystem
// (`tokio::fs`) and the other the remote one (SFTP), so each owns its own walk. This
// module only defines the plan they both fill in and the decision they both make per file.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use russh_sftp::protocol::FileAttributes;
use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;

/// A walked directory tree, ready to recreate on the other side (§17, §19). Paths are kept as
/// their component lists relative to the tree's root — not joined strings — so each side can
/// rebuild them with its own separator (`/` on the remote, the OS's locally) without re-parsing.
#[derive(Debug, Default)]
pub(crate) struct TreePlan {
	/// Every directory in the tree, PARENTS BEFORE CHILDREN, so creating them in order never
	/// asks for a folder whose parent does not exist yet. The root itself is the empty path.
	pub dirs: Vec<Vec<String>>,
	/// Every regular file — its place in the tree, its size, and the source metadata to stamp
	/// onto the copy once it lands (§17).
	pub files: Vec<PlannedFile>,
	/// Symbolic links the walk could NOT follow (§17). A link is followed by default — what it
	/// points at is copied as if it sat there itself — so this counts only the two cases where
	/// there is nothing to follow it to: a **dangling** link, whose target is not there to read,
	/// and a link that **leads back up its own tree** ([`loops_back`]), which walking into would
	/// never end. Counted rather than fatal: one bad link should not lose the other ten thousand
	/// files, so the transfer finishes and says how many it left behind.
	pub skipped_links: usize,
}

/// One regular file in a walked tree (§17): where it goes, how big it is, and the source's
/// timestamps and permission bits so the copy can be stamped to match once it is written. Every
/// metadata field is optional — a source that does not expose one (a Windows file has no Unix
/// mode; a filesystem may refuse an access time) leaves it `None`, and the stamp then omits that
/// attribute rather than inventing one.
#[derive(Debug)]
pub(crate) struct PlannedFile {
	/// The file's path relative to the tree root, as a component list each side joins its own way.
	pub rel: Vec<String>,
	/// The byte count the progress bar totals over.
	pub size: u64,
	/// Seconds since the Unix epoch of the source's last modification, if known.
	pub mtime: Option<u32>,
	/// Seconds since the Unix epoch of the source's last access, if known — kept because SFTP
	/// carries it in the same attribute as mtime, so an upload stamp must send the two together.
	pub atime: Option<u32>,
	/// The source's Unix permission bits (`& 0o7777`), if it has any — always `None` from a
	/// Windows source, which has no Unix mode to carry.
	pub mode: Option<u32>,
}

impl TreePlan {
	/// The total bytes to copy — what `SshEvent::TransferProgress` reports progress against.
	pub fn total(&self) -> u64 {
		self.files.iter().map(|file| file.size).sum()
	}
}

/// Whether following a directory symlink would walk the tree into itself (§17) — the one question
/// that decides whether a link is followed or only counted.
///
/// `target` is where the link points, `here` is the directory the link was found in, and both are
/// CANONICAL paths (`realpath` on the remote, `canonicalize` locally), so every link on the way to
/// either one is already resolved. Written that way the rule is short: a link is a cycle exactly
/// when its target is the directory holding it, or one above it. Walking in would come straight
/// back out through the same link, and again, forever. Every other link — sideways into a sibling
/// folder, or right out of the tree altogether — leads somewhere that ends, so it is followed. That
/// costs a folder reached two ways being copied twice, which is what `cp -L` does too, and which a
/// user can see and undo; a walk that never returns is neither.
///
/// The comparison is by PATH COMPONENT, not by text: `/home/ab` must not read as sitting inside
/// `/home/a`, which a plain string prefix would claim it does. The local walk asks the same question
/// of `std::path::Path::starts_with`, which already compares that way — this is the remote's `/`
/// paths, which have no `Path` to ask.
pub(crate) fn loops_back(target: &str, here: &str) -> bool {
	let mut here = here.split('/').filter(|part| !part.is_empty());
	// Every component of the target must be the next one of `here` for the target to be an
	// ancestor. A target LONGER than `here` runs `here` out and fails on the `None`, which is the
	// right answer: something deeper cannot contain what it sits in.
	target
		.split('/')
		.filter(|part| !part.is_empty())
		.all(|part| here.next() == Some(part))
}

/// A collision answer that outlives the one file it was given for (§17): "overwrite everything"
/// or "skip everything" from here on. Remembered by the transfer so a `*All` answer settles the
/// rest of the tree with no further prompts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Sticky {
	Overwrite,
	Skip,
}

/// What to do with ONE colliding file, once every "…all" sticky policy has been resolved down to
/// a plain per-file action. This is what the transfer acts on: write over the original, write a
/// free copy beside it, leave it alone, or stop the whole transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAction {
	Overwrite,
	KeepBoth,
	Skip,
	Cancel,
}

/// How a copy loop ended when it was NOT an error (§16 cancel/resume). `Done` is the whole file
/// across; `Cancelled` is the user pressing the status bar's ✕ mid-flight — the loop notices the
/// shared flag, deletes the partial it was writing, and stops. A real I/O failure is the third
/// outcome, but it travels as the loop's `Err`, not here: a failure mid-flight keeps its partial so
/// the transfer can be resumed, whereas a cancel throws it away, so the two must not be confused.
/// A failure with no partial to keep is a fourth ending again — see [`Refused`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyOutcome {
	Done,
	Cancelled,
}

/// A failure the destination refused outright, before there was anywhere to put a single byte
/// (§16, §17) — uploading into a folder the account cannot write to, saving into a protected local
/// one, or a `mkdir` the server says no to.
///
/// The two ways a transfer can fail are not the same ending, and the status bar must not offer the
/// same thing for both. A copy that stopped MID-FLIGHT left a partial on the far side, so a Resume
/// is real work: it sends only the tail that is still missing. A `create` answered with "permission
/// denied" left NOTHING — there is no partial, and a Resume would run that very same refused create
/// again. Offering it puts a button that can only fail where the plain "it did not happen" belongs,
/// and leaves the transfer looking half-done when it never started.
///
/// So the few places that make or open the destination wrap their failure in this, and the
/// reporting end reads it back with [`was_refused`] to end the transfer cleanly instead: the notice
/// says what the server said, the batch behind it carries on, and no Resume appears.
#[derive(Debug)]
pub(crate) struct Refused(anyhow::Error);

impl std::fmt::Display for Refused {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		// `{:#}` prints the whole chain on one line. Plain `{}` would print only the outermost
		// context — "could not create /srv/data/notes.txt on the server" — which is the half that
		// says what was attempted, dropping the half that says why it was refused.
		write!(formatter, "{:#}", self.0)
	}
}

impl std::error::Error for Refused {}

/// Mark a failure as the destination's refusal — see [`Refused`]. A function rather than a method
/// so a call site reads `.map_err(transfer::refused)` on the very line that makes or opens the
/// destination: past that line there IS somewhere for bytes to survive, so past that line a failure
/// is resumable again.
pub(crate) fn refused(error: anyhow::Error) -> anyhow::Error {
	anyhow::Error::new(Refused(error))
}

/// Whether this failure was the destination's refusal. Walks the whole chain rather than testing
/// only the outermost link, so a caller between the refusal and the report stays free to add its
/// own context without hiding the class underneath it.
pub(crate) fn was_refused(error: &anyhow::Error) -> bool {
	error.chain().any(|link| link.is::<Refused>())
}

/// Where a copy should begin, given whether this is a resume and how big the destination already
/// is (§16). A fresh transfer always starts at zero — its `create` truncates, so nothing is
/// carried over. A resume sizes the destination against the source it is continuing:
///
///   * as many bytes as the source (or more) are already there, so it finished before the
///     interruption — `Skip` it rather than re-send a byte; and
///   * fewer bytes means that many survived, so continue writing from exactly there (`At`).
///
/// An absent or empty partial (`None` / `Some(0)`) is `At(0)` — a plain fresh send — so a resume
/// that reaches a file never started simply sends it whole. This is size-based, so it trusts the
/// bytes already written to be the file's own prefix (`ponytail:` no checksum — an SFTP append is
/// the same assumption `curl -C -` and rsync's naive mode make).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start {
	Skip,
	At(u64),
}

/// Decide where a copy starts (§16) — see [`Start`]. Pure, so the resume arithmetic is unit-tested
/// without a server: the two file systems only differ in how they read `dest_size`, not in what
/// the number means.
pub(crate) fn resume_start(resume: bool, dest_size: Option<u64>, source_size: u64) -> Start {
	if !resume {
		return Start::At(0);
	}
	match dest_size {
		None | Some(0) => Start::At(0),
		Some(have) if have >= source_size => Start::Skip,
		Some(have) => Start::At(have),
	}
}

/// Whole seconds since the Unix epoch for a wall-clock time (§17) — the unit both SFTP's mtime /
/// atime and our local stamp work in. A time before the epoch has no representation in SFTP's
/// unsigned field, so it reads as `None` and the caller omits the timestamp rather than sending a
/// wrapped one; a far-future time is clamped to the field's ceiling for the same reason. Pure, so
/// the conversion is unit-tested against fixed instants.
pub(crate) fn epoch_secs(time: SystemTime) -> Option<u32> {
	time.duration_since(UNIX_EPOCH)
		.ok()
		.map(|elapsed| elapsed.as_secs().min(u32::MAX as u64) as u32)
}

/// Build the attributes an upload stamps onto the freshly written remote file (§17), setting only
/// the metadata we actually have. Permissions go on only when the source carried a Unix mode.
/// Timestamps are the subtle part: SFTP carries access and modification time as ONE attribute, so
/// setting the modification time forces an access time alongside it, and an omitted-but-flagged
/// field goes on the wire as zero — which would reset the file's access time to 1970. So whenever
/// there is an mtime we also send an atime (the source's real one if we read it, otherwise the
/// mtime itself), and when there is no mtime we send neither. Pure, so this field-selection logic
/// is unit-tested without a server.
pub(crate) fn upload_stamp(
	mtime: Option<u32>,
	atime: Option<u32>,
	mode: Option<u32>,
) -> FileAttributes {
	let mut attrs = FileAttributes::empty();
	attrs.permissions = mode;
	if let Some(modified) = mtime {
		attrs.mtime = Some(modified);
		attrs.atime = Some(atime.unwrap_or(modified));
	}
	attrs
}

/// Decide what to do about a file whose destination `name` is already taken (§17, §19).
///
/// A sticky policy already in force answers without troubling the user; otherwise the transfer
/// parks: it sends one `TransferConflict` and awaits the reply on `answers`. A `*All` answer is
/// recorded in `sticky` before being applied to this file, so every later collision is settled
/// silently. A dropped channel — the GUI went away, or the session is tearing down — reads as
/// Cancel, which stops the walk cleanly rather than blocking forever.
pub(crate) async fn resolve(
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	sticky: &mut Option<Sticky>,
	name: &str,
) -> FileAction {
	if let Some(policy) = sticky {
		return match policy {
			Sticky::Overwrite => FileAction::Overwrite,
			Sticky::Skip => FileAction::Skip,
		};
	}

	// Ask once, then wait. If the event cannot even be sent the GUI is gone, so cancel.
	if events
		.send(SshEvent::TransferConflict {
			name: name.to_owned(),
		})
		.await
		.is_err()
	{
		return FileAction::Cancel;
	}

	match answers.recv().await {
		Some(ConflictChoice::Overwrite) => FileAction::Overwrite,
		Some(ConflictChoice::KeepBoth) => FileAction::KeepBoth,
		Some(ConflictChoice::Skip) => FileAction::Skip,
		Some(ConflictChoice::OverwriteAll) => {
			*sticky = Some(Sticky::Overwrite);
			FileAction::Overwrite
		}
		Some(ConflictChoice::SkipAll) => {
			*sticky = Some(Sticky::Skip);
			FileAction::Skip
		}
		// An explicit cancel, or the channel closing under us, both end the transfer.
		Some(ConflictChoice::Cancel) | None => FileAction::Cancel,
	}
}

/// Build a remote path from the tree's destination root and a relative component list (§17,
/// §19), joining POSIX-style the way every remote path does. Shared so the two directions build
/// the same string from the same parts.
pub(crate) fn remote_join(root: &str, rel: &[String]) -> String {
	let mut path = root.to_owned();
	for component in rel {
		path = explorer::join(&path, component);
	}
	path
}

/// The same for a local path, using THIS OS's own separator — a component list joined onto a
/// local root.
pub(crate) fn local_join(root: &Path, rel: &[String]) -> PathBuf {
	let mut path = root.to_path_buf();
	for component in rel {
		path.push(component);
	}
	path
}

#[cfg(test)]
mod tests {
	use super::{Start, loops_back, resume_start};

	#[test]
	fn a_link_pointing_at_its_own_folder_loops() {
		// The shortest cycle there is: `/srv/data/here` -> `/srv/data`. Walking in lists the same
		// folder again, finds the same link, and never stops.
		assert!(loops_back("/srv/data", "/srv/data"));
		assert!(loops_back("/srv", "/srv/data"));
		assert!(loops_back("/", "/srv/data/deep"));
	}

	#[test]
	fn a_link_pointing_sideways_or_out_is_followed() {
		// A sibling, a cousin, and somewhere else entirely: each ends, so each is copied.
		assert!(!loops_back("/srv/other", "/srv/data"));
		assert!(!loops_back("/srv/data/inner", "/srv/data"));
		assert!(!loops_back("/etc", "/srv/data"));
	}

	#[test]
	fn a_shared_prefix_is_not_a_shared_path() {
		// `/srv/da` is not `/srv/data`'s parent — it is a different folder whose name happens to
		// start the same way. A text prefix would call this a cycle and drop a good link.
		assert!(!loops_back("/srv/da", "/srv/data"));
		assert!(!loops_back("/srv/data2", "/srv/data"));
	}

	#[test]
	fn trailing_and_doubled_slashes_do_not_change_the_answer() {
		// Servers spell the same path several ways; the components are what the rule is about.
		assert!(loops_back("/srv/data/", "/srv/data"));
		assert!(loops_back("//srv//data", "/srv/data/"));
	}

	#[test]
	fn a_fresh_transfer_always_starts_at_zero() {
		// Not a resume: whatever is at the destination is about to be truncated, so the size of
		// it never matters — the copy sends the whole file from the top.
		assert_eq!(resume_start(false, None, 100), Start::At(0));
		assert_eq!(resume_start(false, Some(40), 100), Start::At(0));
		assert_eq!(resume_start(false, Some(100), 100), Start::At(0));
	}

	#[test]
	fn a_resume_with_no_partial_sends_the_whole_file() {
		// The resume reached a file the interruption never got to: nothing on the far side (or an
		// empty stub) means send it whole, from zero.
		assert_eq!(resume_start(true, None, 100), Start::At(0));
		assert_eq!(resume_start(true, Some(0), 100), Start::At(0));
	}

	#[test]
	fn a_resume_continues_a_partial_from_where_it_stopped() {
		assert_eq!(resume_start(true, Some(40), 100), Start::At(40));
		assert_eq!(resume_start(true, Some(1), 100), Start::At(1));
	}

	#[test]
	fn a_resume_skips_a_file_already_fully_there() {
		// Exactly the source's size means it landed in full before the interruption; larger than
		// the source can only be a stale, different file, and re-sending would not improve it —
		// either way there is nothing to append, so skip it.
		assert_eq!(resume_start(true, Some(100), 100), Start::Skip);
		assert_eq!(resume_start(true, Some(140), 100), Start::Skip);
	}

	#[test]
	fn epoch_seconds_count_from_1970() {
		use std::time::Duration;
		assert_eq!(super::epoch_secs(super::UNIX_EPOCH), Some(0));
		assert_eq!(
			super::epoch_secs(super::UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
			Some(1_700_000_000)
		);
	}

	#[test]
	fn a_time_before_the_epoch_has_no_stamp() {
		use std::time::Duration;
		// SFTP's timestamp field is unsigned, so a pre-1970 time cannot be represented — better to
		// carry no timestamp than a wrapped-around one.
		let before = super::UNIX_EPOCH
			.checked_sub(Duration::from_secs(1))
			.unwrap();
		assert_eq!(super::epoch_secs(before), None);
	}

	#[test]
	fn an_upload_stamp_sets_only_what_it_is_given() {
		let attrs = super::upload_stamp(Some(100), Some(50), Some(0o644));
		assert_eq!(attrs.mtime, Some(100));
		assert_eq!(attrs.atime, Some(50));
		assert_eq!(attrs.permissions, Some(0o644));
	}

	#[test]
	fn an_upload_stamp_backfills_a_missing_access_time_from_the_mtime() {
		// SFTP couples atime with mtime; sending an mtime alone would zero the access time, so an
		// absent atime borrows the mtime rather than going out as 1970.
		let attrs = super::upload_stamp(Some(100), None, None);
		assert_eq!(attrs.mtime, Some(100));
		assert_eq!(attrs.atime, Some(100));
		assert_eq!(attrs.permissions, None);
	}

	#[test]
	fn an_ordinary_failure_is_not_a_refusal() {
		// The default direction, and the safe one: a failure says nothing about the destination
		// unless the code that touched the destination said so, and an unmarked failure keeps the
		// resume it always had.
		let error = anyhow::anyhow!("write failed");
		assert!(!super::was_refused(&error));
	}

	#[test]
	fn a_refused_destination_is_recognised_as_one() {
		let error = super::refused(anyhow::anyhow!("could not create /srv/data on the server"));
		assert!(super::was_refused(&error));
	}

	#[test]
	fn a_refusal_survives_the_context_added_on_its_way_up() {
		// The mark goes on where the `create` failed, but the report that reads it back is several
		// frames higher, and every frame in between may add its own context. The class must ride
		// under all of it — a refusal that a `.context()` could hide would be a Resume button back.
		use anyhow::Context;
		let error = Err::<(), _>(super::refused(anyhow::anyhow!("permission denied")))
			.context("could not upload notes.txt")
			.unwrap_err();
		assert!(super::was_refused(&error));
	}

	#[test]
	fn a_refusal_reads_out_with_the_reason_that_was_refused() {
		// What the status bar ends up showing. The wrapped chain prints whole, so the notice names
		// both what was attempted and why the server said no — one without the other is not
		// actionable.
		use anyhow::Context;
		let inner = Err::<(), _>(anyhow::anyhow!("permission denied"))
			.context("could not create /srv/data/notes.txt on the server")
			.unwrap_err();
		let error = super::refused(inner);
		assert_eq!(
			error.to_string(),
			"could not create /srv/data/notes.txt on the server: permission denied"
		);
	}

	#[test]
	fn an_upload_stamp_with_no_mtime_sends_no_timestamp_at_all() {
		// No mtime means no timestamp pair — never a lone atime, which would drag a zero mtime
		// onto the wire and reset the modification time.
		let attrs = super::upload_stamp(None, Some(50), None);
		assert_eq!(attrs.mtime, None);
		assert_eq!(attrs.atime, None);
		assert_eq!(attrs.permissions, None);
	}
}

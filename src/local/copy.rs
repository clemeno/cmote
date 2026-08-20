// local/copy.rs — the transfer queue's work when both ends are this machine (PLAN §103).
//
// A remote session's transfers move bytes across a network: "upload" and "download" are two
// directions because the two ends are two machines. On a LOCAL session they are not — a copy from a
// folder the user picked in a dialog into the folder the pane is showing is the same operation
// whichever way round it is described. So this module has ONE copy engine and ONE tree walker, and
// the only thing the direction decides is which events the outcome is reported in, because the GUI's
// transfer queue listens for `UploadDone` in one case and `DownloadDone` in the other (§17, §19).
//
// Nothing here re-derives the transfer rules. The pieces that make a transfer a transfer —
// where a resume picks up (`resume_start`), when a progress event is worth sending (`Ticker`), what
// the six-way collision answers mean and which of them stick (`resolve`, `Sticky`), and the
// difference between a failure that can be resumed and one that cannot (`refused`) — all live in
// `ssh::transfer`, shared with the two SFTP directions. That module is not about SSH; it is about
// copying, and this is the third and fourth caller. The local tree walk is shared the same way
// (`ssh::upload::walk_local`), symlink cycle rules and all.
//
// One difference from the remote path is deliberate and one is a gap:
//
//   * **Deliberate:** a local copy is refused when the source and the destination are the same file.
//     Across a network that cannot happen; here it can, and the naive answer — open both, read, write
//     — truncates the user's file to nothing before reading a byte of it.
//   * **`ponytail:` a gap.** The copy does NOT carry the source's modification time over, where the
//     SFTP upload does (`stamp_remote`). std can read a file time and not write one, and setting one
//     means `SetFileTime` on Windows and `futimens` on macOS — two more platform calls for a cosmetic
//     property. A local copy therefore lands stamped "now", the same as one made with Explorer's
//     copy-paste or `cp` without `-p`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::path;
use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;
use crate::ssh::transfer::{
	self, CopyOutcome, FileAction, Start, Sticky, Ticker, TreePlan, resume_start,
};
use crate::ssh::upload::walk_local;

/// How much to move per read/write. Larger than the SFTP chunk (32 KiB): that number is bounded by
/// the protocol's packet size, and a local copy has no packets — it is bounded only by how much
/// memory is worth holding per transfer.
const CHUNK: usize = 256 * 1024;

/// Which direction the GUI believes this transfer is going.
///
/// It changes nothing about the copy. It decides only which of the two pairs of terminal events the
/// outcome is reported in, because the transfer queue's state machine listens for the pair belonging
/// to the direction it started (§17, §19) — report the wrong one and the queue never frees its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyDirection {
	Up,
	Down,
}

impl CopyDirection {
	/// The word the status bar uses for this direction, so a message reads as the action the user took
	/// rather than as "copy", which is what both of them actually are.
	fn noun(self) -> &'static str {
		match self {
			Self::Up => "Upload",
			Self::Down => "Download",
		}
	}

	/// The "it worked" event for this direction, carrying where the bytes landed.
	fn done(self, path: String) -> SshEvent {
		match self {
			Self::Up => SshEvent::UploadDone(path),
			Self::Down => SshEvent::DownloadDone(path),
		}
	}

	/// The "it did not work" event for this direction.
	fn failed(self, reason: String) -> SshEvent {
		match self {
			Self::Up => SshEvent::UploadFailed(reason),
			Self::Down => SshEvent::DownloadFailed(reason),
		}
	}
}

/// Copy a file INTO the pane's folder — what the GUI calls an upload (§17).
///
/// `destination` is a pane path (the folder the user dropped onto, plus the file's name); `source` is
/// a real path off the file dialog. The existence check runs first and answers `UploadExists`, so a
/// file already there becomes the same question it is over a network rather than a casualty — the
/// copy below truncates, so by the time a write failed the old contents would be gone.
pub fn upload(
	events: &mpsc::Sender<SshEvent>,
	source: PathBuf,
	destination: String,
	overwrite: bool,
	resume: bool,
	cancel: Arc<AtomicBool>,
) {
	let events = events.clone();
	tokio::spawn(async move {
		let native = match path::native(&destination) {
			Ok(native) => native,
			Err(reason) => {
				let _ = events.send(SshEvent::UploadFailed(reason)).await;
				return;
			}
		};
		// A resume carries `overwrite`, so it skips straight to the copy — where it appends rather
		// than truncates (§16). The file already there is its own interrupted work, not a clash.
		if !overwrite && native.symlink_metadata().is_ok() {
			let _ = events.send(SshEvent::UploadExists(destination)).await;
			return;
		}
		let outcome = one_file(&source, &native, resume, &events, &cancel).await;
		report(&events, CopyDirection::Up, outcome, destination).await;
	});
}

/// Copy a file OUT of the pane's folder — what the GUI calls a download (§19).
///
/// No existence question: the destination came from a save dialog, which has already asked (§21).
pub fn download(
	events: &mpsc::Sender<SshEvent>,
	source: String,
	destination: PathBuf,
	resume: bool,
	cancel: Arc<AtomicBool>,
) {
	let events = events.clone();
	tokio::spawn(async move {
		let native = match path::native(&source) {
			Ok(native) => native,
			Err(reason) => {
				let _ = events.send(SshEvent::DownloadFailed(reason)).await;
				return;
			}
		};
		let landed = destination.to_string_lossy().into_owned();
		let outcome = one_file(&native, &destination, resume, &events, &cancel).await;
		report(&events, CopyDirection::Down, outcome, landed).await;
	});
}

/// Which of `names` are already taken in the pane folder `dir`, each with a free `name-1` beside it
/// (§17) — the answer that lets the GUI ask the "some are already there" question once for a whole
/// batch instead of once per file.
pub fn precheck(events: &mpsc::Sender<SshEvent>, dir: String, names: Vec<String>) {
	let events = events.clone();
	tokio::spawn(async move {
		let native_dir = match path::native(&dir) {
			Ok(native) => native,
			Err(reason) => {
				let _ = events.send(SshEvent::UploadFailed(reason)).await;
				return;
			}
		};
		let mut collisions: Vec<(String, String)> = Vec::new();
		for name in &names {
			if native_dir.join(name).symlink_metadata().is_ok() {
				collisions.push((name.clone(), free_name(&dir, &native_dir, name)));
			}
		}
		let _ = events.send(SshEvent::UploadPrescan { collisions }).await;
	});
}

/// Copy a whole folder INTO the pane's folder (§17), keeping its own name inside the destination.
pub fn upload_tree(
	events: &mpsc::Sender<SshEvent>,
	source: PathBuf,
	destination: String,
	resume: bool,
	answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	let events = events.clone();
	tokio::spawn(async move {
		let mut answers = answers;
		let outcome = match path::native(&destination) {
			Ok(native) => tree(&source, &native, resume, &events, &mut answers, &cancel).await,
			Err(reason) => Err(transfer::mark_refused(anyhow::anyhow!("{reason}"))),
		};
		// The pane path is what is reported, not the native one: the folder tree is about to re-list
		// what it names, and it can only do that in its own dialect.
		let landed = outcome
			.as_ref()
			.ok()
			.and_then(|done| done.as_ref().map(|name| explorer::join(&destination, name)));
		report_tree(&events, CopyDirection::Up, outcome, landed).await;
	});
}

/// Copy a whole folder OUT of the pane's folder (§19) — the mirror of [`upload_tree`].
pub fn download_tree(
	events: &mpsc::Sender<SshEvent>,
	source: String,
	destination: PathBuf,
	resume: bool,
	answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	let events = events.clone();
	tokio::spawn(async move {
		let mut answers = answers;
		let outcome = match path::native(&source) {
			Ok(native) => {
				tree(
					&native,
					&destination,
					resume,
					&events,
					&mut answers,
					&cancel,
				)
				.await
			}
			Err(reason) => Err(transfer::mark_refused(anyhow::anyhow!("{reason}"))),
		};
		let landed = outcome.as_ref().ok().and_then(|done| {
			done.as_ref()
				.map(|name| destination.join(name).to_string_lossy().into_owned())
		});
		report_tree(&events, CopyDirection::Down, outcome, landed).await;
	});
}

/// One file, source to destination, with progress, resume and cancel — the whole copy engine.
///
/// The progress total is the SOURCE's size, and a resume counts what the destination already holds
/// towards it at once, so a resumed bar picks up where it stopped rather than starting again. Cancel
/// is polled between chunks and deletes the partial: a deliberate cancel is final, unlike a failure,
/// which keeps its partial so a Resume has something to continue (§16).
async fn one_file(
	source: &Path,
	destination: &Path,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	let mut ticker = Ticker::default();
	let total = size_of(source).await?;
	let mut run = transfer::CopyRun {
		resume,
		total,
		events,
		ticker: &mut ticker,
		cancel,
	};
	let outcome = stream(source, destination, total, &mut run).await?;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: run.ticker.moved(),
			total,
		})
		.await;
	Ok(outcome)
}

/// Copy one file's bytes, folding them into a run that may be moving a whole tree.
///
/// `size` is this file's own length and is separate from the run because it is the one thing that
/// changes per file: a resume needs it to know whether the destination is already complete, while
/// `run.total` is what the progress bar is measured against.
async fn stream(
	source: &Path,
	destination: &Path,
	size: u64,
	run: &mut transfer::CopyRun<'_>,
) -> Result<CopyOutcome> {
	// The refusal that has no network equivalent: reading and writing one file at once truncates it to
	// nothing first. Checked by resolving both sides, so a link, a `.` on the way or a different
	// spelling of the same path is still caught.
	if same_file(source, destination).await {
		return Err(transfer::mark_refused(anyhow::anyhow!(
			"the source and the destination are the same file"
		)));
	}

	let existing = if run.resume {
		tokio::fs::metadata(destination)
			.await
			.ok()
			.map(|meta| meta.len())
	} else {
		None
	};
	let offset = match resume_start(run.resume, existing, size) {
		// Already fully there from before the interruption: count its bytes so the bar still reaches
		// the end, and move on without opening anything.
		Start::Skip => {
			let sent = run.ticker.settle(size);
			let _ = run
				.events
				.send(SshEvent::TransferProgress {
					sent,
					total: run.total,
				})
				.await;
			return Ok(CopyOutcome::Done);
		}
		Start::At(offset) => offset,
	};

	let mut reader = tokio::fs::File::open(source)
		.await
		.with_context(|| format!("could not open {}", source.display()))?;
	let mut writer = open_at(destination, offset).await?;
	if offset > 0 {
		reader
			.seek(std::io::SeekFrom::Start(offset))
			.await
			.context("could not seek the source to the resume point")?;
		// The bytes already written count towards the running total straight away, and are ANNOUNCED
		// straight away too. Settling alone is not enough: the ticker only emits once another
		// `PROGRESS_STEP` has moved, so a resume of the last few bytes of a file would send its first
		// progress event at the end — and until then the bar would read zero, which says the transfer
		// is starting again from nothing. The SFTP path emits here for the same reason.
		let sent = run.ticker.settle(offset);
		let _ = run
			.events
			.send(SshEvent::TransferProgress {
				sent,
				total: run.total,
			})
			.await;
	}

	let mut buffer = vec![0u8; CHUNK];
	loop {
		// Checked before each read, so a cancel is honoured before any further byte is written and the
		// partial can be dropped cleanly (§16).
		if run.cancel.load(Ordering::Relaxed) {
			drop(writer);
			let _ = tokio::fs::remove_file(destination).await;
			return Ok(CopyOutcome::Cancelled);
		}
		let read = reader.read(&mut buffer).await.context("read failed")?;
		if read == 0 {
			break;
		}
		writer
			.write_all(&buffer[..read])
			.await
			.context("write failed")?;
		if let Some(sent) = run.ticker.advance(read as u64) {
			let _ = run
				.events
				.send(SshEvent::TransferProgress {
					sent,
					total: run.total,
				})
				.await;
		}
	}
	// Flush before reporting success: buffered bytes not yet on disk are not a finished copy.
	writer.shutdown().await.context("close failed")?;
	Ok(CopyOutcome::Done)
}

/// Open the destination to write at `offset`.
///
/// A zero offset is a fresh copy: create, truncating whatever is there — the transfer's ordinary
/// behaviour, and the reason the caller asked its collision question first. A non-zero offset is a
/// resume: open without truncating and seek, so the append lands exactly at the byte the last run
/// reached. Either failure is marked REFUSED (§16): a destination that could not be opened holds no
/// partial to continue from, and asking again would be refused identically, so there is no Resume to
/// offer. This is the commonest failure by far — copying into a folder the user cannot write.
async fn open_at(destination: &Path, offset: u64) -> Result<tokio::fs::File> {
	if offset == 0 {
		return tokio::fs::File::create(destination)
			.await
			.with_context(|| format!("could not create {}", destination.display()))
			.map_err(transfer::mark_refused);
	}
	let mut file = tokio::fs::OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(false)
		.open(destination)
		.await
		.with_context(|| format!("could not open {} to resume", destination.display()))
		.map_err(transfer::mark_refused)?;
	file.seek(std::io::SeekFrom::Start(offset))
		.await
		.with_context(|| {
			format!(
				"could not seek {} to the resume point",
				destination.display()
			)
		})?;
	Ok(file)
}

/// Whether two paths are the same file on disk.
///
/// Compared after `canonicalize`, so a symlink, a `..` on the way or a different case on a
/// case-insensitive volume all resolve to the same answer. A path that will not canonicalize is not
/// the same file as anything: the destination usually does not exist yet, which is the whole ordinary
/// case, so a failure here has to read as "different" and not as "cannot tell".
async fn same_file(source: &Path, destination: &Path) -> bool {
	let (Ok(left), Ok(right)) = (
		tokio::fs::canonicalize(source).await,
		tokio::fs::canonicalize(destination).await,
	) else {
		return false;
	};
	left == right
}

/// Copy a folder and everything under it, merging into whatever is already at the destination and
/// asking about each colliding FILE (§17, §19).
///
/// Returns the name the folder landed under, or `None` when the user cancelled partway. The name
/// rather than the whole path, because the two callers spell a path in different dialects and each
/// can compose its own.
async fn tree(
	source: &Path,
	destination: &Path,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	cancel: &Arc<AtomicBool>,
) -> Result<Option<String>> {
	let name = source
		.file_name()
		.and_then(std::ffi::OsStr::to_str)
		.context("the folder has no name to copy under")?
		.to_owned();
	let root = destination.join(&name);
	let plan: TreePlan = walk_local(source)
		.await
		.with_context(|| format!("could not read {}", source.display()))?;
	let total = plan.total();

	// Every directory before any file goes into one. `plan.dirs` is parents-before-children, so a
	// folder is never asked for before the one holding it.
	ensure_dir(&root).await?;
	for rel in &plan.dirs {
		ensure_dir(&transfer::local_join(&root, rel)).await?;
	}

	let mut ticker = Ticker::default();
	let mut sticky: Option<Sticky> = None;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: ticker.moved(),
			total,
		})
		.await;

	let mut run = transfer::CopyRun {
		resume,
		total,
		events,
		ticker: &mut ticker,
		cancel,
	};
	for file in &plan.files {
		let planned = transfer::local_join(&root, &file.rel);
		let leaf = file.rel.last().map_or("", String::as_str);
		// A resume never prompts (§16): a destination already there is this transfer's own earlier
		// work, which `stream` size-compares and appends to, not a fresh collision.
		let target = if !resume && planned.symlink_metadata().is_ok() {
			match transfer::resolve(events, answers, &mut sticky, leaf).await {
				FileAction::Overwrite => planned,
				FileAction::KeepBoth => {
					let dir = planned.parent().unwrap_or(&root).to_path_buf();
					dir.join(free_leaf(&dir, leaf))
				}
				FileAction::Skip => {
					// Count the skipped bytes as handled so the bar still reaches the end.
					let sent = run.ticker.settle(file.size);
					let _ = events
						.send(SshEvent::TransferProgress { sent, total })
						.await;
					continue;
				}
				FileAction::Cancel => return Ok(None),
			}
		} else {
			planned
		};
		let from = transfer::local_join(source, &file.rel);
		// A cancel mid-file drops that file's partial and stops the whole tree (§16); the files
		// already fully copied stay, mirroring a single-file cancel keeping nothing but its own.
		if stream(&from, &target, file.size, &mut run).await? == CopyOutcome::Cancelled {
			return Ok(None);
		}
	}

	let _ = events
		.send(SshEvent::TransferProgress {
			sent: run.ticker.moved(),
			total,
		})
		.await;
	if plan.skipped_links > 0 {
		eprintln!(
			"folder copy could not follow {} symlink(s)",
			plan.skipped_links
		);
	}
	Ok(Some(name))
}

/// Create a directory unless it is already there, so a copy MERGES into an existing folder rather
/// than failing on it (§17). Refused rather than interrupted for the same reason as `open_at`: every
/// directory is made before a byte is copied, so a failure here means nothing has moved.
async fn ensure_dir(path: &Path) -> Result<()> {
	if path.is_dir() {
		return Ok(());
	}
	tokio::fs::create_dir_all(path)
		.await
		.with_context(|| format!("could not create {}", path.display()))
		.map_err(transfer::mark_refused)
}

/// The first free `name-1.ext`, `name-2.ext`… in `dir`, as a pane path — the "keep both" destination
/// for a name already taken (§17).
///
/// The candidate's SHAPE comes from `explorer::free_candidate`, shared with every other backend that
/// answers this question, so "keep both" produces the same names everywhere. Running out of tries
/// returns the last candidate unchecked, exactly as the SFTP version does: the copy re-creates the
/// file anyway, so the worst a hundred-deep collision costs is one file overwritten by its own
/// transfer rather than a wrong name.
fn free_name(pane_dir: &str, native_dir: &Path, name: &str) -> String {
	explorer::join(pane_dir, &free_leaf(native_dir, name))
}

/// The same, as a bare file name — what the tree walk needs, since it composes native paths.
fn free_leaf(native_dir: &Path, name: &str) -> String {
	for attempt in 1..=explorer::FREE_NAME_TRIES {
		let candidate = explorer::free_candidate(name, attempt);
		if native_dir.join(&candidate).symlink_metadata().is_err() {
			return candidate;
		}
	}
	explorer::free_candidate(name, explorer::FREE_NAME_TRIES)
}

/// A source file's length, or the reason it could not be read. Fatal rather than guessed: a progress
/// bar measured against a made-up total is worse than no bar.
async fn size_of(source: &Path) -> Result<u64> {
	Ok(tokio::fs::metadata(source)
		.await
		.with_context(|| format!("could not read {}", source.display()))?
		.len())
}

/// Turn one file copy's outcome into exactly one terminal event (§16, §17, §19).
///
/// The three cases are the ones the transfer queue distinguishes, and they are distinguished by what
/// is left on disk: a cancel deleted its partial, so it is final and neutral; a refusal never created
/// one, so it is final and an error; anything else KEPT its partial, so it is offered as a Resume.
async fn report(
	events: &mpsc::Sender<SshEvent>,
	direction: CopyDirection,
	outcome: Result<CopyOutcome>,
	landed: String,
) {
	let event = match outcome {
		Ok(CopyOutcome::Done) => direction.done(landed),
		Ok(CopyOutcome::Cancelled) => direction.failed(format!("{} cancelled.", direction.noun())),
		Err(error) if transfer::was_refused(&error) => {
			eprintln!("local copy refused: {error:#}");
			direction.failed(format!("{} failed: {error}", direction.noun()))
		}
		Err(error) => {
			eprintln!("local copy interrupted: {error:#}");
			SshEvent::TransferInterrupted {
				message: format!(
					"{} interrupted: {error} — Resume to continue.",
					direction.noun()
				),
			}
		}
	};
	let _ = events.send(event).await;
}

/// The same for a whole-folder copy. `landed` is `None` exactly when the outcome is a cancel, which
/// is why the cancel arm reads it off the outcome rather than off a missing path.
async fn report_tree(
	events: &mpsc::Sender<SshEvent>,
	direction: CopyDirection,
	outcome: Result<Option<String>>,
	landed: Option<String>,
) {
	let event = match outcome {
		Ok(Some(_)) => direction.done(landed.unwrap_or_default()),
		Ok(None) => direction.failed(format!(
			"Folder {} cancelled.",
			direction.noun().to_lowercase()
		)),
		Err(error) if transfer::was_refused(&error) => {
			eprintln!("local folder copy refused: {error:#}");
			direction.failed(format!(
				"Folder {} failed: {error}",
				direction.noun().to_lowercase()
			))
		}
		Err(error) => {
			eprintln!("local folder copy interrupted: {error:#}");
			SshEvent::TransferInterrupted {
				message: format!(
					"Folder {} interrupted: {error} — Resume to continue.",
					direction.noun().to_lowercase()
				),
			}
		}
	};
	let _ = events.send(event).await;
}

#[cfg(test)]
mod tests {
	use super::{CHUNK, CopyDirection, free_leaf, same_file, stream};
	use crate::bridge::SshEvent;
	use crate::ssh::transfer::{self, CopyOutcome, Ticker};
	use std::sync::Arc;
	use std::sync::atomic::AtomicBool;
	use tokio::sync::mpsc;

	/// Run one copy and hand back what happened plus every event it sent.
	async fn copy(
		source: &std::path::Path,
		destination: &std::path::Path,
		resume: bool,
	) -> (anyhow::Result<CopyOutcome>, Vec<SshEvent>) {
		let (tx, mut rx) = mpsc::channel::<SshEvent>(64);
		let size = source.metadata().map_or(0, |meta| meta.len());
		let mut ticker = Ticker::default();
		let cancel = Arc::new(AtomicBool::new(false));
		let mut run = transfer::CopyRun {
			resume,
			total: size,
			events: &tx,
			ticker: &mut ticker,
			cancel: &cancel,
		};
		let outcome = stream(source, destination, size, &mut run).await;
		drop(tx);
		let mut events = Vec::new();
		while let Ok(event) = rx.try_recv() {
			events.push(event);
		}
		(outcome, events)
	}

	#[tokio::test]
	async fn a_file_is_copied_whole() {
		let temp = tempfile::tempdir().unwrap();
		let source = temp.path().join("from.bin");
		let destination = temp.path().join("to.bin");
		// Larger than one chunk, so the loop runs more than once and the tail is not lost.
		// The pattern only has to be non-uniform, so the low byte of the index serves.
		let bytes: Vec<u8> = (0..(CHUNK + 1234))
			.map(|index| u8::try_from(index % 256).expect("a value below 256"))
			.collect();
		std::fs::write(&source, &bytes).unwrap();

		let (outcome, _) = copy(&source, &destination, false).await;

		assert_eq!(outcome.unwrap(), CopyOutcome::Done);
		assert_eq!(std::fs::read(&destination).unwrap(), bytes);
	}

	#[tokio::test]
	async fn copying_a_file_onto_itself_is_refused_before_it_is_truncated() {
		// The one failure a network transfer cannot have. `File::create` would empty the file before a
		// byte of it was read, so the check has to come first — and it resolves both paths, so a
		// different spelling of the same file is caught too.
		let temp = tempfile::tempdir().unwrap();
		let file = temp.path().join("only.txt");
		std::fs::write(&file, b"precious").unwrap();
		let round_about = temp.path().join(".").join("only.txt");

		assert!(same_file(&file, &round_about).await);
		let (outcome, _) = copy(&file, &round_about, false).await;

		let error = outcome.expect_err("refused");
		assert!(crate::ssh::transfer::was_refused(&error), "{error:#}");
		assert_eq!(
			std::fs::read(&file).unwrap(),
			b"precious",
			"and the file still has its contents"
		);
	}

	#[tokio::test]
	async fn a_destination_that_does_not_exist_yet_is_not_the_same_file_as_the_source() {
		// The ordinary case: `canonicalize` fails on the destination, and that has to read as
		// "different" rather than "cannot tell" — otherwise every first copy would be refused.
		let temp = tempfile::tempdir().unwrap();
		let source = temp.path().join("from.txt");
		std::fs::write(&source, b"x").unwrap();
		assert!(!same_file(&source, &temp.path().join("not-yet.txt")).await);
	}

	#[tokio::test]
	async fn a_resume_appends_only_the_missing_tail() {
		// The resume rule (§16): the destination's size says where to pick up, and the bytes already
		// there count towards the progress total at once so the bar does not drop back to zero.
		let temp = tempfile::tempdir().unwrap();
		let source = temp.path().join("from.txt");
		let destination = temp.path().join("to.txt");
		std::fs::write(&source, b"0123456789").unwrap();
		std::fs::write(&destination, b"01234").unwrap();

		let (outcome, events) = copy(&source, &destination, true).await;

		assert_eq!(outcome.unwrap(), CopyOutcome::Done);
		assert_eq!(std::fs::read(&destination).unwrap(), b"0123456789");
		// The first progress event already accounts for the five bytes that were there.
		let first = events
			.iter()
			.find_map(|event| match event {
				SshEvent::TransferProgress { sent, .. } => Some(*sent),
				_ => None,
			})
			.unwrap_or(0);
		assert!(first >= 5, "the resumed bytes were counted: {first}");
	}

	#[tokio::test]
	async fn a_destination_already_complete_is_skipped_and_still_counted() {
		// Nothing to send, but the bar must still reach the end — otherwise a resumed batch stalls at
		// 90% forever with every file already in place.
		let temp = tempfile::tempdir().unwrap();
		let source = temp.path().join("from.txt");
		let destination = temp.path().join("to.txt");
		std::fs::write(&source, b"same").unwrap();
		std::fs::write(&destination, b"same").unwrap();

		let (outcome, events) = copy(&source, &destination, true).await;

		assert_eq!(outcome.unwrap(), CopyOutcome::Done);
		assert!(events.iter().any(|event| matches!(
			event,
			SshEvent::TransferProgress { sent, total } if sent == total && *total == 4
		)));
	}

	#[tokio::test]
	async fn a_cancel_deletes_the_partial_rather_than_leaving_it_to_resume() {
		// A cancel is a choice and is final; a failure is an accident and is resumable. The difference
		// on disk is exactly this (§16).
		let temp = tempfile::tempdir().unwrap();
		let source = temp.path().join("from.bin");
		let destination = temp.path().join("to.bin");
		std::fs::write(&source, vec![7u8; CHUNK * 2]).unwrap();
		let (tx, _rx) = mpsc::channel::<SshEvent>(64);
		let cancel = Arc::new(AtomicBool::new(true));
		let mut ticker = Ticker::default();
		let mut run = transfer::CopyRun {
			resume: false,
			total: CHUNK as u64 * 2,
			events: &tx,
			ticker: &mut ticker,
			cancel: &cancel,
		};

		let outcome = stream(&source, &destination, CHUNK as u64 * 2, &mut run).await;

		assert_eq!(outcome.unwrap(), CopyOutcome::Cancelled);
		assert!(!destination.exists(), "the partial went with the cancel");
	}

	#[test]
	fn keep_both_finds_the_first_free_name_beside_the_taken_one() {
		// The candidate shape is `explorer::free_candidate`, shared with every other backend, so
		// "keep both" produces the same names on a local session as on a remote one.
		let temp = tempfile::tempdir().unwrap();
		std::fs::write(temp.path().join("notes.txt"), b"a").unwrap();
		assert_eq!(free_leaf(temp.path(), "notes.txt"), "notes-1.txt");
		std::fs::write(temp.path().join("notes-1.txt"), b"b").unwrap();
		assert_eq!(free_leaf(temp.path(), "notes.txt"), "notes-2.txt");
		// A name that is not taken at all keeps its own spelling — no candidate is invented.
		assert_eq!(free_leaf(temp.path(), "fresh.txt"), "fresh-1.txt");
	}

	#[test]
	fn each_direction_reports_in_the_events_its_own_queue_listens_for() {
		// The only thing the direction decides. Report the wrong pair and the transfer queue never
		// frees its slot, so every later transfer in that tab is stuck behind a finished one.
		assert!(matches!(
			CopyDirection::Up.done("x".to_owned()),
			SshEvent::UploadDone(_)
		));
		assert!(matches!(
			CopyDirection::Down.done("x".to_owned()),
			SshEvent::DownloadDone(_)
		));
		assert!(matches!(
			CopyDirection::Up.failed("x".to_owned()),
			SshEvent::UploadFailed(_)
		));
		assert!(matches!(
			CopyDirection::Down.failed("x".to_owned()),
			SshEvent::DownloadFailed(_)
		));
	}
}

// ssh/upload.rs — send a local file to the remote over SFTP (PLAN §17, §46).
//
// The interactive shell is a pty channel: perfect for keystrokes, wrong for file
// bytes (everything is echoed back, binary needs encoding, and the terminal would
// have to render the transfer). SSH's answer is a second channel running the **sftp
// subsystem**, which is what this module opens — the shell keeps running untouched
// while the file goes over its own channel.
//
// The transfer itself is spawned as its own task so a large file never stalls the
// shell pump (`client::stream`), and reports back through the same event channel the
// rest of the session uses: `UploadProgress` while it runs, then `UploadDone`,
// `UploadFailed`, or — when the destination is already taken and the user has not
// confirmed an overwrite — `UploadExists`.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use russh_sftp::client::SftpSession;
use russh_sftp::client::fs::File;
use russh_sftp::protocol::OpenFlags;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;
use crate::ssh::asuser::{AsuserFiles, Runner};
use crate::ssh::shellfs;
use crate::ssh::transfer::{
	self, CopyOutcome, FileAction, PlannedFile, Start, Sticky, Ticker, TreePlan, resume_start,
};

/// How much of the file to move per write. 32 KiB sits comfortably under the SFTP
/// packet limit while keeping the number of round trips low.
const CHUNK: usize = 32 * 1024;

/// Hand the transfer to a background task (§17) over whichever backend the account it writes as
/// could offer (§46). Opening a channel borrows the session handle, which is why that already
/// happened in the session loop: a spawned task cannot hold a borrow, so only owned values move
/// into it and the terminal stays live throughout.
pub async fn start(
	backend: AsuserFiles,
	events: &mpsc::Sender<SshEvent>,
	local: PathBuf,
	remote: String,
	overwrite: bool,
	resume: bool,
	cancel: Arc<AtomicBool>,
) {
	match backend {
		AsuserFiles::Sftp(sftp) => {
			tokio::spawn(transfer(
				sftp,
				local,
				remote,
				overwrite,
				resume,
				events.clone(),
				cancel,
			));
		}
		AsuserFiles::Shell(runner) => {
			let events = events.clone();
			tokio::spawn(async move {
				// The same question the SFTP path asks first, and for the same reason: a write
				// truncates, so a file already there has to be a question rather than a casualty.
				if !overwrite && shellfs::exists(&runner, &remote).await {
					let _ = events.send(SshEvent::UploadExists(remote)).await;
					return;
				}
				let outcome =
					shellfs::send(&runner, &local, &remote, resume, &events, &cancel).await;
				// No stamp and no canonicalize on this backend (see `shellfs`): the copy lands with
				// what the remote's own umask gives it, and the path reported is the one written to.
				report(outcome, &remote, &events).await;
			});
		}
		AsuserFiles::Denied(reason) => {
			let _ = events.send(SshEvent::UploadFailed(reason)).await;
		}
	}
}

/// Check which of `names` already exist under `dir` before an upload batch starts (§17), and
/// answer with `UploadPrescan`. This is what lets the GUI ask the "some are already there"
/// question once for the whole batch rather than once per file, the up-front collision model
/// the multi-file download uses (§21). For each clashing name a free `name-1` alternative is
/// found too, so a "keep both" answer has a server-checked path to write to.
///
/// Opens its own SFTP channel, like `start`, so the pre-scan never contends with the shared
/// listing session. A server that will not answer an existence check fails the whole scan
/// rather than guessing a path is free — guessing would let the batch overwrite silently, the
/// same caution the transfer itself takes (§17).
pub async fn precheck(
	backend: AsuserFiles,
	events: &mpsc::Sender<SshEvent>,
	dir: String,
	names: Vec<String>,
) {
	let sftp = match backend {
		AsuserFiles::Sftp(sftp) => sftp,
		// The shell backend answers the same question with `[ -e path ]`, one command per name.
		AsuserFiles::Shell(runner) => {
			let mut collisions: Vec<(String, String)> = Vec::new();
			for name in &names {
				let remote = crate::explorer::join(&dir, name);
				if shellfs::exists(&runner, &remote).await {
					collisions.push((name.clone(), shellfs::free_name(&runner, &dir, name).await));
				}
			}
			let _ = events.send(SshEvent::UploadPrescan { collisions }).await;
			return;
		}
		AsuserFiles::Denied(reason) => {
			let _ = events.send(SshEvent::UploadFailed(reason)).await;
			return;
		}
	};

	let mut collisions: Vec<(String, String)> = Vec::new();
	for name in &names {
		let remote = crate::explorer::join(&dir, name);
		match sftp.try_exists(&remote).await {
			Ok(true) => {
				let free = free_remote(&sftp, &dir, name).await;
				collisions.push((name.clone(), free));
			}
			Ok(false) => {}
			Err(error) => {
				eprintln!("sftp exists check failed: {error}");
				let _ = events
					.send(SshEvent::UploadFailed(format!(
						"Could not check {remote}: {error}"
					)))
					.await;
				let _ = sftp.close().await;
				return;
			}
		}
	}

	let _ = events.send(SshEvent::UploadPrescan { collisions }).await;
	let _ = sftp.close().await;
}

/// The first free `name-1.ext`, `name-2.ext`… under `dir` on the server — the "keep both"
/// destination for a name already taken (§17). The candidate's SHAPE is
/// `explorer::free_candidate`, shared with the three other backends that answer the same
/// question; what is local to here is that each probe is a round trip, so it is bounded to
/// `explorer::FREE_NAME_TRIES`. An existence check that errors is treated as "free" and stops the
/// probe rather than spinning: the transfer re-checks before it creates the file (§17), so a wrong
/// guess is skipped, never overwritten.
///
/// Running out returns the LAST candidate without probing it, for that same reason — the create
/// that follows re-checks, so the worst a hundred-deep collision costs is one skipped file, never
/// an overwrite.
async fn free_remote(sftp: &SftpSession, dir: &str, name: &str) -> String {
	for attempt in 1..=explorer::FREE_NAME_TRIES {
		let candidate = explorer::join(dir, &explorer::free_candidate(name, attempt));
		if !sftp.try_exists(&candidate).await.unwrap_or(false) {
			return candidate;
		}
	}
	explorer::join(
		dir,
		&explorer::free_candidate(name, explorer::FREE_NAME_TRIES),
	)
}

/// Stream the file to the remote, reporting progress as it goes. Runs to completion in
/// its own task; every outcome is reported as exactly one terminal event.
async fn transfer(
	sftp: SftpSession,
	local: PathBuf,
	remote: String,
	overwrite: bool,
	resume: bool,
	events: mpsc::Sender<SshEvent>,
	cancel: Arc<AtomicBool>,
) {
	// A file already there is not an error — it is a question, and the user has already
	// been asked exactly once. Checking before opening the destination matters: SFTP's
	// create truncates, so by the time a write fails the old contents are gone. A resume
	// carries `overwrite`, so it skips straight to the copy, where it appends rather than
	// truncates (§16) — the file already there is its own interrupted work, not a clash.
	if !overwrite {
		match sftp.try_exists(&remote).await {
			Ok(true) => {
				let _ = events.send(SshEvent::UploadExists(remote)).await;
				return;
			}
			Ok(false) => {}
			Err(error) => {
				// The server would not say. Treat that as a failure rather than
				// assuming the path is free — assuming would overwrite silently.
				eprintln!("sftp exists check failed: {error}");
				let _ = events
					.send(SshEvent::UploadFailed(format!(
						"Could not check {remote}: {error}"
					)))
					.await;
				return;
			}
		}
	}

	match copy(&sftp, &local, &remote, resume, &events, &cancel).await {
		// Only this backend can stamp and canonicalize, so only this arm is written here; the other
		// two outcomes read the same for both and are reported by `report`.
		Ok(CopyOutcome::Done) => {
			// Best-effort, before announcing it: stamp the copy with the source's timestamps and,
			// from a Unix source, its permission bits (§17). A re-stat here is a cheap local call,
			// and a server that refuses setstat just leaves the bytes undecorated — still a good
			// upload.
			if let Ok(meta) = tokio::fs::metadata(&local).await {
				let (mtime, atime, mode) = source_stamp(&meta);
				stamp_remote(&sftp, &remote, mtime, atime, mode).await;
			}
			// Report where the bytes actually landed: a relative path (what the dialog
			// offers when the shell's cwd is unknown) resolves against the login
			// directory, and the user should see the full path, not their own input.
			let resolved = sftp
				.canonicalize(remote.clone())
				.await
				.unwrap_or_else(|_| remote.clone());
			let _ = events.send(SshEvent::UploadDone(resolved)).await;
		}
		outcome => report(outcome, &remote, &events).await,
	}
	let _ = sftp.close().await;
}

/// Turn one copy's outcome into exactly one terminal event, for either backend (§46). The SFTP path
/// handles its own success (it can stamp the copy and resolve the path it landed on); everything
/// else reads the same whichever backend carried the bytes.
async fn report(outcome: Result<CopyOutcome>, remote: &str, events: &mpsc::Sender<SshEvent>) {
	match outcome {
		Ok(CopyOutcome::Done) => {
			let _ = events.send(SshEvent::UploadDone(remote.to_owned())).await;
		}
		// The user pressed ✕ (§16): the copy loop already deleted the partial, so this is a
		// neutral end, not an error — a message the status bar shows without crying failure.
		Ok(CopyOutcome::Cancelled) => {
			let _ = events
				.send(SshEvent::UploadFailed("Upload cancelled.".to_string()))
				.await;
		}
		// The destination refused to be created at all (§16): no partial exists, so this ends the
		// same way a file that never left does — the reason in the status bar, the queue behind it
		// moving on, and NO Resume, which could only run the same refused create again.
		Err(error) if transfer::was_refused(&error) => {
			eprintln!("upload refused: {error}");
			let _ = events
				.send(SshEvent::UploadFailed(format!("Upload failed: {error}")))
				.await;
		}
		// A mid-flight failure keeps its partial (§16), so this is resumable rather than final:
		// the GUI offers a Resume that re-sends only the bytes still missing.
		Err(error) => {
			eprintln!("upload interrupted: {error:#}");
			let _ = events
				.send(SshEvent::TransferInterrupted {
					message: format!("Upload interrupted: {error} — Resume to continue."),
				})
				.await;
		}
	}
}

/// The copy loop: local file in, remote file out, a progress event every
/// `PROGRESS_STEP` bytes. Split from `transfer` so the outcome handling above reads as
/// one `match` over one `Result`. On a resume it opens the destination for append at its
/// current size and skips that many bytes of the source, so only the missing tail crosses
/// (§16); between chunks it polls `cancel`, and on a cancel it drops the partial and stops.
async fn copy(
	sftp: &SftpSession,
	local: &Path,
	remote: &str,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	let total = tokio::fs::metadata(local)
		.await
		.with_context(|| format!("could not read {}", local.display()))?
		.len();
	// On a resume, how much is already on the server decides where to pick up; a fresh send
	// ignores it and starts at zero (a truncating create).
	let dest_size = if resume {
		sftp.metadata(remote.to_owned())
			.await
			.ok()
			.and_then(|meta| meta.size)
	} else {
		None
	};
	let offset = match resume_start(resume, dest_size, total) {
		// The whole file is already there from before the interruption: nothing to send.
		Start::Skip => {
			let _ = events
				.send(SshEvent::TransferProgress { sent: total, total })
				.await;
			return Ok(CopyOutcome::Done);
		}
		Start::At(offset) => offset,
	};

	let mut source = tokio::fs::File::open(local)
		.await
		.with_context(|| format!("could not open {}", local.display()))?;
	let mut destination = open_remote_at(sftp, remote, offset).await?;
	if offset > 0 {
		source
			.seek(SeekFrom::Start(offset))
			.await
			.context("could not seek the local file to the resume point")?;
	}

	let mut buffer = vec![0u8; CHUNK];
	// The bytes already on the server from the interrupted run count towards the total at once, so
	// a resumed bar picks up where it left off instead of starting again from nothing.
	let mut ticker = Ticker::default();
	ticker.settle(offset);
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: ticker.moved(),
			total,
		})
		.await;
	loop {
		// Checked before each read so a cancel is honoured promptly and, crucially, before any
		// more bytes are written: the partial is then deleted and the transfer ends here (§16).
		if cancel.load(Ordering::Relaxed) {
			drop(destination);
			let _ = sftp.remove_file(remote.to_owned()).await;
			return Ok(CopyOutcome::Cancelled);
		}
		let read = source.read(&mut buffer).await.context("read failed")?;
		if read == 0 {
			break;
		}
		destination
			.write_all(&buffer[..read])
			.await
			.context("write failed")?;
		if let Some(sent) = ticker.advance(read as u64) {
			let _ = events
				.send(SshEvent::TransferProgress { sent, total })
				.await;
		}
	}

	// `shutdown` flushes and closes the remote handle; without it the last writes can
	// still be in flight when we report success.
	destination.shutdown().await.context("close failed")?;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: ticker.moved(),
			total,
		})
		.await;
	Ok(CopyOutcome::Done)
}

/// Open a remote file to write at `offset` (§16). A zero offset is a fresh send: `create`
/// truncates whatever is there, the transfer's normal behaviour. A non-zero offset is a resume:
/// open the existing file for writing WITHOUT truncating and seek to where the last run stopped,
/// so the append lands exactly at the byte boundary the destination already reached.
async fn open_remote_at(sftp: &SftpSession, remote: &str, offset: u64) -> Result<File> {
	if offset == 0 {
		return sftp
			.create(remote.to_owned())
			.await
			.with_context(|| format!("could not create {remote} on the server"))
			// A destination that was never created holds no bytes to continue from, and asking a
			// second time would be refused the same way — so this failure is final, not resumable
			// (§16). The commonest case by far: uploading into a folder this account cannot write.
			.map_err(transfer::mark_refused);
	}
	let mut file = sftp
		.open_with_flags(remote.to_owned(), OpenFlags::WRITE | OpenFlags::CREATE)
		.await
		.with_context(|| format!("could not open {remote} on the server to resume"))
		// The partial is there, but out of reach: a further Resume would ask for exactly this and
		// be refused exactly the same, so there is nothing to offer.
		.map_err(transfer::mark_refused)?;
	file.seek(SeekFrom::Start(offset))
		.await
		.with_context(|| format!("could not seek {remote} to the resume point"))?;
	Ok(file)
}

/// Pull the timestamps and (on a Unix host) the permission bits off a local file's metadata (§17),
/// in the seconds-since-epoch form the SFTP stamp wants. A timestamp the filesystem will not give
/// reads as `None`, and the stamp then omits it rather than guessing.
fn source_stamp(meta: &std::fs::Metadata) -> (Option<u32>, Option<u32>, Option<u32>) {
	let mtime = meta.modified().ok().and_then(transfer::epoch_secs);
	let atime = meta.accessed().ok().and_then(transfer::epoch_secs);
	(mtime, atime, source_mode(meta))
}

/// The source file's Unix permission bits, on a Unix host that has them.
#[cfg(unix)]
#[expect(
	clippy::unnecessary_wraps,
	reason = "the `Option` is the shared signature: the `not(unix)` twin below answers `None`, and \
	          `source_stamp` passes whichever answer straight into the SFTP stamp. Clippy lints one \
	          `cfg` at a time, so it cannot see the sibling."
)]
fn source_mode(meta: &std::fs::Metadata) -> Option<u32> {
	use std::os::unix::fs::MetadataExt;
	Some(meta.mode() & 0o7777)
}

/// A Windows source has no Unix permission bits to carry — so an upload from Windows sends only
/// the timestamps, and the server's own umask decides the new file's mode.
#[cfg(not(unix))]
fn source_mode(_meta: &std::fs::Metadata) -> Option<u32> {
	None
}

/// Stamp the just-uploaded remote file with the source's modification time and, from a Unix
/// source, its permission bits (§17). Best-effort by design: many servers refuse setstat
/// (read-only exports, chrooted SFTP), so a failure is logged and swallowed — the bytes are
/// across, which is the transfer's promise, and a metadata refusal must never turn a good upload
/// into a failure. A source with no readable metadata at all makes no round trip.
async fn stamp_remote(
	sftp: &SftpSession,
	remote: &str,
	mtime: Option<u32>,
	atime: Option<u32>,
	mode: Option<u32>,
) {
	if mtime.is_none() && mode.is_none() {
		return;
	}
	let attrs = transfer::upload_stamp(mtime, atime, mode);
	if let Err(error) = sftp.set_metadata(remote.to_owned(), attrs).await {
		eprintln!("could not stamp {remote} with the source's metadata: {error}");
	}
}

/// Open the SFTP channel and hand a whole-folder upload to a background task (§17). The mirror
/// of `start` for a single file, but the transfer may PAUSE mid-way to ask the user about a file
/// whose destination is already taken — so it is handed the `answers` receiver `run()` keeps the
/// other end of, and parks on it while the shell keeps flowing behind the prompt.
pub async fn start_tree(
	backend: AsuserFiles,
	events: &mpsc::Sender<SshEvent>,
	local: PathBuf,
	remote: String,
	resume: bool,
	answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	match backend {
		AsuserFiles::Sftp(sftp) => {
			tokio::spawn(transfer_tree(
				sftp,
				local,
				remote,
				resume,
				events.clone(),
				answers,
				cancel,
			));
		}
		AsuserFiles::Shell(runner) => {
			let events = events.clone();
			let mut answers = answers;
			tokio::spawn(async move {
				let outcome = shell_tree(
					&runner,
					&local,
					&remote,
					resume,
					&events,
					&mut answers,
					&cancel,
				)
				.await;
				report_tree(outcome, &events).await;
			});
		}
		AsuserFiles::Denied(reason) => {
			let _ = events.send(SshEvent::UploadFailed(reason)).await;
		}
	}
}

/// The shell backend's tree upload (§46): walk the local folder as the SFTP path does, then create
/// each directory with `mkdir -p` and send each file through `cat`. The folder keeps its own name
/// inside the destination, and the plan's total is announced up front so the bar has a target.
async fn shell_tree(
	runner: &Runner,
	local_root: &Path,
	remote_dir: &str,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	cancel: &Arc<AtomicBool>,
) -> Result<Option<String>> {
	let name = local_root
		.file_name()
		.map(|name| name.to_string_lossy().into_owned())
		.context("the folder has no name")?;
	let remote_target = crate::explorer::join(remote_dir, &name);
	let plan = walk_local(local_root)
		.await
		.with_context(|| format!("could not read {}", local_root.display()))?;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: 0,
			total: plan.total(),
		})
		.await;
	let outcome = shellfs::send_tree(
		runner,
		&plan,
		local_root,
		&remote_target,
		resume,
		shellfs::TreeRun {
			events,
			answers,
			cancel,
		},
	)
	.await?;
	Ok(match outcome {
		CopyOutcome::Done => Some(remote_target),
		CopyOutcome::Cancelled => None,
	})
}

/// Send the local folder to the remote, reporting one terminal event (§17). A clean run ends in
/// `UploadDone` with the folder's remote path; a user cancel and a real failure both end in
/// `UploadFailed` — the message tells them apart, and either way the shell is left untouched.
async fn transfer_tree(
	sftp: SftpSession,
	local_root: PathBuf,
	remote_dir: String,
	resume: bool,
	events: mpsc::Sender<SshEvent>,
	mut answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	let outcome = send_tree(
		&sftp,
		&local_root,
		&remote_dir,
		resume,
		&events,
		&mut answers,
		&cancel,
	)
	.await;
	report_tree(outcome, &events).await;
	let _ = sftp.close().await;
}

/// Turn a tree upload's outcome into exactly one terminal event, for either backend (§46).
async fn report_tree(outcome: Result<Option<String>>, events: &mpsc::Sender<SshEvent>) {
	match outcome {
		Ok(Some(path)) => {
			let _ = events.send(SshEvent::UploadDone(path)).await;
		}
		// A cancel — the ✕ (§16) or the conflict dialog's Cancel — is not a failure, but it IS the
		// terminal signal the GUI waits for to free the transfer slot: a neutral message, so the
		// status bar does not cry error over a choice, and no resume, since a cancel is final.
		Ok(None) => {
			let _ = events
				.send(SshEvent::UploadFailed(
					"Folder upload cancelled.".to_string(),
				))
				.await;
		}
		// The destination refused to be made (§16) — the folder itself, or one inside it, before a
		// single file was copied. Nothing to pick up, so the tree ends cleanly rather than offering
		// a Resume that would be refused at the same `mkdir`.
		Err(error) if transfer::was_refused(&error) => {
			eprintln!("folder upload refused: {error}");
			let _ = events
				.send(SshEvent::UploadFailed(format!(
					"Folder upload failed: {error}"
				)))
				.await;
		}
		// A mid-flight failure keeps every byte already copied (§16): a Resume re-walks the tree
		// and size-compares, so only the files still missing (and the tail of the interrupted one)
		// are sent again.
		Err(error) => {
			eprintln!("folder upload interrupted: {error:#}");
			let _ = events
				.send(SshEvent::TransferInterrupted {
					message: format!("Folder upload interrupted: {error} — Resume to continue."),
				})
				.await;
		}
	}
}

/// The tree upload itself: recreate the folder under `remote_dir`, then copy every file into it,
/// merging into whatever is already there and asking about each collision (§17). Returns the
/// destination path on success, or `None` when the user cancelled partway.
async fn send_tree(
	sftp: &SftpSession,
	local_root: &Path,
	remote_dir: &str,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	cancel: &Arc<AtomicBool>,
) -> Result<Option<String>> {
	// The folder keeps its own name inside the destination, the same rule a single file follows.
	let name = local_root
		.file_name()
		.and_then(std::ffi::OsStr::to_str)
		.context("the folder has no name to upload under")?;
	let remote_target = explorer::join(remote_dir, name);

	let plan = walk_local(local_root)
		.await
		.with_context(|| format!("could not read {}", local_root.display()))?;
	let total = plan.total();

	// Create the destination and every subdirectory before any file goes into them. `plan.dirs`
	// is parents-before-children, so a folder is never asked for before the one that holds it.
	ensure_remote_dir(sftp, &remote_target).await?;
	for rel in &plan.dirs {
		ensure_remote_dir(sftp, &transfer::remote_join(&remote_target, rel)).await?;
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
		let dest = transfer::remote_join(&remote_target, &file.rel);
		let leaf = file.rel.last().map_or("", String::as_str);
		// A resume never prompts (§16): an existing destination is the transfer's own earlier
		// work, which `send_file` size-compares and appends to — not a fresh collision. A first
		// run, though, treats a destination already taken as a question, not an overwrite (§17);
		// everything else falls through to a plain write to `dest`.
		let dest = if !resume && sftp.try_exists(&dest).await.unwrap_or(false) {
			match transfer::resolve(events, answers, &mut sticky, leaf).await {
				FileAction::Overwrite => dest,
				FileAction::KeepBoth => {
					let dir = explorer::parent(&dest).unwrap_or(&remote_target).to_owned();
					free_remote(sftp, &dir, leaf).await
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
			dest
		};
		let local = transfer::local_join(local_root, &file.rel);
		// A cancel mid-file drops that file's partial and stops the whole tree (§16): the files
		// already fully copied stay, mirroring how a single-file cancel keeps nothing but its own.
		if send_file(sftp, &local, &dest, file.size, &mut run).await? == CopyOutcome::Cancelled {
			return Ok(None);
		}
		// The file is fully across: stamp it with the metadata the walk captured off the source
		// (§17), the per-file mirror of the single-file stamp above.
		stamp_remote(sftp, &dest, file.mtime, file.atime, file.mode).await;
	}

	let _ = events
		.send(SshEvent::TransferProgress {
			sent: run.ticker.moved(),
			total,
		})
		.await;
	if plan.skipped_links > 0 {
		eprintln!(
			"folder upload could not follow {} symlink(s)",
			plan.skipped_links
		);
	}
	Ok(Some(remote_target))
}

/// Copy one local file to the remote, folding its bytes into the tree-wide `ticker` and emitting a
/// progress event every `PROGRESS_STEP` (§17). Split from `copy` because a tree's progress runs
/// across many files against one running total, not per file from zero — which is exactly what the
/// [`Ticker`] carries between calls, and why it arrives as one argument rather than the pair of
/// `&mut u64` counters this used to thread through by hand. On a resume it size-compares the
/// destination (§16): a file already fully there is skipped (its bytes still counted, so the bar
/// reaches the end), and a partial is appended from where it stopped; between chunks it polls
/// `run.cancel`, dropping the partial and reporting `Cancelled` if it is set. The rest of the run's
/// state travels in the [`transfer::CopyRun`] for the same reason the counters became a ticker
/// (§111).
async fn send_file(
	sftp: &SftpSession,
	local: &Path,
	remote: &str,
	size: u64,
	run: &mut transfer::CopyRun<'_>,
) -> Result<CopyOutcome> {
	let dest_size = if run.resume {
		sftp.metadata(remote.to_owned())
			.await
			.ok()
			.and_then(|meta| meta.size)
	} else {
		None
	};
	let offset = match resume_start(run.resume, dest_size, size) {
		// Already fully there from before the interruption: count its bytes and move on.
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

	let mut source = tokio::fs::File::open(local)
		.await
		.with_context(|| format!("could not open {}", local.display()))?;
	let mut destination = open_remote_at(sftp, remote, offset).await?;
	if offset > 0 {
		source
			.seek(SeekFrom::Start(offset))
			.await
			.context("could not seek the local file to the resume point")?;
		// The bytes already on the server count towards the running total straight away, so the
		// bar reflects the resumed progress rather than dropping back.
		run.ticker.settle(offset);
	}

	let mut buffer = vec![0u8; CHUNK];
	loop {
		if run.cancel.load(Ordering::Relaxed) {
			drop(destination);
			let _ = sftp.remove_file(remote.to_owned()).await;
			return Ok(CopyOutcome::Cancelled);
		}
		let read = source.read(&mut buffer).await.context("read failed")?;
		if read == 0 {
			break;
		}
		destination
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
	destination.shutdown().await.context("close failed")?;
	Ok(CopyOutcome::Done)
}

/// Create `path` on the server unless it is already there, so an upload merges into an existing
/// folder rather than failing on it (§17). An existence check that the server refuses is treated
/// as "not there" and the create is attempted — its own error then surfaces if that was wrong.
async fn ensure_remote_dir(sftp: &SftpSession, path: &str) -> Result<()> {
	if sftp.try_exists(path).await.unwrap_or(false) {
		return Ok(());
	}
	sftp.create_dir(path)
		.await
		.with_context(|| format!("could not create {path} on the server"))
		// Every directory is made before any file goes into one, so a refusal here means the tree
		// has not copied a byte — nothing to resume, and the same `mkdir` would be refused again.
		.map_err(transfer::mark_refused)
}

/// Walk a local directory tree into the plan both transfer directions share (§17). Iterative,
/// not recursive, so a deep tree costs heap not stack.
///
/// **Symlinks are followed** (§17), which is what a user dropping a folder means by "send this
/// folder": a link to a file sends the file's bytes, a link to a folder sends that folder's
/// contents, and the far side gets real files and real directories — the same thing `cp -L` or
/// `rsync -L` produces. Nothing writes a link on the remote, because a link's target is a path
/// on THIS machine and would point at nothing over there.
///
/// The danger that keeps is a cycle: a link back up its own tree makes an endless walk. Each
/// frontier item therefore carries the CANONICAL path of the directory it is, and a directory
/// link is followed only when `transfer::loops_back` says its target is not that directory or one
/// above it. `Path::starts_with` asks it here, component-wise, because both paths are `Path`s.
///
/// `ponytail:` the whole listing is held in memory before a byte is sent — fine for an ordinary
/// folder, but a tree of millions of files would be felt. Upgrade path: stream the walk and the
/// transfer together, the way the files pane's batched listing does (§19).
///
/// `pub(crate)` for a second caller since §103: a LOCAL session's transfers have a local tree at both
/// ends, so both directions walk it with this. Nothing about the walk is about SSH — it reads this
/// machine's disk and describes what it found — so it is shared rather than copied, and the symlink
/// rules above (follow, but never into a cycle) hold for a local copy without being restated.
pub(crate) async fn walk_local(root: &Path) -> Result<TreePlan> {
	let mut plan = TreePlan::default();
	// Each frontier item is a directory to read, its path RELATIVE to the root (empty for the root
	// itself, which is created by the caller, not listed here), and its CANONICAL path — the one
	// the cycle test compares a link's target against.
	// Fatal rather than guessed: without the root's real path every cycle test below compares
	// against something that may not mean what it says, and a missed cycle is a walk that never
	// returns. A folder the user just picked resolves, so this is the "it vanished" case.
	let root_here = tokio::fs::canonicalize(root)
		.await
		.with_context(|| format!("could not resolve {}", root.display()))?;
	let mut frontier: Vec<(PathBuf, Vec<String>, PathBuf)> =
		vec![(root.to_path_buf(), Vec::new(), root_here)];
	while let Some((dir, rel, here)) = frontier.pop() {
		let mut reader = tokio::fs::read_dir(&dir)
			.await
			.with_context(|| format!("could not read {}", dir.display()))?;
		while let Some(entry) = reader
			.next_entry()
			.await
			.context("could not read an entry")?
		{
			let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
				// A name this platform cannot render as UTF-8 has no clean remote counterpart;
				// leave it rather than invent one.
				continue;
			};
			let child_here = here.join(&name);
			let mut child_rel = rel.clone();
			child_rel.push(name);
			let file_type = entry
				.file_type()
				.await
				.context("could not read a file type")?;
			if file_type.is_symlink() {
				walk_link(&mut plan, &mut frontier, entry.path(), child_rel, &here).await;
			} else if file_type.is_dir() {
				plan.dirs.push(child_rel.clone());
				// No link on the way, so the canonical path of a real subdirectory is its parent's
				// canonical path plus its name — no `canonicalize` call needed to know it.
				frontier.push((entry.path(), child_rel, child_here));
			} else if file_type.is_file() {
				// Read the source's size and metadata once, here, so the transfer can stamp each
				// copy to match without a second stat when it reaches the file (§17).
				plan.files.push(planned_local(
					child_rel,
					entry.metadata().await.ok().as_ref(),
				));
			}
			// Anything else — a fifo, a socket, a device node — is not a file to send.
		}
	}
	Ok(plan)
}

/// Follow one local symlink and put what it points at into the plan (§17), or count it when there
/// is nothing to follow it to. Split out of `walk_local` so the ordinary entries above stay one
/// glance wide, and because this is where every extra system call a link costs is paid:
///
///   * `metadata` follows the link — `DirEntry::metadata` does NOT, it reports the link itself —
///     and its failure IS the dangling case, so the answer and the error are the same call; and
///   * `canonicalize`, only for a link to a directory, resolves the whole chain so the cycle test
///     compares two paths that mean what they say.
///
/// Both are per-symlink: a tree without one pays neither.
async fn walk_link(
	plan: &mut TreePlan,
	frontier: &mut Vec<(PathBuf, Vec<String>, PathBuf)>,
	path: PathBuf,
	rel: Vec<String>,
	here: &Path,
) {
	let Ok(meta) = tokio::fs::metadata(&path).await else {
		// Dangling — nothing to read and nowhere to walk. Counted, not fatal: a broken link in one
		// corner of a tree must not lose the rest of it.
		plan.skipped_links += 1;
		return;
	};
	if meta.is_dir() {
		let Ok(target) = tokio::fs::canonicalize(&path).await else {
			plan.skipped_links += 1;
			return;
		};
		// The cycle test, `transfer::loops_back` in `Path` form: walking into a folder that holds
		// this very link would come back out through it forever.
		if here.starts_with(&target) {
			plan.skipped_links += 1;
			return;
		}
		plan.dirs.push(rel.clone());
		frontier.push((path, rel, target));
	} else if meta.is_file() {
		// The metadata is the TARGET's, already in hand from the follow above, so the stamp costs
		// no further call than the one that decided the link was worth following.
		plan.files.push(planned_local(rel, Some(&meta)));
	}
	// A link to a fifo, a socket or a device node is no more sendable than the thing itself.
}

/// One local file as the plan describes it (§17). Metadata the filesystem refused leaves a
/// zero-sized, unstamped entry rather than failing the walk: the bytes still copy, and the progress
/// total is only short by whatever that one file weighs.
fn planned_local(rel: Vec<String>, meta: Option<&std::fs::Metadata>) -> PlannedFile {
	let (size, mtime, atime, mode) = match meta {
		Some(meta) => {
			let (mtime, atime, mode) = source_stamp(meta);
			(meta.len(), mtime, atime, mode)
		}
		None => (0, None, None, None),
	};
	PlannedFile {
		rel,
		size,
		mtime,
		atime,
		mode,
	}
}

#[cfg(test)]
mod tests {
	use std::path::Path;

	use super::{TreePlan, walk_local};

	/// Make a symlink for a test, saying whether this machine allowed it. Windows only lets an
	/// unprivileged process create one with Developer Mode on, so the link tests below check this
	/// and step aside rather than fail on a machine that simply cannot hold the fixture.
	fn link(target: &Path, at: &Path, folder: bool) -> bool {
		#[cfg(unix)]
		{
			let _ = folder;
			std::os::unix::fs::symlink(target, at).is_ok()
		}
		#[cfg(windows)]
		{
			if folder {
				std::os::windows::fs::symlink_dir(target, at).is_ok()
			} else {
				std::os::windows::fs::symlink_file(target, at).is_ok()
			}
		}
	}

	/// Say a link could not be made here, so a run with `--nocapture` shows which tests did not
	/// actually get to prove anything on this machine.
	fn no_links(what: &str) {
		eprintln!("skipped {what}: this machine will not create a symlink");
	}

	/// The plan's directories as sorted `a/b` strings, which read like the tree they describe.
	fn dirs(plan: &TreePlan) -> Vec<String> {
		let mut out: Vec<String> = plan.dirs.iter().map(|rel| rel.join("/")).collect();
		out.sort();
		out
	}

	/// The plan's files as sorted `path=size` strings — the size is in because a followed link is
	/// only right if it planned the TARGET's bytes rather than the link's own.
	fn files(plan: &TreePlan) -> Vec<String> {
		let mut out: Vec<String> = plan
			.files
			.iter()
			.map(|file| format!("{}={}", file.rel.join("/"), file.size))
			.collect();
		out.sort();
		out
	}

	#[tokio::test]
	async fn a_plain_tree_is_walked_into_its_folders_and_files() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::create_dir(root.join("sub")).unwrap();
		std::fs::write(root.join("top.txt"), b"abc").unwrap();
		std::fs::write(root.join("sub").join("deep.txt"), b"de").unwrap();

		let plan = walk_local(root).await.unwrap();

		// The root itself is not listed — the caller creates it — so only `sub` is here.
		assert_eq!(dirs(&plan), vec!["sub".to_owned()]);
		assert_eq!(files(&plan), vec!["sub/deep.txt=2", "top.txt=3"]);
		assert_eq!(plan.skipped_links, 0);
	}

	#[tokio::test]
	async fn a_link_to_a_file_is_planned_as_the_file_it_points_at() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::write(root.join("real.txt"), b"hello").unwrap();
		if !link(&root.join("real.txt"), &root.join("shortcut.txt"), false) {
			no_links("a_link_to_a_file_is_planned_as_the_file_it_points_at");
			return;
		}

		let plan = walk_local(root).await.unwrap();

		// Both names, both five bytes: the link is planned as a plain file carrying the target's
		// content, which is what lands on the remote.
		assert_eq!(files(&plan), vec!["real.txt=5", "shortcut.txt=5"]);
		assert_eq!(plan.skipped_links, 0);
	}

	#[tokio::test]
	async fn a_link_to_a_folder_is_walked_into() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::create_dir(root.join("real")).unwrap();
		std::fs::write(root.join("real").join("inside.txt"), b"x").unwrap();
		if !link(&root.join("real"), &root.join("shortcut"), true) {
			no_links("a_link_to_a_folder_is_walked_into");
			return;
		}

		let plan = walk_local(root).await.unwrap();

		// The linked folder is a folder in the plan and its content is planned underneath it — the
		// same tree twice, which is what following a link means and what `cp -L` produces.
		assert_eq!(dirs(&plan), vec!["real".to_owned(), "shortcut".to_owned()]);
		assert_eq!(
			files(&plan),
			vec!["real/inside.txt=1", "shortcut/inside.txt=1"]
		);
		assert_eq!(plan.skipped_links, 0);
	}

	#[tokio::test]
	async fn a_link_back_up_the_tree_is_counted_not_followed() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::create_dir(root.join("sub")).unwrap();
		std::fs::write(root.join("sub").join("inside.txt"), b"x").unwrap();
		// The classic cycle: a link inside the tree pointing at the tree's own top.
		if !link(root, &root.join("sub").join("loop"), true) {
			no_links("a_link_back_up_the_tree_is_counted_not_followed");
			return;
		}

		let plan = walk_local(root).await.unwrap();

		// Walking in would find `sub/loop` again, and again. The walk RETURNS, having counted the
		// one link it would not take, and everything else in the tree is still planned.
		assert_eq!(dirs(&plan), vec!["sub".to_owned()]);
		assert_eq!(files(&plan), vec!["sub/inside.txt=1"]);
		assert_eq!(plan.skipped_links, 1);
	}

	#[tokio::test]
	async fn a_link_to_its_own_folder_is_counted_not_followed() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::create_dir(root.join("sub")).unwrap();
		// The shortest cycle of all: a link in `sub` pointing at `sub`.
		if !link(&root.join("sub"), &root.join("sub").join("self"), true) {
			no_links("a_link_to_its_own_folder_is_counted_not_followed");
			return;
		}

		let plan = walk_local(root).await.unwrap();

		assert_eq!(dirs(&plan), vec!["sub".to_owned()]);
		assert_eq!(plan.skipped_links, 1);
	}

	#[tokio::test]
	async fn a_dangling_link_is_counted_not_followed() {
		let temp = tempfile::tempdir().unwrap();
		let root = temp.path();
		std::fs::write(root.join("real.txt"), b"ok").unwrap();
		if !link(&root.join("gone.txt"), &root.join("broken.txt"), false) {
			no_links("a_dangling_link_is_counted_not_followed");
			return;
		}

		let plan = walk_local(root).await.unwrap();

		// Nothing to read and nowhere to walk, so it is counted — and the file beside it still
		// goes, because one broken link must not cost the rest of the tree.
		assert_eq!(files(&plan), vec!["real.txt=2"]);
		assert_eq!(plan.skipped_links, 1);
	}
}

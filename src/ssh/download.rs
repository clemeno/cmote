// ssh/download.rs — pull a remote file down to this machine over SFTP (PLAN §19).
//
// The mirror image of `upload` (§17), and built the same way: its own sftp channel so
// the interactive shell is untouched, the transfer spawned so a large file never stalls
// the shell pump, and progress reported through the session's own event channel.
//
// There is no overwrite prompt here on purpose. The destination comes from the native
// save dialog, which already asks before replacing a local file — a second question in
// our own chrome would only be a second chance to answer it wrong.

use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::FileAttributes;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;
use crate::ssh::transfer::{
	self, CopyOutcome, FileAction, PlannedFile, Start, Sticky, TreePlan, resume_start,
};

/// How much of the file to move per read, and how many bytes between progress events.
/// The same figures the upload uses, for the same reasons (§17).
const CHUNK: usize = 32 * 1024;
const PROGRESS_STEP: u64 = 256 * 1024;

/// Open the SFTP channel and hand the transfer to a background task (§19). Opening
/// borrows the session handle, so it happens inline; only the owned `SftpSession` moves
/// into the spawned task.
pub async fn start<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	remote: String,
	local: PathBuf,
	resume: bool,
	cancel: Arc<AtomicBool>,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(fetch(sftp, remote, local, resume, events.clone(), cancel));
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::DownloadFailed(
					"Could not open an SFTP channel — the server may not offer the sftp \
					 subsystem."
						.to_string(),
				))
				.await;
		}
	}
}

/// Stream the remote file into the local one, reporting progress as it goes. Runs to
/// completion in its own task; every outcome is exactly one terminal event.
async fn fetch(
	sftp: SftpSession,
	remote: String,
	local: PathBuf,
	resume: bool,
	events: mpsc::Sender<SshEvent>,
	cancel: Arc<AtomicBool>,
) {
	match copy(&sftp, &remote, &local, resume, &events, &cancel).await {
		Ok(CopyOutcome::Done) => {
			let _ = events
				.send(SshEvent::DownloadDone(local.display().to_string()))
				.await;
		}
		// The user pressed ✕ (§16): the partial local file was deleted, so this is neutral, not
		// an error — the status bar shows the message without the failure styling.
		Ok(CopyOutcome::Cancelled) => {
			let _ = events
				.send(SshEvent::DownloadFailed("Download cancelled.".to_string()))
				.await;
		}
		// A mid-flight failure keeps the partial (§16): a Resume reads the remote from the local
		// file's size and appends the rest.
		Err(error) => {
			eprintln!("download interrupted: {error:#}");
			// The detail is the user's own file and path (not auth material, §12), so
			// showing it is what makes the failure actionable.
			let _ = events
				.send(SshEvent::TransferInterrupted {
					message: format!("Download interrupted: {error} — Resume to continue."),
				})
				.await;
		}
	}
	let _ = sftp.close().await;
}

/// The copy loop: remote file in, local file out, a progress event every `PROGRESS_STEP` bytes.
/// On a resume it opens the local file for append at its current size and reads the remote from
/// that offset, so only the missing tail is pulled (§16); between chunks it polls `cancel`, and on
/// a cancel it deletes the partial and stops.
async fn copy(
	sftp: &SftpSession,
	remote: &str,
	local: &std::path::Path,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	// One metadata fetch serves both the progress total and the stamp the finished file will get —
	// the remote's modification time and, for a Unix destination, its permission bits. A server
	// that will not report metadata is not a failure: the total is then unknown (the transfer runs
	// without one) and no stamp is applied.
	let source_meta = sftp.metadata(remote.to_owned()).await.ok();
	let total = source_meta.as_ref().and_then(|meta| meta.size).unwrap_or(0);
	let mtime = source_meta.as_ref().and_then(|meta| meta.mtime);
	let mode = source_meta.as_ref().and_then(remote_mode);

	// Where to pick up. With the remote size known, the shared rule decides skip / append / fresh;
	// with it unknown (`total == 0`), a size compare cannot say whether the local partial is
	// complete, so a resume simply appends from wherever it got to and reads the remote to EOF —
	// never skipping, which could otherwise drop a file whose size the server just would not give.
	let offset = if resume {
		let have = tokio::fs::metadata(local)
			.await
			.ok()
			.map(|meta| meta.len())
			.unwrap_or(0);
		if total > 0 {
			match resume_start(true, Some(have), total) {
				Start::Skip => {
					let _ = events
						.send(SshEvent::TransferProgress { sent: total, total })
						.await;
					// Already fully here, but a prior run may have died before stamping it — so
					// still apply the source's metadata (§19) before calling it done.
					stamp_local(local, mtime, mode).await;
					return Ok(CopyOutcome::Done);
				}
				Start::At(offset) => offset,
			}
		} else {
			have
		}
	} else {
		0
	};

	let mut source = sftp
		.open(remote.to_owned())
		.await
		.with_context(|| format!("could not open {remote} on the server"))?;
	if offset > 0 {
		source
			.seek(SeekFrom::Start(offset))
			.await
			.with_context(|| format!("could not seek {remote} to the resume point"))?;
	}
	let mut destination = open_local_at(local, offset).await?;

	let mut buffer = vec![0u8; CHUNK];
	let mut received = offset;
	let mut reported = offset;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: received,
			total,
		})
		.await;
	loop {
		if cancel.load(Ordering::Relaxed) {
			drop(destination);
			let _ = tokio::fs::remove_file(local).await;
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
		received += read as u64;
		if received - reported >= PROGRESS_STEP {
			reported = received;
			let _ = events
				.send(SshEvent::TransferProgress {
					sent: received,
					total,
				})
				.await;
		}
	}

	// Flush before reporting success: without this the last writes can still be in the
	// OS buffer when the user is told the file is on disk.
	destination.flush().await.context("close failed")?;
	// The file is complete on disk: stamp it with the remote's modification time and (on Unix) its
	// permission bits (§19), best-effort, before announcing it.
	drop(destination);
	stamp_local(local, mtime, mode).await;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: received,
			total,
		})
		.await;
	Ok(CopyOutcome::Done)
}

/// Open a local file to write at `offset` (§16). A zero offset is a fresh fetch: `create`
/// truncates whatever is there, which is what the save dialog already agreed to. A non-zero offset
/// is a resume: open the existing file for writing without truncating and seek to where the last
/// run stopped, so the append lands exactly at the byte the file already reached.
async fn open_local_at(local: &Path, offset: u64) -> Result<tokio::fs::File> {
	if offset == 0 {
		return tokio::fs::File::create(local)
			.await
			.with_context(|| format!("could not create {}", local.display()));
	}
	let mut file = tokio::fs::OpenOptions::new()
		.write(true)
		.create(true)
		// Keep the bytes already on disk: the resume writes over them from `offset` on, so
		// truncating would throw away exactly the partial we are continuing.
		.truncate(false)
		.open(local)
		.await
		.with_context(|| format!("could not open {} to resume", local.display()))?;
	file.seek(SeekFrom::Start(offset))
		.await
		.with_context(|| format!("could not seek {} to the resume point", local.display()))?;
	Ok(file)
}

/// Stamp the just-downloaded local file with the remote's modification time and, on a Unix
/// destination, its permission bits (§19). The twin of the upload's `stamp_remote`, best-effort
/// for the same reason: a filesystem that will not take the timestamp (some network mounts) is
/// logged and ignored — the bytes are on disk, which is the promise. The std calls block, so they
/// run on the blocking pool rather than the async reactor; a source with nothing to stamp makes no
/// call at all.
async fn stamp_local(local: &Path, mtime: Option<u32>, mode: Option<u32>) {
	if mtime.is_none() && mode.is_none() {
		return;
	}
	let path = local.to_path_buf();
	let _ = tokio::task::spawn_blocking(move || stamp_local_blocking(&path, mtime, mode)).await;
}

/// The blocking half of `stamp_local`: set the modification time (which std, unlike SFTP, lets us
/// set on its own — no coupled access time to worry about) and then the permission bits. The two
/// are independent, so one failing does not stop the other, and both only log on error since a
/// missing timestamp never unmakes the download.
fn stamp_local_blocking(local: &Path, mtime: Option<u32>, mode: Option<u32>) {
	if let Some(secs) = mtime {
		let when = UNIX_EPOCH + Duration::from_secs(u64::from(secs));
		// Setting the modification time needs the file opened for writing; open WITHOUT truncating
		// so the freshly written bytes are left in place.
		match std::fs::OpenOptions::new().write(true).open(local) {
			Ok(file) => {
				if let Err(error) = file.set_modified(when) {
					eprintln!(
						"could not set the modification time on {}: {error}",
						local.display()
					);
				}
			}
			Err(error) => {
				eprintln!(
					"could not open {} to set its modification time: {error}",
					local.display()
				);
			}
		}
	}
	apply_mode(local, mode);
}

/// Apply the source's Unix permission bits to the local file, on a Unix host that has them.
#[cfg(unix)]
fn apply_mode(local: &Path, mode: Option<u32>) {
	use std::os::unix::fs::PermissionsExt;
	// A let-chain (edition 2024) rather than two nested `if let`s: bind the source's mode bits and,
	// only if that succeeded, attempt the change — clippy's `collapsible_if` asks for exactly this,
	// and the flat form reads as the single guarded action it is.
	if let Some(bits) = mode
		&& let Err(error) = std::fs::set_permissions(local, std::fs::Permissions::from_mode(bits))
	{
		eprintln!("could not set permissions on {}: {error}", local.display());
	}
}

/// A Windows destination has no Unix permission bits to apply — the timestamp is all that carries
/// across, and the file keeps the default ACL it was created with.
#[cfg(not(unix))]
fn apply_mode(_local: &Path, _mode: Option<u32>) {}

/// Keep just the permission bits (`& 0o7777`) of a remote file's mode (§19). The upper bits are
/// the file type, which is the server's business, not something to stamp onto a plain local file.
fn remote_mode(meta: &FileAttributes) -> Option<u32> {
	meta.permissions.map(|bits| bits & 0o7777)
}

/// How far to probe for a free `name-1`, `name-2`… beside a local name already taken (§19), the
/// "keep both" answer to a tree download's collision. Bounded like every other free-name search
/// here: after a hundred the folder is telling us something.
const FREE_NAME_TRIES: u32 = 100;

/// Open the SFTP channel and hand a whole-folder download to a background task (§19). The mirror
/// of `start_tree` on the upload side: the transfer may pause to ask the user about a local file
/// already there, so it is handed the `answers` receiver `run()` keeps the other end of.
pub async fn start_tree<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	remote: String,
	local: PathBuf,
	resume: bool,
	answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(fetch_tree(
				sftp,
				remote,
				local,
				resume,
				events.clone(),
				answers,
				cancel,
			));
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::DownloadFailed(
					"Could not open an SFTP channel — the server may not offer the sftp \
					 subsystem."
						.to_string(),
				))
				.await;
		}
	}
}

/// Fetch the remote folder to this machine, reporting one terminal event (§19). Like the upload
/// twin, a clean run ends in `DownloadDone` with the local path, and both a cancel and a real
/// failure end in `DownloadFailed` — the message tells them apart.
async fn fetch_tree(
	sftp: SftpSession,
	remote_root: String,
	local_dir: PathBuf,
	resume: bool,
	events: mpsc::Sender<SshEvent>,
	mut answers: mpsc::Receiver<ConflictChoice>,
	cancel: Arc<AtomicBool>,
) {
	match receive_tree(
		&sftp,
		&remote_root,
		&local_dir,
		resume,
		&events,
		&mut answers,
		&cancel,
	)
	.await
	{
		Ok(Some(path)) => {
			let _ = events.send(SshEvent::DownloadDone(path)).await;
		}
		// A cancel — the ✕ (§16) or the conflict dialog's Cancel — ends neutrally and offers no
		// resume, since a cancel is final.
		Ok(None) => {
			let _ = events
				.send(SshEvent::DownloadFailed(
					"Folder download cancelled.".to_string(),
				))
				.await;
		}
		// A mid-flight failure keeps every byte already pulled (§16): a Resume re-walks the tree
		// and size-compares, sending down only what is still missing.
		Err(error) => {
			eprintln!("folder download interrupted: {error:#}");
			let _ = events
				.send(SshEvent::TransferInterrupted {
					message: format!("Folder download interrupted: {error} — Resume to continue."),
				})
				.await;
		}
	}
	let _ = sftp.close().await;
}

/// The tree download itself: recreate the remote folder under `local_dir`, then pull every file
/// into it, merging into whatever is already there and asking about each collision (§19). Returns
/// the local destination on success, or `None` when the user cancelled partway.
async fn receive_tree(
	sftp: &SftpSession,
	remote_root: &str,
	local_dir: &Path,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	cancel: &Arc<AtomicBool>,
) -> Result<Option<String>> {
	// The folder keeps its own name inside the destination, the same rule a single file follows.
	let local_target = local_dir.join(explorer::name(remote_root));
	let plan = walk_remote(sftp, remote_root)
		.await
		.with_context(|| format!("could not read {remote_root}"))?;
	let total = plan.total();

	// Create the destination and every subdirectory. `create_dir_all` makes parents too, so the
	// order the walk found them in does not matter here.
	tokio::fs::create_dir_all(&local_target)
		.await
		.with_context(|| format!("could not create {}", local_target.display()))?;
	for rel in &plan.dirs {
		let dir = transfer::local_join(&local_target, rel);
		tokio::fs::create_dir_all(&dir)
			.await
			.with_context(|| format!("could not create {}", dir.display()))?;
	}

	let mut received = 0u64;
	let mut reported = 0u64;
	let mut sticky: Option<Sticky> = None;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: received,
			total,
		})
		.await;

	for file in &plan.files {
		let dest = transfer::local_join(&local_target, &file.rel);
		let leaf = file.rel.last().map_or("", String::as_str);
		// A resume never prompts (§16): an existing local file is the transfer's own earlier work,
		// which `receive_file` size-compares and appends to. A first run treats it as a collision.
		let dest = if !resume && dest.exists() {
			match transfer::resolve(events, answers, &mut sticky, leaf).await {
				FileAction::Overwrite => dest,
				FileAction::KeepBoth => {
					let dir = dest
						.parent()
						.map_or_else(|| local_target.clone(), Path::to_path_buf);
					free_local(&dir, leaf)
				}
				FileAction::Skip => {
					received += file.size;
					let _ = events
						.send(SshEvent::TransferProgress {
							sent: received,
							total,
						})
						.await;
					continue;
				}
				FileAction::Cancel => return Ok(None),
			}
		} else {
			dest
		};
		let remote = transfer::remote_join(remote_root, &file.rel);
		// A cancel mid-file drops that file's partial and stops the whole tree (§16); files already
		// fully pulled stay.
		if receive_file(
			sftp,
			&remote,
			&dest,
			resume,
			file.size,
			events,
			&mut received,
			&mut reported,
			total,
			cancel,
		)
		.await? == CopyOutcome::Cancelled
		{
			return Ok(None);
		}
		// The file is fully on disk: stamp it with the metadata the walk captured off the remote
		// (§19), the per-file mirror of the single-file stamp.
		stamp_local(&dest, file.mtime, file.mode).await;
	}

	let _ = events
		.send(SshEvent::TransferProgress {
			sent: received,
			total,
		})
		.await;
	if plan.skipped_links > 0 {
		eprintln!("folder download skipped {} symlink(s)", plan.skipped_links);
	}
	Ok(Some(local_target.display().to_string()))
}

/// Fetch one remote file into a local one, folding its bytes into the tree-wide `received`
/// counter and emitting a progress event every `PROGRESS_STEP` (§19). The twin of the upload's
/// `send_file`, in the other direction — including the resume size-compare and the cancel poll
/// with its delete of the partial (§16).
#[allow(clippy::too_many_arguments)]
async fn receive_file(
	sftp: &SftpSession,
	remote: &str,
	local: &Path,
	resume: bool,
	size: u64,
	events: &mpsc::Sender<SshEvent>,
	received: &mut u64,
	reported: &mut u64,
	total: u64,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	let dest_size = if resume {
		tokio::fs::metadata(local).await.ok().map(|meta| meta.len())
	} else {
		None
	};
	let offset = match resume_start(resume, dest_size, size) {
		// Already fully here from before the interruption: count its bytes and move on.
		Start::Skip => {
			*received += size;
			let _ = events
				.send(SshEvent::TransferProgress {
					sent: *received,
					total,
				})
				.await;
			return Ok(CopyOutcome::Done);
		}
		Start::At(offset) => offset,
	};

	let mut source = sftp
		.open(remote.to_owned())
		.await
		.with_context(|| format!("could not open {remote} on the server"))?;
	if offset > 0 {
		source
			.seek(SeekFrom::Start(offset))
			.await
			.with_context(|| format!("could not seek {remote} to the resume point"))?;
		// The bytes already on disk count towards the running total straight away, so the bar
		// reflects the resumed progress rather than dropping back.
		*received += offset;
	}
	let mut destination = open_local_at(local, offset).await?;

	let mut buffer = vec![0u8; CHUNK];
	loop {
		if cancel.load(Ordering::Relaxed) {
			drop(destination);
			let _ = tokio::fs::remove_file(local).await;
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
		*received += read as u64;
		if *received - *reported >= PROGRESS_STEP {
			*reported = *received;
			let _ = events
				.send(SshEvent::TransferProgress {
					sent: *received,
					total,
				})
				.await;
		}
	}
	destination.flush().await.context("close failed")?;
	Ok(CopyOutcome::Done)
}

/// Walk a remote directory tree into the plan both directions share (§19). Iterative like the
/// local walk, using SFTP's typed listing so a directory is a directory because the server said
/// so; `.`/`..` are dropped and symlinks counted-and-skipped rather than followed (a link to a
/// folder would otherwise walk into somewhere outside the tree, or loop).
async fn walk_remote(sftp: &SftpSession, root: &str) -> Result<TreePlan> {
	let mut plan = TreePlan::default();
	let mut frontier: Vec<(String, Vec<String>)> = vec![(root.to_owned(), Vec::new())];
	while let Some((dir, rel)) = frontier.pop() {
		let entries = sftp
			.read_dir(dir.clone())
			.await
			.with_context(|| format!("could not list {dir}"))?;
		for entry in entries {
			let name = entry.file_name();
			if explorer::is_dot_link(&name) {
				continue;
			}
			let meta = entry.metadata();
			let mut child_rel = rel.clone();
			child_rel.push(name.clone());
			if meta.is_symlink() {
				plan.skipped_links += 1;
			} else if meta.is_dir() {
				plan.dirs.push(child_rel.clone());
				frontier.push((explorer::join(&dir, &name), child_rel));
			} else {
				// Capture the remote's metadata here, off the same listing, so the copy can be
				// stamped to match without a second round trip per file (§19).
				plan.files.push(PlannedFile {
					rel: child_rel,
					size: meta.len(),
					mtime: meta.mtime,
					atime: meta.atime,
					mode: remote_mode(&meta),
				});
			}
		}
	}
	Ok(plan)
}

/// The first free `name-1.ext`, `name-2.ext`… beside a local name already taken (§19) — the
/// "keep both" destination for a tree download. The twin of the upload's `free_remote`, but a
/// local `exists()` is cheap, so it needs no round-trip bound beyond the shared sanity cap.
fn free_local(dir: &Path, name: &str) -> PathBuf {
	let (stem, extension) = match name.rsplit_once('.') {
		Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
		// A dot-file (`.bashrc`) or a name with no dot at all: the whole thing is the stem.
		_ => (name, String::new()),
	};
	let mut candidate = dir.join(format!("{stem}-1{extension}"));
	for attempt in 2..=FREE_NAME_TRIES {
		if !candidate.exists() {
			break;
		}
		candidate = dir.join(format!("{stem}-{attempt}{extension}"));
	}
	candidate
}

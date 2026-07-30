// ssh/download.rs — pull a remote file down to this machine over SFTP (PLAN §19).
//
// The mirror image of `upload` (§17), and built the same way: its own sftp channel so
// the interactive shell is untouched, the transfer spawned so a large file never stalls
// the shell pump, and progress reported through the session's own event channel.
//
// There is no overwrite prompt here on purpose. The destination comes from the native
// save dialog, which already asks before replacing a local file — a second question in
// our own chrome would only be a second chance to answer it wrong.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;
use crate::ssh::transfer::{self, FileAction, Sticky, TreePlan};

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
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(fetch(sftp, remote, local, events.clone()));
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
async fn fetch(sftp: SftpSession, remote: String, local: PathBuf, events: mpsc::Sender<SshEvent>) {
	match copy(&sftp, &remote, &local, &events).await {
		Ok(()) => {
			let _ = events
				.send(SshEvent::DownloadDone(local.display().to_string()))
				.await;
		}
		Err(error) => {
			eprintln!("download failed: {error:#}");
			// The detail is the user's own file and path (not auth material, §12), so
			// showing it is what makes the failure actionable.
			let _ = events
				.send(SshEvent::DownloadFailed(format!(
					"Download failed: {error}"
				)))
				.await;
		}
	}
	let _ = sftp.close().await;
}

/// The copy loop: remote file in, local file out, a progress event every
/// `PROGRESS_STEP` bytes.
async fn copy(
	sftp: &SftpSession,
	remote: &str,
	local: &std::path::Path,
	events: &mpsc::Sender<SshEvent>,
) -> Result<()> {
	// The size is only for the progress bar, so a server that will not report one is not
	// a failure: the transfer then runs with an unknown total.
	let total = sftp
		.metadata(remote.to_owned())
		.await
		.ok()
		.and_then(|metadata| metadata.size)
		.unwrap_or(0);

	let mut source = sftp
		.open(remote.to_owned())
		.await
		.with_context(|| format!("could not open {remote} on the server"))?;
	// `create` truncates — which is what the user agreed to in the save dialog.
	let mut destination = tokio::fs::File::create(local)
		.await
		.with_context(|| format!("could not create {}", local.display()))?;

	let mut buffer = vec![0u8; CHUNK];
	let mut received = 0u64;
	let mut reported = 0u64;
	loop {
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
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: received,
			total,
		})
		.await;
	Ok(())
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
	answers: mpsc::Receiver<ConflictChoice>,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(fetch_tree(sftp, remote, local, events.clone(), answers));
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
	events: mpsc::Sender<SshEvent>,
	mut answers: mpsc::Receiver<ConflictChoice>,
) {
	match receive_tree(&sftp, &remote_root, &local_dir, &events, &mut answers).await {
		Ok(Some(path)) => {
			let _ = events.send(SshEvent::DownloadDone(path)).await;
		}
		Ok(None) => {
			let _ = events
				.send(SshEvent::DownloadFailed(
					"Folder download cancelled.".to_string(),
				))
				.await;
		}
		Err(error) => {
			eprintln!("folder download failed: {error:#}");
			let _ = events
				.send(SshEvent::DownloadFailed(format!(
					"Folder download failed: {error}"
				)))
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
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
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

	for (rel, size) in &plan.files {
		let dest = transfer::local_join(&local_target, rel);
		let leaf = rel.last().map_or("", String::as_str);
		let dest = if dest.exists() {
			match transfer::resolve(events, answers, &mut sticky, leaf).await {
				FileAction::Overwrite => dest,
				FileAction::KeepBoth => {
					let dir = dest
						.parent()
						.map_or_else(|| local_target.clone(), Path::to_path_buf);
					free_local(&dir, leaf)
				}
				FileAction::Skip => {
					received += size;
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
		let remote = transfer::remote_join(remote_root, rel);
		receive_file(
			sftp,
			&remote,
			&dest,
			events,
			&mut received,
			&mut reported,
			total,
		)
		.await?;
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
/// `send_file`, in the other direction.
async fn receive_file(
	sftp: &SftpSession,
	remote: &str,
	local: &Path,
	events: &mpsc::Sender<SshEvent>,
	received: &mut u64,
	reported: &mut u64,
	total: u64,
) -> Result<()> {
	let mut source = sftp
		.open(remote.to_owned())
		.await
		.with_context(|| format!("could not open {remote} on the server"))?;
	// `create` truncates, which is what the conflict answer already agreed to (overwrite, or a
	// free `name-1` that does not yet exist).
	let mut destination = tokio::fs::File::create(local)
		.await
		.with_context(|| format!("could not create {}", local.display()))?;

	let mut buffer = vec![0u8; CHUNK];
	loop {
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
	Ok(())
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
				plan.files.push((child_rel, meta.len()));
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

// ssh/upload.rs — send a local file to the remote over SFTP (PLAN §17).
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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;
use crate::ssh::transfer::{self, FileAction, Sticky, TreePlan};

/// How much of the file to move per write. 32 KiB sits comfortably under the SFTP
/// packet limit while keeping the number of round trips low.
const CHUNK: usize = 32 * 1024;

/// How many bytes to transfer between progress events. Reporting every chunk would
/// send hundreds of messages per megabyte and redraw the GUI just as often; this keeps
/// the bar moving smoothly without the flood.
const PROGRESS_STEP: u64 = 256 * 1024;

/// Open the SFTP channel and hand the transfer to a background task (§17). Opening
/// costs a couple of round trips and happens inline — the shell pauses for them — while
/// the transfer itself runs detached, so the terminal stays live throughout.
///
/// The session handle is borrowed, which is why the split exists: a spawned task cannot
/// hold a borrow, so the channel is opened here and only the owned `SftpSession` moves
/// into the task.
pub async fn start<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	local: PathBuf,
	remote: String,
	overwrite: bool,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(transfer(sftp, local, remote, overwrite, events.clone()));
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::UploadFailed(
					"Could not open an SFTP channel — the server may not offer the sftp \
					 subsystem."
						.to_string(),
				))
				.await;
		}
	}
}

/// How far to probe for a free `name-1`, `name-2`… when a batch upload's "keep both" answer
/// needs a destination that is not already taken (§17). Bounded like the download side's
/// local `free_name`: after a hundred the folder is telling us something.
const FREE_NAME_TRIES: u32 = 100;

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
pub async fn precheck<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	dir: String,
	names: Vec<String>,
) {
	let sftp = match super::open_sftp(session).await {
		Ok(sftp) => sftp,
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::UploadFailed(
					"Could not open an SFTP channel to check the destination.".to_string(),
				))
				.await;
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
/// destination for a name already taken (§17). The twin of the download side's local
/// `free_name`, but each probe is a round trip, so it is bounded to `FREE_NAME_TRIES`. An
/// existence check that errors is treated as "free" and stops the probe rather than spinning:
/// the transfer re-checks before it creates the file (§17), so a wrong guess is skipped, never
/// overwritten.
async fn free_remote(sftp: &SftpSession, dir: &str, name: &str) -> String {
	let (stem, extension) = match name.rsplit_once('.') {
		Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
		// A dot-file (`.bashrc`) or a name with no dot at all: the whole thing is the stem.
		_ => (name, String::new()),
	};
	for attempt in 1..=FREE_NAME_TRIES {
		let candidate = crate::explorer::join(dir, &format!("{stem}-{attempt}{extension}"));
		if !sftp.try_exists(&candidate).await.unwrap_or(false) {
			return candidate;
		}
	}
	crate::explorer::join(dir, &format!("{stem}-{FREE_NAME_TRIES}{extension}"))
}

/// Stream the file to the remote, reporting progress as it goes. Runs to completion in
/// its own task; every outcome is reported as exactly one terminal event.
async fn transfer(
	sftp: SftpSession,
	local: PathBuf,
	remote: String,
	overwrite: bool,
	events: mpsc::Sender<SshEvent>,
) {
	// A file already there is not an error — it is a question, and the user has already
	// been asked exactly once. Checking before opening the destination matters: SFTP's
	// create truncates, so by the time a write fails the old contents are gone.
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

	match copy(&sftp, &local, &remote, &events).await {
		Ok(()) => {
			// Report where the bytes actually landed: a relative path (what the dialog
			// offers when the shell's cwd is unknown) resolves against the login
			// directory, and the user should see the full path, not their own input.
			let resolved = sftp
				.canonicalize(remote.clone())
				.await
				.unwrap_or_else(|_| remote.clone());
			let _ = events.send(SshEvent::UploadDone(resolved)).await;
		}
		Err(error) => {
			eprintln!("upload failed: {error:#}");
			// Unlike an auth failure (§12), the detail here is the user's own file and
			// path — showing it is what makes the error actionable.
			let _ = events
				.send(SshEvent::UploadFailed(format!("Upload failed: {error}")))
				.await;
		}
	}
	let _ = sftp.close().await;
}

/// The copy loop: local file in, remote file out, a progress event every
/// `PROGRESS_STEP` bytes. Split from `transfer` so the outcome handling above reads as
/// one `match` over one `Result`.
async fn copy(
	sftp: &SftpSession,
	local: &Path,
	remote: &str,
	events: &mpsc::Sender<SshEvent>,
) -> Result<()> {
	let total = tokio::fs::metadata(local)
		.await
		.with_context(|| format!("could not read {}", local.display()))?
		.len();
	let mut source = tokio::fs::File::open(local)
		.await
		.with_context(|| format!("could not open {}", local.display()))?;
	let mut destination = sftp
		.create(remote.to_owned())
		.await
		.with_context(|| format!("could not create {remote} on the server"))?;

	let mut buffer = vec![0u8; CHUNK];
	let mut sent = 0u64;
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
		sent += read as u64;
		if sent - reported >= PROGRESS_STEP {
			reported = sent;
			let _ = events
				.send(SshEvent::TransferProgress { sent, total })
				.await;
		}
	}

	// `shutdown` flushes and closes the remote handle; without it the last writes can
	// still be in flight when we report success.
	destination.shutdown().await.context("close failed")?;
	let _ = events
		.send(SshEvent::TransferProgress { sent, total })
		.await;
	Ok(())
}

/// Open the SFTP channel and hand a whole-folder upload to a background task (§17). The mirror
/// of `start` for a single file, but the transfer may PAUSE mid-way to ask the user about a file
/// whose destination is already taken — so it is handed the `answers` receiver `run()` keeps the
/// other end of, and parks on it while the shell keeps flowing behind the prompt.
pub async fn start_tree<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	local: PathBuf,
	remote: String,
	answers: mpsc::Receiver<ConflictChoice>,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			tokio::spawn(transfer_tree(sftp, local, remote, events.clone(), answers));
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::UploadFailed(
					"Could not open an SFTP channel — the server may not offer the sftp \
					 subsystem."
						.to_string(),
				))
				.await;
		}
	}
}

/// Send the local folder to the remote, reporting one terminal event (§17). A clean run ends in
/// `UploadDone` with the folder's remote path; a user cancel and a real failure both end in
/// `UploadFailed` — the message tells them apart, and either way the shell is left untouched.
async fn transfer_tree(
	sftp: SftpSession,
	local_root: PathBuf,
	remote_dir: String,
	events: mpsc::Sender<SshEvent>,
	mut answers: mpsc::Receiver<ConflictChoice>,
) {
	match send_tree(&sftp, &local_root, &remote_dir, &events, &mut answers).await {
		Ok(Some(path)) => {
			let _ = events.send(SshEvent::UploadDone(path)).await;
		}
		// A cancel is not a failure, but it IS the terminal signal the GUI waits for to free the
		// transfer slot — a neutral message, so the status bar does not cry error over a choice.
		Ok(None) => {
			let _ = events
				.send(SshEvent::UploadFailed(
					"Folder upload cancelled.".to_string(),
				))
				.await;
		}
		Err(error) => {
			eprintln!("folder upload failed: {error:#}");
			let _ = events
				.send(SshEvent::UploadFailed(format!(
					"Folder upload failed: {error}"
				)))
				.await;
		}
	}
	let _ = sftp.close().await;
}

/// The tree upload itself: recreate the folder under `remote_dir`, then copy every file into it,
/// merging into whatever is already there and asking about each collision (§17). Returns the
/// destination path on success, or `None` when the user cancelled partway.
async fn send_tree(
	sftp: &SftpSession,
	local_root: &Path,
	remote_dir: &str,
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
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

	let mut sent = 0u64;
	let mut reported = 0u64;
	let mut sticky: Option<Sticky> = None;
	let _ = events
		.send(SshEvent::TransferProgress { sent, total })
		.await;

	for (rel, size) in &plan.files {
		let dest = transfer::remote_join(&remote_target, rel);
		let leaf = rel.last().map_or("", String::as_str);
		// A destination already taken is a question, not an overwrite (§17). Everything else
		// falls through to a plain write to `dest`.
		let dest = if sftp.try_exists(&dest).await.unwrap_or(false) {
			match transfer::resolve(events, answers, &mut sticky, leaf).await {
				FileAction::Overwrite => dest,
				FileAction::KeepBoth => {
					let dir = explorer::parent(&dest).unwrap_or(&remote_target).to_owned();
					free_remote(sftp, &dir, leaf).await
				}
				FileAction::Skip => {
					// Count the skipped bytes as handled so the bar still reaches the end.
					sent += size;
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
		let local = transfer::local_join(local_root, rel);
		send_file(sftp, &local, &dest, events, &mut sent, &mut reported, total).await?;
	}

	let _ = events
		.send(SshEvent::TransferProgress { sent, total })
		.await;
	if plan.skipped_links > 0 {
		eprintln!("folder upload skipped {} symlink(s)", plan.skipped_links);
	}
	Ok(Some(remote_target))
}

/// Copy one local file to the remote, folding its bytes into the tree-wide `sent` counter and
/// emitting a progress event every `PROGRESS_STEP` (§17). Split from `copy` because a tree's
/// progress runs across many files against one running total, not per file from zero.
async fn send_file(
	sftp: &SftpSession,
	local: &Path,
	remote: &str,
	events: &mpsc::Sender<SshEvent>,
	sent: &mut u64,
	reported: &mut u64,
	total: u64,
) -> Result<()> {
	let mut source = tokio::fs::File::open(local)
		.await
		.with_context(|| format!("could not open {}", local.display()))?;
	let mut destination = sftp
		.create(remote.to_owned())
		.await
		.with_context(|| format!("could not create {remote} on the server"))?;

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
		*sent += read as u64;
		if *sent - *reported >= PROGRESS_STEP {
			*reported = *sent;
			let _ = events
				.send(SshEvent::TransferProgress { sent: *sent, total })
				.await;
		}
	}
	destination.shutdown().await.context("close failed")?;
	Ok(())
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
}

/// Walk a local directory tree into the plan both transfer directions share (§17). Iterative,
/// not recursive, so a deep tree costs heap not stack; symlinks are counted and skipped rather
/// than followed, which is what keeps a cyclic link from walking forever.
///
/// `ponytail:` the whole listing is held in memory before a byte is sent — fine for an ordinary
/// folder, but a tree of millions of files would be felt. Upgrade path: stream the walk and the
/// transfer together, the way the files pane's batched listing does (§19).
async fn walk_local(root: &Path) -> Result<TreePlan> {
	let mut plan = TreePlan::default();
	// Each frontier item is a directory to read and its path RELATIVE to the root (empty for the
	// root itself, which is created by the caller, not listed here).
	let mut frontier: Vec<(PathBuf, Vec<String>)> = vec![(root.to_path_buf(), Vec::new())];
	while let Some((dir, rel)) = frontier.pop() {
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
			let mut child_rel = rel.clone();
			child_rel.push(name);
			let file_type = entry
				.file_type()
				.await
				.context("could not read a file type")?;
			if file_type.is_symlink() {
				plan.skipped_links += 1;
			} else if file_type.is_dir() {
				plan.dirs.push(child_rel.clone());
				frontier.push((entry.path(), child_rel));
			} else if file_type.is_file() {
				let size = entry.metadata().await.map(|meta| meta.len()).unwrap_or(0);
				plan.files.push((child_rel, size));
			}
			// Anything else — a fifo, a socket, a device node — is not a file to send.
		}
	}
	Ok(plan)
}

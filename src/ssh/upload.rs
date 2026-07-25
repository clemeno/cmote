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

use crate::bridge::SshEvent;

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
	match open(session).await {
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

/// Open a second channel on the live session and start its sftp subsystem.
async fn open<H: client::Handler>(session: &client::Handle<H>) -> Result<SftpSession> {
	let channel = session
		.channel_open_session()
		.await
		.context("could not open a channel for sftp")?;
	channel
		.request_subsystem(true, "sftp")
		.await
		.context("the server refused the sftp subsystem")?;
	SftpSession::new(channel.into_stream())
		.await
		.context("the sftp handshake failed")
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
			let _ = events.send(SshEvent::UploadProgress { sent, total }).await;
		}
	}

	// `shutdown` flushes and closes the remote handle; without it the last writes can
	// still be in flight when we report success.
	destination.shutdown().await.context("close failed")?;
	let _ = events.send(SshEvent::UploadProgress { sent, total }).await;
	Ok(())
}

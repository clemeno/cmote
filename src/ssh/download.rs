// ssh/download.rs — pull a remote file down to this machine over SFTP (PLAN §19).
//
// The mirror image of `upload` (§17), and built the same way: its own sftp channel so
// the interactive shell is untouched, the transfer spawned so a large file never stalls
// the shell pump, and progress reported through the session's own event channel.
//
// There is no overwrite prompt here on purpose. The destination comes from the native
// save dialog, which already asks before replacing a local file — a second question in
// our own chrome would only be a second chance to answer it wrong.

use std::path::PathBuf;

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::SshEvent;

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

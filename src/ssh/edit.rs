// ssh/edit.rs — read and write a whole remote file for the in-tab text editor (PLAN §32).
//
// The download/upload modules (§17, §19) stream a file to or from local disk; the editor works on
// an in-memory buffer instead, so this module is buffer-shaped: `load` reads the whole remote file
// into a `Vec<u8>` (bounded, since it all has to fit in a buffer the user edits), and `save` writes
// a `Vec<u8>` back atomically. Both open their own sftp channel off the live session — the shell and
// any transfer are untouched — and report exactly one terminal event.
//
// The bytes cross the bridge raw: encoding detection and re-encoding are the model's job on the GUI
// side (`editor`), so this layer never needs to know a file's charset.

use anyhow::{Context, Result, bail};
use russh::client;
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::bridge::SshEvent;

/// The largest file the editor will open (§32). The whole file becomes one editable in-memory
/// buffer, laid out every frame, so this is a guard against opening a disk image by accident, not a
/// judgement about text: 8 MiB is far past any config or script and well short of trouble.
pub const MAX_SIZE: u64 = 8 * 1024 * 1024;

/// How much to pull per read. The same order as the transfers' chunk (§17), tuned for SFTP's packet
/// size rather than for the buffer, which grows regardless.
const READ_CHUNK: usize = 64 * 1024;

/// The suffix a save's temp sibling wears before it is renamed over the target (§32). Kept in the
/// SAME directory as the file so the rename is a metadata move on one filesystem, never a copy.
const TEMP_SUFFIX: &str = "~cmote.tmp";

/// Open an sftp channel and read a whole remote file for the editor (§32), reporting `EditLoaded`
/// with its bytes or `EditLoadFailed` with a reason. `editor_id` is echoed back so the reply routes
/// to the editor tab that asked, not the session tab whose channel carried it.
pub async fn load<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	editor_id: u64,
	path: String,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			let outcome = read_file(&sftp, &path).await;
			let _ = sftp.close().await;
			match outcome {
				Ok(bytes) => {
					let _ = events
						.send(SshEvent::EditLoaded {
							editor_id,
							path,
							bytes,
						})
						.await;
				}
				Err(error) => {
					let _ = events
						.send(SshEvent::EditLoadFailed {
							editor_id,
							reason: format!("{error}"),
						})
						.await;
				}
			}
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::EditLoadFailed {
					editor_id,
					reason: "Could not open an SFTP channel — the server may not offer the sftp \
					         subsystem."
						.to_string(),
				})
				.await;
		}
	}
}

/// Read the file into a bounded buffer (§32). The size gate runs first off the metadata, so a huge
/// file is refused before a byte is pulled; a server that will not report a size is caught by a
/// second check as the buffer grows, so the cap holds either way.
async fn read_file(sftp: &SftpSession, path: &str) -> Result<Vec<u8>> {
	if let Ok(meta) = sftp.metadata(path.to_owned()).await
		&& let Some(size) = meta.size
		&& size > MAX_SIZE
	{
		bail!(
			"This file is {} — too large for the editor (limit {}).",
			human_size(size),
			human_size(MAX_SIZE)
		);
	}

	let mut file = sftp
		.open(path.to_owned())
		.await
		.with_context(|| format!("could not open {path} on the server"))?;
	let mut buffer = Vec::new();
	let mut chunk = vec![0u8; READ_CHUNK];
	loop {
		let read = file.read(&mut chunk).await.context("read failed")?;
		if read == 0 {
			break;
		}
		buffer.extend_from_slice(&chunk[..read]);
		if buffer.len() as u64 > MAX_SIZE {
			bail!(
				"This file is over the editor's {} limit.",
				human_size(MAX_SIZE)
			);
		}
	}
	Ok(buffer)
}

/// Open an sftp channel and write the editor's buffer back to the remote (§32), reporting `EditSaved`
/// or `EditSaveFailed`. The write is atomic: the bytes go to a temp sibling first and only a rename
/// makes them the file, so a connection dropped mid-write cannot leave a half-written file.
pub async fn save<H: client::Handler>(
	session: &client::Handle<H>,
	events: &mpsc::Sender<SshEvent>,
	editor_id: u64,
	path: String,
	bytes: Vec<u8>,
) {
	match super::open_sftp(session).await {
		Ok(sftp) => {
			let outcome = write_atomic(&sftp, &path, &bytes).await;
			let _ = sftp.close().await;
			match outcome {
				Ok(()) => {
					let _ = events.send(SshEvent::EditSaved { editor_id, path }).await;
				}
				Err(error) => {
					let _ = events
						.send(SshEvent::EditSaveFailed {
							editor_id,
							reason: format!("{error}"),
						})
						.await;
				}
			}
		}
		Err(error) => {
			eprintln!("sftp channel failed: {error:#}");
			let _ = events
				.send(SshEvent::EditSaveFailed {
					editor_id,
					reason: "Could not open an SFTP channel to save.".to_string(),
				})
				.await;
		}
	}
}

/// Write the bytes to a temp sibling, then rename it over the target (§32). The rename is the commit
/// point: until it lands the original file is untouched and the new content sits complete in the
/// temp. SFTP v3's rename refuses to overwrite an existing name, so a failed rename falls back to
/// removing the target and renaming again — the only non-atomic window is between that remove and
/// the rename, sub-millisecond. A failure best-effort removes the temp so a dead `.tmp` is not left
/// behind — EXCEPT the one case where doing so would lose data: if the target was already removed and
/// the second rename then fails, the temp is the file's only remaining copy, so it is kept and named
/// in the error for a manual rescue rather than deleted.
async fn write_atomic(sftp: &SftpSession, path: &str, bytes: &[u8]) -> Result<()> {
	let temp = format!("{path}{TEMP_SUFFIX}");

	// Write the whole buffer to the temp. `shutdown` flushes and closes the remote handle, so the
	// temp holds every byte before the rename that commits it.
	{
		let mut file = sftp
			.create(temp.clone())
			.await
			.with_context(|| format!("could not create {temp} on the server"))?;
		file.write_all(bytes).await.context("write failed")?;
		file.shutdown().await.context("close failed")?;
	}

	// Commit. A plain rename works when the target is absent (a Save As to a new name) or the server
	// allows overwrite; otherwise remove the stale target and rename into its place.
	if sftp.rename(temp.clone(), path.to_owned()).await.is_err() {
		// Remember whether the remove actually took: if it did, the original is now GONE and the temp
		// is the file's only copy of the new content — which decides what we may safely delete below.
		let removed_original = sftp.remove_file(path.to_owned()).await.is_ok();
		if let Err(error) = sftp.rename(temp.clone(), path.to_owned()).await {
			if removed_original {
				// The original is gone and the temp holds the whole new file. Deleting it now would
				// destroy the user's only copy, so keep it and name it — a stray `.tmp` is a far
				// smaller harm than data loss, and the content is then recoverable by hand.
				return Err(error).with_context(|| {
					format!(
						"could not replace {path}; your edits are saved on the server as {temp}"
					)
				});
			}
			// The original is untouched (its removal failed too), so the temp is a redundant copy —
			// drop it so nothing dangles, then report the failure.
			let _ = sftp.remove_file(temp).await;
			return Err(error).with_context(|| format!("could not replace {path}"));
		}
	}
	Ok(())
}

/// A file size in the terse `4.0 KB` / `8.0 MB` form the refusal message shows (§32). Local to this
/// module so the network layer carries no dependency on the files pane's own formatter.
fn human_size(bytes: u64) -> String {
	const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
	let mut value = bytes as f64;
	let mut unit = 0;
	while value >= 1024.0 && unit < UNITS.len() - 1 {
		value /= 1024.0;
		unit += 1;
	}
	if unit == 0 {
		format!("{bytes} {}", UNITS[0])
	} else {
		format!("{value:.1} {}", UNITS[unit])
	}
}

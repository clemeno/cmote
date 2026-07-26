// ssh/mod.rs — the SSH layer (PLAN §6-§8), all running on the tokio task.
//
// Split by responsibility so no single file owns the whole protocol:
//   client  — the russh Handler + the connect→auth→shell→stream task loop (§6)
//   auth    — method selection and attempts: publickey then password (§7)
//   hostkey — TOFU host-key verification against a portable known_hosts (§8)
//   keyfile — load PEM/OpenSSH keys and PuTTY .ppk, handle passphrases (§7)
//   upload  — send a local file to the remote over an sftp channel (§17)
//   browse  — read (and rename) remote folders for the explorer tree (§18)

pub mod auth;
pub mod browse;
pub mod client;
pub mod download;
pub mod hostkey;
pub mod keyfile;
pub mod upload;

use anyhow::{Context, Result};
// Aliased: this module already has a `client` submodule of its own, and the two names
// would collide in the type namespace.
use russh::client as russh_client;
use russh_sftp::client::{RawSftpSession, SftpSession};

/// Open a second channel on a live session and start its sftp subsystem (§17, §18).
///
/// Shared because both features that need file access reach for it, but they hold the
/// result differently: `upload` opens one per transfer and closes it at the end, while
/// `browse` keeps a single session open for the whole connection (a tree does many small
/// listings). Keeping the *opening* in one place means the two cannot disagree about how
/// the subsystem is requested.
pub async fn open_sftp<H: russh_client::Handler>(
	session: &russh_client::Handle<H>,
) -> Result<SftpSession> {
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

/// The same channel, but driven at the packet level (§20).
///
/// `browse` needs one thing the friendly `SftpSession` throws away: each entry's
/// `longname`, the `ls -l` line the server builds with the owner and group *names* it
/// resolved itself. `SftpSession::read_dir` keeps only the filename and the numeric
/// attributes, and its raw session is private — so the listing paths open the subsystem
/// this way instead and run `opendir`/`readdir`/`close` themselves. Same channel, same
/// handshake, same round trips; only the parsing layer differs.
pub async fn open_raw_sftp<H: russh_client::Handler>(
	session: &russh_client::Handle<H>,
) -> Result<RawSftpSession> {
	let channel = session
		.channel_open_session()
		.await
		.context("could not open a channel for sftp")?;
	channel
		.request_subsystem(true, "sftp")
		.await
		.context("the server refused the sftp subsystem")?;
	let raw = RawSftpSession::new(channel.into_stream());
	raw.init().await.context("the sftp handshake failed")?;
	Ok(raw)
}

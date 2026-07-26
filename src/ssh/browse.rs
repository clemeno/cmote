// ssh/browse.rs — read (and rename) remote folders for the explorer panel (PLAN §18).
//
// The tree needs one thing from the server: "what folders are inside this one?".
// Two ways to ask, tried in that order:
//
//   * **SFTP** — `read_dir` returns typed entries, so a directory is a directory
//     because the server said so, not because we recognised a character in some text.
//     Names containing spaces, quotes or newlines survive intact. The channel is opened
//     once and kept for the whole session (`Sftp` below), because a tree does many small
//     listings and paying two round trips of channel setup for each would be felt.
//   * **`ls` over an exec channel** — the fallback for a server with the sftp subsystem
//     switched off. It is text, so it is a guess; see the `ponytail:` note on `list_exec`.
//
// Either way the listing runs in a **spawned** task: the shell pump (`client::stream`)
// must stay free to move terminal bytes while a slow directory is being read. Only the
// channel opening happens inline, because that borrows the session handle — the same
// split `upload` makes, and for the same reason.

use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::{Channel, ChannelMsg, client};
use russh_sftp::client::SftpSession;
use tokio::sync::mpsc;

use crate::bridge::SshEvent;
use crate::explorer::shell_quote;

/// The most `ls` output the fallback will hold. A directory with a million entries (or a
/// server answering with something that is not a listing at all) must not grow our
/// memory without bound (§12); past this the listing fails cleanly instead.
const MAX_OUTPUT: usize = 1024 * 1024;

/// The session's SFTP channel for browsing, opened the first time a listing is asked for
/// and reused from then on. `refused` records a server that will not give us the
/// subsystem, so we ask once and fall back forever after rather than paying a failed
/// channel open per click.
///
/// The upload path (§17) deliberately keeps its own short-lived channel: it closes the
/// session when the transfer ends, which would take this one down with it.
#[derive(Default)]
pub struct Sftp {
	session: Option<Arc<SftpSession>>,
	refused: bool,
}

impl Sftp {
	/// The shared SFTP session, opening it on first use. `None` means the server does
	/// not offer the subsystem — the caller then falls back to `ls`.
	async fn get<H: client::Handler>(
		&mut self,
		session: &client::Handle<H>,
	) -> Option<Arc<SftpSession>> {
		if self.refused {
			return None;
		}
		if let Some(sftp) = self.session.as_ref() {
			return Some(sftp.clone());
		}
		match super::open_sftp(session).await {
			Ok(sftp) => {
				let sftp = Arc::new(sftp);
				self.session = Some(sftp.clone());
				Some(sftp)
			}
			Err(error) => {
				eprintln!("sftp browse channel unavailable, falling back to ls: {error:#}");
				self.refused = true;
				None
			}
		}
	}
}

/// List the folders inside `path` and report them as one `DirListed` (or one
/// `DirFailed`). Opens whatever channel it needs inline, then hands the work to a
/// spawned task so the shell keeps flowing.
pub async fn list<H: client::Handler>(
	session: &client::Handle<H>,
	sftp: &mut Sftp,
	events: &mpsc::Sender<SshEvent>,
	path: String,
) {
	if let Some(handle) = sftp.get(session).await {
		tokio::spawn(list_sftp(handle, path, events.clone()));
		return;
	}
	match session.channel_open_session().await {
		Ok(channel) => {
			tokio::spawn(list_exec(channel, path, events.clone()));
		}
		Err(error) => fail_dir(events, path, format!("Could not ask the server: {error}")).await,
	}
}

/// Rename a folder on the server, reporting `RenameDone` or `RenameFailed`. Same
/// channel choice as `list`.
pub async fn rename<H: client::Handler>(
	session: &client::Handle<H>,
	sftp: &mut Sftp,
	events: &mpsc::Sender<SshEvent>,
	from: String,
	to: String,
) {
	if let Some(handle) = sftp.get(session).await {
		tokio::spawn(rename_sftp(handle, from, to, events.clone()));
		return;
	}
	match session.channel_open_session().await {
		Ok(channel) => {
			tokio::spawn(rename_exec(channel, from, to, events.clone()));
		}
		Err(error) => {
			let _ = events
				.send(SshEvent::RenameFailed(format!(
					"Could not ask the server: {error}"
				)))
				.await;
		}
	}
}

/// The SFTP listing: ask for the directory's entries and keep the ones that are folders.
async fn list_sftp(sftp: Arc<SftpSession>, path: String, events: mpsc::Sender<SshEvent>) {
	match read_dirs(&sftp, &path).await {
		Ok(dirs) => {
			let _ = events.send(SshEvent::DirListed { path, dirs }).await;
		}
		Err(error) => fail_dir(&events, path, format!("{error}")).await,
	}
}

/// The folder names inside `path`. A symlink's own type says nothing about what it
/// points at, so each one is stat'ed (which follows it) and kept only if the target is a
/// directory — that costs a round trip per symlink, and only per symlink.
async fn read_dirs(sftp: &SftpSession, path: &str) -> Result<Vec<String>> {
	let entries = sftp
		.read_dir(path.to_owned())
		.await
		.with_context(|| format!("Could not list {path}"))?;

	let mut dirs = Vec::new();
	for entry in entries {
		let kind = entry.file_type();
		// `||` short-circuits, so a real directory costs nothing extra; only a symlink
		// pays the stat. A broken link errors there and is simply left out, which is
		// what it is.
		let is_dir = kind.is_dir()
			|| (kind.is_symlink()
				&& sftp
					.metadata(entry.path())
					.await
					.is_ok_and(|metadata| metadata.file_type().is_dir()));
		if is_dir {
			dirs.push(entry.file_name());
		}
	}
	Ok(dirs)
}

/// The SFTP rename. The destination is checked **first**: SFTP's own rename refuses an
/// occupied path on most servers but not all, and a folder quietly replaced is not
/// something the user can undo.
async fn rename_sftp(
	sftp: Arc<SftpSession>,
	from: String,
	to: String,
	events: mpsc::Sender<SshEvent>,
) {
	let event = match sftp.try_exists(to.clone()).await {
		Ok(true) => SshEvent::RenameFailed(format!("{to} already exists — nothing was renamed.")),
		// The server would not say whether the path is free; assuming it is could
		// destroy whatever is there.
		Err(error) => SshEvent::RenameFailed(format!("Could not check {to}: {error}")),
		Ok(false) => match sftp.rename(from.clone(), to.clone()).await {
			Ok(()) => SshEvent::RenameDone { from, to },
			Err(error) => SshEvent::RenameFailed(format!("Could not rename: {error}")),
		},
	};
	let _ = events.send(event).await;
}

/// The `ls` fallback: one line per entry, with `/` appended to directories (`-p`),
/// dot-entries included but `.`/`..` left out (`-A`), and `--` so a path starting with a
/// dash is a path and not a flag.
///
/// `ponytail:` this is text, and text lies. A folder whose name contains a newline is
/// read as two entries, and a symlink pointing at a directory is missed (`-p` marks only
/// real directories, and `-L` would turn every broken link into an error). Both are
/// correct on the SFTP path, which is what runs unless the server refuses the subsystem.
/// Upgrade path: `find -maxdepth 1 -type d -print0` where the server has it.
async fn list_exec(channel: Channel<client::Msg>, path: String, events: mpsc::Sender<SshEvent>) {
	let command = format!("ls -1Ap -- {}", shell_quote(&path));
	match exec(channel, command).await {
		Ok(output) => {
			let dirs = output
				.lines()
				.filter_map(|line| line.strip_suffix('/'))
				.map(str::to_owned)
				.collect();
			let _ = events.send(SshEvent::DirListed { path, dirs }).await;
		}
		Err(error) => fail_dir(&events, path, format!("Could not list {error}")).await,
	}
}

/// The `mv` fallback. The existence test and the move are one command so nothing can
/// slip into the destination between them, and `-e` catches a file, a folder or a
/// dangling symlink sitting there.
async fn rename_exec(
	channel: Channel<client::Msg>,
	from: String,
	to: String,
	events: mpsc::Sender<SshEvent>,
) {
	let command = format!(
		"if [ -e {to} ]; then echo 'already exists' >&2; exit 1; fi; mv -- {from} {to}",
		from = shell_quote(&from),
		to = shell_quote(&to),
	);
	let event = match exec(channel, command).await {
		Ok(_) => SshEvent::RenameDone { from, to },
		Err(error) => SshEvent::RenameFailed(format!("Could not rename: {error}")),
	};
	let _ = events.send(event).await;
}

/// Run one command on its own channel and collect its stdout. A non-zero exit becomes
/// an error carrying the command's own stderr, which is the part worth showing the user.
async fn exec(mut channel: Channel<client::Msg>, command: String) -> Result<String> {
	channel
		.exec(true, command)
		.await
		.context("the server refused to run a command")?;

	let mut out: Vec<u8> = Vec::new();
	let mut err: Vec<u8> = Vec::new();
	let mut status: Option<u32> = None;
	while let Some(message) = channel.wait().await {
		match message {
			ChannelMsg::Data { data } => {
				out.extend_from_slice(&data);
				if out.len() > MAX_OUTPUT {
					bail!("the listing is too large to read");
				}
			}
			ChannelMsg::ExtendedData { data, .. } => {
				// stderr is only ever shown as a short reason, so a cap well under the
				// stdout one is plenty.
				if err.len() < 4096 {
					err.extend_from_slice(&data);
				}
			}
			ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
			ChannelMsg::Eof | ChannelMsg::Close => break,
			_ => {}
		}
	}

	if status.is_some_and(|code| code != 0) {
		let reason = String::from_utf8_lossy(&err);
		let reason = reason.trim();
		bail!(
			"{}",
			if reason.is_empty() {
				"the command failed"
			} else {
				reason
			}
		);
	}
	Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Report a listing failure for one folder. The path is the user's own, so naming it is
/// what makes the message actionable (same call as an upload failure, §17).
async fn fail_dir(events: &mpsc::Sender<SshEvent>, path: String, reason: String) {
	eprintln!("listing {path} failed: {reason}");
	let _ = events.send(SshEvent::DirFailed { path, reason }).await;
}

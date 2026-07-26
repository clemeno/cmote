// ssh/browse.rs — read (and rename) remote folders for the explorer panel (PLAN §18).
//
// The tree needs one thing from the server: "what folders are inside this one?".
// Two ways to ask, tried in that order:
//
//   * **SFTP** — the listing is typed, so a directory is a directory because the server
//     said so, not because we recognised a character in some text. Names containing
//     spaces, quotes or newlines survive intact. The channel is opened once and kept for
//     the whole session (`Sftp` below), because a tree does many small listings and
//     paying two round trips of channel setup for each would be felt. It is a
//     `RawSftpSession` rather than the friendly `SftpSession`: the details popup wants the
//     owner and group *names*, which live only in each entry's `longname` — the `ls -l`
//     line the server resolved itself — and `read_dir` discards it (§20).
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
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{File, StatusCode};
use tokio::sync::mpsc;

use crate::bridge::SshEvent;
use crate::explorer::{join, shell_quote};
use crate::files::{self, Entry, Kind, Meta};

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
	session: Option<Arc<RawSftpSession>>,
	refused: bool,
	/// Whether the timezone probe has been sent for this session (§20). Asked once, on
	/// the first files-pane listing — every mtime in the pane is rendered against it.
	zone_asked: bool,
}

impl Sftp {
	/// The shared SFTP session, opening it on first use. `None` means the server does
	/// not offer the subsystem — the caller then falls back to `ls`.
	async fn get<H: client::Handler>(
		&mut self,
		session: &client::Handle<H>,
	) -> Option<Arc<RawSftpSession>> {
		if self.refused {
			return None;
		}
		if let Some(sftp) = self.session.as_ref() {
			return Some(sftp.clone());
		}
		match super::open_raw_sftp(session).await {
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

/// List EVERY entry inside `path` — files included — for the files pane (§19), and
/// report them as batches of `files::BATCH`. `request` identifies the listing so the
/// pane can drop batches for a directory it has already left.
pub async fn list_all<H: client::Handler>(
	session: &client::Handle<H>,
	sftp: &mut Sftp,
	events: &mpsc::Sender<SshEvent>,
	path: String,
	request: u64,
) {
	// The pane shows modification times, and a time needs the server's zone to be read
	// as the server's own clock (§20). Asked once, alongside the first listing.
	probe_zone(session, sftp, events).await;
	if let Some(handle) = sftp.get(session).await {
		tokio::spawn(all_sftp(handle, path, request, events.clone()));
		return;
	}
	match session.channel_open_session().await {
		Ok(channel) => {
			tokio::spawn(all_exec(channel, path, request, events.clone()));
		}
		Err(error) => {
			fail_files(
				events,
				request,
				format!("Could not ask the server: {error}"),
			)
			.await;
		}
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
async fn list_sftp(sftp: Arc<RawSftpSession>, path: String, events: mpsc::Sender<SshEvent>) {
	match read_dirs(&sftp, &path).await {
		Ok(dirs) => {
			let _ = events.send(SshEvent::DirListed { path, dirs }).await;
		}
		Err(error) => fail_dir(&events, path, format!("{error}")).await,
	}
}

/// Every name the server lists inside `path`, with its attributes and its `longname`
/// (§20) — `opendir`, then `readdir` until the server answers EOF, then `close`.
///
/// This is what `SftpSession::read_dir` does, minus the two things it discards: the
/// `longname` line the owner and group names live in, and `.`/`..`, which the model
/// drops at ingest anyway (`explorer::is_dot_link`, §19).
async fn read_names(sftp: &RawSftpSession, path: &str) -> Result<Vec<File>> {
	let handle = sftp
		.opendir(path.to_owned())
		.await
		.with_context(|| format!("Could not list {path}"))?
		.handle;

	let mut files = Vec::new();
	loop {
		match sftp.readdir(handle.as_str()).await {
			Ok(name) => files.extend(name.files),
			// The end of the directory, not a failure: the server says EOF once it has
			// handed over every name.
			Err(SftpError::Status(status)) if status.status_code == StatusCode::Eof => break,
			Err(error) => {
				// Give the handle back before leaving; a server has a finite number.
				let _ = sftp.close(handle).await;
				return Err(error).with_context(|| format!("Could not list {path}"));
			}
		}
	}
	let _ = sftp.close(handle).await;
	Ok(files)
}

/// The folder names inside `path`. A symlink's own type says nothing about what it
/// points at, so each one is stat'ed (which follows it) and kept only if the target is a
/// directory — that costs a round trip per symlink, and only per symlink.
async fn read_dirs(sftp: &RawSftpSession, path: &str) -> Result<Vec<String>> {
	let mut dirs = Vec::new();
	for file in read_names(sftp, path).await? {
		// `||` short-circuits, so a real directory costs nothing extra; only a symlink
		// pays the stat. A broken link errors there and is simply left out, which is
		// what it is.
		let is_dir = file.attrs.is_dir()
			|| (file.attrs.is_symlink()
				&& sftp
					.stat(join(path, &file.filename))
					.await
					.is_ok_and(|attrs| attrs.attrs.is_dir()));
		if is_dir {
			dirs.push(file.filename);
		}
	}
	Ok(dirs)
}

/// The SFTP listing for the files pane: every entry, sorted, then cut into batches.
///
/// `ponytail:` the batching bounds the MESSAGE size and the relayout, not the fetch —
/// russh-sftp's `read_dir` runs the whole readdir loop before it returns. That costs
/// nothing extra in round trips (SFTP sends a name's attributes along with the name, so
/// there is no per-file stat either way) but it does hold the whole listing in memory
/// once. Upgrade path: drive `RawSftpSession::opendir`/`readdir` directly and emit a
/// batch per protocol packet.
async fn all_sftp(
	sftp: Arc<RawSftpSession>,
	path: String,
	request: u64,
	events: mpsc::Sender<SshEvent>,
) {
	match read_entries(&sftp, &path).await {
		Ok(mut entries) => {
			files::sort(&mut entries);
			send_batches(&events, request, entries).await;
		}
		Err(error) => fail_files(&events, request, format!("{error}")).await,
	}
}

/// Every entry inside `path`, with the kind and the details the server reported (§19,
/// §20). A symlink keeps its own kind rather than being followed: resolving each one
/// costs a round trip, and a crowded directory is exactly where that adds up — the pane
/// asks for the one link the user selects instead (`read_link`).
async fn read_entries(sftp: &RawSftpSession, path: &str) -> Result<Vec<Entry>> {
	Ok(read_names(sftp, path)
		.await?
		.into_iter()
		.map(entry_of)
		.collect())
}

/// Turn one listed name into a pane entry (§20). The size, time and ids ride along with
/// the name — SFTP sends a directory's attributes with its listing, so none of this costs
/// an extra round trip.
fn entry_of(file: File) -> Entry {
	let kind = if file.attrs.is_dir() {
		Kind::Dir
	} else if file.attrs.is_symlink() {
		Kind::Link
	} else {
		Kind::File
	};
	// Names first, from the server's own `ls -l` line; the numeric ids are the fallback
	// for a server that sends no longname (SFTP v3 carries no names in the attributes).
	let (owner, group) = match files::parse_longname(&file.longname) {
		Some((owner, group)) => (Some(owner), Some(group)),
		None => (
			file.attrs
				.user
				.clone()
				.or_else(|| file.attrs.uid.map(|uid| uid.to_string())),
			file.attrs
				.group
				.clone()
				.or_else(|| file.attrs.gid.map(|gid| gid.to_string())),
		),
	};
	Entry {
		name: file.filename,
		kind,
		meta: Meta {
			size: file.attrs.size,
			mtime: file.attrs.mtime,
			owner,
			group,
		},
	}
}

/// Send a listing as `FilesChunk` batches, the last one flagged `done`. An empty
/// directory still sends one empty batch — that is what tells the pane to stop waiting.
async fn send_batches(events: &mpsc::Sender<SshEvent>, request: u64, entries: Vec<Entry>) {
	let total = entries.len();
	let mut sent = 0;
	loop {
		let batch = entries[sent..(sent + files::BATCH).min(total)].to_vec();
		sent += batch.len();
		let done = sent == total;
		let delivered = events
			.send(SshEvent::FilesChunk {
				request,
				entries: batch,
				done,
			})
			.await
			.is_ok();
		// Stop on the last batch — or the moment the GUI stops listening.
		if done || !delivered {
			return;
		}
	}
}

/// The `ls` fallback for the files pane. `-F` appends a type indicator — `/` directory,
/// `@` symlink, and `*`/`|`/`=` for executables, fifos and sockets, which are all files
/// as far as this pane is concerned.
///
/// `ponytail:` same caveat as `list_exec` — this is text, so a name containing a newline
/// is read as two entries, and a name genuinely ending in one of the indicator characters
/// loses it. Correct on the SFTP path, which is what runs unless the server refuses the
/// subsystem.
async fn all_exec(
	channel: Channel<client::Msg>,
	path: String,
	request: u64,
	events: mpsc::Sender<SshEvent>,
) {
	let command = format!("ls -1AF -- {}", shell_quote(&path));
	match exec(channel, command).await {
		Ok(output) => {
			let mut entries: Vec<Entry> = output
				.lines()
				.filter(|line| !line.is_empty())
				// No size, time or owner on this path: `ls -1AF` reports none of it, and
				// asking for them would be a second, differently-shaped listing to parse.
				// The details popup shows the type and leaves the rest blank (§20).
				.map(|line| match line.strip_suffix('/') {
					Some(name) => Entry {
						name: name.to_owned(),
						kind: Kind::Dir,
						meta: Meta::default(),
					},
					None => match line.strip_suffix('@') {
						Some(name) => Entry {
							name: name.to_owned(),
							kind: Kind::Link,
							meta: Meta::default(),
						},
						None => Entry {
							name: line.trim_end_matches(['*', '|', '=']).to_owned(),
							kind: Kind::File,
							meta: Meta::default(),
						},
					},
				})
				.collect();
			files::sort(&mut entries);
			send_batches(&events, request, entries).await;
		}
		Err(error) => fail_files(&events, request, format!("Could not list {error}")).await,
	}
}

/// The SFTP rename. The destination is checked **first**: SFTP's own rename refuses an
/// occupied path on most servers but not all, and a folder quietly replaced is not
/// something the user can undo.
async fn rename_sftp(
	sftp: Arc<RawSftpSession>,
	from: String,
	to: String,
	events: mpsc::Sender<SshEvent>,
) {
	// A `stat` that comes back "no such file" is the only answer that means the
	// destination is free. Anything else — it exists, or the server would not say — must
	// not lead to a rename: a folder quietly replaced is not something the user can undo.
	let event = match sftp.stat(to.clone()).await {
		Ok(_) => SshEvent::RenameFailed(format!("{to} already exists — nothing was renamed.")),
		Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
			match sftp.rename(from.clone(), to.clone()).await {
				Ok(_) => SshEvent::RenameDone { from, to },
				Err(error) => SshEvent::RenameFailed(format!("Could not rename: {error}")),
			}
		}
		Err(error) => SshEvent::RenameFailed(format!("Could not check {to}: {error}")),
	};
	let _ = events.send(event).await;
}

/// Resolve one symlink for the details popup (§20), reporting `LinkTarget` — or nothing
/// at all, since a link that will not resolve (a broken one, a server that refuses)
/// simply leaves the popup without that line.
///
/// One link at a time, on the user's selection: doing it for every entry in a listing is
/// a round trip per link, which is the cost the pane exists to avoid (§19). No `ls`
/// fallback either — `readlink` is SFTP's own, and the text fallback has no metadata to
/// show this beside.
pub async fn read_link<H: client::Handler>(
	session: &client::Handle<H>,
	sftp: &mut Sftp,
	events: &mpsc::Sender<SshEvent>,
	path: String,
) {
	let Some(handle) = sftp.get(session).await else {
		return;
	};
	let events = events.clone();
	tokio::spawn(async move {
		let Ok(name) = handle.readlink(path.clone()).await else {
			return;
		};
		if let Some(file) = name.files.first() {
			let _ = events
				.send(SshEvent::LinkTarget {
					path,
					target: file.filename.clone(),
				})
				.await;
		}
	});
}

/// Ask the server what timezone it is in, once per session (§20): `date +'%z %Z'` on an
/// exec channel. Nothing is reported when the probe fails — the pane then renders its
/// times as UTC, which is right about the instant if not about the wall clock.
async fn probe_zone<H: client::Handler>(
	session: &client::Handle<H>,
	sftp: &mut Sftp,
	events: &mpsc::Sender<SshEvent>,
) {
	if sftp.zone_asked {
		return;
	}
	sftp.zone_asked = true;
	let Ok(channel) = session.channel_open_session().await else {
		return;
	};
	let events = events.clone();
	tokio::spawn(async move {
		let Ok(output) = exec(channel, "date +'%z %Z'".to_owned()).await else {
			return;
		};
		if let Some(zone) = files::parse_zone(&output) {
			let _ = events.send(SshEvent::Zone(zone)).await;
		}
	});
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

/// The same, for a files-pane listing (§19). Carries the request number so a failure
/// arriving after the user has moved on is dropped rather than shown.
async fn fail_files(events: &mpsc::Sender<SshEvent>, request: u64, reason: String) {
	eprintln!("files listing failed: {reason}");
	let _ = events.send(SshEvent::FilesFailed { request, reason }).await;
}

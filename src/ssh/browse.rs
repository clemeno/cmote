// ssh/browse.rs — read (and rename) remote folders for the explorer pane (PLAN §18, §46).
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
//   * **`ls` over an exec channel** (`shellfs`) — the fallback for a server with the sftp
//     subsystem switched off. It is text, so it is a guess; see the `ponytail:` note there.
//
// Since §46 neither is necessarily the LOGIN account's. Which account a listing reads as, and
// which of the two ways is available for it, is decided by `asuser::Accounts` before the work
// starts and arrives here as one `Browse` value — so nothing in this file has to know that
// accounts exist. A third case comes with it: `Browse::Denied`, for an account whose files cannot
// be reached at all, which is reported as the listing failing rather than silently answered by
// some other account.
//
// Either way the listing runs in a **spawned** task: the shell pump (`client::stream`)
// must stay free to move terminal bytes while a slow directory is being read.

use std::sync::Arc;

use anyhow::{Context, Result};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{File, FileAttributes, StatusCode};
use tokio::sync::mpsc;

use super::asuser::{Browse, Runner};
use super::shellfs;
use crate::bridge::SshEvent;
use crate::explorer::join;
use crate::files::{self, Entry, FilesKind, Meta};

/// List the folders inside `path` and report them as one `DirListed` (or one `DirFailed`),
/// reading as whichever account `backend` belongs to (§46).
pub async fn list(backend: Browse, events: &mpsc::Sender<SshEvent>, path: String) {
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(list_sftp(sftp, path, events.clone()));
		}
		Browse::Shell(runner) => {
			tokio::spawn(list_shell(runner, path, events.clone()));
		}
		Browse::Denied(reason) => fail_dir(events, path, reason).await,
	}
}

/// List EVERY entry inside `path` — files included — for the files pane (§19), and
/// report them as batches of `files::BATCH`. `request` identifies the listing so the
/// pane can drop batches for a directory it has already left.
pub async fn list_all(
	backend: Browse,
	events: &mpsc::Sender<SshEvent>,
	path: String,
	request: u64,
) {
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(all_sftp(sftp, path, request, events.clone()));
		}
		Browse::Shell(runner) => {
			tokio::spawn(all_shell(runner, path, request, events.clone()));
		}
		Browse::Denied(reason) => fail_files(events, request, reason).await,
	}
}

/// Rename a folder on the server, reporting `RenameDone` or `RenameFailed`.
pub async fn rename(backend: Browse, events: &mpsc::Sender<SshEvent>, from: String, to: String) {
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(rename_sftp(sftp, from, to, events.clone()));
		}
		Browse::Shell(runner) => {
			tokio::spawn(rename_shell(runner, from, to, events.clone()));
		}
		Browse::Denied(reason) => {
			let _ = events.send(SshEvent::RenameFailed(reason)).await;
		}
	}
}

/// Create a new folder on the server, reporting `MakeDirDone` or `MakeDirFailed` (§18).
pub async fn make_dir(backend: Browse, events: &mpsc::Sender<SshEvent>, path: String) {
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(make_dir_sftp(sftp, path, events.clone()));
		}
		Browse::Shell(runner) => {
			tokio::spawn(make_dir_shell(runner, path, events.clone()));
		}
		Browse::Denied(reason) => {
			let _ = events.send(SshEvent::MakeDirFailed(reason)).await;
		}
	}
}

/// Delete remote entries, reporting one `DeleteDone` for the whole set or one `DeleteFailed`
/// (§18). Each path is removed whatever it is — a file, a symlink (unlinked, never followed), or
/// a folder and its whole subtree. On the SFTP path the removal is a walk this module drives, on
/// the shell fallback a single `rm -rf`.
pub async fn remove(backend: Browse, events: &mpsc::Sender<SshEvent>, paths: Vec<String>) {
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(remove_sftp(sftp, paths, events.clone()));
		}
		Browse::Shell(runner) => {
			tokio::spawn(remove_shell(runner, paths, events.clone()));
		}
		Browse::Denied(reason) => {
			let _ = events.send(SshEvent::DeleteFailed(reason)).await;
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
		FilesKind::Dir
	} else if file.attrs.is_symlink() {
		FilesKind::Link
	} else {
		FilesKind::File
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
			// The numeric mode carries the type and permission bits together; render it the
			// way `ls -l` reads (§20). Absent only if this server sent no permissions flag.
			mode: file.attrs.permissions.map(files::format_mode),
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

/// The SFTP folder creation. Like the rename, the destination is checked FIRST — `lstat`, so a
/// symlink sitting there is seen as itself rather than followed — because `mkdir` on an occupied
/// path gives a terse server error, and "already exists" is the reason worth showing (§18).
async fn make_dir_sftp(sftp: Arc<RawSftpSession>, path: String, events: mpsc::Sender<SshEvent>) {
	let event = match sftp.lstat(path.clone()).await {
		Ok(_) => SshEvent::MakeDirFailed(format!("{path} already exists — nothing was created.")),
		Err(SftpError::Status(status)) if status.status_code == StatusCode::NoSuchFile => {
			// The default attributes let the server apply the connecting user's umask, the same
			// permissions a plain `mkdir` at the shell would give.
			match sftp.mkdir(path.clone(), FileAttributes::default()).await {
				Ok(_) => SshEvent::MakeDirDone(path),
				Err(error) => {
					SshEvent::MakeDirFailed(format!("Could not create the folder: {error}"))
				}
			}
		}
		Err(error) => SshEvent::MakeDirFailed(format!("Could not check {path}: {error}")),
	};
	let _ = events.send(event).await;
}

/// The SFTP delete: remove each target in turn, walking a folder's whole subtree. A failure
/// stops at the first one and names it — a delete that half-happened is worth being told about,
/// and the panes re-list either way so what did go survives the message (§18).
async fn remove_sftp(
	sftp: Arc<RawSftpSession>,
	paths: Vec<String>,
	events: mpsc::Sender<SshEvent>,
) {
	for path in &paths {
		if let Err(error) = remove_tree(&sftp, path).await {
			let _ = events
				.send(SshEvent::DeleteFailed(format!(
					"Could not delete {path}: {error}"
				)))
				.await;
			return;
		}
	}
	let _ = events.send(SshEvent::DeleteDone(paths)).await;
}

/// Remove one entry whatever it is (§18). A symlink is seen by `lstat` as itself and unlinked
/// with `remove`, NEVER followed — following it would delete whatever it points at. A plain file
/// is unlinked the same way; a real directory is emptied and then removed by `remove_subtree`.
async fn remove_tree(sftp: &RawSftpSession, root: &str) -> Result<()> {
	let attrs = sftp
		.lstat(root.to_owned())
		.await
		.with_context(|| format!("could not stat {root}"))?;
	if attrs.attrs.is_dir() {
		remove_subtree(sftp, root).await
	} else {
		sftp.remove(root.to_owned())
			.await
			.map(|_| ())
			.with_context(|| format!("could not remove {root}"))
	}
}

/// Empty a directory and remove it (§18). Breadth-first rather than recursive so a deep tree
/// costs heap, not stack: every descendant is discovered into `dirs` (parents before children)
/// and `files`, then the files are unlinked and the directories removed DEEPEST FIRST — a
/// directory only goes once nothing inside it is left. A symlink to a folder is a file here (its
/// own `lstat` type is a link), so it is unlinked, not descended into.
async fn remove_subtree(sftp: &RawSftpSession, root: &str) -> Result<()> {
	let mut dirs = vec![root.to_owned()];
	let mut files: Vec<String> = Vec::new();
	let mut frontier = vec![root.to_owned()];
	while let Some(dir) = frontier.pop() {
		for entry in read_names(sftp, &dir).await? {
			let child = join(&dir, &entry.filename);
			if entry.attrs.is_dir() {
				dirs.push(child.clone());
				frontier.push(child);
			} else {
				files.push(child);
			}
		}
	}

	for file in &files {
		sftp.remove(file.clone())
			.await
			.with_context(|| format!("could not remove {file}"))?;
	}
	// Deepest first: `dirs` is in discovery order (a parent before its children), so removing it
	// in reverse takes the children before the parent — which is what `rmdir` needs.
	for dir in dirs.iter().rev() {
		sftp.rmdir(dir.clone())
			.await
			.with_context(|| format!("could not remove {dir}"))?;
	}
	Ok(())
}

/// Resolve one symlink for the details popup (§20), reporting `LinkTarget` — or nothing
/// at all, since a link that will not resolve (a broken one, a server that refuses)
/// simply leaves the popup without that line.
///
/// One link at a time, on the user's selection: doing it for every entry in a listing is
/// a round trip per link, which is the cost the pane exists to avoid (§19).
pub fn read_link(backend: Browse, events: &mpsc::Sender<SshEvent>, path: String) {
	let events = events.clone();
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(async move {
				let Ok(name) = sftp.readlink(path.clone()).await else {
					return;
				};
				if let Some(file) = name.files.first() {
					report_link(&events, path, file.filename.clone()).await;
				}
			});
		}
		Browse::Shell(runner) => {
			tokio::spawn(async move {
				if let Some(target) = shellfs::read_link(&runner, &path).await {
					report_link(&events, path, target).await;
				}
			});
		}
		// A link whose target cannot be read leaves the popup without that line, which is
		// exactly what a broken link does — nothing to report.
		Browse::Denied(_) => {}
	}
}

/// Send one resolved symlink to the GUI.
async fn report_link(events: &mpsc::Sender<SshEvent>, path: String, target: String) {
	let _ = events.send(SshEvent::LinkTarget { path, target }).await;
}

/// Ask the server what timezone it is in, once per session (§20): `date +'%z %Z'`. Nothing is
/// reported when the probe fails — the pane then renders its times as UTC, which is right about
/// the instant if not about the wall clock.
///
/// Runs as whichever account is selected, because it needs no privilege either way; the zone
/// belongs to the machine, so `asuser::Accounts` only ever lets this be asked once.
pub fn probe_zone(runner: Runner, events: &mpsc::Sender<SshEvent>) {
	let events = events.clone();
	tokio::spawn(async move {
		let Ok(output) = runner.stdout("date +'%z %Z'").await else {
			return;
		};
		if let Some(zone) = files::parse_zone(&output) {
			let _ = events.send(SshEvent::Zone(zone)).await;
		}
	});
}

/// Ask the server where the login shell stands, once per session (§160), so the panes can open
/// there instead of at `/`. Reported as `LoginDir`; nothing at all when the remote will not say.
///
/// SFTP resolves `.`, which on a freshly opened session is the home directory — the server starts
/// every sftp session there. The shell backend asks for `$HOME` (`shellfs::home`), the same answer
/// by the other road. Both run on the browse channel the first listing opens anyway, which is why
/// this takes a `Browse` rather than the `AsuserFiles` the shell-config errand uses (§17).
///
/// A path that is not absolute is dropped rather than reported. The tree is rooted at `/` and a
/// remote that answered `C:\Users\…` has nowhere on it to hang — the same `ponytail:` the explorer's
/// own `reveal` records, refused here so the pane cannot be sent somewhere the tree cannot follow.
pub fn probe_login_dir(backend: Browse, events: &mpsc::Sender<SshEvent>) {
	let events = events.clone();
	match backend {
		Browse::Sftp(sftp) => {
			tokio::spawn(async move {
				let Ok(name) = sftp.realpath(".".to_owned()).await else {
					return;
				};
				if let Some(file) = name.files.first() {
					report_login_dir(&events, file.filename.clone()).await;
				}
			});
		}
		Browse::Shell(runner) => {
			tokio::spawn(async move {
				if let Ok(home) = shellfs::home(&runner).await {
					report_login_dir(&events, home).await;
				}
			});
		}
		// An account whose files cannot be reached at all has no directory to offer, and the
		// listing that follows will report the refusal in its own words.
		Browse::Denied(_) => {}
	}
}

/// Send one login directory to the GUI, if it is a path the tree can hold.
async fn report_login_dir(events: &mpsc::Sender<SshEvent>, path: String) {
	let path = path.trim();
	if !path.starts_with('/') {
		return;
	}
	let _ = events.send(SshEvent::LoginDir(path.to_owned())).await;
}

/// The shell-backend listing for the tree: `ls` under whichever account this is (§46).
async fn list_shell(runner: Runner, path: String, events: mpsc::Sender<SshEvent>) {
	match shellfs::dirs(&runner, &path).await {
		Ok(dirs) => {
			let _ = events.send(SshEvent::DirListed { path, dirs }).await;
		}
		Err(error) => fail_dir(&events, path, format!("{error}")).await,
	}
}

/// The shell-backend listing for the files pane.
async fn all_shell(runner: Runner, path: String, request: u64, events: mpsc::Sender<SshEvent>) {
	match shellfs::entries(&runner, &path).await {
		Ok(entries) => send_batches(&events, request, entries).await,
		Err(error) => fail_files(&events, request, format!("{error}")).await,
	}
}

/// The shell-backend rename: `mv`, refusing an occupied destination.
async fn rename_shell(runner: Runner, from: String, to: String, events: mpsc::Sender<SshEvent>) {
	let event = match shellfs::rename(&runner, &from, &to).await {
		Ok(()) => SshEvent::RenameDone { from, to },
		Err(error) => SshEvent::RenameFailed(format!("Could not rename: {error}")),
	};
	let _ = events.send(event).await;
}

/// The shell-backend folder creation: `mkdir`, refusing an occupied path.
async fn make_dir_shell(runner: Runner, path: String, events: mpsc::Sender<SshEvent>) {
	let event = match shellfs::make_dir(&runner, &path).await {
		Ok(()) => SshEvent::MakeDirDone(path),
		Err(error) => SshEvent::MakeDirFailed(format!("Could not create the folder: {error}")),
	};
	let _ = events.send(event).await;
}

/// The shell-backend delete: one `rm -rf` for the whole set.
async fn remove_shell(runner: Runner, paths: Vec<String>, events: mpsc::Sender<SshEvent>) {
	let event = match shellfs::remove(&runner, &paths).await {
		Ok(()) => SshEvent::DeleteDone(paths),
		Err(error) => SshEvent::DeleteFailed(format!("Could not delete: {error}")),
	};
	let _ = events.send(event).await;
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

#[cfg(test)]
mod tests {
	use super::*;

	/// What the login-directory probe reports, and what it refuses to (§160). The two remote answers
	/// are hard to reach — one needs an SFTP server, the other a shell — but the rule about what may
	/// be reported at all is one function, and it is the one with a consequence: the tree is rooted
	/// at `/`, so a path that is not absolute has nowhere on it to hang, and sending it would move
	/// the files pane somewhere the tree could not follow.
	#[tokio::test]
	async fn only_an_absolute_path_is_reported_as_the_login_directory() {
		let reported = |answer: &str| {
			let answer = answer.to_owned();
			async move {
				let (tx, mut rx) = mpsc::channel(1);
				report_login_dir(&tx, answer).await;
				match rx.try_recv() {
					Ok(SshEvent::LoginDir(path)) => Some(path),
					_ => None,
				}
			}
		};

		// The ordinary answer, with the newline `pwd` and `$HOME` alike come back wearing.
		assert_eq!(
			reported("/home/u\n").await,
			Some("/home/u".to_owned()),
			"trimmed and passed on"
		);
		// A Windows remote answers with a drive, which is not a place on this tree (§17, §18).
		assert_eq!(reported(r"C:\Users\CLEm").await, None, "no drive letters");
		// And a remote that says nothing says nothing.
		assert_eq!(reported("   ").await, None, "nor an empty answer");
	}
}

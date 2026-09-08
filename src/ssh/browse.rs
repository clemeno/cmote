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

use std::future::Future;
use std::sync::Arc;

use anyhow::{Context, Result};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{Attrs, File, FileAttributes, Handle, Name, Status, StatusCode};
use tokio::sync::mpsc;

use super::asuser::{Browse, Exec, Runner};
use super::shellfs;
use crate::bridge::SshEvent;
use crate::explorer::join;
use crate::files::{self, Entry, FilesKind, Meta};

/// List the folders inside `path` and report them as one `DirListed` (or one `DirFailed`),
/// reading as whichever account `backend` belongs to (§46).
pub async fn list(backend: Browse, events: &mpsc::Sender<SshEvent>, path: String) {
	match backend {
		Browse::Sftp { sftp, runner } => {
			tokio::spawn(list_sftp(sftp, runner, path, events.clone()));
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
		Browse::Sftp { sftp, .. } => {
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
		Browse::Sftp { sftp, .. } => {
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
		Browse::Sftp { sftp, .. } => {
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
		Browse::Sftp { sftp, .. } => {
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
async fn list_sftp(
	sftp: Arc<RawSftpSession>,
	runner: Runner,
	path: String,
	events: mpsc::Sender<SshEvent>,
) {
	match dirs_inside(&sftp, &runner, &path).await {
		Ok(dirs) => {
			let _ = events.send(SshEvent::DirListed { path, dirs }).await;
		}
		Err(error) => fail_dir(&events, path, format!("{error}")).await,
	}
}

/// How many waves of the SFTP walk are read before the shell is asked instead (§167).
///
/// Neither way is faster than the other for every folder, so the choice is made by WATCHING rather
/// than guessing. A small folder answers in one wave over the SFTP channel that is already open —
/// measured at 0.32-0.53 s for the folders along one path — while `find` needs a channel of its
/// own, and a channel open plus exec plus close is two or three round trips, so asking the shell
/// first would make every ordinary tree click about twice as slow.
///
/// A folder that has not finished after this many waves is a folder where the walk is the wrong
/// tool: `.../processed` took 11.6 s of it, against roughly a second for `find`. So the walk starts,
/// and once a wave arrives AFTER the last allowed one — the folder holds more than this many waves'
/// worth of names — it is abandoned and the question asked the other way. That arrival is the whole
/// signal, so a folder whose names run out exactly on the last allowed wave still counts as
/// answered by the walk. The waves already read are not wasted work avoided: they are the round trip
/// the walk would have cost anyway.
const WALK_WAVES_BEFORE_FIND: usize = 2;

/// The folder names inside `path`, by whichever route suits the folder's size (§167).
///
/// The SFTP walk answers small folders in one round trip and huge ones in eleven seconds; `find`
/// answers any folder in about one, having paid for a channel. So the walk goes first and gives way
/// once the folder proves big — see `WALK_WAVES_BEFORE_FIND`.
///
/// `find` is not trusted to exist: a server without it, or one where `-L` met a symlink loop, exits
/// non-zero, and then the walk finishes the job. That costs the abandoned waves twice over on such
/// a server, which is the price of not asking every server up front whether it has `find`.
///
/// `ponytail:` and that answer is not remembered. A server with no `find` pays the wasted waves on
/// every crowded tree click rather than once per connection. `asuser::Accounts` is where such things
/// are learned and cached (it already remembers where `sftp-server` lives and whether sudo wants a
/// password); worth adding a third if a server without `find` ever turns up in practice.
async fn dirs_inside<W: Walk + Send + Sync + 'static, E: Exec + Sync>(
	sftp: &Arc<W>,
	runner: &E,
	path: &str,
) -> Result<Vec<String>> {
	let (producer, mut landing) = spawn_walk(sftp, path);

	// Read up to the budget, and notice the wave that arrives AFTER it: that arrival is the whole
	// signal. A folder that ends within the budget closes the channel instead, so a folder whose
	// names finish exactly on the last allowed wave still counts as answered by the walk.
	let mut read: Vec<Vec<File>> = Vec::new();
	let mut overflowed = false;
	while let Some(wave) = landing.recv().await {
		if read.len() >= WALK_WAVES_BEFORE_FIND {
			overflowed = true;
			break;
		}
		read.push(wave);
	}

	if !overflowed {
		finish(producer, path).await?;
		return keep_dirs(sftp, path, read.concat()).await;
	}

	// Big enough that reading every name is the wrong way to find the folders. Dropping the
	// receiver is what stops the walk — `stream_names` sees the send fail and winds down, closing
	// the directory handle on its way out, which aborting the task would have skipped.
	drop(landing);
	match shellfs::dirs_via_find(runner, path).await {
		Ok(dirs) => Ok(dirs),
		// No `find` on this server, or `-L` met a symlink loop. The walk is slow here, but slow is
		// the whole answer and this is the only way left to get it.
		Err(error) => {
			eprintln!("find unavailable for {path}, walking it instead: {error:#}");
			let files = read_names(sftp, path).await?;
			keep_dirs(sftp, path, files).await
		}
	}
}

/// The SFTP requests a directory walk makes, and nothing else (§168).
///
/// The walk below is the one piece of cmote whose correctness is a **count**: 32 `readdir`s in
/// flight answer a crowded folder in 34 round trips, where the same 1,057 requests taken one at a
/// time cost 334 seconds (§167). Written against `RawSftpSession` that count was unobservable — a
/// session that will answer anything needs a live server — so the walk could fall back to
/// one-at-a-time and every test would still pass. §167 recorded that as the finding it is; this is
/// the seam it asked for.
///
/// What makes the count assertable is not how many requests were made — that number is identical
/// either way — but **how many were in flight at once**, which is the whole of the fix. `Steps`, the
/// fake below, answers each request only after yielding, so a wave's requests overlap and their
/// high-water mark is a number a test can read.
///
/// It is the same seam [`Exec`](super::asuser::Exec) draws for the shell backend, on the same rule:
/// a request that is **a value in and a value out** belongs on the trait, and an operation handing
/// back a live `russh` stream does not. The transfer loops' `open`/`read`/`write` therefore stay on
/// the concrete session, for exactly the reason `Exec` leaves `stream` off (§46, §113).
///
/// **Foreign types on purpose.** A trait speaking a vocabulary of its own would have to translate
/// `StatusCode::Eof` into it — and then the rule that EOF *ends* a listing while any other error
/// *fails* it would live in the adapter, the one part of this a fake cannot exercise. Speaking
/// russh-sftp's own types keeps that rule in the walk, where the tests are. It costs nothing: every
/// reply shape is constructible (`File::new`, `FileAttributes::set_type`, `Error::Status`).
trait Walk {
	/// Open a directory for reading, giving back the handle every `readdir` then quotes.
	fn opendir(&self, path: String) -> impl Future<Output = Result<Handle, SftpError>> + Send;

	/// The next block of names. The cursor is the SERVER's, which is what lets a wave of these
	/// be in flight at once: each reply is simply "the next block", whoever asked.
	fn readdir(&self, handle: String) -> impl Future<Output = Result<Name, SftpError>> + Send;

	/// Follow a path — a symlink included — and say what is at the end of it.
	fn stat(&self, path: String) -> impl Future<Output = Result<Attrs, SftpError>> + Send;

	/// The same question WITHOUT following: a symlink answers as itself. The one a delete has to
	/// ask, since following a link would take it to whatever the link points at (§18).
	fn lstat(&self, path: String) -> impl Future<Output = Result<Attrs, SftpError>> + Send;

	/// Unlink one name — a file, or a symlink whatever it points at. Never a directory.
	fn remove(&self, path: String) -> impl Future<Output = Result<Status, SftpError>> + Send;

	/// Remove one directory, which a server refuses while any name is still inside it.
	fn rmdir(&self, path: String) -> impl Future<Output = Result<Status, SftpError>> + Send;

	/// Give a handle back. A server has a finite number of them.
	fn close(&self, handle: String) -> impl Future<Output = Result<Status, SftpError>> + Send;
}

/// The real remote. Each method is the session's own, named explicitly rather than through `self`
/// so it calls the inherent method instead of recursing into this one.
impl Walk for RawSftpSession {
	async fn opendir(&self, path: String) -> Result<Handle, SftpError> {
		RawSftpSession::opendir(self, path).await
	}

	async fn readdir(&self, handle: String) -> Result<Name, SftpError> {
		RawSftpSession::readdir(self, handle).await
	}

	async fn stat(&self, path: String) -> Result<Attrs, SftpError> {
		RawSftpSession::stat(self, path).await
	}

	async fn lstat(&self, path: String) -> Result<Attrs, SftpError> {
		RawSftpSession::lstat(self, path).await
	}

	async fn remove(&self, path: String) -> Result<Status, SftpError> {
		RawSftpSession::remove(self, path).await
	}

	async fn rmdir(&self, path: String) -> Result<Status, SftpError> {
		RawSftpSession::rmdir(self, path).await
	}

	async fn close(&self, handle: String) -> Result<Status, SftpError> {
		RawSftpSession::close(self, handle).await
	}
}

/// How many `readdir` requests ride the wire at once (§167).
///
/// One `readdir` reply carries at most 100 names on OpenSSH (`MAX_READDIR_NAMES`), whatever the
/// packet size allows — so a directory of 105,610 forces 1,057 of them, and that number is the
/// server's to choose, not ours. Awaited one at a time it is 1,057 round trips *in series*:
/// measured at 316 ms each over a link with latency, which is 334 SECONDS to open one folder, with
/// the link otherwise idle the whole time. Sent as a wave they cost one round trip per WINDOW.
///
/// 32 is chosen against the round trip, not the directory: it takes the same folder to about 34
/// waves, and the remaining cost is the data itself rather than the waiting. Raising it further
/// buys progressively less and asks the server for more outstanding requests than a conservative
/// one may want to hold.
///
/// `ponytail:` a wave is a barrier — the whole 32 land before the next 32 are sent, so one slow
/// reply idles the rest of the window. A sliding window that keeps 32 in flight at all times would
/// recover that, at the price of tracking completions individually. Worth it only if the measured
/// time stops matching `trips / WINDOW × RTT`.
const READDIR_WINDOW: usize = 32;

/// Every name the server lists inside `path`, with its attributes and its `longname` (§20) —
/// `stream_names` collected into one answer, for the caller that wants the whole directory before
/// it does anything with it. That is the TREE: it has to know which children are folders, and a
/// half-read listing would show a branch as childless.
///
/// This is what `SftpSession::read_dir` does, minus the two things it discards: the
/// `longname` line the owner and group names live in, and `.`/`..`, which the model
/// drops at ingest anyway (`explorer::is_dot_link`, §19).
async fn read_names<W: Walk + Send + Sync + 'static>(
	sftp: &Arc<W>,
	path: &str,
) -> Result<Vec<File>> {
	let (producer, mut landing) = spawn_walk(sftp, path);
	let mut files = Vec::new();
	while let Some(wave) = landing.recv().await {
		files.extend(wave);
	}
	finish(producer, path).await?;
	Ok(files)
}

/// Start a walk in a task of its own, giving back the waves it lands and the handle to `finish` on
/// (§168). All three callers of `stream_names` want this same pair, and each grew its own copy of it
/// one commit at a time.
///
/// Capacity 1, with the walk spawned: the waves are consumed as fast as they land, and a bound
/// rather than an unbounded channel is what stops a huge directory being held twice — once by the
/// channel and once by whoever is draining it.
fn spawn_walk<W: Walk + Send + Sync + 'static>(
	sftp: &Arc<W>,
	path: &str,
) -> (
	tokio::task::JoinHandle<Result<()>>,
	mpsc::Receiver<Vec<File>>,
) {
	let (waves, landing) = mpsc::channel(1);
	let producer = tokio::spawn({
		let sftp = Arc::clone(sftp);
		let path = path.to_owned();
		async move { stream_names(&sftp, &path, &waves).await }
	});
	(producer, landing)
}

/// What a spawned walk has to say once its waves have run out: nothing, its own failure, or the
/// task's (§168).
///
/// The third case is not the same as the second and is worth keeping distinct: a `JoinError` means
/// the walk PANICKED or was cancelled, so no listing exists at all, where a walk error is a server
/// that answered with a refusal. Both fail the listing, and naming the folder is what makes either
/// message actionable — the rule `fail_dir` states.
async fn finish(producer: tokio::task::JoinHandle<Result<()>>, path: &str) -> Result<()> {
	match producer.await {
		Ok(Ok(())) => Ok(()),
		Ok(Err(failure)) => Err(failure),
		Err(join) => {
			Err(anyhow::Error::new(join)).with_context(|| format!("Could not list {path}"))
		}
	}
}

/// The same walk, handing each wave over as it lands instead of at the end (§167).
///
/// This is what lets the files pane fill progressively: a wave is a batch the pane can draw, and
/// the first one arrives after a single round trip rather than after all 34 of them. `read_names`
/// is the collecting wrapper for the caller that wants the whole answer in one piece.
///
/// The waves are NOT in display order — the server returns names in whatever order it keeps them,
/// which on ext4 is hash order — so whoever consumes these has to put them in order. The pane's
/// model does it once, when the listing is complete (`Files::chunk`).
async fn stream_names<W: Walk + Send + Sync + 'static>(
	sftp: &Arc<W>,
	path: &str,
	waves: &mpsc::Sender<Vec<File>>,
) -> Result<()> {
	let handle = sftp
		.opendir(path.to_owned())
		.await
		.with_context(|| format!("Could not list {path}"))?
		.handle;

	let mut done = false;
	let mut failure = None;
	while !done && failure.is_none() {
		let mut wave = tokio::task::JoinSet::new();
		for _ in 0..READDIR_WINDOW {
			let sftp = Arc::clone(sftp);
			let handle = handle.clone();
			wave.spawn(async move { sftp.readdir(handle).await });
		}
		// The whole wave is drained even once one reply has said EOF: the others were sent before
		// anyone knew that, and the ones that came back with names came back with real ones.
		let mut landed = Vec::new();
		while let Some(joined) = wave.join_next().await {
			match joined {
				Ok(Ok(name)) => landed.extend(name.files),
				// The end of the directory, not a failure: the server says EOF once it has
				// handed over every name.
				Ok(Err(SftpError::Status(status))) if status.status_code == StatusCode::Eof => {
					done = true;
				}
				// A real error must FAIL the listing rather than end it — a directory shown short
				// of what it holds, with no sign that anything is missing, is the one outcome
				// worse than an error message. The remaining replies are still drained first so
				// the wave's tasks are not left aborting mid-flight.
				Ok(Err(error)) => {
					failure = Some(anyhow::Error::new(error));
				}
				Err(join) => failure = Some(anyhow::Error::new(join)),
			}
		}
		// A conformant server answers EOF when a directory runs out (draft-ietf-secsh-filexfer-02
		// §6.7 makes it a MUST), so a whole wave of empty replies means it never will. Stopping
		// here keeps that server's listing short; the single `readdir` loop this replaced spun on
		// it forever, which is the one behaviour worth not preserving.
		if landed.is_empty() {
			done = true;
		} else if waves.send(landed).await.is_err() {
			// Nobody is listening any more — the pane has left this directory. Stop walking it
			// rather than reading a hundred thousand names for a receiver that is gone.
			break;
		}
	}
	// Give the handle back before leaving, on the way out of a failure as much as a success; a
	// server has a finite number of them.
	let _ = sftp.close(handle).await;
	match failure {
		Some(failure) => Err(failure).with_context(|| format!("Could not list {path}")),
		None => Ok(()),
	}
}

/// Which of `files` are folders — the names the tree wants, out of a walk already read.
///
/// A symlink's own type says nothing about what it points at, so each one is stat'ed (which follows
/// it) and kept only if the target is a directory — a round trip per symlink, and only per symlink.
/// `shellfs::dirs_via_find` answers the same question in one round trip with `-L`, which is why
/// `dirs_inside` prefers it once a folder turns out to be big.
///
/// Those stats go out in waves of `READDIR_WINDOW` too (§167), for the same reason the `readdir`s
/// do: awaited one at a time, a folder holding fifty symlinks cost fifty round trips *in series*,
/// which on a link measured at 316 ms is sixteen seconds to answer "which of these are folders".
/// A real directory still costs nothing extra — only symlinks are asked about at all — and the
/// order the answers come back in does not matter, since `Explorer::listed` sorts the children.
async fn keep_dirs<W: Walk + Send + Sync + 'static>(
	sftp: &Arc<W>,
	path: &str,
	files: Vec<File>,
) -> Result<Vec<String>> {
	let mut dirs = Vec::new();
	let mut links = Vec::new();
	for file in files {
		if file.attrs.is_dir() {
			dirs.push(file.filename);
		} else if file.attrs.is_symlink() {
			links.push(file.filename);
		}
	}

	// Bounded by the same window rather than "all of them at once": a directory of ten thousand
	// symlinks would otherwise put ten thousand requests on the wire in one breath.
	for wave_names in links.chunks(READDIR_WINDOW) {
		let mut wave = tokio::task::JoinSet::new();
		for name in wave_names {
			let sftp = Arc::clone(sftp);
			let target = join(path, name);
			let name = name.clone();
			// A broken link errors here and is simply left out, which is what it is.
			wave.spawn(async move {
				let is_dir = sftp
					.stat(target)
					.await
					.is_ok_and(|attrs| attrs.attrs.is_dir());
				(name, is_dir)
			});
		}
		while let Some(joined) = wave.join_next().await {
			if let Ok((name, true)) = joined {
				dirs.push(name);
			}
		}
	}
	Ok(dirs)
}

/// The SFTP listing for the files pane: every entry, sent on as each wave lands (§167).
///
/// The pane used to get nothing until the whole directory had arrived, been sorted and been cut
/// into batches. On a folder of 106,382 entries that was 11.6 seconds of blank pane — the walk is
/// bandwidth-bound at that size, so the wait cannot be made shorter, but it can be made VISIBLE:
/// a wave is already a batch the pane can draw, and the first lands after one round trip.
///
/// The listing therefore arrives out of display order, and the model sorts once at `done`
/// (`Files::chunk`) rather than the backend sorting before it sends. Rows appear in the server's
/// order and settle into display order when the listing completes.
///
/// `ponytail:` that settling is one visible reshuffle at the end. The alternative is a model that
/// merges each wave into sorted position, so the list is ordered at every instant and rows appear
/// mid-list as it grows — more code in `Files`, and a sort per wave instead of one. Worth building
/// if the single reshuffle proves more annoying than rows moving continuously would be.
async fn all_sftp(
	sftp: Arc<RawSftpSession>,
	path: String,
	request: u64,
	events: mpsc::Sender<SshEvent>,
) {
	let (producer, mut landing) = spawn_walk(&sftp, &path);

	while let Some(wave) = landing.recv().await {
		// Cut to `files::BATCH` on the way out: a wave is up to `READDIR_WINDOW` × 100 names, and
		// the batch size is what bounds ONE message rather than what bounds the fetch.
		let entries: Vec<Entry> = wave.into_iter().map(entry_of).collect();
		for batch in entries.chunks(files::BATCH) {
			// Never `done` here — the walk says when it is finished, not the last full wave.
			let delivered = events
				.send(SshEvent::FilesChunk {
					request,
					entries: batch.to_vec(),
					done: false,
				})
				.await
				.is_ok();
			if !delivered {
				return;
			}
		}
	}

	match finish(producer, &path).await {
		// One empty batch closes the listing. It is what tells an EMPTY directory to stop waiting
		// too, so the same message ends both cases and neither needs a rule of its own.
		Ok(()) => {
			let _ = events
				.send(SshEvent::FilesChunk {
					request,
					entries: Vec::new(),
					done: true,
				})
				.await;
		}
		// A failure after some waves have already gone leaves those rows on screen with the reason
		// on the notice line — which is the honest report. What must not happen is a short listing
		// presented as a whole one, and `FilesFailed` is what stops that.
		Err(error) => fail_files(&events, request, format!("{error}")).await,
	}
}

/// Turn one listed name into a pane entry (§19, §20). The size, time and ids ride along with
/// the name — SFTP sends a directory's attributes with its listing, so none of this costs
/// an extra round trip.
///
/// A symlink keeps its own kind rather than being followed: resolving each one costs a round trip,
/// and a crowded directory is exactly where that adds up — the pane asks for the one link the user
/// selects instead (`read_link`). That is the opposite of what the TREE does a few functions up,
/// and deliberately so: the tree has to know whether a link is a branch it can open.
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
async fn remove_tree<W: Walk + Send + Sync + 'static>(sftp: &Arc<W>, root: &str) -> Result<()> {
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
async fn remove_subtree<W: Walk + Send + Sync + 'static>(sftp: &Arc<W>, root: &str) -> Result<()> {
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
		Browse::Sftp { sftp, .. } => {
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
		Browse::Sftp { sftp, .. } => {
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

/// A remote that answers a walk out of a script instead of over a socket (§168), for the walk tests
/// below. The shell half of the same tests uses `shellfs::Script`, which does this for `Exec`.
///
/// It records every request, and — the part that matters — how many were **in flight at once**. A
/// walk that awaited each `readdir` before sending the next makes exactly as many requests as one
/// that sends them in waves, so the count alone cannot tell the 334-second version from the
/// 8.7-second one (§167). The high-water mark can.
#[cfg(test)]
#[derive(Default)]
struct Steps {
	/// Every request made, in order, as `"opendir /p"`, `"readdir h"`, `"stat /p/l"`, `"close h"`.
	///
	/// A `Mutex` rather than a `RefCell` because `Walk`'s futures are `Send` and a `&RefCell` is
	/// not — the same bound making the same choice it makes for `Script`.
	made: std::sync::Mutex<Vec<String>>,
	/// In flight now, and the most that ever were at once.
	live: std::sync::Mutex<(usize, usize)>,
	/// What each `readdir` answers, in order. An **exhausted queue answers EOF**, which is what a
	/// real server does when a directory runs out — so a test only queues the interesting replies.
	replies: std::sync::Mutex<std::collections::VecDeque<Result<Vec<File>, SftpError>>>,
	/// The paths `stat` answers as a directory, and the ones it answers as a plain file. Any other
	/// path is "no such file" — a dangling symlink.
	dirs: Vec<String>,
	plain: Vec<String>,
	/// The paths that are symlinks. Only `lstat` sees these; `stat` follows them, so a path listed
	/// here AND in `dirs` is a link to a folder — the case a delete must not walk into (§18).
	links: Vec<String>,
	/// What each directory holds, for a walk that visits more than one of them — which `replies`,
	/// one script for whoever asks, cannot say. Keyed by path, so `opendir` hands back the path
	/// itself as the handle and a `readdir` says which directory it is reading.
	tree: std::collections::HashMap<String, Vec<File>>,
	/// The directories of `tree` that have already answered. A directory gives its names once and
	/// EOF after, the way a server's own cursor does.
	read: std::sync::Mutex<std::collections::HashSet<String>>,
	/// What is still on the remote, so `rmdir` can refuse a directory with names still inside it.
	/// That refusal is the whole reason a removal ORDER is something a test can fail on: without
	/// it, taking a parent before its children looks exactly like taking them in the right order.
	present: std::sync::Mutex<std::collections::HashSet<String>>,
	/// Woken when the handle is given back, so a test can wait for a walk it does not own to wind
	/// down without guessing at a delay. `Notify` keeps the wake-up if it arrives first, which is
	/// what makes the wait raceless either way round.
	closed: tokio::sync::Notify,
}

#[cfg(test)]
impl Steps {
	/// A remote whose directory answers `replies` and then ends.
	fn answering(replies: Vec<Result<Vec<File>, SftpError>>) -> Self {
		Self {
			replies: std::sync::Mutex::new(replies.into()),
			..Self::default()
		}
	}

	/// A remote holding `blocks` blocks of one name each — enough to make a walk take waves.
	fn holding(blocks: usize) -> Self {
		Self::answering(
			(0..blocks)
				.map(|which| Ok(vec![plain_file(&format!("f{which}"))]))
				.collect(),
		)
	}

	/// A remote holding a real tree: each directory by path, and the names inside it. `present` is
	/// seeded from it, so what the walk removes stops being there.
	fn tree(dirs: &[(&str, &[File])]) -> Self {
		let tree: std::collections::HashMap<String, Vec<File>> = dirs
			.iter()
			.map(|(path, names)| ((*path).to_owned(), names.to_vec()))
			.collect();
		let present = tree
			.iter()
			.flat_map(|(path, names)| {
				std::iter::once(path.clone())
					.chain(names.iter().map(|name| join(path, &name.filename)))
			})
			.collect();
		Self {
			tree,
			present: std::sync::Mutex::new(present),
			..Self::default()
		}
	}

	/// Note a request as begun, and raise the high-water mark if this one is the highest yet.
	///
	/// Its own function so the guards are temporaries that drop at the end of each statement, never
	/// held across an `.await` — a live `MutexGuard` is what would cost `Walk`'s futures their
	/// `Send`, and the compiler would say so.
	fn enter(&self, request: String) {
		self.made
			.lock()
			.expect("no test panics while holding this lock")
			.push(request);
		let mut live = self
			.live
			.lock()
			.expect("no test panics while holding this lock");
		live.0 += 1;
		live.1 = live.1.max(live.0);
	}

	/// Note a request as answered.
	fn leave(&self) {
		self.live
			.lock()
			.expect("no test panics while holding this lock")
			.0 -= 1;
	}

	/// The most requests that were ever in flight at once.
	fn peak(&self) -> usize {
		self.live
			.lock()
			.expect("no test panics while holding this lock")
			.1
	}

	/// How many requests of one kind were made — `"readdir"`, `"opendir"`, `"stat"`, `"close"`.
	fn counted(&self, kind: &str) -> usize {
		self.made
			.lock()
			.expect("no test panics while holding this lock")
			.iter()
			.filter(|request| request.starts_with(kind))
			.count()
	}

	/// What one path is, by the lists — a directory, a plain file, or nothing there at all. The
	/// answer `stat` gives, and the answer `lstat` gives for everything that is not a link.
	fn found(&self, path: &str) -> Result<Attrs, SftpError> {
		let mut attrs = FileAttributes::default();
		if self.dirs.iter().any(|dir| dir == path) {
			attrs.set_dir(true);
		} else if self.plain.iter().any(|file| file == path) {
			attrs.set_regular(true);
		} else {
			return Err(SftpError::Status(Status {
				id: 0,
				status_code: StatusCode::NoSuchFile,
				error_message: "no such file".to_owned(),
				language_tag: String::new(),
			}));
		}
		Ok(Attrs { id: 0, attrs })
	}

	/// What one `readdir` answers. A directory named in `tree` gives its own names and then EOF;
	/// with no tree at all the flat script answers, whoever asked, which is what a test about one
	/// directory wants. `None` is EOF either way.
	fn listing(&self, dir: &str) -> Option<Result<Vec<File>, SftpError>> {
		if self.tree.is_empty() {
			return self
				.replies
				.lock()
				.expect("no test panics while holding this lock")
				.pop_front();
		}
		let first = self
			.read
			.lock()
			.expect("no test panics while holding this lock")
			.insert(dir.to_owned());
		if first {
			Some(Ok(self.tree.get(dir).cloned().unwrap_or_default()))
		} else {
			None
		}
	}

	/// Take one name away, whatever it was.
	fn unlink(&self, path: &str) {
		self.present
			.lock()
			.expect("no test panics while holding this lock")
			.remove(path);
	}

	/// Take one directory away — or refuse it the way a server does while anything is still inside.
	fn empty_then_gone(&self, path: &str) -> Result<Status, SftpError> {
		let inside = format!("{path}/");
		let mut present = self
			.present
			.lock()
			.expect("no test panics while holding this lock");
		if present.iter().any(|name| name.starts_with(&inside)) {
			return Err(SftpError::Status(Status {
				id: 0,
				status_code: StatusCode::Failure,
				error_message: format!("{path} is not empty"),
				language_tag: String::new(),
			}));
		}
		present.remove(path);
		Ok(ok())
	}

	/// The arguments of every request of one kind, in the order they were made.
	fn arguments(&self, kind: &str) -> Vec<String> {
		self.made
			.lock()
			.expect("no test panics while holding this lock")
			.iter()
			.filter_map(|request| request.strip_prefix(kind)?.trim().to_owned().into())
			.collect()
	}
}

#[cfg(test)]
impl Walk for Steps {
	/// Not an `async fn`, though the trait allows it and `RawSftpSession`'s impl is one: there is
	/// nothing here to await, and saying so with `future::ready` is the same choice `Script` makes
	/// for the same reason (§113). `readdir` and `stat` DO await — that is their whole point.
	fn opendir(&self, path: String) -> impl Future<Output = Result<Handle, SftpError>> + Send {
		self.enter(format!("opendir {path}"));
		self.leave();
		// The handle is the path. A real server's is opaque and this one's says which directory it
		// belongs to, which is what lets a walk over a tree be answered directory by directory.
		std::future::ready(Ok(Handle {
			id: 0,
			handle: path,
		}))
	}

	async fn readdir(&self, handle: String) -> Result<Name, SftpError> {
		self.enter(format!("readdir {handle}"));
		// The yield is the whole mechanism. Every request in a wave reaches this point before any
		// of them answers, so `live` climbs to the width of the wave; awaited one at a time it
		// never passes 1. `#[tokio::test]` runs on the current thread, which is what makes the
		// number exact rather than a race — on a multi-thread runtime a reply could land before
		// the last request had left.
		tokio::task::yield_now().await;
		let reply = self.listing(&handle);
		self.leave();
		match reply {
			Some(Ok(files)) => Ok(Name { id: 0, files }),
			Some(Err(error)) => Err(error),
			// A directory that has run out says so. This is the reply the walk must read as an
			// ending rather than a failure.
			None => Err(eof()),
		}
	}

	async fn stat(&self, path: String) -> Result<Attrs, SftpError> {
		self.enter(format!("stat {path}"));
		tokio::task::yield_now().await;
		self.leave();
		self.found(&path)
	}

	/// The link-aware half of the pair: `links` is consulted FIRST, so a symlink answers as itself
	/// and the walk never learns what is on the other side of it.
	fn lstat(&self, path: String) -> impl Future<Output = Result<Attrs, SftpError>> + Send {
		self.enter(format!("lstat {path}"));
		self.leave();
		let answer = if self.links.iter().any(|link| link == &path) {
			let mut attrs = FileAttributes::default();
			attrs.set_symlink(true);
			Ok(Attrs { id: 0, attrs })
		} else {
			self.found(&path)
		};
		std::future::ready(answer)
	}

	fn remove(&self, path: String) -> impl Future<Output = Result<Status, SftpError>> + Send {
		self.enter(format!("remove {path}"));
		self.leave();
		self.unlink(&path);
		std::future::ready(Ok(ok()))
	}

	fn rmdir(&self, path: String) -> impl Future<Output = Result<Status, SftpError>> + Send {
		self.enter(format!("rmdir {path}"));
		self.leave();
		std::future::ready(self.empty_then_gone(&path))
	}

	fn close(&self, handle: String) -> impl Future<Output = Result<Status, SftpError>> + Send {
		self.enter(format!("close {handle}"));
		self.leave();
		self.closed.notify_one();
		std::future::ready(Ok(ok()))
	}
}

/// The reply a server sends when a request simply worked.
#[cfg(test)]
fn ok() -> Status {
	Status {
		id: 0,
		status_code: StatusCode::Ok,
		error_message: String::new(),
		language_tag: String::new(),
	}
}

/// The reply a server sends when a directory has no more names.
#[cfg(test)]
fn eof() -> SftpError {
	SftpError::Status(Status {
		id: 0,
		status_code: StatusCode::Eof,
		error_message: String::new(),
		language_tag: String::new(),
	})
}

/// One listed name of each kind the walk has to tell apart.
#[cfg(test)]
fn named(name: &str, set: impl Fn(&mut FileAttributes)) -> File {
	let mut attrs = FileAttributes::default();
	set(&mut attrs);
	File {
		filename: name.to_owned(),
		longname: String::new(),
		attrs,
	}
}

#[cfg(test)]
fn dir_file(name: &str) -> File {
	named(name, |attrs| attrs.set_dir(true))
}

#[cfg(test)]
fn plain_file(name: &str) -> File {
	named(name, |attrs| attrs.set_regular(true))
}

#[cfg(test)]
fn link_file(name: &str) -> File {
	named(name, |attrs| attrs.set_symlink(true))
}

/// The walk, driven through the seam §167 said it did not have (§168).
#[cfg(test)]
mod walk_tests {
	use super::*;
	use crate::ssh::shellfs::Script;

	/// The one that would have caught a regression from waves back to one-at-a-time — and the
	/// reason this whole seam exists. Note what is NOT asserted: the request count, which is
	/// identical either way. 1,057 requests sent 32 at a time and 1,057 sent in turn differ only in
	/// how many were outstanding, and that difference was 334 seconds against 8.7 (§167).
	#[tokio::test]
	async fn a_wave_of_readdirs_is_in_flight_at_once_and_not_a_queue_of_one() {
		let steps = Arc::new(Steps::holding(3));

		let names = read_names(&steps, "/p").await.expect("the walk finished");

		assert_eq!(names.len(), 3, "every queued name arrived");
		assert_eq!(
			steps.peak(),
			READDIR_WINDOW,
			"a whole wave was outstanding at once, not one request at a time"
		);
		assert_eq!(steps.counted("opendir"), 1, "one handle for the listing");
		assert_eq!(steps.counted("close"), 1, "given back at the end");
	}

	/// A directory shown short of what it holds, with nothing saying so, is the one outcome worse
	/// than an error message — so anything that is not EOF fails the whole listing.
	#[tokio::test]
	async fn a_real_error_fails_the_listing_rather_than_shortening_it() {
		let steps = Arc::new(Steps::answering(vec![
			Ok(vec![plain_file("a")]),
			Err(SftpError::Status(Status {
				id: 0,
				status_code: StatusCode::PermissionDenied,
				error_message: "denied".to_owned(),
				language_tag: String::new(),
			})),
		]));

		let failed = read_names(&steps, "/p")
			.await
			.expect_err("a refused listing");

		assert!(
			format!("{failed:#}").contains("/p"),
			"the failure names the folder the user asked for: {failed:#}"
		);
		assert_eq!(
			steps.counted("close"),
			1,
			"the handle goes back on the way out of a failure too"
		);
	}

	/// The other 31 requests of a wave were sent before anyone knew the directory had ended, and
	/// the ones that came back with names came back with real ones.
	#[tokio::test]
	async fn the_rest_of_a_wave_survives_one_reply_saying_eof() {
		let steps = Arc::new(Steps::answering(vec![
			Err(eof()),
			Ok(vec![plain_file("a")]),
			Ok(vec![plain_file("b")]),
		]));

		let names = read_names(&steps, "/p").await.expect("the walk finished");

		let mut found: Vec<String> = names.into_iter().map(|file| file.filename).collect();
		found.sort();
		assert_eq!(
			found,
			vec!["a".to_owned(), "b".to_owned()],
			"an early EOF ended the walk without discarding the wave it arrived in"
		);
	}

	/// EOF is a MUST in draft-ietf-secsh-filexfer-02 §6.7, so a whole wave of empty replies means
	/// this server will never send one. The single-`readdir` loop this walk replaced spun on that
	/// forever; stopping is the one prior behaviour deliberately not preserved (§167).
	#[tokio::test]
	async fn a_wave_that_lands_no_names_at_all_stops_the_walk() {
		let empty = (0..READDIR_WINDOW).map(|_| Ok(Vec::new())).collect();
		let steps = Arc::new(Steps::answering(empty));

		let names = read_names(&steps, "/p").await.expect("the walk finished");

		assert!(names.is_empty(), "there were no names to find");
		assert_eq!(
			steps.counted("readdir"),
			READDIR_WINDOW,
			"one wave and no more — a second would be a server being asked to prove itself twice"
		);
	}

	/// A symlink's own type says nothing about what it points at, so the tree resolves each one —
	/// and only each one. The round trips are per symlink, never per entry.
	#[tokio::test]
	async fn only_the_symlinks_are_resolved_and_a_broken_one_is_left_out() {
		let steps = Arc::new(Steps {
			dirs: vec!["/p/to_dir".to_owned()],
			plain: vec!["/p/to_file".to_owned()],
			..Steps::default()
		});

		let mut kept = keep_dirs(
			&steps,
			"/p",
			vec![
				dir_file("real"),
				plain_file("file"),
				link_file("to_dir"),
				link_file("to_file"),
				link_file("dangling"),
			],
		)
		.await
		.expect("the folders were sorted out");
		kept.sort();

		assert_eq!(
			kept,
			vec!["real".to_owned(), "to_dir".to_owned()],
			"a link to a folder is a branch; a link to a file and a broken link are not"
		);
		let mut asked = steps.arguments("stat");
		asked.sort();
		assert_eq!(
			asked,
			vec![
				"/p/dangling".to_owned(),
				"/p/to_dir".to_owned(),
				"/p/to_file".to_owned()
			],
			"the folder and the plain file cost no round trip at all"
		);
		assert_eq!(steps.peak(), 3, "and the three that did went out together");
	}

	/// The adaptive route, from the side that should not reach the shell. Nearly every folder is
	/// this one, which is why the walk goes first: `find` needs a channel of its own, and that is
	/// two or three round trips against the one this took (§167).
	#[tokio::test]
	async fn a_folder_small_enough_to_walk_never_asks_the_shell() {
		let steps = Arc::new(Steps::answering(vec![Ok(vec![dir_file("child")])]));
		let script = Script::refusing();

		let dirs = dirs_inside(&steps, &script, "/p")
			.await
			.expect("the walk answered");

		assert_eq!(dirs, vec!["child".to_owned()], "answered by the walk");
		assert!(
			script.commands().is_empty(),
			"and the shell was never reached: {:?}",
			script.commands()
		);
	}

	/// The other side of it. A folder still going after `WALK_WAVES_BEFORE_FIND` waves is one the
	/// walk is the wrong tool for, so the question is asked the other way — which is the whole
	/// point of watching rather than guessing.
	#[tokio::test]
	async fn a_folder_too_big_to_walk_is_asked_of_the_shell_instead() {
		// Three waves' worth: one wave consumes up to `READDIR_WINDOW` replies, so this is more
		// than the budget of two and a wave arrives after the last allowed one, which is the
		// signal. Spelled as a fixed number rather than off `WALK_WAVES_BEFORE_FIND` on purpose —
		// scaled to the constant this test could never fail on it, and the budget is a decision
		// (§167), so raising it past three should make a test speak up.
		let steps = Arc::new(Steps::holding(READDIR_WINDOW * 3));
		let script = Script::saying("/p/x\0/p/y\0");

		let dirs = dirs_inside(&steps, &script, "/p")
			.await
			.expect("find answered");

		assert_eq!(
			dirs,
			vec!["x".to_owned(), "y".to_owned()],
			"the names find printed, not the ones the walk had read"
		);
		assert!(
			script.only_command().starts_with("find -L '/p'"),
			"asked with the path quoted: {}",
			script.only_command()
		);
	}

	/// `find` is not trusted to exist: a server without it exits non-zero, and then the walk has to
	/// finish the job however slow that is. A second `opendir` is what proves it did.
	#[tokio::test]
	async fn a_server_without_find_falls_back_to_finishing_the_walk() {
		let steps = Arc::new(Steps::holding(READDIR_WINDOW * 3));
		let script = Script::refusing();

		dirs_inside(&steps, &script, "/p")
			.await
			.expect("the walk finished what find would not");

		assert_eq!(
			steps.counted("opendir"),
			2,
			"the abandoned walk, and then the one that finished the folder"
		);
	}

	/// Two rules on one line of §167, and this test was cited for both while only pinning one.
	///
	/// Dropping the receiver is what stops an abandoned walk, rather than aborting its task, and the
	/// difference is a directory handle: `stream_names` sees the send fail, breaks, and closes the
	/// handle on its way out where an `abort()` would have skipped it. That much an earlier version
	/// of this test did catch. What it did NOT catch is the stopping — the folder held exactly the
	/// replies the three read waves consumed, so the walk ran out of names and left through the
	/// EOF path. Deleting the `break` changed nothing it asserted.
	///
	/// So the folder holds far more than the switch reads. Now the walk has names left when nobody
	/// is listening, and whether it stops is a number: four waves against every wave in the folder.
	#[tokio::test]
	async fn an_abandoned_walk_stops_walking_and_gives_the_handle_back() {
		let plenty = READDIR_WINDOW * 20;
		let steps = Arc::new(Steps::holding(plenty));

		let _ = dirs_inside(&steps, &Script::saying(""), "/p").await;

		// Waiting on the walk's own last act rather than on a delay: the producer is not joined in
		// the overflow path, it winds down alone, and `Notify` holds the wake-up if it got there
		// first — so there is no window to race with and no interval to guess at.
		steps.closed.notified().await;
		assert_eq!(
			steps.counted("close"),
			1,
			"the handle was given back by the walk that was left behind"
		);
		assert!(
			steps.counted("readdir") <= READDIR_WINDOW * 5,
			"it stopped when the receiver went, rather than reading the whole folder for nobody: \
			 {} readdirs",
			steps.counted("readdir")
		);
	}

	/// The one delete rule whose regression reaches OUTSIDE what the user selected. A folder they
	/// picked is emptied and removed; a symlink they picked is one name, and unlinking it leaves
	/// whatever it points at alone. Following it would delete a tree nobody chose — so the delete
	/// asks `lstat` and not `stat`, and that difference is what this pins (§18).
	#[tokio::test]
	async fn a_link_to_a_folder_is_unlinked_and_never_descended_into() {
		let steps = Arc::new(Steps {
			// What the link is, and — the trap — what following it WOULD find: a real folder,
			// holding a file that has to still be there afterwards.
			links: vec!["/p/link".to_owned()],
			dirs: vec!["/p/link".to_owned()],
			..Steps::answering(vec![Ok(vec![plain_file("precious")])])
		});

		remove_tree(&steps, "/p/link").await.expect("the link went");

		assert_eq!(
			steps.arguments("remove"),
			vec!["/p/link".to_owned()],
			"the link itself was unlinked, and nothing on the far side of it"
		);
		assert_eq!(steps.counted("rmdir"), 0, "a link is not a folder to empty");
		assert_eq!(steps.counted("opendir"), 0, "and it was never read either");
	}

	/// A directory only goes once nothing is inside it, which is why the removal runs DEEPEST FIRST
	/// — `dirs` is discovery order, parents before children, and it is walked in reverse.
	///
	/// What makes this a test rather than an observation: `Steps` refuses a `rmdir` with names still
	/// under it, the way a server does. So a wrong order does not merely look odd here, it fails.
	#[tokio::test]
	async fn a_tree_goes_deepest_first_so_no_rmdir_finds_names_still_inside() {
		let steps = Arc::new(Steps::tree(&[
			("/p", &[dir_file("mid"), plain_file("top.txt")]),
			("/p/mid", &[dir_file("deep")]),
			("/p/mid/deep", &[plain_file("deep.txt")]),
		]));

		remove_subtree(&steps, "/p").await.expect("the tree went");

		assert_eq!(
			steps.arguments("rmdir"),
			vec![
				"/p/mid/deep".to_owned(),
				"/p/mid".to_owned(),
				"/p".to_owned()
			],
			"children before parents, all the way up"
		);
		// The order files go in is not a rule — nothing depends on it — so this asks only that every
		// one of them went, at whatever depth it sat.
		let mut unlinked = steps.arguments("remove");
		unlinked.sort();
		assert_eq!(
			unlinked,
			vec!["/p/mid/deep/deep.txt".to_owned(), "/p/top.txt".to_owned()],
			"every file in the tree, and only the files"
		);
	}

	/// The link rule again, one level down — and by a different route, which is why it is a second
	/// test and not the same one. At the root, `remove_tree` asks `lstat`; inside the tree there is
	/// no asking at all, because the listing already said what each name is and a link's own type is
	/// a link. So it is unlinked with the files.
	///
	/// The regression to fear is a copy: `keep_dirs`, in this same module, resolves every listed
	/// link ON PURPOSE, because a link to a folder is a branch of the tree pane (§19). Reading a
	/// link as a folder is right there and right for that job — and here it would empty someone
	/// else's folder.
	#[tokio::test]
	async fn a_link_inside_the_tree_is_unlinked_with_the_files_and_never_followed() {
		let steps = Arc::new(Steps::tree(&[(
			"/p",
			&[link_file("to_elsewhere"), plain_file("mine.txt")],
		)]));

		remove_subtree(&steps, "/p").await.expect("the folder went");

		let mut unlinked = steps.arguments("remove");
		unlinked.sort();
		assert_eq!(
			unlinked,
			vec!["/p/mine.txt".to_owned(), "/p/to_elsewhere".to_owned()],
			"the link went the way the file did — as one name"
		);
		assert_eq!(
			steps.counted("opendir"),
			1,
			"the folder was read, and the link was not read at all"
		);
		assert_eq!(
			steps.arguments("rmdir"),
			vec!["/p".to_owned()],
			"one folder was here to remove, whatever the link points at"
		);
	}
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

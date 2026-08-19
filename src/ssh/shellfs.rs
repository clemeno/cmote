// ssh/shellfs.rs — the file backend made of shell commands (PLAN §46).
//
// The last resort of the chain in `asuser`: a remote with no `sftp-server` binary to run as another
// account still has a shell, and a shell can read a directory, copy bytes and make a folder. So
// every file operation has a second implementation here, built out of `ls`, `cat`, `wc`, `find`,
// `mkdir`, `mv` and `rm` — run through `asuser::Runner`, which wraps each one in the elevation.
//
// The bytes are the surprising part: they need no encoding. An exec channel carries binary data,
// and cmote deliberately asks for NO pty on these channels (a pty would translate line endings and
// interpret control bytes), so `cat file` puts the file's exact bytes on the channel and
// `cat > file` writes exactly what is sent. Stdout and stderr arrive as separate SSH messages, so a
// warning on stderr cannot corrupt the data either. No base64, no chunk framing, no dependency.
//
// What this backend cannot do as well as SFTP, said plainly and once:
//
//   * **Types and metadata are text.** `ls` output is parsed, so a name containing a newline reads
//     as two entries and a symlink to a directory looks like a file. The size a transfer plans
//     against comes from `wc -c`, which is exact, but mtimes and permission bits are NOT carried:
//     a copy made this way lands with the time and mode the remote's own umask gives it. The SFTP
//     path keeps all of it, and it is what runs unless the remote has no `sftp-server`.
//   * **`ponytail:`** the two backends therefore have two COPY LOOPS, not one generic one. Making
//     the SFTP loops generic over a filesystem trait would have meant rewriting the working
//     transfer, resume and conflict code (§16, §17, §19) with no way to test it against a real
//     server — so the risk was put here, in the path that runs on almost no server, instead of
//     there, in the path that runs on all of them.
//
// That refusal is about the copy loops and stays exactly where it was. The functions in this file
// whose whole content is "compose a command, read the reply" are on the other side of it, and they
// are generic — `&impl Exec` (`asuser::Exec`) rather than `&Runner`. The reason is testability and
// nothing else: a `Runner` that will answer anything needs a live session, so every listing, every
// metadata read and every mutation here was unreachable by any test, including the QUOTING, which
// is a security boundary. `Script` at the foot of this file answers out of a canned reply and
// records what it was asked to run, which makes both halves assertable at once — the command that
// went out and the parse of what came back.
//
// `Exec` deliberately carries no `stream`: that returns a `russh::Channel`, a foreign type nothing
// but the real runner can produce, and putting it on the trait would make the trait implementable
// only by the thing it exists to stand in for. So `read_all`, `write_all`, `fetch` and `send` — the
// four that move bytes — still take a concrete `&Runner`, which is the same line the note above
// draws.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use russh::{Channel, ChannelMsg, client};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::asuser::{Exec, Runner};
use super::transfer::{self, CopyOutcome, FileAction, PlannedFile, Start, TreePlan};
use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer::{self, join, shell_quote};
use crate::files::{self, Entry, FilesKind, Meta};

// A file read through a shell is bounded by the ceiling its CALLER passes, exactly as the SFTP path
// is (§53) — there is no constant here on purpose. There was one, `MAX_READ = edit::MAX_SIZE`, and
// it was right until a second viewer arrived with a different number: the picture preview would
// have quietly kept the editor's 8 MiB whenever the files pane was elevated (§46) and reading
// through `cat` instead of sftp, so the same photograph opened or was refused depending on which
// account the pane happened to be showing.

/// The folders inside `path`, for the explorer tree (§18).
///
/// `ls -1Ap` marks a real directory with a trailing `/`, includes dotfiles and leaves `.`/`..` out.
/// `--` stops a path that begins with a dash being read as an option.
pub async fn dirs(runner: &impl Exec, path: &str) -> Result<Vec<String>> {
	let output = runner
		.stdout(&format!("ls -1Ap -- {}", shell_quote(path)))
		.await?;
	Ok(output
		.lines()
		.filter_map(|line| line.strip_suffix('/'))
		.map(str::to_owned)
		.collect())
}

/// Every entry inside `path`, for the files pane (§19).
///
/// `ls -1AF` marks the type: `/` a directory, `@` a symlink, and `*`/`|`/`=` an executable, fifo or
/// socket — all of which are files as far as the pane is concerned. No size, time or owner comes
/// with it, so the pane shows the name and the type and leaves the rest blank (§20).
pub async fn entries(runner: &impl Exec, path: &str) -> Result<Vec<Entry>> {
	let output = runner
		.stdout(&format!("ls -1AF -- {}", shell_quote(path)))
		.await?;
	let mut entries: Vec<Entry> = output
		.lines()
		.filter(|line| !line.is_empty())
		.map(|line| match line.strip_suffix('/') {
			Some(name) => Entry {
				name: name.to_owned(),
				kind: FilesKind::Dir,
				meta: Meta::default(),
			},
			None => match line.strip_suffix('@') {
				Some(name) => Entry {
					name: name.to_owned(),
					kind: FilesKind::Link,
					meta: Meta::default(),
				},
				None => Entry {
					name: line.trim_end_matches(['*', '|', '=']).to_owned(),
					kind: FilesKind::File,
					meta: Meta::default(),
				},
			},
		})
		.collect();
	files::sort(&mut entries);
	Ok(entries)
}

/// Where a symlink points (§20), for the details popup. `readlink` prints the link's own target;
/// nothing is reported when it will not resolve, which leaves the popup without that line.
pub async fn read_link(runner: &impl Exec, path: &str) -> Option<String> {
	let target = runner
		.stdout(&format!("readlink -- {}", shell_quote(path)))
		.await
		.ok()?;
	let target = target.trim();
	(!target.is_empty()).then(|| target.to_owned())
}

/// A file's size in bytes, or `None` when it cannot be read at all.
///
/// `wc -c <` rather than `stat`: the redirect means the count is of the file's contents whatever the
/// path looks like, and `wc` is in every environment while `stat`'s flags differ between GNU, BSD
/// and BusyBox.
pub async fn size(runner: &impl Exec, path: &str) -> Option<u64> {
	let output = runner
		.stdout(&format!("wc -c < {}", shell_quote(path)))
		.await
		.ok()?;
	output.trim().parse().ok()
}

/// Whether anything at all sits at `path` — a file, a folder, or a dangling symlink.
pub async fn exists(runner: &impl Exec, path: &str) -> bool {
	runner
		.succeeds(&format!("[ -e {} ]", shell_quote(path)))
		.await
}

/// Create one folder, refusing to replace whatever is already there (§18). The test and the create
/// are one command so nothing can slip into the path between them.
pub async fn make_dir(runner: &impl Exec, path: &str) -> Result<()> {
	let quoted = shell_quote(path);
	runner
		.stdout(&format!(
			"if [ -e {quoted} ]; then echo 'already exists' >&2; exit 1; fi; mkdir -- {quoted}"
		))
		.await
		.map(|_| ())
}

/// Create a folder and every missing parent, for a tree transfer's destination. Unlike `make_dir`
/// this is happy to find it already there — merging into an existing tree is the point (§17).
pub async fn make_dirs(runner: &impl Exec, path: &str) -> Result<()> {
	runner
		.stdout(&format!("mkdir -p -- {}", shell_quote(path)))
		.await
		.map(|_| ())
		// Every directory is made before any file is copied into one, so a refusal here means the
		// transfer has not written a byte: a clean failure, not something to offer a Resume for
		// (§16). The `mkdir` would be refused the same way next time.
		.map_err(transfer::mark_refused)
}

/// Rename, refusing an occupied destination (§18) — again as one command, for the same reason.
pub async fn rename(runner: &impl Exec, from: &str, to: &str) -> Result<()> {
	let from = shell_quote(from);
	let to = shell_quote(to);
	runner
		.stdout(&format!(
			"if [ -e {to} ]; then echo 'already exists' >&2; exit 1; fi; mv -- {from} {to}"
		))
		.await
		.map(|_| ())
}

/// Delete entries, folders and their contents included (§18). `--` matters more here than anywhere:
/// a blunt instrument must only ever see paths, never options.
pub async fn remove(runner: &impl Exec, paths: &[String]) -> Result<()> {
	let quoted = paths
		.iter()
		.map(|path| shell_quote(path))
		.collect::<Vec<_>>()
		.join(" ");
	runner
		.stdout(&format!("rm -rf -- {quoted}"))
		.await
		.map(|_| ())
}

/// A free `name-1`-style path beside an occupied one (§17), for a "keep both" answer. Asks the
/// remote about each candidate in turn, exactly as the SFTP path does — and now with exactly the
/// same shape and the same ceiling, through `explorer::free_candidate` and
/// `explorer::FREE_NAME_TRIES`. Each probe here is a `[ -e ]` round trip, which is dearer than
/// SFTP's own existence check rather than cheaper, so the bound matters more on this backend than
/// on that one; it used to be a bare `1000` written twice in this file.
pub async fn free_name(runner: &impl Exec, dir: &str, name: &str) -> String {
	for attempt in 1..=explorer::FREE_NAME_TRIES {
		let candidate = join(dir, &explorer::free_candidate(name, attempt));
		if !exists(runner, &candidate).await {
			return candidate;
		}
	}
	join(
		dir,
		&explorer::free_candidate(name, explorer::FREE_NAME_TRIES),
	)
}

/// Read a whole remote file into memory, for a viewer tab (§32, §53). Refuses one over `limit`
/// before reading a byte, from the size the remote itself reports, and again as the bytes arrive
/// in case that size was a lie.
pub async fn read_all(runner: &Runner, path: &str, limit: u64) -> Result<Vec<u8>> {
	match size(runner, path).await {
		Some(bytes) if bytes > limit => bail!(
			"the file is {} — larger than the {} cmote will open",
			crate::human::bytes(bytes),
			crate::human::bytes(limit)
		),
		Some(_) => {}
		None => bail!("could not read the file"),
	}
	let mut channel = open_read(runner, path, 0).await?;
	let mut bytes: Vec<u8> = Vec::new();
	let mut stderr = String::new();
	let mut status = None;
	while let Some(message) = channel.wait().await {
		match message {
			ChannelMsg::Data { data } => {
				bytes.extend_from_slice(&data);
				if bytes.len() as u64 > limit {
					bail!("the file grew past the size cmote will open");
				}
			}
			ChannelMsg::ExtendedData { data, .. } => {
				stderr.push_str(&String::from_utf8_lossy(&data))
			}
			ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
			ChannelMsg::Eof | ChannelMsg::Close => break,
			_ => {}
		}
	}
	if status.is_some_and(|code| code != 0) {
		bail!("{}", reason_of(&stderr));
	}
	Ok(bytes)
}

/// Write a whole buffer to a remote file ATOMICALLY, for the editor's save (§32): the bytes go to a
/// temp sibling and the temp is renamed over the target, so a connection dropped mid-write cannot
/// leave the user's file truncated.
pub async fn write_all(runner: &Runner, path: &str, bytes: &[u8]) -> Result<()> {
	let temp = format!("{path}.cmote-tmp");
	write_stream(runner, &temp, false, bytes).await?;
	// `mv` over the target: same directory, so it is a rename within one filesystem — atomic.
	runner
		.stdout(&format!(
			"mv -f -- {} {}",
			shell_quote(&temp),
			shell_quote(path)
		))
		.await
		.map(|_| ())
		.context("could not put the file in place")
}

/// Start a read of `path` from `offset`, returning the live channel its bytes arrive on.
///
/// `tail -c +N` counts from ONE, so continuing after N bytes starts at `N + 1`. `cat` is used for a
/// whole file rather than `tail -c +1` because it is the plainer command and the common case.
async fn open_read(runner: &Runner, path: &str, offset: u64) -> Result<Channel<client::Msg>> {
	let quoted = shell_quote(path);
	let snippet = if offset == 0 {
		format!("cat -- {quoted}")
	} else {
		format!("tail -c +{} -- {quoted}", offset + 1)
	};
	runner.stream(&snippet).await
}

/// Send `bytes` to `path`, truncating it or appending to it. Used whole-buffer (the editor) and
/// chunk-by-chunk by the copy loops through `write_stream_from`.
async fn write_stream(runner: &Runner, path: &str, append: bool, bytes: &[u8]) -> Result<()> {
	let mut channel = open_write(runner, path, append).await?;
	channel
		.data(bytes)
		.await
		.context("could not send the file's bytes")?;
	finish_write(&mut channel).await
}

/// Start a write to `path` and return the live channel to send bytes down.
///
/// The redirection is what needs a shell: `cat` writes to its stdout, so the destination is the
/// shell's `>` (truncate) or `>>` (append) — and an append is exactly what a resume needs.
async fn open_write(runner: &Runner, path: &str, append: bool) -> Result<Channel<client::Msg>> {
	let quoted = shell_quote(path);
	let redirect = if append { ">>" } else { ">" };
	runner.stream(&format!("cat {redirect} {quoted}")).await
}

/// Close a write and find out whether the remote was happy with it: EOF, then read to the end for
/// the exit status. Without this a full disk or a read-only mount would look like a success.
async fn finish_write(channel: &mut Channel<client::Msg>) -> Result<()> {
	channel.eof().await.context("could not finish the write")?;
	let mut stderr = String::new();
	let mut status = None;
	while let Some(message) = channel.wait().await {
		match message {
			ChannelMsg::ExtendedData { data, .. } => {
				stderr.push_str(&String::from_utf8_lossy(&data))
			}
			ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
			ChannelMsg::Eof | ChannelMsg::Close => break,
			_ => {}
		}
	}
	if status.is_some_and(|code| code != 0) {
		bail!("{}", reason_of(&stderr));
	}
	Ok(())
}

/// A command's stderr as a message, or a plain statement when it wrote none.
fn reason_of(stderr: &str) -> String {
	let trimmed = stderr.trim();
	if trimmed.is_empty() {
		"the command failed".to_owned()
	} else {
		trimmed.to_owned()
	}
}

/// Copy one remote file down to `local` (§19), reporting progress and honouring a cancel.
///
/// The same contract as the SFTP path: a resume appends to the local partial from its own size, a
/// cancel deletes what it was writing (a deliberate stop is final), and a failure LEAVES the
/// partial so the transfer can be resumed.
pub async fn fetch(
	runner: &Runner,
	remote: &str,
	local: &Path,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	let total = size(runner, remote)
		.await
		.context("could not read the file's size")?;
	let have = match tokio::fs::metadata(local).await {
		Ok(meta) => Some(meta.len()),
		Err(_) => None,
	};
	let offset = match transfer::resume_start(resume, have, total) {
		Start::Skip => return Ok(CopyOutcome::Done),
		Start::At(offset) => offset,
	};

	let mut file = open_local(local, offset).await?;
	let mut channel = open_read(runner, remote, offset).await?;
	let mut ticker = transfer::Ticker::default();
	ticker.settle(offset);
	let mut stderr = String::new();
	let mut status = None;
	while let Some(message) = channel.wait().await {
		if cancel.load(Ordering::Relaxed) {
			drop(file);
			let _ = tokio::fs::remove_file(local).await;
			return Ok(CopyOutcome::Cancelled);
		}
		match message {
			ChannelMsg::Data { data } => {
				file.write_all(&data)
					.await
					.context("could not write to the local file")?;
				if let Some(sent) = ticker.advance(data.len() as u64) {
					let _ = events
						.send(SshEvent::TransferProgress { sent, total })
						.await;
				}
			}
			ChannelMsg::ExtendedData { data, .. } => {
				stderr.push_str(&String::from_utf8_lossy(&data))
			}
			ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
			ChannelMsg::Eof | ChannelMsg::Close => break,
			_ => {}
		}
	}
	file.flush()
		.await
		.context("could not finish the local file")?;
	if status.is_some_and(|code| code != 0) {
		bail!("{}", reason_of(&stderr));
	}
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: ticker.moved(),
			total,
		})
		.await;
	Ok(CopyOutcome::Done)
}

/// Send one local file up to `remote` (§17), the mirror of `fetch`.
pub async fn send(
	runner: &Runner,
	local: &Path,
	remote: &str,
	resume: bool,
	events: &mpsc::Sender<SshEvent>,
	cancel: &Arc<AtomicBool>,
) -> Result<CopyOutcome> {
	let total = tokio::fs::metadata(local)
		.await
		.context("could not read the local file")?
		.len();
	let have = if resume {
		size(runner, remote).await
	} else {
		None
	};
	let offset = match transfer::resume_start(resume, have, total) {
		Start::Skip => return Ok(CopyOutcome::Done),
		Start::At(offset) => offset,
	};

	let mut file = tokio::fs::File::open(local)
		.await
		.context("could not open the local file")?;
	if offset > 0 {
		use tokio::io::AsyncSeekExt;
		file.seek(std::io::SeekFrom::Start(offset))
			.await
			.context("could not seek the local file")?;
	}
	let mut channel = open_write(runner, remote, offset > 0).await?;
	let mut buffer = vec![0u8; 32 * 1024];
	let mut ticker = transfer::Ticker::default();
	ticker.settle(offset);
	loop {
		if cancel.load(Ordering::Relaxed) {
			// A cancelled upload leaves no half file behind, matching the SFTP path.
			let _ = channel.eof().await;
			let _ = remove(runner, &[remote.to_owned()]).await;
			return Ok(CopyOutcome::Cancelled);
		}
		let read = file
			.read(&mut buffer)
			.await
			.context("could not read the local file")?;
		if read == 0 {
			break;
		}
		channel
			.data(&buffer[..read])
			.await
			.context("could not send the file's bytes")?;
		if let Some(sent) = ticker.advance(read as u64) {
			let _ = events
				.send(SshEvent::TransferProgress { sent, total })
				.await;
		}
	}
	finish_write(&mut channel).await?;
	let _ = events
		.send(SshEvent::TransferProgress {
			sent: ticker.moved(),
			total,
		})
		.await;
	Ok(CopyOutcome::Done)
}

/// Open the local destination at `offset`: appending to a partial for a resume, truncating for a
/// fresh copy. The parent directory is made first, so a tree copy never fails for want of it.
async fn open_local(local: &Path, offset: u64) -> Result<tokio::fs::File> {
	if let Some(parent) = local.parent() {
		let _ = tokio::fs::create_dir_all(parent).await;
	}
	if offset == 0 {
		return tokio::fs::File::create(local)
			.await
			.context("could not create the local file")
			// Same classing as the SFTP backend's `open_local_at`: a destination that was never
			// created has no partial, so the failure is final rather than resumable (§16).
			.map_err(transfer::mark_refused);
	}
	let file = tokio::fs::OpenOptions::new()
		.write(true)
		.open(local)
		.await
		.context("could not open the local partial")
		.map_err(transfer::mark_refused)?;
	use tokio::io::AsyncSeekExt;
	let mut file = file;
	file.seek(std::io::SeekFrom::Start(offset))
		.await
		.context("could not seek the local partial")?;
	Ok(file)
}

/// Walk a remote folder into a plan (§19), for a recursive download.
///
/// Two `find` runs rather than one clever one: the directories, then the files with their sizes.
/// `find -exec wc -c {} +` prints `size path` per file, which is exact — the size a progress bar
/// totals over should not be guessed from a listing's columns.
///
/// **`-L` makes every run follow symlinks**, exactly as the SFTP walk does since §17: a link to a
/// folder is walked as a folder and a link to a file is measured and copied as a file. It also
/// changes what `-type l` still matches — under `-L` a link that resolves has become its target,
/// so only the DANGLING ones are left, which is precisely the count `skipped_links` wants.
///
/// `ponytail:` the cycle is `find`'s problem here, not ours. GNU find under `-L` notices a
/// directory it is already inside and refuses to descend, printing "File system loop detected" on
/// stderr — which is why stderr is dropped from these runs, it is a warning about the tree and not
/// an error about the command. A `find` without that check would spin instead. The SFTP path,
/// which is what runs unless the server has no sftp subsystem, decides this itself with `realpath`
/// and `transfer::loops_back`. Upgrade path: `find -L ... -printf` the inode and cut cycles here.
pub async fn walk(runner: &impl Exec, root: &str) -> Result<TreePlan> {
	let quoted = shell_quote(root);
	let mut plan = TreePlan::default();

	let dirs = runner
		.stdout(&format!("find -L {quoted} -type d -print 2>/dev/null"))
		.await?;
	// `find` lists a parent before its children, which is the order the plan promises.
	for line in dirs.lines().filter(|line| !line.is_empty()) {
		if let Some(rel) = relative_parts(root, line) {
			plan.dirs.push(rel);
		}
	}
	// The root must be first even when `find` names it in some other form.
	if !plan.dirs.iter().any(std::vec::Vec::is_empty) {
		plan.dirs.insert(0, Vec::new());
	}

	let files = runner
		.stdout(&format!(
			"find -L {quoted} -type f -exec wc -c {{}} + 2>/dev/null"
		))
		.await?;
	for line in files.lines().filter(|line| !line.trim().is_empty()) {
		let Some((size, path)) = split_wc(line) else {
			continue;
		};
		// `wc` with several files ends with a `total` line, which is not a path on the remote.
		if path == "total" {
			continue;
		}
		if let Some(rel) = relative_parts(root, path) {
			plan.files.push(PlannedFile {
				rel,
				size,
				// No timestamps or mode: see the module note. The copy lands with what the remote's
				// own umask gives it rather than with a stamp invented here.
				mtime: None,
				atime: None,
				mode: None,
			});
		}
	}

	// Under `-L` this finds only the links that did NOT resolve — see the note above.
	let links = runner
		.stdout(&format!("find -L {quoted} -type l -print 2>/dev/null"))
		.await
		.unwrap_or_default();
	plan.skipped_links = links.lines().filter(|line| !line.is_empty()).count();
	Ok(plan)
}

/// One `wc -c` line as its size and its path. The count is right-aligned and the path is the rest
/// of the line, so the split is at the FIRST run of spaces — a path containing spaces survives.
fn split_wc(line: &str) -> Option<(u64, &str)> {
	let trimmed = line.trim_start();
	let (count, rest) = trimmed.split_once(' ')?;
	Some((count.parse().ok()?, rest.trim_start()))
}

/// A remote path's components relative to `root`, or `None` when it is not inside it. The root
/// itself is the empty list, which is what a plan calls its own top.
fn relative_parts(root: &str, path: &str) -> Option<Vec<String>> {
	let root = root.trim_end_matches('/');
	let rest = path.strip_prefix(root)?;
	Some(
		rest.split('/')
			.filter(|part| !part.is_empty())
			.map(str::to_owned)
			.collect(),
	)
}

/// The wiring a tree copy needs beyond the files themselves: where to report progress, where a
/// collision answer will come from, and the flag that says stop (§16, §17, §19). One value rather
/// than three parameters, so the copy functions read as "these files, that way" instead of a row of
/// channels.
pub struct TreeRun<'a> {
	pub events: &'a mpsc::Sender<SshEvent>,
	pub answers: &'a mut mpsc::Receiver<ConflictChoice>,
	pub cancel: &'a Arc<AtomicBool>,
}

/// Copy a whole remote folder down (§19) — the shell backend's tree download.
///
/// The shape is the SFTP path's: create every directory, then copy every file, asking about each
/// collision through the shared conflict protocol (`transfer::resolve`) so a sticky "…all" answer
/// settles the rest without prompting. Returns the outcome and how many files were skipped.
pub async fn fetch_tree(
	runner: &Runner,
	plan: &TreePlan,
	remote_root: &str,
	local_root: &Path,
	resume: bool,
	run: TreeRun<'_>,
) -> Result<CopyOutcome> {
	let TreeRun {
		events,
		answers,
		cancel,
	} = run;
	let mut sticky = None;
	for rel in &plan.dirs {
		let dir = transfer::local_join(local_root, rel);
		tokio::fs::create_dir_all(&dir)
			.await
			.with_context(|| format!("could not create {}", dir.display()))
			.map_err(transfer::mark_refused)?;
	}
	for file in &plan.files {
		if cancel.load(Ordering::Relaxed) {
			return Ok(CopyOutcome::Cancelled);
		}
		let source = transfer::remote_join(remote_root, &file.rel);
		let mut destination = transfer::local_join(local_root, &file.rel);
		// A resume is continuing its own earlier work, so an existing destination is expected and
		// never a collision to ask about (§16).
		if !resume && tokio::fs::metadata(&destination).await.is_ok() {
			let name = file.rel.last().cloned().unwrap_or_default();
			match transfer::resolve(events, answers, &mut sticky, &name).await {
				FileAction::Overwrite => {}
				FileAction::KeepBoth => {
					destination = free_local(&destination);
				}
				FileAction::Skip => continue,
				FileAction::Cancel => return Ok(CopyOutcome::Cancelled),
			}
		}
		match fetch(runner, &source, &destination, resume, events, cancel).await? {
			CopyOutcome::Done => {}
			CopyOutcome::Cancelled => return Ok(CopyOutcome::Cancelled),
		}
	}
	Ok(CopyOutcome::Done)
}

/// Send a whole local folder up (§17) — the shell backend's tree upload, the mirror of `fetch_tree`.
pub async fn send_tree(
	runner: &Runner,
	plan: &TreePlan,
	local_root: &Path,
	remote_root: &str,
	resume: bool,
	run: TreeRun<'_>,
) -> Result<CopyOutcome> {
	let TreeRun {
		events,
		answers,
		cancel,
	} = run;
	let mut sticky = None;
	make_dirs(runner, remote_root).await?;
	for rel in &plan.dirs {
		if rel.is_empty() {
			continue;
		}
		make_dirs(runner, &transfer::remote_join(remote_root, rel)).await?;
	}
	for file in &plan.files {
		if cancel.load(Ordering::Relaxed) {
			return Ok(CopyOutcome::Cancelled);
		}
		let source = transfer::local_join(local_root, &file.rel);
		let mut destination = transfer::remote_join(remote_root, &file.rel);
		if !resume && exists(runner, &destination).await {
			let name = file.rel.last().cloned().unwrap_or_default();
			match transfer::resolve(events, answers, &mut sticky, &name).await {
				FileAction::Overwrite => {}
				FileAction::KeepBoth => {
					let (dir, leaf) = split_remote(&destination);
					destination = free_name(runner, &dir, &leaf).await;
				}
				FileAction::Skip => continue,
				FileAction::Cancel => return Ok(CopyOutcome::Cancelled),
			}
		}
		match send(runner, &source, &destination, resume, events, cancel).await? {
			CopyOutcome::Done => {}
			CopyOutcome::Cancelled => return Ok(CopyOutcome::Cancelled),
		}
	}
	Ok(CopyOutcome::Done)
}

/// A remote path split into its directory and its last component, for a "keep both" answer.
fn split_remote(path: &str) -> (String, String) {
	match path.rsplit_once('/') {
		Some(("", leaf)) => ("/".to_owned(), leaf.to_owned()),
		Some((dir, leaf)) => (dir.to_owned(), leaf.to_owned()),
		None => (".".to_owned(), path.to_owned()),
	}
}

/// A free `name-1` beside an occupied LOCAL path, for a "keep both" answer on a download.
///
/// This one took a whole path where its three twins took a folder and a name, and split it with
/// `file_stem`/`extension` where they used `rsplit_once`, so it was the only one that could ever
/// have disagreed with the rest about what a name's extension is. It now asks the same shared rule
/// as everything else. A path with no parent and no name has nothing to make a candidate out of,
/// so it comes back unchanged — the caller's `exists` check has already told it that much.
fn free_local(path: &Path) -> std::path::PathBuf {
	let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
		return path.to_path_buf();
	};
	let name = name.to_string_lossy();
	for attempt in 1..=explorer::FREE_NAME_TRIES {
		let candidate = parent.join(explorer::free_candidate(&name, attempt));
		if !candidate.exists() {
			return candidate;
		}
	}
	parent.join(explorer::free_candidate(&name, explorer::FREE_NAME_TRIES))
}

/// A remote that answers out of a script instead of over a socket (§46), for the tests below.
///
/// Every function above that only composes a command and reads the reply now takes `&impl Exec`
/// rather than `&Runner`, and this is why: a `Runner` that will answer anything needs a live
/// session, so none of them could be tested at all. This one records the snippet it was asked to
/// run and hands back whatever was queued for it — which makes BOTH halves assertable at once, the
/// command that went out and the parse of what came back. They are worth testing together: the
/// quoting is a security boundary and the parsing is a compatibility one, and the pair of them is
/// the entire shell backend.
#[cfg(test)]
#[derive(Default)]
struct Script {
	/// What the caller asked to run, in order.
	ran: std::cell::RefCell<Vec<String>>,
	/// What to answer with. `None` means the command failed.
	reply: Option<String>,
}

#[cfg(test)]
impl Script {
	/// A remote that answers every command with `reply`.
	fn saying(reply: &str) -> Self {
		Self {
			ran: std::cell::RefCell::new(Vec::new()),
			reply: Some(reply.to_owned()),
		}
	}

	/// A remote that refuses every command.
	fn refusing() -> Self {
		Self::default()
	}

	/// The one command that was run. Panics if there was not exactly one, which is itself part of
	/// what these tests check: a listing is one round trip, not several.
	fn only_command(&self) -> String {
		let ran = self.ran.borrow();
		assert_eq!(ran.len(), 1, "expected exactly one command: {ran:?}");
		ran[0].clone()
	}
}

#[cfg(test)]
impl Exec for Script {
	async fn stdout(&self, snippet: &str) -> Result<String> {
		self.ran.borrow_mut().push(snippet.to_owned());
		match &self.reply {
			Some(reply) => Ok(reply.clone()),
			None => bail!("the remote refused"),
		}
	}

	async fn succeeds(&self, snippet: &str) -> bool {
		self.ran.borrow_mut().push(snippet.to_owned());
		self.reply.is_some()
	}
}

#[cfg(test)]
mod backend_tests {
	use super::*;

	/// The tree's listing: one `ls`, and only the entries marked as directories (§18).
	#[tokio::test]
	async fn the_tree_asks_ls_once_and_keeps_only_the_folders() {
		let remote = Script::saying("bin/\nhosts\nnginx/\nresolv.conf\n");
		let dirs = dirs(&remote, "/etc").await.expect("the listing arrived");

		// `-1Ap`: one per line, dotfiles included, a `/` on real directories. `--` stops a path
		// beginning with a dash being read as an option.
		assert_eq!(remote.only_command(), "ls -1Ap -- '/etc'");
		assert_eq!(dirs, vec!["bin".to_owned(), "nginx".to_owned()]);
	}

	/// The pane's listing reads the TYPE off the suffix `ls -F` puts there (§19).
	#[tokio::test]
	async fn the_pane_reads_the_type_marker_off_each_name() {
		let remote = Script::saying("bin/\nlink@\nrun*\nplain\n");
		let entries = entries(&remote, "/srv").await.expect("the listing arrived");

		assert_eq!(remote.only_command(), "ls -1AF -- '/srv'");
		let seen: Vec<(&str, FilesKind)> = entries
			.iter()
			.map(|entry| (entry.name.as_str(), entry.kind))
			.collect();
		// The executable's `*` is stripped and it is a plain file: the pane cares about folder,
		// link or file, and nothing else `-F` marks is a fourth kind.
		assert!(seen.contains(&("bin", FilesKind::Dir)));
		assert!(seen.contains(&("link", FilesKind::Link)));
		assert!(seen.contains(&("run", FilesKind::File)));
		assert!(seen.contains(&("plain", FilesKind::File)));
	}

	/// A NAME CARRYING A QUOTE reaches the remote as a name, not as commands (§18).
	///
	/// This is the security-bearing one, and until now it could only be checked by reading
	/// `shell_quote` in isolation and trusting every call site to have used it. Here the composed
	/// command itself is the assertion.
	#[tokio::test]
	async fn a_name_that_looks_like_a_command_is_still_a_name() {
		let remote = Script::saying("");
		let _ = dirs(&remote, "/tmp/'; rm -rf ~").await;
		assert_eq!(
			remote.only_command(),
			r"ls -1Ap -- '/tmp/'\''; rm -rf ~'",
			"the quote is closed, escaped and reopened — never left to the shell"
		);
	}

	/// A size comes from `wc -c` through a REDIRECT, so it counts the file's contents whatever the
	/// path looks like.
	#[tokio::test]
	async fn a_size_is_counted_through_a_redirect() {
		let remote = Script::saying("  4096\n");
		assert_eq!(size(&remote, "/etc/hosts").await, Some(4096));
		assert_eq!(remote.only_command(), "wc -c < '/etc/hosts'");
	}

	/// A remote that will not answer is not a size of zero.
	#[tokio::test]
	async fn a_size_that_cannot_be_read_is_absent_rather_than_zero() {
		let remote = Script::refusing();
		assert_eq!(size(&remote, "/etc/shadow").await, None);
	}

	/// The test and the create are ONE command, so nothing can slip into the path between them
	/// (§18).
	#[tokio::test]
	async fn making_a_folder_tests_and_creates_in_one_breath() {
		let remote = Script::saying("");
		let _ = make_dir(&remote, "/srv/new").await;

		let command = remote.only_command();
		assert!(
			command.contains("if [ -e '/srv/new' ]") && command.contains("mkdir -- '/srv/new'"),
			"one command, both halves: {command}"
		);
		// And it is a refusal, not a merge: an existing entry must not be replaced.
		assert!(command.contains("exit 1"), "{command}");
	}

	/// A rename refuses an occupied destination, by the same one-command rule (§18).
	#[tokio::test]
	async fn a_rename_refuses_to_replace_what_is_already_there() {
		let remote = Script::saying("");
		let _ = rename(&remote, "/srv/a", "/srv/b").await;

		let command = remote.only_command();
		assert!(command.contains("if [ -e '/srv/b' ]"), "{command}");
		assert!(command.contains("mv -- '/srv/a' '/srv/b'"), "{command}");
	}

	/// A delete takes every path in one command, each quoted separately (§18). `--` matters more
	/// here than anywhere: a blunt instrument must only ever see paths, never options.
	#[tokio::test]
	async fn a_delete_quotes_every_path_and_stops_option_parsing() {
		let remote = Script::saying("");
		let _ = remove(&remote, &["/srv/a".to_owned(), "-rf".to_owned()]).await;
		assert_eq!(remote.only_command(), "rm -rf -- '/srv/a' '-rf'");
	}

	/// "Keep both" probes candidate names in order and stops at the first free one (§17), sharing
	/// its shape with every other backend through `explorer::free_candidate`.
	#[tokio::test]
	async fn keep_both_takes_the_first_name_the_remote_does_not_have() {
		// A remote that says every candidate is free: the first one wins.
		let remote = Script::refusing();
		let free = free_name(&remote, "/srv", "notes.txt").await;
		assert_eq!(free, "/srv/notes-1.txt");
		assert_eq!(remote.only_command(), "[ -e '/srv/notes-1.txt' ]");
	}

	/// An existence check that the remote will not answer reads as "not there" (§18) — the caller
	/// asked about the remote's state, and "could not ask" is not "yes".
	#[tokio::test]
	async fn a_question_the_remote_will_not_answer_is_not_a_yes() {
		let refused = Script::refusing();
		assert!(!exists(&refused, "/etc/hosts").await);

		let answered = Script::saying("");
		assert!(exists(&answered, "/etc/hosts").await);
		assert_eq!(answered.only_command(), "[ -e '/etc/hosts' ]");
	}
}

#[cfg(test)]
mod tests {
	use super::{relative_parts, split_remote, split_wc};

	#[test]
	fn a_wc_line_splits_into_a_size_and_a_path_that_may_hold_spaces() {
		assert_eq!(split_wc("  1234 /etc/hosts"), Some((1234, "/etc/hosts")));
		assert_eq!(
			split_wc("42 /srv/my files/a b.txt"),
			Some((42, "/srv/my files/a b.txt"))
		);
		// The trailing summary line `wc` adds for several files is not a path.
		assert_eq!(split_wc("  1276 total"), Some((1276, "total")));
		assert_eq!(split_wc("nonsense"), None);
	}

	#[test]
	fn a_walked_path_is_read_as_components_under_the_root() {
		assert_eq!(
			relative_parts("/srv/app", "/srv/app/logs/today.log"),
			Some(vec!["logs".to_owned(), "today.log".to_owned()])
		);
		// The root itself is the empty list — a plan's own top.
		assert_eq!(relative_parts("/srv/app", "/srv/app"), Some(Vec::new()));
		assert_eq!(relative_parts("/srv/app/", "/srv/app"), Some(Vec::new()));
		// Anything outside the root is not part of the tree.
		assert_eq!(relative_parts("/srv/app", "/etc/passwd"), None);
	}

	#[test]
	fn a_remote_path_splits_into_its_folder_and_its_name() {
		assert_eq!(
			split_remote("/etc/nginx/nginx.conf"),
			("/etc/nginx".to_owned(), "nginx.conf".to_owned())
		);
		// A file directly at the root keeps the root as its folder, not an empty string.
		assert_eq!(
			split_remote("/passwd"),
			("/".to_owned(), "passwd".to_owned())
		);
		assert_eq!(
			split_remote("relative.txt"),
			(".".to_owned(), "relative.txt".to_owned())
		);
	}
}

// local/pty.rs — a pseudo-terminal on this machine (PLAN §103).
//
// The terminal side of cmote is already written against a pty: SSH asks the server for one
// (`request_pty`), the shell runs on it, and `term` gets bytes. A local session is the same picture
// with the network taken out — a pty pair here, a shell on its slave end, and the same bytes. So this
// module is deliberately small: it opens the pair, spawns the shell, and hands back the two ends the
// session loop needs. Everything above it (the emulator, the grid, the keymap, the queries) is
// untouched, because from `term`'s point of view nothing has changed.
//
// Four things about a pty are not obvious, and all four are why `portable-pty` is a dependency rather
// than a few lines of `windows-sys`:
//
//   * **The slave must be dropped after the spawn.** The child inherits its own handles to the slave
//     end; the copy this process holds is a second owner of it. It is dropped in [`Pty::open`], one
//     line after `spawn_command`, and the ordering is the whole reason that line exists.
//   * **Reading and writing block.** A pty read parks until a byte arrives, which for an idle shell
//     is forever, and it is not an `async` read on either platform. So each direction gets its own
//     OS thread and talks to the session loop over a channel — the same shape `bridge` uses for the
//     SSH thread (§4), and for the same reason: a blocking call must never sit on an async runtime's
//     worker.
//   * **A shell exiting does NOT close the pty, so EOF is not how a session ends.** This was the
//     first design here and it was wrong; a test that ran a real child and waited for the stream to
//     end sat there for twenty seconds. On Windows the ConPTY object owns the output pipe and keeps it
//     open until `ClosePseudoConsole` — which happens when the master is dropped, and the master is
//     dropped when the session decides it is over. Waiting on EOF to decide that is waiting on
//     something the decision itself causes. So a THIRD thread waits on the child, and its exit — not
//     the stream's end — is the event the session loop watches ([`Stream::exited`]); the reader is
//     unblocked afterwards, by the master being dropped.
//   * **A ConPTY asks the terminal a question before it will say anything.** `portable-pty` creates it
//     with `PSUEDOCONSOLE_INHERIT_CURSOR` (hard-coded), and a ConPTY made that way sends `CSI 6 n` —
//     "where is the cursor?" — and holds every byte the child prints until it is answered. cmote
//     answers as a matter of course, because the engine replies to DSR (§23) and `app` sends whatever
//     `Terminal::process` hands back straight down the input path. Nothing here had to be added for
//     it, but it is load-bearing, so the test at the bottom of this file asserts the exchange happens
//     rather than leaving it to be rediscovered.

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::mpsc;

use super::shells::Shell;
use crate::term;

/// How many bytes one read off the pty may return. A terminal flood arrives in whatever sizes the
/// OS chooses; this only bounds how much one read hands over at a time, and a generous buffer means
/// a `cat` of a large file costs fewer channel sends per screenful.
const READ_CHUNK: usize = 32 * 1024;

/// How many chunks may wait between the pty and the session loop before the reader thread parks.
/// Bounded so a program printing faster than the GUI draws cannot grow memory without limit — the
/// same backpressure rule as the SSH channel (§4).
const CHANNEL_BOUND: usize = 64;

/// A live local shell: the pty it runs on, the way to end it, and the way to write to it.
///
/// The master end stays here rather than being cloned about, because it is what `resize` is called on
/// and what — when dropped — closes the pty and so unblocks the reader thread.
///
/// The child itself is NOT here: it moved into the thread that waits on it (see the module note), and
/// what is left is a killer handle, which is all `close` needs. That split is deliberate — waiting and
/// killing are the only two things anyone does to it, they happen on different threads, and
/// `portable-pty` hands out a killer for exactly this reason.
pub struct Pty {
	master: Box<dyn MasterPty + Send>,
	killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
	/// The writer thread's inbox. Writing to a pty blocks, so nothing is written from the session
	/// loop; bytes are handed to a thread that does nothing else.
	writes: mpsc::Sender<Vec<u8>>,
}

/// Everything the session loop READS from a live shell: the bytes it prints, and the one event that
/// ends a session.
///
/// Handed back separately from [`Pty`] rather than as methods on it, and that separation is load-bearing
/// in two ways at once.
///
/// The first is borrowing: the session loop selects over both of these while its other branch types
/// into the pty, so a receiver reached through `&mut pty` would hold the pty for the whole `select!`
/// and lock out the write it exists to allow.
///
/// The second is CANCELLATION, and it is the reason this shape exists rather than a pair of methods.
/// The first version of this stored a `oneshot::Receiver` inside `Pty` behind an `async fn exit(&mut
/// self)` that took it out of an `Option`. `tokio::select!` drops the branch futures that did not win,
/// so the very first chunk of output cancelled that future — after it had already taken the receiver,
/// which was then dropped with it. Every later call found `None` and parked forever, so the shell could
/// exit and the session would never notice. It looked exactly like `child.wait()` not working. Both
/// receivers here are `tokio::sync::mpsc`, whose `recv` is documented cancel-safe: a poll that loses the
/// race leaves the channel exactly as it was.
pub struct Stream {
	/// What the shell printed. Ends only when the pty is torn down, which cmote does itself.
	pub bytes: mpsc::Receiver<Vec<u8>>,
	/// Yields once when the child exits, then closes. THIS is what ends a local session (see the
	/// module note).
	pub exited: mpsc::Receiver<()>,
}

impl Pty {
	/// Open a pty, start `shell` on it, and return the handle plus the stream of everything the shell
	/// prints.
	///
	/// The pty is sized to the emulator's own initial grid, exactly as the SSH path sizes the remote
	/// one (`term::DEFAULT_COLS` / `DEFAULT_ROWS`) — the single source of truth is `term`, so the
	/// shell's idea of the window and cmote's agree from the first byte. The GUI refits both a moment
	/// later, once it knows the real window size.
	pub fn open(shell: &Shell) -> Result<(Self, Stream)> {
		let pair = native_pty_system()
			.openpty(size(term::DEFAULT_COLS, term::DEFAULT_ROWS))
			.context("could not open a pseudo-terminal")?;

		let mut command = CommandBuilder::new(&shell.program);
		command.args(&shell.args);
		// `TERM` is what tells the shell and every program under it which escape sequences they may
		// use. The same value the SSH path requests, so a program behaves the same in a local tab as
		// in a remote one — and so cmote's own compatibility work is exercised by both.
		command.env("TERM", "xterm-256color");
		// Start where the user is. A shell that opens at cmote's own working directory would be
		// standing wherever the exe happens to live, which is nobody's idea of home.
		if let Some(home) = super::path::native_home() {
			command.cwd(home);
		}

		let child = pair
			.slave
			.spawn_command(command)
			.with_context(|| format!("could not start {}", shell.program.display()))?;
		// The ordering that matters (see the module note): the child has its own handles now, so this
		// process's copy of the slave end must go rather than sitting here as a second owner of it.
		drop(pair.slave);
		// The killer is taken BEFORE the child moves into its waiter thread — it is the only handle on
		// the process this side keeps, and `close` is written entirely in terms of it.
		let killer = child.clone_killer();

		let reader = pair
			.master
			.try_clone_reader()
			.context("could not read from the pseudo-terminal")?;
		let writer = pair
			.master
			.take_writer()
			.context("could not write to the pseudo-terminal")?;

		let (output_tx, output_rx) = mpsc::channel::<Vec<u8>>(CHANNEL_BOUND);
		spawn_reader(reader, output_tx);
		let writes = spawn_writer(writer);
		let exited = spawn_waiter(child);

		Ok((
			Self {
				master: pair.master,
				killer,
				writes,
			},
			Stream {
				bytes: output_rx,
				exited,
			},
		))
	}

	/// Send bytes to the shell — a keystroke, a pasted line, or an emulator reply to a query (§23).
	///
	/// Failing silently is right here: the only way the send fails is that the writer thread has gone,
	/// which means the shell has exited. The session is already ending by the route that noticed it
	/// (the output channel closing), and a second complaint about the same fact would only race it.
	///
	/// It hands back a future rather than being an `async fn`, which is not a style choice. A pty
	/// master is `Send` but not `Sync` — the OS handle inside it may be used from one thread at a time
	/// — so a `&self` held across an `await` would make the whole session task non-`Send` and
	/// unspawnable. Cloning the channel here and letting the returned future own it means nothing
	/// borrows the pty while the send waits, and `pty.write(bytes).await` reads exactly the same at the
	/// call site.
	pub fn write(&self, bytes: Vec<u8>) -> impl std::future::Future<Output = ()> + Send + use<> {
		let writes = self.writes.clone();
		async move {
			let _ = writes.send(bytes).await;
		}
	}

	/// Tell the pty the window changed size, so the shell reflows and full-screen programs redraw.
	/// The mirror of `SshCommand::Resize` (§9): the GUI is the single source of the grid size and both
	/// backends are told the same numbers.
	pub fn resize(&self, cols: u16, rows: u16) {
		if let Err(error) = self.master.resize(size(cols, rows)) {
			// Not fatal and not worth a dialog: the shell keeps running at the size it had, and the
			// next resize will try again. Logged so a pty that refuses every resize is findable.
			eprintln!("could not resize the local pty: {error:#}");
		}
	}

	/// End the shell — the Disconnect button, or the tab closing.
	///
	/// Kills rather than asking: cmote has no way to know whether this shell would exit on an EOF (a
	/// `cmd.exe` sitting at a prompt does, one running a program does not), and a Disconnect the user
	/// confirmed must actually end the session. The child's own children go with it on Windows, where
	/// the ConPTY owns the console they are attached to.
	/// The result is deliberately dropped, and not out of laziness: on Windows `portable-pty` 0.9's
	/// killer has its success test INVERTED — `WinChildKiller::kill` returns `Err(last_os_error())` when
	/// `TerminateProcess` succeeded (nonzero) and `Ok(())` when it failed. So the value carries no
	/// information at all, and logging it printed "could not end the local shell: the operation completed
	/// successfully" after every clean Disconnect. There is also nothing a caller could do with the
	/// answer: a child that has already exited cannot be killed, and that is the ordinary case here —
	/// `close` is called both on Disconnect and again on the way out of the session loop.
	pub fn close(&mut self) {
		let _ = self.killer.kill();
	}
}

/// The pty size in the crate's own shape. The pixel fields are zero, which is what says "no opinion":
/// they exist for programs that ask a terminal for its pixel dimensions, and cmote's grid is measured
/// in cells (§11) — a made-up pixel size would be worse than none.
fn size(cols: u16, rows: u16) -> PtySize {
	PtySize {
		rows,
		cols,
		pixel_width: 0,
		pixel_height: 0,
	}
}

/// The thread that waits for the child, and the receiver that fires when it is gone.
///
/// This is the only authority on "the shell has exited" (see the module note): the output stream is
/// not, because the ConPTY holds it open until the master is dropped, and the master is dropped as a
/// CONSEQUENCE of the session ending. `wait` blocks, so it gets a thread of its own like the other two.
///
/// An `mpsc` of capacity one rather than a `oneshot`, for cancel-safety inside the session's `select!`
/// — see [`Stream`], where getting this wrong cost a debugging session.
///
/// The child moves in here and is dropped when the thread returns, which reaps it. Nothing outside
/// needs it: `close` works through the killer handle taken before the move.
fn spawn_waiter(mut child: Box<dyn portable_pty::Child + Send + Sync>) -> mpsc::Receiver<()> {
	let (tx, rx) = mpsc::channel::<()>(1);
	std::thread::Builder::new()
		.name("cmote-local-pty-wait".to_owned())
		.spawn(move || {
			if let Err(error) = child.wait() {
				// The status could not be read. The child is still gone — or unreachable, which comes to
				// the same thing for a session — so this reports it either way and only logs the detail.
				eprintln!("could not wait for the local shell: {error}");
			}
			// `blocking_send`, because this is a plain OS thread and not a runtime worker. A failure means
			// the session already ended by another route (Disconnect, the tab closing) and nobody is
			// listening — nothing to do about that.
			let _ = tx.blocking_send(());
		})
		.expect("failed to spawn the local pty waiter thread");
	rx
}

/// The thread that reads the shell's output forever.
///
/// It ends on EOF or on a read error, and either way dropping `output` closes the channel. On Windows
/// that only happens once the master is dropped, so this thread outlives the child and is unblocked by
/// the session tearing the pty down — which is why the CHILD's exit, not this thread's, is what ends a
/// session. A send that fails means the GUI dropped the session first, so there is nothing left to
/// read for.
fn spawn_reader(mut reader: Box<dyn std::io::Read + Send>, output: mpsc::Sender<Vec<u8>>) {
	std::thread::Builder::new()
		.name("cmote-local-pty-read".to_owned())
		.spawn(move || {
			let mut buffer = vec![0u8; READ_CHUNK];
			loop {
				match reader.read(&mut buffer) {
					// EOF: the shell exited and the pty closed.
					Ok(0) => break,
					Ok(read) => {
						if output.blocking_send(buffer[..read].to_vec()).is_err() {
							break;
						}
					}
					Err(error) => {
						// A closed pty reads as an error rather than EOF on Windows, so this is the
						// ordinary end of a session as often as it is a fault. Logged, not reported.
						eprintln!("local pty read ended: {error}");
						break;
					}
				}
			}
		})
		.expect("failed to spawn the local pty reader thread");
}

/// The thread that writes to the shell, and its inbox.
///
/// One flush per batch rather than per byte: a paste arrives as one `Vec` and should reach the shell
/// as one write, and a keystroke is a batch of one either way.
fn spawn_writer(mut writer: Box<dyn std::io::Write + Send>) -> mpsc::Sender<Vec<u8>> {
	let (tx, mut rx) = mpsc::channel::<Vec<u8>>(CHANNEL_BOUND);
	std::thread::Builder::new()
		.name("cmote-local-pty-write".to_owned())
		.spawn(move || {
			while let Some(bytes) = rx.blocking_recv() {
				if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
					// The shell has gone. The reader thread is seeing the same thing and it is the one
					// that reports it, so this simply stops.
					break;
				}
			}
		})
		.expect("failed to spawn the local pty writer thread");
	tx
}

#[cfg(test)]
mod tests {
	use super::{Pty, READ_CHUNK, size};
	use crate::local::shells::{Kind, Shell};
	use crate::term;
	use std::path::PathBuf;

	#[test]
	fn the_pty_opens_at_the_emulators_own_grid_size() {
		// The same rule the SSH path follows (§9): `term` owns the size, both backends are told it, so
		// the shell's idea of the window and cmote's can never start out disagreeing.
		let opened = size(term::DEFAULT_COLS, term::DEFAULT_ROWS);
		assert_eq!(opened.cols, term::DEFAULT_COLS);
		assert_eq!(opened.rows, term::DEFAULT_ROWS);
		// No made-up pixel dimensions: zero is how a terminal says it has no opinion, and cmote
		// measures its grid in cells (§11).
		assert_eq!((opened.pixel_width, opened.pixel_height), (0, 0));
	}

	#[test]
	fn the_read_buffer_is_bounded() {
		// A flood of output must cost a bounded amount per read rather than whatever the OS offers.
		// A `const` block, so the buffer's shape is checked at compile time and the test only records
		// what the rule is.
		const { assert!(READ_CHUNK > 0 && READ_CHUNK <= 64 * 1024) };
	}

	/// The marker the child below prints. Distinctive enough that finding it in the output cannot be a
	/// coincidence — a pty carries the shell's own banners and prompts too.
	const MARKER: &str = "cmote-pty-marker-9f2a";

	/// A shell entry that prints [`MARKER`] and exits, or `None` on a machine with neither interpreter.
	///
	/// Deliberately NOT one of `catalogue`'s entries: those are interactive shells that never exit, so
	/// they could not test the EOF that ends a session. This runs one command and stops, which is what
	/// makes both halves — the output arriving and the channel closing — observable in one test.
	fn echoing_shell() -> Option<Shell> {
		#[cfg(windows)]
		{
			let program = std::env::var_os("ComSpec")
				.map(PathBuf::from)
				.filter(|path| path.is_file())?;
			Some(Shell {
				kind: Kind::Cmd,
				program,
				args: vec!["/c".to_owned(), "echo".to_owned(), MARKER.to_owned()],
			})
		}
		#[cfg(target_os = "macos")]
		{
			let program = PathBuf::from("/bin/sh");
			program.is_file().then(|| Shell {
				kind: Kind::Bash,
				program,
				args: vec!["-c".to_owned(), format!("echo {MARKER}")],
			})
		}
	}

	#[tokio::test]
	async fn a_real_child_runs_on_a_real_pty_and_its_exit_is_observed() {
		// The one test that exercises the PLATFORM rather than the arithmetic, and the only place the
		// three facts the module note rests on are actually observable:
		//   * the pty opens and a child really starts on its slave end;
		//   * its output reaches the session loop's channel; and
		//   * the channel CLOSES when the child exits — which is what a local session's `Disconnected`
		//     is made of, and what a slave handle left open after the spawn would silently break.
		//
		// It drives a real `term::Terminal` over the stream instead of just collecting bytes, because on
		// Windows that is not decoration — it is a REQUIREMENT. `portable-pty` creates the ConPTY with
		// `PSUEDOCONSOLE_INHERIT_CURSOR` (hard-coded, not a choice it offers), and a ConPTY created that
		// way asks the terminal where the cursor is (`CSI 6 n`) and then WAITS: nothing the child prints
		// arrives until it is answered. cmote answers as a matter of course — the engine replies to
		// DSR (§23) and `app` sends what `process` hands back straight down the input path, which is the
		// very loop reproduced here — but a version of this test that only read bytes hung for twenty
		// seconds and saw exactly four of them, `\x1b[6n`. So the coupling is asserted rather than
		// assumed: `answered` below is what proves the query was put and met.
		let Some(shell) = echoing_shell() else {
			eprintln!("skipped the pty round trip: this machine has no command interpreter");
			return;
		};
		let (pty, mut stream) = Pty::open(&shell).expect("a pty opens");
		// A resize on a live pty, in passing: it must not disturb the child or the stream.
		pty.resize(100, 30);

		let mut emulator = term::Terminal::new(term::DEFAULT_ROWS, term::DEFAULT_COLS);
		let mut seen = String::new();
		let mut answered = false;
		// The session loop in miniature: forward what the shell prints, feed it to the emulator, and send
		// back whatever the emulator answers — while watching for the child to exit. That last branch is
		// the point twice over: the stream will NOT end on its own, so a loop without it hangs, and the
		// branch has to survive being cancelled by the one beside it, which is what `Stream` is about.
		let ended = tokio::time::timeout(std::time::Duration::from_secs(20), async {
			loop {
				tokio::select! {
					biased;
					chunk = stream.bytes.recv() => {
						let Some(bytes) = chunk else { break };
						seen.push_str(&String::from_utf8_lossy(&bytes));
						let replies = emulator.process(&bytes);
						if !replies.is_empty() {
							answered = true;
							pty.write(replies).await;
						}
					}
					_ = stream.exited.recv() => break,
				}
			}
			// `cmd /c echo` prints and exits in the same breath, so its output can still be inside the
			// reader thread when the process is already gone. The session loop drains what is queued and
			// leaves; a test that has to SEE the line waits a moment longer for it. This is the one place
			// the two differ, and it is a property of the fixture — a shell that lives longer than one
			// command has always flushed its greeting well before it exits.
			while let Ok(Some(bytes)) =
				tokio::time::timeout(std::time::Duration::from_millis(500), stream.bytes.recv())
					.await
			{
				seen.push_str(&String::from_utf8_lossy(&bytes));
			}
		})
		.await
		.is_ok();

		assert!(
			ended,
			"the child's exit was observed — waiting on the output stream instead hangs, because the \
			 ConPTY holds it open until the master is dropped: {seen:?}"
		);
		assert!(
			answered,
			"the ConPTY asked where the cursor is and cmote answered — without this the child's output \
			 never arrives at all: {seen:?}"
		);
		assert!(
			seen.contains(MARKER),
			"the child's output arrived: {seen:?}"
		);
	}
}

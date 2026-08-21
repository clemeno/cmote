// local/session.rs — one local session's whole life (PLAN §103).
//
// This is the twin of `ssh::client::stream`, and it is deliberately the same shape: a `select!` over
// the shell's output and the commands from the GUI, answering both in the same events. It consumes
// the SAME [`SessionMsg`] the SSH session task does, which is the whole trick behind §103 — the
// command loop in `ssh::client::run` forwards every message without caring which kind of session is
// on the other end, so nothing between the GUI and here had to learn that local sessions exist.
//
// What a local session does NOT have is as important as what it does, and each absence is answered
// rather than ignored:
//
//   * **No handshake and no authentication.** `Connected` is sent immediately — there is no host key
//     to verify and no credential to prove. That is why the Local bar's buttons go straight to a
//     terminal with no form in between.
//   * **No second account (§45).** Elevating on Windows means UAC, which is a new process at a new
//     integrity level and not another shell on this one; there is nothing here for `sudo -u` to be.
//     An `Elevate` is refused with that reason rather than dropped, so the GUI's own flow ends
//     cleanly instead of waiting for a shell that will never open.
//   * **No port forwarding (§27).** A tunnel's whole purpose is to carry a connection through the
//     remote's network. There is no remote, so a forward is refused with that sentence.
//   * **No shell integration (§17).** That feature writes a cwd announcer into the REMOTE's shell
//     config so cmote can learn where the shell is. Refused here on purpose rather than made to work:
//     it would be cmote editing the user's own everyday profile, which is a much larger promise than
//     "open a terminal" — see §103's Not done.
//
// The one thing that ends a local session by itself is the shell exiting — there is no other party to
// hear it from. What SAYS the shell exited is the child process being waited on, not the output stream
// running dry: on Windows the ConPTY keeps that stream open until cmote closes the pty, so ending on it
// would be waiting for a consequence of the decision to be made before making it. `pty::Pty::exit` is
// the branch that matters, and `pty`'s module note has the whole story.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use super::pty::Pty;
use super::shells::LocalShell;
use super::{copy, fs};
use crate::bridge::{ConflictChoice, SshEvent};
use crate::ssh::client::SessionMsg;

/// How many conflict answers may queue for a recursive copy. Two would do — the user answers one
/// question at a time — but a sticky "…all" answer can be followed immediately by the next, so a
/// little slack costs nothing.
const ANSWER_BOUND: usize = 8;

/// How long a shell is given to leave on its own before it is killed (§104).
///
/// The GUI types the shell's own `exit` before it asks for a teardown (`Tab::end_session`), and a shell
/// standing at its prompt acts on that in tens of milliseconds. This is roughly an order of magnitude
/// more than that, so a slow profile or a shell writing its history still gets out on its own terms —
/// and it is short enough that nobody watches it, since the GUI has already left for the home screen by
/// the time this runs.
///
/// The wait is spent whether or not anything was typed: the session task cannot see the grid, so it
/// does not know whether the GUI judged typing safe. That costs a full window on the one path where
/// nothing was typed (a full-screen program was up), which is invisible except when quitting cmote.
const GOODBYE: std::time::Duration = std::time::Duration::from_millis(800);

/// Quitting cmote waits for every live session to report itself down, and gives up after
/// `QUIT_DRAIN_TIMEOUT` (§30). A goodbye window that reached into that budget would turn every quit
/// with a local tab open into a wait for the timeout, and the drain would report a session that never
/// came down. Checked here rather than remembered: the two constants live in different modules.
const _: () = assert!(GOODBYE.as_millis() * 2 <= crate::app::QUIT_DRAIN_TIMEOUT.as_millis());

/// Run a local session until the shell exits or the GUI asks it to stop.
///
/// Every outcome is reported as exactly one terminal event, the same contract the SSH session task
/// has: `Error` when the shell could not be started at all, `Disconnected` for every other ending —
/// the user typed `exit`, the program died, or Disconnect was confirmed.
#[expect(
	clippy::too_many_lines,
	reason = "the local session's message loop, deliberately the same shape as ssh::client::run (§103)"
)]
pub async fn run(
	shell: LocalShell,
	events: mpsc::Sender<SshEvent>,
	mut commands: mpsc::Receiver<SessionMsg>,
) {
	let (mut pty, mut stream) = match Pty::open(&shell) {
		Ok(opened) => opened,
		Err(error) => {
			// The detail goes to the log and a plain sentence to the user, the same rule the SSH task
			// follows (§12) — except that here the sentence can safely NAME the program, because it is
			// one cmote found on this machine rather than anything a remote said.
			eprintln!("could not start the local shell: {error:#}");
			let _ = events
				.send(SshEvent::Error(format!(
					"Could not start {}.",
					shell.kind.label()
				)))
				.await;
			return;
		}
	};

	// A shell is running: from the GUI's point of view this is exactly a connection opening, so it
	// gets the event that moves it to the terminal screen.
	let _ = events.send(SshEvent::Connected).await;
	// The pane renders every mtime against the machine's zone (§20). For a remote that is a probe with
	// a round trip, so it is deferred until the first listing; here it is one call, so it is answered
	// up front and the first listing is already correct rather than being re-rendered a moment later.
	fs::report_zone(&events).await;

	// The reply channel for the recursive copy currently running, and its cancel flag — held exactly
	// as `ssh::client::stream` holds them, and for the same reason: one transfer runs at a time, so
	// starting another simply replaces these, and a stale one belongs to a transfer that has ended and
	// is harmless.
	let mut conflicts: Option<mpsc::Sender<ConflictChoice>> = None;
	let mut cancel: Option<Arc<AtomicBool>> = None;

	// The cancel flags for the viewer reads in flight, keyed by the tab that asked (§121). The
	// remote session loop keeps the same map for the same reason — see the `FileLoad` arm below.
	let mut viewer_cancels: HashMap<u64, Arc<AtomicBool>> = HashMap::new();

	loop {
		tokio::select! {
					// Output first, always. `select!` picks at random among the branches that are ready, so
					// without this a shell's last line could be sitting in the channel while the exit branch
					// wins the toss and breaks the loop out from under it. Biased polling costs nothing and
					// says the priority out loud: bytes the shell has already produced are delivered before
					// the fact that it has stopped producing them.
					biased;

					// The shell printed something. `None` is the pty stream ending, which on Windows only happens
					// once cmote itself tore the pty down — so it is a way OUT of this loop but never the reason,
					// and the reason is the branch below.
					chunk = stream.bytes.recv() => {
						match chunk {
							Some(bytes) => {
								let _ = events
									.send(SshEvent::Output { identity: crate::bridge::LOGIN_IDENTITY, bytes })
									.await;
							}
							None => break,
						}
					}
					// The shell exited: the user typed `exit`, or the program died. THIS is what ends a local
					// session by itself — see `pty`'s note on why waiting for the output stream to end instead
					// would be waiting on something this decision causes.
					//
					// One last flush before leaving. `biased` above already guarantees that anything QUEUED has
					// been delivered; this catches what the reader thread has read but not yet handed over,
					// which is a different race and a real one — the exit is observed on its own thread, and a
					// shell's final line can be in flight when the process is already gone.
					_ = stream.exited.recv() => {
						while let Ok(bytes) = stream.bytes.try_recv() {
							let _ = events
								.send(SshEvent::Output { identity: crate::bridge::LOGIN_IDENTITY, bytes })
								.await;
						}
						break;
					}
					command = commands.recv() => {
						match command {
							// Typing, and the emulator's own replies to the queries a program sent (§23). Both go
							// to the one shell this session has, which is why neither needs its identity read.
							Some(SessionMsg::Data(bytes) | SessionMsg::Reply { bytes, .. }) => {
								pty.write(bytes).await;
							}
							Some(SessionMsg::Resize { cols, rows }) => pty.resize(cols, rows),

							// The file panes, the details popup and both viewers (§18, §19, §20, §32, §53). Each
							// runs on its own blocking task so a crowded directory or a large file never holds up
							// the terminal — the same rule the SFTP listings follow.
							//
							// Only the two listings are awaited, and the `.await` marks a real difference rather
							// than a habit: those two answer the VIRTUAL ROOT inline (the drive list is already in
							// hand, so there is nothing to spawn), and everything else hands its work to a task and
							// returns at once. An arm with no `.await` is an arm that cannot report anything before
							// the loop comes round again.
							Some(SessionMsg::ListDir(path)) => fs::list(&events, path).await,
							Some(SessionMsg::ListFiles { path, request }) => {
								fs::list_all(&events, path, request).await;
							}
							Some(SessionMsg::ReadLink(path)) => fs::read_link(&events, path),
							Some(SessionMsg::MakeDir(path)) => fs::make_dir(&events, path),
							Some(SessionMsg::Delete(paths)) => fs::remove(&events, paths),
							Some(SessionMsg::RenameDir { from, to }) => fs::rename(&events, from, to),
							// A viewer read gets its own cancel flag, keyed by the tab (§121) — NOT the single
							// slot `arm` hands the transfers below. That slot is right for a transfer because
							// only one runs at a time; any number of viewer tabs can be opening files at once,
							// so a close has to reach the read that was closed and no other. Stale entries are
							// pruned by strong count on insert: a finished read has dropped its `Arc`, and
							// nothing else tells this loop that it ended.
							Some(SessionMsg::FileLoad { viewer_id, path, limit, .. }) => {
								viewer_cancels.retain(|_, flag| Arc::strong_count(flag) > 1);
								let flag = Arc::new(AtomicBool::new(false));
								viewer_cancels.insert(viewer_id, flag.clone());
								fs::load(&events, viewer_id, path, limit, flag);
							}
							Some(SessionMsg::CancelFileLoad { viewer_id }) => {
								if let Some(flag) = viewer_cancels.remove(&viewer_id) {
									flag.store(true, Ordering::Relaxed);
								}
							}
							Some(SessionMsg::EditSave { viewer_id, path, bytes, .. }) => {
								fs::save(&events, viewer_id, path, bytes);
							}

							// The transfers (§16, §17, §19). A fresh cancel flag per transfer, kept here so the
							// status bar's ✕ can reach the one in flight; a fresh answer channel for the recursive
							// ones, which can park mid-way to ask about a collision.
							Some(SessionMsg::Upload { local, remote, overwrite, resume }) => {
								let flag = arm(&mut cancel);
								copy::upload(&events, local, remote, overwrite, resume, flag);
							}
							Some(SessionMsg::Download { remote, local, resume }) => {
								let flag = arm(&mut cancel);
								copy::download(&events, remote, local, resume, flag);
							}
							Some(SessionMsg::CheckUploads { dir, names }) => {
								copy::precheck(&events, dir, names);
							}
							Some(SessionMsg::UploadTree { local, remote, resume }) => {
								let (answers, receiver) = mpsc::channel::<ConflictChoice>(ANSWER_BOUND);
								conflicts = Some(answers);
								let flag = arm(&mut cancel);
								copy::upload_tree(&events, local, remote, resume, receiver, flag);
							}
							Some(SessionMsg::DownloadTree { remote, local, resume }) => {
								let (answers, receiver) = mpsc::channel::<ConflictChoice>(ANSWER_BOUND);
								conflicts = Some(answers);
								let flag = arm(&mut cancel);
								copy::download_tree(&events, remote, local, resume, receiver, flag);
							}
							// Forward the answer to the copy parked on it. A send that fails is nothing to act
							// on: the transfer already ended, so the answer simply had no one waiting.
							Some(SessionMsg::ResolveConflict(choice)) => {
								if let Some(answers) = conflicts.as_ref() {
									let _ = answers.send(choice).await;
								}
							}
							Some(SessionMsg::CancelTransfer) => {
								if let Some(flag) = cancel.as_ref() {
									flag.store(true, Ordering::Relaxed);
								}
							}

							// The three features a local session does not have (see the module note). Each is
							// REFUSED with its reason rather than dropped, so the GUI flow that asked ends instead
							// of waiting on an answer that would never come.
							Some(SessionMsg::Elevate { identity, .. }) => {
								let _ = events
									.send(SshEvent::IdentityEnded {
										identity,
										reason: Some(
											"A local session has one shell. Becoming another user on Windows means \
											 UAC, which starts a separate process at a different integrity level \
											 rather than another shell on this one."
												.to_owned(),
										),
									})
									.await;
							}
							Some(SessionMsg::AddForward { id, .. }) => {
								let _ = events
									.send(SshEvent::ForwardFailed {
										id,
										reason: "A local session has no connection to tunnel through.".to_owned(),
									})
									.await;
							}
							Some(SessionMsg::ProbeIntegration { .. } | SessionMsg::WriteIntegration { ..
		}) => {
								let _ = events
									.send(SshEvent::IntegrationFailed(
										"Shell integration writes a cwd announcer into a REMOTE shell's config. \
										 cmote does not edit your own profile on this machine."
											.to_owned(),
									))
									.await;
							}

							// Nothing to do, and nothing to report. `SelectIdentity` and `CloseIdentity` name
							// accounts a local session cannot have; `RemoveForward` names a forward that was
							// refused when it was asked for; the two auth answers belong to a handshake that never
							// happened. Each of them is the GUI being tidy, not the GUI being wrong.
							Some(SessionMsg::SelectIdentity(_) | SessionMsg::CloseIdentity(_) |
		SessionMsg::RemoveForward(_) | SessionMsg::ElevateAnswer { .. } |
		SessionMsg::Passphrase(_) | SessionMsg::Interactive(_)) => {}

							// Disconnect: the user confirmed one, the tab is closing, cmote is quitting. The GUI has
							// typed the shell's own `exit` just before this wherever typing was safe (§104), so the
							// shell is most likely already on its way out — give it a moment to finish going before
							// the kill below, and it gets to run its own exit path instead of being terminated
							// mid-history-write. A shell that will not go (something is running in front of it, it
							// ignored the word) is killed a fraction of a second later, which is what happened
							// immediately before this and is still the guarantee: a confirmed Disconnect ends the
							// session.
							Some(SessionMsg::Disconnect) => {
								farewell(&mut stream).await;
								break;
							}
							// The command loop dropped the link without a Disconnect — the tab was dropped, or the
							// worker went away. Nothing was typed, so there is nothing to wait for and no one left
							// to tell: straight to the kill.
							None => break,
						}
					}
				}
	}

	// Every ending but a failed start arrives here: the shell exited on its own, or it was killed
	// above. Either way the session is over and the GUI is told once.
	pty.close();
	let _ = events.send(SshEvent::Disconnected).await;
}

/// Wait up to [`GOODBYE`] for the shell to exit on its own, and say whether it did (§104).
///
/// The output is DRAINED and dropped rather than forwarded. By the time this runs the GUI has already
/// dropped its emulator and gone home, so a goodbye line has nowhere to be drawn — but the bytes still
/// have to be taken, because the channel between the reader thread and here is bounded: a shell whose
/// last words filled it would block writing them and never reach its own exit, and the wait would then
/// time out on a shell that was trying to leave.
///
/// The return value is for the log only. Both outcomes are correct — one is the shell leaving, the other
/// is the kill that follows doing its job — so nothing branches on it.
async fn farewell(stream: &mut super::pty::Stream) -> bool {
	let left = tokio::time::timeout(GOODBYE, async {
		loop {
			tokio::select! {
				biased;
				// Drained, not forwarded. `None` is the stream ending, which means the pty is already gone.
				chunk = stream.bytes.recv() => {
					if chunk.is_none() { break }
				}
				// The shell exited on its own, which is the whole point of waiting.
				_ = stream.exited.recv() => break,
			}
		}
	})
	.await
	.is_ok();
	if !left {
		// Not an error — a shell with a program running in front of it cannot act on a typed word. Logged
		// because it is the difference between a shell that ended itself and one cmote terminated, and
		// that difference is invisible from the GUI.
		eprintln!("the local shell did not leave on its own; ending it the hard way");
	}
	left
}

/// A fresh cancel flag for a transfer about to start, keeping a clone for the ✕ to raise (§16).
///
/// One transfer at a time, so each start simply replaces the previous flag; the copy loop polling an
/// old one is a loop that has already finished.
fn arm(cancel: &mut Option<Arc<AtomicBool>>) -> Arc<AtomicBool> {
	let flag = Arc::new(AtomicBool::new(false));
	*cancel = Some(flag.clone());
	flag
}

#[cfg(test)]
mod tests {
	use super::{SessionMsg, SshEvent, arm, mpsc, run};
	use crate::local::shells;
	use std::sync::atomic::Ordering;

	/// Drive a real local session: start the first shell the Local bar would offer, send it `commands`,
	/// and hand back every event it produced until it was disconnected.
	///
	/// This is the only test that runs the whole pipe — a real pty, the real select loop, the real path
	/// translation and the real `std::fs` — so it is the one that would catch a file arm wired to the
	/// wrong function or a pane path that never reaches the disk. `None` on a machine with no shell to
	/// start, which is how the assertions below step aside rather than failing on a bare box.
	async fn drive(commands: Vec<SessionMsg>) -> Option<Vec<SshEvent>> {
		/// How long to wait for the session to go quiet before ending it. Every file operation runs on
		/// its own detached task, so `Disconnect` sent straight after the commands would break the loop
		/// while the answers were still being computed — and they would then arrive AFTER `Disconnected`,
		/// which is exactly what happened the first time this was written. That ordering is fine in the
		/// app (the tab has gone home and drops them, §19) and useless in a test.
		const QUIET: std::time::Duration = std::time::Duration::from_secs(2);

		let shell = shells::catalogue().first()?.clone();
		let (event_tx, mut event_rx) = mpsc::channel::<SshEvent>(256);
		let (command_tx, command_rx) = mpsc::channel::<SessionMsg>(64);
		let session = tokio::spawn(run(shell, event_tx, command_rx));

		for command in commands {
			command_tx.send(command).await.expect("the session is live");
		}

		let mut events = Vec::new();
		while let Ok(Some(event)) = tokio::time::timeout(QUIET, event_rx.recv()).await {
			events.push(event);
		}
		let _ = command_tx.send(SessionMsg::Disconnect).await;
		while let Ok(Some(event)) = tokio::time::timeout(QUIET, event_rx.recv()).await {
			let done = matches!(event, SshEvent::Disconnected);
			events.push(event);
			if done {
				break;
			}
		}
		let _ = session.await;
		Some(events)
	}

	#[tokio::test]
	async fn a_session_opens_answers_a_listing_and_ends() {
		// The whole shape of §103 in one test: no handshake (`Connected` arrives first and immediately),
		// the machine's own timezone volunteered up front so the first listing's times are already right,
		// a real directory listed through the real path translation, and a clean `Disconnected`.
		let home = crate::local::path::home();
		let Some(events) = drive(vec![
			SessionMsg::ListDir(home.clone()),
			SessionMsg::ListFiles {
				path: home.clone(),
				request: 7,
			},
		])
		.await
		else {
			eprintln!("skipped the session round trip: this machine offers no local shell");
			return;
		};

		assert!(
			matches!(events.first(), Some(SshEvent::Connected)),
			"a local session opens with no question to answer: {events:?}"
		);
		assert!(
			events
				.iter()
				.any(|event| matches!(event, SshEvent::Zone(_))),
			"the machine's zone is volunteered before the first listing: {events:?}"
		);
		assert!(
			events
				.iter()
				.any(|event| matches!(event, SshEvent::DirListed { path, .. } if *path == home)),
			"the tree's listing came back for the folder it asked about: {events:?}"
		);
		// The pane's listing arrives in batches keyed by the request number, exactly as SFTP's does —
		// that is what lets the pane drop an answer for a directory it has already left (§19).
		assert!(
			events.iter().any(|event| matches!(
				event,
				SshEvent::FilesChunk {
					request: 7,
					done: true,
					..
				}
			)),
			"the pane's listing finished, and under its own request number: {events:?}"
		);
		assert!(
			matches!(events.last(), Some(SshEvent::Disconnected)),
			"and the session ended once: {events:?}"
		);
	}

	#[tokio::test]
	async fn a_pane_path_that_is_not_on_this_machine_fails_the_listing_rather_than_the_session() {
		// `/etc` is not a place on Windows. The refusal happens in `local::path` before anything touches
		// the disk, and it has to come back as that listing failing — not as the session dying, and not
		// as an empty folder, which would read as "this directory is empty".
		let Some(events) = drive(vec![SessionMsg::ListFiles {
			path: "/nowhere-on-this-machine".to_owned(),
			request: 3,
		}])
		.await
		else {
			eprintln!("skipped: this machine offers no local shell");
			return;
		};

		if cfg!(windows) {
			assert!(
				events.iter().any(|event| matches!(
					event,
					SshEvent::FilesFailed { request: 3, reason } if reason.contains("not a path on this machine")
				)),
				"the refusal names the path and the reason: {events:?}"
			);
		}
		assert!(
			matches!(events.last(), Some(SshEvent::Disconnected)),
			"and the session survived it: {events:?}"
		);
	}

	#[test]
	fn each_transfer_starts_with_its_own_unset_flag() {
		// A flag left raised by the previous transfer would cancel the next one the instant it began.
		let mut held = None;
		let first = arm(&mut held);
		first.store(true, Ordering::Relaxed);
		let second = arm(&mut held);
		assert!(!second.load(Ordering::Relaxed));
		// And the ✕ reaches the CURRENT transfer, which is the one the kept clone points at.
		held.as_ref()
			.expect("a flag is kept")
			.store(true, Ordering::Relaxed);
		assert!(second.load(Ordering::Relaxed));
		assert!(
			first.load(Ordering::Relaxed),
			"the old flag is simply unread"
		);
	}
}

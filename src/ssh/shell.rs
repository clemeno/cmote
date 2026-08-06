// ssh/shell.rs — the shells running on one connection, and the conversation that opens a new one
// as another account (PLAN §45).
//
// A session used to be one channel: `stream()` held it, awaited it, wrote to it. Elevation breaks
// that one-to-one, because becoming root is a PROGRAM run on the remote (`sudo -u root -i`), and a
// program needs a channel of its own. So a session now holds a SET of shells — the one it
// authenticated as (`bridge::LOGIN_IDENTITY`, always there) plus one per account elevated into —
// and this module owns that set.
//
// Two problems come with the set, and the shape here is the answer to both:
//
//   * **Awaiting many channels at once.** `Channel::wait` needs `&mut`, so awaiting N of them in
//     one `select!` would mean holding N mutable borrows across an await while the same loop also
//     wants to WRITE to them. Instead every channel is `split()`: the reading half moves into a
//     task of its own that forwards each message down one shared mpsc, tagged with its identity,
//     and the writing half stays here. The session loop then awaits a single receiver, and writing
//     borrows nothing that reading holds.
//
//   * **Output that is not terminal output.** While `sudo` is still asking its questions, the
//     channel's bytes are a credential conversation, not something to draw in a grid. A shell is
//     therefore `Elevating` before it is `Live`, and only a `Live` shell's bytes become
//     `SshEvent::Output`. This is also what makes the feature safe: the elevating channel runs
//     `sudo` and NOTHING else, so a password written to it cannot reach a shell, a running command
//     or the user's history — the classic hazard of typing `sudo` into an existing prompt.

use std::collections::HashMap;

use anyhow::{Context, Result};
use russh::client;
use russh::{Channel, ChannelMsg, ChannelWriteHalf};
use tokio::sync::mpsc;

use crate::bridge::{LOGIN_IDENTITY, SshEvent};
use crate::elevate;
use crate::secret::Secret;
use crate::term;

/// How many messages may queue between the per-channel reader tasks and the session loop. Generous
/// like the GUI channel's bound, and bounded for the same reason: a shell dumping a large file has
/// to wait for the loop to keep up rather than grow memory without limit (§4).
const CHANNEL_BOUND_SHELL: usize = 256;

/// How many questions cmote will answer for one elevation before giving up (§45). sudo itself
/// stops after three wrong passwords; this only guards against a program that would ask for ever,
/// so it is generously above any real conversation (a password plus a second factor, retried).
const MAX_PROMPTS: u32 = 8;

/// One message read off one shell's channel, tagged with the shell it came from. The reader tasks
/// all send these down the same channel, so the session loop awaits one receiver however many
/// shells are open.
pub struct ShellMsg {
	pub identity: u64,
	pub msg: ChannelMsg,
}

/// Where a shell is in its life (§45).
enum State {
	/// The elevation program is still talking: its output is a conversation to answer, not
	/// terminal output to draw. `buffer` is what it has said since the last question was answered
	/// — cleared at each question, so a stale prompt is never mistaken for a fresh one — `asked`
	/// counts the questions put to the user, and `pending` is set while one is unanswered so the
	/// same prompt cannot raise two dialogs.
	///
	/// `password` records whether the outstanding question is the one cmote NAMED itself (`-p
	/// MARKER`), which is the only question whose answer may be kept: it is the caller's own
	/// password, and the file layer needs the same one to authenticate sudo on a file channel
	/// (§46). Anything else — a second factor, `su`'s prompt for another account's password — is
	/// answered and forgotten.
	Elevating {
		buffer: String,
		asked: u32,
		pending: bool,
		password: bool,
	},
	/// A live terminal: bytes are output, keystrokes may be routed here.
	Live,
}

/// What one shell message meant for the session as a whole (§45, §46) — the session loop acts on
/// this rather than reading the shells' internals.
pub enum After {
	/// It concerned that shell alone; carry on.
	Nothing,
	/// That identity's shell has gone — the user typed `exit`, or the elevation was refused. Its
	/// file access goes with it, which is what closes the elevated `sftp-server` running for it.
	Ended(u64),
	/// The LOGIN shell closed, which is the session itself ending.
	SessionOver,
}

/// One shell: the writing half of its channel, and what it is currently doing.
struct Shell {
	write: ChannelWriteHalf<client::Msg>,
	state: State,
}

/// Every shell on one connection (§45), and which one typing belongs to.
pub struct Shells {
	shells: HashMap<u64, Shell>,
	/// The identity the GUI has on screen, told to us by `SelectIdentity`. `Input` goes here and
	/// nowhere else — a keystroke must never be broadcast, or one line typed at a root prompt
	/// would also run in the login shell.
	selected: u64,
	/// The pty size every shell is currently at. Kept because a shell opened LATER must start at
	/// the size the window is NOW, not at the emulator's defaults — otherwise a full-screen program
	/// in a freshly elevated shell would lay itself out for a window that has since been resized.
	size: (u16, u16),
	/// Cloned into each reader task so it can forward what it reads.
	to_loop: mpsc::Sender<ShellMsg>,
}

impl Shells {
	/// Take over the session's login shell and start reading it. Returns the set plus the receiver
	/// the session loop awaits for every shell's messages.
	pub fn new(login: Channel<client::Msg>) -> (Self, mpsc::Receiver<ShellMsg>) {
		let (to_loop, from_shells) = mpsc::channel::<ShellMsg>(CHANNEL_BOUND_SHELL);
		let mut shells = Self {
			shells: HashMap::new(),
			selected: LOGIN_IDENTITY,
			size: (term::DEFAULT_COLS, term::DEFAULT_ROWS),
			to_loop,
		};
		// The login shell is `Live` from the start: it was opened by `request_shell` after the SSH
		// authentication succeeded, so there is no conversation to hold with it.
		shells.adopt(LOGIN_IDENTITY, login, State::Live);
		(shells, from_shells)
	}

	/// Split `channel`, keep its writing half under `identity`, and move its reading half into a
	/// task that forwards everything it says down the shared channel.
	fn adopt(&mut self, identity: u64, channel: Channel<client::Msg>, state: State) {
		let (mut read, write) = channel.split();
		let to_loop = self.to_loop.clone();
		tokio::spawn(async move {
			while let Some(msg) = read.wait().await {
				if to_loop.send(ShellMsg { identity, msg }).await.is_err() {
					// The session loop has gone; nothing left to forward to.
					return;
				}
			}
			// `wait` returning `None` means the channel is fully closed, and that is not itself a
			// `ChannelMsg` — so say it as one, or a shell that vanished without an explicit close
			// would sit in the map for ever.
			let _ = to_loop
				.send(ShellMsg {
					identity,
					msg: ChannelMsg::Close,
				})
				.await;
		});
		self.shells.insert(identity, Shell { write, state });
	}

	/// Open another shell on this connection running `command`, to become another account (§45).
	///
	/// A pty is requested for it exactly as for the login shell: `sudo` and `su` both refuse to
	/// read a password from anything else, and the shell that replaces them needs one anyway. It
	/// starts at the CURRENT window size, so it is laid out for the window as it is.
	///
	/// A failure to even open the channel is reported as the identity ending, so the GUI never
	/// leaves a half-built account in its switcher.
	pub async fn elevate(
		&mut self,
		session: &client::Handle<super::client::Handler>,
		events: &mpsc::Sender<SshEvent>,
		identity: u64,
		command: String,
	) {
		match self.open_elevated(session, identity, command).await {
			Ok(()) => {}
			Err(error) => {
				// Detail to the log, a generic line to the user (§12): a channel that would not
				// open says nothing about the remote's sudo policy, so there is nothing here worth
				// quoting to them.
				eprintln!("could not open an elevated shell: {error:#}");
				let _ = events
					.send(SshEvent::IdentityEnded {
						identity,
						reason: Some("The remote refused a second shell.".to_owned()),
					})
					.await;
			}
		}
	}

	/// The opening itself, split out so `elevate` can turn any failure into one event.
	async fn open_elevated(
		&mut self,
		session: &client::Handle<super::client::Handler>,
		identity: u64,
		command: String,
	) -> Result<()> {
		let channel = session
			.channel_open_session()
			.await
			.context("could not open a channel for the elevated shell")?;
		let (cols, rows) = self.size;
		channel
			.request_pty(
				false,
				"xterm-256color",
				u32::from(cols),
				u32::from(rows),
				0,
				0,
				&[],
			)
			.await
			.context("the server refused a pty for the elevated shell")?;
		// `exec`, not `request_shell`: the channel runs the elevation program and nothing else,
		// which is what confines the credential conversation to a process that expects one.
		channel
			.exec(true, command.as_bytes())
			.await
			.context("the server refused to run the elevation command")?;
		self.adopt(
			identity,
			channel,
			State::Elevating {
				buffer: String::new(),
				asked: 0,
				pending: false,
				password: false,
			},
		);
		Ok(())
	}

	/// Handle one message from one shell, and say what it meant for the session as a whole.
	pub async fn on_msg(&mut self, msg: ShellMsg, events: &mpsc::Sender<SshEvent>) -> After {
		let ShellMsg { identity, msg } = msg;
		match msg {
			// Both streams are the shell talking: a pty merges them anyway, and cmote renders
			// stderr inline, so they are handled identically.
			ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
				self.on_data(identity, data.to_vec(), events).await;
				After::Nothing
			}
			// The channel is finished. For the login shell that ends the session; for an elevated
			// one it ends that identity — either because the user typed `exit`, or because the
			// elevation was refused, which is the difference the reason carries.
			ChannelMsg::Eof | ChannelMsg::Close | ChannelMsg::ExitStatus { .. } => {
				if identity == LOGIN_IDENTITY {
					return After::SessionOver;
				}
				// A channel reports its end more than once (an exit status, then a close), so the
				// removal is what makes this idempotent: the second message finds no shell and
				// reports nothing.
				if let Some(shell) = self.shells.remove(&identity) {
					let reason = match shell.state {
						// It died mid-conversation: the last thing it said is why, and those are
						// the remote's own words about its own policy.
						State::Elevating { buffer, .. } => Some(
							elevate::reason(&buffer)
								.unwrap_or_else(|| "The elevation was refused.".to_owned()),
						),
						// It had become a shell and that shell exited — an ordinary `exit`.
						State::Live => None,
					};
					let _ = events
						.send(SshEvent::IdentityEnded { identity, reason })
						.await;
				}
				// Said even for a repeat message: `Accounts::remove` is idempotent, and an account
				// whose file access outlived its shell would keep an elevated `sftp-server` running on
				// the remote (§46).
				After::Ended(identity)
			}
			_ => After::Nothing,
		}
	}

	/// Bytes arrived on `identity`'s channel: draw them, or read them as the elevation's next line.
	async fn on_data(&mut self, identity: u64, bytes: Vec<u8>, events: &mpsc::Sender<SshEvent>) {
		let Some(shell) = self.shells.get_mut(&identity) else {
			return; // a late chunk for a shell already gone
		};
		match &mut shell.state {
			// A live shell's bytes are terminal output, tagged so the GUI feeds the right grid.
			State::Live => {
				let _ = events.send(SshEvent::Output { identity, bytes }).await;
			}
			State::Elevating {
				buffer,
				asked,
				pending,
				password,
			} => {
				// Lossy on purpose: a chunk can end mid-UTF-8, and this text is only ever compared
				// against prompt shapes — the terminal, which does care, never sees it.
				buffer.push_str(&String::from_utf8_lossy(&bytes));

				// A shell prompt means the program is gone and the account's own shell has the
				// channel: the conversation is over, whatever else is in the buffer. Everything
				// buffered since the last answer is flushed as output, so the shell's greeting and
				// its first prompt appear in the grid rather than being swallowed.
				if elevate::looks_like_shell(buffer) {
					let flush = std::mem::take(buffer).into_bytes();
					shell.state = State::Live;
					let _ = events
						.send(SshEvent::Output {
							identity,
							bytes: flush,
						})
						.await;
					let _ = events.send(SshEvent::IdentityReady { identity }).await;
					return;
				}

				// Otherwise: is it asking something? Only while no question is already outstanding
				// — the same buffer arriving in two chunks must not raise two dialogs — and only up
				// to a bound, so a program that asks for ever cannot pin the user in a dialog loop.
				if *pending || *asked >= MAX_PROMPTS {
					return;
				}
				if let Some(label) = elevate::prompt(buffer) {
					// Whether this is cmote's OWN password question, decided before the buffer is
					// cleared: the answer to that one is the caller password the file layer will need
					// (§46), and to any other question it is a secret to use once and forget.
					*password = buffer.contains(elevate::MARKER);
					*pending = true;
					*asked += 1;
					// Cleared now: the question has been put, so these bytes are spent. What
					// arrives next is either the answer's outcome or the next question, and
					// neither should be read against text that has already been dealt with.
					buffer.clear();
					let _ = events
						.send(SshEvent::ElevatePrompt { identity, label })
						.await;
				}
			}
		}
	}

	/// Write one answer to an elevating shell's channel (§45), newline included — a program reading
	/// a password reads a LINE, and waits for ever without the terminator.
	///
	/// Ignored unless that shell is actually mid-conversation. That guard is the whole safety
	/// property: a secret can only ever be written to a channel that is running `sudo` and has just
	/// asked for one, never to a live shell.
	///
	/// Answers `true` when what was just answered is the password cmote itself asked for by name —
	/// the caller's own, which the file layer needs to authenticate sudo on a file channel (§46). Any
	/// other question answers `false`, so a one-time code is used once and never kept.
	pub async fn answer(&mut self, identity: u64, secret: Secret) -> bool {
		let Some(shell) = self.shells.get_mut(&identity) else {
			return false;
		};
		let State::Elevating {
			buffer,
			pending,
			password,
			..
		} = &mut shell.state
		else {
			return false;
		};
		if !*pending {
			return false;
		}
		*pending = false;
		let was_password = *password;
		buffer.clear();
		let mut line = secret.expose().as_bytes().to_vec();
		line.push(b'\n');
		let _ = shell.write.data_bytes(line).await;
		was_password
	}

	/// Send keystrokes to the shell the user is looking at (§45).
	///
	/// A shell still elevating gets nothing: its channel is a credential conversation, and a
	/// keystroke arriving in the middle of one would be read as part of the answer.
	pub async fn input(&self, bytes: Vec<u8>) {
		if let Some(shell) = self.shells.get(&self.selected)
			&& matches!(shell.state, State::Live)
		{
			let _ = shell.write.data_bytes(bytes).await;
		}
	}

	/// Write the emulator's answer to a query to ONE named shell (§23, §45), selected or not: the
	/// program that asked is blocked reading its stdin until it arrives, and it is waiting on its own
	/// channel, not on whichever one the user is looking at.
	///
	/// A shell still elevating gets nothing, for the same reason as `input`: its channel is a
	/// credential conversation, and it has no emulator to have asked anything.
	pub async fn reply(&self, identity: u64, bytes: Vec<u8>) {
		if let Some(shell) = self.shells.get(&identity)
			&& matches!(shell.state, State::Live)
		{
			let _ = shell.write.data_bytes(bytes).await;
		}
	}

	/// Note which identity the GUI has on screen. Sent ahead of the input for it, on the same
	/// ordered channel, so the two can never cross.
	pub fn select(&mut self, identity: u64) {
		self.selected = identity;
	}

	/// Reflow EVERY shell's pty (§45). They share one window, so they share one size — including
	/// the ones off screen, which would otherwise be laid out for a window that no longer exists
	/// the moment the user switched to them.
	pub async fn resize(&mut self, cols: u16, rows: u16) {
		self.size = (cols, rows);
		for shell in self.shells.values() {
			let _ = shell
				.write
				.window_change(u32::from(cols), u32::from(rows), 0, 0)
				.await;
		}
	}

	/// Close one elevated identity (§45): EOF on its channel ends the login shell running there,
	/// and the reader task then reports it gone the same way an `exit` would.
	///
	/// The login identity is refused — the session's own shell goes down with the session, through
	/// `Disconnect`, and closing it here would leave a connection with no terminal at all.
	pub async fn close(&mut self, identity: u64) {
		if identity == LOGIN_IDENTITY {
			return;
		}
		if let Some(shell) = self.shells.get(&identity) {
			let _ = shell.write.eof().await;
		}
	}

	/// End every shell, for a session that is shutting down.
	pub async fn eof_all(&self) {
		for shell in self.shells.values() {
			let _ = shell.write.eof().await;
		}
	}
}

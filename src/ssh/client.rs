// ssh/client.rs — the russh client Handler and the tokio task loop (PLAN §6).
//
// Structure (all on the dedicated tokio thread, §4):
//   run()          — drains SshCommands from the GUI, owns the current session's
//                    channels, and routes input/resize/decisions to it.
//   session_task() — one connection's whole life: connect -> host-key gate (§8)
//                    -> password auth (§7) -> pty + shell -> byte stream.
//   Handler        — russh calls `check_server_key` during the handshake; that
//                    is our TOFU gate. Unknown keys are surfaced to the GUI and
//                    the user's decision awaited; a changed key is refused.
//
// Why a *spawned* session task instead of running the connect inline: the
// host-key gate must pause mid-handshake and wait for the user to click
// Accept/Reject. That answer arrives as another SshCommand — so `run()` has to
// stay free to receive it. Spawning the session keeps the command loop live.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use russh::Channel;
use russh::client;
use russh::keys::PublicKey;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::bridge::{ConflictChoice, ConnectParams, HostKeyChoice, SshCommand, SshEvent};
use crate::secret::Secret;
use crate::ssh::asuser;
use crate::ssh::auth;
use crate::ssh::browse;
use crate::ssh::download;
use crate::ssh::edit;
use crate::ssh::forward;
use crate::ssh::hostkey::{self, HostKeyVerdict};
use crate::ssh::shell;
use crate::ssh::upload;
use crate::term;

/// How long to wait for the TCP connect + SSH handshake before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many accepted-but-not-yet-opened forward connections may queue between the listeners and
/// the session loop before a listener awaits (§27). Generous — a burst of tunnel connections is
/// drained as fast as channels open — and bounded so it cannot grow without limit.
const CHANNEL_BOUND_FORWARD: usize = 64;

/// The SSH task loop. Owns the channels to the one live session (v1 is single-
/// session) and routes commands to it. Returns when the GUI drops its command
/// sender (app exit).
pub async fn run(mut commands: mpsc::Receiver<SshCommand>, events: mpsc::Sender<SshEvent>) {
	let mut session: Option<SessionLink> = None;

	while let Some(command) = commands.recv().await {
		match command {
			SshCommand::Connect(params) => {
				// Starting a new session drops any previous link; the old
				// session sees its command channel close and winds down.
				session = Some(SessionLink::start(params, events.clone()));
			}
			SshCommand::HostKeyResponse(choice) => {
				if let Some(link) = session.as_mut() {
					link.send_decision(choice);
				}
			}
			SshCommand::Passphrase(secret) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::Passphrase(secret)).await;
				}
			}
			SshCommand::Interactive(answers) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::Interactive(answers)).await;
				}
			}
			SshCommand::Input(bytes) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::Data(bytes)).await;
				}
			}
			SshCommand::Resize { cols, rows } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Resize { cols, rows })
						.await;
				}
			}
			// The elevation commands (§45), all forwarded the same way as the rest: the session
			// task owns its shells, so this loop only has to reach the right session.
			SshCommand::Elevate {
				identity,
				kind,
				user,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Elevate {
							identity,
							kind,
							user,
						})
						.await;
				}
			}
			SshCommand::ElevateAnswer { identity, secret } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::ElevateAnswer { identity, secret })
						.await;
				}
			}
			SshCommand::SelectIdentity(identity) => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::SelectIdentity(identity))
						.await;
				}
			}
			SshCommand::Reply { identity, bytes } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Reply { identity, bytes })
						.await;
				}
			}
			SshCommand::CloseIdentity(identity) => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::CloseIdentity(identity))
						.await;
				}
			}
			SshCommand::Upload {
				local,
				remote,
				overwrite,
				resume,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Upload {
							local,
							remote,
							overwrite,
							resume,
						})
						.await;
				}
			}
			SshCommand::CheckUploads { dir, names } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::CheckUploads { dir, names })
						.await;
				}
			}
			SshCommand::ListDir(path) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::ListDir(path)).await;
				}
			}
			SshCommand::ListFiles { path, request } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::ListFiles { path, request })
						.await;
				}
			}
			SshCommand::ReadLink(path) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::ReadLink(path)).await;
				}
			}
			SshCommand::Download {
				remote,
				local,
				resume,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Download {
							remote,
							local,
							resume,
						})
						.await;
				}
			}
			SshCommand::UploadTree {
				local,
				remote,
				resume,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::UploadTree {
							local,
							remote,
							resume,
						})
						.await;
				}
			}
			SshCommand::DownloadTree {
				remote,
				local,
				resume,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::DownloadTree {
							remote,
							local,
							resume,
						})
						.await;
				}
			}
			SshCommand::EditLoad {
				identity,
				editor_id,
				path,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::EditLoad {
							identity,
							editor_id,
							path,
						})
						.await;
				}
			}
			SshCommand::EditSave {
				identity,
				editor_id,
				path,
				bytes,
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::EditSave {
							identity,
							editor_id,
							path,
							bytes,
						})
						.await;
				}
			}
			SshCommand::ResolveConflict(choice) => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::ResolveConflict(choice))
						.await;
				}
			}
			SshCommand::CancelTransfer => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::CancelTransfer).await;
				}
			}
			SshCommand::MakeDir(path) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::MakeDir(path)).await;
				}
			}
			SshCommand::Delete(paths) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::Delete(paths)).await;
				}
			}
			SshCommand::RenameDir { from, to } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::RenameDir { from, to })
						.await;
				}
			}
			SshCommand::AddForward { id, spec } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::AddForward { id, spec })
						.await;
				}
			}
			SshCommand::RemoveForward(id) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::RemoveForward(id)).await;
				}
			}
			SshCommand::Disconnect => {
				if let Some(link) = session.take() {
					let _ = link.to_session.send(SessionMsg::Disconnect).await;
				}
			}
		}
	}
}

/// Messages `run()` forwards to a live session task. `pub(crate)` because the auth module
/// (`ssh::auth`) receives passphrase and keyboard-interactive answers off this same channel.
pub(crate) enum SessionMsg {
	/// Keyboard bytes to write to the shell.
	Data(Vec<u8>),
	/// Terminal resized; reflow every shell's remote pty (§45).
	Resize { cols: u16, rows: u16 },
	/// Open another shell on this connection, running `command` to become another account (§45).
	Elevate {
		identity: u64,
		kind: crate::elevate::Kind,
		user: String,
	},
	/// One answer to an elevating shell's question (§45), written to its channel.
	ElevateAnswer { identity: u64, secret: Secret },
	/// Which shell typing belongs to from now on (§45).
	SelectIdentity(u64),
	/// A query reply for one named shell, whether or not it is the selected one (§23, §45).
	Reply { identity: u64, bytes: Vec<u8> },
	/// End one elevated shell (§45); the login shell is not closable this way.
	CloseIdentity(u64),
	/// A passphrase the user typed to unlock an encrypted key (§7).
	Passphrase(Secret),
	/// The user's answers to a keyboard-interactive request (§7), one per prompt in order.
	Interactive(Vec<Secret>),
	/// Send a local file to the remote over a second, sftp channel (§17). `resume` appends from
	/// the destination's current size rather than truncating (§16).
	Upload {
		local: PathBuf,
		remote: String,
		overwrite: bool,
		resume: bool,
	},
	/// Check which of an upload batch's names already exist before it sends (§17).
	CheckUploads { dir: String, names: Vec<String> },
	/// List the folders inside a remote directory, for the explorer tree (§18).
	ListDir(String),
	/// List every entry inside a remote directory, for the files pane (§19).
	ListFiles { path: String, request: u64 },
	/// Fetch a remote file to a local path (§19). `resume` appends from the local partial's size
	/// rather than truncating (§16).
	Download {
		remote: String,
		local: PathBuf,
		resume: bool,
	},
	/// Send a whole local folder to the remote, recreating its tree (§17). `resume` size-compares
	/// each file and sends only the missing tail, with no collision prompts (§16).
	UploadTree {
		local: PathBuf,
		remote: String,
		resume: bool,
	},
	/// Fetch a whole remote folder to this machine, recreating its tree (§19). `resume` as above.
	DownloadTree {
		remote: String,
		local: PathBuf,
		resume: bool,
	},
	/// Read a whole remote file into the in-tab text editor (§32), as the account named by
	/// `identity` (§46). `editor_id` routes the reply.
	EditLoad {
		identity: u64,
		editor_id: u64,
		path: String,
	},
	/// Write the editor's buffer back to the remote, atomically (§32), as the account that opened it.
	EditSave {
		identity: u64,
		editor_id: u64,
		path: String,
		bytes: Vec<u8>,
	},
	/// The user's answer to a recursive transfer's file-collision prompt (§17, §19), forwarded
	/// to the transfer waiting on it.
	ResolveConflict(ConflictChoice),
	/// Stop the running transfer (§16): set the flag its copy loop polls, so it deletes its
	/// partial and stops. Routed to the flag `stream` keeps for the transfer currently in flight.
	CancelTransfer,
	/// Create a new remote folder (§18).
	MakeDir(String),
	/// Delete remote entries, folders and their contents included (§18).
	Delete(Vec<String>),
	/// Resolve one symlink for the files pane's details popup (§20).
	ReadLink(String),
	/// Rename a remote folder (§18).
	RenameDir { from: String, to: String },
	/// Start a port forward on the live connection (§27).
	AddForward {
		id: u64,
		spec: crate::forward::ForwardSpec,
	},
	/// Tear a port forward down (§27).
	RemoveForward(u64),
	/// Tear the session down.
	Disconnect,
}

/// `run()`'s handle to a spawned session task: a channel for input/resize/quit
/// and a one-shot for the host-key decision (used at most once).
struct SessionLink {
	to_session: mpsc::Sender<SessionMsg>,
	decision: Option<oneshot::Sender<HostKeyChoice>>,
}

impl SessionLink {
	/// Spawn a session task for `params` and return the handle to talk to it.
	fn start(params: ConnectParams, events: mpsc::Sender<SshEvent>) -> Self {
		let (to_session_tx, to_session_rx) = mpsc::channel::<SessionMsg>(256);
		let (decision_tx, decision_rx) = oneshot::channel::<HostKeyChoice>();

		tokio::spawn(session_task(params, events, to_session_rx, decision_rx));

		Self {
			to_session: to_session_tx,
			decision: Some(decision_tx),
		}
	}

	/// Deliver the user's host-key decision to the waiting handshake (§8). Consumes
	/// the one-shot; further calls are no-ops.
	fn send_decision(&mut self, choice: HostKeyChoice) {
		if let Some(tx) = self.decision.take() {
			let _ = tx.send(choice);
		}
	}
}

/// One connection's whole life. Translates the outcome into a final event and
/// keeps all error detail out of the message shown to the user (§12).
async fn session_task(
	params: ConnectParams,
	events: mpsc::Sender<SshEvent>,
	to_session_rx: mpsc::Receiver<SessionMsg>,
	decision_rx: oneshot::Receiver<HostKeyChoice>,
) {
	let _ = events.send(SshEvent::Connecting).await;

	match connect_and_run(params, &events, to_session_rx, decision_rx).await {
		Ok(()) => {
			let _ = events.send(SshEvent::Disconnected).await;
		}
		Err(error) => {
			// Log detail server-side; show the user a generic message.
			eprintln!("ssh session error: {error:#}");
			let _ = events
				.send(SshEvent::Error(
					"Could not establish the SSH session.".to_string(),
				))
				.await;
		}
	}
}

/// Connect, gate the host key, authenticate, open a shell, and pump bytes until
/// the session ends.
async fn connect_and_run(
	params: ConnectParams,
	events: &mpsc::Sender<SshEvent>,
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
	decision_rx: oneshot::Receiver<HostKeyChoice>,
) -> Result<()> {
	let config = Arc::new(client::Config {
		// No inactivity timeout: an interactive shell may sit idle for a long
		// time and must not be dropped for being quiet.
		inactivity_timeout: None,
		..Default::default()
	});

	// The table remote forwards share between the Handler (which receives the server's
	// forwarded-tcpip channels) and the session loop (which adds/removes them, §27). One per
	// session, cloned into both.
	let remote_forwards = forward::remote_table();

	let handler = Handler {
		host: params.host.clone(),
		port: params.port,
		known_hosts: hostkey::known_hosts_path()?,
		events: events.clone(),
		decision: Some(decision_rx),
		remote_forwards: remote_forwards.clone(),
	};

	// TCP connect + SSH handshake, bounded by a timeout. The handshake runs the
	// host-key gate (Handler::check_server_key) before returning.
	let mut session = timeout(
		CONNECT_TIMEOUT,
		client::connect(config, (params.host.as_str(), params.port), handler),
	)
	.await
	.context("connection timed out")?
	.context("could not connect")?;

	// Authenticate: the chosen method first, then chaining into keyboard-interactive as the
	// server directs — 2FA / OTP and challenge-response (§7). A failure is a single generic
	// error, with no hint about which factor was wrong (no credential oracle, §12).
	auth::authenticate(&mut session, &params, events, &mut to_session_rx).await?;

	let _ = events.send(SshEvent::Connected).await;

	// Open a shell channel with a pty so interactive programs render correctly.
	let channel = session.channel_open_session().await?;
	// Match the pty to the emulator's initial grid (§9): the single source of
	// truth lives in `term`, so the remote pty and our local view agree.
	channel
		.request_pty(
			false,
			"xterm-256color",
			u32::from(term::DEFAULT_COLS),
			u32::from(term::DEFAULT_ROWS),
			0,
			0,
			&[],
		)
		.await?;
	channel.request_shell(true).await?;

	// cmote types nothing into the shell. SSH tells us nothing about where the shell is, so cmote
	// learns the working directory from the OSC the shell emits on each prompt (`term::cwd`): a
	// shell that announces its cwd (fish, a Windows OSC 9;9 shell) is followed, and a silent
	// bash/zsh simply leaves the cwd unknown — the remote's shell history is left untouched (§17).
	stream(channel, &session, events, to_session_rx, remote_forwards).await
}

/// The bidirectional pump: server output -> GUI, GUI input/resize -> server.
/// Runs until either side closes. `session` is borrowed alongside the shell channel so
/// an upload can open its own sftp channel on the same connection (§17).
///
/// The shell is no longer a single channel: `shell::Shells` holds every shell the session has —
/// the login one and any account elevated into (§45) — and hands this loop one receiver for all of
/// them, so the `select!` below stays the same shape however many are open.
async fn stream(
	channel: Channel<client::Msg>,
	session: &client::Handle<Handler>,
	events: &mpsc::Sender<SshEvent>,
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
	remote_forwards: forward::RemoteTable,
) -> Result<()> {
	// The session's shells (§45), starting with the login one this loop was handed.
	let (mut shells, mut from_shells) = shell::Shells::new(channel);
	// The accounts this session can read FILES as (§46): the login one, plus each account elevated
	// into. It owns what used to be a single `browse::Sftp` here — one sftp session per account,
	// opened on that account's first listing and kept, since a tree asks many small questions (§18).
	//
	// `channels` is the other half of the same feature: a shell-backend operation runs one command
	// per channel and cannot open one itself (russh's handle is not `Clone` and lives here), so it
	// asks this loop, which serves the request in the arm below.
	let (channels, mut channel_requests) = asuser::Channels::new();
	let mut accounts = asuser::Accounts::new(channels);

	// The port forwards on this session (§27). A local/dynamic listener cannot open its own SSH
	// channel (the session `Handle` is not `Sync`), so it hands each accepted socket back here on
	// `accepted_rx`; the select loop opens the `direct-tcpip` channel and spawns a detached pump.
	// Dropping `forwards` at the end of the loop aborts every local listener.
	let (accepted_tx, mut accepted_rx) = mpsc::channel::<forward::Accepted>(CHANNEL_BOUND_FORWARD);
	let mut forwards = forward::Forwards::new(remote_forwards, accepted_tx);

	// The reply channel for the transfer currently running, if it is a recursive one (§17, §19).
	// A tree transfer parks mid-way to ask about a file collision; its answer arrives as a
	// `ResolveConflict` and is forwarded here. Held across the whole stream: only one transfer
	// runs at a time, so starting a new one simply replaces this, and a stale sender (its transfer
	// already ended) just fails its send harmlessly. `None` when nothing recursive is in flight.
	let mut conflict_tx: Option<mpsc::Sender<ConflictChoice>> = None;

	// The cancel flag for the transfer currently running (§16), held the same way as `conflict_tx`:
	// one transfer at a time, so each start makes a fresh flag and keeps a clone here, and a
	// `CancelTransfer` sets it. The spawned copy loop polls it between chunks; a stale flag whose
	// transfer already ended simply never gets read. `None` when nothing is transferring.
	let mut cancel: Option<Arc<AtomicBool>> = None;

	loop {
		tokio::select! {
			// Something arrived on one of the session's shells (§45). Which one is in the message;
			// `Shells` decides whether those bytes are terminal output or the next line of an
			// elevation's credential conversation, and says when the SESSION is over — which only
			// the login shell closing means.
			Some(message) = from_shells.recv() => {
				match shells.on_msg(message, events).await {
					shell::After::Nothing => {}
					// That account's shell has gone, so its file access goes too (§46): dropping its
					// entry closes the sftp session it held, which ends the elevated `sftp-server`.
					shell::After::Ended(identity) => accounts.remove(identity),
					shell::After::SessionOver => break,
				}
			}
			// A local/dynamic forward accepted a connection (§27). Open its SSH channel here —
			// the one place allowed to — and let the pump run detached.
			Some(accepted) = accepted_rx.recv() => {
				forward::open_local_tunnel(session, accepted, events.clone()).await;
			}
			// A file operation running as another account wants a channel to run a command on (§46).
			// Same reason as the forward above: this loop is the only place holding the session
			// handle, so it is the only place that can open one. The requester waits on the one-shot.
			Some(asuser::ChannelRequest(reply)) = channel_requests.recv() => {
				let opened = session
					.channel_open_session()
					.await
					.map_err(|error| format!("{error}"));
				let _ = reply.send(opened);
			}
			// A command arrived from the GUI (via run()).
			command = to_session_rx.recv() => {
				match command {
					// Typing goes to the shell the user is looking at, and a resize to every one of
					// them — they share a window, so they share a pty size (§45).
					Some(SessionMsg::Data(bytes)) => shells.input(bytes).await,
					Some(SessionMsg::Resize { cols, rows }) => shells.resize(cols, rows).await,
					// Become another account on this same connection (§45): another shell, running
					// the elevation program, which holds its own conversation on its own channel.
					// The account is registered for FILE work at the same time (§46), so the panes
					// can read as it the moment its shell is live.
					Some(SessionMsg::Elevate { identity, kind, user }) => {
						accounts.add(identity, kind, user.clone());
						shells.elevate(session, events, identity, kind.command(&user)).await;
					}
					Some(SessionMsg::ElevateAnswer { identity, secret }) => {
						// A `true` here means that answer was the password cmote itself asked for by
						// name — so it is the one sudo will want on a file channel too, and it is kept
						// for this connection (§46). A one-time code answers `false` and is never kept.
						if shells.answer(identity, secret.clone()).await {
							accounts.set_secret(identity, secret);
						}
					}
					// Both halves of "which account is on screen" move together (§45, §46): typing
					// goes to that shell, and every file operation from now on reads as that account.
					Some(SessionMsg::SelectIdentity(identity)) => {
						shells.select(identity);
						accounts.select(identity);
					}
					Some(SessionMsg::Reply { identity, bytes }) => {
						shells.reply(identity, bytes).await;
					}
					Some(SessionMsg::CloseIdentity(identity)) => shells.close(identity).await,
					// Passphrase and keyboard-interactive answers only matter during auth;
					// ignore any that arrive late, once the shell is already streaming.
					Some(SessionMsg::Passphrase(_)) | Some(SessionMsg::Interactive(_)) => {}
					// The transfer runs on its own channel and its own task, so the
					// shell keeps flowing while a big file goes across (§17).
					Some(SessionMsg::Upload { local, remote, overwrite, resume }) => {
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						let backend = accounts.files(session).await;
						upload::start(backend, events, local, remote, overwrite, resume, flag).await;
					}
					// The batch collision pre-scan (§17): a couple of round trips on its own
					// channel before the first byte, so the "some are already there" question
					// is asked once for the whole batch.
					Some(SessionMsg::CheckUploads { dir, names }) => {
						let backend = accounts.files(session).await;
						upload::precheck(backend, events, dir, names).await;
					}
					// Listings, renames and downloads also run on their own channel and
					// their own task, so a slow directory or a big file never holds up
					// the terminal (§18, §19). Which ACCOUNT each one reads as is settled here,
					// before the work starts (§46): `accounts` answers with the best backend that
					// account has — its own sftp session, shell commands, or a reason it has neither.
					Some(SessionMsg::ListDir(path)) => {
						let backend = accounts.browse(session).await;
						browse::list(backend, events, path).await;
					}
					Some(SessionMsg::ListFiles { path, request }) => {
						// The pane shows modification times, and a time needs the server's zone to be
						// read as the server's own clock (§20). Asked once per session, alongside the
						// first listing — as the login account, since a machine's timezone needs no
						// privilege and the answer belongs to the machine, not to an account.
						if accounts.take_zone_probe() {
							browse::probe_zone(accounts.login_runner(), events);
						}
						let backend = accounts.browse(session).await;
						browse::list_all(backend, events, path, request).await;
					}
					Some(SessionMsg::ReadLink(path)) => {
						let backend = accounts.browse(session).await;
						browse::read_link(backend, events, path).await;
					}
					Some(SessionMsg::Download { remote, local, resume }) => {
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						let backend = accounts.files(session).await;
						download::start(backend, events, remote, local, resume, flag).await;
					}
					// A recursive transfer runs on its own channel and task like a single file,
					// but it can pause to ask about a collision — so a fresh reply channel is made
					// here and its sending end kept, to forward the answers to (§17, §19). It gets a
					// fresh cancel flag too, on the same one-at-a-time reasoning (§16).
					Some(SessionMsg::UploadTree { local, remote, resume }) => {
						let (answers_tx, answers_rx) = mpsc::channel::<ConflictChoice>(8);
						conflict_tx = Some(answers_tx);
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						let backend = accounts.files(session).await;
						upload::start_tree(backend, events, local, remote, resume, answers_rx, flag).await;
					}
					Some(SessionMsg::DownloadTree { remote, local, resume }) => {
						let (answers_tx, answers_rx) = mpsc::channel::<ConflictChoice>(8);
						conflict_tx = Some(answers_tx);
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						let backend = accounts.files(session).await;
						download::start_tree(backend, events, remote, local, resume, answers_rx, flag).await;
					}
					// The editor reads and writes a whole remote file on its own sftp channel, like a
					// transfer, but buffer-shaped and reply-routed by the editor tab's id (§32).
					//
					// It names its own account rather than using the selected one (§46): a file opened
					// as root must be read and saved as root, whichever account the panes have moved on
					// to while it was being edited.
					Some(SessionMsg::EditLoad { identity, editor_id, path }) => {
						let backend = accounts.files_as(session, identity).await;
						edit::load(backend, events, editor_id, path).await;
					}
					Some(SessionMsg::EditSave { identity, editor_id, path, bytes }) => {
						let backend = accounts.files_as(session, identity).await;
						edit::save(backend, events, editor_id, path, bytes).await;
					}
					// Forward a collision answer to the transfer parked on it. A send that fails —
					// the transfer already finished, or there was never a recursive one — is
					// nothing to act on; the answer simply had no one waiting.
					Some(SessionMsg::ResolveConflict(choice)) => {
						if let Some(answers) = conflict_tx.as_ref() {
							let _ = answers.send(choice).await;
						}
					}
					// Stop the running transfer (§16): raise the flag its copy loop polls. A cancel
					// with nothing in flight, or after the transfer already ended, sets a flag no one
					// reads — harmless.
					Some(SessionMsg::CancelTransfer) => {
						if let Some(flag) = cancel.as_ref() {
							flag.store(true, Ordering::Relaxed);
						}
					}
					// Creating and deleting share the browse session with the listings, the same
					// as rename — one channel for all the tree's small operations (§18).
					Some(SessionMsg::MakeDir(path)) => {
						let backend = accounts.browse(session).await;
						browse::make_dir(backend, events, path).await;
					}
					Some(SessionMsg::Delete(paths)) => {
						let backend = accounts.browse(session).await;
						browse::remove(backend, events, paths).await;
					}
					Some(SessionMsg::RenameDir { from, to }) => {
						let backend = accounts.browse(session).await;
						browse::rename(backend, events, from, to).await;
					}
					// Start / stop a port forward on this connection (§27). Add spawns a local
					// listener or asks the server to listen; remove aborts / cancels it. Both run
					// on the same session, so no new authentication.
					Some(SessionMsg::AddForward { id, spec }) => {
						forwards.add(session, events, id, spec).await;
					}
					Some(SessionMsg::RemoveForward(id)) => {
						forwards.remove(session, id).await;
					}
					// Explicit disconnect, or run() dropped the link. Every shell goes, not just the
					// login one (§45) — an elevated shell left running would hold the connection.
					Some(SessionMsg::Disconnect) | None => {
						shells.eof_all().await;
						break;
					}
				}
			}
		}
	}
	Ok(())
}

/// Our russh event handler. The one method that matters for v1 is the host-key
/// gate; every other callback keeps its default (no-op) behavior. `pub(crate)` because
/// the auth module (`ssh::auth`) names it as the session's handler type.
pub(crate) struct Handler {
	host: String,
	port: u16,
	known_hosts: PathBuf,
	events: mpsc::Sender<SshEvent>,
	/// Consumed once, in `check_server_key`, to await the user's decision.
	decision: Option<oneshot::Receiver<HostKeyChoice>>,
	/// The active remote forwards, shared with the session loop (§27): the server's bound port →
	/// the local target to dial. Read in `server_channel_open_forwarded_tcpip` when the server
	/// opens a channel for a connection that arrived on one of those ports.
	remote_forwards: forward::RemoteTable,
}

impl client::Handler for Handler {
	type Error = russh::Error;

	/// TOFU host-key gate (§8), called by russh during the handshake, before
	/// authentication. Returning `Ok(false)` refuses the connection.
	async fn check_server_key(
		&mut self,
		server_public_key: &PublicKey,
	) -> Result<bool, Self::Error> {
		let verdict =
			match hostkey::verify(&self.host, self.port, server_public_key, &self.known_hosts) {
				Ok(verdict) => verdict,
				Err(error) => {
					eprintln!("host-key check failed: {error:#}");
					let _ = self
						.events
						.send(SshEvent::Error(
							"Could not read the known_hosts file.".to_string(),
						))
						.await;
					return Ok(false);
				}
			};

		match verdict {
			// Pinned and matches: proceed silently.
			HostKeyVerdict::Known => Ok(true),

			// Pinned but DIFFERENT: key rotation or a man-in-the-middle. Show both fingerprints
			// and wait for the user's explicit override — reject / trust once / replace (§8). No
			// auto-trust: the change is a security event, so the decision is always the user's.
			HostKeyVerdict::Changed { line } => {
				// The fingerprint currently pinned, so the dialog can show what was trusted beside
				// what the server now sends. A read failure is non-fatal — the dialog still opens
				// with the presented key and a placeholder for the stored one.
				let stored =
					hostkey::stored_fingerprint(&self.known_hosts, line).unwrap_or_else(|error| {
						eprintln!("failed to read stored host key: {error:#}");
						"(could not read the stored key)".to_string()
					});
				let presented = hostkey::fingerprint(server_public_key);
				let _ = self
					.events
					.send(SshEvent::HostKeyChanged { stored, presented })
					.await;

				match self.await_decision().await {
					// Refuse: the safe default, and what a dropped GUI counts as.
					HostKeyChoice::Reject => Ok(false),
					// Trust this session only; leave known_hosts as it is, so it warns again.
					HostKeyChoice::TrustOnce => Ok(true),
					// Replace the stale entry so future connections verify against the new key.
					HostKeyChoice::Pin => {
						if let Err(error) = hostkey::replace(
							&self.host,
							self.port,
							server_public_key,
							&self.known_hosts,
							line,
						) {
							eprintln!("failed to replace host key: {error:#}");
							let _ = self
								.events
								.send(SshEvent::Error(
									"Could not update the saved host key.".to_string(),
								))
								.await;
							return Ok(false);
						}
						Ok(true)
					}
				}
			}

			// First contact: show the fingerprint and wait for explicit consent.
			HostKeyVerdict::Unknown => {
				let fingerprint = hostkey::fingerprint(server_public_key);
				let _ = self.events.send(SshEvent::HostKey(fingerprint)).await;

				match self.await_decision().await {
					// Reject (the default for a dropped GUI too), or connect once without pinning.
					HostKeyChoice::Reject => Ok(false),
					HostKeyChoice::TrustOnce => Ok(true),
					// Pin the accepted key so future connections are verified against it.
					HostKeyChoice::Pin => {
						if let Err(error) = hostkey::learn(
							&self.host,
							self.port,
							server_public_key,
							&self.known_hosts,
						) {
							eprintln!("failed to record host key: {error:#}");
							let _ = self
								.events
								.send(SshEvent::Error(
									"Could not save the accepted host key.".to_string(),
								))
								.await;
							return Ok(false);
						}
						Ok(true)
					}
				}
			}
		}
	}

	/// A connection arrived on one of our remote forwards (§27): the server bound a port
	/// (`tcpip_forward`) and someone connected to it, so it opens this `forwarded-tcpip` channel
	/// back to us. Route it to the local target mapped for that port — accept and pump if one is
	/// there, reject otherwise. All the logic is in `ssh::forward` so this stays a one-liner.
	async fn server_channel_open_forwarded_tcpip(
		&mut self,
		channel: Channel<client::Msg>,
		_connected_address: &str,
		connected_port: u32,
		_originator_address: &str,
		_originator_port: u32,
		reply: client::ChannelOpenHandle,
		_session: &mut client::Session,
	) -> Result<(), Self::Error> {
		forward::accept_remote(
			&self.remote_forwards,
			channel,
			reply,
			connected_port as u16,
			self.events.clone(),
		)
		.await;
		Ok(())
	}
}

impl Handler {
	/// Block the handshake on the user's host-key choice (§8), shared by the first-contact and
	/// mismatch gates. Consumes the one-shot; a dropped sender — the GUI went away before
	/// answering — is treated as `Reject`, the safe default. Called at most once per connection
	/// (a handshake sees either an unknown key or a changed one, never both).
	async fn await_decision(&mut self) -> HostKeyChoice {
		match self.decision.take() {
			Some(rx) => rx.await.unwrap_or(HostKeyChoice::Reject),
			None => HostKeyChoice::Reject,
		}
	}
}

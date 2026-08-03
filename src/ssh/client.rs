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
use russh::client;
use russh::keys::PublicKey;
use russh::{Channel, ChannelMsg};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::bridge::{ConflictChoice, ConnectParams, HostKeyChoice, SshCommand, SshEvent};
use crate::secret::Secret;
use crate::ssh::auth;
use crate::ssh::browse;
use crate::ssh::download;
use crate::ssh::forward;
use crate::ssh::hostkey::{self, HostKeyVerdict};
use crate::ssh::upload;
use crate::term;

/// How long to wait for the TCP connect + SSH handshake before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many accepted-but-not-yet-opened forward connections may queue between the listeners and
/// the session loop before a listener awaits (§27). Generous — a burst of tunnel connections is
/// drained as fast as channels open — and bounded so it cannot grow without limit.
const CHANNEL_BOUND_FORWARD: usize = 64;

/// The shell integration cmote installs once, right after the shell opens (§17).
///
/// SSH never tells a client where the remote shell *is*, so — like every terminal that
/// shows the remote directory — we have the shell announce it. `cmote_cwd` prints an
/// **OSC 7** sequence (`ESC ] 7 ; file://host/path ESC \`), which is invisible in the
/// terminal and picked up by `term::cwd`; hooking it into `PROMPT_COMMAND` (bash) and
/// `precmd_functions` (zsh) makes it fire on every prompt, so the directory follows
/// `cd` with no further typing. The trailing call reports the starting directory
/// immediately instead of waiting for the next prompt.
///
/// `ponytail:` bash and zsh only. fish already emits OSC 7 on its own, and a Windows
/// shell that emits OSC 9;9 is read too, so the passive tracker covers those; any other
/// shell simply prints a syntax error on this one line and leaves the cwd unknown — the
/// upload dialog then asks for the path. Upgrade path: detect the shell first (`echo
/// $0`) and send the matching snippet.
const CWD_HOOK: &str = concat!(
	r#"cmote_cwd() { printf '\033]7;file://%s%s\033\\' "${HOSTNAME-}" "$PWD"; }; "#,
	r#"PROMPT_COMMAND="cmote_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; "#,
	r#"precmd_functions+=(cmote_cwd); cmote_cwd"#,
	"\n",
);

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
	/// Terminal resized; reflow the remote pty.
	Resize { cols: u16, rows: u16 },
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

	// Install the cwd announcer before the user can type (§17). Sent as ordinary shell
	// input, so it is echoed once like any typed command; from then on the directory
	// arrives invisibly on every prompt.
	channel.data(CWD_HOOK.as_bytes()).await?;

	stream(channel, &session, events, to_session_rx, remote_forwards).await
}

/// The bidirectional pump: server output -> GUI, GUI input/resize -> server.
/// Runs until either side closes. `session` is borrowed alongside the shell channel so
/// an upload can open its own sftp channel on the same connection (§17).
async fn stream(
	mut channel: Channel<client::Msg>,
	session: &client::Handle<Handler>,
	events: &mpsc::Sender<SshEvent>,
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
	remote_forwards: forward::RemoteTable,
) -> Result<()> {
	// The explorer's SFTP channel (§18): opened on the first listing and kept for the
	// rest of the session, since a tree asks many small questions.
	let mut sftp = browse::Sftp::default();

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
			// Something arrived from the server on the channel.
			message = channel.wait() => {
				match message {
					Some(ChannelMsg::Data { data }) => {
						let _ = events.send(SshEvent::Output(data.to_vec())).await;
					}
					// stderr of the remote shell; render it inline too.
					Some(ChannelMsg::ExtendedData { data, .. }) => {
						let _ = events.send(SshEvent::Output(data.to_vec())).await;
					}
					// Remote closed, or the shell exited: end the session.
					Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | Some(ChannelMsg::ExitStatus { .. }) => break,
					Some(_) => {}
					None => break, // channel fully closed
				}
			}
			// A local/dynamic forward accepted a connection (§27). Open its SSH channel here —
			// the one place allowed to — and let the pump run detached.
			Some(accepted) = accepted_rx.recv() => {
				forward::open_local_tunnel(session, accepted, events.clone()).await;
			}
			// A command arrived from the GUI (via run()).
			command = to_session_rx.recv() => {
				match command {
					Some(SessionMsg::Data(bytes)) => channel.data(&bytes[..]).await?,
					Some(SessionMsg::Resize { cols, rows }) => {
						channel.window_change(cols as u32, rows as u32, 0, 0).await?;
					}
					// Passphrase and keyboard-interactive answers only matter during auth;
					// ignore any that arrive late, once the shell is already streaming.
					Some(SessionMsg::Passphrase(_)) | Some(SessionMsg::Interactive(_)) => {}
					// The transfer runs on its own channel and its own task, so the
					// shell keeps flowing while a big file goes across (§17).
					Some(SessionMsg::Upload { local, remote, overwrite, resume }) => {
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						upload::start(session, events, local, remote, overwrite, resume, flag).await;
					}
					// The batch collision pre-scan (§17): a couple of round trips on its own
					// channel before the first byte, so the "some are already there" question
					// is asked once for the whole batch.
					Some(SessionMsg::CheckUploads { dir, names }) => {
						upload::precheck(session, events, dir, names).await;
					}
					// Listings, renames and downloads also run on their own channel and
					// their own task, so a slow directory or a big file never holds up
					// the terminal (§18, §19).
					Some(SessionMsg::ListDir(path)) => {
						browse::list(session, &mut sftp, events, path).await;
					}
					Some(SessionMsg::ListFiles { path, request }) => {
						browse::list_all(session, &mut sftp, events, path, request).await;
					}
					Some(SessionMsg::ReadLink(path)) => {
						browse::read_link(session, &mut sftp, events, path).await;
					}
					Some(SessionMsg::Download { remote, local, resume }) => {
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						download::start(session, events, remote, local, resume, flag).await;
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
						upload::start_tree(session, events, local, remote, resume, answers_rx, flag).await;
					}
					Some(SessionMsg::DownloadTree { remote, local, resume }) => {
						let (answers_tx, answers_rx) = mpsc::channel::<ConflictChoice>(8);
						conflict_tx = Some(answers_tx);
						let flag = Arc::new(AtomicBool::new(false));
						cancel = Some(flag.clone());
						download::start_tree(session, events, remote, local, resume, answers_rx, flag).await;
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
						browse::make_dir(session, &mut sftp, events, path).await;
					}
					Some(SessionMsg::Delete(paths)) => {
						browse::remove(session, &mut sftp, events, paths).await;
					}
					Some(SessionMsg::RenameDir { from, to }) => {
						browse::rename(session, &mut sftp, events, from, to).await;
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
					// Explicit disconnect, or run() dropped the link.
					Some(SessionMsg::Disconnect) | None => {
						let _ = channel.eof().await;
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

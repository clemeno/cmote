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
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::PublicKey;
use russh::{Channel, ChannelMsg};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::bridge::{ConnectParams, SshCommand, SshEvent};
use crate::ssh::hostkey::{self, HostKeyVerdict};
use crate::term;

/// How long to wait for the TCP connect + SSH handshake before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

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
			SshCommand::HostKeyResponse(accept) => {
				if let Some(link) = session.as_mut() {
					link.send_decision(accept);
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
			SshCommand::Disconnect => {
				if let Some(link) = session.take() {
					let _ = link.to_session.send(SessionMsg::Disconnect).await;
				}
			}
		}
	}
}

/// Messages `run()` forwards to a live session task.
enum SessionMsg {
	/// Keyboard bytes to write to the shell.
	Data(Vec<u8>),
	/// Terminal resized; reflow the remote pty.
	Resize { cols: u16, rows: u16 },
	/// Tear the session down.
	Disconnect,
}

/// `run()`'s handle to a spawned session task: a channel for input/resize/quit
/// and a one-shot for the host-key decision (used at most once).
struct SessionLink {
	to_session: mpsc::Sender<SessionMsg>,
	decision: Option<oneshot::Sender<bool>>,
}

impl SessionLink {
	/// Spawn a session task for `params` and return the handle to talk to it.
	fn start(params: ConnectParams, events: mpsc::Sender<SshEvent>) -> Self {
		let (to_session_tx, to_session_rx) = mpsc::channel::<SessionMsg>(256);
		let (decision_tx, decision_rx) = oneshot::channel::<bool>();

		tokio::spawn(session_task(params, events, to_session_rx, decision_rx));

		Self {
			to_session: to_session_tx,
			decision: Some(decision_tx),
		}
	}

	/// Deliver the user's host-key decision to the waiting handshake. Consumes
	/// the one-shot; further calls are no-ops.
	fn send_decision(&mut self, accept: bool) {
		if let Some(tx) = self.decision.take() {
			let _ = tx.send(accept);
		}
	}
}

/// One connection's whole life. Translates the outcome into a final event and
/// keeps all error detail out of the message shown to the user (§12).
async fn session_task(
	params: ConnectParams,
	events: mpsc::Sender<SshEvent>,
	to_session_rx: mpsc::Receiver<SessionMsg>,
	decision_rx: oneshot::Receiver<bool>,
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
	to_session_rx: mpsc::Receiver<SessionMsg>,
	decision_rx: oneshot::Receiver<bool>,
) -> Result<()> {
	let config = Arc::new(client::Config {
		// No inactivity timeout: an interactive shell may sit idle for a long
		// time and must not be dropped for being quiet.
		inactivity_timeout: None,
		..Default::default()
	});

	let handler = Handler {
		host: params.host.clone(),
		port: params.port,
		known_hosts: hostkey::known_hosts_path()?,
		events: events.clone(),
		decision: Some(decision_rx),
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

	// Password authentication (§7). A failure is deliberately generic — we never
	// reveal whether the user or the password was wrong (no credential oracle).
	let auth = session
		.authenticate_password(params.user.as_str(), params.password.expose())
		.await
		.context("authentication request failed")?;
	if !auth.success() {
		bail!("authentication failed");
	}

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

	stream(channel, events, to_session_rx).await
}

/// The bidirectional pump: server output -> GUI, GUI input/resize -> server.
/// Runs until either side closes.
async fn stream(
	mut channel: Channel<client::Msg>,
	events: &mpsc::Sender<SshEvent>,
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
) -> Result<()> {
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
			// A command arrived from the GUI (via run()).
			command = to_session_rx.recv() => {
				match command {
					Some(SessionMsg::Data(bytes)) => channel.data(&bytes[..]).await?,
					Some(SessionMsg::Resize { cols, rows }) => {
						channel.window_change(cols as u32, rows as u32, 0, 0).await?;
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
/// gate; every other callback keeps its default (no-op) behavior.
struct Handler {
	host: String,
	port: u16,
	known_hosts: PathBuf,
	events: mpsc::Sender<SshEvent>,
	/// Consumed once, in `check_server_key`, to await the user's decision.
	decision: Option<oneshot::Receiver<bool>>,
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

			// Pinned but different: possible MITM. Refuse, no override (§8).
			HostKeyVerdict::Changed { .. } => {
				let _ = self
					.events
					.send(SshEvent::Error(
						"Host key has CHANGED — refusing to connect (possible attack). \
						 Remove the stale known_hosts entry if this change is expected."
							.to_string(),
					))
					.await;
				Ok(false)
			}

			// First contact: show the fingerprint and wait for explicit consent.
			HostKeyVerdict::Unknown => {
				let fingerprint = hostkey::fingerprint(server_public_key);
				let _ = self.events.send(SshEvent::HostKey(fingerprint)).await;

				// Block the handshake here until the GUI answers. A dropped
				// sender (GUI gone) counts as "reject".
				let accept = match self.decision.take() {
					Some(rx) => rx.await.unwrap_or(false),
					None => false,
				};
				if !accept {
					return Ok(false);
				}

				// Pin the accepted key so future connections are verified.
				if let Err(error) =
					hostkey::learn(&self.host, self.port, server_public_key, &self.known_hosts)
				{
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

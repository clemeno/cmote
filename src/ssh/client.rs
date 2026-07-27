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

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKey};
use russh::{Channel, ChannelMsg};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::bridge::{AuthMethod, ConnectParams, SshCommand, SshEvent};
use crate::secret::Secret;
use crate::ssh::browse;
use crate::ssh::download;
use crate::ssh::hostkey::{self, HostKeyVerdict};
use crate::ssh::keyfile::{self, Loaded};
use crate::ssh::upload;
use crate::term;

/// How long to wait for the TCP connect + SSH handshake before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many times to re-prompt for a private-key passphrase before giving up.
const MAX_PASSPHRASE_ATTEMPTS: u32 = 3;

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
			SshCommand::HostKeyResponse(accept) => {
				if let Some(link) = session.as_mut() {
					link.send_decision(accept);
				}
			}
			SshCommand::Passphrase(secret) => {
				if let Some(link) = session.as_ref() {
					let _ = link.to_session.send(SessionMsg::Passphrase(secret)).await;
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
			} => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Upload {
							local,
							remote,
							overwrite,
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
			SshCommand::Download { remote, local } => {
				if let Some(link) = session.as_ref() {
					let _ = link
						.to_session
						.send(SessionMsg::Download { remote, local })
						.await;
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
	/// A passphrase the user typed to unlock an encrypted key (§7).
	Passphrase(Secret),
	/// Send a local file to the remote over a second, sftp channel (§17).
	Upload {
		local: PathBuf,
		remote: String,
		overwrite: bool,
	},
	/// Check which of an upload batch's names already exist before it sends (§17).
	CheckUploads { dir: String, names: Vec<String> },
	/// List the folders inside a remote directory, for the explorer tree (§18).
	ListDir(String),
	/// List every entry inside a remote directory, for the files pane (§19).
	ListFiles { path: String, request: u64 },
	/// Fetch a remote file to a local path (§19).
	Download { remote: String, local: PathBuf },
	/// Resolve one symlink for the files pane's details popup (§20).
	ReadLink(String),
	/// Rename a remote folder (§18).
	RenameDir { from: String, to: String },
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
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
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

	// Authenticate with the method the user chose (§7). A failure is deliberately
	// generic — we never reveal whether the user, the password, or the key was
	// wrong (no credential oracle).
	let authenticated = match &params.auth {
		AuthMethod::Password(password) => session
			.authenticate_password(params.user.as_str(), password.expose())
			.await
			.context("authentication request failed")?
			.success(),

		AuthMethod::Key { path, passphrase } => {
			// Load the key. A passphrase pre-seeded from the form (§14) is tried first;
			// otherwise an encrypted key prompts interactively (§7). `clone` because the
			// passphrase is borrowed from `params` and `resolve_key` needs to own it.
			let key = resolve_key(path, passphrase.clone(), events, &mut to_session_rx).await?;
			// RSA keys must pick a signature hash: OpenSSH offers rsa-sha2-512,
			// rsa-sha2-256, or the legacy ssh-rsa (SHA-1). Ask the server which
			// it accepts and use the strongest; other key types ignore this.
			let hash_alg = if key.algorithm().is_rsa() {
				session.best_supported_rsa_hash().await?.flatten()
			} else {
				None
			};
			let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg);
			session
				.authenticate_publickey(params.user.as_str(), key)
				.await
				.context("authentication request failed")?
				.success()
		}
	};
	if !authenticated {
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

	// Install the cwd announcer before the user can type (§17). Sent as ordinary shell
	// input, so it is echoed once like any typed command; from then on the directory
	// arrives invisibly on every prompt.
	channel.data(CWD_HOOK.as_bytes()).await?;

	stream(channel, &session, events, to_session_rx).await
}

/// Load the chosen private key (§7, §14), prompting for a passphrase only when the
/// key is actually encrypted. `initial` is the optional passphrase pre-seeded from
/// the form: `Some` is tried before any prompt (so a known passphrase unlocks the
/// key silently), `None` keeps the original interactive-only behavior.
///
/// The load happens in two stages. First we probe with NO passphrase, which cleanly
/// classifies the file: an unencrypted key loads here (any typed passphrase is
/// meaningless for it and is correctly ignored); an unencrypted but malformed key is
/// a hard error; an encrypted key reports `NeedsPassphrase` and drops to the retry
/// loop. There we try the pre-seed (if any), then ask the GUI and retry — up to
/// `MAX_PASSPHRASE_ATTEMPTS` prompts. A wrong passphrase (pre-seeded or typed) just
/// asks again.
async fn resolve_key(
	path: &Path,
	initial: Option<Secret>,
	events: &mpsc::Sender<SshEvent>,
	to_session_rx: &mut mpsc::Receiver<SessionMsg>,
) -> Result<PrivateKey> {
	// Stage one: classify the file with no passphrase.
	match keyfile::load_private_key(path, None)? {
		Loaded::Key(key) => return Ok(*key),
		// Encrypted: fall through to the passphrase loop below.
		Loaded::NeedsPassphrase => {}
	}

	// Stage two: the key is encrypted. Try the pre-seed first, then prompt and retry.
	let mut passphrase = initial;
	let mut attempts = 0u32;

	loop {
		// A passphrase in hand (pre-seed or typed) that unlocks the key wins immediately.
		if let Some(secret) = passphrase.as_ref()
			&& let Ok(Loaded::Key(key)) = keyfile::load_private_key(path, Some(secret))
		{
			return Ok(*key);
		}

		if attempts >= MAX_PASSPHRASE_ATTEMPTS {
			bail!("too many incorrect passphrase attempts");
		}
		attempts += 1;

		let _ = events.send(SshEvent::NeedPassphrase).await;
		passphrase = Some(recv_passphrase(to_session_rx).await?);
	}
}

/// Await the user's passphrase from the GUI, ignoring any stray input/resize
/// that could arrive before the shell is open. A disconnect or a dropped channel
/// means the user gave up on the prompt.
async fn recv_passphrase(to_session_rx: &mut mpsc::Receiver<SessionMsg>) -> Result<Secret> {
	loop {
		match to_session_rx.recv().await {
			Some(SessionMsg::Passphrase(secret)) => return Ok(secret),
			Some(SessionMsg::Disconnect) | None => {
				bail!("cancelled before a passphrase was entered")
			}
			Some(_) => {} // ignore keystrokes/resize until the shell exists
		}
	}
}

/// The bidirectional pump: server output -> GUI, GUI input/resize -> server.
/// Runs until either side closes. `session` is borrowed alongside the shell channel so
/// an upload can open its own sftp channel on the same connection (§17).
async fn stream(
	mut channel: Channel<client::Msg>,
	session: &client::Handle<Handler>,
	events: &mpsc::Sender<SshEvent>,
	mut to_session_rx: mpsc::Receiver<SessionMsg>,
) -> Result<()> {
	// The explorer's SFTP channel (§18): opened on the first listing and kept for the
	// rest of the session, since a tree asks many small questions.
	let mut sftp = browse::Sftp::default();

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
					// A passphrase only matters during auth; ignore a late one.
					Some(SessionMsg::Passphrase(_)) => {}
					// The transfer runs on its own channel and its own task, so the
					// shell keeps flowing while a big file goes across (§17).
					Some(SessionMsg::Upload { local, remote, overwrite }) => {
						upload::start(session, events, local, remote, overwrite).await;
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
					Some(SessionMsg::Download { remote, local }) => {
						download::start(session, events, remote, local).await;
					}
					Some(SessionMsg::RenameDir { from, to }) => {
						browse::rename(session, &mut sftp, events, from, to).await;
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

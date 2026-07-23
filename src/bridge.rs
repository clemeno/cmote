// bridge.rs — the message types AND channel wiring that join the GUI thread to
// the background tokio thread (PLAN §4).
//
// The GUI thread (iced, synchronous) and the SSH thread (tokio, async) never
// share memory. They communicate ONLY through two channels:
//   GUI  --SshCommand-->  tokio task   (user intent: connect, type, resize, quit)
//   GUI  <--SshEvent----  tokio task   (results: output bytes, status, errors)
//
// This module owns the wiring: `subscription()` starts the tokio thread and
// turns its outbound events into an iced `Subscription`. The command sender is
// handed back to the GUI as the first event (`SshEvent::Ready`) — the standard
// iced pattern for a two-way "worker" (the GUI can't pull a value out of a
// subscription, so the subscription pushes the sender to it).

use std::path::PathBuf;

use iced::Subscription;
use iced::futures::SinkExt; // brings `.send()` onto the futures mpsc Sender
use iced::futures::Stream;
use tokio::sync::mpsc;

use crate::secret::Secret;
use crate::ssh;

/// Bounded-channel capacity. Bounded so a flood of terminal output can't grow
/// memory without limit — the producer awaits when the consumer falls behind
/// (backpressure, §4). `ponytail:` a generous fixed bound; tune only if needed.
const CHANNEL_BOUND: usize = 256;

/// How the user proves who they are (§7). Exactly one method per connection, so
/// a sum type is the right shape: "password OR key, never both and never
/// neither" becomes impossible to represent wrongly. Both variants carry their
/// secret material in `Secret`, so it is redacted in `Debug` and wiped on drop.
#[derive(Debug, Clone)]
pub enum AuthMethod {
	/// A password typed into the form.
	Password(Secret),
	/// A private-key file (PEM / OpenSSH / PuTTY `.ppk`). No passphrase is carried
	/// here: if the key turns out to be encrypted, the session asks for it
	/// interactively (§7) — `SshEvent::NeedPassphrase` out, `SshCommand::Passphrase`
	/// back — so an unencrypted key is never made to prompt.
	Key { path: PathBuf },
}

/// Parameters the user fills in on the connect form, handed to the SSH task once
/// to start a session. A plain owned struct so it moves across the channel
/// without borrowing GUI state.
#[derive(Debug, Clone)]
pub struct ConnectParams {
	pub host: String,
	pub port: u16,
	pub user: String,
	/// The chosen authentication method and its secret material (§7).
	pub auth: AuthMethod,
}

/// GUI -> SSH task. Everything the user can ask the connection to do.
#[derive(Debug, Clone)]
pub enum SshCommand {
	/// Open a new connection with these parameters.
	Connect(ConnectParams),
	/// The user's answer to an unknown-host-key prompt (§8): accept (pin it and
	/// continue) or reject (refuse the connection). `true` = accept.
	HostKeyResponse(bool),
	/// The passphrase the user typed after a `NeedPassphrase` prompt, to decrypt
	/// the chosen private key (§7).
	Passphrase(Secret),
	/// Raw keyboard bytes to send down the channel (keystroke, escape seq, ...).
	Input(Vec<u8>),
	/// The terminal view changed size; reflow the remote pty.
	Resize { cols: u16, rows: u16 },
	/// Close the channel and tear down the connection.
	Disconnect,
}

/// SSH task -> GUI. Everything the connection reports back. The GUI turns each
/// of these into an `app::Message::Ssh(..)`.
#[derive(Debug, Clone)]
pub enum SshEvent {
	/// First event: the channel the GUI uses to send commands back to the task.
	/// Delivered once, right after the worker starts.
	Ready(mpsc::Sender<SshCommand>),
	/// TCP + handshake started; drives the "Connecting…" status line.
	Connecting,
	/// The server presented an unseen host key. The GUI shows this SHA-256
	/// fingerprint and asks the user to accept before we continue (§8).
	HostKey(String),
	/// The private key is encrypted and we need its passphrase (§7).
	NeedPassphrase,
	/// Authentication succeeded and a shell is open — switch to the terminal.
	Connected,
	/// A chunk of terminal output to feed the vt100 parser (§9).
	Output(Vec<u8>),
	/// The session ended (server closed, or user disconnected).
	Disconnected,
	/// Something failed. A generic, non-leaking message (§12).
	Error(String),
}

/// Build the subscription that carries SSH events into the GUI. iced identifies
/// a subscription by the `worker` function's type, so it starts exactly once and
/// keeps running for the life of the app.
pub fn subscription() -> Subscription<SshEvent> {
	Subscription::run(worker)
}

/// The worker stream. Runs on iced's executor; its job is only to *shuttle*
/// events — the real network I/O runs on a separate tokio runtime thread (§4).
fn worker() -> impl Stream<Item = SshEvent> {
	// `iced::stream::channel` gives us `output`, a sink into the subscription.
	// Its concrete type is a futures mpsc sender; annotate so inference is happy.
	iced::stream::channel(
		CHANNEL_BOUND,
		|mut output: iced::futures::channel::mpsc::Sender<SshEvent>| async move {
			// Two channels: commands to the network thread, events back from it.
			let (command_tx, command_rx) = mpsc::channel::<SshCommand>(CHANNEL_BOUND);
			let (event_tx, mut event_rx) = mpsc::channel::<SshEvent>(CHANNEL_BOUND);

			// Spawn the network thread with its OWN tokio runtime. russh needs a
			// tokio I/O driver, so it must run on a real tokio runtime; keeping it on
			// a dedicated thread means the GUI never blocks on the socket.
			std::thread::Builder::new()
				.name("cmote-ssh".to_string())
				.spawn(move || {
					let runtime = tokio::runtime::Builder::new_multi_thread()
						.enable_all()
						.build()
						.expect("failed to build the SSH tokio runtime");
					runtime.block_on(ssh::client::run(command_rx, event_tx));
				})
				.expect("failed to spawn the SSH thread");

			// Hand the command sender to the GUI so `update` can talk back.
			if output.send(SshEvent::Ready(command_tx)).await.is_err() {
				return; // GUI went away before we started; nothing to do.
			}

			// Forward every event from the network thread into the subscription.
			// tokio's mpsc receiver works fine off the tokio runtime (it needs no
			// reactor), so awaiting it here on iced's executor is correct.
			while let Some(event) = event_rx.recv().await {
				if output.send(event).await.is_err() {
					break; // GUI dropped the subscription; stop forwarding.
				}
			}
		},
	)
}

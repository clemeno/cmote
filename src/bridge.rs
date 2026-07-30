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
	/// A private-key file (PEM / OpenSSH / PuTTY `.ppk`), with an OPTIONAL passphrase
	/// pre-seeded from the form (§7, §14). `passphrase`:
	///   * `None` — the field was left empty: keep the original behavior. An encrypted
	///     key prompts interactively (`SshEvent::NeedPassphrase` out,
	///     `SshCommand::Passphrase` back); an unencrypted key never prompts.
	///   * `Some(..)` — try this passphrase first, so a known passphrase unlocks the
	///     key without a prompt; if it is wrong we fall back to prompting.
	///
	/// The passphrase is session-only — it rides in a `Secret` (redacted, wiped on
	/// drop) and is never persisted with the saved target (§12).
	Key {
		path: PathBuf,
		passphrase: Option<Secret>,
	},
	/// Server-driven keyboard-interactive auth (§7): 2FA / OTP and any challenge-response
	/// scheme. It carries NO secret — every prompt is answered live during the handshake, so
	/// there is nothing to pre-seed on the form or persist with the target (§12). cmote also
	/// falls into this method automatically when a password or key attempt leaves
	/// keyboard-interactive as the remaining factor (a second factor, or a fallback), so it
	/// covers both the explicit choice and the implicit continuation.
	Interactive,
}

/// One field of a keyboard-interactive request (§7). It mirrors russh's `Prompt` in a type
/// the GUI owns and can move across the channel: `label` is the server's caption for the
/// field ("Password:", "Verification code:"), and `echo` is its hint about visibility —
/// `true` for a value safe to show (a username), `false` for a secret (a password / OTP) the
/// field must mask.
#[derive(Debug, Clone)]
pub struct InteractivePrompt {
	pub label: String,
	pub echo: bool,
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
	/// The user's answers to a keyboard-interactive request (§7), one per prompt in the same
	/// order the request listed them. Each rides in a `Secret` so an OTP or password is
	/// redacted in logs and wiped on drop — even the echoed prompts, which costs nothing and
	/// keeps the type uniform.
	Interactive(Vec<Secret>),
	/// Raw keyboard bytes to send down the channel (keystroke, escape seq, ...).
	Input(Vec<u8>),
	/// The terminal view changed size; reflow the remote pty.
	Resize { cols: u16, rows: u16 },
	/// Upload a local file to the remote over SFTP (§17). `remote` is the destination
	/// path the user confirmed — absolute when the shell's cwd is known, otherwise
	/// relative, which the server resolves against the login directory. `overwrite` is
	/// false on the first attempt: the task then reports `UploadExists` instead of
	/// clobbering a file, and the GUI re-sends with `true` only if the user confirms.
	Upload {
		local: PathBuf,
		remote: String,
		overwrite: bool,
	},
	/// Check, before an upload batch sends a single byte, which of `names` already exist
	/// in the remote directory `dir` (§17). The task answers with `UploadPrescan`, so the
	/// GUI can ask the "some are already there" question once for the whole batch — the
	/// same up-front collision model the multi-file download uses (§21). For each name that
	/// clashes the task also proposes a free `name-1` alternative, so the "keep both" answer
	/// has a server-checked destination to write to.
	CheckUploads { dir: String, names: Vec<String> },
	/// List the folders inside a remote directory, for the explorer tree (§18). One
	/// command per folder the user opens — the tree is lazy, so nothing is walked.
	ListDir(String),
	/// List EVERY entry inside a remote directory, for the files pane (§19). `request`
	/// comes back on each batch so the pane can tell a listing it still wants from one
	/// for a directory it has already left.
	ListFiles { path: String, request: u64 },
	/// Fetch a remote file to a local path the user picked in the save dialog (§19).
	Download { remote: String, local: PathBuf },
	/// Resolve one symlink for the files pane's details popup (§20). Sent when a link is
	/// selected — one round trip for the entry being looked at, rather than one per link
	/// in the listing.
	ReadLink(String),
	/// Rename a remote folder (§18). `to` is the same directory with a new last
	/// component; the task refuses to replace an occupied path.
	RenameDir { from: String, to: String },
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
	/// The server posed a keyboard-interactive challenge (§7): `name` and `instructions` are
	/// its optional heading and blurb (either may be empty), and `prompts` the fields to
	/// answer. The GUI shows a prompt dialog and sends the answers back as
	/// `SshCommand::Interactive`. One of these arrives per request; a server can send several
	/// in a row (password, then a one-time code), so the dialog reappears until auth resolves.
	Interactive {
		name: String,
		instructions: String,
		prompts: Vec<InteractivePrompt>,
	},
	/// Authentication succeeded and a shell is open — switch to the terminal.
	Connected,
	/// A chunk of terminal output to feed the vt100 parser (§9).
	Output(Vec<u8>),
	/// The upload's destination already holds a file (§17). Carries the path, so the
	/// GUI can name it in the overwrite confirmation; nothing has been written.
	UploadExists(String),
	/// The answer to a `CheckUploads` (§17): the entries of the batch that already exist in
	/// the destination, each paired with a free `name-1`-style path to write to if the user
	/// chooses "keep both". Empty when nothing clashes — the batch then starts straight away.
	/// The names not listed here are free, so the GUI needs no per-file reply for them.
	UploadPrescan { collisions: Vec<(String, String)> },
	/// Bytes moved so far, out of the file's size (§17, §19). Sent at intervals, not per
	/// chunk, so a big transfer does not flood the GUI with redraws. Shared by uploads
	/// and downloads — only one transfer runs at a time, and the bar reads the same
	/// either way.
	TransferProgress { sent: u64, total: u64 },
	/// The upload finished; carries the destination path as the server resolved it.
	UploadDone(String),
	/// The upload failed; carries a short reason to show in the status bar (§17).
	UploadFailed(String),
	/// The file landed on this machine (§19); carries the local path it was saved to.
	DownloadDone(String),
	/// The download failed; carries a short reason for the status bar (§19).
	DownloadFailed(String),
	/// The folders inside `path`, for the explorer tree (§18). Names only, not paths —
	/// the tree already knows the parent it asked about.
	DirListed { path: String, dirs: Vec<String> },
	/// One batch of a files-pane listing (§19), in display order. `done` marks the last
	/// batch — including for an empty directory, which sends exactly one empty batch.
	FilesChunk {
		request: u64,
		entries: Vec<crate::files::Entry>,
		done: bool,
	},
	/// A files-pane listing failed (no permission, gone, the server refused). Carries the
	/// request number so a failure for a directory the user has left is dropped (§19).
	FilesFailed { request: u64, reason: String },
	/// The remote machine's timezone (§20), from one `date` probe per session. Every mtime
	/// in the pane is rendered against it, so it arrives once and applies to all of them.
	Zone(crate::files::Zone),
	/// Where a selected symlink points (§20). Carries the link's own path so an answer for
	/// a link the selection has moved off is recognisable.
	LinkTarget { path: String, target: String },
	/// A directory could not be listed (no permission, gone, the server refused). Carries
	/// the path so the tree can stop waiting on that folder, and a reason for its notice
	/// line — the path is the user's own, so naming it is what makes it actionable (§17).
	DirFailed { path: String, reason: String },
	/// A folder was renamed (§18); the tree re-lists the parent and follows the new path.
	RenameDone { from: String, to: String },
	/// The rename did not happen, with the reason for the panel's notice line.
	RenameFailed(String),
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

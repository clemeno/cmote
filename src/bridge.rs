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

use crate::forward::ForwardSpec;
use crate::secret::Secret;
use crate::ssh;

/// Bounded-channel capacity. Bounded so a flood of terminal output can't grow
/// memory without limit — the producer awaits when the consumer falls behind
/// (backpressure, §4). `ponytail:` a generous fixed bound; tune only if needed.
const CHANNEL_BOUND: usize = 256;

/// The identity of the shell a session opens for the account it authenticated as (§45). Every
/// session has exactly this one to begin with, it is never elevated and never closes before the
/// connection does — so it is a fixed number both sides can name, rather than something the GUI
/// has to be told. Elevated identities are numbered from 1 upward by the tab that opens them.
pub const LOGIN_IDENTITY: u64 = 0;

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
	///
	/// `certificate` is an OPTIONAL OpenSSH user certificate (`<key>-cert.pub`) to present
	/// alongside the key (§7). An SSH certificate is a public key signed by a trusted CA, so
	/// the server trusts the one CA rather than every individual key. It is an add-on to key
	/// auth, not a method of its own: the private key here still does the signing — the
	/// certificate is the extra blob sent with the offer.
	///   * `None` — plain public-key auth, exactly as before.
	///   * `Some(path)` — load the certificate from `path` and authenticate with the
	///     key-and-certificate pair (russh's `authenticate_openssh_cert`).
	///
	/// A certificate is public data (like the key *path*), so — unlike the passphrase — it is
	/// remembered with the saved target (§14).
	Key {
		path: PathBuf,
		passphrase: Option<Secret>,
		certificate: Option<PathBuf>,
	},
	/// Server-driven keyboard-interactive auth (§7): 2FA / OTP and any challenge-response
	/// scheme. It carries NO secret — every prompt is answered live during the handshake, so
	/// there is nothing to pre-seed on the form or persist with the target (§12). cmote also
	/// falls into this method automatically when a password or key attempt leaves
	/// keyboard-interactive as the remaining factor (a second factor, or a fallback), so it
	/// covers both the explicit choice and the implicit continuation.
	Interactive,
	/// Public-key auth delegated to a running SSH agent — the Windows OpenSSH agent or Pageant
	/// on Windows, `ssh-agent` (via `SSH_AUTH_SOCK`) on macOS (§7). It carries NO secret and NO
	/// file path: the agent holds the private keys already unlocked and does the signing itself,
	/// so cmote never sees the key material. Nothing to pre-seed on the form or persist with the
	/// target (§12); the connection just asks the agent to sign the auth challenge.
	Agent,
}

/// The user's answer to a host-key prompt (§8). A first-contact UNKNOWN key offers only
/// `Reject` / `Pin` (accept and remember it); a CHANGED key — the mismatch dialog — offers all
/// three, because trusting a changed key *once*, without pinning, is a distinct and safer choice
/// when the change might be transient or you cannot verify it yet. The SSH task reads this against
/// the verdict it is blocked on: `Pin` learns a new key or REPLACES the stale line of a changed
/// one; `TrustOnce` connects this session without touching `known_hosts`; `Reject` refuses. The
/// safe default — a dismissed dialog, or a GUI that went away — is always `Reject`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostKeyChoice {
	/// Refuse the connection. The safe default: closing the dialog (✕ / backdrop) picks this.
	Reject,
	/// Connect this session only; leave `known_hosts` unchanged, so the same key warns again next
	/// time. Offered for a CHANGED key when the user wants to proceed without committing trust.
	TrustOnce,
	/// Connect and persist the key to `known_hosts` — LEARN it (first contact) or REPLACE the
	/// stale line (a changed key) — so future connections verify against it silently.
	Pin,
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

/// How a recursive transfer settles ONE file whose destination is already taken (§17, §19).
/// A tree upload or download merges into an existing folder, so a colliding *file* stops the
/// transfer and asks the user — the mirror of the flat batch's up-front question, but posed one
/// file at a time because a deep tree cannot be pre-scanned into a single list the user would
/// read. The six answers are the ones a file manager offers: three that settle just this file,
/// two "…all" that settle every collision still to come without asking again, and Cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
	/// Overwrite this one file; keep asking about the next.
	Overwrite,
	/// Write this one to a free `name-1` beside the original; keep asking about the next.
	KeepBoth,
	/// Leave this one alone; keep asking about the next.
	Skip,
	/// Overwrite this one AND every later collision, without asking again (a sticky policy).
	OverwriteAll,
	/// Skip this one AND every later collision, without asking again (a sticky policy).
	SkipAll,
	/// Abandon the whole transfer; files already copied stay where they are.
	Cancel,
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
	/// The user's answer to a host-key prompt (§8): reject, trust just this session, or pin the
	/// key. Answers both the first-contact (`HostKey`) and the mismatch (`HostKeyChanged`) prompts
	/// — the SSH task reads the choice against whichever verdict it is waiting on (§8).
	HostKeyResponse(HostKeyChoice),
	/// The passphrase the user typed after a `NeedPassphrase` prompt, to decrypt
	/// the chosen private key (§7).
	Passphrase(Secret),
	/// The user's answers to a keyboard-interactive request (§7), one per prompt in the same
	/// order the request listed them. Each rides in a `Secret` so an OTP or password is
	/// redacted in logs and wiped on drop — even the echoed prompts, which costs nothing and
	/// keeps the type uniform.
	Interactive(Vec<Secret>),
	/// Raw keyboard bytes to send down the channel (keystroke, escape seq, ...).
	///
	/// Untagged on purpose, even though a session can hold several shells (§45): typing goes to
	/// whichever identity is SELECTED, and the SSH task is told which that is by
	/// `SelectIdentity`. Both ride the same ordered channel, so a switch followed by a keystroke
	/// can never be delivered the other way round — and the GUI never has to name a channel it
	/// does not own.
	Input(Vec<u8>),
	/// The terminal view changed size; reflow the remote pty.
	///
	/// Untagged, and for the opposite reason to `Input`: a resize applies to EVERY shell on the
	/// session (§45). They all draw into the same window, so they all need the same pty size —
	/// including the ones not on screen, or switching to one would show a grid laid out for a
	/// window that no longer exists.
	Resize { cols: u16, rows: u16 },
	/// Open another shell on this connection to become another account (§45): `sudo -u root -i` or
	/// `su - postgres`. `identity` is the number the GUI has assigned it, which every later event
	/// about this shell carries.
	///
	/// The ACCOUNT is sent rather than the command line, and `crate::elevate` builds the command on
	/// the SSH side. Two reasons: the one place that composes a remote command line stays the one
	/// place that vets what goes into it, and the file layer needs the same two values to read files
	/// as that account (§46) — a command string would have to be taken apart again to get them.
	///
	/// No new SSH authentication happens — this is a program run on the existing connection, which
	/// holds its own conversation (a password, perhaps a one-time code) on its own channel. Until
	/// that conversation ends the channel's output is NOT terminal output: it is answered through
	/// `ElevateAnswer` and reported through `ElevatePrompt`.
	Elevate {
		identity: u64,
		kind: crate::elevate::Kind,
		user: String,
	},
	/// One answer to an `ElevatePrompt` (§45), written to that shell's channel followed by a
	/// newline. Rides in a `Secret` so a sudo password or a one-time code is redacted in logs and
	/// wiped on drop (§12), exactly like the answers to an SSH keyboard-interactive request.
	ElevateAnswer { identity: u64, secret: Secret },
	/// Which identity typing now belongs to (§45). Sent when the user picks another account, ahead
	/// of any input for it; the SSH task keeps the number and routes `Input` to that shell.
	SelectIdentity(u64),
	/// The emulator's answer to a status or identity query, written to ONE named shell (§23, §45).
	///
	/// Separate from `Input` because it is not the user typing: a program that sent a query blocks
	/// reading its stdin until the reply arrives, and a background identity's program must be
	/// answered on ITS channel — the typing path would deliver the reply to whichever account the
	/// user happens to be looking at instead.
	Reply { identity: u64, bytes: Vec<u8> },
	/// Close one elevated identity's shell (§45) — EOF on its channel, which ends the login shell
	/// and, with it, the elevation. The login identity is not closable this way; ending it is what
	/// `Disconnect` does.
	CloseIdentity(u64),
	/// Upload a local file to the remote over SFTP (§17). `remote` is the destination
	/// path the user confirmed — absolute when the shell's cwd is known, otherwise
	/// relative, which the server resolves against the login directory. `overwrite` is
	/// false on the first attempt: the task then reports `UploadExists` instead of
	/// clobbering a file, and the GUI re-sends with `true` only if the user confirms.
	/// `resume` continues an interrupted transfer (§16): the task sizes the destination
	/// and appends only the bytes still missing rather than truncating and re-sending.
	Upload {
		local: PathBuf,
		remote: String,
		overwrite: bool,
		resume: bool,
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
	/// Fetch a remote file to a local path the user picked in the save dialog (§19). `resume`
	/// continues an interrupted download (§16): the task reads the remote from the local partial's
	/// size and appends, rather than truncating the local file and pulling it again.
	Download {
		remote: String,
		local: PathBuf,
		resume: bool,
	},
	/// Send a whole local folder to the remote, recreating its tree under `remote` (§17). The
	/// folder keeps its own name inside the destination; missing remote directories are made and
	/// existing ones merged into, and a file that would land on one already there raises a
	/// per-file conflict (`TransferConflict` out, `ResolveConflict` back). `resume` continues an
	/// interrupted tree (§16): each file is size-compared and only the missing tail is sent, with
	/// no collision prompts — an existing destination is the transfer's own earlier work.
	UploadTree {
		local: PathBuf,
		remote: String,
		resume: bool,
	},
	/// Fetch a whole remote folder to this machine, recreating its tree under `local` (§19). The
	/// mirror of `UploadTree`: same merge-and-per-file-conflict behaviour, in the other direction.
	DownloadTree {
		remote: String,
		local: PathBuf,
		resume: bool,
	},
	/// Read a whole remote file into the in-tab text editor (§32). `editor_id` is the EDITOR tab's
	/// id, echoed back on `EditLoaded` / `EditLoadFailed` so the reply routes to the tab that asked
	/// — not the session tab whose channel carried it (an editor has no channel of its own). The
	/// whole file is one in-memory buffer, so the read is bounded by `edit::MAX_SIZE`.
	///
	/// `identity` names the ACCOUNT to read as (§46), rather than letting the read follow whichever
	/// account the panes are showing: a file opened as root belongs to that account for as long as
	/// the editor tab lives, and the save has to reach the same file it came from.
	EditLoad {
		identity: u64,
		editor_id: u64,
		path: String,
	},
	/// Write the editor's buffer back to the remote (§32). `editor_id` routes the reply; `path` is
	/// the destination (a Save As names a new one); `bytes` are already encoded as the file was
	/// opened (BOM and all — the GUI side owns the encoding). Written atomically: a temp sibling
	/// then a rename over the target, so a drop mid-write cannot truncate the user's file.
	///
	/// `identity` is the account the file was opened as (§46), carried so a save lands as the same
	/// account that read it — root-owned files stay writable, and nothing is written as an account
	/// that only happens to be on screen now.
	EditSave {
		identity: u64,
		editor_id: u64,
		path: String,
		bytes: Vec<u8>,
	},
	/// Find out whether the LOGIN account's shell announces its working directory, and where its
	/// config file is (§17). `user` is the account name, which the task matches against
	/// `/etc/passwd` to learn the login shell — the GUI already knows it from the endpoint, so
	/// sending it saves the session layer from having to remember the connect parameters.
	///
	/// Reads only; nothing is written until the user has seen the block and pressed Install. The
	/// answer is `IntegrationProbed`, or `IntegrationFailed` if the account's home could not even
	/// be resolved.
	ProbeIntegration { user: String },
	/// Write the shell-integration block into `path`, or cut it back out (§17). `install` says
	/// which — one command for both directions, because they are the same read-modify-write with a
	/// different edit in the middle, and the dialog offers exactly one of them at a time.
	///
	/// `shell` decides which block goes in; it is ignored when removing, since the markers bound
	/// the block whichever shell wrote it. Answered with `IntegrationWritten` or
	/// `IntegrationFailed`.
	WriteIntegration {
		path: String,
		shell: crate::integration::Shell,
		install: bool,
	},
	/// The user's answer to a recursive transfer's file-collision prompt (§17, §19). Routed to
	/// the transfer waiting on it; a `*All` answer makes the task stop asking for the rest.
	ResolveConflict(ConflictChoice),
	/// Stop the transfer running right now (§16) — the status bar's ✕. The task sets a flag its
	/// copy loop checks between chunks; on seeing it, the loop deletes the partial it was writing
	/// (a deliberate cancel is final, unlike a failure) and reports the neutral "cancelled"
	/// outcome. A no-op when nothing is transferring.
	CancelTransfer,
	/// Create a new empty folder on the server (§18). `path` is the full path of the folder to
	/// make; the task refuses to replace anything already sitting there.
	MakeDir(String),
	/// Delete remote entries (§18). Each path is removed whatever it is — a file, a symlink, or a
	/// folder and everything inside it (a recursive walk). Not undoable, so the GUI only sends
	/// this after an explicit confirmation naming the targets.
	Delete(Vec<String>),
	/// Resolve one symlink for the files pane's details popup (§20). Sent when a link is
	/// selected — one round trip for the entry being looked at, rather than one per link
	/// in the listing.
	ReadLink(String),
	/// Rename a remote folder (§18). `to` is the same directory with a new last
	/// component; the task refuses to replace an occupied path.
	RenameDir { from: String, to: String },
	/// Start a port forward on the live connection (§27). `id` is the app-assigned handle used
	/// to report the outcome back (`ForwardReady` / `ForwardFailed`) and to cancel it later
	/// (`RemoveForward`); `spec` says which of the three shapes it is and where it binds/goes.
	/// A Local/Dynamic forward binds a local listener; a Remote forward asks the server to bind
	/// one. The tunnels run on the same SSH connection as the shell — no new authentication.
	AddForward { id: u64, spec: ForwardSpec },
	/// Tear down the forward with this id (§27). The local listener (Local/Dynamic) stops
	/// accepting and its in-flight tunnels are dropped; a Remote forward's server listener is
	/// cancelled. A missing id is a no-op.
	RemoveForward(u64),
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
	/// The server's host key does NOT match the one pinned for it (§8) — key rotation, or a
	/// man-in-the-middle. The GUI shows a loud override dialog with BOTH SHA-256 fingerprints so
	/// the change can be judged out-of-band: `stored` is what was trusted before, `presented` is
	/// what the server sent now. The user's choice comes back as `HostKeyResponse` (reject / trust
	/// once / replace); dismissing rejects. A changed key is never auto-trusted.
	HostKeyChanged { stored: String, presented: String },
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
	/// A chunk of terminal output to feed the vt100 parser (§9), and which shell said it (§45).
	///
	/// Tagged because a session can hold several shells at once and the ones NOT on screen keep
	/// running: a build left in the login shell must go on filling that shell's scrollback while
	/// the user works in root's. Untagged bytes would land in whichever grid happened to be
	/// showing, which is the one thing a scrollback must never do.
	Output { identity: u64, bytes: Vec<u8> },
	/// An elevating shell is asking a question (§45): sudo's password prompt, or a second factor a
	/// PAM module posed. `label` is the remote's own wording, stripped of escape sequences and
	/// capped in length (`crate::elevate`), so the dialog asks exactly what the remote asked.
	///
	/// Answered with `SshCommand::ElevateAnswer`. Several of these arrive for one elevation, and for
	/// two quite different reasons: another factor is being asked for, or the last answer was
	/// refused and the question is being put again. `refusal` is which — the program's own words
	/// about the previous answer, taken from what it printed between it and this question, and `None`
	/// when it printed no such thing. The GUI cannot tell them apart from the wording alone: sudo
	/// dresses every standard prompt in the stack in its own `-p` text, so a password and a second
	/// factor can arrive under one label.
	ElevatePrompt {
		identity: u64,
		label: String,
		refusal: Option<String>,
	},
	/// An elevated shell is through its conversation and is now a live terminal (§45): its output
	/// is ordinary output from here on, and typing may be routed to it.
	IdentityReady { identity: u64 },
	/// An elevated shell has gone (§45). `reason` is `Some` when it never opened or died on a
	/// failure — the last thing the program said, which is the remote's own words about its own
	/// policy ("not in the sudoers file", "3 incorrect password attempts") — and `None` when it
	/// simply exited, which is what typing `exit` at an elevated prompt does.
	///
	/// The login identity never sends this; the session ending is `Disconnected`.
	IdentityEnded {
		identity: u64,
		reason: Option<String>,
	},
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
	/// A file was read for the editor (§32). `editor_id` is the tab that asked, `path` the file,
	/// and `bytes` its raw contents — the GUI side decodes them (BOM / UTF detection lives there,
	/// so the network layer stays encoding-agnostic).
	EditLoaded {
		editor_id: u64,
		path: String,
		bytes: Vec<u8>,
	},
	/// The editor load failed (§32): the file is over `edit::MAX_SIZE`, could not be read, or the
	/// sftp channel would not open. The editor tab shows the reason in place of a buffer.
	EditLoadFailed { editor_id: u64, reason: String },
	/// The editor's buffer was saved to `path` (§32); the tab clears its dirty marks and, on a
	/// Save As, is now editing the new file.
	EditSaved { editor_id: u64, path: String },
	/// The editor save failed (§32): the buffer stays dirty and the reason is shown, so the edits
	/// that failed to persist are never thrown away.
	EditSaveFailed { editor_id: u64, reason: String },
	/// What the login account's shell config looks like (§17), in answer to `ProbeIntegration`.
	/// `shell` is the family read out of `/etc/passwd` — `None` when the account is not a local one
	/// or names a shell cmote has no block for, in which case the dialog says so and offers
	/// nothing. `path` is the config file that would be written, and `installed` whether cmote's
	/// block is already in it, which is what makes the dialog offer Remove instead of Install.
	IntegrationProbed {
		shell: Option<crate::integration::Shell>,
		path: String,
		installed: bool,
	},
	/// The config file was written (§17). `installed` is its state AFTER the write — true for an
	/// install, false for a removal — so the dialog reports what is now true rather than what was
	/// asked for.
	IntegrationWritten { path: String, installed: bool },
	/// The probe or the write did not happen (§17), with the server's own reason. Never tears the
	/// session down: this is a side errand on its own channel, and a remote that refuses it is
	/// simply a remote where the cwd stays unknown.
	IntegrationFailed(String),
	/// A transfer stopped mid-flight on a failure, but its partial was KEPT so it can be resumed
	/// (§16). Distinct from `*Failed` (final, nothing to continue) and from a cancel (which
	/// deletes its partial): the GUI shows `message` and offers a Resume, which re-runs the same
	/// transfer with `resume` set so it appends only the bytes still missing. Shared by both
	/// directions — the GUI remembers which one it launched, so this carries no path of its own.
	TransferInterrupted { message: String },
	/// A recursive transfer hit a file whose destination is already taken (§17, §19). Carries
	/// the entry's name to show; the GUI raises the six-way conflict dialog and sends the answer
	/// back as `ResolveConflict`. The transfer is parked until it arrives, so the shell keeps
	/// flowing behind the prompt. Only a per-FILE collision asks — directories merge and a sticky
	/// "…all" answer settles every later one without another of these.
	TransferConflict { name: String },
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
	/// A new folder was created (§18); carries its full path so both panels re-list the parent
	/// it appeared in and the row shows up in the right sort position.
	MakeDirDone(String),
	/// The folder was not created, with the reason for the panel's notice line.
	MakeDirFailed(String),
	/// Remote entries were deleted (§18); carries the paths that were removed so both panels
	/// re-list the parents they vanished from — and step a pane out of a folder that is now gone.
	DeleteDone(Vec<String>),
	/// A delete did not happen (or only partly did), with the reason for the panel's notice line.
	DeleteFailed(String),
	/// A port forward is up (§27): its local listener bound, or the server accepted its remote
	/// listen request. Carries the id so the GUI marks the right row live in the tunnels dialog,
	/// and `assigned_port` — `Some` only for a `-R 0`, the port the server chose, so the row can
	/// show where it is actually listening; `None` for every forward that named its own port.
	ForwardReady { id: u64, assigned_port: Option<u16> },
	/// A port forward could not start (§27): the local port was already taken, the server
	/// refused the remote listen, or the bind address was bad. Carries the id and a short reason
	/// for the row — a forward's own failure never tears the shell down, unlike a session error.
	ForwardFailed { id: u64, reason: String },
	/// A connection began flowing through forward `id` (§27): a client dialed a local/dynamic
	/// listener, or the server opened a `forwarded-tcpip` channel for a remote one and the dial
	/// succeeded. The tunnels dialog raises that row's live "open" and cumulative "total" counts.
	ForwardConnectionOpened { id: u64 },
	/// A connection through forward `id` ended (§27): its byte pump finished. The dialog lowers
	/// that row's live "open" count by one; the cumulative total stays, as a record of traffic seen.
	ForwardConnectionClosed { id: u64 },
	/// The session ended (server closed, or user disconnected).
	Disconnected,
	/// Something failed. A generic, non-leaking message (§12).
	Error(String),
}

impl SshEvent {
	/// The EDITOR tab an Edit* event is destined for (§32), or `None` for every other event. The
	/// worker tags its whole stream with the SESSION tab's id (§26), but a file loaded or saved for
	/// the editor belongs to the EDITOR tab that asked — so `App` routes these four by this id
	/// instead, whichever session's channel carried them.
	pub fn editor_target(&self) -> Option<u64> {
		match self {
			Self::EditLoaded { editor_id, .. }
			| Self::EditLoadFailed { editor_id, .. }
			| Self::EditSaved { editor_id, .. }
			| Self::EditSaveFailed { editor_id, .. } => Some(*editor_id),
			_ => None,
		}
	}
}

/// Build the SSH-event subscription for ONE tab's session (§4, §26). iced identifies a
/// subscription by the `(data, builder)` pair, so passing the tab's id keys a DISTINCT worker
/// per tab: each starts its own network thread and its own `run` loop, and lives exactly as long
/// as its tab is in the batch — a closed tab drops out and its worker is torn down. The id is
/// only an identity here; `App` tags this stream's events with the same id via `.map` so it can
/// route them back to the tab that owns the session (a background tab keeps receiving output).
pub fn session_subscription(id: u64) -> Subscription<SshEvent> {
	Subscription::run_with(id, worker)
}

/// The worker stream. Runs on iced's executor; its job is only to *shuttle*
/// events — the real network I/O runs on a separate tokio runtime thread (§4). The `_id` is
/// unused by the logic; it is part of the subscription's identity so each tab gets its own
/// worker (§26).
fn worker(_id: &u64) -> impl Stream<Item = SshEvent> + use<> {
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

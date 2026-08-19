// ssh/asuser.rs — reading and writing files as an account the session did not log in as (PLAN §46).
//
// §45 gave a session more than one SHELL: elevation is a program (`sudo -u root -i`) run on the
// connection, holding its own conversation on its own channel. The file panes were left behind by
// that, and not by accident — they do not go through a shell at all. The tree, the pane, every
// transfer and the editor all speak SFTP, and SFTP is a SUBSYSTEM: sshd starts it, once, as the
// account that authenticated. No amount of sudo inside a shell can reach it, which is why a root
// terminal used to sit beside login-user file panes.
//
// The way round it is to stop asking sshd for the subsystem and run the same program ourselves:
//
//   * **`sudo -u root -- /usr/lib/openssh/sftp-server` on an exec channel.** `sftp-server` speaks
//     the SFTP protocol on its stdin and stdout, which is exactly what the subsystem gives us — so
//     every existing feature works unchanged over it, as another account. cmote has to find the
//     binary (packaging moves it) and authenticate sudo without a terminal.
//   * **Shell commands as a fallback** (`shellfs`), for a remote with no `sftp-server` binary to
//     run: `ls`, `cat`, `mkdir` under the same sudo. Less faithful — text instead of a typed
//     listing — but it needs nothing but a shell.
//   * **Nothing, said plainly.** Where neither works (sudoers refuses, `su` needs a terminal),
//     every operation fails with the remote's own reason and the panes list nothing. They never
//     quietly show the login account's files while the terminal is root: a file pane that lies
//     about whose eyes it is using is worse than an empty one.
//
// Two rules in here carry the security weight, and both are about the password:
//
//   1. It is written ONLY after a `-n` (non-interactive) attempt has been refused for the want of
//      it. sudo with a valid credential does not read stdin at all, so a password sent on a guess
//      would not go to sudo — it would go to whatever sudo exec'd, as that program's input.
//   2. It reaches `sudo` and nothing else. The command line holds only an account name and a path,
//      each vetted by `crate::elevate` (`valid_user`, `valid_program`) rather than merely quoted,
//      and the secret travels as channel data, never as an argument — so it cannot appear in a
//      process list, a shell history, or a log.
//
// One structural rule, which is not about security but will deadlock the session if broken: a
// [`Runner`] asks the SESSION LOOP for its channels (russh's handle is not `Clone`, so a spawned
// task cannot open one itself). So a runner may only ever be awaited from a spawned task. Work that
// happens INLINE in the loop — discovery, opening an sftp session — uses [`exec_inline`], which
// opens its channel from the handle the loop is holding.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, anyhow, bail};
use russh::{Channel, ChannelMsg, client};
use russh_sftp::client::{RawSftpSession, SftpSession};
use tokio::sync::{mpsc, oneshot};

use crate::bridge::LOGIN_IDENTITY;
use crate::elevate::{self, ElevateKind};
use crate::secret::Secret;

/// How much output one shell-backend command may produce. A listing, a `wc -c` or a `find` is
/// small; a command answering with something that is not what we asked for must not grow our
/// memory without bound (§12). File CONTENT does not come through here — it streams (`shellfs`).
const MAX_OUTPUT: usize = 1024 * 1024;

/// How much of a command's stderr is kept for its failure message. A reason is one short line;
/// this is generous for a few of them.
const MAX_STDERR: usize = 4096;

/// A task's request for a fresh channel on the live session.
///
/// russh's session handle is not `Clone` (it owns the receiving end of the session's own replies),
/// so a spawned task CANNOT open a channel for itself — only the select loop that holds the handle
/// can. A shell-backend operation needs one channel per command, which is why this exists: the
/// task sends a request, the loop opens a channel and sends it back down the one-shot.
pub struct ChannelRequest(pub oneshot::Sender<Result<Channel<client::Msg>, String>>);

/// A cloneable way to ask the session loop for a channel — the task-side half of
/// [`ChannelRequest`].
#[derive(Clone)]
pub struct Channels(mpsc::Sender<ChannelRequest>);

impl Channels {
	/// Make the pair: this handle for the tasks, and the receiver the session loop serves.
	pub fn new() -> (Self, mpsc::Receiver<ChannelRequest>) {
		let (tx, rx) = mpsc::channel::<ChannelRequest>(16);
		(Self(tx), rx)
	}

	/// One fresh channel on the live session. Fails when the session loop has gone (the connection
	/// is tearing down) or when the server refuses another channel.
	///
	/// Never call this from the session loop itself: the loop is what answers it.
	pub async fn open(&self) -> Result<Channel<client::Msg>> {
		let (reply_tx, reply_rx) = oneshot::channel();
		self.0
			.send(ChannelRequest(reply_tx))
			.await
			.map_err(|_| anyhow!("the session is closing"))?;
		reply_rx
			.await
			.map_err(|_| anyhow!("the session is closing"))?
			.map_err(|error| anyhow!("{error}"))
	}
}

/// One command's result: what it printed, and whether it worked.
pub struct Output {
	pub stdout: String,
	pub stderr: String,
	pub status: Option<u32>,
}

impl Output {
	/// Whether the command reported success. A missing exit status counts as success: some servers
	/// close the channel without sending one, and the alternative — calling every such command a
	/// failure — would break listings on those servers.
	pub fn ok(&self) -> bool {
		self.status.is_none_or(|code| code == 0)
	}

	/// The remote's own words about the failure, for a notice line. Its stderr when it wrote any,
	/// otherwise a plain statement — never an empty message.
	pub fn reason(&self) -> String {
		let trimmed = self.stderr.trim();
		if trimmed.is_empty() {
			"the command failed".to_owned()
		} else {
			trimmed.to_owned()
		}
	}
}

/// Whether a failure is sudo saying "I would need a password for that" — the ONLY thing that
/// permits the cached one to be written (see the module note).
///
/// Matched on sudo's own wording, in the several shapes it uses across versions. A miss is safe:
/// the operation fails and the user is told, exactly as if there were no password to try.
fn wants_password(stderr: &str) -> bool {
	let lowered = stderr.to_lowercase();
	lowered.contains("password is required")
		|| lowered.contains("no password was provided")
		|| lowered.contains("no tty present and no askpass")
}

/// Read one running command's channel to the end.
///
/// Shared by the task-side [`Runner`] and the inline [`exec_inline`] so both judge an exit status,
/// a stderr cap and an output cap the same way.
async fn collect(channel: &mut Channel<client::Msg>) -> Result<Output> {
	let mut stdout: Vec<u8> = Vec::new();
	let mut stderr = String::new();
	let mut status = None;
	while let Some(message) = channel.wait().await {
		match message {
			ChannelMsg::Data { data } => {
				stdout.extend_from_slice(&data);
				if stdout.len() > MAX_OUTPUT {
					bail!("the answer is too large to read");
				}
			}
			ChannelMsg::ExtendedData { data, .. } => {
				if stderr.len() < MAX_STDERR {
					stderr.push_str(&String::from_utf8_lossy(&data));
				}
			}
			ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
			ChannelMsg::Eof | ChannelMsg::Close => break,
			_ => {}
		}
	}
	Ok(Output {
		stdout: String::from_utf8_lossy(&stdout).into_owned(),
		stderr,
		status,
	})
}

/// Run one command from INSIDE the session loop, which is the only place holding russh's session
/// handle. Used for the few things that must be settled before a task can be spawned — finding the
/// `sftp-server` binary — and for nothing else: it blocks the shell pump for the command's round
/// trip, so it must stay small and rare.
async fn exec_inline(
	session: &client::Handle<super::client::Handler>,
	command: String,
) -> Result<Output> {
	let mut channel = session
		.channel_open_session()
		.await
		.context("could not open a channel")?;
	channel
		.exec(true, command.as_bytes())
		.await
		.context("the server refused to run a command")?;
	collect(&mut channel).await
}

/// Who a file operation runs as, and how to make that happen — cloneable, so it can be moved into
/// the spawned task that does the work.
///
/// The login account's runner is the plain one: it wraps nothing, so every command is exactly what
/// Running one shell snippet on the remote and hearing what it said (§46).
///
/// Three questions, which is all the shell backend's directory listings, metadata reads and
/// mutations ever ask: what did it print, did it print it successfully, and did it work at all.
/// [`Runner`] is the real implementation — the one that opens a channel and wraps the snippet in an
/// elevation — and `shellfs` is written against the trait instead, so a test can drive `ls -1Ap`
/// and `wc -c` with canned output and assert both the command that was composed AND the parse of
/// the reply. Neither was reachable before: every one of those functions named `Runner`, and a
/// `Runner` that will answer needs a live session.
///
/// **`stream` is deliberately NOT on here**, and that is the whole boundary. The operations that
/// stream bytes — a file read through `cat`, a file written through `cat >`, and the two copy loops
/// behind them — keep taking a concrete `Runner`, because a stream hands back a `russh::Channel`
/// and putting a foreign, non-constructible type on the trait would make the trait unimplementable
/// by anything but the real thing, which is the opposite of the point. It is also the line
/// `shellfs`'s own `ponytail:` note draws: making the COPY LOOPS generic over a filesystem would
/// mean rewriting working transfer, resume and conflict code with no way to test it against a real
/// server. That refusal still stands, unchanged. This trait sits entirely on the other side of it —
/// the commands whose whole content is a string and a reply.
///
/// `async fn` in a trait, so it is not `dyn`-compatible; every caller is generic (`&impl Exec`) and
/// dispatches statically, which is what the callers wanted anyway.
#[allow(async_fn_in_trait)]
pub trait Exec {
	/// A snippet's output, or an error carrying the remote's own reason. The shape most callers
	/// want: a listing either arrives or explains itself.
	async fn stdout(&self, snippet: &str) -> Result<String>;

	/// Whether the snippet ran cleanly — for a question only ever asked as a yes or no. A failure
	/// to even run it is `false`, not an error: the question was about the remote's state, and
	/// "could not ask" is not "yes".
	async fn succeeds(&self, snippet: &str) -> bool;
}

impl Exec for Runner {
	async fn stdout(&self, snippet: &str) -> Result<String> {
		Runner::stdout(self, snippet).await
	}

	async fn succeeds(&self, snippet: &str) -> bool {
		Runner::succeeds(self, snippet).await
	}
}

/// cmote has always sent. An elevated runner carries the account, the kind of elevation, and the
/// password to authenticate it with when sudo asks for one.
#[derive(Clone)]
pub struct Runner {
	channels: Channels,
	/// `None` for the account the session logged in as: nothing to elevate into.
	account: Option<Account>,
}

/// The account an elevated runner becomes.
#[derive(Clone)]
struct Account {
	kind: ElevateKind,
	user: String,
	/// The password for this elevation, kept in memory for the connection (§45). `None` when the
	/// user supplied none, and then sudo only ever runs non-interactively.
	secret: Option<Secret>,
	/// Whether sudo has been OBSERVED to want a password. Shared between every clone of this
	/// runner, so the refusal is paid for once rather than by each operation in turn.
	needs_password: Arc<AtomicBool>,
}

impl Runner {
	/// The runner for the account the session authenticated as.
	pub fn login(channels: Channels) -> Self {
		Self {
			channels,
			account: None,
		}
	}

	/// The runner for another account on the same connection (§45, §46).
	pub fn elevated(
		channels: Channels,
		kind: ElevateKind,
		user: String,
		secret: Option<Secret>,
	) -> Self {
		Self {
			channels,
			account: Some(Account {
				kind,
				user,
				secret,
				needs_password: Arc::new(AtomicBool::new(false)),
			}),
		}
	}

	/// Whether this is the login account — the case where nothing is wrapped and the sftp
	/// subsystem is asked for as usual.
	pub fn is_login(&self) -> bool {
		self.account.is_none()
	}

	/// Remember the password this elevation was answered with (§45): the one the user typed to open
	/// its shell, which is the same one sudo wants on a file channel.
	pub fn set_secret(&mut self, secret: Secret) {
		if let Some(account) = self.account.as_mut() {
			account.secret = Some(secret);
		}
	}

	/// Run a shell snippet as this account and collect what it printed. The inherent form of
	/// [`Exec::output`]; the trait exists so the callers that only ever ask these three questions
	/// can be driven by something other than a live connection.
	///
	/// The retry is the password rule in code: the first attempt is always non-interactive, and
	/// only a refusal that names the missing password earns a second attempt with it. A `-S`
	/// attempt that fails for any other reason clears the verdict, so a credential that has since
	/// expired is re-learned rather than assumed for the rest of the session.
	pub async fn output(&self, snippet: &str) -> Result<Output> {
		let first = self.attempt(snippet, self.armed()).await?;
		if first.ok() || self.armed() || !self.has_secret() || !wants_password(&first.stderr) {
			return Ok(first);
		}
		let Some(account) = self.account.as_ref() else {
			return Ok(first);
		};
		self.remember_password();
		let second = self.attempt(snippet, true).await?;
		if !second.ok() && !wants_password(&second.stderr) {
			// It was not the password after all (sudoers refuses, the program is missing): forget the
			// verdict rather than keep writing a secret that is not what stands in the way.
			account.needs_password.store(false, Ordering::Relaxed);
		}
		Ok(second)
	}

	/// A shell snippet's output, or an error carrying the remote's own reason. The shape most
	/// callers want: a listing either arrives or explains itself.
	pub async fn stdout(&self, snippet: &str) -> Result<String> {
		let output = self.output(snippet).await?;
		if output.ok() {
			Ok(output.stdout)
		} else {
			bail!("{}", output.reason())
		}
	}

	/// Whether this snippet ran cleanly — for a test that is only ever asked as a yes or no
	/// (`[ -e path ]`). A failure to even run the command is `false`, not an error: the caller's
	/// question was about the remote's state, and "could not ask" is not "yes".
	pub async fn succeeds(&self, snippet: &str) -> bool {
		matches!(self.output(snippet).await, Ok(output) if output.ok())
	}

	/// Start a shell snippet as this account and hand back its live channel, for the operations
	/// that STREAM rather than collect: a file's bytes out (`cat`) or in (`cat > path`).
	///
	/// The password, when one is needed, has already been written as the first line by the time
	/// this returns — sudo reads it a byte at a time and stops at the newline, so everything the
	/// caller reads or writes afterwards belongs to the program sudo exec'd.
	///
	/// A stream cannot carry the retry that `output` does: there is no exit status to read before its
	/// bytes are wanted, and by the time sudo's refusal is visible the caller's first chunk has gone.
	/// So the password question is settled BEFORE the stream starts, with one throwaway command —
	/// and only when a password is held and sudo has not yet been seen to ask for it, which is at
	/// most once per account per connection. That command goes out unarmed like any other, so the
	/// rule still holds: nothing is written until sudo has refused for the want of it.
	pub async fn stream(&self, snippet: &str) -> Result<Channel<client::Msg>> {
		if self.has_secret() && !self.armed() {
			let _ = self.output("exit 0").await;
		}
		let armed = self.armed();
		let channel = self.channels.open().await?;
		channel
			.exec(true, self.command(snippet, armed).as_bytes())
			.await
			.context("the server refused to run a command")?;
		if armed {
			self.write_password(&channel).await?;
		}
		Ok(channel)
	}

	/// Record that sudo has asked for a password (§46) — the one thing that arms later attempts.
	///
	/// Only ever called from an observed refusal, never from a guess: that is what keeps a password
	/// from being written to a sudo which never wanted one. Shared between every clone of this
	/// runner, so the refusal is paid for once rather than by each operation in turn.
	fn remember_password(&self) {
		if let Some(account) = self.account.as_ref() {
			account.needs_password.store(true, Ordering::Relaxed);
		}
	}

	/// Whether the password should be written on the next attempt: only when one is held AND sudo
	/// has been seen to ask for it.
	fn armed(&self) -> bool {
		self.account.as_ref().is_some_and(|account| {
			account.secret.is_some() && account.needs_password.load(Ordering::Relaxed)
		})
	}

	/// Whether a password is held for this account at all.
	fn has_secret(&self) -> bool {
		self.account
			.as_ref()
			.is_some_and(|account| account.secret.is_some())
	}

	/// The command line for a snippet: the snippet itself for the login account, or the same thing
	/// wrapped in the elevation for another.
	fn command(&self, snippet: &str, password: bool) -> String {
		match self.account.as_ref() {
			None => snippet.to_owned(),
			Some(account) => elevate::shell_command(account.kind, &account.user, snippet, password),
		}
	}

	/// Run one command on a fresh channel and read it to the end.
	async fn attempt(&self, snippet: &str, password: bool) -> Result<Output> {
		let mut channel = self.channels.open().await?;
		channel
			.exec(true, self.command(snippet, password).as_bytes())
			.await
			.context("the server refused to run a command")?;
		if password {
			self.write_password(&channel).await?;
		}
		collect(&mut channel).await
	}

	/// Write the cached password to a channel whose sudo has asked for one, newline included.
	async fn write_password(&self, channel: &Channel<client::Msg>) -> Result<()> {
		let Some(secret) = self
			.account
			.as_ref()
			.and_then(|account| account.secret.as_ref())
		else {
			return Ok(());
		};
		let mut line = secret.expose().as_bytes().to_vec();
		line.push(b'\n');
		channel
			.data(&line[..])
			.await
			.context("could not answer sudo")?;
		Ok(())
	}

	/// Start `sftp-server` on an already-open channel as this account, writing the password first
	/// when `armed` — the one program cmote runs directly, with no shell around it.
	///
	/// Inline-safe: the channel is opened by the caller (the session loop), not through `channels`.
	async fn launch_program(
		&self,
		channel: &Channel<client::Msg>,
		program: &str,
		armed: bool,
	) -> Result<()> {
		let Some(account) = self.account.as_ref() else {
			bail!("the login account uses the sftp subsystem");
		};
		let armed = armed && account.secret.is_some();
		if armed {
			// Remembered before the attempt, not after: if this one works, every later operation
			// should go straight to the form that just succeeded.
			self.remember_password();
		}
		let command = elevate::program_command(account.kind, &account.user, program, armed);
		channel
			.exec(true, command.as_bytes())
			.await
			.context("the server refused to run sftp-server")?;
		if armed {
			self.write_password(channel).await?;
		}
		Ok(())
	}
}

/// The file backend for the tree and the files pane: one SFTP session kept for the whole
/// connection, shell commands, or nothing at all (§46).
pub enum Browse {
	/// The typed listing — a directory is a directory because the server said so.
	Sftp(Arc<RawSftpSession>),
	/// `ls` under the elevation. Text, so it is a guess about types and carries no metadata.
	Shell(Runner),
	/// There is no account to read as — it has just gone away. Distinct from a shell fallback whose
	/// commands fail: that one reports the remote's own reason per operation.
	Denied(String),
}

/// The same, for the operations that open a channel of their own per job — transfers and the
/// editor. A fresh `SftpSession` each time, exactly as before §46.
pub enum AsuserFiles {
	Sftp(SftpSession),
	Shell(Runner),
	Denied(String),
}

/// Every account a session can read files as (§46), and which one the panes are showing.
///
/// One entry per identity in §45's sense: the login account (always present, always able) plus each
/// account elevated into. Each remembers what has been LEARNED about it — where the `sftp-server`
/// binary is, whether sudo wants a password, whether SFTP works at all — so the cost of finding out
/// is paid once per connection rather than per click.
pub struct Accounts {
	entries: HashMap<u64, AsuserEntry>,
	/// The login account's runner, kept apart from the entries because a new entry is built from its
	/// channel factory whatever account is selected.
	login: Runner,
	/// The identity whose files the panes are showing. Set by `SelectIdentity`, the same message
	/// that says where typing goes (§45), so a listing asked for after a switch can never be
	/// answered by the account that was showing before it.
	selected: u64,
	/// Whether the remote's timezone has been asked for (§20). The machine's, not an account's, so
	/// it is asked once for the whole session however many accounts are used.
	zone_asked: bool,
}

/// What one account has learned about reading files as itself.
struct AsuserEntry {
	runner: Runner,
	/// The persistent browse session, opened on the first listing and kept — a tree asks many
	/// small questions, and paying a channel setup per click would be felt (§18).
	sftp: Option<Arc<RawSftpSession>>,
	/// The `sftp-server` binary on this remote, once looked for.
	program: Option<String>,
	discovered: bool,
	/// Why SFTP as this account will not work, once that is settled — so the fallback is chosen
	/// once instead of a failed channel being opened per click.
	broken: Option<String>,
	/// Why NOTHING will work as this account, when that is known before anything is tried. Distinct
	/// from `broken`, which sends the work to the shell backend: this refuses both, because both
	/// authenticate the same way and there is no credential to authenticate with.
	denied: Option<String>,
}

impl Accounts {
	/// The set for a fresh session: the login account alone, which is what §45 starts with.
	pub fn new(channels: Channels) -> Self {
		let login = Runner::login(channels);
		let mut entries = HashMap::new();
		entries.insert(LOGIN_IDENTITY, AsuserEntry::new(login.clone()));
		Self {
			entries,
			login,
			selected: LOGIN_IDENTITY,
			zone_asked: false,
		}
	}

	/// Note an account being elevated into (§45), so file operations can run as it. Called when the
	/// elevation is asked for rather than when it succeeds: an identity that never opens is removed
	/// again, and one that does is ready without a second message.
	pub fn add(&mut self, identity: u64, kind: ElevateKind, user: String) {
		let runner = Runner::elevated(self.login.channels.clone(), kind, user, None);
		self.entries.insert(identity, AsuserEntry::new(runner));
	}

	/// Forget an account whose shell has gone (§45). Its sftp session goes with it, which ends the
	/// elevated `sftp-server` on the remote.
	pub fn remove(&mut self, identity: u64) {
		self.entries.remove(&identity);
	}

	/// Remember the password an elevation was answered with (§45), for the file channels that need
	/// the same one. Only ever the password cmote itself asked for by name — never a one-time code,
	/// which is spent the moment it is used.
	pub fn set_secret(&mut self, identity: u64, secret: Secret) {
		if let Some(entry) = self.entries.get_mut(&identity) {
			entry.runner.set_secret(secret);
		}
	}

	/// Say that this account's files are out of reach before anything is attempted (§46), because
	/// logging in as it took more than one factor.
	///
	/// A file channel can replay a password to `sudo -S` and nothing else: it cannot ask for a second
	/// factor (there is no dialog on that side, and the stream carries a binary protocol), and a
	/// one-time code is spent the moment the terminal used it. So the two sftp attempts and the shell
	/// fallback would each authenticate with a credential that cannot be enough — and what the user saw
	/// for it was two ten-second handshake timeouts, a burnt channel apiece, and then empty panes that
	/// said nothing. Knowing it up front turns all of that into one sentence.
	pub fn deny_second_factor(&mut self, identity: u64) {
		if let Some(entry) = self.entries.get_mut(&identity) {
			entry.denied = Some(
				"Logging in as this account needed a second factor. AsuserFiles cannot be read as it: a \
				 file channel can repeat a password to sudo, but it cannot ask for a code."
					.to_owned(),
			);
		}
	}

	/// Point the panes at another account (§46). The selected identity is what every later file
	/// operation runs as.
	pub fn select(&mut self, identity: u64) {
		self.selected = identity;
	}

	/// Whether the timezone probe still has to be sent (§20), marking it asked. The zone belongs to
	/// the machine, so this answers `true` exactly once per session.
	pub fn take_zone_probe(&mut self) -> bool {
		if self.zone_asked {
			return false;
		}
		self.zone_asked = true;
		true
	}

	/// A runner for the work that needs no privilege and no SFTP — the timezone probe (§20).
	///
	/// Deliberately the LOGIN account's, whichever one is selected: the machine's timezone is not a
	/// secret, so asking as the account that is certainly able to run a command means the pane's
	/// times never depend on whether sudo happened to want a password at that moment.
	pub fn login_runner(&self) -> Runner {
		self.login.clone()
	}

	/// The browse backend for the selected account, opening or discovering whatever it needs.
	pub async fn browse(&mut self, session: &client::Handle<super::client::Handler>) -> Browse {
		let Some(entry) = self.entries.get_mut(&self.selected) else {
			return Browse::Denied("That account is no longer open.".to_owned());
		};
		// Refused outright rather than fallen back on: the shell backend authenticates the same way
		// and would fail the same way, one channel and one timeout at a time.
		if let Some(denied) = entry.denied.as_ref() {
			return Browse::Denied(denied.clone());
		}
		if let Some(sftp) = entry.sftp.as_ref() {
			return Browse::Sftp(sftp.clone());
		}
		match entry.open_raw(session).await {
			Ok(sftp) => {
				entry.sftp = Some(sftp.clone());
				Browse::Sftp(sftp)
			}
			Err(reason) => Browse::Shell(entry.fall_back(reason)),
		}
	}

	/// The per-job backend for a transfer or the editor: its own fresh SFTP session, or the shell.
	pub async fn files(&mut self, session: &client::Handle<super::client::Handler>) -> AsuserFiles {
		self.files_as(session, self.selected).await
	}

	/// The same for a NAMED account rather than the selected one (§46). The editor needs this: a
	/// file opened as root must be saved back as root, whichever account the panes have moved on to
	/// while it was being edited.
	pub async fn files_as(
		&mut self,
		session: &client::Handle<super::client::Handler>,
		identity: u64,
	) -> AsuserFiles {
		let Some(entry) = self.entries.get_mut(&identity) else {
			return AsuserFiles::Denied("That account is no longer open.".to_owned());
		};
		if let Some(denied) = entry.denied.as_ref() {
			return AsuserFiles::Denied(denied.clone());
		}
		match entry.open_sftp(session).await {
			Ok(sftp) => AsuserFiles::Sftp(sftp),
			Err(reason) => AsuserFiles::Shell(entry.fall_back(reason)),
		}
	}
}

impl AsuserEntry {
	fn new(runner: Runner) -> Self {
		Self {
			runner,
			sftp: None,
			program: None,
			discovered: false,
			broken: None,
			denied: None,
		}
	}

	/// What to use when SFTP as this account will not work: shell commands as the same account.
	///
	/// For the login account that is the pre-§46 `ls` fallback, unchanged — its commands run as
	/// itself, so they work whenever the connection does. For an elevated account it is worth
	/// trying (`sudo ls` works on a remote that simply has no `sftp-server`), and where sudo itself
	/// is what refused, those commands fail with sudo's own words and each operation reports them —
	/// so the panes stay empty and say why rather than showing another account's files.
	fn fall_back(&mut self, reason: String) -> Runner {
		self.broken = Some(reason);
		self.runner.clone()
	}

	/// Open a raw SFTP session as this account: the subsystem for the login account, `sftp-server`
	/// under the elevation for any other.
	async fn open_raw(
		&mut self,
		session: &client::Handle<super::client::Handler>,
	) -> Result<Arc<RawSftpSession>, String> {
		if let Some(reason) = self.broken.as_ref() {
			return Err(reason.clone());
		}
		if self.runner.is_login() {
			return super::open_raw_sftp(session)
				.await
				.map(Arc::new)
				.map_err(|error| {
					eprintln!("sftp browse channel unavailable, falling back to ls: {error:#}");
					"The server would not start the sftp subsystem.".to_owned()
				});
		}
		let program = self.program(session).await?;
		let mut last = None;
		// The handshake is INSIDE the attempt, not after it (see `attempts`): a sudo refused for the
		// want of a password accepts the exec request happily and then dies, so "did the exec work" is
		// not the question — "did it speak SFTP" is.
		for armed in self.attempts() {
			match self.try_channel(session, &program, *armed).await {
				Ok(channel) => {
					let raw = RawSftpSession::new(channel.into_stream());
					match raw.init().await {
						Ok(_) => return Ok(Arc::new(raw)),
						Err(error) => last = Some(handshake_failed(&error.to_string())),
					}
				}
				Err(error) => last = Some(error),
			}
		}
		Err(last.unwrap_or_else(|| handshake_failed("no attempt was made")))
	}

	/// The same for the friendly session shape the transfers and the editor use.
	async fn open_sftp(
		&mut self,
		session: &client::Handle<super::client::Handler>,
	) -> Result<SftpSession, String> {
		if let Some(reason) = self.broken.as_ref() {
			return Err(reason.clone());
		}
		if self.runner.is_login() {
			return super::open_sftp(session).await.map_err(|error| {
				eprintln!("sftp channel failed: {error:#}");
				"Could not open an SFTP channel — the server may not offer the sftp subsystem."
					.to_owned()
			});
		}
		let program = self.program(session).await?;
		let mut last = None;
		for armed in self.attempts() {
			match self.try_channel(session, &program, *armed).await {
				Ok(channel) => match SftpSession::new(channel.into_stream()).await {
					Ok(sftp) => return Ok(sftp),
					Err(error) => last = Some(handshake_failed(&error.to_string())),
				},
				Err(error) => last = Some(error),
			}
		}
		Err(last.unwrap_or_else(|| handshake_failed("no attempt was made")))
	}

	/// The attempts to make at an elevated `sftp-server`, in order — the password rule as a list
	/// (see the module note).
	///
	/// The failure cannot be read from sudo's own words here: the protocol stream carries the
	/// channel's stdout alone, and sudo writes its complaints to stderr. So the ORDER is what
	/// substitutes for reading them. An unarmed attempt goes first, so nothing is ever written to a
	/// sudo that may not want it; the armed one follows only if the first would not speak SFTP, which
	/// is exactly what a sudo refused for the want of a password does. Once armed successfully, later
	/// channels start there — the verdict is remembered.
	fn attempts(&self) -> &'static [bool] {
		if self.runner.armed() {
			&[true]
		} else if self.runner.has_secret() {
			&[false, true]
		} else {
			&[false]
		}
	}

	/// One attempt at a channel running `sftp-server` as this account. `armed` writes the cached
	/// password as the first line, which sudo takes and the program never sees.
	async fn try_channel(
		&mut self,
		session: &client::Handle<super::client::Handler>,
		program: &str,
		armed: bool,
	) -> Result<Channel<client::Msg>, String> {
		let channel = session
			.channel_open_session()
			.await
			.map_err(|error| format!("Could not open a channel: {error}"))?;
		// No pty, deliberately: SFTP is a binary protocol and a pty would translate line endings
		// and interpret control bytes in it. It is also why `su` cannot serve this channel — it
		// reads a password from a terminal only.
		self.runner
			.launch_program(&channel, program, armed)
			.await
			.map_err(|error| format!("{error:#}"))?;
		Ok(channel)
	}

	/// The `sftp-server` path on this remote, looked for once per account and remembered.
	///
	/// The search runs as the LOGIN account and INLINE, from the session loop's own handle: a path
	/// is public information, so finding it needs no privilege — and a probe that needed sudo could
	/// not tell "no binary here" from "sudo said no".
	async fn program(
		&mut self,
		session: &client::Handle<super::client::Handler>,
	) -> Result<String, String> {
		if let Some(program) = self.program.as_ref() {
			return Ok(program.clone());
		}
		if self.discovered {
			return Err(no_program());
		}
		self.discovered = true;
		let found = match exec_inline(session, elevate::discover()).await {
			Ok(output) => elevate::parse_program(&output.stdout),
			Err(error) => {
				eprintln!("could not look for sftp-server: {error:#}");
				None
			}
		};
		match found {
			Some(program) => {
				self.program = Some(program.clone());
				Ok(program)
			}
			None => Err(no_program()),
		}
	}
}

/// The reason shown when the remote has no `sftp-server` binary to run as another account.
fn no_program() -> String {
	"This server has no sftp-server program to run as another account.".to_owned()
}

/// The reason shown when the elevated `sftp-server` never spoke SFTP. Its own words are on stderr,
/// which the protocol stream does not carry, so this says what is actually known.
fn handshake_failed(detail: &str) -> String {
	eprintln!("elevated sftp handshake failed: {detail}");
	"Could not start sftp-server as that account.".to_owned()
}

#[cfg(test)]
mod tests {
	use super::{
		Accounts, AsuserEntry, Channels, ElevateKind, LOGIN_IDENTITY, Output, Runner, Secret,
		wants_password,
	};

	/// An entry for an elevated account, with or without a cached password. No session and no
	/// network: `Channels` is only an mpsc sender, so the DECISIONS in here are testable on their own
	/// — which is the point of keeping them separate from the I/O.
	fn entry(secret: Option<&str>) -> AsuserEntry {
		let (channels, _requests) = Channels::new();
		AsuserEntry::new(Runner::elevated(
			channels,
			ElevateKind::Sudo,
			"root".to_owned(),
			secret.map(|value| Secret::new(value.to_owned())),
		))
	}

	#[test]
	fn a_password_is_never_written_before_sudo_has_refused_without_one() {
		// The rule the whole file rests on. With no password there is only ever the plain attempt.
		assert_eq!(entry(None).attempts(), &[false]);
		// With one, the UNARMED attempt still goes first: sudo holding a valid credential does not
		// read its stdin at all, so a password sent on a guess would land in `sftp-server`'s input
		// instead — as protocol garbage, to a program running as root.
		assert_eq!(entry(Some("hunter2")).attempts(), &[false, true]);
	}

	#[test]
	fn once_sudo_has_asked_later_channels_start_armed() {
		// Learned, not guessed: the verdict is only ever set by an observed refusal, and from then on
		// the wasted first attempt is not paid again for the rest of the connection.
		let entry = entry(Some("hunter2"));
		entry.runner.remember_password();
		assert_eq!(entry.attempts(), &[true]);
	}

	#[test]
	fn an_account_that_took_two_factors_has_its_files_refused_before_anything_is_tried() {
		// A file channel can repeat a password to `sudo -S` and nothing else, so an account whose
		// elevation needed a code has no credential this side can use. Said up front, because finding
		// out by trying costs two ten-second handshake timeouts and a channel apiece — and then leaves
		// the panes empty anyway.
		let (channels, _requests) = Channels::new();
		let mut accounts = Accounts::new(channels);
		accounts.add(7, ElevateKind::Sudo, "root".to_owned());
		accounts.deny_second_factor(7);
		let denied = accounts.entries[&7]
			.denied
			.as_deref()
			.expect("the account is refused");
		assert!(
			denied.contains("second factor"),
			"and it says why, not merely that it failed: {denied}"
		);
		// The login account is never touched by this: its file access needs no credential at all.
		assert!(accounts.entries[&LOGIN_IDENTITY].denied.is_none());
	}

	#[test]
	fn the_login_account_wraps_nothing() {
		// Every command cmote sent before §46 must still go out byte for byte.
		let (channels, _requests) = Channels::new();
		let login = Runner::login(channels);
		assert!(login.is_login());
		assert_eq!(
			login.command("ls -1Ap -- '/etc'", false),
			"ls -1Ap -- '/etc'"
		);
	}

	/// An output with just a stderr, for the judgement below.
	fn failed(stderr: &str) -> Output {
		Output {
			stdout: String::new(),
			stderr: stderr.to_owned(),
			status: Some(1),
		}
	}

	#[test]
	fn a_command_with_no_exit_status_counts_as_having_worked() {
		// Some servers close the channel without sending one. Calling that a failure would break
		// every listing on those servers, which is worse than trusting a silent success.
		let quiet = Output {
			stdout: "etc\n".to_owned(),
			stderr: String::new(),
			status: None,
		};
		assert!(quiet.ok());
		assert!(!failed("nope").ok());
	}

	#[test]
	fn a_failure_reports_the_remotes_own_words() {
		assert_eq!(
			failed("cme is not in the sudoers file.\n").reason(),
			"cme is not in the sudoers file."
		);
		// Never an empty message: a reason the user cannot read is not a reason.
		assert_eq!(failed("   ").reason(), "the command failed");
	}

	#[test]
	fn only_sudo_asking_for_a_password_unlocks_the_cached_one() {
		// The whole password rule rests on this: recognise sudo's request, and nothing else.
		assert!(wants_password("sudo: a password is required\n"));
		assert!(wants_password(
			"sudo: no tty present and no askpass program specified"
		));
		assert!(wants_password("sudo: no password was provided"));
		// Every other failure leaves the secret where it is — a refusal is not an invitation.
		assert!(!wants_password("cme is not in the sudoers file."));
		assert!(!wants_password("sudo: 1 incorrect password attempt"));
		assert!(!wants_password("/usr/lib/openssh/sftp-server: not found"));
		assert!(!wants_password(""));
	}
}

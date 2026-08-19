// elevate.rs — becoming another account on a machine cmote is already logged in to (PLAN §45).
//
// An SSH session authenticates ONCE, as one user. Everything after that — the shell, every sftp
// channel — runs as that user. Becoming root is therefore not an SSH matter at all: it is a program
// (`sudo`, `su`) that cmote runs on the remote, which holds a short conversation on its own channel
// ("password?", "verification code?") and then, if it is satisfied, replaces itself with a login
// shell for the other account. So an elevated identity is one extra channel per account, and this
// module is the *text* of that conversation: the command to run, and how to tell one of its
// questions from ordinary output.
//
// It is deliberately pure — `&str` in, `String` out. No channel, no iced type, no session. That
// keeps the two judgements that actually carry risk (what gets executed on the remote, and what
// cmote decides is a password prompt) testable line by line, and it lets both sides of the app use
// them: `app` builds the command, `ssh::shell` reads the replies.
//
// The one rule that matters most here: when cmote is not SURE a line is a credential prompt, it
// says nothing. Guessing wrong in the other direction means putting a secret dialog in front of the
// user for a line that was really the root shell's own prompt — and whatever they typed would land
// on that shell's command line. What a missed prompt costs is the elevation: an `Elevating` channel
// draws nothing and takes no typing (`ssh::shell`), so an unrecognised question leaves the dialog
// waiting until it is cancelled. Vocabulary is therefore the whole game, and so is `refusal` below:
// a question asked again is not the same event as an answer refused, and only the program's own
// words in between tell them apart.

/// The text cmote makes `sudo` use for its password prompt, via `-p`. A prompt is otherwise the
/// remote's own wording — localized, `[sudo] password for cme:`, `Password:`, whatever sudoers
/// says — and matching all of those is guesswork. Naming the prompt ourselves turns the one
/// question we can predict into an EXACT string match, and leaves the vocabulary below to cover
/// only what we cannot predict: a second factor, asked by a PAM module sudo knows nothing about.
///
/// Contains no `%` (sudo would expand `%p`, `%u`, `%H` in it) and no shell metacharacter. It ends
/// with a colon because that is what marks a line as a question at all (see `prompt`), and it is
/// not one of the endings a SHELL prompt uses, so it can never be read as the conversation ending.
pub const MARKER: &str = "cmote-password:";

/// The longest prompt label cmote will show. A prompt is one short question; anything longer is
/// program output that happens to end in a colon, and truncating keeps a runaway line from
/// stretching the dialog off the screen.
const MAX_LABEL: usize = 120;

/// How a user becomes another user on the remote. Two shapes cover it: `sudo`, which asks for the
/// CALLER's own password (and is what a sudoers-managed machine expects), and `su`, which asks for
/// the TARGET account's — the fallback where sudo is absent or the user is not in sudoers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ElevateKind {
	/// `sudo -u <user> -i`: a login shell for `user`, authenticated with the caller's password.
	#[default]
	Sudo,
	/// `su - <user>`: a login shell for `user`, authenticated with that account's own password.
	Su,
}

impl ElevateKind {
	/// The label shown beside the choice in the elevate dialog.
	pub fn label(self) -> &'static str {
		match self {
			ElevateKind::Sudo => "sudo",
			ElevateKind::Su => "su",
		}
	}

	/// The command line to execute on the remote to open a login shell as `user`.
	///
	/// `user` is quoted for the remote shell (sshd runs an exec request through the login shell,
	/// so it IS shell text) *and* checked by `valid_user` before it ever reaches here — belt and
	/// braces, because this string is the one place in cmote that composes a command from
	/// something the user typed.
	///
	/// The sudo form names its own password prompt (`-p MARKER`) so the reply can be recognised
	/// exactly; `su` has no such option, which is why the vocabulary in `prompt` exists at all.
	pub fn command(self, user: &str) -> String {
		let quoted = crate::explorer::shell_quote(user);
		match self {
			// `-u` explicitly, even for root: it makes the command say who it is becoming, so the
			// same string works for `root` and for a service account with no special-casing.
			ElevateKind::Sudo => format!(
				"sudo -p {} -u {quoted} -i",
				crate::explorer::shell_quote(MARKER)
			),
			// A bare `-` is what makes it a LOGIN shell (fresh environment, the account's own
			// $HOME and $PATH), matching what `sudo -i` gives.
			ElevateKind::Su => format!("su - {quoted}"),
		}
	}
}

/// The places an OpenSSH installation keeps the `sftp-server` binary, in the order they are
/// tried (§46). The file layer runs THIS program as the other account to browse and transfer
/// files as them — see `program_command` — because the sftp *subsystem* is started by sshd as
/// the account that authenticated, and no amount of sudo in a shell can change that.
///
/// A list rather than one path because it is packaging, not standard: Debian and Ubuntu put it
/// under `/usr/lib/openssh`, Red Hat under `/usr/libexec/openssh`, Arch and Alpine under
/// `/usr/lib/ssh`, the BSDs under `/usr/libexec`. The list is only a fast path — `discover`
/// falls back to asking sshd's own configuration.
const SFTP_SERVERS: [&str; 6] = [
	"/usr/lib/openssh/sftp-server",
	"/usr/libexec/openssh/sftp-server",
	"/usr/lib/ssh/sftp-server",
	"/usr/libexec/sftp-server",
	"/usr/lib/sftp-server",
	"/usr/local/libexec/sftp-server",
];

/// The shell snippet that finds the remote's `sftp-server` binary (§46), printing its path or
/// nothing at all.
///
/// Run as the LOGIN account, not as the account being elevated to: a path is public information
/// (`-x` on a program every user may run), so finding it needs no privilege — and a probe that
/// needed sudo could not tell "no sftp-server here" from "sudo said no".
///
/// The `sed` is the fallback that matters on a server whose packaging we do not know: sshd's own
/// `Subsystem sftp` line names the program it starts for the login user, so if the binary exists
/// anywhere it is named there. It can also say `internal-sftp` — sftp implemented INSIDE sshd,
/// with no binary to run — which `parse_program` rejects, because there is nothing to exec.
pub fn discover() -> String {
	let candidates = SFTP_SERVERS.join(" ");
	format!(
		"for p in {candidates}; do if [ -x \"$p\" ]; then echo \"$p\"; exit 0; fi; done; \
		 sed -n 's/^[Ss]ubsystem[[:space:]]\\{{1,\\}}sftp[[:space:]]\\{{1,\\}}\\([^[:space:]]\\{{1,\\}}\\).*/\\1/p' \
		 /etc/ssh/sshd_config 2>/dev/null | head -n 1"
	)
}

/// The `sftp-server` path in `discover`'s output, if it named a usable one.
///
/// Pure so the judgement can be tested without a server, and separate from `discover` because
/// this is the half that carries the risk: the answer may come from `/etc/ssh/sshd_config`, so it
/// is remote-controlled text about to be composed into a command that runs as ANOTHER account.
/// `valid_program` is therefore a whitelist, not an escape.
pub fn parse_program(output: &str) -> Option<String> {
	output
		.lines()
		.map(str::trim)
		.find(|line| valid_program(line))
		.map(str::to_owned)
}

/// Whether `path` is something cmote will run as another account (§46).
///
/// Deliberately narrow, and checked instead of quoted: an absolute path, of ordinary path
/// characters only, whose file name mentions `sftp` — so a doctored `Subsystem` line naming
/// something else entirely (or `internal-sftp`, which is not a program at all) never becomes a
/// command. Quoting alone would faithfully run whatever it was told to.
pub fn valid_program(path: &str) -> bool {
	let name = path.rsplit('/').next().unwrap_or_default();
	path.starts_with('/')
		&& path.len() <= 128
		&& !path.contains("..")
		&& path
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
		&& name.contains("sftp")
		&& name != "internal-sftp"
}

/// The command that runs ONE program as another account, for a file-layer channel (§46).
///
/// No shell is involved on purpose: `sudo` is given the program to exec directly, so the only
/// strings on that command line are a name `valid_user` has vetted and a path `valid_program`
/// has vetted. `password` picks how sudo will authenticate:
///
///   * `false` — `-n`, non-interactive: sudo either has a valid credential (a cached ticket or
///     NOPASSWD) or fails immediately. Nothing is written to the channel, so nothing can be
///     misread as the program's own input.
///   * `true` — `-S`, read the password from stdin: the caller writes it as the first line, and
///     the program's data follows. Safe because sudo reads that line one byte at a time and stops
///     at the newline, leaving the rest of the stream to the program it execs.
///
/// The order matters and is why the caller must PROBE before choosing: writing a password to a
/// sudo that does not want one would push it into `sftp-server`'s input as protocol garbage.
///
/// `su` is offered for completeness only. It reads a password from a terminal and this channel
/// deliberately has none (a pty would mangle the binary protocol), so it works for an account
/// that needs no password and fails cleanly otherwise — see the NOT list in PLAN §46.
pub fn program_command(kind: ElevateKind, user: &str, program: &str, password: bool) -> String {
	let account = crate::explorer::shell_quote(user);
	let path = crate::explorer::shell_quote(program);
	match kind {
		ElevateKind::Sudo if password => format!("sudo -S -p '' -u {account} -- {path}"),
		ElevateKind::Sudo => format!("sudo -n -u {account} -- {path}"),
		ElevateKind::Su => format!("su - {account} -c {path}"),
	}
}

/// The command that runs a shell SNIPPET as another account (§46) — the fallback file backend,
/// where each operation is `ls`, `cat`, `mkdir` and friends rather than an sftp packet.
///
/// The snippet is quoted as one argument to `/bin/sh -c`, so everything cmote composes into it
/// (always through `crate::explorer::shell_quote`) stays data. Authentication works exactly as in
/// `program_command`, including the stdin rule: after sudo has taken its password line, the rest
/// of the channel belongs to the snippet — which is what lets a file be written by piping its
/// bytes into `cat`.
pub fn shell_command(kind: ElevateKind, user: &str, snippet: &str, password: bool) -> String {
	let account = crate::explorer::shell_quote(user);
	let script = crate::explorer::shell_quote(snippet);
	match kind {
		ElevateKind::Sudo if password => {
			format!("sudo -S -p '' -u {account} -- /bin/sh -c {script}")
		}
		ElevateKind::Sudo => format!("sudo -n -u {account} -- /bin/sh -c {script}"),
		ElevateKind::Su => format!("su - {account} -c {script}"),
	}
}

/// Whether `user` is a plausible account name, checked before it is put in a command line.
///
/// Deliberately narrower than what a system will accept: letters, digits, and `._-`, not starting
/// with `-` (which a command would read as an option) and not empty. A name outside this set is
/// refused at the field rather than quoted and hoped for — that field feeds a command that runs on
/// a remote machine as another user, which is exactly the boundary to validate at, not near.
///
/// `allow(dead_code)`: nothing calls it while there is no UI that types an account name — the
/// elevate dialog was withdrawn pending a different approach — and this is deliberately kept rather
/// than deleted. It is a security boundary with its own tests, and whatever replaces that dialog
/// will need exactly this check before it composes a command. Deleting it would invite the next
/// implementation to quote and hope.
#[allow(dead_code)]
pub fn valid_user(user: &str) -> bool {
	!user.is_empty()
		&& !user.starts_with('-')
		&& user.len() <= 32
		&& user
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// The words that mark a line as a credential question, for the prompts cmote cannot name itself
/// (`su`'s own, and whatever a PAM module asks for a second factor). Lower-case; matched as
/// substrings against a lower-cased line, so "Verification code:" and "Duo passcode or option
/// (1-3):" both land.
///
/// A few common non-English spellings are included because a remote's prompts follow ITS locale,
/// not the desktop's — and an unmatched prompt means no dialog, which the user then has to notice
/// and answer in the terminal.
const CREDENTIAL_WORDS: [&str; 12] = [
	"password",
	"passwd",
	"passcode",
	"pass code",
	"code",
	"otp",
	"token",
	"verification",
	"mot de passe",
	"contraseña",
	"senha",
	"kennwort",
];

/// The words that mark a line as the program REJECTING the answer it was just given, rather than
/// getting on with the conversation (§45). Lower-case, matched as substrings against the lines a
/// program printed between one answer and its next question.
///
/// This list exists because "the same question twice" is NOT the same thing as "that was wrong".
/// A two-factor stack asks for the password and then for the code, and when the second module's
/// prompt is the standard one, sudo substitutes its own `-p` text for BOTH — so cmote sees one
/// label arrive twice in a conversation that is going perfectly well. Reading the repetition as a
/// refusal told the user their good password had been rejected, threw that password away, and left
/// them retyping it into what was really the second factor's field.
///
/// What actually separates the two cases is what the program says in between: a rejection is
/// announced ("Sorry, try again.", "Authentication failure"), a further factor is not. Matching
/// nothing means no notice and nothing discarded, which is the safe direction — the user sees the
/// question asked again, exactly as a terminal would have shown it.
const REFUSAL_WORDS: [&str; 8] = [
	"sorry",
	"try again",
	"incorrect",
	"failure",
	"failed",
	"denied",
	"invalid",
	"not allowed",
];

/// The characters a shell prompt ends with. Seeing one of these is how `ssh::shell` knows the
/// conversation is over and the account's own shell is talking (§45) — after that, nothing on the
/// channel is ever treated as a credential question again.
const SHELL_ENDINGS: [char; 4] = ['$', '#', '%', '>'];

/// The credential question at the end of `buffer`, if that is what it is.
///
/// `buffer` is everything the elevating channel has said since the last question was answered.
/// Only its TAIL — the text after the last newline — can be a prompt, because a program asking a
/// question leaves the cursor on the line it asked on; a line that has been terminated is output,
/// not a question. So this returns `Some` only when that tail:
///
///   1. ends with `:` or `?` (ignoring trailing spaces), which is what a prompt looks like, and
///   2. either IS cmote's own marker, or contains one of `CREDENTIAL_WORDS`.
///
/// The second test is what keeps a secret dialog from appearing over an ordinary shell prompt that
/// happens to end in a colon. Getting it wrong the safe way (no match, no dialog) leaves the bytes
/// to the terminal; getting it wrong the other way would put whatever the user types onto a root
/// shell's command line.
pub fn prompt(buffer: &str) -> Option<String> {
	let tail = buffer.rsplit('\n').next()?;
	// A pty echoes `\r` before its `\n` and programs use it to rewrite a line; only what follows
	// the last one is on screen.
	let tail = tail.rsplit('\r').next()?;
	// Sanitized BEFORE the shape is judged, not after: a coloured prompt ends with the escape
	// sequence that turns the colour off, so testing the raw bytes would decide that `Verification
	// code:` in red is not a question at all.
	let label = sanitize(tail);
	let label = label.trim_end();
	if !label.ends_with(':') && !label.ends_with('?') {
		return None;
	}
	// cmote's own prompt: shown to the user as a plain question, since the marker itself is an
	// internal token and would mean nothing to them.
	if label.contains(MARKER) {
		return Some("Password:".to_owned());
	}
	let lowered = label.to_lowercase();
	CREDENTIAL_WORDS
		.iter()
		.any(|word| lowered.contains(word))
		.then(|| label.to_owned())
}

/// What the program said between the last answer and the question it is now asking, when what it
/// said was that the answer is refused (§45). `None` when it simply asked something else.
///
/// `buffer` is everything said since the last answer, so its TAIL is the new question and every line
/// before it is what the program printed on the way to it. The tail is dropped before the search: a
/// question is not a verdict on the answer before it, and `Password:` itself contains no refusal
/// word only by luck of vocabulary.
///
/// The remote's own wording comes back rather than a bare flag, because "Sorry, try again." and
/// "Sorry, user cme is not allowed to execute…" ask completely different things of the user, and only
/// the remote knows which of them it means.
pub fn refusal(buffer: &str) -> Option<String> {
	let mut lines: Vec<&str> = buffer.split('\n').collect();
	lines.pop();
	lines
		.into_iter()
		.rev()
		.map(|line| sanitize(line).trim().to_owned())
		.find(|line| {
			let lowered = line.to_lowercase();
			REFUSAL_WORDS.iter().any(|word| lowered.contains(word))
		})
}

/// Whether the tail of `buffer` is a shell's own prompt — the sign that the elevation succeeded
/// and the account's shell now has the channel (§45).
///
/// Read the same way as `prompt`: only the text after the last newline is on screen. A shell's
/// prompt ends in one of `SHELL_ENDINGS`, conventionally followed by a space, and — unlike a
/// credential question — nothing about it is a question.
pub fn looks_like_shell(buffer: &str) -> bool {
	let Some(tail) = buffer
		.rsplit('\n')
		.next()
		.and_then(|t| t.rsplit('\r').next())
	else {
		return false;
	};
	let trimmed = sanitize(tail);
	let trimmed = trimmed.trim_end();
	trimmed.ends_with(SHELL_ENDINGS)
}

/// The last thing the program said, for the failure notice when an elevation channel dies (§45):
/// `sudo: 3 incorrect password attempts`, `cme is not in the sudoers file`, `su: Authentication
/// failure`. `None` when it said nothing printable.
///
/// This text is the REMOTE's own words about the remote's own policy, so showing it is not the
/// credential oracle §12 forbids — that rule is about cmote never telling an attacker which of ITS
/// factors was wrong. Here, a user who cannot tell "wrong password" from "not in sudoers" cannot
/// fix either.
pub fn reason(buffer: &str) -> Option<String> {
	buffer
		.lines()
		.rev()
		.map(sanitize)
		.find(|line| !line.trim().is_empty())
		.map(|line| line.trim().to_owned())
}

/// One line of remote output, made safe and short enough to put in a dialog: escape sequences and
/// control bytes removed, length capped.
///
/// Remote output is not text cmote may pass through to a widget as it stands — it can carry CSI
/// and OSC sequences, and a label is not a terminal (the grid is the only thing in cmote that
/// interprets them). Stripping them here means a prompt cannot repaint, reposition or retitle
/// anything by being shown.
fn sanitize(line: &str) -> String {
	let mut out = String::with_capacity(line.len());
	let mut chars = line.chars();
	while let Some(c) = chars.next() {
		match c {
			// An escape starts a sequence: swallow it and everything up to its terminator. CSI
			// (`\x1b[`) ends at a byte in `@`..`~`; OSC (`\x1b]`) ends at BEL or the ST pair,
			// and skipping to the first control byte covers both without a parser.
			'\x1b' => {
				for c in chars.by_ref() {
					if c.is_ascii_alphabetic() || matches!(c, '\x07' | '~' | '\\') {
						break;
					}
				}
			}
			// Every other control byte is dropped outright; a tab becomes a space so words that
			// were separated stay separated.
			'\t' => out.push(' '),
			c if c.is_control() => {}
			c => out.push(c),
		}
		if out.chars().count() >= MAX_LABEL {
			break;
		}
	}
	out
}

/// How many questions cmote will answer for one elevation before giving up (§45). sudo itself
/// stops after three wrong passwords; this only guards against a program that would ask for ever,
/// so it is generously above any real conversation (a password plus a second factor, retried).
const MAX_PROMPTS: u32 = 8;

/// What the bytes that just arrived on an elevating channel MEAN (§45) — the answer
/// [`Handshake::on_bytes`] gives, and the whole of what the caller has to act on.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
	/// Nothing to do: output that is neither a question nor the end of the conversation, or a
	/// question that must not be put (one is already outstanding, or too many have been asked).
	Nothing,
	/// Put this question to the user. `refusal` is the program's own words about the PREVIOUS
	/// answer, when it rejected one — shown beside the new question so "asked again" and "you got
	/// it wrong" do not look alike.
	Ask {
		label: String,
		refusal: Option<String>,
	},
	/// The program is gone and the account's own shell has the channel. `flush` is everything
	/// buffered since the last answer — the shell's greeting and its first prompt — which the caller
	/// must put on screen or the freshly elevated terminal comes up empty. `factors` is how many
	/// DISTINCT things were asked for on the way, which decides whether the file layer may follow
	/// this account (§46).
	Live { flush: String, factors: u32 },
}

/// The conversation cmote holds with `sudo` or `su` while becoming another account (§45): what has
/// been said, what has been asked, and what the next chunk of bytes means.
///
/// This is a state machine over a `String` and three counters, and it used to live inside a match
/// arm in `ssh::shell` — where it could not be reached by a test, because building the type that
/// held it needs a `russh::Channel` and therefore a real server. The rules it carries are the ones
/// that most want testing: which questions may be asked at all, and which single answer out of the
/// conversation may be KEPT as the caller's password. It holds no channel and no session, so the
/// whole conversation can now be played out against it in memory.
///
/// The subtle field is `factors`, and it is not `asked`. A question the program puts AGAIN after
/// refusing the answer is the same factor over again, so only a question that follows no refusal
/// increments it. Two decisions turn on that distinction: a corrected password is still a password
/// and is worth keeping, and an account that took more than one factor to log in as cannot have its
/// files read as it (§46) — a file channel can replay a password to sudo, but it can neither ask
/// for a second factor nor reuse a spent one.
#[derive(Debug, Default)]
pub struct Handshake {
	/// What the program has said since the last question was answered. Cleared at each question, so
	/// a stale prompt is never mistaken for a fresh one.
	buffer: String,
	/// How many questions have been put to the user, bounded by [`MAX_PROMPTS`].
	asked: u32,
	/// How many DISTINCT things have been asked for — see the type's own note.
	factors: u32,
	/// Set while a question is unanswered, so the same buffer arriving in two chunks cannot raise
	/// two dialogs.
	pending: bool,
	/// Whether the outstanding question is the one cmote NAMED itself (`-p MARKER`).
	password: bool,
}

impl Handshake {
	/// Read the next chunk the program sent, and say what it means.
	///
	/// Lossy UTF-8 on purpose: a chunk can end mid-character, and this text is only ever compared
	/// against prompt shapes — the terminal, which does care, never sees it.
	pub fn on_bytes(&mut self, bytes: &[u8]) -> Step {
		self.buffer.push_str(&String::from_utf8_lossy(bytes));

		// A shell prompt means the program is gone and the account's own shell has the channel: the
		// conversation is over, whatever else is in the buffer.
		if looks_like_shell(&self.buffer) {
			return Step::Live {
				flush: std::mem::take(&mut self.buffer),
				factors: self.factors,
			};
		}

		// Otherwise: is it asking something? Only while no question is already outstanding, and
		// only up to a bound, so a program that asks for ever cannot pin the user in a dialog loop.
		if self.pending || self.asked >= MAX_PROMPTS {
			return Step::Nothing;
		}
		let Some(label) = prompt(&self.buffer) else {
			return Step::Nothing;
		};
		// Whether the program rejected the PREVIOUS answer, in its own words. Read before the
		// buffer is cleared: what it holds between one answer and the next question is the only
		// evidence there is. The alternative the GUI once relied on — "the same wording twice means
		// refused" — is wrong on a two-factor machine.
		let refusal = refusal(&self.buffer);
		// Whether this is cmote's OWN password question, also decided before the clear: the answer
		// to that one is the caller password the file layer will need (§46), and the answer to any
		// other question is a secret to use once and forget.
		self.password = self.buffer.contains(MARKER);
		self.pending = true;
		self.asked += 1;
		if refusal.is_none() {
			self.factors += 1;
		}
		// Cleared now: the question has been put, so these bytes are spent. What arrives next is
		// either the answer's outcome or the next question, and neither should be read against text
		// that has already been dealt with.
		self.buffer.clear();
		Step::Ask { label, refusal }
	}

	/// Record that the outstanding question has just been answered, and say whether that answer was
	/// the caller's own password — the one the file layer may keep and replay to sudo on a file
	/// channel (§46). `None` when there was no question outstanding, which is the caller's signal
	/// to write nothing at all.
	///
	/// Being cmote's own prompt is necessary but NOT sufficient, which is why `factors` is in the
	/// test. sudo substitutes its `-p` text for every standard prompt in its PAM stack, so on a
	/// two-factor machine the one-time code is asked for under the marker too; answering "yes"
	/// there handed the code to the file layer as the connection's sudo password, from where it
	/// could only ever be refused. The FIRST factor's answer is the password — including when it
	/// took two goes, since a refused question is the same factor asked again.
	pub fn answered(&mut self) -> Option<bool> {
		if !self.pending {
			return None;
		}
		self.pending = false;
		let was_password = self.password && self.factors == 1;
		self.buffer.clear();
		Some(was_password)
	}

	/// Why the channel died mid-conversation (§45) — the last thing the program said, which is the
	/// remote's own words about its own policy, falling back to a plain sentence when it said
	/// nothing at all.
	pub fn death_reason(&self) -> String {
		reason(&self.buffer).unwrap_or_else(|| "The elevation was refused.".to_owned())
	}
}

#[cfg(test)]
mod handshake_tests {
	use super::{Handshake, MARKER, MAX_PROMPTS, Step};

	/// The ordinary elevation, start to finish: sudo asks under cmote's own marker, the password is
	/// given, the root shell greets and prompts.
	///
	/// This whole test was unreachable before the conversation was lifted out of `ssh::shell`,
	/// because the type that held it took a `russh::Channel` and so needed a real server.
	#[test]
	fn one_password_and_the_shell_arrives() {
		let mut handshake = Handshake::default();

		// sudo's prompt, worded by us so it is an exact match rather than a guess.
		let Step::Ask { label, refusal } = handshake.on_bytes(MARKER.as_bytes()) else {
			panic!("the marker is a question");
		};
		// Shown as a plain question: the marker is an internal token and would mean nothing.
		assert_eq!(label, "Password:");
		assert_eq!(refusal, None, "nothing was refused; this is the first ask");

		// It IS the caller's own password, so the file layer may keep it (§46).
		assert_eq!(handshake.answered(), Some(true));

		// The greeting and the first prompt arrive together and must reach the grid — losing them
		// is what left a freshly elevated terminal empty but for a caret.
		let step = handshake.on_bytes(b"Welcome to Debian\nroot@host:~# ");
		assert_eq!(
			step,
			Step::Live {
				flush: "Welcome to Debian\nroot@host:~# ".to_owned(),
				factors: 1,
			}
		);
	}

	/// A mistyped password is the SAME factor asked again, so the corrected one is still the
	/// caller's password and is still cacheable (§45, §46).
	#[test]
	fn a_corrected_password_is_still_a_password() {
		let mut handshake = Handshake::default();
		assert!(matches!(
			handshake.on_bytes(MARKER.as_bytes()),
			Step::Ask { .. }
		));
		assert_eq!(handshake.answered(), Some(true));

		// sudo says no, in its own words, and asks the same thing again.
		let Step::Ask { refusal, .. } = handshake.on_bytes(b"Sorry, try again.\ncmote-password:")
		else {
			panic!("it asked again");
		};
		// The refusal is carried through so the dialog can say WHY, rather than looking like a
		// question that simply repeated itself.
		assert_eq!(refusal.as_deref(), Some("Sorry, try again."));
		// Still one factor — so still the password, and still keepable.
		assert_eq!(handshake.answered(), Some(true));
		assert_eq!(
			handshake.on_bytes(b"root@host:~# "),
			Step::Live {
				flush: "root@host:~# ".to_owned(),
				factors: 1,
			}
		);
	}

	/// THE SECURITY RULE, and the reason this extraction was worth doing: a one-time code asked for
	/// under cmote's own marker must NOT be kept as the connection's sudo password.
	///
	/// sudo substitutes its `-p` text for every standard prompt in its PAM stack, so on a
	/// two-factor machine the marker appears twice — once for the password, once for the code.
	/// Answering "this was the password" the second time handed the code to the file layer, from
	/// where it could only ever be refused. Being cmote's own prompt is necessary but not
	/// sufficient; the factor count is what tells the two apart, and until now nothing tested it.
	#[test]
	fn a_second_factor_under_our_own_marker_is_never_kept() {
		let mut handshake = Handshake::default();

		assert!(matches!(
			handshake.on_bytes(MARKER.as_bytes()),
			Step::Ask { .. }
		));
		assert_eq!(
			handshake.answered(),
			Some(true),
			"the first factor is the password"
		);

		// No refusal in between — so this is a NEW thing being asked for, not the same one again.
		let Step::Ask { refusal, .. } = handshake.on_bytes(b"\ncmote-password:") else {
			panic!("the second factor is a question too");
		};
		assert_eq!(refusal, None, "nothing was refused; it moved on");
		assert_eq!(
			handshake.answered(),
			Some(false),
			"a second factor is answered and forgotten, whatever prompt it wore"
		);

		// And the file layer is told two factors, which is what stops it following this account.
		assert_eq!(
			handshake.on_bytes(b"root@host:~# "),
			Step::Live {
				flush: "root@host:~# ".to_owned(),
				factors: 2,
			}
		);
	}

	/// One question raises one dialog, however the bytes are chopped up.
	#[test]
	fn a_prompt_split_across_chunks_is_asked_once() {
		let mut handshake = Handshake::default();
		// A chunk that ends mid-prompt is not yet a question.
		assert_eq!(handshake.on_bytes(b"cmote-pass"), Step::Nothing);
		assert!(matches!(handshake.on_bytes(b"word:"), Step::Ask { .. }));
		// More bytes while the question is outstanding cannot raise a second dialog — the same
		// buffer arriving twice is exactly how that used to happen.
		assert_eq!(handshake.on_bytes(b"cmote-password:"), Step::Nothing);
	}

	/// A program that would ask for ever cannot pin the user in a dialog loop.
	#[test]
	fn the_questions_run_out() {
		let mut handshake = Handshake::default();
		for _ in 0..MAX_PROMPTS {
			assert!(matches!(
				handshake.on_bytes(MARKER.as_bytes()),
				Step::Ask { .. }
			));
			handshake.answered();
		}
		assert_eq!(handshake.on_bytes(MARKER.as_bytes()), Step::Nothing);
	}

	/// Answering when nothing was asked writes nothing at all — the guard that keeps a secret off a
	/// channel that is not waiting for one.
	#[test]
	fn there_is_nothing_to_answer_until_something_is_asked() {
		let mut handshake = Handshake::default();
		assert_eq!(handshake.answered(), None);
		// And not twice for one question either.
		assert!(matches!(
			handshake.on_bytes(MARKER.as_bytes()),
			Step::Ask { .. }
		));
		assert_eq!(handshake.answered(), Some(true));
		assert_eq!(handshake.answered(), None);
	}

	/// When the channel dies mid-conversation, the notice is the remote's own last words.
	#[test]
	fn a_refused_elevation_reports_what_the_remote_said() {
		let mut handshake = Handshake::default();
		assert_eq!(
			handshake
				.on_bytes(b"cme is not in the sudoers file.  This incident will be reported.\n"),
			Step::Nothing,
			"a terminated line is output, not a question"
		);
		assert_eq!(
			handshake.death_reason(),
			"cme is not in the sudoers file.  This incident will be reported."
		);

		// And a program that died silently still gets a sentence rather than an empty dialog.
		assert_eq!(
			Handshake::default().death_reason(),
			"The elevation was refused."
		);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_sudo_command_names_its_own_prompt_and_its_target() {
		let command = ElevateKind::Sudo.command("root");
		assert!(
			command.contains(MARKER),
			"the prompt is cmote's to recognise"
		);
		assert!(command.contains("-u 'root'"), "and the target is explicit");
		assert!(command.ends_with("-i"), "a LOGIN shell, not a bare one");
	}

	#[test]
	fn su_takes_a_dash_so_it_is_a_login_shell_too() {
		assert_eq!(ElevateKind::Su.command("postgres"), "su - 'postgres'");
	}

	#[test]
	fn a_user_name_that_could_be_read_as_an_option_is_refused() {
		assert!(valid_user("root"));
		assert!(valid_user("web-app.2"));
		// The three that matter: an empty field, a name a command would read as a flag, and
		// anything carrying shell punctuation — quoting would hold, but this never gets that far.
		assert!(!valid_user(""));
		assert!(!valid_user("-u"));
		assert!(!valid_user("root; rm -rf /"));
		assert!(!valid_user("root$(id)"));
	}

	#[test]
	fn cmotes_own_marker_is_shown_as_a_plain_question() {
		// The marker is an internal token; the user is asked for a password, not for it.
		assert_eq!(prompt(MARKER).as_deref(), Some("Password:"));
		assert_eq!(
			prompt(&format!("some output\r\n{MARKER}")).as_deref(),
			Some("Password:")
		);
	}

	#[test]
	fn a_second_factor_is_recognised_by_its_wording() {
		assert_eq!(
			prompt("\r\nVerification code: ").as_deref(),
			Some("Verification code:")
		);
		assert_eq!(
			prompt("Duo passcode or option (1-3): ").as_deref(),
			Some("Duo passcode or option (1-3):")
		);
		assert_eq!(prompt("Password: ").as_deref(), Some("Password:"));
	}

	#[test]
	fn a_line_that_is_merely_finished_is_output_not_a_question() {
		// Terminated lines are what a program PRINTS; a question leaves the cursor on its line.
		assert_eq!(prompt("Password:\r\n"), None);
		assert_eq!(
			prompt("We trust you have received the usual lecture.\n"),
			None
		);
		assert_eq!(prompt("Sorry, try again.\n"), None);
	}

	#[test]
	fn an_ordinary_prompt_ending_in_a_colon_raises_no_secret_dialog() {
		// The case that decides which way to fail: a shell prompt can end in a colon, and cmote
		// must not put a password field in front of one. No credential word, no dialog — the bytes
		// go to the terminal and the user carries on.
		assert_eq!(prompt("root@rec:/etc:"), None);
		assert_eq!(prompt("[root@host ~]:"), None);
	}

	#[test]
	fn escape_sequences_never_reach_the_label() {
		// A prompt is drawn as text in a dialog, not interpreted; a coloured one must arrive as
		// its words alone.
		assert_eq!(
			prompt("\x1b[1;31mVerification code:\x1b[0m ").as_deref(),
			Some("Verification code:")
		);
	}

	#[test]
	fn a_shell_prompt_ends_the_conversation() {
		assert!(looks_like_shell("root@rec:~# "));
		assert!(looks_like_shell("output\r\nbash-5.2$ "));
		assert!(looks_like_shell("PS1> "));
		// A credential question is not a shell prompt, whatever else it looks like.
		assert!(!looks_like_shell("Verification code: "));
		assert!(!looks_like_shell(MARKER));
	}

	#[test]
	fn a_file_channel_runs_the_program_itself_with_no_shell_around_it() {
		// The whole point of the direct form: the only strings on this command line are a vetted
		// account name and a vetted path, so there is no shell to quote for a second time.
		let command = program_command(
			ElevateKind::Sudo,
			"root",
			"/usr/lib/openssh/sftp-server",
			false,
		);
		assert_eq!(
			command,
			"sudo -n -u 'root' -- '/usr/lib/openssh/sftp-server'"
		);
		assert!(!command.contains("sh -c"), "no shell in the way");
	}

	#[test]
	fn a_password_is_read_from_stdin_only_when_the_caller_asks_for_it() {
		// `-n` writes nothing to the channel, so nothing can be mistaken for the program's input;
		// `-S` is the deliberate opposite, chosen only once sudo has refused for want of a password.
		assert!(
			program_command(ElevateKind::Sudo, "root", "/x/sftp-server", false).contains(" -n ")
		);
		let asked = program_command(ElevateKind::Sudo, "root", "/x/sftp-server", true);
		assert!(asked.contains(" -S "), "reads the password from stdin");
		assert!(asked.contains("-p ''"), "and prints no prompt of its own");
		assert!(!asked.contains(" -n "), "the two flags are exclusive");
	}

	#[test]
	fn a_shell_snippet_reaches_the_other_account_as_one_argument() {
		// A snippet IS shell text, so it is quoted whole: everything cmote composes into it stays
		// data no matter what a path inside it contains.
		let command = shell_command(ElevateKind::Sudo, "root", "ls -1Ap -- '/etc'", false);
		assert_eq!(
			command,
			"sudo -n -u 'root' -- /bin/sh -c 'ls -1Ap -- '\\''/etc'\\'''"
		);
	}

	#[test]
	fn only_a_real_sftp_server_path_is_ever_run() {
		assert!(valid_program("/usr/lib/openssh/sftp-server"));
		assert!(valid_program("/usr/libexec/sftp-server"));
		// The three that matter, all of which could arrive from a remote sshd_config: a relative
		// path, a program that is not an sftp server at all, and sftp implemented inside sshd —
		// which is not a program and cannot be run as anyone.
		assert!(!valid_program("sftp-server"));
		assert!(!valid_program("/usr/bin/curl"));
		assert!(!valid_program("internal-sftp"));
		assert!(!valid_program("/usr/lib/openssh/internal-sftp"));
		// And the shell-injection shapes, refused rather than quoted.
		assert!(!valid_program("/x/sftp-server; rm -rf /"));
		assert!(!valid_program("/x/$(id)/sftp-server"));
		assert!(!valid_program("/x/../../sftp-server"));
	}

	#[test]
	fn discovery_reads_the_first_usable_path_it_is_offered() {
		// The `for` loop prints one path; the sshd_config fallback may print anything, so the
		// parsing — not the snippet — is what decides.
		assert_eq!(
			parse_program("/usr/lib/openssh/sftp-server\n").as_deref(),
			Some("/usr/lib/openssh/sftp-server")
		);
		// An sshd that implements sftp itself names no program: there is nothing to elevate.
		assert_eq!(parse_program("internal-sftp\n"), None);
		assert_eq!(parse_program(""), None);
		// A line that cannot be a program is skipped rather than taken and quoted.
		assert_eq!(
			parse_program("sed: /etc/ssh/sshd_config: Permission denied\n/usr/libexec/sftp-server")
				.as_deref(),
			Some("/usr/libexec/sftp-server")
		);
	}

	#[test]
	fn the_discovery_snippet_covers_the_common_packagings_and_asks_sshd_last() {
		let snippet = discover();
		for path in SFTP_SERVERS {
			assert!(snippet.contains(path), "{path} is not looked for");
		}
		assert!(
			snippet.contains("/etc/ssh/sshd_config"),
			"an unknown packaging still has sshd's own answer"
		);
	}

	#[test]
	fn a_refusal_is_what_the_program_said_between_the_answer_and_the_next_question() {
		// The case this exists for: a wrong password, announced, then asked again.
		assert_eq!(
			refusal("\r\nSorry, try again.\r\ncmote-password:").as_deref(),
			Some("Sorry, try again.")
		);
		assert_eq!(
			refusal("Authentication failure\r\nPassword: ").as_deref(),
			Some("Authentication failure")
		);
		// And the case that used to be mistaken for it: a SECOND FACTOR, asked under the very same
		// wording because sudo puts its own `-p` text on every standard prompt in the stack. Nothing
		// was refused, so nothing is said — the user is not told their good password was rejected.
		assert_eq!(refusal("\r\ncmote-password:"), None);
		assert_eq!(refusal("Password: "), None);
		// A banner on the way to the next question is not a refusal either.
		assert_eq!(
			refusal("Duo two-factor login for cme\r\n\r\nPasscode: "),
			None
		);
	}

	#[test]
	fn the_question_itself_is_never_read_as_a_verdict_on_the_answer_before_it() {
		// The tail is the new question, so a prompt whose own wording carries a refusal word must not
		// make cmote claim the last answer was refused.
		assert_eq!(refusal("Invalid code, enter another:"), None);
		// On its own line, though, that same sentence IS the program's verdict.
		assert_eq!(
			refusal("Invalid code, enter another\r\nPasscode: ").as_deref(),
			Some("Invalid code, enter another")
		);
	}

	#[test]
	fn the_failure_reason_is_the_last_thing_the_program_said() {
		assert_eq!(
			reason("Password: \r\nSorry, try again.\r\nsudo: 3 incorrect password attempts\r\n")
				.as_deref(),
			Some("sudo: 3 incorrect password attempts")
		);
		assert_eq!(reason(""), None);
		assert_eq!(reason("\r\n \r\n"), None);
	}
}

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
// says nothing and lets the bytes through to the terminal. Guessing wrong in the other direction
// means putting a secret dialog in front of the user for a line that was really the root shell's
// own prompt — and whatever they typed would land on that shell's command line. A missed prompt
// costs the user one thing: typing the code into the terminal themselves, exactly as they do today.

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
pub enum Kind {
	/// `sudo -u <user> -i`: a login shell for `user`, authenticated with the caller's password.
	#[default]
	Sudo,
	/// `su - <user>`: a login shell for `user`, authenticated with that account's own password.
	Su,
}

impl Kind {
	/// The label shown beside the choice in the elevate dialog.
	pub fn label(self) -> &'static str {
		match self {
			Kind::Sudo => "sudo",
			Kind::Su => "su",
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
			Kind::Sudo => format!(
				"sudo -p {} -u {quoted} -i",
				crate::explorer::shell_quote(MARKER)
			),
			// A bare `-` is what makes it a LOGIN shell (fresh environment, the account's own
			// $HOME and $PATH), matching what `sudo -i` gives.
			Kind::Su => format!("su - {quoted}"),
		}
	}
}

/// Whether `user` is a plausible account name, checked before it is put in a command line.
///
/// Deliberately narrower than what a system will accept: letters, digits, and `._-`, not starting
/// with `-` (which a command would read as an option) and not empty. A name outside this set is
/// refused in the dialog rather than quoted and hoped for — the field feeds a command that runs on
/// a remote machine as another user, which is exactly the boundary to validate at, not near.
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_sudo_command_names_its_own_prompt_and_its_target() {
		let command = Kind::Sudo.command("root");
		assert!(
			command.contains(MARKER),
			"the prompt is cmote's to recognise"
		);
		assert!(command.contains("-u 'root'"), "and the target is explicit");
		assert!(command.ends_with("-i"), "a LOGIN shell, not a bare one");
	}

	#[test]
	fn su_takes_a_dash_so_it_is_a_login_shell_too() {
		assert_eq!(Kind::Su.command("postgres"), "su - 'postgres'");
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

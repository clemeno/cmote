// integration.rs — the shell-integration block cmote can install on a remote (PLAN §17, §34).
//
// SSH tells a client nothing about where the remote shell *is*, so cmote reads the directory the
// shell ANNOUNCES in an OSC sequence on each prompt (`term::cwd`). fish and a Windows OSC 9;9 shell
// announce it out of the box; a plain bash or zsh says nothing at all, and on such a remote every
// feature built on the cwd is dark: the window title has no directory, Sync and Reveal stay dimmed,
// the upload dialog asks for a path, and — the one that prompted this — §22 has no `terminal_path`
// to remember, so a reconnect cannot resume where the session left off.
//
// §17 settled how NOT to fix that: cmote types nothing into the remote shell. A `PROMPT_COMMAND`
// typed at a prompt is recorded in the remote's command history and recalled on up-arrow, however
// well its echo is hidden — the shell is the user's, and cmote does not scribble in it.
//
// This module is the other way, and the one every terminal with shell integration offers: write the
// announcer into the user's shell CONFIG, once, as an explicit act. Nothing is typed, nothing lands
// in history, and it applies to every later login — cmote's and anyone else's, since the sequences
// are the cross-terminal conventions rather than anything of ours. It is still a change to the
// user's account, so it only ever happens from the dialog that shows the exact text first.
//
// The module is PURE: the block, the marker that finds it again, and the two string edits that put
// it in or take it out. `ssh::integration` does the reading and writing. That split is what lets the
// dialog show the user the very bytes that will be appended — same function, no round trip — and
// lets every rule here be unit-tested without a server.

/// The line that opens cmote's block, and the one that closes it. Together they are the whole
/// bookkeeping: a file is "already set up" if it contains the opening line, and removing the block
/// is finding the pair and cutting between them. Written as comments so the block explains itself
/// to whoever opens the file next — and so a shell reading the file ignores them.
///
/// The `>>>` / `<<<` shape is conda's, borrowed on purpose: it is the marker style users have
/// already seen in their own rc files, so it reads as "a tool put this here" without a legend.
pub const BEGIN: &str = "# >>> cmote shell integration >>>";
pub const END: &str = "# <<< cmote shell integration <<<";

/// The shell family a remote account logs into, which decides both the config file to write and the
/// block to write into it. Only the families cmote can actually help are named: `Fish` is here to be
/// RECOGNISED and left alone, since it announces its directory itself and has nothing to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntegrationShell {
	Bash,
	Zsh,
	/// fish 3.1 and later emit OSC 7 from their own prompt, so there is nothing for cmote to add.
	Fish,
}

impl IntegrationShell {
	/// Read a shell family out of a login-shell path — `/bin/bash`, `/usr/bin/zsh`. Unknown shells
	/// (`/sbin/nologin`, `ksh`, a plain `sh`) answer `None`, which the caller turns into "we could
	/// not tell", never into a guess: writing a bash block into a ksh rc file would break the login.
	pub fn from_login_shell(path: &str) -> Option<Self> {
		let name = path.rsplit('/').next().unwrap_or(path);
		match name {
			"bash" => Some(Self::Bash),
			"zsh" => Some(Self::Zsh),
			"fish" => Some(Self::Fish),
			_ => None,
		}
	}

	/// The config file this shell reads, relative to the account's home directory.
	///
	/// bash gets `.bashrc` rather than `.bash_profile`, even though the shell cmote opens is a LOGIN
	/// shell and bash reads only the profile at login. Every mainstream distribution's default
	/// profile sources `.bashrc` (RHEL/Rocky/CentOS/Amazon ship exactly that line; Debian/Ubuntu's
	/// `.profile` does the same), and `.bashrc` is also what a non-login interactive shell reads — so
	/// it is the file that covers both, and the file every other integration writes to. On an account
	/// whose profile does NOT source it, nothing happens: the dialog says which file was written, so
	/// that shows up as "installed, still silent" rather than as a mystery.
	pub fn rc_file(self) -> &'static str {
		match self {
			Self::Bash => ".bashrc",
			Self::Zsh => ".zshrc",
			// Named for completeness; nothing is ever written here (see `block`).
			Self::Fish => ".config/fish/config.fish",
		}
	}

	/// The human name for the dialog, so it can say which shell it decided on and be corrected by
	/// eye if the guess is wrong.
	pub fn label(self) -> &'static str {
		match self {
			Self::Bash => "bash",
			Self::Zsh => "zsh",
			Self::Fish => "fish",
		}
	}

	/// Whether cmote has anything to install for this shell. False for fish, which already
	/// announces its directory — offering to "fix" a shell that is not broken would be a lie the
	/// user pays for with a change to their config.
	pub fn installable(self) -> bool {
		!matches!(self, Self::Fish)
	}
}

/// The bash block. Everything happens in `PROMPT_COMMAND`, which bash runs just before each prompt:
///
///   * **OSC 7** — the working directory, the point of the exercise (§17).
///   * **OSC 133;D** with `$?` and **OSC 133;A** — the marks that end the command just finished and
///     open the prompt about to be drawn (§34), which light up the prompt ticks, jump-to-prompt and
///     the per-tab exit-code glyph.
///
/// `ponytail:` no **OSC 133;C** (the mark that says a command has started running). bash can only
/// report that through a global `DEBUG` trap, which is a single slot every preexec framework wants
/// and which cmote would be silently taking over. The cost of leaving it out is bounded and known:
/// the tab's status dot never shows "running", and Ctrl+Shift+O finds no output span to select,
/// because a command whose output never started is filed as an empty range (`term::osc133`). Ticks,
/// jumps and the ✓/✗ all work. zsh has proper hooks and gets the mark.
///
/// `ponytail:` the path in the URI is not percent-encoded. Encoding it in portable shell is a
/// per-character loop on every prompt, and cmote's own reader takes a raw path fine; the case it
/// gets wrong is a directory whose name contains a literal `%` followed by two hex digits, which
/// the reader then decodes into something else.
///
/// The sequences end in **BEL** (`\007`) rather than ST (`ESC \`), which is what most integrations
/// in the wild use and what every terminal that reads OSC accepts. It is not a matter of taste:
/// writing ST means a `\\` in the format string, and a `\\` immediately followed by `\033` does NOT
/// come back out of bash's `printf` as backslash-then-escape — the run of backslashes is eaten
/// together and the next sequence is emitted as the literal text `033]7;…`. Found by running the
/// block rather than by reading it, which is the only way this kind of thing is ever found.
const BASH_BLOCK: &str = r#"# Announce the working directory (OSC 7) and the prompt marks (OSC 133)
# on every prompt, which is how a terminal knows where this shell is.
# Installed by cmote. Delete this block to remove it; a terminal that does
# not read these sequences ignores them.
__cmote_report() {
	local __cmote_status=$?
	printf '\033]133;D;%s\007\033]7;file://%s%s\007\033]133;A\007' \
		"$__cmote_status" "${HOSTNAME:-localhost}" "$PWD"
	return $__cmote_status
}
case ";${PROMPT_COMMAND};" in
	*";__cmote_report;"*) ;;
	*) PROMPT_COMMAND="__cmote_report${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac"#;

/// The zsh block. zsh has named hooks for both ends of the command cycle, so it gets the full set
/// of marks with no global trap and no `PS1` surgery: `precmd` runs before each prompt (D, OSC 7,
/// A) and `preexec` just after Enter (C). `add-zsh-hook` refuses to add a function twice, so
/// re-sourcing the file is harmless.
///
/// The exit status is kept in `__cmote_status` rather than the obvious `status`, because in zsh
/// `$status` is already the shell's own synonym for `$?` — a local of that name is asking for the
/// one bug that would report every command as having succeeded.
const ZSH_BLOCK: &str = r#"# Announce the working directory (OSC 7) and the prompt marks (OSC 133),
# which is how a terminal knows where this shell is and what it is doing.
# Installed by cmote. Delete this block to remove it; a terminal that does
# not read these sequences ignores them.
__cmote_precmd() {
	local __cmote_status=$?
	printf '\033]133;D;%s\007\033]7;file://%s%s\007\033]133;A\007' \
		"$__cmote_status" "${HOST:-localhost}" "$PWD"
}
__cmote_preexec() {
	printf '\033]133;C\007'
}
autoload -Uz add-zsh-hook
add-zsh-hook precmd __cmote_precmd
add-zsh-hook preexec __cmote_preexec"#;

/// The whole block for a shell, markers and all — exactly the bytes `install` appends, which is
/// what the dialog shows the user before anything is written. `None` for a shell with nothing to
/// install, so "there is no block" and "here is the block" are distinguishable at the type level
/// rather than by an empty string.
pub fn block(shell: IntegrationShell) -> Option<String> {
	let body = match shell {
		IntegrationShell::Bash => BASH_BLOCK,
		IntegrationShell::Zsh => ZSH_BLOCK,
		IntegrationShell::Fish => return None,
	};
	Some(format!("{BEGIN}\n{body}\n{END}\n"))
}

/// Add the block to a config file's contents, or `None` if it is already there (the marker is the
/// test, so a file cmote has already written to is left exactly as it is — installing twice would
/// announce the directory twice per prompt).
///
/// It goes at the END, after everything the file already does, and that is deliberate: the bash
/// block reads `$PROMPT_COMMAND` to prepend itself to whatever is already set, so it has to run
/// after the lines that set it. A missing final newline is added first, so a file that did not end
/// in one does not get the marker welded onto its last line.
pub fn install_snippet(existing: &str, shell: IntegrationShell) -> Option<String> {
	if existing.contains(BEGIN) {
		return None;
	}
	let block = block(shell)?;
	let mut out = String::with_capacity(existing.len() + block.len() + 2);
	out.push_str(existing);
	if !out.is_empty() {
		if !out.ends_with('\n') {
			out.push('\n');
		}
		// One blank line before the marker, so the block reads as its own paragraph rather than as
		// the continuation of whatever the user's file was saying.
		out.push('\n');
	}
	out.push_str(&block);
	Some(out)
}

/// Cut the block back out, or `None` if it is not there. The pair of markers bounds it exactly, so
/// removal takes back what installation added and nothing else — including the blank line in front,
/// which would otherwise pile up if a user installed and removed a few times.
///
/// A file that has the opening marker but no closing one (someone edited the block by hand and cut
/// it in half) is left ALONE rather than truncated from the marker to the end: the user's own lines
/// may be below it, and losing them to a tidy-up is a far worse outcome than a stray marker.
pub fn remove(existing: &str) -> Option<String> {
	let start = existing.find(BEGIN)?;
	let end = existing[start..]
		.find(END)
		.map(|at| start + at + END.len())?;

	// Everything before the block, with the blank line(s) we put in front of it taken back.
	let head = existing[..start].trim_end_matches(['\n', '\r']);
	// Everything after it, minus the newline that ended the closing marker's own line.
	let tail = existing[end..]
		.strip_prefix("\r\n")
		.or_else(|| existing[end..].strip_prefix('\n'))
		.unwrap_or(&existing[end..]);

	let mut out = String::with_capacity(existing.len());
	out.push_str(head);
	if !out.is_empty() {
		out.push('\n');
	}
	out.push_str(tail);
	Some(out)
}

/// Whether these contents already carry cmote's block.
pub fn installed(existing: &str) -> bool {
	existing.contains(BEGIN)
}

/// The login shell recorded for `user` in an `/etc/passwd` file, or `None` when the account is not
/// in it. That is the authoritative answer to "which shell does a login open", and asking the file
/// is how cmote gets it without typing anything into the shell (§17).
///
/// It is not the only answer: an account served by LDAP/SSSD is not in the file at all, which is
/// why the caller falls back to looking for the config files themselves. A line with fewer than
/// seven fields (a truncated or non-standard file) reports `None` rather than a field from the
/// wrong column.
pub fn login_shell<'a>(passwd: &'a str, user: &str) -> Option<&'a str> {
	passwd
		.lines()
		.filter_map(|line| line.split_once(':'))
		.find(|(name, _)| *name == user)
		// The fields after the name are password, uid, gid, gecos, home, shell — six of them, so
		// the shell is the last, counting from the field after the split.
		.and_then(|(_, rest)| rest.split(':').nth(5))
		.map(str::trim)
		.filter(|shell| !shell.is_empty())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_login_shell_path_names_its_family() {
		assert_eq!(
			IntegrationShell::from_login_shell("/bin/bash"),
			Some(IntegrationShell::Bash)
		);
		assert_eq!(
			IntegrationShell::from_login_shell("/usr/bin/zsh"),
			Some(IntegrationShell::Zsh)
		);
		assert_eq!(
			IntegrationShell::from_login_shell("/usr/local/bin/fish"),
			Some(IntegrationShell::Fish)
		);
		// A shell cmote has no block for is unknown, NOT a bash guess: writing bash syntax into
		// another shell's config is how a login gets broken.
		assert_eq!(IntegrationShell::from_login_shell("/bin/ksh"), None);
		assert_eq!(IntegrationShell::from_login_shell("/sbin/nologin"), None);
	}

	#[test]
	fn fish_has_nothing_to_install() {
		// fish announces its own directory, so cmote offers nothing rather than editing a config
		// to no purpose.
		assert!(!IntegrationShell::Fish.installable());
		assert_eq!(block(IntegrationShell::Fish), None);
		assert_eq!(install_snippet("", IntegrationShell::Fish), None);
	}

	#[test]
	fn the_block_is_appended_whole_with_both_markers() {
		let out = install_snippet("export PATH=$PATH:/opt/bin\n", IntegrationShell::Bash).unwrap();
		assert!(
			out.starts_with("export PATH=$PATH:/opt/bin\n"),
			"the file is kept"
		);
		assert!(
			out.contains(BEGIN) && out.contains(END),
			"both markers land"
		);
		assert!(out.contains("__cmote_report"), "and the announcer itself");
		assert!(out.ends_with('\n'), "the file still ends in a newline");
	}

	#[test]
	fn a_file_with_no_final_newline_is_not_welded_to_the_marker() {
		// A rc file whose last line has no newline is common enough, and appending straight onto it
		// would turn that line into `alias ll='ls -l'# >>> cmote ...` — a comment marker in the
		// middle of a live command.
		let out = install_snippet("alias ll='ls -l'", IntegrationShell::Bash).unwrap();
		assert!(
			out.contains("alias ll='ls -l'\n"),
			"the last line is closed off"
		);
		assert!(
			out.lines().any(|line| line == BEGIN),
			"and the marker is a line of its own"
		);
	}

	#[test]
	fn installing_twice_does_nothing() {
		// The marker is the whole bookkeeping: a second install would announce the directory twice
		// on every prompt, and there is no state anywhere else that would notice.
		let once = install_snippet("# rc\n", IntegrationShell::Bash).unwrap();
		assert!(installed(&once));
		assert_eq!(install_snippet(&once, IntegrationShell::Bash), None);
	}

	#[test]
	fn removing_takes_back_exactly_what_installing_added() {
		// The round trip is the promise the dialog makes: it can be removed. Anything left behind —
		// a blank line, a stray marker — would break that on the second cycle.
		let before = "# rc\nexport EDITOR=vi\n";
		let after = install_snippet(before, IntegrationShell::Zsh).unwrap();
		assert_eq!(remove(&after).as_deref(), Some(before));
	}

	#[test]
	fn removing_keeps_what_was_written_after_the_block() {
		// A user who added their own lines below cmote's block keeps them.
		let installed = install_snippet("# rc\n", IntegrationShell::Bash).unwrap();
		let with_tail = format!("{installed}export LANG=C.UTF-8\n");
		let out = remove(&with_tail).unwrap();
		assert_eq!(out, "# rc\nexport LANG=C.UTF-8\n");
		assert!(!super::installed(&out));
	}

	#[test]
	fn a_half_cut_block_is_left_alone_rather_than_truncated() {
		// The closing marker is gone (someone edited by hand). Cutting from the opening marker to
		// the end of the file would take the user's own lines with it, so nothing is cut at all.
		let mangled = format!("# rc\n{BEGIN}\n__cmote_report() {{ :; }}\nexport LANG=C\n");
		assert_eq!(remove(&mangled), None);
	}

	#[test]
	fn removing_from_a_file_that_never_had_it_reports_nothing_to_do() {
		assert_eq!(remove("# rc\n"), None);
	}

	#[test]
	fn the_login_shell_is_read_out_of_etc_passwd() {
		let passwd = "root:x:0:0:root:/root:/bin/bash\n\
			rocky:x:1000:1000:Rocky:/home/rocky:/usr/bin/zsh\n\
			nobody:x:65534:65534:Nobody:/:/sbin/nologin\n";
		assert_eq!(login_shell(passwd, "root"), Some("/bin/bash"));
		assert_eq!(login_shell(passwd, "rocky"), Some("/usr/bin/zsh"));
		assert_eq!(login_shell(passwd, "nobody"), Some("/sbin/nologin"));
		// An account served by LDAP/SSSD is not in the file — the caller then falls back to
		// looking for the config files, rather than assuming a shell.
		assert_eq!(login_shell(passwd, "ldapuser"), None);
	}

	#[test]
	fn a_passwd_line_that_is_short_or_odd_reports_nothing() {
		// A truncated line must not hand back the home directory as though it were the shell: a
		// wrong answer here would write a bash block into whatever path came out.
		assert_eq!(login_shell("half:x:0:0:root:/root\n", "half"), None);
		assert_eq!(login_shell("empty:x:0:0::/root:\n", "empty"), None);
		// The name is matched whole, so a prefix of another account is not mistaken for it.
		assert_eq!(login_shell("rockyadmin:x:1:1::/h:/bin/sh\n", "rocky"), None);
	}

	#[test]
	fn every_installable_shell_writes_the_sequences_cmote_reads() {
		// The block is only worth writing if `term::cwd` and `term::osc133` can read what it emits,
		// and those two read `ESC ] 7 ;` and `ESC ] 133 ;`. Written as a test because the block is a
		// string literal: nothing else would notice a typo in an escape until a real server did.
		for shell in [IntegrationShell::Bash, IntegrationShell::Zsh] {
			let block = block(shell).unwrap();
			assert!(
				block.contains(r"\033]7;file://"),
				"{} announces the cwd",
				shell.label()
			);
			assert!(
				block.contains(r"\033]133;A"),
				"{} opens the prompt",
				shell.label()
			);
			assert!(
				block.contains(r"\033]133;D;%s"),
				"{} reports the exit code",
				shell.label()
			);
			// BEL ends every one of them, and no ST does. Not a style rule: `\\` immediately
			// followed by `\033` does not survive bash's printf — the backslashes are eaten
			// together and the next sequence comes out as the literal text `033]7;…` — so a stray
			// ST in here is a block that silently announces nothing (see `BASH_BLOCK`).
			assert!(
				block.contains(r"\007"),
				"{} terminates its strings with BEL",
				shell.label()
			);
			assert!(
				!block.contains(r"\033\\"),
				"{} has no ST left in it",
				shell.label()
			);
		}
	}
}

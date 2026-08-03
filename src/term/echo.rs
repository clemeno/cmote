// term/echo.rs — the cwd announcer cmote types into a silent shell, and the elider that
// hides its echo (PLAN §17).
//
// SSH tells a client nothing about where the remote shell *is*; a shell has to announce its
// directory in an OSC escape for the terminal to read it (see `term::cwd`). fish does this out
// of the box, and a Windows shell may emit OSC 9;9 — those cmote reads passively, injecting
// nothing. bash and zsh announce nothing on their own, so for them cmote installs the announcer
// itself: `CWD_HOOK` below, one line that defines a function printing OSC 7 and hooks it into the
// prompt so the reported directory follows every `cd`.
//
// cmote does not inject blindly. On connect the shell is watched for a moment (the app's cwd
// probe, `app::CWD_PROBE`); only if it has announced nothing by then is `CWD_HOOK` typed in. A
// shell that speaks for itself (fish, a configured bash/zsh) is left untouched — no wasted line,
// and no bash syntax spat at a shell that would choke on it.
//
// The catch is the echo. The hook is sent as pty input, and an interactive shell echoes what is
// typed at it — the remote's line editor (readline in bash, zle in zsh) writes each character
// back into the output stream — so the whole setup line would surface once as a wall of shell
// source the user never asked to see. It cannot be silenced at the source: `stty -echo` does not
// stop it, because the line editor drives the terminal itself and echoes regardless of the tty's
// echo flag.
//
// So, since cmote *is* the terminal, `HookEcho` elides that echo from its own display. It is
// armed only when cmote injects (`Terminal::begin_cwd_injection`); a passive session never arms
// it and it stays a transparent pass-through. Once armed it watches the DISPLAY stream for the
// hook's signature — the literal function name `cmote_cwd(`, which nothing a shell prints before
// a prompt begins with — and drops everything from it through the line's terminating newline,
// which it replaces with a clean CR+LF so the next prompt lands back at the left margin. It
// disarms after that one line: a later `cat` of a script mentioning `cmote_cwd(` is shown intact.
// A shell whose editor echoes differently simply never matches and shows the line as before —
// graceful, and the stream is never corrupted (only the signature and the bytes up to its newline
// are ever dropped, and only on a full match).
//
// Like the cwd tracker this is a byte state machine, not a regex over a buffer, because output
// arrives in arbitrary chunks and the signature can be split anywhere.

use std::borrow::Cow;

/// The announcer cmote types into a shell that reports no directory of its own (§17). It defines
/// `cmote_cwd` — a `printf` of an OSC 7 sequence, `ESC ] 7 ; file://host/path ESC \`, which is
/// invisible on screen and read by `term::cwd` — and hooks it into `PROMPT_COMMAND` (bash) and
/// `precmd_functions` (zsh) so it fires on every prompt, then calls it once for the starting
/// directory. bash and zsh only; other shells are covered passively or leave the cwd unknown, and
/// the trailing `\n` submits the line. Its echo is hidden by `HookEcho`, armed as it is sent.
pub const CWD_HOOK: &str = concat!(
	r#"cmote_cwd() { printf '\033]7;file://%s%s\033\\' "${HOSTNAME-}" "$PWD"; }; "#,
	r#"PROMPT_COMMAND="cmote_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}"; "#,
	r#"precmd_functions+=(cmote_cwd); cmote_cwd"#,
	"\n",
);

/// The line feed that ends the echoed command once the shell's line editor accepts it.
const LF: u8 = b'\n';

/// The signature the hook's echo opens with — the head of `CWD_HOOK`'s `cmote_cwd() {…}`
/// definition. Matching the opening paren keeps it from firing on a bare word `cmote_cwd` a
/// program might print, though arming only around an injection already guards against that.
const SIGNATURE: &[u8] = b"cmote_cwd(";

/// Where the eliding scanner is in the stream.
#[derive(Debug, Default, PartialEq, Eq)]
enum State {
	/// Not eliding: either never armed (a passive session), or the one line is already gone.
	/// Output passes straight through.
	#[default]
	Idle,
	/// Armed and hunting the signature, passing ordinary output through as it goes.
	Arming,
	/// Inside the echoed line: drop every byte until its newline.
	Eliding,
}

/// Hides the one echoed setup line from what the grid displays (§17). Constructed idle — a
/// transparent pass-through that costs nothing — and armed only when cmote actually types the
/// cwd hook in; it self-disarms the moment that line is gone. Feed it each chunk of shell output
/// before the emulator draws it. The cwd tracker still reads the raw bytes: the real OSC 7 the
/// hook prints arrives *after* this line, and in any case the echoed `\033` is literal text, not
/// a real escape, so it never confuses the tracker either way.
#[derive(Debug, Default)]
pub struct HookEcho {
	state: State,
	/// Bytes that match a prefix of the signature so far, held back until we know whether the
	/// whole signature follows — once emitted to the grid they could not be taken back off the
	/// screen. Empty except mid-match.
	pending: Vec<u8>,
}

impl HookEcho {
	/// Arm the elider to hide the next echoed setup line. Called as cmote injects `CWD_HOOK`, so
	/// the line the shell will echo back never reaches the grid. A passive session never calls
	/// this and the elider stays idle.
	pub fn arm(&mut self) {
		self.state = State::Arming;
		self.pending.clear();
	}

	/// Filter a chunk of shell output for display, dropping the one echoed setup line once armed.
	/// Safe at any chunk boundary — the state carries across calls. Idle (unarmed, or already
	/// done) returns the chunk borrowed, so the steady state allocates nothing; only the brief
	/// window between arming and the elided line copies.
	pub fn filter<'a>(&mut self, bytes: &'a [u8]) -> Cow<'a, [u8]> {
		if self.state == State::Idle {
			return Cow::Borrowed(bytes);
		}
		let mut out = Vec::with_capacity(bytes.len() + self.pending.len());
		for &byte in bytes {
			match self.state {
				State::Arming => self.arm_step(byte, &mut out),
				State::Eliding => {
					// Drop the echoed command. When its newline lands, put back a clean CR+LF —
					// the elided command left our cursor mid-line, so without the CR the next
					// prompt would print indented under where the hidden command sat — then hand
					// the rest of the stream (the invisible OSC 7, the fresh prompt) straight on.
					if byte == LF {
						out.extend_from_slice(b"\r\n");
						self.state = State::Idle;
					}
				}
				State::Idle => out.push(byte),
			}
		}
		Cow::Owned(out)
	}

	/// One byte while still hunting the signature: hold it if it extends the match, emit the held
	/// prefix and re-test the byte if it breaks the match, and switch to eliding once the whole
	/// signature is in hand.
	fn arm_step(&mut self, byte: u8, out: &mut Vec<u8>) {
		if byte == SIGNATURE[self.pending.len()] {
			self.pending.push(byte);
			if self.pending.len() == SIGNATURE.len() {
				// The whole signature matched: this is the hook's echo. Drop the held bytes —
				// they belong to the line we are hiding — and elide the rest through its newline.
				self.pending.clear();
				self.state = State::Eliding;
			}
		} else {
			// The held bytes were ordinary output after all, so show them. The current byte may
			// itself open a fresh match (the signature's first byte), so seed the held prefix with
			// it in that case rather than emit it and miss the start.
			out.append(&mut self.pending);
			if byte == SIGNATURE[0] {
				self.pending.push(byte);
			} else {
				out.push(byte);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed a sequence of chunks to a freshly ARMED elider and collect what it would display.
	fn elide(chunks: &[&[u8]]) -> Vec<u8> {
		let mut echo = HookEcho::default();
		echo.arm();
		let mut out = Vec::new();
		for chunk in chunks {
			out.extend_from_slice(&echo.filter(chunk));
		}
		out
	}

	#[test]
	fn the_signature_is_the_head_of_the_injected_hook() {
		// The two halves of the feature must agree: the elider can only hide what the hook opens
		// with. Guard against either drifting from the other.
		assert!(CWD_HOOK.as_bytes().starts_with(SIGNATURE));
	}

	#[test]
	fn an_idle_elider_passes_everything_through() {
		// A passive session (fish, a Windows shell) never arms it, so even a line that looks
		// exactly like the hook's echo must reach the screen untouched.
		let mut echo = HookEcho::default();
		let line = b"cmote_cwd() { ... }; cmote_cwd\r\n$ ";
		assert_eq!(echo.filter(line).as_ref(), line);
	}

	#[test]
	fn the_hooks_echo_is_dropped_from_the_display() {
		// A prompt, the shell echoing the whole setup line, then a fresh prompt: only the echoed
		// line disappears, and the prompts around it are untouched.
		let out = elide(&[b"user@host:~$ cmote_cwd() { printf ...; }; cmote_cwd\r\nuser@host:~$ "]);
		assert_eq!(out, b"user@host:~$ \r\nuser@host:~$ ");
	}

	#[test]
	fn a_signature_split_across_chunks_is_still_caught() {
		// Output arrives in arbitrary chunks — including a split part-way through the signature.
		let out = elide(&[b"$ cmote_c", b"wd() {}; cmote_cwd\r\n$ "]);
		assert_eq!(out, b"$ \r\n$ ");
	}

	#[test]
	fn a_word_that_only_resembles_the_signature_is_left_alone() {
		// `cmote_cwd` with no opening paren is a near-miss — the held prefix must be shown, not
		// eaten, so a bare mention of the function survives.
		let out = elide(&[b"see cmote_cwd in the docs\n"]);
		assert_eq!(out, b"see cmote_cwd in the docs\n");
	}

	#[test]
	fn only_the_first_matching_line_is_hidden() {
		// After the setup line is gone the elider is idle again: a command that literally prints
		// `cmote_cwd(` later is shown in full.
		let out = elide(&[
			b"$ cmote_cwd() {}; cmote_cwd\r\n",
			b"$ grep cmote_cwd( file\r\n",
		]);
		assert_eq!(out, b"$ \r\n$ grep cmote_cwd( file\r\n");
	}

	#[test]
	fn the_elided_line_ends_at_the_left_margin() {
		// The line editor may end the line with a bare LF; we still emit CR+LF so the next prompt
		// is not left indented under where the hidden command sat.
		let out = elide(&[b"$ cmote_cwd()\nx"]);
		assert_eq!(out, b"$ \r\nx");
	}

	#[test]
	fn output_before_the_setup_line_passes_through_untouched() {
		// A login banner (which precedes the hook's echo, and can contain a stray `c`) must reach
		// the screen exactly as sent.
		let out = elide(&[b"Welcome to host!\nLast login: today\n"]);
		assert_eq!(out, b"Welcome to host!\nLast login: today\n");
	}
}

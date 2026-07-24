// term/keymap.rs — translate GUI key presses into the bytes a terminal sends
// down the SSH channel (PLAN §9).
//
// A terminal is dumb on the way in: it just forwards bytes. Printable keys send
// their character(s); everything else sends a small agreed-upon code:
//   * control combos     -> C0 control bytes (Ctrl-C = 0x03, Ctrl-D = 0x04, …)
//   * Enter/Tab/Backspace -> \r, \t, 0x7f
//   * arrows/Home/End/…   -> CSI escape sequences ("\x1b[A", …)
// The remote pty and the shell agree on these conventions (the "xterm" model we
// asked for when opening the pty), so we emit exactly what a real xterm would.

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};

/// ASCII escape (`ESC`), the lead byte of every CSI sequence and the meta prefix.
const ESC: u8 = 0x1b;

/// Bracketed-paste markers (DECSET 2004). When the remote program enables the
/// mode it wants pasted text framed by these so it can tell a paste from typed
/// input — readline/editors then insert the whole block literally instead of
/// acting on embedded newlines and control characters (§9).
const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

/// Turn one key press into the bytes to send, or `None` if the key produces no
/// input (a bare modifier, an unmapped named key). `text` is the OS-produced
/// string for the key (already honoring layout and Shift); we prefer it for
/// printable input and fall back to the logical key only when it is absent.
/// `application_cursor` is the emulator's DECCKM state (read from
/// `screen.application_cursor()`): full-screen apps such as vim, less, and nano
/// turn it on and then expect the SS3 arrow-key form, so it is threaded down to
/// `named_bytes` to pick the matching cursor-key encoding.
pub fn encode(
	key: &Key,
	text: Option<&str>,
	modifiers: Modifiers,
	application_cursor: bool,
) -> Option<Vec<u8>> {
	// Control combos first, so Ctrl-C beats the plain 'c' it would otherwise be.
	// (Alt is excluded here; Alt+Ctrl combos are rare and left to the OS.)
	if modifiers.control()
		&& !modifiers.alt()
		&& let Key::Character(character) = key
		&& let Some(byte) = control_byte(character)
	{
		return Some(vec![byte]);
	}

	match key {
		// Named keys map to their fixed control byte or escape sequence.
		Key::Named(named) => named_bytes(named, application_cursor),

		// A printable key: send its produced text. Alt acts as "meta", which the
		// xterm convention encodes as an ESC prefix before the character.
		Key::Character(character) => {
			let produced = text.unwrap_or(character.as_str());
			let mut out = Vec::with_capacity(produced.len() + 1);
			if modifiers.alt() {
				out.push(ESC);
			}
			out.extend_from_slice(produced.as_bytes());
			Some(out)
		}

		// Unknown key: forward whatever text the OS produced, if any.
		Key::Unidentified => text.map(|value| value.as_bytes().to_vec()),
	}
}

/// Encode clipboard text for a paste into the shell (§9). When `bracketed` is
/// true — the remote enabled DECSET 2004, which the caller reads from the
/// emulator's `bracketed_paste()` state — wrap the text in the paste markers so
/// the shell treats it as one literal block; otherwise send the bytes as they are.
///
/// SECURITY: a hostile clipboard could embed the end marker `ESC[201~` in its
/// payload to close the bracket early and have whatever follows run as typed
/// commands — a paste-injection. Legitimate pasted text never contains that
/// marker, so we strip every occurrence before wrapping (this mirrors what xterm
/// does). Without bracketing there is nothing to break out of, so the raw bytes
/// go through unchanged.
///
/// `ponytail:` in the non-bracketed case, embedded newlines in the paste execute
/// immediately — that is how a plain terminal has always behaved, and bracketed
/// paste (which most modern shells enable) is the fix. We do not second-guess it
/// with our own confirmation prompt in v1.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
	if !bracketed {
		return text.as_bytes().to_vec();
	}
	let bytes = text.as_bytes();
	let mut out = Vec::with_capacity(bytes.len() + PASTE_START.len() + PASTE_END.len());
	out.extend_from_slice(PASTE_START);
	scrub_end_marker(bytes, &mut out);
	out.extend_from_slice(PASTE_END);
	out
}

/// Copy `bytes` into `out`, dropping every embedded `ESC[201~` end marker. A
/// single left-to-right pass: at each position, if the end marker starts here skip
/// past it, otherwise keep the byte. Stripping (rather than escaping) is enough —
/// the terminator is meaningless in pasted content, so losing it changes nothing a
/// user intended.
fn scrub_end_marker(bytes: &[u8], out: &mut Vec<u8>) {
	let mut index = 0;
	while index < bytes.len() {
		if bytes[index..].starts_with(PASTE_END) {
			index += PASTE_END.len();
		} else {
			out.push(bytes[index]);
			index += 1;
		}
	}
}

/// The C0 control byte for a Ctrl+<char> combo, following the standard mapping
/// (Ctrl-A..Ctrl-Z = 0x01..0x1a, plus the handful of symbol combos). `None` for
/// characters that have no control code.
fn control_byte(character: &str) -> Option<u8> {
	let first = character.chars().next()?;
	let byte = match first {
		'a'..='z' => first as u8 - b'a' + 1,
		'A'..='Z' => first as u8 - b'A' + 1,
		'@' | ' ' => 0x00,
		'[' => 0x1b,
		'\\' => 0x1c,
		']' => 0x1d,
		'^' => 0x1e,
		'_' => 0x1f,
		'?' => 0x7f,
		_ => return None,
	};
	Some(byte)
}

/// The bytes for a named (non-character) key. Returns `None` for named keys we
/// do not forward (bare modifiers, function keys we have not mapped yet). The
/// cursor and Home/End keys depend on `application_cursor`: see `cursor_key`.
fn named_bytes(named: &Named, application_cursor: bool) -> Option<Vec<u8>> {
	let sequence: &[u8] = match named {
		Named::Enter => b"\r",
		Named::Tab => b"\t",
		Named::Space => b" ",
		Named::Backspace => &[0x7f],
		Named::Escape => &[ESC],
		// Cursor and Home/End keys carry the DECCKM-dependent prefix, so they build
		// their bytes through `cursor_key` and return directly.
		Named::ArrowUp => return Some(cursor_key(b'A', application_cursor)),
		Named::ArrowDown => return Some(cursor_key(b'B', application_cursor)),
		Named::ArrowRight => return Some(cursor_key(b'C', application_cursor)),
		Named::ArrowLeft => return Some(cursor_key(b'D', application_cursor)),
		Named::Home => return Some(cursor_key(b'H', application_cursor)),
		Named::End => return Some(cursor_key(b'F', application_cursor)),
		// The remaining navigation keys are the "~" CSI sequences, which DECCKM does
		// not change, so they are the same in both modes.
		Named::Insert => b"\x1b[2~",
		Named::Delete => b"\x1b[3~",
		Named::PageUp => b"\x1b[5~",
		Named::PageDown => b"\x1b[6~",
		_ => return None,
	};
	Some(sequence.to_vec())
}

/// Encode one cursor/navigation key given its final byte (`A`=Up, `B`=Down,
/// `C`=Right, `D`=Left, `H`=Home, `F`=End). Only the prefix differs by mode: in
/// application cursor mode (DECCKM set) a real xterm sends the SS3 form `ESC O <b>`,
/// otherwise the CSI form `ESC [ <b>`. The two share the final byte, so we pick the
/// second byte and reuse the rest. Getting this wrong is exactly why full-screen
/// apps ignore the arrow keys: vim binds them to the SS3 form once it enables DECCKM.
fn cursor_key(final_byte: u8, application_cursor: bool) -> Vec<u8> {
	let prefix = if application_cursor { b'O' } else { b'[' };
	vec![ESC, prefix, final_byte]
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::keyboard::key::Named;

	// A convenience: no modifiers held.
	fn none() -> Modifiers {
		Modifiers::empty()
	}

	#[test]
	fn plain_character_sends_its_text() {
		let key = Key::Character("a".into());
		assert_eq!(encode(&key, Some("a"), none(), false), Some(b"a".to_vec()));
	}

	#[test]
	fn shifted_character_uses_produced_text() {
		// The OS reports the logical key as "a" but the produced text as "A".
		let key = Key::Character("a".into());
		assert_eq!(
			encode(&key, Some("A"), Modifiers::SHIFT, false),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn ctrl_c_is_etx() {
		let key = Key::Character("c".into());
		assert_eq!(encode(&key, None, Modifiers::CTRL, false), Some(vec![0x03]));
	}

	#[test]
	fn enter_is_carriage_return() {
		let key = Key::Named(Named::Enter);
		assert_eq!(
			encode(&key, Some("\r"), none(), false),
			Some(b"\r".to_vec())
		);
	}

	#[test]
	fn arrow_up_is_csi_sequence_in_normal_mode() {
		// DECCKM reset (the shell's default): arrows are the CSI form ESC[A.
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(encode(&key, None, none(), false), Some(b"\x1b[A".to_vec()));
	}

	#[test]
	fn arrow_up_is_ss3_sequence_in_application_mode() {
		// DECCKM set (vim/less/nano): arrows switch to the SS3 form ESC O A, which is
		// what those apps bind their arrow keys to — the fix for "arrows do nothing".
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(encode(&key, None, none(), true), Some(b"\x1bOA".to_vec()));
	}

	#[test]
	fn home_and_end_follow_the_cursor_mode_too() {
		// Home/End share the DECCKM behaviour: CSI when reset, SS3 when set.
		let home = Key::Named(Named::Home);
		let end = Key::Named(Named::End);
		assert_eq!(encode(&home, None, none(), false), Some(b"\x1b[H".to_vec()));
		assert_eq!(encode(&home, None, none(), true), Some(b"\x1bOH".to_vec()));
		assert_eq!(encode(&end, None, none(), false), Some(b"\x1b[F".to_vec()));
		assert_eq!(encode(&end, None, none(), true), Some(b"\x1bOF".to_vec()));
	}

	#[test]
	fn tilde_navigation_keys_ignore_cursor_mode() {
		// PageUp/PageDown/Insert/Delete are "~" sequences DECCKM does not touch, so
		// application mode leaves them unchanged.
		let page_up = Key::Named(Named::PageUp);
		assert_eq!(
			encode(&page_up, None, none(), false),
			encode(&page_up, None, none(), true)
		);
		assert_eq!(
			encode(&page_up, None, none(), true),
			Some(b"\x1b[5~".to_vec())
		);
	}

	#[test]
	fn alt_character_gets_esc_prefix() {
		let key = Key::Character("x".into());
		assert_eq!(
			encode(&key, Some("x"), Modifiers::ALT, false),
			Some(b"\x1bx".to_vec())
		);
	}

	#[test]
	fn bare_modifier_key_sends_nothing() {
		let key = Key::Named(Named::Shift);
		assert_eq!(encode(&key, None, none(), false), None);
	}

	#[test]
	fn paste_without_bracketing_is_raw() {
		assert_eq!(encode_paste("ls -la\n", false), b"ls -la\n".to_vec());
	}

	#[test]
	fn paste_with_bracketing_is_wrapped() {
		// The text is framed by ESC[200~ … ESC[201~ so the shell inserts it literally.
		let out = encode_paste("hi", true);
		assert_eq!(out, b"\x1b[200~hi\x1b[201~".to_vec());
	}

	#[test]
	fn paste_strips_embedded_end_marker() {
		// A hostile clipboard tries to close the bracket early and inject a command.
		// The embedded ESC[201~ must be removed so only one real terminator remains.
		let payload = "safe\x1b[201~rm -rf /";
		let out = encode_paste(payload, true);
		assert_eq!(out, b"\x1b[200~saferm -rf /\x1b[201~".to_vec());
		// Exactly one terminator survives: the one we appended.
		let terminators = out
			.windows(PASTE_END.len())
			.filter(|window| *window == PASTE_END)
			.count();
		assert_eq!(terminators, 1);
	}

	#[test]
	fn paste_keeps_the_start_marker_since_it_cannot_break_out() {
		// Only the end marker enables injection; an embedded start marker is harmless
		// and left in place (matching xterm, which filters just the terminator).
		let out = encode_paste("a\x1b[200~b", true);
		assert_eq!(out, b"\x1b[200~a\x1b[200~b\x1b[201~".to_vec());
	}
}

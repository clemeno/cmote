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

use iced::keyboard::key::{Code, Named, Physical};
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
/// input (a bare modifier, an unmapped named key). `physical` is the key's physical
/// location on the keyboard (used only to single out the numpad — see below). `text`
/// is the OS-produced string for the key (already honoring layout and Shift); we
/// prefer it for printable input and fall back to the logical key only when it is
/// absent. `application_cursor` is the emulator's DECCKM state (read from
/// `screen.application_cursor()`): full-screen apps such as vim, less, and nano
/// turn it on and then expect the SS3 arrow-key form, so it is threaded down to
/// `named_bytes` to pick the matching cursor-key encoding.
pub fn encode(
	key: &Key,
	physical: Physical,
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

	// The numpad number keys (0-9 and the decimal) have a dual identity that the
	// logical `key` alone hides. With NumLock on they type a digit; with NumLock off
	// the same physical key is navigation (arrow / Home / PageUp / …). winit reports
	// the logical `key` for the *navigation* role — so a NumLock-on digit can arrive
	// as `Named(ArrowDown)` and, matched on `key`, would send an arrow instead of "2"
	// (the `pm2 ls` bug). iced does not expose NumLock state, but the OS-produced
	// `text` is the tell: it is present exactly when a character was typed. So for a
	// numpad number key with printable text (and no Ctrl/Alt/Logo turning it into a
	// combo), send that text. NumLock-off presses carry no text and fall through to
	// the navigation mapping below. Scoped to numpad physical codes so it can never
	// disturb Backspace or the main-row keys.
	if is_numpad_number(physical)
		&& !modifiers.control()
		&& !modifiers.alt()
		&& !modifiers.logo()
		&& let Some(typed) = text
		&& !typed.is_empty()
	{
		return Some(typed.as_bytes().to_vec());
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

/// Whether `physical` is one of the numpad number keys — the digits `0`-`9` or the
/// decimal `.`. These are the keys whose meaning flips with NumLock (digit vs.
/// navigation), so `encode` treats them specially; every other key (including the
/// numpad operators `+ - * / Enter`, which never navigate) is left to the normal
/// logical-key path.
fn is_numpad_number(physical: Physical) -> bool {
	matches!(
		physical,
		Physical::Code(
			Code::Numpad0
				| Code::Numpad1
				| Code::Numpad2
				| Code::Numpad3
				| Code::Numpad4
				| Code::Numpad5
				| Code::Numpad6
				| Code::Numpad7
				| Code::Numpad8
				| Code::Numpad9
				| Code::NumpadDecimal
		)
	)
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
		// The function keys, exactly as the terminfo entry for the pty we asked for
		// (`xterm-256color`) describes them — which is what a remote program looks them up
		// in. The split is historical, not arbitrary: F1-F4 inherited the VT100 keypad's
		// SS3 form (`kf1=\EOP`), F5 onwards are the later CSI "~" form with a gap in the
		// numbering at 16 and 22. Getting one wrong means a dead key in every full-screen
		// program — btop's options menu is F2, midnight commander lives on F1-F10.
		Named::F1 => b"\x1bOP",
		Named::F2 => b"\x1bOQ",
		Named::F3 => b"\x1bOR",
		Named::F4 => b"\x1bOS",
		Named::F5 => b"\x1b[15~",
		Named::F6 => b"\x1b[17~",
		Named::F7 => b"\x1b[18~",
		Named::F8 => b"\x1b[19~",
		Named::F9 => b"\x1b[20~",
		Named::F10 => b"\x1b[21~",
		Named::F11 => b"\x1b[23~",
		Named::F12 => b"\x1b[24~",
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
	use iced::keyboard::key::{Code, Named, Physical};

	// A convenience: no modifiers held.
	fn none() -> Modifiers {
		Modifiers::empty()
	}

	// Wrap a physical `Code` as the `Physical` `encode` expects. Most tests are about
	// the logical key, so they pass a neutral non-numpad code via `main` below.
	fn phys(code: Code) -> Physical {
		Physical::Code(code)
	}

	// A neutral, non-numpad physical key for tests that do not exercise the numpad.
	fn main_key() -> Physical {
		phys(Code::KeyA)
	}

	#[test]
	fn plain_character_sends_its_text() {
		let key = Key::Character("a".into());
		assert_eq!(
			encode(&key, main_key(), Some("a"), none(), false),
			Some(b"a".to_vec())
		);
	}

	#[test]
	fn shifted_character_uses_produced_text() {
		// The OS reports the logical key as "a" but the produced text as "A".
		let key = Key::Character("a".into());
		assert_eq!(
			encode(&key, main_key(), Some("A"), Modifiers::SHIFT, false),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn ctrl_c_is_etx() {
		let key = Key::Character("c".into());
		assert_eq!(
			encode(&key, main_key(), None, Modifiers::CTRL, false),
			Some(vec![0x03])
		);
	}

	#[test]
	fn enter_is_carriage_return() {
		let key = Key::Named(Named::Enter);
		assert_eq!(
			encode(&key, phys(Code::Enter), Some("\r"), none(), false),
			Some(b"\r".to_vec())
		);
	}

	#[test]
	fn arrow_up_is_csi_sequence_in_normal_mode() {
		// DECCKM reset (the shell's default): arrows are the CSI form ESC[A.
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(
			encode(&key, phys(Code::ArrowUp), None, none(), false),
			Some(b"\x1b[A".to_vec())
		);
	}

	#[test]
	fn arrow_up_is_ss3_sequence_in_application_mode() {
		// DECCKM set (vim/less/nano): arrows switch to the SS3 form ESC O A, which is
		// what those apps bind their arrow keys to — the fix for "arrows do nothing".
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(
			encode(&key, phys(Code::ArrowUp), None, none(), true),
			Some(b"\x1bOA".to_vec())
		);
	}

	#[test]
	fn home_and_end_follow_the_cursor_mode_too() {
		// Home/End share the DECCKM behaviour: CSI when reset, SS3 when set.
		let home = Key::Named(Named::Home);
		let end = Key::Named(Named::End);
		assert_eq!(
			encode(&home, phys(Code::Home), None, none(), false),
			Some(b"\x1b[H".to_vec())
		);
		assert_eq!(
			encode(&home, phys(Code::Home), None, none(), true),
			Some(b"\x1bOH".to_vec())
		);
		assert_eq!(
			encode(&end, phys(Code::End), None, none(), false),
			Some(b"\x1b[F".to_vec())
		);
		assert_eq!(
			encode(&end, phys(Code::End), None, none(), true),
			Some(b"\x1bOF".to_vec())
		);
	}

	#[test]
	fn tilde_navigation_keys_ignore_cursor_mode() {
		// PageUp/PageDown/Insert/Delete are "~" sequences DECCKM does not touch, so
		// application mode leaves them unchanged.
		let page_up = Key::Named(Named::PageUp);
		assert_eq!(
			encode(&page_up, phys(Code::PageUp), None, none(), false),
			encode(&page_up, phys(Code::PageUp), None, none(), true)
		);
		assert_eq!(
			encode(&page_up, phys(Code::PageUp), None, none(), true),
			Some(b"\x1b[5~".to_vec())
		);
	}

	#[test]
	fn the_function_keys_follow_the_terminfo_entry_for_our_pty() {
		// F1-F4 are the SS3 form, F5 onwards the CSI "~" form with its historical gaps —
		// 15,17,18,19,20,21,23,24, never 16 or 22. A remote program reads these out of
		// terminfo, so a single wrong byte is a key that does nothing.
		let expected: [(Named, &[u8]); 12] = [
			(Named::F1, b"\x1bOP"),
			(Named::F2, b"\x1bOQ"),
			(Named::F3, b"\x1bOR"),
			(Named::F4, b"\x1bOS"),
			(Named::F5, b"\x1b[15~"),
			(Named::F6, b"\x1b[17~"),
			(Named::F7, b"\x1b[18~"),
			(Named::F8, b"\x1b[19~"),
			(Named::F9, b"\x1b[20~"),
			(Named::F10, b"\x1b[21~"),
			(Named::F11, b"\x1b[23~"),
			(Named::F12, b"\x1b[24~"),
		];
		for (named, bytes) in expected {
			let key = Key::Named(named);
			// The form does not change with the cursor-key mode, so both must match.
			assert_eq!(
				encode(&key, main_key(), None, none(), false).as_deref(),
				Some(bytes)
			);
			assert_eq!(
				encode(&key, main_key(), None, none(), true).as_deref(),
				Some(bytes)
			);
		}
	}

	#[test]
	fn alt_character_gets_esc_prefix() {
		let key = Key::Character("x".into());
		assert_eq!(
			encode(&key, main_key(), Some("x"), Modifiers::ALT, false),
			Some(b"\x1bx".to_vec())
		);
	}

	#[test]
	fn bare_modifier_key_sends_nothing() {
		let key = Key::Named(Named::Shift);
		assert_eq!(
			encode(&key, phys(Code::ShiftLeft), None, none(), false),
			None
		);
	}

	#[test]
	fn numpad_digit_with_numlock_sends_the_digit() {
		// The `pm2 ls` bug: with NumLock on, winit still reports the numpad's logical
		// key as its navigation role (here Down), but fills `text` with the digit. We
		// must send the digit, not an arrow. Physical code Numpad2 + text "2" => "2".
		let key = Key::Named(Named::ArrowDown);
		assert_eq!(
			encode(&key, phys(Code::Numpad2), Some("2"), none(), false),
			Some(b"2".to_vec())
		);
	}

	#[test]
	fn numpad_digit_without_numlock_still_navigates() {
		// NumLock off: the OS produces no text and the logical key is the navigation
		// role, so numpad 2 keeps acting as Down (CSI ESC[B) — unchanged behaviour.
		let key = Key::Named(Named::ArrowDown);
		assert_eq!(
			encode(&key, phys(Code::Numpad2), None, none(), false),
			Some(b"\x1b[B".to_vec())
		);
	}

	#[test]
	fn numpad_decimal_with_numlock_sends_its_character() {
		// The decimal key has the same dual identity (Delete vs "."). With NumLock on
		// the OS produces the locale's separator as text; send it verbatim.
		let key = Key::Named(Named::Delete);
		assert_eq!(
			encode(&key, phys(Code::NumpadDecimal), Some("."), none(), false),
			Some(b".".to_vec())
		);
	}

	#[test]
	fn numpad_enter_is_left_to_the_normal_path() {
		// NumpadEnter is not a "number" key (it never navigates), so the numpad
		// shortcut ignores it and the logical Enter mapping applies: carriage return.
		let key = Key::Named(Named::Enter);
		assert_eq!(
			encode(&key, phys(Code::NumpadEnter), Some("\r"), none(), false),
			Some(b"\r".to_vec())
		);
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

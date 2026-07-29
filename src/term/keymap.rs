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

use super::modkeys::ModifyOtherKeys;

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
/// `named_bytes` to pick the matching cursor-key encoding. `modifiers` is threaded
/// there too: a Ctrl/Shift/Alt held with a navigation or function key encodes the
/// xterm modifier parameter (`CSI 1;<mod><final>` / `CSI <n>;<mod>~`), so a remote
/// editor reads Ctrl+Right as word-motion and Shift+Down as select-line. `modify_other_keys`
/// is the remote's `modifyOtherKeys` level (read from `Terminal::modify_other_keys`): when a
/// program turns it on, a Ctrl/Alt combo on a main-keyboard character is reported as the
/// unambiguous `CSI 27;<mod>;<code>~` form instead of the lossy C0 byte or nothing at all.
pub fn encode(
	key: &Key,
	physical: Physical,
	text: Option<&str>,
	modifiers: Modifiers,
	application_cursor: bool,
	modify_other_keys: ModifyOtherKeys,
) -> Option<Vec<u8>> {
	// modifyOtherKeys first, so at level 2 it claims Ctrl+C before the C0 path turns it into
	// 0x03: an editor that turned the mode on wants the raw key event, not the interrupt byte.
	// Only Ctrl/Alt combos on a printable key are ever claimed — Shift-only and unmodified keys,
	// and every named/navigation/function key, keep their ordinary encoding below.
	if let Key::Character(character) = key
		&& (modifiers.control() || modifiers.alt())
		&& let Some(bytes) = modify_other_key(character, modifiers, modify_other_keys)
	{
		return Some(bytes);
	}

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
		Key::Named(named) => named_bytes(named, modifiers, application_cursor),

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
/// do not forward (bare modifiers, keys past our map). `modifiers` carries any
/// Ctrl/Shift/Alt held with the key, encoded the xterm way for the navigation and
/// function keys (see `letter_key` / `tilde_key`); the cursor and Home/End keys also
/// depend on `application_cursor` (DECCKM) for their *unmodified* form.
fn named_bytes(named: &Named, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
	let bytes = match named {
		Named::Enter => b"\r".to_vec(),
		Named::Tab => b"\t".to_vec(),
		Named::Space => b" ".to_vec(),
		Named::Backspace => vec![0x7f],
		Named::Escape => vec![ESC],

		// Cursor keys + Home/End. Unmodified they are the CSI form, or the SS3 form while
		// DECCKM is set; a held modifier overrides that to the CSI `1;<mod>` form.
		Named::ArrowUp => letter_key(b'A', application_cursor, modifiers),
		Named::ArrowDown => letter_key(b'B', application_cursor, modifiers),
		Named::ArrowRight => letter_key(b'C', application_cursor, modifiers),
		Named::ArrowLeft => letter_key(b'D', application_cursor, modifiers),
		Named::Home => letter_key(b'H', application_cursor, modifiers),
		Named::End => letter_key(b'F', application_cursor, modifiers),

		// The "~" navigation keys. DECCKM never changes these; a modifier inserts the
		// same `;<mod>` parameter (Ctrl+Delete, Ctrl+PageUp, …).
		//
		// Note: Shift + PageUp/PageDown/Home/End never arrive here — the app layer claims
		// them for cmote's own scrollback (§23) before calling `encode`. Their Ctrl/Alt
		// variants are not claimed, so those do reach this map and are encoded.
		Named::Insert => tilde_key(2, modifiers),
		Named::Delete => tilde_key(3, modifiers),
		Named::PageUp => tilde_key(5, modifiers),
		Named::PageDown => tilde_key(6, modifiers),

		// The function keys, exactly as the terminfo entry for the pty we asked for
		// (`xterm-256color`) describes them — which is what a remote program looks them up
		// in. The split is historical, not arbitrary: F1-F4 inherited the VT100 keypad's
		// SS3 form (`kf1=\EOP`), F5 onwards are the later CSI "~" form with a gap in the
		// numbering at 16 and 22. Getting one wrong means a dead key in every full-screen
		// program — btop's options menu is F2, midnight commander lives on F1-F10. Modified,
		// F1-F4 switch to the CSI `1;<mod>` letter form and F5-F12 gain the `;<mod>`
		// parameter — the `kf13`… terminfo entries a remote program looks up.
		Named::F1 => letter_key(b'P', true, modifiers),
		Named::F2 => letter_key(b'Q', true, modifiers),
		Named::F3 => letter_key(b'R', true, modifiers),
		Named::F4 => letter_key(b'S', true, modifiers),
		Named::F5 => tilde_key(15, modifiers),
		Named::F6 => tilde_key(17, modifiers),
		Named::F7 => tilde_key(18, modifiers),
		Named::F8 => tilde_key(19, modifiers),
		Named::F9 => tilde_key(20, modifiers),
		Named::F10 => tilde_key(21, modifiers),
		Named::F11 => tilde_key(23, modifiers),
		Named::F12 => tilde_key(24, modifiers),

		// F13-F24. xterm defines these as the Shift-modified F1-F12 sequences, so they are
		// fixed forms — the base keys' `1;2` / `;2` encodings written out. A further modifier
		// on the physical F13-F24 keys (rare hardware) is not layered on, matching xterm,
		// which does not stack a second modifier here.
		Named::F13 => b"\x1b[1;2P".to_vec(),
		Named::F14 => b"\x1b[1;2Q".to_vec(),
		Named::F15 => b"\x1b[1;2R".to_vec(),
		Named::F16 => b"\x1b[1;2S".to_vec(),
		Named::F17 => b"\x1b[15;2~".to_vec(),
		Named::F18 => b"\x1b[17;2~".to_vec(),
		Named::F19 => b"\x1b[18;2~".to_vec(),
		Named::F20 => b"\x1b[19;2~".to_vec(),
		Named::F21 => b"\x1b[20;2~".to_vec(),
		Named::F22 => b"\x1b[21;2~".to_vec(),
		Named::F23 => b"\x1b[23;2~".to_vec(),
		Named::F24 => b"\x1b[24;2~".to_vec(),

		_ => return None,
	};
	Some(bytes)
}

/// The xterm modifier parameter for a key that carries one: `1` plus a bitmask of the
/// held modifiers — Shift = 1, Alt = 2, Ctrl = 4 (so Ctrl alone = 5, Ctrl+Shift = 6,
/// Ctrl+Alt = 7, all three = 8), exactly as terminfo's `kRIT5`, `kf13`, … spell it.
/// Returns `None` when no encodable modifier is down: the unmodified value is a bare
/// `1`, which xterm omits, so the caller then emits the plain sequence with no parameter.
fn modifier_param(modifiers: Modifiers) -> Option<u8> {
	let mut bits = 0u8;
	if modifiers.shift() {
		bits += 1;
	}
	if modifiers.alt() {
		bits += 2;
	}
	if modifiers.control() {
		bits += 4;
	}
	(bits != 0).then_some(1 + bits)
}

/// Assemble a CSI sequence — `ESC [ <params> <final>` — from pre-formatted parameter
/// bytes and a final byte. Every modified-key form funnels through here so the envelope
/// (`ESC`, `[`, params, final) lives in one place.
fn csi(params: &[u8], final_byte: u8) -> Vec<u8> {
	let mut out = Vec::with_capacity(params.len() + 3);
	out.push(ESC);
	out.push(b'[');
	out.extend_from_slice(params);
	out.push(final_byte);
	out
}

/// A number as its decimal ASCII digits (15 -> `b"15"`), for a CSI parameter. Wide enough to
/// hold a Unicode codepoint, since `modifyOtherKeys` puts one in the parameter (see `other_key_bytes`).
fn ascii_number(value: u32) -> Vec<u8> {
	value.to_string().into_bytes()
}

/// Encode a key whose final byte is a letter — the cursor keys `A`-`D`, Home `H`, End
/// `F`, and F1-F4 `P`-`S`. `application_ss3` picks the *unmodified* form: the SS3
/// `ESC O <final>` (always so for F1-F4, and for the cursor keys only while DECCKM is
/// set) versus the CSI `ESC [ <final>`. A held modifier overrides both — xterm then
/// always sends `ESC [ 1 ; <mod> <final>`, even in application-cursor mode — so an editor
/// reads Ctrl+Right as `ESC [ 1 ; 5 C`. Getting the unmodified prefix wrong is exactly
/// why full-screen apps ignore the arrow keys: vim binds them to the SS3 form under DECCKM.
fn letter_key(final_byte: u8, application_ss3: bool, modifiers: Modifiers) -> Vec<u8> {
	if let Some(code) = modifier_param(modifiers) {
		let mut params = b"1;".to_vec();
		params.extend_from_slice(&ascii_number(u32::from(code)));
		return csi(&params, final_byte);
	}
	let prefix = if application_ss3 { b'O' } else { b'[' };
	vec![ESC, prefix, final_byte]
}

/// Encode a "~"-terminated navigation key by its parameter number (Insert = 2, Delete =
/// 3, PageUp = 5, PageDown = 6, F5-F12 = 15/17/…/24). Unmodified that is `ESC [ <n> ~`;
/// a held modifier inserts the `;<mod>` parameter to give `ESC [ <n> ; <mod> ~`. DECCKM
/// never changes these keys, so no application-cursor flag reaches here.
fn tilde_key(number: u16, modifiers: Modifiers) -> Vec<u8> {
	let mut params = ascii_number(u32::from(number));
	if let Some(code) = modifier_param(modifiers) {
		params.push(b';');
		params.extend_from_slice(&ascii_number(u32::from(code)));
	}
	csi(&params, b'~')
}

/// The bytes for a Ctrl/Alt combo on a main-keyboard character under `modifyOtherKeys`, or
/// `None` when the mode leaves this combo alone (see `encode`). Level 2 reports every Ctrl/Alt
/// combo; level 1 reports only the gaps — a Ctrl combo whose base has no C0 control byte
/// (Ctrl+digit, Ctrl+`;`, …) — and leaves Ctrl+letter as its C0 and Alt as its ESC-meta prefix.
/// The `character` is the key's base (unshifted-role) text, so its first codepoint is the `code`
/// xterm reports, with Shift folded into the modifier parameter rather than the letter's case.
fn modify_other_key(
	character: &str,
	modifiers: Modifiers,
	level: ModifyOtherKeys,
) -> Option<Vec<u8>> {
	match level {
		ModifyOtherKeys::Off => None,
		ModifyOtherKeys::Level2 => other_key_bytes(character, modifiers),
		// Level 1 fills only the gaps: a Ctrl combo with no ordinary byte. Anything that already
		// has one (Ctrl+letter -> C0, Alt-as-meta) falls through to keep it.
		ModifyOtherKeys::Level1 => (modifiers.control() && control_byte(character).is_none())
			.then(|| other_key_bytes(character, modifiers))
			.flatten(),
	}
}

/// Assemble the `CSI 27 ; <modifier> ; <codepoint> ~` report for one character key, or `None`
/// if the key carries no character. The modifier parameter is the same `1 + bits` scheme as the
/// navigation keys (`modifier_param`); the codepoint is the key's base character. This is the
/// default (`formatOtherKeys=0`) xterm form — an editor reads it as an unambiguous key event.
fn other_key_bytes(character: &str, modifiers: Modifiers) -> Option<Vec<u8>> {
	let code = character.chars().next()? as u32;
	// The caller only reaches here with Ctrl or Alt down, so a parameter always exists; the
	// fallback keeps the arithmetic total for the impossible bare-Shift case.
	let parameter = modifier_param(modifiers).unwrap_or(1);
	let mut params = b"27;".to_vec();
	params.extend_from_slice(&ascii_number(u32::from(parameter)));
	params.push(b';');
	params.extend_from_slice(&ascii_number(code));
	Some(csi(&params, b'~'))
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::keyboard::key::{Code, Named, Physical};

	// A convenience: no modifiers held.
	fn none() -> Modifiers {
		Modifiers::empty()
	}

	// A convenience: the default (off) modifyOtherKeys level, which most tests run under.
	fn off() -> ModifyOtherKeys {
		ModifyOtherKeys::Off
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
			encode(&key, main_key(), Some("a"), none(), false, off()),
			Some(b"a".to_vec())
		);
	}

	#[test]
	fn shifted_character_uses_produced_text() {
		// The OS reports the logical key as "a" but the produced text as "A".
		let key = Key::Character("a".into());
		assert_eq!(
			encode(&key, main_key(), Some("A"), Modifiers::SHIFT, false, off()),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn ctrl_c_is_etx() {
		let key = Key::Character("c".into());
		assert_eq!(
			encode(&key, main_key(), None, Modifiers::CTRL, false, off()),
			Some(vec![0x03])
		);
	}

	#[test]
	fn enter_is_carriage_return() {
		let key = Key::Named(Named::Enter);
		assert_eq!(
			encode(&key, phys(Code::Enter), Some("\r"), none(), false, off()),
			Some(b"\r".to_vec())
		);
	}

	#[test]
	fn arrow_up_is_csi_sequence_in_normal_mode() {
		// DECCKM reset (the shell's default): arrows are the CSI form ESC[A.
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(
			encode(&key, phys(Code::ArrowUp), None, none(), false, off()),
			Some(b"\x1b[A".to_vec())
		);
	}

	#[test]
	fn arrow_up_is_ss3_sequence_in_application_mode() {
		// DECCKM set (vim/less/nano): arrows switch to the SS3 form ESC O A, which is
		// what those apps bind their arrow keys to — the fix for "arrows do nothing".
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(
			encode(&key, phys(Code::ArrowUp), None, none(), true, off()),
			Some(b"\x1bOA".to_vec())
		);
	}

	#[test]
	fn home_and_end_follow_the_cursor_mode_too() {
		// Home/End share the DECCKM behaviour: CSI when reset, SS3 when set.
		let home = Key::Named(Named::Home);
		let end = Key::Named(Named::End);
		assert_eq!(
			encode(&home, phys(Code::Home), None, none(), false, off()),
			Some(b"\x1b[H".to_vec())
		);
		assert_eq!(
			encode(&home, phys(Code::Home), None, none(), true, off()),
			Some(b"\x1bOH".to_vec())
		);
		assert_eq!(
			encode(&end, phys(Code::End), None, none(), false, off()),
			Some(b"\x1b[F".to_vec())
		);
		assert_eq!(
			encode(&end, phys(Code::End), None, none(), true, off()),
			Some(b"\x1bOF".to_vec())
		);
	}

	#[test]
	fn tilde_navigation_keys_ignore_cursor_mode() {
		// PageUp/PageDown/Insert/Delete are "~" sequences DECCKM does not touch, so
		// application mode leaves them unchanged.
		let page_up = Key::Named(Named::PageUp);
		assert_eq!(
			encode(&page_up, phys(Code::PageUp), None, none(), false, off()),
			encode(&page_up, phys(Code::PageUp), None, none(), true, off())
		);
		assert_eq!(
			encode(&page_up, phys(Code::PageUp), None, none(), true, off()),
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
				encode(&key, main_key(), None, none(), false, off()).as_deref(),
				Some(bytes)
			);
			assert_eq!(
				encode(&key, main_key(), None, none(), true, off()).as_deref(),
				Some(bytes)
			);
		}
	}

	#[test]
	fn a_modifier_on_an_arrow_uses_the_csi_param_form() {
		// Ctrl+Right is word-motion in shells and editors: the CSI form with the xterm
		// modifier parameter (Ctrl = 5), ESC[1;5C. Shift is select (2), Alt is 3.
		let right = Key::Named(Named::ArrowRight);
		assert_eq!(
			encode(&right, main_key(), None, Modifiers::CTRL, false, off()),
			Some(b"\x1b[1;5C".to_vec())
		);
		let down = Key::Named(Named::ArrowDown);
		assert_eq!(
			encode(&down, main_key(), None, Modifiers::SHIFT, false, off()),
			Some(b"\x1b[1;2B".to_vec())
		);
		let left = Key::Named(Named::ArrowLeft);
		assert_eq!(
			encode(&left, main_key(), None, Modifiers::ALT, false, off()),
			Some(b"\x1b[1;3D".to_vec())
		);
	}

	#[test]
	fn a_modified_arrow_ignores_application_cursor_mode() {
		// Even with DECCKM set, a held modifier forces the CSI `1;mod` form, never SS3 —
		// xterm behaves the same, so editors match on it in both cursor-key modes.
		let right = Key::Named(Named::ArrowRight);
		assert_eq!(
			encode(&right, main_key(), None, Modifiers::CTRL, true, off()),
			Some(b"\x1b[1;5C".to_vec())
		);
	}

	#[test]
	fn stacked_modifiers_sum_into_the_parameter() {
		// The parameter is 1 + Shift(1) + Alt(2) + Ctrl(4). Ctrl+Shift = 6, all three = 8.
		let left = Key::Named(Named::ArrowLeft);
		assert_eq!(
			encode(
				&left,
				main_key(),
				None,
				Modifiers::CTRL | Modifiers::SHIFT,
				false,
				off()
			),
			Some(b"\x1b[1;6D".to_vec())
		);
		assert_eq!(
			encode(
				&left,
				main_key(),
				None,
				Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
				false,
				off()
			),
			Some(b"\x1b[1;8D".to_vec())
		);
	}

	#[test]
	fn a_modifier_on_home_or_end_uses_the_letter_param_form() {
		// Home `H` / End `F` are letter-final like the arrows, so a modifier gives the same
		// ESC[1;mod<final> shape. (Bare Shift+End is claimed for scrollback before encode;
		// Ctrl+End is not, so it reaches here.)
		let end = Key::Named(Named::End);
		assert_eq!(
			encode(&end, phys(Code::End), None, Modifiers::CTRL, false, off()),
			Some(b"\x1b[1;5F".to_vec())
		);
	}

	#[test]
	fn a_modifier_on_a_tilde_key_inserts_the_parameter() {
		// The "~" keys keep their number and gain `;mod`: Ctrl+Delete = ESC[3;5~,
		// Ctrl+PageUp = ESC[5;5~ (Shift+PageUp is claimed for scrollback before encode).
		let delete = Key::Named(Named::Delete);
		assert_eq!(
			encode(
				&delete,
				phys(Code::Delete),
				None,
				Modifiers::CTRL,
				false,
				off()
			),
			Some(b"\x1b[3;5~".to_vec())
		);
		let page_up = Key::Named(Named::PageUp);
		assert_eq!(
			encode(
				&page_up,
				phys(Code::PageUp),
				None,
				Modifiers::CTRL,
				false,
				off()
			),
			Some(b"\x1b[5;5~".to_vec())
		);
	}

	#[test]
	fn a_modifier_on_f1_to_f4_switches_ss3_to_the_csi_param_form() {
		// Unmodified F1 is SS3 ESC O P; Shift+F1 becomes the CSI letter form ESC[1;2P — the
		// same bytes terminfo lists as `kf13`. F5 onward keep the "~" form and gain `;mod`.
		let f1 = Key::Named(Named::F1);
		assert_eq!(
			encode(&f1, main_key(), None, Modifiers::SHIFT, false, off()),
			Some(b"\x1b[1;2P".to_vec())
		);
		let f5 = Key::Named(Named::F5);
		assert_eq!(
			encode(&f5, main_key(), None, Modifiers::CTRL, false, off()),
			Some(b"\x1b[15;5~".to_vec())
		);
	}

	#[test]
	fn the_high_function_keys_map_to_their_terminfo_forms() {
		// F13-F24 = the Shift-modified F1-F12 sequences (kf13…kf24). A single wrong byte is
		// a dead key in any program that binds the high F-keys.
		let expected: [(Named, &[u8]); 12] = [
			(Named::F13, b"\x1b[1;2P"),
			(Named::F14, b"\x1b[1;2Q"),
			(Named::F15, b"\x1b[1;2R"),
			(Named::F16, b"\x1b[1;2S"),
			(Named::F17, b"\x1b[15;2~"),
			(Named::F18, b"\x1b[17;2~"),
			(Named::F19, b"\x1b[18;2~"),
			(Named::F20, b"\x1b[19;2~"),
			(Named::F21, b"\x1b[20;2~"),
			(Named::F22, b"\x1b[21;2~"),
			(Named::F23, b"\x1b[23;2~"),
			(Named::F24, b"\x1b[24;2~"),
		];
		for (named, bytes) in expected {
			let key = Key::Named(named);
			assert_eq!(
				encode(&key, main_key(), None, none(), false, off()).as_deref(),
				Some(bytes)
			);
		}
	}

	#[test]
	fn an_unmodified_named_key_is_unchanged() {
		// The modifier work must not disturb the bare keys: no modifier means no parameter,
		// so a plain Right is still ESC[C and a plain Delete still ESC[3~.
		let right = Key::Named(Named::ArrowRight);
		assert_eq!(
			encode(&right, main_key(), None, none(), false, off()),
			Some(b"\x1b[C".to_vec())
		);
		let delete = Key::Named(Named::Delete);
		assert_eq!(
			encode(&delete, phys(Code::Delete), None, none(), false, off()),
			Some(b"\x1b[3~".to_vec())
		);
	}

	#[test]
	fn modify_other_keys_level_two_wraps_a_ctrl_combo() {
		// With modifyOtherKeys level 2 the remote wants the raw key event, so Ctrl+C is the
		// unambiguous `CSI 27 ; mod ; code ~` (mod 5 = Ctrl, code 99 = 'c'), NOT the C0 0x03.
		let key = Key::Character("c".into());
		assert_eq!(
			encode(
				&key,
				main_key(),
				Some("c"),
				Modifiers::CTRL,
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"\x1b[27;5;99~".to_vec())
		);
	}

	#[test]
	fn modify_other_keys_encodes_a_ctrl_digit_that_had_no_byte() {
		// Ctrl+2 has no C0 byte, so today it is lost; under the mode it becomes a real event
		// (code 50 = '2'). This is the whole point — combos editors otherwise never see.
		let key = Key::Character("2".into());
		assert_eq!(
			encode(
				&key,
				main_key(),
				Some("2"),
				Modifiers::CTRL,
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"\x1b[27;5;50~".to_vec())
		);
	}

	#[test]
	fn modify_other_keys_folds_shift_and_alt_into_the_parameter() {
		// The parameter is the same 1 + Shift(1) + Alt(2) + Ctrl(4) scheme as the arrows, and
		// the code stays the base letter: Ctrl+Alt+a is mod 7, code 97.
		let key = Key::Character("a".into());
		assert_eq!(
			encode(
				&key,
				main_key(),
				Some("a"),
				Modifiers::CTRL | Modifiers::ALT,
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"\x1b[27;7;97~".to_vec())
		);
	}

	#[test]
	fn modify_other_keys_level_one_fills_only_the_gaps() {
		// Level 1 encodes the gap combos (Ctrl+digit, no C0) but leaves the ones that already
		// have a byte: Ctrl+C stays 0x03, so a shell keeps its interrupt.
		let digit = Key::Character("2".into());
		assert_eq!(
			encode(
				&digit,
				main_key(),
				Some("2"),
				Modifiers::CTRL,
				false,
				ModifyOtherKeys::Level1
			),
			Some(b"\x1b[27;5;50~".to_vec())
		);
		let letter = Key::Character("c".into());
		assert_eq!(
			encode(
				&letter,
				main_key(),
				Some("c"),
				Modifiers::CTRL,
				false,
				ModifyOtherKeys::Level1
			),
			Some(vec![0x03])
		);
	}

	#[test]
	fn modify_other_keys_leaves_plain_and_shift_only_typing_alone() {
		// The mode governs Ctrl/Alt combos only: ordinary typing and capitals still send their
		// text even at level 2, so an editor's normal input is untouched.
		let plain = Key::Character("a".into());
		assert_eq!(
			encode(
				&plain,
				main_key(),
				Some("a"),
				none(),
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"a".to_vec())
		);
		let shifted = Key::Character("a".into());
		assert_eq!(
			encode(
				&shifted,
				main_key(),
				Some("A"),
				Modifiers::SHIFT,
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn modify_other_keys_does_not_touch_named_keys() {
		// Navigation keys keep their own sequences whatever the level: Ctrl+Right is still the
		// arrow's CSI param form, never the 27-form (which is for the main-keyboard keys).
		let right = Key::Named(Named::ArrowRight);
		assert_eq!(
			encode(
				&right,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				ModifyOtherKeys::Level2
			),
			Some(b"\x1b[1;5C".to_vec())
		);
	}

	#[test]
	fn alt_character_gets_esc_prefix() {
		let key = Key::Character("x".into());
		assert_eq!(
			encode(&key, main_key(), Some("x"), Modifiers::ALT, false, off()),
			Some(b"\x1bx".to_vec())
		);
	}

	#[test]
	fn bare_modifier_key_sends_nothing() {
		let key = Key::Named(Named::Shift);
		assert_eq!(
			encode(&key, phys(Code::ShiftLeft), None, none(), false, off()),
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
			encode(&key, phys(Code::Numpad2), Some("2"), none(), false, off()),
			Some(b"2".to_vec())
		);
	}

	#[test]
	fn numpad_digit_without_numlock_still_navigates() {
		// NumLock off: the OS produces no text and the logical key is the navigation
		// role, so numpad 2 keeps acting as Down (CSI ESC[B) — unchanged behaviour.
		let key = Key::Named(Named::ArrowDown);
		assert_eq!(
			encode(&key, phys(Code::Numpad2), None, none(), false, off()),
			Some(b"\x1b[B".to_vec())
		);
	}

	#[test]
	fn numpad_decimal_with_numlock_sends_its_character() {
		// The decimal key has the same dual identity (Delete vs "."). With NumLock on
		// the OS produces the locale's separator as text; send it verbatim.
		let key = Key::Named(Named::Delete);
		assert_eq!(
			encode(
				&key,
				phys(Code::NumpadDecimal),
				Some("."),
				none(),
				false,
				off()
			),
			Some(b".".to_vec())
		);
	}

	#[test]
	fn numpad_enter_is_left_to_the_normal_path() {
		// NumpadEnter is not a "number" key (it never navigates), so the numpad
		// shortcut ignores it and the logical Enter mapping applies: carriage return.
		let key = Key::Named(Named::Enter);
		assert_eq!(
			encode(
				&key,
				phys(Code::NumpadEnter),
				Some("\r"),
				none(),
				false,
				off()
			),
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

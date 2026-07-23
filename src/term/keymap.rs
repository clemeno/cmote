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

/// Turn one key press into the bytes to send, or `None` if the key produces no
/// input (a bare modifier, an unmapped named key). `text` is the OS-produced
/// string for the key (already honoring layout and Shift); we prefer it for
/// printable input and fall back to the logical key only when it is absent.
pub fn encode(key: &Key, text: Option<&str>, modifiers: Modifiers) -> Option<Vec<u8>> {
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
		Key::Named(named) => named_bytes(named),

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
/// do not forward (bare modifiers, function keys we have not mapped yet).
fn named_bytes(named: &Named) -> Option<Vec<u8>> {
	let sequence: &[u8] = match named {
		Named::Enter => b"\r",
		Named::Tab => b"\t",
		Named::Space => b" ",
		Named::Backspace => &[0x7f],
		Named::Escape => &[ESC],
		// CSI cursor and navigation sequences, as an xterm would send them.
		Named::ArrowUp => b"\x1b[A",
		Named::ArrowDown => b"\x1b[B",
		Named::ArrowRight => b"\x1b[C",
		Named::ArrowLeft => b"\x1b[D",
		Named::Home => b"\x1b[H",
		Named::End => b"\x1b[F",
		Named::Insert => b"\x1b[2~",
		Named::Delete => b"\x1b[3~",
		Named::PageUp => b"\x1b[5~",
		Named::PageDown => b"\x1b[6~",
		_ => return None,
	};
	Some(sequence.to_vec())
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
		assert_eq!(encode(&key, Some("a"), none()), Some(b"a".to_vec()));
	}

	#[test]
	fn shifted_character_uses_produced_text() {
		// The OS reports the logical key as "a" but the produced text as "A".
		let key = Key::Character("a".into());
		assert_eq!(
			encode(&key, Some("A"), Modifiers::SHIFT),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn ctrl_c_is_etx() {
		let key = Key::Character("c".into());
		assert_eq!(encode(&key, None, Modifiers::CTRL), Some(vec![0x03]));
	}

	#[test]
	fn enter_is_carriage_return() {
		let key = Key::Named(Named::Enter);
		assert_eq!(encode(&key, Some("\r"), none()), Some(b"\r".to_vec()));
	}

	#[test]
	fn arrow_up_is_csi_sequence() {
		let key = Key::Named(Named::ArrowUp);
		assert_eq!(encode(&key, None, none()), Some(b"\x1b[A".to_vec()));
	}

	#[test]
	fn alt_character_gets_esc_prefix() {
		let key = Key::Character("x".into());
		assert_eq!(
			encode(&key, Some("x"), Modifiers::ALT),
			Some(b"\x1bx".to_vec())
		);
	}

	#[test]
	fn bare_modifier_key_sends_nothing() {
		let key = Key::Named(Named::Shift);
		assert_eq!(encode(&key, None, none()), None);
	}
}

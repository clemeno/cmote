// term/kitty.rs — encode a key event in the kitty keyboard protocol (PLAN §25).
//
// The classic terminal input alphabet loses information: Ctrl+I and Tab are both 0x09, Esc and
// the start of an escape sequence are both 0x1b, and a bare key-up is invisible. modifyOtherKeys
// (§9) patches the worst of it; kitty's protocol replaces the lot. A program turns it on by
// PUSHING a set of progressive-enhancement flags — the engine tracks the push/pop/query stack for
// us (see `Terminal::new`), and cmote reads the active flags off the seam (`Screen::kitty_flags`)
// and encodes each key press accordingly here.
//
// The flags, from least to most intrusive:
//   * disambiguate (0b1)      — Esc becomes `CSI 27 u`, Ctrl+key an unambiguous `CSI code;mod u`
//   * report events (0b10)    — press / repeat / release are told apart (the key-up cmote can now
//                               see, because iced delivers a KeyReleased and `app` forwards it)
//   * report alternates (0b100) — the keycode gains the shifted glyph as a sub-field
//   * report all keys (0b1000)  — even a plain letter becomes an escape code, not text
//   * report text (0b10000)     — the produced text rides along as trailing code points
//
// The wire form is `CSI <keycode>[:<shifted>] ; <modifiers>[:<event>] ; <text> u`, with every
// trailing field dropped when it is at its default. Keys that already had a legacy escape code —
// the arrows, Home/End, the function and navigation keys — keep their historic final byte (A, ~,
// …) and only gain the modifier/event parameters, so an editor that also understands the old
// forms is never surprised. See the kitty spec: https://sw.kovidgoyal.net/kitty/keyboard-protocol/
//
// cmote leans on one simplification, true of every real client: it treats the protocol as "on"
// whenever any flag is pushed and always applies the disambiguating encoding. A program that
// pushed, say, report-events WITHOUT disambiguate (which none do) would get disambiguated keys
// too — harmless, and it keeps the branching honest.

use iced::keyboard::key::{Named, Physical};
use iced::keyboard::{Key, Modifiers};

/// ASCII escape (`ESC`), the lead byte of every sequence this module emits.
const ESC: u8 = 0x1b;

/// The five progressive-enhancement flags a program can push, as cmote's own copy of the engine's
/// mode bits (`Screen::kitty_flags` fills it in). A plain `Default` — every flag off — is the
/// legacy state, in which `keymap` never routes here at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KittyFlags {
	/// Report otherwise-ambiguous keys (Esc, Ctrl+key, the keypad) as `CSI u` codes.
	pub disambiguate: bool,
	/// Tell press, repeat and release apart with an `:event` sub-parameter.
	pub report_events: bool,
	/// Add the shifted glyph to the keycode field as a `:shifted` sub-parameter.
	pub report_alternates: bool,
	/// Encode every key as an escape code, including plain text keys.
	pub report_all: bool,
	/// Append the text a key produced as trailing code points.
	pub report_text: bool,
}

impl KittyFlags {
	/// Whether any flag is set — i.e. whether a program has turned the protocol on at all. `keymap`
	/// checks this to decide between this encoder and the legacy path.
	pub fn is_active(self) -> bool {
		self.disambiguate
			|| self.report_events
			|| self.report_alternates
			|| self.report_all
			|| self.report_text
	}
}

/// Which transition a key event is. `app` maps iced's `KeyPressed { repeat }` onto `Press` /
/// `Repeat` and its `KeyReleased` onto `Release`; the encoder reports the distinction only when a
/// program asked for it (`report_events`), and otherwise collapses a repeat to a press and drops a
/// release entirely — exactly as a legacy terminal behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
	Press,
	Repeat,
	Release,
}

/// How a key maps onto the protocol's families. The wire encoding differs by family — a letter-
/// final legacy key keeps its final byte and a fixed keycode of 1, a tilde key keeps its number,
/// and everything else is a `u`-terminated code point — so the key is classified once here and the
/// byte assembly below reads the family back.
enum Form {
	/// A legacy letter-final key (arrows, Home, End, F1-F4). The keycode is a fixed `1`; `ss3` says
	/// the *bare* (unmodified, press) form is SS3 `ESC O <final>` rather than CSI — true for the
	/// cursor keys under DECCKM and always for F1-F4, matching the legacy encoder.
	Letter { final_byte: u8, ss3: bool },
	/// A legacy tilde-final key (Insert, Delete, PageUp/Down, F5-F12). The keycode is its number.
	Tilde(u16),
	/// A key with no text of its own that is always an escape code — here only Escape (27).
	Special(u32),
	/// Enter / Tab / Backspace: the `legacy` byte when unmodified, else a `codepoint` `CSI u` code.
	Compat { codepoint: u32, legacy: u8 },
	/// A text-producing key — its `codepoint` is the base (unshifted) key. Sent as text unless a
	/// non-text modifier is held or `report_all` is on.
	Text { codepoint: u32 },
	/// A key forwarded as fixed bytes with no protocol parameters (F13-F24, which have no legacy
	/// number and are rare enough not to earn kitty's private-use code points here).
	Fixed(&'static [u8]),
}

/// Encode one key event, or `None` when it produces no bytes: a bare modifier, an unmapped key, a
/// release of a key that is not itself an escape code, or a release/repeat the program did not ask
/// to hear about. `flags` is the active protocol state; `application_cursor` is DECCKM, which still
/// picks SS3 for a bare cursor key. `text` is the OS-produced glyph (absent on a release).
pub fn encode(
	key: &Key,
	physical: Physical,
	text: Option<&str>,
	modifiers: Modifiers,
	application_cursor: bool,
	flags: KittyFlags,
	event: KeyEvent,
) -> Option<Vec<u8>> {
	let _ = physical; // classification is by logical key here; the numpad is handled in `keymap`
	let Some(form) = classify(key, application_cursor) else {
		// An unidentified key still carries its text on a press, so typing through one is not lost;
		// everything unmapped (a bare modifier, a media key) produces nothing.
		return match (key, event) {
			(Key::Unidentified, KeyEvent::Press | KeyEvent::Repeat) => {
				text.map(|typed| typed.as_bytes().to_vec())
			}
			_ => None,
		};
	};

	// The modifier bitmask (the wire field is this plus one). Shift is included for the field but
	// NOT for the "is this an escape code?" test below — Shift+a is the letter A, still text.
	let modmask = modifier_mask(modifiers);
	let has_nontext_modifier = modifiers.control() || modifiers.alt() || modifiers.logo();

	let is_escape_code = match form {
		// Legacy functional keys are always escape codes, exactly as they always were.
		Form::Letter { .. } | Form::Tilde(_) | Form::Special(_) | Form::Fixed(_) => true,
		// Enter/Tab/Backspace escape only when a modifier is held (Shift+Enter included) or the
		// program asked for every key; bare, they stay `\r` / `\t` / 0x7f for shell compatibility.
		Form::Compat { .. } => modmask != 0 || flags.report_all,
		// A text key escapes on a non-text modifier (Ctrl/Alt/Super) or under report-all; a bare or
		// shift-only press is just its text.
		Form::Text { .. } => has_nontext_modifier || flags.report_all,
	};

	if !is_escape_code {
		// Plain text: send the produced glyph on a press or repeat, nothing on a release (a text
		// key has no escape code to hang a release on — that is what report-all is for).
		return match event {
			KeyEvent::Release => None,
			_ => text_bytes(&form, text),
		};
	}

	// A release or repeat is only reported when the program turned on event types; otherwise a
	// release yields nothing and a repeat is indistinguishable from a fresh press.
	let event = match event {
		KeyEvent::Release if !flags.report_events => return None,
		KeyEvent::Repeat if !flags.report_events => KeyEvent::Press,
		other => other,
	};

	// A fixed key (F13-F24) carries no parameters, so it ignores modifiers and event types.
	if let Form::Fixed(bytes) = form {
		return Some(bytes.to_vec());
	}

	let shifted = shifted_alternate(&form, text, modifiers, flags);
	let assoc = associated_text(&form, text, event, flags);
	Some(assemble(&form, modmask, event, shifted, assoc))
}

/// Classify a key into its protocol family, or `None` for a key this module does not forward.
fn classify(key: &Key, application_cursor: bool) -> Option<Form> {
	let form = match key {
		Key::Character(character) => Form::Text {
			codepoint: character.chars().next()? as u32,
		},
		Key::Named(named) => named_form(*named, application_cursor)?,
		Key::Unidentified => return None,
	};
	Some(form)
}

/// The family of a named (non-character) key, or `None` for one we do not forward (a bare modifier,
/// a key past our map). Mirrors the legacy `keymap::named_bytes` map, so a program sees the same
/// keys — only dressed in the protocol's parameters.
fn named_form(named: Named, application_cursor: bool) -> Option<Form> {
	let form = match named {
		Named::Enter => Form::Compat {
			codepoint: 13,
			legacy: b'\r',
		},
		Named::Tab => Form::Compat {
			codepoint: 9,
			legacy: b'\t',
		},
		Named::Backspace => Form::Compat {
			codepoint: 127,
			legacy: 0x7f,
		},
		Named::Escape => Form::Special(27),
		// Space produces a glyph, so it rides the text path (a bare space is still just " ").
		Named::Space => Form::Text { codepoint: 32 },

		// Cursor keys and Home/End: letter-final, SS3 in application-cursor mode.
		Named::ArrowUp => letter(b'A', application_cursor),
		Named::ArrowDown => letter(b'B', application_cursor),
		Named::ArrowRight => letter(b'C', application_cursor),
		Named::ArrowLeft => letter(b'D', application_cursor),
		Named::Home => letter(b'H', application_cursor),
		Named::End => letter(b'F', application_cursor),

		// F1-F4 are letter-final and always SS3 when bare (their VT100 keypad heritage).
		Named::F1 => letter(b'P', true),
		Named::F2 => letter(b'Q', true),
		Named::F3 => letter(b'R', true),
		Named::F4 => letter(b'S', true),

		// The tilde-final navigation and function keys, by their historic numbers.
		Named::Insert => Form::Tilde(2),
		Named::Delete => Form::Tilde(3),
		Named::PageUp => Form::Tilde(5),
		Named::PageDown => Form::Tilde(6),
		Named::F5 => Form::Tilde(15),
		Named::F6 => Form::Tilde(17),
		Named::F7 => Form::Tilde(18),
		Named::F8 => Form::Tilde(19),
		Named::F9 => Form::Tilde(20),
		Named::F10 => Form::Tilde(21),
		Named::F11 => Form::Tilde(23),
		Named::F12 => Form::Tilde(24),

		// F13-F24 keep the fixed legacy sequences the legacy encoder uses (the Shift+F1-F12 forms).
		Named::F13 => Form::Fixed(b"\x1b[1;2P"),
		Named::F14 => Form::Fixed(b"\x1b[1;2Q"),
		Named::F15 => Form::Fixed(b"\x1b[1;2R"),
		Named::F16 => Form::Fixed(b"\x1b[1;2S"),
		Named::F17 => Form::Fixed(b"\x1b[15;2~"),
		Named::F18 => Form::Fixed(b"\x1b[17;2~"),
		Named::F19 => Form::Fixed(b"\x1b[18;2~"),
		Named::F20 => Form::Fixed(b"\x1b[19;2~"),
		Named::F21 => Form::Fixed(b"\x1b[20;2~"),
		Named::F22 => Form::Fixed(b"\x1b[21;2~"),
		Named::F23 => Form::Fixed(b"\x1b[23;2~"),
		Named::F24 => Form::Fixed(b"\x1b[24;2~"),

		_ => return None,
	};
	Some(form)
}

/// A letter-final `Form`, shorthand for the several cursor/function keys that share the shape.
fn letter(final_byte: u8, ss3: bool) -> Form {
	Form::Letter { final_byte, ss3 }
}

/// The modifier bitmask kitty uses: shift 1, alt 2, ctrl 4, super 8 (iced's Logo). The wire field
/// is this value plus one; hyper / meta / lock bits are not surfaced by iced, so they never appear.
fn modifier_mask(modifiers: Modifiers) -> u8 {
	let mut bits = 0u8;
	if modifiers.shift() {
		bits |= 0b1;
	}
	if modifiers.alt() {
		bits |= 0b10;
	}
	if modifiers.control() {
		bits |= 0b100;
	}
	if modifiers.logo() {
		bits |= 0b1000;
	}
	bits
}

/// The event type's wire digit: press 1, repeat 2, release 3.
fn event_digit(event: KeyEvent) -> u8 {
	match event {
		KeyEvent::Press => 1,
		KeyEvent::Repeat => 2,
		KeyEvent::Release => 3,
	}
}

/// The bytes for a key that is NOT an escape code (a bare or shift-only text key, or an unmodified
/// Enter/Tab/Backspace): its produced glyph, or the fixed legacy byte for the compat keys.
fn text_bytes(form: &Form, text: Option<&str>) -> Option<Vec<u8>> {
	match form {
		Form::Compat { legacy, .. } => Some(vec![*legacy]),
		Form::Text { codepoint } => {
			// Prefer the OS-produced glyph (it already honours Shift and the layout); fall back to
			// the base code point if the OS gave none.
			let produced = text.filter(|typed| !typed.is_empty());
			match produced {
				Some(typed) => Some(typed.as_bytes().to_vec()),
				None => char::from_u32(*codepoint).map(|glyph| glyph.to_string().into_bytes()),
			}
		}
		// The other families are always escape codes, so they never reach here.
		_ => None,
	}
}

/// The shifted-glyph sub-field for the keycode, when the program asked for alternate keys and Shift
/// is held on a text key. `None` otherwise — including when the shifted glyph equals the base (a key
/// with no distinct shifted form), where the sub-field would say nothing.
fn shifted_alternate(
	form: &Form,
	text: Option<&str>,
	modifiers: Modifiers,
	flags: KittyFlags,
) -> Option<u32> {
	if !flags.report_alternates || !modifiers.shift() {
		return None;
	}
	let Form::Text { codepoint } = form else {
		return None;
	};
	let shifted = text?.chars().next()? as u32;
	(shifted != *codepoint).then_some(shifted)
}

/// The trailing associated-text code points, when the program asked for them and the key produced
/// text on a press/repeat. Only text keys carry it — a functional key has no glyph to report.
fn associated_text(
	form: &Form,
	text: Option<&str>,
	event: KeyEvent,
	flags: KittyFlags,
) -> Option<Vec<u32>> {
	if !flags.report_text || event == KeyEvent::Release {
		return None;
	}
	let Form::Text { .. } = form else {
		return None;
	};
	let typed = text?;
	if typed.is_empty() {
		return None;
	}
	Some(typed.chars().map(|glyph| glyph as u32).collect())
}

/// Assemble the escape sequence for a key that IS an escape code. Picks the family's keycode and
/// final byte, emits the bare legacy form when no parameter is needed, and otherwise threads the
/// keycode (with any shifted alternate), the modifier/event field, and the associated text into the
/// `CSI … <final>` envelope.
fn assemble(
	form: &Form,
	modmask: u8,
	event: KeyEvent,
	shifted: Option<u32>,
	assoc: Option<Vec<u32>>,
) -> Vec<u8> {
	let needs_params =
		modmask != 0 || event != KeyEvent::Press || shifted.is_some() || assoc.is_some();

	// The letter- and tilde-final keys have a bare form (no keycode / no parameters) that must be
	// kept when nothing modifies them, so a plain arrow is still `ESC [ A` and vim reads it.
	match form {
		Form::Letter { final_byte, ss3 } if !needs_params => {
			let prefix = if *ss3 { b'O' } else { b'[' };
			return vec![ESC, prefix, *final_byte];
		}
		Form::Tilde(number) if !needs_params => {
			let mut out = vec![ESC, b'['];
			push_decimal(&mut out, u32::from(*number));
			out.push(b'~');
			return out;
		}
		_ => {}
	}

	let (keycode, final_byte) = match form {
		Form::Letter { final_byte, .. } => (1, *final_byte),
		Form::Tilde(number) => (u32::from(*number), b'~'),
		// Every codepoint-carrying form reports the codepoint with a `u` final byte; what differs
		// between them is how the codepoint was FOUND, which is settled by the time this runs.
		Form::Special(codepoint) | Form::Text { codepoint } | Form::Compat { codepoint, .. } => {
			(*codepoint, b'u')
		}
		// A fixed key never reaches assembly — `encode` returns its bytes directly.
		Form::Fixed(_) => (0, b'u'),
	};

	let mut out = vec![ESC, b'['];
	push_decimal(&mut out, keycode);
	if let Some(shifted) = shifted {
		out.push(b':');
		push_decimal(&mut out, shifted);
	}
	// The modifier field must be present whenever an event or text field follows it, written as a
	// bare `1` (no modifiers) so the later fields keep their place.
	if modmask != 0 || event != KeyEvent::Press || assoc.is_some() {
		out.push(b';');
		push_decimal(&mut out, u32::from(modmask) + 1);
		if event != KeyEvent::Press {
			out.push(b':');
			out.push(b'0' + event_digit(event));
		}
	}
	if let Some(codepoints) = assoc {
		out.push(b';');
		for (index, codepoint) in codepoints.iter().enumerate() {
			if index > 0 {
				out.push(b':');
			}
			push_decimal(&mut out, *codepoint);
		}
	}
	out.push(final_byte);
	out
}

/// Append a number as its decimal ASCII digits — the one place a parameter is turned into bytes.
fn push_decimal(out: &mut Vec<u8>, value: u32) {
	out.extend_from_slice(value.to_string().as_bytes());
}

#[cfg(test)]
mod tests {
	use super::*;
	use iced::keyboard::key::{Code, Named, Physical};

	// A non-numpad physical key: the numpad is filtered in `keymap`, so this module never cares.
	fn main_key() -> Physical {
		Physical::Code(Code::KeyA)
	}

	// The disambiguate-only flag set, the base every real program pushes.
	fn disambiguate() -> KittyFlags {
		KittyFlags {
			disambiguate: true,
			..KittyFlags::default()
		}
	}

	// Encode a character-key press under the given flags, with the given produced text/modifiers.
	fn press_char(
		character: &str,
		text: Option<&str>,
		modifiers: Modifiers,
		flags: KittyFlags,
	) -> Option<Vec<u8>> {
		let key = Key::Character(character.into());
		encode(
			&key,
			main_key(),
			text,
			modifiers,
			false,
			flags,
			KeyEvent::Press,
		)
	}

	// Encode a named-key press under the given flags.
	fn press_named(named: Named, modifiers: Modifiers, flags: KittyFlags) -> Option<Vec<u8>> {
		let key = Key::Named(named);
		encode(
			&key,
			main_key(),
			None,
			modifiers,
			false,
			flags,
			KeyEvent::Press,
		)
	}

	#[test]
	fn inactive_flags_are_not_active() {
		// The default (every flag off) is the legacy state, and `keymap` never routes here for it.
		assert!(!KittyFlags::default().is_active());
		assert!(disambiguate().is_active());
	}

	#[test]
	fn escape_disambiguates_to_a_csi_u_code() {
		// The headline of the disambiguate flag: a bare Esc is `CSI 27 u`, so an editor can tell it
		// from the ESC that begins an Alt combo or an arrow-key sequence.
		assert_eq!(
			press_named(Named::Escape, Modifiers::empty(), disambiguate()),
			Some(b"\x1b[27u".to_vec())
		);
	}

	#[test]
	fn a_plain_letter_is_still_text() {
		// Disambiguate does not touch ordinary typing: `a` is `a`, and Shift+a is the letter A —
		// the OS-produced glyph, not an escape code.
		assert_eq!(
			press_char("a", Some("a"), Modifiers::empty(), disambiguate()),
			Some(b"a".to_vec())
		);
		assert_eq!(
			press_char("a", Some("A"), Modifiers::SHIFT, disambiguate()),
			Some(b"A".to_vec())
		);
	}

	#[test]
	fn ctrl_and_alt_letters_become_unambiguous_codes() {
		// Ctrl+a is `CSI 97;5u` (mod 5 = ctrl), NOT the 0x01 the legacy path sends — so Ctrl+I and
		// Tab are finally distinct. Alt+a is `CSI 97;3u` (mod 3 = alt), not an ESC-prefixed byte.
		assert_eq!(
			press_char("a", None, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[97;5u".to_vec())
		);
		assert_eq!(
			press_char("a", Some("a"), Modifiers::ALT, disambiguate()),
			Some(b"\x1b[97;3u".to_vec())
		);
	}

	#[test]
	fn enter_tab_backspace_stay_legacy_until_modified() {
		// Bare, the three keep their C0 bytes for shell compatibility; with a modifier they switch
		// to the `CSI code;mod u` form (Ctrl+Enter = `CSI 13;5u`).
		assert_eq!(
			press_named(Named::Enter, Modifiers::empty(), disambiguate()),
			Some(b"\r".to_vec())
		);
		assert_eq!(
			press_named(Named::Tab, Modifiers::empty(), disambiguate()),
			Some(b"\t".to_vec())
		);
		assert_eq!(
			press_named(Named::Backspace, Modifiers::empty(), disambiguate()),
			Some(vec![0x7f])
		);
		assert_eq!(
			press_named(Named::Enter, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[13;5u".to_vec())
		);
	}

	#[test]
	fn functional_keys_keep_their_legacy_final_byte() {
		// The arrows, Home/End and the F-keys keep their historic finals and gain only the modifier
		// parameter, so a program that knows the old sequences still reads them. Plain Left is
		// `ESC [ D`; Ctrl+Left is `ESC [ 1;5 D`. F1 is SS3 `ESC O P`; Ctrl+F1 is `ESC [ 1;5 P`. F5
		// is the tilde form `ESC [ 15 ~`; Ctrl+F5 is `ESC [ 15;5 ~`.
		assert_eq!(
			press_named(Named::ArrowLeft, Modifiers::empty(), disambiguate()),
			Some(b"\x1b[D".to_vec())
		);
		assert_eq!(
			press_named(Named::ArrowLeft, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[1;5D".to_vec())
		);
		assert_eq!(
			press_named(Named::F1, Modifiers::empty(), disambiguate()),
			Some(b"\x1bOP".to_vec())
		);
		assert_eq!(
			press_named(Named::F1, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[1;5P".to_vec())
		);
		assert_eq!(
			press_named(Named::F5, Modifiers::empty(), disambiguate()),
			Some(b"\x1b[15~".to_vec())
		);
		assert_eq!(
			press_named(Named::F5, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[15;5~".to_vec())
		);
		assert_eq!(
			press_named(Named::Delete, Modifiers::CTRL, disambiguate()),
			Some(b"\x1b[3;5~".to_vec())
		);
	}

	#[test]
	fn a_bare_cursor_key_still_follows_application_mode() {
		// DECCKM is respected for the unmodified press so vim's arrows keep working: `ESC O D`.
		let key = Key::Named(Named::ArrowLeft);
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::empty(),
				true,
				disambiguate(),
				KeyEvent::Press
			),
			Some(b"\x1bOD".to_vec())
		);
	}

	#[test]
	fn a_bare_modifier_produces_nothing() {
		assert_eq!(
			press_named(Named::Shift, Modifiers::SHIFT, disambiguate()),
			None
		);
	}

	// The event-types flag set (disambiguate is always the base in practice).
	fn events() -> KittyFlags {
		KittyFlags {
			disambiguate: true,
			report_events: true,
			..KittyFlags::default()
		}
	}

	#[test]
	fn a_release_is_reported_only_with_event_types() {
		let key = Key::Character("a".into());
		// With event types on, Ctrl+a release is `CSI 97;5:3u`; its press is the plain `CSI 97;5u`.
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				events(),
				KeyEvent::Release
			),
			Some(b"\x1b[97;5:3u".to_vec())
		);
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				events(),
				KeyEvent::Press
			),
			Some(b"\x1b[97;5u".to_vec())
		);
		// Without event types, a release yields nothing at all.
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				disambiguate(),
				KeyEvent::Release
			),
			None
		);
	}

	#[test]
	fn a_repeat_needs_event_types_to_differ_from_a_press() {
		let key = Key::Character("a".into());
		// With event types, a Ctrl+a repeat is `CSI 97;5:2u`.
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				events(),
				KeyEvent::Repeat
			),
			Some(b"\x1b[97;5:2u".to_vec())
		);
		// Without them, a repeat is just another press.
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::CTRL,
				false,
				disambiguate(),
				KeyEvent::Repeat
			),
			Some(b"\x1b[97;5u".to_vec())
		);
	}

	#[test]
	fn a_text_key_has_no_release_without_report_all() {
		// A plain letter is sent as text, so there is no escape code to hang a release on — even
		// with event types, its key-up is silent until report-all promotes it to a code.
		let key = Key::Character("a".into());
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::empty(),
				false,
				events(),
				KeyEvent::Release
			),
			None
		);
	}

	#[test]
	fn report_all_makes_even_plain_letters_escape_codes() {
		// With report-all, a plain `a` press is `CSI 97u` and — with event types — its release is
		// `CSI 97;1:3u` (mod 1 = none, event 3 = release).
		let flags = KittyFlags {
			disambiguate: true,
			report_events: true,
			report_all: true,
			..KittyFlags::default()
		};
		let key = Key::Character("a".into());
		assert_eq!(
			encode(
				&key,
				main_key(),
				Some("a"),
				Modifiers::empty(),
				false,
				flags,
				KeyEvent::Press
			),
			Some(b"\x1b[97u".to_vec())
		);
		assert_eq!(
			encode(
				&key,
				main_key(),
				None,
				Modifiers::empty(),
				false,
				flags,
				KeyEvent::Release
			),
			Some(b"\x1b[97;1:3u".to_vec())
		);
	}

	#[test]
	fn associated_text_rides_along_when_asked() {
		// report-all + report-text: a plain `a` is `CSI 97;1;97u` — keycode 97, the mandatory `1`
		// modifier placeholder, then the produced code point 97.
		let flags = KittyFlags {
			disambiguate: true,
			report_all: true,
			report_text: true,
			..KittyFlags::default()
		};
		assert_eq!(
			press_char("a", Some("a"), Modifiers::empty(), flags),
			Some(b"\x1b[97;1;97u".to_vec())
		);
	}

	#[test]
	fn alternate_keys_add_the_shifted_glyph() {
		// report-all + report-alternates: Shift+a is `CSI 97:65;2u` — base keycode 97, shifted glyph
		// 65 (A), modifier 2 (shift).
		let flags = KittyFlags {
			disambiguate: true,
			report_all: true,
			report_alternates: true,
			..KittyFlags::default()
		};
		assert_eq!(
			press_char("a", Some("A"), Modifiers::SHIFT, flags),
			Some(b"\x1b[97:65;2u".to_vec())
		);
	}
}

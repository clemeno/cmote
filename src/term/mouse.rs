// term/mouse.rs — encode pointer events as the reports a terminal sends (PLAN §9).
//
// A full-screen program that wants the mouse asks for it: `ESC [ ? 1000 h` and friends
// turn on one of the xterm mouse protocols, and from then on the terminal answers every
// click, release and (in the motion modes) every move between cells with a short report
// on the input channel. The emulator tracks which mode and which encoding the program
// asked for, surfaced engine-agnostically as `screen::MouseMode` / `screen::MouseEncoding`
// (§16); this module turns one pointer event plus that state into the bytes to send.
//
// Two encodings are in use in the wild:
//   Default   ESC [ M  <32+button> <32+col> <32+row>   — one byte per field, so it
//                                                        cannot name a cell past 223
//   SGR       ESC [ < button ; col ; row M             — press (`M`) / release (`m`),
//                                                        no coordinate ceiling
// Everything modern (btop, htop, vim, tmux) asks for SGR; the classic form is kept for
// programs that never learned about it.
//
// §150 added the two ends of that range. X10 (`CSI ? 9 h`) is the ORIGINAL protocol, below the
// classic form; SGR-Pixels (`CSI ? 1016 h`) is SGR with pixels in place of cells, above it. Both are
// cmote's own modes, held in `term/decmodes.rs`, because the engine's `NamedPrivateMode` has neither.
//
// X10 IS THE CLASSIC ENCODING MINUS TWO THINGS, and both come from the same primary source —
// XFree86's copy of xterm's ctlseqs, which is the one that is not truncated at the Mouse Tracking
// section (§87 recorded the truncation; §150 found the way round it):
//
//   "X10 compatibility mode sends an escape sequence only on button press, encoding the location
//    and the mouse button pressed. It is enabled by specifying parameter 9 to DECSET."
//
//   "On button press, xterm sends CSI M Cb Cx Cy (6 characters). Cb is button-1. Cx and Cy are the
//    x and y coordinates of the mouse when the button was pressed."
//
// So: **press only** — no release, no motion — and **no modifier bits**, which the same document says
// by omission. Its paragraph on the X11 protocol one page down begins "Modifier key (shift, ctrl,
// meta) information is ALSO sent", and lists the three bits as that protocol's addition.
//
// SGR-PIXELS IS AN INFERENCE, AND IT IS LABELLED ONE. Every source reachable for §150 says exactly
// this much: "Ps = 1 0 1 6 -> Enable SGR Mouse PixelMode, xterm." The coordinate convention — origin,
// units, whether it counts from 1 — is stated nowhere that could be read. cmote reports **1-based
// pixels from the top-left of the text area**, because that is the only reading under which the mode
// is what its name says: SGR mouse mode with pixels where the cells were. SGR reports 1-based cells
// from the same corner, so the two differ in their unit and in nothing else.
//
// The caller decides *whether* to consult us: a Shift-held click is the user's own text
// selection and never becomes a report (the xterm convention), so the grid widget checks
// that before calling in.

use iced::keyboard::Modifiers;

use crate::term::screen::{MouseEncoding, MouseMode};

/// ASCII escape, the lead byte of every report.
const ESC: u8 = 0x1b;

/// The largest coordinate the single-byte `Default` encoding can carry (255 - 32).
const CLASSIC_MAX: u16 = 223;

/// Which button an event is about. The wheel is a button in this protocol — a scroll is
/// reported as a press of button 64/65 and never released.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
	Left,
	Middle,
	Right,
	WheelUp,
	WheelDown,
}

impl Button {
	/// The button's number in the report's button field.
	fn code(self) -> u8 {
		match self {
			Button::Left => 0,
			Button::Middle => 1,
			Button::Right => 2,
			Button::WheelUp => 64,
			Button::WheelDown => 65,
		}
	}
}

/// What the pointer did. `Motion` carries whichever button is held, because the two
/// motion modes differ on exactly that: `ButtonMotion` reports a drag only, `AnyMotion`
/// reports a bare hover too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
	Press(Button),
	Release(Button),
	Motion(Option<Button>),
}

/// Where the pointer was, in both vocabularies at once (§150).
///
/// One struct rather than four arguments, and not only to keep the count down: `row`/`col` and `x`/`y`
/// describe the SAME point, and passing them separately is how a caller ends up handing over a cell
/// from one frame and a pixel from another. They are built together at the one place that has the
/// pointer position (`ui::grid`).
///
/// `row`/`col` are zero-based grid cells and `x`/`y` are pixels from the top-left of the TEXT AREA —
/// inside the grid's own padding, which is the corner cell (0, 0) sits in. Both are shifted to the
/// protocol's one-based counting here, in the single place that knows about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
	pub row: u16,
	pub col: u16,
	pub x: u16,
	pub y: u16,
}

/// The report for one pointer event, or `None` when the program has not asked for that
/// kind of event (or for the mouse at all).
pub fn encode(
	mode: MouseMode,
	encoding: MouseEncoding,
	event: MouseEvent,
	at: Position,
	modifiers: Modifiers,
) -> Option<Vec<u8>> {
	if !wants(mode, event) {
		return None;
	}

	// The button field. A release in either SGR encoding says the real button and marks the release
	// with its final byte; the classic form can only say "some button came up" (3). A move adds 32, and
	// "no button held" is that same 3.
	let released = matches!(event, MouseEvent::Release(_));
	let sgr = matches!(encoding, MouseEncoding::Sgr | MouseEncoding::SgrPixels);
	let mut field = match event {
		MouseEvent::Press(button) => button.code(),
		MouseEvent::Release(button) if sgr => button.code(),
		MouseEvent::Release(_) => 3,
		MouseEvent::Motion(held) => held.map_or(3, Button::code) + 32,
	};
	// The modifier bits — every protocol but X10, which predates them (§150). The document that says
	// so says it by omission: its X11 paragraph opens "Modifier key (shift, ctrl, meta) information is
	// ALSO sent" and lists these three values as that protocol's addition.
	if mode != MouseMode::X10 {
		field += u8::from(modifiers.shift()) * 4
			+ u8::from(modifiers.alt()) * 8
			+ u8::from(modifiers.control()) * 16;
	}

	let column = at.col.saturating_add(1);
	let line = at.row.saturating_add(1);
	Some(match encoding {
		// SGR-Pixels: the SGR report with pixels where the cells were, and nothing else different
		// (§150). Shifted to one-based like the cells above, on the same reading — see the module
		// header for what no source states here.
		MouseEncoding::SgrPixels => {
			let final_byte = if released { 'm' } else { 'M' };
			let (x, y) = (at.x.saturating_add(1), at.y.saturating_add(1));
			format!("\x1b[<{field};{x};{y}{final_byte}").into_bytes()
		}
		MouseEncoding::Sgr => {
			let final_byte = if released { 'm' } else { 'M' };
			format!("\x1b[<{field};{column};{line}{final_byte}").into_bytes()
		}
		// Same three fields, each written as a code point rather than a byte, which is how
		// this mode lifts the 223 ceiling.
		MouseEncoding::Utf8 => {
			let mut out = vec![ESC, b'[', b'M'];
			let mut buffer = [0u8; 4];
			for value in [u32::from(field), u32::from(column), u32::from(line)] {
				let glyph = char::from_u32(value + 32)?;
				out.extend_from_slice(glyph.encode_utf8(&mut buffer).as_bytes());
			}
			out
		}
		// One byte per field. Past the ceiling the coordinate cannot be said at all, so
		// clamp: a report about the edge cell beats a report about a wrapped-around one.
		MouseEncoding::Default => {
			let mut out = vec![ESC, b'[', b'M'];
			for value in [u16::from(field), column, line] {
				out.push(32 + value.min(CLASSIC_MAX) as u8);
			}
			out
		}
	})
}

/// Whether `mode` asks to hear about `event` at all. A press goes to every reporting mode; a RELEASE
/// to every one but X10, which "sends an escape sequence only on button press" (§150); a move only to
/// the two motion modes, and `ButtonMotion` wants it only while a button is down.
fn wants(mode: MouseMode, event: MouseEvent) -> bool {
	match event {
		MouseEvent::Press(_) => mode != MouseMode::None,
		MouseEvent::Release(_) => !matches!(mode, MouseMode::None | MouseMode::X10),
		MouseEvent::Motion(held) => match mode {
			MouseMode::ButtonMotion => held.is_some(),
			MouseMode::AnyMotion => true,
			_ => false,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// No modifiers held — the common case for every report.
	fn none() -> Modifiers {
		Modifiers::empty()
	}

	/// A nominal cell, for turning a test's cell coordinates into the pixels that go with them.
	/// Deliberately not `ui::terminal`'s real metrics: this module knows nothing about the font, and a
	/// test that imported those numbers would fail the day somebody changed the type size.
	const CELL: (u16, u16) = (8, 17);

	/// A pointer position from a CELL, with the pixels that cell's top-left corner sits at — which is
	/// what every test written before §150 meant by its two numbers, and what the caller really passes.
	fn at(row: u16, col: u16) -> Position {
		Position {
			row,
			col,
			x: col * CELL.0,
			y: row * CELL.1,
		}
	}

	#[test]
	fn no_mouse_mode_reports_nothing() {
		// Until a program asks for the mouse, a click is the user's own business.
		assert_eq!(
			encode(
				MouseMode::None,
				MouseEncoding::Sgr,
				MouseEvent::Press(Button::Left),
				at(0, 0),
				none()
			),
			None
		);
	}

	#[test]
	fn sgr_press_and_release_differ_only_in_the_final_byte() {
		// Cell (row 4, col 9) is line 5, column 10 in the protocol's one-based numbering.
		let press = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Press(Button::Left),
			at(4, 9),
			none(),
		);
		let release = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Release(Button::Left),
			at(4, 9),
			none(),
		);
		assert_eq!(press.as_deref(), Some(&b"\x1b[<0;10;5M"[..]));
		assert_eq!(release.as_deref(), Some(&b"\x1b[<0;10;5m"[..]));
	}

	#[test]
	fn the_classic_encoding_offsets_every_field_by_32() {
		// ESC [ M then three bytes: button, column, row — each biased by 32.
		let report = encode(
			MouseMode::PressRelease,
			MouseEncoding::Default,
			MouseEvent::Press(Button::Right),
			at(0, 0),
			none(),
		);
		assert_eq!(report.as_deref(), Some(&[ESC, b'[', b'M', 34, 33, 33][..]));
	}

	#[test]
	fn the_classic_release_loses_the_button_but_sgr_keeps_it() {
		// The one-byte form can only say "a button came up"; SGR names which.
		let classic = encode(
			MouseMode::PressRelease,
			MouseEncoding::Default,
			MouseEvent::Release(Button::Right),
			at(0, 0),
			none(),
		)
		.unwrap();
		assert_eq!(classic[3], 32 + 3);
		let sgr = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Release(Button::Right),
			at(0, 0),
			none(),
		)
		.unwrap();
		assert_eq!(sgr, b"\x1b[<2;1;1m".to_vec());
	}

	#[test]
	fn a_coordinate_past_the_classic_ceiling_is_clamped() {
		// 223 is the last cell the single-byte form can name; a wider grid must not wrap
		// around and report a completely different column.
		let report = encode(
			MouseMode::PressRelease,
			MouseEncoding::Default,
			MouseEvent::Press(Button::Left),
			at(0, 400),
			none(),
		)
		.unwrap();
		assert_eq!(report[4], 255);
		// The same cell in SGR has no ceiling at all.
		let sgr = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Press(Button::Left),
			at(0, 400),
			none(),
		)
		.unwrap();
		assert_eq!(sgr, b"\x1b[<0;401;1M".to_vec());
	}

	#[test]
	fn motion_is_reported_only_by_the_modes_that_asked_for_it() {
		// A drag: ButtonMotion wants it, PressRelease does not.
		let drag = MouseEvent::Motion(Some(Button::Left));
		assert!(
			encode(
				MouseMode::PressRelease,
				MouseEncoding::Sgr,
				drag,
				at(0, 0),
				none()
			)
			.is_none()
		);
		assert_eq!(
			encode(
				MouseMode::ButtonMotion,
				MouseEncoding::Sgr,
				drag,
				at(0, 0),
				none()
			)
			.as_deref(),
			// 0 (left) + 32 (motion)
			Some(&b"\x1b[<32;1;1M"[..])
		);

		// A bare hover: only AnyMotion wants it, and it says "no button" (3 + 32).
		let hover = MouseEvent::Motion(None);
		assert!(
			encode(
				MouseMode::ButtonMotion,
				MouseEncoding::Sgr,
				hover,
				at(0, 0),
				none()
			)
			.is_none()
		);
		assert_eq!(
			encode(
				MouseMode::AnyMotion,
				MouseEncoding::Sgr,
				hover,
				at(0, 0),
				none()
			)
			.as_deref(),
			Some(&b"\x1b[<35;1;1M"[..])
		);
	}

	#[test]
	fn the_wheel_is_a_press_of_its_own_button() {
		// Scrolling reports button 64/65 as a press; there is no matching release.
		assert_eq!(
			encode(
				MouseMode::PressRelease,
				MouseEncoding::Sgr,
				MouseEvent::Press(Button::WheelUp),
				at(0, 0),
				none()
			)
			.as_deref(),
			Some(&b"\x1b[<64;1;1M"[..])
		);
		assert_eq!(
			encode(
				MouseMode::PressRelease,
				MouseEncoding::Sgr,
				MouseEvent::Press(Button::WheelDown),
				at(0, 0),
				none()
			)
			.as_deref(),
			Some(&b"\x1b[<65;1;1M"[..])
		);
	}

	// --- X10 and SGR-Pixels (§150) -----------------------------------------------------------------

	/// X10 "sends an escape sequence only on button press". No release, no motion — which is the whole
	/// of what separates it from the classic form beside the missing modifier bits.
	#[test]
	fn x10_reports_a_press_and_nothing_else() {
		let report = |event| {
			encode(
				MouseMode::X10,
				MouseEncoding::Default,
				event,
				at(4, 9),
				none(),
			)
		};
		assert_eq!(
			report(MouseEvent::Press(Button::Left)).as_deref(),
			Some(&[ESC, b'[', b'M', 32, 32 + 10, 32 + 5][..]),
			"CSI M Cb Cx Cy, each biased by 32"
		);
		assert!(report(MouseEvent::Release(Button::Left)).is_none());
		assert!(report(MouseEvent::Motion(Some(Button::Left))).is_none());
		assert!(report(MouseEvent::Motion(None)).is_none());
	}

	/// The modifier bits are the X11 protocol's addition, so X10 carries none — while the very same
	/// click under the mode that succeeded it carries all three.
	#[test]
	fn x10_carries_no_modifier_bits() {
		let held = Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT;
		let x10 = encode(
			MouseMode::X10,
			MouseEncoding::Default,
			MouseEvent::Press(Button::Left),
			at(0, 0),
			held,
		)
		.unwrap();
		assert_eq!(x10[3], 32, "button 0, and nothing added to it");
		let x11 = encode(
			MouseMode::PressRelease,
			MouseEncoding::Default,
			MouseEvent::Press(Button::Left),
			at(0, 0),
			held,
		)
		.unwrap();
		assert_eq!(x11[3], 32 + 4 + 8 + 16, "shift, meta and control");
	}

	/// SGR-Pixels is the SGR report with pixels where the cells were, and nothing else different —
	/// same button field, same `M`/`m` split. The inference about the coordinates is the module
	/// header's; this pins what it produces.
	#[test]
	fn the_pixel_encoding_is_sgr_with_pixels_in_place_of_cells() {
		let position = Position {
			row: 4,
			col: 9,
			x: 73,
			y: 68,
		};
		let press = encode(
			MouseMode::PressRelease,
			MouseEncoding::SgrPixels,
			MouseEvent::Press(Button::Left),
			position,
			none(),
		);
		assert_eq!(press.as_deref(), Some(&b"\x1b[<0;74;69M"[..]), "one-based");
		let release = encode(
			MouseMode::PressRelease,
			MouseEncoding::SgrPixels,
			MouseEvent::Release(Button::Left),
			position,
			none(),
		);
		assert_eq!(
			release.as_deref(),
			Some(&b"\x1b[<0;74;69m"[..]),
			"the release keeps its button, as SGR's does"
		);
		// The same event in cells, for the comparison the mode is defined by: one report, two units.
		let cells = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Press(Button::Left),
			position,
			none(),
		);
		assert_eq!(cells.as_deref(), Some(&b"\x1b[<0;10;5M"[..]));
	}

	/// A pixel coordinate has no 223 ceiling and no code-point widening to need — it is decimal text,
	/// like SGR's cells, which is the other half of why this encoding is SGR's and not the classic
	/// form's.
	#[test]
	fn a_pixel_coordinate_far_past_the_classic_ceiling_is_reported_whole() {
		let report = encode(
			MouseMode::AnyMotion,
			MouseEncoding::SgrPixels,
			MouseEvent::Motion(None),
			Position {
				row: 40,
				col: 200,
				x: 1600,
				y: 680,
			},
			none(),
		);
		assert_eq!(report.as_deref(), Some(&b"\x1b[<35;1601;681M"[..]));
	}

	#[test]
	fn modifiers_add_their_bits() {
		// Ctrl (16) + Alt (8) on a left press = 24.
		let report = encode(
			MouseMode::PressRelease,
			MouseEncoding::Sgr,
			MouseEvent::Press(Button::Left),
			at(0, 0),
			Modifiers::CTRL | Modifiers::ALT,
		);
		assert_eq!(report.as_deref(), Some(&b"\x1b[<24;1;1M"[..]));
	}

	#[test]
	fn the_utf8_encoding_writes_each_field_as_a_code_point() {
		// Column 300 is code point 300 + 32 + 1 = 333, two bytes in UTF-8 — which is the
		// whole point of this mode over the single-byte one.
		let report = encode(
			MouseMode::PressRelease,
			MouseEncoding::Utf8,
			MouseEvent::Press(Button::Left),
			at(0, 300),
			none(),
		)
		.unwrap();
		let mut expected = vec![ESC, b'[', b'M', 32];
		expected.extend_from_slice('\u{14d}'.to_string().as_bytes());
		expected.push(33);
		assert_eq!(report, expected);
	}
}

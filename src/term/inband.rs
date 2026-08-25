// term/inband.rs — the resize notification a program asks to be told about (PLAN §148).
//
//   CSI ? 2048 h                                   turn the notifications on
//   CSI ? 2048 l                                   turn them off
//   CSI 48 ; rows ; cols ; height ; width t        what the terminal then sends on every size change
//
// A terminal resize normally reaches a program as SIGWINCH, which is a signal delivered to the
// process group on the far side of a pty. That works for a program running directly under the shell
// and fails for everything that is not: a program behind another multiplexer, one reading a pipe, one
// on a platform with no such signal. In-band notification is the answer — the terminal writes the new
// size onto the same channel as everything else, so whatever is reading the stream learns about it.
//
// From the specification (`gist.github.com/rockorager/e695fb2924d36b2bcf1fff4a3704bd83`), quoted
// because two of these sentences decide code below:
//
//   "When first enabled, the terminal MUST send a report of the current size."
//   "If a terminal is not capable of reporting pixel sizes, it must report them as 0."
//
// WHY THE FLAG LIVES ON THE REPLY BUFFER AND NOT IN THIS MODULE.
//
// This module is a grammar and two constants; it holds no state, and the state it would have held is
// already sitting somewhere better. `ReplyBuffer` carries the grid size and one cell's pixel size —
// put there for the colour and size answers the engine's listener has to resolve without reaching
// back into the engine — and those FOUR NUMBERS ARE EXACTLY THE REPORT. Keeping the flag anywhere
// else would mean a second place that has to be locked in step with them.
//
// It is the same arrangement §145 made for the 7-bit/8-bit control form and for the same shape of
// reason, one step further along: that one lives there because every reply passes through it; this
// one because everything the reply is MADE of is already there.
//
// WHY THE GATE OWNS THE MODE. `alacritty_terminal` has no name for 2048 — it is absent from
// `NamedPrivateMode`, so it arrives as `PrivateMode::Unknown(2048)` and would be ignored, DECRQM
// included. That is mode 69's situation exactly (§102), and it gets mode 69's answer: the gate keeps
// it and does not forward, so there is one writer and the engine is not asked to hold a bit it has
// never heard of.

/// The private mode that turns in-band resize notifications on.
pub const MODE: u16 = 2048;

/// Whether the notifications are on at power-up. They are not: a terminal that volunteered its size
/// to a program that never asked would be writing bytes into a stream the program is parsing as its
/// own input.
pub const DEFAULT_ENABLED: bool = false;

/// The notification's own parameter, which is the `48` in `CSI 48 ; … t` — the report code, not an
/// echo of the mode number.
const REPORT: u16 = 48;

/// The resize notification: `CSI 48 ; rows ; cols ; height ; width t` (§148).
///
/// **Cells first, then pixels, and each pair is height before width** — the order the specification
/// gives, and the same trap `CSI 16 t`'s reply carries one module along (§147). Named arguments at
/// every call site rather than four bare numbers, for that reason.
///
/// The pixel figures are the cell size multiplied by the grid, which is the same arithmetic the
/// engine does for `CSI 14 t` — one source, now three spellings, and none of them able to disagree
/// with the others because all three read the one pair `Terminal::set_cell_pixels` wrote.
///
/// Multiplied in `u32` and printed as such. Neither product can approach that ceiling in a real
/// window — a grid is a few hundred cells each way and a cell a few dozen pixels — but the alternative
/// is a `u16` multiply that would be a panic in a debug build if it ever did, and there is nothing to
/// weigh against not writing that.
///
/// A cell size the GUI has not measured yet is zero, and zero is what gets reported. That is the
/// specification's own answer for a terminal that cannot report pixels at all, so a program reading
/// it is on documented ground rather than being handed a number cmote made up.
pub fn report(rows: u16, cols: u16, cell_width: u16, cell_height: u16) -> Vec<u8> {
	let height = u32::from(rows) * u32::from(cell_height);
	let width = u32::from(cols) * u32::from(cell_width);
	format!("\x1b[{REPORT};{rows};{cols};{height};{width}t").into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_report_states_the_cells_then_the_pixels() {
		// 10 rows by 40 columns, 8 x 17 pixel cells: 170 pixels high, 320 wide.
		assert_eq!(report(10, 40, 8, 17), b"\x1b[48;10;40;170;320t".to_vec());
	}

	/// Height before width in BOTH pairs, which is the one thing easy to get backwards here — and the
	/// reason a rectangular grid and a rectangular cell are used, so a transposition cannot hide.
	#[test]
	fn every_pair_is_height_before_width() {
		let bytes = report(24, 80, 7, 15);
		assert_eq!(bytes, b"\x1b[48;24;80;360;560t".to_vec());
		let fields: Vec<&[u8]> = bytes[2..bytes.len() - 1]
			.split(|&byte| byte == b';')
			.collect();
		assert_eq!(fields[1], b"24", "rows, not columns");
		assert_eq!(fields[2], b"80", "columns");
		assert_eq!(fields[3], b"360", "24 rows x 15 = the pixel HEIGHT");
		assert_eq!(fields[4], b"560", "80 cols x 7 = the pixel WIDTH");
	}

	/// The specification's own answer for a terminal with no pixel figures to give, which is the state
	/// cmote is in for the moment between construction and the GUI's first measurement.
	#[test]
	fn an_unmeasured_cell_reports_zero_pixels() {
		assert_eq!(report(10, 40, 0, 0), b"\x1b[48;10;40;0;0t".to_vec());
	}

	/// The products are taken in `u32`, so a grid and a cell whose product passes 65,535 is reported
	/// rather than wrapped — or, in a debug build, rather than panicking.
	#[test]
	fn a_product_past_a_u16_is_still_reported() {
		assert_eq!(
			report(1000, 1000, 100, 100),
			b"\x1b[48;1000;1000;100000;100000t".to_vec()
		);
	}

	/// The `48` is the REPORT's code and the `2048` is the MODE's. They are not the same number and
	/// neither is derived from the other, which is exactly the sort of coincidence that invites one to
	/// be written in terms of the other.
	#[test]
	fn the_report_code_is_not_the_mode_number() {
		assert_eq!(REPORT, 48);
		assert_eq!(MODE, 2048);
		assert!(!report(1, 1, 1, 1).starts_with(b"\x1b[2048"));
	}
}

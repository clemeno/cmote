// term/decmodes.rs — the DEC private modes cmote holds because the engine has no bit for them
// (PLAN §149).
//
//   CSI ? 5 h / l     DECSCNM   reverse video over the whole screen
//   CSI ? 45 h / l    XTREVWRAP reverse wraparound — a backspace at the left edge backs up a line
//   CSI ? 9 h / l               X10 mouse tracking — press only, no modifiers (§150)
//   CSI ? 1016 h / l            SGR-Pixels mouse — the SGR reports, in pixels (§150)
//
// `alacritty_terminal`'s `NamedPrivateMode` names fifteen modes and neither of these is among them,
// so both arrive at `Handler::set_private_mode` as `PrivateMode::Unknown(n)` and are dropped — DECRQM
// included, which would answer "not recognised" for a mode cmote implements. That is mode 69's
// situation (§102) and it gets mode 69's answer: the gate claims the sequence and does not forward
// it, so there is one writer and the engine is never asked to hold a bit it has never heard of.
//
// WHY THESE TWO ARE A TABLE AND THE OTHER TWO ARE NOT. cmote holds four private modes of its own now
// and they live in three places, which looks like an accident and is not:
//
//   * **69** (DECLRMM) lives in `term/margins.rs`, because turning it on brings a whole geometry with
//     it — a left and a right column, a backstop, a deferred wrap — and the bit is the smallest part
//     of what that module holds (§102).
//   * **2048** (in-band resize) lives on `ReplyBuffer`, because the four numbers its notification is
//     built from are already there and a second home would be a second thing to keep in step (§148).
//   * **5**, **45**, **9** and **1016** are *just bits*. None carries any other state, and the
//     operations wanted of them are table operations: set one, read one back for DECRQM and XTSAVE,
//     clear them all on RIS. So they are a table — and §150 was the test of that: adding the two mouse
//     modes was four lines here and nothing at all in the gate.
//
// THE TWO MOUSE MODES, AND WHY NEITHER CLEARS ANYTHING (§150).
//
// xterm holds ONE variable for the mouse protocol, so `CSI ? 1000 h` overwrites X10 and X10 overwrites
// it. cmote cannot copy that and does not try: modes 1000, 1002 and 1003 are the ENGINE's, and clearing
// one of them from here would make cmote a second writer of engine state (§71, §73).
//
// It does not need to. `alacritty_terminal` does not keep xterm's single variable either — it holds
// three independent flags and `Screen::mouse_mode` resolves them by PRIORITY, most specific first. So
// the divergence from xterm's model is one cmote already had, and mode 9 joins the bottom of that same
// ladder: any of the engine's three wins, and X10 reports only when none of them is set. The same for
// the encoding, where 1016 goes to the TOP, being the most specific of the four.
//
// That is a real divergence and it is worth stating plainly rather than burying: a program that sets
// 1000 and then 9 gets X11 reports from cmote and X10 reports from xterm. The alternative was writing
// engine state from here, and the ladder is where cmote already resolves this family.
//
// WHAT REVERSE WRAPAROUND MEANS HERE, AND WHAT NO SOURCE SAYS.
//
// xterm's ctlseqs names the mode and says nothing about its behaviour — "Ps = 4 5 -> Reverse-wraparound
// mode (XTREVWRAP), xterm", and that is the entire entry. The xterm manual page is where the
// definition is, on the `reverseWrap` resource:
//
//   "This allows the cursor to back up from the leftmost column of one line to the rightmost column
//    of the previous line."
//
// That sentence is what `term/gate.rs` implements and nothing more. Four things it does NOT say, each
// answered here by the narrowest reading rather than by a guess:
//
//   * **Which motions.** "Back up" is a backspace, and the resource exists for "editing long shell
//     command lines", where BS is what the shell sends. CUB (`CSI Ps D`) is left alone. xterm is
//     reported to wrap every leftward motion under this mode; that behaviour is in no document read
//     here, it is said to disagree with vttest, and another implementation refused it outright. A
//     cursor that moves where no source says it should is the divergence §102 exists to prevent.
//   * **Past the top.** There is no "previous line" above the first, so the cursor stays. xterm has a
//     SECOND mode for the wider behaviour — 1045, "Extended Reverse-wraparound" — which is itself
//     evidence that 45 alone is the restricted form.
//   * **Whether the line above must be a wrapped one.** The sentence puts no condition on it, so
//     neither does this. The alternative reading is safer and is not what the source says.
//   * **DECAWM.** The two are not coupled here. xterm is reported to fix a `need_wrap` corner only
//     when both are on; that is a statement about xterm's flag, not about what the mode means.

/// DECSCNM — reverse video over the whole screen.
pub const REVERSE_VIDEO: u16 = 5;

/// XTREVWRAP — reverse wraparound.
pub const REVERSE_WRAP: u16 = 45;

/// X10 mouse tracking — the original protocol, press-only and with no modifier bits (§150).
pub const X10_MOUSE: u16 = 9;

/// SGR-Pixels mouse mode — the SGR reports with pixel coordinates in place of cells (§150).
pub const PIXEL_MOUSE: u16 = 1016;

/// Every mode this table holds, in the order its bits are stored (§149, §150).
///
/// **This array is the table**, and it is the whole of what "adding a mode is a line" means: a number
/// here gets a bit, a DECRQM answer, an XTSAVE slot and a place in the RIS, with nothing else to edit.
/// The accessors below name their own mode rather than an index, so the ORDER of these four is not a
/// fact anything depends on.
///
/// A named field apiece would read more directly and was what §149 wrote first. It became this when
/// §150's two mouse modes took the count to four bools in one struct — which clippy refuses, and
/// rightly: four bools in a row is where a caller starts transposing them. Four bits under four names
/// cannot be transposed at all.
const HELD: [u16; 4] = [REVERSE_VIDEO, REVERSE_WRAP, X10_MOUSE, PIXEL_MOUSE];

/// The DEC private modes cmote holds itself, as a small table (§149, §150).
///
/// `Copy`, because it is four bits: the renderer reads it through `Screen` and the gate writes it, and
/// a borrow either way is larger than the thing borrowed.
#[derive(Debug, Default, Clone, Copy)]
pub struct DecModes {
	flags: [bool; HELD.len()],
}

/// Which bit of the table `mode` is, or `None` for a mode it does not hold.
fn slot(mode: u16) -> Option<usize> {
	HELD.iter().position(|held| *held == mode)
}

impl DecModes {
	/// Set or clear one mode, and say whether it was one of ours.
	///
	/// **The return value is what keeps the gate honest.** `false` means "not mine", and the gate then
	/// forwards the sequence to the engine exactly as it always did — so a mode added here is claimed
	/// in one place and a mode not added here cannot be swallowed by accident.
	pub fn set(&mut self, mode: u16, on: bool) -> bool {
		let Some(slot) = slot(mode) else {
			return false;
		};
		self.flags[slot] = on;
		true
	}

	/// Whether one mode is set, or `None` for a mode this table does not hold — which is what DECRQM
	/// and XTSAVE both need, and the same shape `Screen::private_mode` answers in (§141).
	pub fn get(self, mode: u16) -> Option<bool> {
		slot(mode).map(|slot| self.flags[slot])
	}

	/// Whether one mode this table is KNOWN to hold is set. The accessors below; a mode that is not in
	/// the table reads `false`, which cannot happen for any of them and is the honest answer if it did.
	fn holds(self, mode: u16) -> bool {
		self.get(mode) == Some(true)
	}

	/// DECSCNM — whether the whole screen is drawn with its foreground and background swapped.
	pub fn reverse_video(self) -> bool {
		self.holds(REVERSE_VIDEO)
	}

	/// XTREVWRAP — whether a backspace at the left edge backs up to the line above.
	pub fn reverse_wrap(self) -> bool {
		self.holds(REVERSE_WRAP)
	}

	/// Mode 9 — whether X10 mouse tracking is on (§150).
	pub fn x10_mouse(self) -> bool {
		self.holds(X10_MOUSE)
	}

	/// Mode 1016 — whether mouse reports carry pixel coordinates in place of cells (§150).
	pub fn pixel_mouse(self) -> bool {
		self.holds(PIXEL_MOUSE)
	}

	/// RIS — both modes back to their power-up state, which is off.
	///
	/// **RIS only.** Neither mode is on DEC's published DECSTR list, and §72 was careful not to widen
	/// the one DEC wrote — the same line §145 and §148 hold for the control form and the resize
	/// notifications.
	pub fn reset(&mut self) {
		*self = Self::default();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_mode_is_off_at_power_up() {
		let modes = DecModes::default();
		for mode in HELD {
			assert_eq!(modes.get(mode), Some(false), "mode {mode} at power-up");
		}
	}

	#[test]
	fn each_mode_is_set_and_read_back_on_its_own() {
		let mut modes = DecModes::default();
		assert!(modes.set(REVERSE_VIDEO, true));
		assert!(modes.reverse_video());
		assert!(!modes.reverse_wrap(), "the other one did not follow");
		assert!(modes.set(REVERSE_WRAP, true));
		assert!(modes.set(REVERSE_VIDEO, false));
		assert!(!modes.reverse_video());
		assert!(modes.reverse_wrap());
	}

	/// The return value is the gate's instruction to forward or not, so a mode this table does not hold
	/// must say so — otherwise the gate would swallow every private mode in the protocol.
	#[test]
	fn a_mode_the_table_does_not_hold_is_not_claimed() {
		let mut modes = DecModes::default();
		for mode in [1, 6, 7, 12, 25, 69, 1000, 1006, 1049, 2004, 2026, 2048] {
			assert!(!modes.set(mode, true), "mode {mode} is not this table's");
			assert_eq!(modes.get(mode), None);
		}
		// And nothing was written on the way past.
		assert!(!modes.reverse_video());
		assert!(!modes.reverse_wrap());
		assert!(!modes.x10_mouse());
		assert!(!modes.pixel_mouse());
	}

	/// One mode of the table paired with the accessor that reads it.
	type Reader = (u16, fn(DecModes) -> bool);

	/// Each accessor names its own mode, so the ORDER of `HELD` is not a fact anything depends on —
	/// which is what makes adding a mode a line rather than an edit in four places that have to agree.
	#[test]
	fn every_mode_in_the_table_has_exactly_one_accessor() {
		let readers: [Reader; HELD.len()] = [
			(REVERSE_VIDEO, DecModes::reverse_video),
			(REVERSE_WRAP, DecModes::reverse_wrap),
			(X10_MOUSE, DecModes::x10_mouse),
			(PIXEL_MOUSE, DecModes::pixel_mouse),
		];
		for (mode, read) in readers {
			let mut modes = DecModes::default();
			assert!(modes.set(mode, true));
			assert!(read(modes), "mode {mode} reads its own bit");
			// And nobody else's: every other accessor is still false.
			let others = readers
				.iter()
				.filter(|(other, _)| *other != mode)
				.filter(|(_, read)| read(modes))
				.count();
			assert_eq!(others, 0, "setting {mode} moved another mode's bit");
		}
	}

	/// `get` is what DECRQM and XTSAVE both read, so it has to report a mode that is RESET as `Some`
	/// rather than as absent — "recognised and off" and "never heard of it" are different answers.
	#[test]
	fn a_mode_that_is_off_is_still_recognised() {
		let modes = DecModes::default();
		assert_eq!(modes.get(REVERSE_VIDEO), Some(false));
		assert_eq!(modes.get(REVERSE_WRAP), Some(false));
	}

	#[test]
	fn a_reset_clears_every_mode_in_the_table() {
		let mut modes = DecModes::default();
		for mode in HELD {
			modes.set(mode, true);
		}
		modes.reset();
		for mode in HELD {
			assert_eq!(modes.get(mode), Some(false), "mode {mode} after RIS");
		}
	}
}

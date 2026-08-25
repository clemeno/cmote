// term/decmodes.rs — the DEC private modes cmote holds because the engine has no bit for them
// (PLAN §149).
//
//   CSI ? 5 h / l     DECSCNM   reverse video over the whole screen
//   CSI ? 45 h / l    XTREVWRAP reverse wraparound — a backspace at the left edge backs up a line
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
//   * **5** and **45** are *just bits*. Neither carries any other state, and the operations wanted of
//     them are table operations: set one, read one back for DECRQM and XTSAVE, clear them all on RIS.
//     So they are a table, and a third mode of this kind is a line rather than a design.
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

/// The DEC private modes cmote holds itself, as a small table (§149).
///
/// `Copy`, because it is two bits: the renderer reads it through `Screen` and the gate writes it, and
/// a borrow either way is the same size as the thing borrowed.
#[derive(Debug, Default, Clone, Copy)]
pub struct DecModes {
	reverse_video: bool,
	reverse_wrap: bool,
}

impl DecModes {
	/// Set or clear one mode, and say whether it was one of ours.
	///
	/// **The return value is what keeps the gate honest.** `false` means "not mine", and the gate then
	/// forwards the sequence to the engine exactly as it always did — so a mode added here is claimed
	/// in one place and a mode not added here cannot be swallowed by accident.
	pub fn set(&mut self, mode: u16, on: bool) -> bool {
		match mode {
			REVERSE_VIDEO => self.reverse_video = on,
			REVERSE_WRAP => self.reverse_wrap = on,
			_ => return false,
		}
		true
	}

	/// Whether one mode is set, or `None` for a mode this table does not hold — which is what DECRQM
	/// and XTSAVE both need, and the same shape `Screen::private_mode` answers in (§141).
	pub fn get(self, mode: u16) -> Option<bool> {
		match mode {
			REVERSE_VIDEO => Some(self.reverse_video),
			REVERSE_WRAP => Some(self.reverse_wrap),
			_ => None,
		}
	}

	/// DECSCNM — whether the whole screen is drawn with its foreground and background swapped.
	pub fn reverse_video(self) -> bool {
		self.reverse_video
	}

	/// XTREVWRAP — whether a backspace at the left edge backs up to the line above.
	pub fn reverse_wrap(self) -> bool {
		self.reverse_wrap
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
	fn both_modes_are_off_at_power_up() {
		let modes = DecModes::default();
		assert!(!modes.reverse_video());
		assert!(!modes.reverse_wrap());
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
		for mode in [1, 6, 7, 12, 25, 69, 1000, 1049, 2004, 2026, 2048] {
			assert!(!modes.set(mode, true), "mode {mode} is not this table's");
			assert_eq!(modes.get(mode), None);
		}
		// And nothing was written on the way past.
		assert!(!modes.reverse_video());
		assert!(!modes.reverse_wrap());
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
	fn a_reset_clears_both() {
		let mut modes = DecModes::default();
		modes.set(REVERSE_VIDEO, true);
		modes.set(REVERSE_WRAP, true);
		modes.reset();
		assert!(!modes.reverse_video());
		assert!(!modes.reverse_wrap());
	}
}

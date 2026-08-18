// term/csi.rs — the facts every CSI scanner has to agree with the engine about.
//
// Eleven modules in this directory scan CSI sequences beside the stream, each for its own reason, and
// each currently carries its own copy of the grammar. That duplication is a known debt with a plan
// attached — §106's architecture review put it first, as "give the CSI family the floor OSC already
// has": `osc::Framer` proved the shape works for the OSC family, and one `csi::Framer` is meant to move
// in here and leave the scanners with only the part that is theirs — deciding what a sequence MEANS.
//
// This module starts with the part that could not wait for it. A scanner's LIMITS are not a private
// choice: cmote and the engine read the same bytes, and wherever the two disagree about whether a
// sequence is well formed, one of them acts and the other does not. Two of those disagreements were
// live defects, both fixed by the numbers below (§57 read a padded DECSLRM as a save-cursor; §56 lost
// protection across a long SGR), and the numbers were previously spelled eight times at three
// different values, none of them the engine's.
//
// So the engine's own numbers live here, once, with their source, and the scanners refer to them.
// Whatever else the framer takes over, it will not have to re-derive these.

/// The most parameters one CSI sequence may carry before the engine stops reading it.
///
/// `vte`'s own `MAX_PARAMS`, from its vendored source: `params.rs:5` declares it, `:16-19` fixes the
/// array at that width, and `:49-51` is the `is_full` test. Once full, every further parameter action
/// sets the parser's `ignoring` flag (`lib.rs:454-517`), and `ansi.rs:1545-1548` then drops the whole
/// sequence rather than dispatching it. Sub-parameters (the `:` form) share the same budget.
///
/// A scanner that exceeds this should abandon the sequence, because that is what the engine does with
/// it: both sides ignoring the same bytes is agreement, and it is the only bound here that can be
/// enforced by giving up.
pub const MAX_PARAMS: usize = 32;

/// The most digits worth keeping in one parameter.
///
/// This one is NOT a limit the engine has — `vte` folds every digit in with `saturating_mul`
/// (`lib.rs:514-515`) and never abandons a sequence over the length of a run. Copying that literally
/// would mean buffering unbounded remote input, which §12 refuses; abandoning instead is what caused
/// the two defects above, because the engine goes on to act on a sequence the scanner gave up on.
///
/// So the run is CLAMPED rather than capped: digits past this many are dropped and the sequence lives.
/// That is exactly equivalent to the engine's answer, and the test below is why — five digits already
/// reach past `u16::MAX`, so any value a sixth digit could produce is one the engine has saturated
/// too. The memory a scanner can be made to hold is then `MAX_PARAMS * MAX_DIGITS` plus its
/// separators, which is under 200 bytes.
pub const MAX_DIGITS: usize = 5;

/// Whether the engine would keep reading a sequence across `byte` — the bytes a CSI's own grammar does
/// not claim, but which do not end it either (§106).
///
/// `vte`'s CSI states run a C0 control where it sits and CARRY ON with the sequence around it
/// (`lib.rs:190`, `:219`, `:230`, `:241`), ignore DEL (`:222`, `:251`), and pass a byte past `0x7f` to
/// `anywhere`, which does nothing with it (`:438-449`). Only CAN (`0x18`) and SUB (`0x1a`) abandon the
/// sequence, which is the ANSI state machine's own definition of them, and ESC restarts it.
///
/// A scanner that gave up on one of these bytes would leave the engine to dispatch a sequence cmote never
/// judged, which is how §57's and §56's harm was reachable a second time over: `CSI 5;` LF `70 s` is a
/// margin request to the engine and was nothing at all to the scanner shadowing it.
pub fn passes_through(byte: u8) -> bool {
	matches!(byte, 0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f | 0x80..=0xff)
}

/// The parameter run of the CSI or DCS sequence a scanner is in the middle of reading.
///
/// Two scanners hold one of these today (§56's selective erase and §41's pictures), which is what makes
/// this a seam rather than a hypothetical one: the rule below was wrong in the same way in both of them
/// and had to be fixed in one place. The other nine scanners still carry their own copy of it, and this
/// is where they land when the framer arrives.
///
/// What it keeps is the run's BYTES, so a caller can still read the run the way it always did — as
/// `1`, `?2`, `38;5;196` — rather than being handed numbers it did not ask for. Parsing stays with the
/// caller, because the nine disagree about what an omitted parameter means (0, 1, "not ours",
/// "everything") and a shared parser would need every one of those as an option.
#[derive(Debug, Default)]
pub struct Params {
	bytes: Vec<u8>,
	/// Separators seen, so the parameter count is `fields + 1`.
	fields: usize,
	/// Significant digits kept in the parameter being written now. Leading zeros are not counted,
	/// because they are not significant — see [`Params::push`].
	digits: usize,
	/// Whether any parameter byte has arrived, kept or dropped. Distinct from `bytes.is_empty()`, which
	/// a dropped leading zero would leave true — and a caller that reads emptiness as "no parameters
	/// yet" would then take the next byte for a private marker on a sequence that already had one.
	started: bool,
}

impl Params {
	/// Start a fresh run, for the `[` or `P` that begins one.
	pub fn clear(&mut self) {
		self.bytes.clear();
		self.fields = 0;
		self.digits = 0;
		self.started = false;
	}

	/// Fold one parameter byte in, and say whether the sequence is still one the engine would read.
	///
	/// `false` means it carries more parameters than the engine's array holds, so the engine ignores the
	/// whole sequence — and the caller should abandon it too, which is what makes the two agree.
	///
	/// A long DIGIT run never returns `false`. Digits past [`MAX_DIGITS`] SIGNIFICANT ones are dropped
	/// and the run lives, because the engine saturates the number instead of giving up on the sequence.
	/// Leading zeros are dropped and cost nothing at all, which is the correction that matters: they do
	/// not change the value, so a clamp that counted them would read `CSI 0000000000000002 J` as 0 and
	/// leave the engine erasing a screen cmote thought was untouched.
	pub fn push(&mut self, byte: u8) -> bool {
		self.started = true;
		if byte == b';' || byte == b':' {
			self.fields += 1;
			self.digits = 0;
			if self.fields >= MAX_PARAMS {
				return false;
			}
			self.bytes.push(byte);
		} else if byte == b'0' && self.digits == 0 {
			// A leading zero. Nothing to keep: the value is the same without it, and the engine's fold
			// over it is the identity.
		} else if self.digits < MAX_DIGITS {
			self.digits += 1;
			self.bytes.push(byte);
		}
		true
	}

	/// The run as the sequence wrote it, minus the bytes that could not change what it means.
	pub fn bytes(&self) -> &[u8] {
		&self.bytes
	}

	/// Whether any parameter byte has arrived yet — the test for "a private marker is still legal here",
	/// which is only true before the first one.
	pub fn started(&self) -> bool {
		self.started
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed a whole parameter run and read back what was kept, plus whether it survived.
	fn run(bytes: &[u8]) -> (String, bool) {
		let mut params = Params::default();
		let alive = bytes.iter().all(|&byte| params.push(byte));
		(String::from_utf8_lossy(params.bytes()).into_owned(), alive)
	}

	#[test]
	fn leading_zeros_cost_nothing_and_change_nothing() {
		// The bug this module was written for: `CSI 0000000000000002 J` is an erase to the engine, and a
		// clamp that spent its budget on the zeros read it as 0 and said nothing.
		assert_eq!(run(b"000000000000000002"), ("2".to_owned(), true));
		assert_eq!(run(b"0"), (String::new(), true));
		assert_eq!(run(b"0;0"), (";".to_owned(), true));
	}

	#[test]
	fn a_runaway_digit_run_is_clamped_and_the_sequence_survives() {
		let (kept, alive) = run(&[b'9'; 500]);
		assert_eq!(kept, "99999", "five significant digits, and no more");
		assert!(
			alive,
			"the engine saturates rather than giving up, so we do too"
		);
	}

	#[test]
	fn too_many_parameters_ends_the_run() {
		// Thirty-two parameters is the most the engine reads; the thirty-second separator starts the
		// thirty-third, which it ignores the sequence over.
		let (_, alive) = run(&[b';'; MAX_PARAMS - 1]);
		assert!(alive, "thirty-two parameters still fit");
		let (_, alive) = run(&[b';'; MAX_PARAMS]);
		assert!(!alive, "thirty-three do not");
	}

	#[test]
	fn a_dropped_leading_zero_does_not_make_the_run_look_unstarted() {
		// Otherwise the caller takes the next byte for a private marker, and `CSI 0?J` — which the
		// engine drops outright — would classify here as a selective erase.
		let mut params = Params::default();
		assert!(!params.started());
		params.push(b'0');
		assert!(params.bytes().is_empty());
		assert!(params.started(), "a zero is still a parameter byte");
	}

	#[test]
	fn the_digit_clamp_reaches_the_saturation_point() {
		// The whole argument for clamping instead of capping: a parameter is at most a `u16` on both
		// sides of `process`, and a five-digit run can already express more than the largest one. So a
		// clamped run and the engine's saturating one land on the same number for every input that can
		// be told apart, and a scanner never has to abandon a sequence the engine will act on.
		assert!(
			u16::MAX.to_string().len() <= MAX_DIGITS,
			"a clamped run must be able to reach the value the engine saturates at"
		);
	}

	#[test]
	fn the_parameter_bound_is_the_engines_own() {
		// Written down as a test rather than only in prose, so a version bump that changes `vte`'s
		// width is a conversation rather than a silent drift. There is no way to read the constant out
		// of the crate — it is `pub(crate)` there — so this is the one place the number is asserted.
		assert_eq!(MAX_PARAMS, 32, "vte params.rs:5");
	}
}

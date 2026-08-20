// term/sgrstack.rs — xterm's video-attribute stack (PLAN §85).
//
//   CSI Pm # {   XTPUSHSGR — push the current video attributes onto a stack
//   CSI # }      XTPOPSGR  — pop them back
//   CSI Pm # p   the same push, spelled with a lower-case final byte
//   CSI # q      the same pop
//
// xterm's ctlseqs, on the two spellings and on the parameters:
//
//     Push video attributes onto stack (XTPUSHSGR), xterm.  The optional parameters correspond to
//     the SGR encoding for video attributes, except for colors (which do not have a unique SGR
//     code):
//       Ps = 1  =>  Bold          Ps = 2  =>  Faint         Ps = 3  =>  Italicized
//       Ps = 4  =>  Underlined    Ps = 5  =>  Blink         Ps = 7  =>  Inverse
//       Ps = 8  =>  Invisible     Ps = 9  =>  Crossed-out   Ps = 21 =>  Doubly-underlined
//       Ps = 30 =>  Foreground color                        Ps = 31 =>  Background color
//     If no parameters are given, all of the video attributes are saved.  The stack is limited to
//     10 levels.
//
//     CSI # p is an alias for CSI # { , used to work around language limitations of C#.
//
// WHY THIS IS WORK RATHER THAN A REFUSAL. §84 found this sequence wearing another one's name: the
// matrix had `CSI # p` / `# q` labelled the COLOUR stack (XTPUSHCOLORS, which is the capitals
// `CSI # P` / `# Q`) and refused it with the colour stack's argument — "a stack over a palette that
// is never read has nothing to save or restore". That argument is void here. This stack is over bold,
// italic, underline, reverse and the two colours, every one of which cmote draws, and the failure mode
// of ignoring it is not a missing feature but a WRONG SCREEN: a program that pushes, paints itself red
// and pops expects the pen it had, and a terminal that drops both halves leaves everything after it
// red. There is no §6 policy to refuse on either — nothing here leaves the tab, nothing speaks for the
// machine, nothing touches anything of the user's. A remote may change what its own tab looks like.
//
// WHY IT IS FED RATHER THAN WRITTEN. The pen is the engine's — the template cell it stamps onto every
// glyph — and cmote declines to become a second writer of engine state (§71, §73). So this scanner
// only reports where each request sat, `term/mod.rs` READS the template at that point (which it
// already does for DECRQSS, §33), and a pop is carried out by feeding the engine the pen it is being
// restored to, spelled in SGR. That is §72's route for DECSTR and §74's for DECST8C, and every byte
// fed is a sequence the compatibility matrix already marks supported.
//
// One consequence of the route, disclosed rather than papered over: a pop restores everything SGR can
// say and nothing it cannot. Blink is not in the engine at all (no cell flag — §5), so `Ps = 5` names
// a value there is nothing to save, and the OSC 8 hyperlink a cell may carry is not an SGR attribute
// and does not travel. cmote's borrowed DECSCA protection bit (§56) rides in the same flag word as the
// attributes and would be cleared by the `CSI 0 m` that opens a restore, so `term/mod.rs` reads it
// first and puts it back afterwards — the pen's protection is not a video attribute and a stack of
// video attributes must not move it.
//
// WHERE THIS DEPARTS FROM XTERM. The stack is ten deep, as documented. An eleventh push is dropped —
// so is xterm's — but cmote also COUNTS the drop and drops the matching pop with it. xterm does not,
// and the difference matters: with the push dropped and the pop honoured, every pop after an overflow
// is one level out, so a program that nests deeply gets its outer attributes restored at an inner
// level and no error anywhere. Counting keeps the nesting aligned at the price of one `usize`.

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The intermediate byte both sequences carry, and the whole of what separates them from their
/// neighbours: `CSI ! p` is DECSTR, `CSI $ p` is DECRQM, `CSI SP q` is DECSCUSR.
const HASH: u8 = b'#';

/// How deep the stack goes, quoted from xterm above.
pub const DEPTH: usize = 10;

/// Which video attributes a push saves — xterm's parameter list, one bit each.
///
/// Kept as cmote's own bitset rather than a list of raw numbers so the wire's vocabulary is turned
/// into meaning once, here, where it can be tested without a terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mask(u16);

impl Mask {
	/// `Ps = 1`, bold.
	pub const BOLD: Mask = Mask(1 << 0);
	/// `Ps = 2`, faint — the engine's `DIM`.
	pub const FAINT: Mask = Mask(1 << 1);
	/// `Ps = 3`, italic.
	pub const ITALIC: Mask = Mask(1 << 2);
	/// `Ps = 4`, underlined.
	pub const UNDERLINE: Mask = Mask(1 << 3);
	/// `Ps = 5`, blink. Selects nothing here: the engine carries no blink flag, so there is no value
	/// to save and none to restore (§5, and the SGR table's `5 / 6` row).
	pub const BLINK: Mask = Mask(1 << 4);
	/// `Ps = 7`, inverse.
	pub const INVERSE: Mask = Mask(1 << 5);
	/// `Ps = 8`, invisible — the engine's `HIDDEN`.
	pub const INVISIBLE: Mask = Mask(1 << 6);
	/// `Ps = 9`, crossed out — the engine's `STRIKEOUT`.
	pub const CROSSED_OUT: Mask = Mask(1 << 7);
	/// `Ps = 21`, doubly underlined.
	pub const DOUBLY_UNDERLINED: Mask = Mask(1 << 8);
	/// `Ps = 30`, the foreground colour.
	pub const FOREGROUND: Mask = Mask(1 << 9);
	/// `Ps = 31`, the background colour.
	pub const BACKGROUND: Mask = Mask(1 << 10);

	/// Every attribute — what a push with no parameters at all saves.
	pub const ALL: Mask = Mask(0x7ff);

	/// Nothing, the empty accumulator a parameter list is built onto.
	pub const NONE: Mask = Mask(0);

	/// Whether every bit of `other` is set here. Used with a single-bit constant, so it reads as
	/// "does this push cover the foreground?".
	#[must_use]
	pub fn contains(self, other: Mask) -> bool {
		self.0 & other.0 == other.0
	}

	/// Both masks together.
	#[must_use]
	fn with(self, other: Mask) -> Mask {
		Mask(self.0 | other.0)
	}

	/// One of xterm's parameter values as the attribute it names, or `None` for a number this
	/// terminal has no attribute for.
	///
	/// An unrecognised value is IGNORED and the rest of the list still applies, which is how an SGR
	/// behaves and the rule §59 already wrote down for DECCARA's selector list. An OMITTED parameter is
	/// a different thing and drops the whole sequence — see [`push_mask`].
	fn from_code(code: u16) -> Option<Mask> {
		match code {
			1 => Some(Mask::BOLD),
			2 => Some(Mask::FAINT),
			3 => Some(Mask::ITALIC),
			4 => Some(Mask::UNDERLINE),
			5 => Some(Mask::BLINK),
			7 => Some(Mask::INVERSE),
			8 => Some(Mask::INVISIBLE),
			9 => Some(Mask::CROSSED_OUT),
			21 => Some(Mask::DOUBLY_UNDERLINED),
			30 => Some(Mask::FOREGROUND),
			31 => Some(Mask::BACKGROUND),
			_ => None,
		}
	}
}

/// What one recognised sequence asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SgrStackRequest {
	/// Save the named attributes of the pen as it stands. `Mask::ALL` when no parameters were given.
	Push(Mask),
	/// Restore the top of the stack. Nothing at all when the stack is empty.
	Pop,
	/// RIS (`ESC c`) — throw the whole stack away (§86).
	///
	/// A hard reset puts the terminal back to power-on, and a power-on terminal has nothing pushed.
	/// Without this a program that pushed, reset and popped would be handed a pen from BEFORE the
	/// reset — a remote's state outliving the one sequence whose whole job is to remove it.
	///
	/// **DECSTR does not do this**, deliberately, and that is the same split `term/rect.rs` makes for
	/// DECSACE: RIS resets it, the soft reset does not, because DEC's published DECSTR list does not
	/// name it and §72 honours that list rather than widening it. Neither does the alternate-screen
	/// swap: the engine saves and restores the pen across it (DECSC / DECRC, mode 1049) and a stack of
	/// pens is not the pen. No source read so far says what xterm does with its own stack at either —
	/// see PLAN §86, where the reasoning is on the record rather than the finding.
	Reset,
}

/// The XTPUSHSGR / XTPOPSGR scanner (§85). Feed it every byte of shell output; it reports where each
/// request sat and what it asks for, for `term/mod.rs` to carry out against the live pen.
#[derive(Debug, Default)]
pub struct SgrStack {
	/// The CSI grammar, shared with the other scanners (§111). What is left in this module is xterm's
	/// parameter list and the two spellings of each half — the only part of the reading that is nobody
	/// else's.
	framer: super::csi::Framer,
	/// Whether the previous byte was an ESC, for RIS (`ESC c`) — not a CSI, so not the framer's.
	///
	/// Read here rather than borrowed from `term/scp.rs`, which reads the same byte for its own store:
	/// each scanner reads the stream itself, so neither can come to depend on the other's idea of where
	/// a sequence sat.
	after_escape: bool,
}

impl SgrStack {
	/// Scan a chunk of shell output, returning `(offset, request)` for each sequence found. Safe at
	/// any chunk boundary — the state machine carries over between calls, so a sequence may be split
	/// anywhere, even between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the rectangles (§58), the tab-stop
	/// reset (§74) and DECXCPR (§82). A push must read the pen as it stood where the push was written,
	/// and a pop must restore it there, so both are answered with the engine advanced exactly that far
	/// and no further.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, SgrStackRequest)> {
		// Two families in one stream, so they are collected apart and merged by offset. Order is not
		// cosmetic: a reset empties the stack, so a push that arrived before one has to be applied
		// before it, or the stack would be left holding a pen from before the reset (§86).
		let mut requests = Vec::new();
		self.framer.feed(bytes, |offset, csi| {
			if let Some(found) = request(csi) {
				requests.push((offset, found));
			}
		});
		// RIS, the one sequence here that is not a CSI — see `SgrStackRequest::Reset` for why this one
		// and not the soft reset.
		for (index, &byte) in bytes.iter().enumerate() {
			if self.after_escape && byte == b'c' {
				requests.push((index + 1, SgrStackRequest::Reset));
			}
			self.after_escape = byte == ESC;
		}
		requests.sort_by_key(|&(offset, _)| offset);
		requests
	}
}

/// What a finished sequence asks for, or `None` when it is not one of ours.
///
/// All three of final byte, marker and intermediates are tested, which is the near-miss rule §56
/// wrote down and §82 kept: `CSI ! p` is DECSTR, `CSI $ p` and `CSI ? $ p` are DECRQM, `CSI SP q`
/// is DECSCUSR, and `CSI p` bare is nothing at all. Only the `#` intermediate, with no private
/// marker, is this pair.
///
/// A pop with parameters is refused rather than read generously. XTPOPSGR takes none, so
/// `CSI 1 # }` is a sequence cmote does not understand, and half-understanding one is the reading
/// this project keeps finding at the bottom of its own mistakes (§82 tightened DECXCPR the same
/// way). The count, not the run's emptiness, is what asks that question: `CSI 0 # q` carries a
/// parameter that happens to be zero, and `CSI ; # q` carries two that are both omitted.
fn request(csi: &super::csi::Csi<'_>) -> Option<SgrStackRequest> {
	if !matches!((csi.marker(), csi.intermediates()), (None, [HASH])) {
		return None;
	}
	match csi.final_byte() {
		b'{' | b'p' => push_mask(csi).map(SgrStackRequest::Push),
		b'}' | b'q' => (csi.param_count() == 0).then_some(SgrStackRequest::Pop),
		_ => None,
	}
}

/// The attributes a push names. No parameters at all is every attribute, as xterm has it.
///
/// `None` — the whole sequence dropped — when a parameter is present but EMPTY. An unknown number is
/// ignored and the rest of the list still applies (`Mask::from_code`); an omitted one means the
/// parameters were not what this scanner thinks they were, and acting on the part it recognised would
/// be guessing at the rest.
///
/// Two readings changed when the grammar moved into the framer (§111), and both were accidents of the
/// hand-rolled walk this replaced rather than rules the module had written down:
///
///   * A number past a `u16` — `CSI 99999 # {` — used to drop the sequence, because the walk folded
///     with `checked_mul` and answered `None`. It saturates now, as the engine does, so what comes out
///     is an UNKNOWN code and the rest of the list still applies. That is what the rule above always
///     said should happen to a number this terminal has no attribute for.
///   * A sub-parameter — `CSI 1 : 3 # {` — used to drop it too, but only because the `:` made the
///     field unreadable as a number. It reads as two codes now, which is how the engine's own parser
///     groups those bytes. No source read so far says what xterm makes of a sub-parameter here, so
///     this follows the parser rather than a guess — and the same shape of accident was all that
///     covered `term/scp.rs` until §111 found it there.
fn push_mask(csi: &super::csi::Csi<'_>) -> Option<Mask> {
	if csi.param_count() == 0 {
		return Some(Mask::ALL);
	}
	let mut mask = Mask::NONE;
	for index in 0..csi.param_count() {
		// `None` here is an EMPTY field, and nothing else: the framer keeps a parameter run to digits
		// and separators, so there is no longer such a thing as an unreadable one. A written `0` is a
		// code — an unknown one — and telling it from a parameter nobody wrote is what §111 restored to
		// `Params` for this scanner's sake.
		let code = csi.param(index)?;
		if let Some(attribute) = Mask::from_code(code) {
			mask = mask.with(attribute);
		}
	}
	Some(mask)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<(usize, SgrStackRequest)> {
		SgrStack::default().feed(bytes)
	}

	/// A push carrying `fields` parameters, every one of them bold — so a sequence that survives the
	/// grammar reports `Push(BOLD)` and one that does not reports nothing, which makes the bound the
	/// only thing a test built on this can be measuring.
	fn a_list_of(fields: usize) -> Vec<u8> {
		let params = vec!["1"; fields].join(";");
		format!("\x1b[{params}#{{").into_bytes()
	}

	/// Both sequences, and the offset each reports: ONE PAST the final byte.
	#[test]
	fn a_push_and_a_pop_are_found_just_past_their_final_bytes() {
		assert_eq!(
			scan(b"\x1b[#{"),
			vec![(4, SgrStackRequest::Push(Mask::ALL))]
		);
		assert_eq!(scan(b"\x1b[#}"), vec![(4, SgrStackRequest::Pop)]);
		assert_eq!(
			scan(b"ab\x1b[#{cd"),
			vec![(6, SgrStackRequest::Push(Mask::ALL))]
		);
	}

	/// xterm's own aliases, which exist "to work around language limitations of C#" and are the
	/// spelling the matrix carried under the wrong name until §84.
	#[test]
	fn the_lower_case_aliases_are_the_same_two_requests() {
		assert_eq!(
			scan(b"\x1b[#p"),
			vec![(4, SgrStackRequest::Push(Mask::ALL))]
		);
		assert_eq!(scan(b"\x1b[#q"), vec![(4, SgrStackRequest::Pop)]);
	}

	/// No parameters saves everything, which is what xterm says and what a program that pushes
	/// without thinking about it expects.
	#[test]
	fn a_push_with_no_parameters_saves_every_attribute() {
		assert_eq!(
			scan(b"\x1b[#{"),
			vec![(4, SgrStackRequest::Push(Mask::ALL))]
		);
		assert!(Mask::ALL.contains(Mask::BOLD));
		assert!(Mask::ALL.contains(Mask::BACKGROUND));
	}

	/// Each of xterm's eleven values, mapped to the attribute it names.
	#[test]
	fn each_parameter_names_the_attribute_xterm_gives_it() {
		let cases: [(&[u8], Mask); 11] = [
			(b"\x1b[1#{", Mask::BOLD),
			(b"\x1b[2#{", Mask::FAINT),
			(b"\x1b[3#{", Mask::ITALIC),
			(b"\x1b[4#{", Mask::UNDERLINE),
			(b"\x1b[5#{", Mask::BLINK),
			(b"\x1b[7#{", Mask::INVERSE),
			(b"\x1b[8#{", Mask::INVISIBLE),
			(b"\x1b[9#{", Mask::CROSSED_OUT),
			(b"\x1b[21#{", Mask::DOUBLY_UNDERLINED),
			(b"\x1b[30#{", Mask::FOREGROUND),
			(b"\x1b[31#{", Mask::BACKGROUND),
		];
		for (bytes, expected) in cases {
			let found = scan(bytes);
			assert_eq!(
				found.len(),
				1,
				"{:?} should be one push",
				String::from_utf8_lossy(bytes)
			);
			assert_eq!(found[0].1, SgrStackRequest::Push(expected));
		}
	}

	/// A list accumulates, and the numbers are matched whole rather than by prefix — `31` is the
	/// background and `3` is italic, which a prefix match would conflate.
	#[test]
	fn a_list_of_parameters_accumulates() {
		assert_eq!(
			scan(b"\x1b[1;4;31#{"),
			vec![(
				10,
				SgrStackRequest::Push(Mask::BOLD.with(Mask::UNDERLINE).with(Mask::BACKGROUND))
			)]
		);
	}

	/// A value this terminal has no attribute for is ignored and the rest of the list still applies —
	/// the rule §59 wrote down for DECCARA's selectors, which is how an SGR itself behaves.
	#[test]
	fn an_unknown_parameter_is_ignored_and_the_rest_still_applies() {
		assert_eq!(
			scan(b"\x1b[1;6;99#{"),
			vec![(10, SgrStackRequest::Push(Mask::BOLD))]
		);
	}

	/// A parameter nobody wrote means the parameters were not what this scanner thinks, so the sequence
	/// is dropped rather than half-read — and a written zero is not that.
	#[test]
	fn an_omitted_parameter_drops_the_whole_sequence_and_a_written_zero_does_not() {
		assert!(scan(b"\x1b[1;;4#{").is_empty(), "an empty field is not a 0");
		assert!(scan(b"\x1b[;#{").is_empty());
		// `0` is a code xterm never defined, so it is IGNORED and the rest of the list applies. The two
		// came out identical until §111 gave `Params` its all-zero field back, and this scanner is the
		// only one in the directory that can tell the difference.
		assert_eq!(
			scan(b"\x1b[1;0#{"),
			vec![(7, SgrStackRequest::Push(Mask::BOLD))]
		);
		assert_eq!(
			scan(b"\x1b[0#{"),
			vec![(5, SgrStackRequest::Push(Mask::NONE))],
			"a lone zero is a push that saves nothing, not a dropped sequence"
		);
	}

	/// A sub-parameter reads as another code in the list, which is how the engine's own parser groups
	/// those bytes. Before the shared grammar the `:` dropped the sequence — by making the field
	/// unreadable as a number, not by any rule this module had written down (§111).
	#[test]
	fn a_sub_parameter_reads_as_another_code_in_the_list() {
		assert_eq!(
			scan(b"\x1b[1:3#{"),
			vec![(7, SgrStackRequest::Push(Mask::BOLD.with(Mask::ITALIC)))]
		);
	}

	/// A number too big for a parameter is an UNKNOWN code, not a malformed one: the engine saturates
	/// rather than giving up, so what comes out is simply a value xterm never defined (§111).
	#[test]
	fn a_number_past_a_u16_is_an_unknown_code_rather_than_a_malformed_one() {
		assert_eq!(
			scan(b"\x1b[1;99999#{"),
			vec![(11, SgrStackRequest::Push(Mask::BOLD))]
		);
	}

	/// XTPOPSGR takes no parameters, so one carrying any is a sequence cmote does not understand.
	#[test]
	fn a_pop_with_parameters_is_not_ours() {
		assert!(scan(b"\x1b[1#}").is_empty());
		assert!(scan(b"\x1b[0#q").is_empty());
	}

	/// The `#` intermediate is the whole of what separates these from their neighbours on the same
	/// final bytes — DECSTR, DECRQM and DECSCUSR all sit one intermediate away.
	#[test]
	fn the_neighbours_on_the_same_final_bytes_are_left_alone() {
		assert!(scan(b"\x1b[!p").is_empty(), "DECSTR");
		assert!(scan(b"\x1b[$p").is_empty(), "DECRQM, ANSI");
		assert!(scan(b"\x1b[?4$p").is_empty(), "DECRQM, private");
		assert!(scan(b"\x1b[ q").is_empty(), "DECSCUSR");
		assert!(scan(b"\x1b[p").is_empty(), "no intermediate at all");
		assert!(scan(b"\x1b[#y").is_empty(), "a different final byte");
	}

	/// A private marker makes it a different sequence, whatever the rest looks like.
	#[test]
	fn a_private_marker_rules_it_out() {
		assert!(scan(b"\x1b[?#{").is_empty());
		assert!(scan(b"\x1b[>1#{").is_empty());
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to
	/// carry across a boundary drawn anywhere — including between the ESC and the `[`.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut stack = SgrStack::default();
		assert!(stack.feed(b"\x1b").is_empty());
		assert!(stack.feed(b"[1").is_empty());
		assert!(stack.feed(b";4").is_empty());
		// The offset is into THIS chunk, which is where the interruption advance uses it.
		assert_eq!(
			stack.feed(b"#{"),
			vec![(2, SgrStackRequest::Push(Mask::BOLD.with(Mask::UNDERLINE)))]
		);
	}

	/// A control byte inside a CSI abandons the sequence rather than extending it.
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		// The reverse of what this asserted before §106: the engine reads a mid-sequence control byte
		// through and keeps the sequence, so cmote does too, or the two disagree about the same bytes.
		assert!(!scan(b"\x1b[1\x07#{").is_empty());
		// CAN and SUB are the only two that really cancel one.
		assert!(scan(b"\x1b[1\x18#{").is_empty());
	}

	/// A hostile stream must not be able to make the scanner buffer without bound — and the two bounds
	/// answer differently on purpose, which is what this pins now that the grammar is shared (§111).
	#[test]
	fn the_two_parameter_bounds_answer_differently() {
		// More parameters than the engine's array holds: the engine ignores the whole sequence, so the
		// scanner abandons it too. Both sides ignoring the same bytes is agreement. Every field is a `1`
		// rather than empty, so abandonment is the only thing that can make the result empty.
		let bound = super::super::csi::MAX_PARAMS;
		let fits = a_list_of(bound);
		assert_eq!(
			scan(&fits),
			vec![(fits.len(), SgrStackRequest::Push(Mask::BOLD))],
			"thirty-two parameters still fit"
		);
		assert!(
			scan(&a_list_of(bound + 1)).is_empty(),
			"thirty-three do not"
		);

		// A runaway DIGIT run is clamped instead, and the sequence LIVES — because the engine saturates
		// the number rather than giving up on it. What the clamp leaves is a code xterm never defined,
		// which is ignored, so the push saves nothing and the pop that matches it restores nothing.
		let mut digits = b"\x1b[".to_vec();
		digits.extend(std::iter::repeat_n(b'1', 500));
		digits.extend_from_slice(b"#{");
		assert_eq!(
			scan(&digits),
			vec![(digits.len(), SgrStackRequest::Push(Mask::NONE))]
		);
	}

	/// RIS is read here as well, because a hard reset must not leave a remote's pens standing (§86).
	#[test]
	fn a_hard_reset_is_reported_so_the_stack_can_be_emptied() {
		assert_eq!(scan(b"\x1bc"), vec![(2, SgrStackRequest::Reset)]);
		assert_eq!(
			scan(b"\x1b[#{\x1bc"),
			vec![
				(4, SgrStackRequest::Push(Mask::ALL)),
				(6, SgrStackRequest::Reset)
			]
		);
	}

	/// And only RIS. The soft reset is a different sequence with a different published list (§72), and
	/// the near neighbours on `ESC` must not be read as a reset either.
	#[test]
	fn the_other_escape_sequences_are_not_a_reset() {
		assert!(scan(b"\x1b[!p").is_empty(), "DECSTR");
		assert!(scan(b"\x1b7").is_empty(), "DECSC");
		assert!(scan(b"\x1b#8").is_empty(), "DECALN");
		assert!(scan(b"\x1bD").is_empty(), "IND");
	}

	/// A push and its pop in one chunk, both reported, in stream order — the interruption advance walks them
	/// in the order they came, so the pen is read where the push sat and restored where the pop did.
	#[test]
	fn a_push_and_its_pop_in_one_chunk_are_both_reported() {
		assert_eq!(
			scan(b"\x1b[#{text\x1b[#}"),
			vec![
				(4, SgrStackRequest::Push(Mask::ALL)),
				(12, SgrStackRequest::Pop)
			]
		);
	}
}

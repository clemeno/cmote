// term/locator.rs — the DEC locator protocol, in the one spelling a terminal without a locator has
// (PLAN §140).
//
//   CSI Ps ; Pu ' z    DECELR  — enable locator reporting, in cells or in pixels
//   CSI Pm ' {         DECSLE  — select which locator events send a report
//   CSI Ps ' |         DECRQLP — ask where the locator is, once
//
// DEC's own mouse protocol, and the body §93 left with no row at all. Its two *status* questions
// live next door in `term/dsr.rs` and have been answered since §93 with xterm's negatives —
// `CSI ? 53 n` "No Locator" and `CSI ? 57 ; 0 n` "Cannot identify". The three sequences above are
// the protocol those two questions are ABOUT, and until §140 they reached the engine and stopped.
//
// They stop there for the reason every scanner in this directory exists. `vte`'s `csi_dispatch`
// matches on `(action, intermediates)` (`ansi.rs:1558`) and holds no arm for `('z', [b'\''])`,
// `('{', [b'\''])` or `('|', [b'\''])` — the only `z`, `{` and `|` in the file are the DEC special
// graphics glyphs at `:1243-1245`, which are another table entirely. So all three are parsed,
// logged through `unhandled!()` and discarded.
//
// ONLY ONE OF THE THREE PRODUCES ANYTHING, and the split is the whole of this module's argument.
//
// DECELR and DECSLE are SETTINGS. One arms the locator and picks the unit its coordinates are
// counted in; the other picks which button transitions send a report. Neither has a reply, and
// neither has an effect that is visible from outside a terminal that HAS a locator: with no
// locator there is nothing to arm, no coordinate to count and no transition to select. Reading them
// and recording what they said would build state whose every reader is unreachable, which is a
// second copy of the protocol's vocabulary kept only so that it can be inspected in a debugger. So
// they are read here, understood here, and deliberately left inert — and the test below pins that
// they are not mistaken for the sequence that does answer, which is the near miss that matters when
// all three wear the same `'` intermediate and differ by one final byte.
//
// DECRQLP is a QUESTION, and an unanswered question stalls its sender — `term/query.rs`'s founding
// argument, and the one §93 used to start sending the two negatives next door. The protocol supplies
// its own word for the answer. From xterm's ctlseqs, on the locator report DECRQLP asks for:
//
//     CSI Pe ; Pb ; Pr ; Pc ; Pp & w
//     Parameters are [event;button;row;column;page].
//     Valid values for the event:
//       Pe = 0  <- locator unavailable - no other parameters sent.
//
// `Pe = 0` is not cmote's invention and not a near-enough reading of somebody else's field: it is the
// event code DEC defined for exactly this state, and it is defined to carry nothing else — no
// position, no button mask, no page. So the reply states an ABSENCE and advertises nothing, which is
// the test §93 set for the two negatives beside it, met here by the same margin. Three doors onto one
// fact, and cmote now answers all three the same way instead of two of them.
//
// WHAT CTLSEQS DOES NOT SAY, recorded because the answer below is unconditional. The document gives
// `Pe = 0` its meaning but never says what xterm sends for a DECRQLP when the locator is disabled by
// DECELR, nor what it does with the sequence when built without `OPT_DEC_LOCATOR` — where, by the
// same document's account of DSR 55, it reports "No Locator". xterm's source was not read. cmote
// answers every DECRQLP alike because cmote's answer does not depend on the thing that is unknown:
// there is no locator here under any mode, so `Pe = 0` is true before DECELR, after it, and after
// DECELR 0 turns it off again. A conditional would be a state machine built to reproduce a behaviour
// nobody here has observed.
//
// WHY CMOTE DOES NOT SIMPLY BECOME A LOCATOR. It has a mouse — §9's reports go out in xterm's
// spelling, modes 1000–1006 — so "no locator" is a choice about protocols and not a fact about
// hardware, and it deserves its reason in writing.
//
// xterm holds ONE variable for this. `send_mouse_pos` is a single mode, and DEC_LOCATOR is one of its
// values beside VT200_MOUSE and ANY_EVENT_MOUSE: DECELR overwrites whatever `CSI ? 1000 h` set, and
// `CSI ? 1000 h` overwrites DECELR. The two protocols can never both be live. In cmote the xterm
// modes belong to the ENGINE — `Screen::mouse_mode` reads the engine's own mode flags — while DECELR
// would belong to cmote, and neither can clear the other without cmote becoming a second writer of
// engine state, which §71 and §73 both refused to become. A locator built here could therefore run at
// the same time as mode 1006, and a program that set both would read two protocols' reports
// interleaved where xterm sends one. That is a divergence in the byte stream a program READS, which
// is worse than an absence: an absence is something it can detect with the very question this module
// answers, and two interleaved report streams are something it cannot detect at all.
//
// The second cost is smaller and would only be paid after the first: `ui/grid.rs` gates every pointer
// event on the engine's `MouseMode`, so unsolicited reports on button transitions would need a new
// channel carrying the pointer's cell into `Terminal`, in a stack that has kept the terminal a reader
// of the byte stream and nothing else.

/// DECRQLP's final byte, the one sequence of the three that gets an answer.
///
/// The other two final bytes — DECELR's `z` and DECSLE's `{` — are named in the tests rather than
/// here, because nothing in the shipping code ever compares against them: they are not matched, and a
/// constant this module never reads would be dead code the `[lints]` rule rejects. Which is the rule
/// working: a "documentation constant" is a comment that has to be kept compiling.
const REQUEST_POSITION: u8 = b'|';

/// The intermediate byte all three of the locator sequences carry, and the thing that makes them a
/// family rather than three unrelated final bytes.
const APOSTROPHE: u8 = b'\'';

/// The DEC locator scanner (§140). Feed it every byte of shell output; it reports where each DECRQLP
/// sat, for `term/mod.rs` to answer.
///
/// The CSI grammar is [`csi::Framer`]'s (§111); what is left here is this module's own question —
/// whether a finished sequence is the locator request, as against the two locator settings that wear
/// the same intermediate and are deliberately inert.
#[derive(Debug, Default)]
pub struct Locator {
	framer: super::csi::Framer,
}

impl Locator {
	/// Scan a chunk of shell output, returning where each DECRQLP sat. Safe at any chunk boundary —
	/// the state machine carries over between calls, so a sequence may be split anywhere, even between
	/// the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the DEC-private status reports (§82)
	/// and the tab-stop reset (§74). The engine has no arm for these bytes and does nothing with them,
	/// so the side of the boundary is not about the engine's behaviour — it is about the reply landing
	/// in the buffer at the point in the stream the question was asked, so that a DECXCPR and a DECRQLP
	/// written in one breath come back in the order they were written.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<usize> {
		let mut requests = Vec::new();
		self.framer.feed(bytes, |span, csi| {
			if is_position_request(csi) {
				requests.push(span.past());
			}
		});
		requests
	}
}

/// Whether a finished sequence is DECRQLP — `CSI Ps ' |`, the intermediate required and the parameter
/// one of the two values DEC defined for it.
///
/// ctlseqs gives the parameter as "Ps = 0, 1 or omitted -> transmit a single DECLRP locator report",
/// and no other value is defined. An undefined value is a no-op rather than a guess (§54), so `CSI 2 ' |`
/// matches nothing here.
///
/// The near misses this keeps out, in the order they are easy to get wrong:
///
///   * `CSI Ps ; Pu ' z` and `CSI Pm ' {` are the family's other two members, sharing the intermediate
///     and differing only in the final byte. Matching the intermediate alone would answer all three,
///     and a DECELR answered with a locator report is a terminal claiming a locator it does not have.
///   * `CSI Ps |` with NO intermediate is DECTST-adjacent territory and not this sequence at all; the
///     intermediate is what makes a locator sequence a locator sequence.
///   * `CSI ? 0 ' |` carries a private marker DEC never defined for this final byte.
///
/// A SECOND parameter rules the sequence out rather than being ignored, the same tightening
/// `term/dsr.rs` takes for DSR: DECRQLP takes exactly one `Ps`, so `CSI 0 ; 1 ' |` is a sequence
/// cmote does not fully understand, and answering the part it recognises is the generous reading this
/// project keeps finding at the bottom of its own mistakes. That test also excludes a sub-parameter
/// for free — one `:` makes the run two fields wide, so `CSI 0 : 1 ' |` never reaches the value check.
fn is_position_request(csi: &super::csi::Csi<'_>) -> bool {
	if (csi.final_byte(), csi.marker(), csi.intermediates())
		!= (REQUEST_POSITION, None, &[APOSTROPHE][..])
	{
		return false;
	}
	// A count of 0 is `CSI ' |`, the omitted parameter ctlseqs lists beside the two written values;
	// a count of 1 is one of those two, spelled out.
	matches!((csi.param_count(), csi.param(0)), (0, _) | (1, Some(0 | 1)))
}

/// "There is no locator" — DECLRP with event code 0, the answer to every DECRQLP (§140).
///
/// `CSI 0 & w`, and the `&` intermediate is the report's own, not the `'` of the question. ctlseqs
/// defines `Pe = 0` as "locator unavailable - no other parameters sent", which is why this constant is
/// five bytes rather than a row, a column and a page cmote would have to invent — the protocol builds
/// the empty answer in, and a terminal without the equipment is what it built it for.
///
/// A constant, and it can stay one for as long as the module header's argument holds: cmote has no
/// locator under any mode, so nothing about the stream or the grid could move this answer.
pub const UNAVAILABLE: &[u8] = b"\x1b[0&w";

#[cfg(test)]
mod tests {
	use super::*;

	/// DECELR's final byte — the family member that arms a locator cmote does not have.
	const ENABLE_REPORTING: u8 = b'z';

	/// DECSLE's final byte, which picks the events a locator cmote does not have would report.
	const SELECT_EVENTS: u8 = b'{';

	/// DECEFR's final byte, the family's fourth member: the filter rectangle DECSLE 0 cancels.
	const FILTER_RECTANGLE: u8 = b'w';

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<usize> {
		Locator::default().feed(bytes)
	}

	/// One of the family's sequences, built from its final byte so the near-miss test below spells
	/// each of them once. `params` is the run between `CSI` and the `'`.
	fn sequence(params: &str, final_byte: u8) -> Vec<u8> {
		[b"\x1b[".as_slice(), params.as_bytes(), b"'", &[final_byte]].concat()
	}

	/// The sequence itself, and the offset it reports: ONE PAST the final byte.
	#[test]
	fn a_position_request_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b['|"), vec![4]);
		assert_eq!(scan(b"ab\x1b[0'|cd"), vec![7]);
	}

	/// The near miss this module is built around. All three locator sequences carry the `'`
	/// intermediate and differ by one final byte, so a scanner that matched the intermediate alone
	/// would answer a DECELR with a locator report — a terminal claiming the very equipment the reply
	/// is supposed to deny.
	#[test]
	fn the_other_locator_sequences_are_not_position_requests() {
		// DECELR, with and without its unit parameter; DECSLE, which takes a list; and DECEFR, the
		// filter rectangle DECSLE 0 cancels — the family's fourth member, which cmote reads no more
		// than the other two.
		let others = [
			("1", ENABLE_REPORTING),
			("1;2", ENABLE_REPORTING),
			("0;0", ENABLE_REPORTING),
			("0", SELECT_EVENTS),
			("1;3", SELECT_EVENTS),
			("1;1;10;10", FILTER_RECTANGLE),
		];
		for (params, final_byte) in others {
			let bytes = sequence(params, final_byte);
			assert!(
				scan(&bytes).is_empty(),
				"CSI {params} ' {} must not be read as a position request",
				char::from(final_byte)
			);
		}
		// The same parameters under DECRQLP's own final byte DO match where the shape allows it, so
		// the loop above is testing the final byte and not merely rejecting everything.
		assert_eq!(scan(&sequence("1", REQUEST_POSITION)).len(), 1);
	}

	/// ctlseqs defines `Ps = 0`, `1` and omitted, and nothing else. An undefined value is a no-op
	/// rather than a guess (§54), and the match is on the whole number rather than a prefix of it —
	/// the same rule `term/dsr.rs` keeps for its allow-list.
	#[test]
	fn only_the_parameters_decrqlp_defines_are_answered() {
		assert_eq!(scan(b"\x1b['|").len(), 1, "omitted");
		assert_eq!(scan(b"\x1b[0'|").len(), 1);
		assert_eq!(scan(b"\x1b[1'|").len(), 1);
		assert!(scan(b"\x1b[2'|").is_empty(), "DEC defined no 2");
		assert!(scan(b"\x1b[10'|").is_empty(), "not a prefix match");
		assert!(scan(b"\x1b[99999'|").is_empty());
	}

	/// DECRQLP takes exactly one `Ps`, so a second parameter means a sequence this scanner does not
	/// fully understand — left alone rather than half-answered, as `term/dsr.rs` leaves `CSI ? 6 ; n`.
	/// An EMPTY second parameter is still a second one, which is the distinction `param_count` exists
	/// to keep (§111).
	#[test]
	fn a_second_parameter_rules_it_out() {
		assert!(scan(b"\x1b[0;1'|").is_empty());
		assert!(scan(b"\x1b[0;'|").is_empty());
		// A sub-parameter is excluded by the same count test, without a rule of its own.
		assert!(scan(b"\x1b[0:1'|").is_empty());
	}

	/// The intermediate is what makes this a locator sequence at all. Without it the final byte is
	/// somebody else's, and cmote must not answer it.
	#[test]
	fn the_apostrophe_intermediate_is_required() {
		assert!(scan(b"\x1b[0|").is_empty());
		assert!(
			scan(b"\x1b[0 '|").is_empty(),
			"a second intermediate is not one"
		);
		assert!(scan(b"\x1b[0''|").is_empty());
	}

	/// DEC defined no private marker for this final byte, so a sequence carrying one is a different
	/// sequence — the near-miss rule §56 wrote down, tested on all four markers the framer accepts.
	#[test]
	fn a_private_marker_rules_it_out() {
		for marker in *b"?<=>" {
			let request = [b"\x1b[".as_slice(), &[marker], b"0'|"].concat();
			assert!(
				scan(&request).is_empty(),
				"CSI {} 0 ' | must not be answered",
				char::from(marker)
			);
		}
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to carry
	/// across a boundary drawn anywhere — including between the ESC and the `[`, and between the
	/// parameter and the intermediate.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut locator = Locator::default();
		assert!(locator.feed(b"\x1b").is_empty());
		assert!(locator.feed(b"[").is_empty());
		assert!(locator.feed(b"0").is_empty());
		assert!(locator.feed(b"'").is_empty());
		// The offset is into THIS chunk, which is where the interruption advance uses it.
		assert_eq!(locator.feed(b"|"), vec![1]);
	}

	/// A control byte inside a CSI is run where it sits and the sequence carries on around it, which is
	/// what the engine does — so a scanner that gave up here would leave the engine to read a sequence
	/// cmote never judged (§106). CAN and SUB are the two that really do cancel.
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert!(!scan(b"\x1b[0\x07'|").is_empty(), "BEL is read through");
		assert!(scan(b"\x1b[0\x18'|").is_empty(), "CAN cancels");
		assert!(scan(b"\x1b[0\x1a'|").is_empty(), "SUB cancels");
	}

	/// Two in one chunk, both reported, in stream order — the interruption advance walks them in the
	/// order they came, so the replies land in the buffer in the order the questions were written.
	#[test]
	fn two_requests_in_one_chunk_are_both_reported() {
		assert_eq!(scan(b"\x1b[0'|\x1b[1'|"), vec![5, 10]);
	}

	/// The reply is the protocol's own word for this, quoted in the module header: event code 0,
	/// "locator unavailable", and the report is defined to carry nothing after it. The `&` is the
	/// REPORT's intermediate and not the `'` of the question, which is the one byte easy to copy across
	/// from the sequence being answered.
	#[test]
	fn the_reply_is_the_protocols_own_word_for_no_locator() {
		assert_eq!(UNAVAILABLE, b"\x1b[0&w");
		let (event, intermediate) = (UNAVAILABLE[2], UNAVAILABLE[3]);
		assert_eq!(event, b'0', "Pe = 0, locator unavailable");
		assert_eq!(intermediate, b'&', "DECLRP's intermediate, not DECRQLP's");
		assert_ne!(intermediate, APOSTROPHE);
	}

	/// The answer cmote gives here has to agree with the two it has been giving next door since §93,
	/// because they are three doors onto one fact. `dsr.rs` says "No Locator" and "Cannot identify";
	/// this says "locator unavailable". If a later hand ever teaches cmote a real locator, this test is
	/// where the three are forced to move together.
	#[test]
	fn the_three_doors_onto_the_locator_agree() {
		assert_eq!(super::super::dsr::NO_LOCATOR, b"\x1b[?53n");
		assert_eq!(super::super::dsr::NO_LOCATOR_TYPE, b"\x1b[?57;0n");
		assert_eq!(UNAVAILABLE, b"\x1b[0&w");
	}
}

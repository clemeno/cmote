// term/window.rs — the window-op questions the engine drops, and the one cmote answers (PLAN §147).
//
// `CSI Ps t` is xterm's window-operation family: a single final byte carrying two dozen unrelated
// verbs, half of them commands to the window manager and half of them questions about the window.
// `vte` dispatches exactly four of them (`ansi.rs:1739-1745`):
//
//   14 -> text_area_size_pixels      18 -> text_area_size_chars
//   22 -> push_title                 23 -> pop_title
//
// and sends every other value to `unhandled!()`. So the four above are the engine's, answered inside
// it and drained through the reply listener, and everything else falls on the floor — which is this
// directory's usual division and the reason this module is a scanner beside the stream rather than an
// arm on the gate.
//
// FOUR QUESTIONS FALL HERE. Only one is answered, and the split is the whole of this module:
//
//   11  window state         "are you iconified?"        -> CSI 1 t / CSI 2 t
//   13  window position      "where are you?"            -> CSI 3 ; x ; y t
//   15  screen size, pixels  "how big is the display?"   -> CSI 5 ; height ; width t
//   16  CELL size, pixels    "how big is one cell?"      -> CSI 6 ; height ; width t
//   19  screen size, chars   "and in characters?"        -> CSI 9 ; height ; width t
//
// **16 is a fact about the terminal. The other four are facts about the person using it.**
//
// That is §36's rule, and it decides all five rows without a second argument. A cell's pixel size is
// a property of the font cmote renders with; a program asks it to size a picture (§41) or to place a
// sixel exactly, and the answer says nothing about the machine. Where the window sits on the desktop,
// whether the user has minimised it, and how large their display is are none of the remote's business
// — they change under a mouse the remote cannot see, they differ per monitor, and together they
// fingerprint a desktop for the cost of five bytes on a wire the user never looks at.
//
// THE SECOND ARGUMENT, WHICH IS CMOTE'S OWN, and it applies to 11 alone. Even setting the privacy
// question aside, "is the window iconified?" HAS NO TRUE ANSWER HERE. cmote's window holds tabs and
// splits (§19, §101), and this terminal is one pane of one tab. A pane in a background tab is not
// iconified and is just as invisible as one in a minimised window, so `CSI 1 t` — the reply that says
// "I am on screen" — would be wrong for a case the question does not even have a code for. A wrong
// answer to a question about visibility is worse than no answer, because a program that gets no reply
// falls back to drawing anyway and a program that gets `CSI 1 t` may stop.
//
// The refusal costs the sender a stall, as every refusal in this directory does, and the tests below
// pin all four so the allow-list cannot widen by accident.
//
// WHY THE ANSWER IS AN INTERRUPTION RATHER THAN A REPLY AFTER THE CHUNK. Every other query cmote
// sniffs and answers from a FIXED fact — its version, its unit id, its graphics limits, its displayed
// extent — is answered after the chunk, on the argument §144 wrote down: the answer cannot go stale
// between the question and the reply, so the point in the stream does not matter. The cell size is a
// fixed fact by that test too. It is answered mid-stream anyway, because of something none of the
// others has: **a sibling the ENGINE answers**.
//
// `CSI 14 t` and `CSI 16 t` are the pair a program uses to work out a cell — 14 gives the text area in
// pixels, 18 gives it in cells, 16 gives the quotient directly — and they are routinely written in one
// breath. The engine answers 14 the instant it parses it, into the reply buffer; a 16 answered after
// the chunk would land after it whatever order they were asked in. A program that sends `CSI 16 t`
// then `CSI 14 t` and reads two replies by position would then assign each to the other. So this one
// question is answered where it sat, and the two spellings interleave exactly as they were written.

/// The window-op parameter cmote answers: `CSI 16 t`, "how big is one character cell in pixels?".
///
/// The four it refuses — 11, 13, 15 and 19 — are named in the tests rather than here, for the reason
/// `term/locator.rs` gives: a constant this module never compares against is dead code the `[lints]`
/// rule rejects, and a "documentation constant" is a comment that has to be kept compiling.
const CELL_SIZE: u16 = 16;

/// The reply code for a cell-size report. xterm's answer to `CSI 16 t` is `CSI 6 ; height ; width t`,
/// and the `6` is the report's own code rather than an echo of the question's `16`.
const CELL_SIZE_REPORT: u16 = 6;

/// The window-report scanner (§147). Feed it every byte of shell output; it reports where each
/// `CSI 16 t` sat, for `term/mod.rs` to answer from the cell size the GUI measured.
///
/// The CSI grammar is [`super::csi::Framer`]'s (§111); what is left here is this module's own
/// question — which of the two dozen verbs on this final byte a finished sequence carries, and
/// whether it is the one cmote will answer.
#[derive(Debug, Default)]
pub struct WindowReports {
	framer: super::csi::Framer,
}

impl WindowReports {
	/// Scan a chunk of shell output, returning where each cell-size request sat. Safe at any chunk
	/// boundary — the state machine carries over between calls, so a sequence may be split anywhere,
	/// even between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the DEC-private status reports (§82) and
	/// the locator question (§140). The engine has no arm for this parameter and does nothing with it,
	/// so the side of the boundary is not about the engine's behaviour — it is about the reply landing
	/// in the buffer at the point the question was asked, which is the whole reason this is an
	/// interruption at all (see the module header).
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<usize> {
		let mut requests = Vec::new();
		self.framer.feed(bytes, |span, csi| {
			if is_cell_size_request(csi) {
				requests.push(span.past());
			}
		});
		requests
	}
}

/// Whether a finished sequence is `CSI 16 t` — the cell-size question, and no other window op.
///
/// The near misses this keeps out, in the order they are easy to get wrong:
///
///   * **The engine's own four.** `CSI 14 t`, `CSI 18 t`, `CSI 22 t` and `CSI 23 t` are dispatched by
///     `vte` and answered inside `alacritty_terminal`. A scanner that matched the final byte alone
///     would answer 14 a SECOND time, and a program reading two replies to one question parses the
///     second as input.
///   * **The four cmote refuses.** 11, 13, 15 and 19 wear the same final byte and are the module
///     header's argument; matching loosely here is exactly how they would leak.
///   * **The commands.** `CSI 1 t` through `CSI 10 t` iconify, move, resize, raise and fullscreen the
///     window, and several of them take further parameters (`CSI 4 ; height ; width t`). They are
///     part 6's refusal and reach nothing here either way.
///
/// A SECOND parameter rules the sequence out rather than being ignored, the tightening `term/dsr.rs`
/// and `term/locator.rs` both take: xterm defines 16 with no parameters after it, so `CSI 16 ; 2 t` is
/// a sequence cmote does not fully understand, and answering the part it recognises is the generous
/// reading this project keeps finding at the bottom of its own mistakes. It also excludes a
/// sub-parameter for free — one `:` makes the run two fields wide.
///
/// An OMITTED parameter is not this sequence either. `CSI t` defaults to 1, which is de-iconify, a
/// command and not a question.
fn is_cell_size_request(csi: &super::csi::Csi<'_>) -> bool {
	if (csi.marker(), csi.intermediates(), csi.final_byte()) != (None, &[][..], b't') {
		return false;
	}
	(csi.param_count(), csi.param(0)) == (1, Some(CELL_SIZE))
}

/// The cell-size report: `CSI 6 ; height ; width t` (§147).
///
/// **Height before width**, which is xterm's order on this reply and the opposite of the order the
/// arguments read in — so the two are named rather than positional at every call site that builds one.
///
/// The numbers are the ones the GUI measured and handed to `Terminal::set_cell_pixels`, which is the
/// same pair the engine multiplies for its own `CSI 14 t` answer. One source, two spellings: a program
/// that asks both questions is told two things that cannot disagree.
///
/// Before the GUI has measured, both are zero and the report says zero — the same answer `CSI 14 t`
/// gives in that window, and the truth about a terminal that does not yet know how large its cells
/// are. The measurement happens before any output arrives, so that window is a test's concern rather
/// than a session's.
pub fn cell_size_reply(width: u16, height: u16) -> Vec<u8> {
	format!("\x1b[{CELL_SIZE_REPORT};{height};{width}t").into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<usize> {
		WindowReports::default().feed(bytes)
	}

	/// One window op, built from its parameter run, so the near-miss tests spell each verb once.
	fn window_op(params: &str) -> Vec<u8> {
		[b"\x1b[".as_slice(), params.as_bytes(), b"t"].concat()
	}

	/// The sequence itself, and the offset it reports: ONE PAST the final byte.
	#[test]
	fn a_cell_size_request_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b[16t"), vec![5]);
		assert_eq!(scan(b"ab\x1b[16tcd"), vec![7]);
	}

	/// The four questions on this final byte that the ENGINE answers. A scanner that matched the final
	/// byte alone would answer `CSI 14 t` a second time, and a program reading two replies to one
	/// question parses the second as input.
	#[test]
	fn the_window_ops_the_engine_answers_are_left_to_it() {
		for params in ["14", "18", "22", "23"] {
			assert!(
				scan(&window_op(params)).is_empty(),
				"CSI {params} t is the engine's"
			);
		}
	}

	/// The four cmote refuses, on §36's rule and — for 11 — on a question a tabbed terminal has no true
	/// answer to. This test IS the refusal: nothing else in the shipping code mentions these numbers, so
	/// widening the allow-list by accident would show up here and nowhere else.
	#[test]
	fn the_questions_about_the_user_are_not_answered() {
		/// `Ps = 11`, "are you iconified?" — which a pane in a background tab cannot answer.
		const WINDOW_STATE: &str = "11";
		/// `Ps = 13`, "where does your window sit on the desktop?".
		const WINDOW_POSITION: &str = "13";
		/// `Ps = 15`, "how large is the display, in pixels?".
		const SCREEN_PIXELS: &str = "15";
		/// `Ps = 19`, "and in characters?".
		const SCREEN_CHARS: &str = "19";
		for params in [WINDOW_STATE, WINDOW_POSITION, SCREEN_PIXELS, SCREEN_CHARS] {
			assert!(
				scan(&window_op(params)).is_empty(),
				"CSI {params} t names the user, not the terminal"
			);
		}
		// The text-area spelling of the position question, which carries a second parameter and would
		// leak the same fact by another door.
		assert!(scan(&window_op("13;2")).is_empty());
	}

	/// The ten commands, which are part 6's refusal and are not questions at all. Several take further
	/// parameters, which is the shape the count test below is really about.
	#[test]
	fn the_window_commands_are_not_cell_size_requests() {
		for params in ["1", "2", "3;100;200", "4;480;640", "5", "9;1", "10;2"] {
			assert!(scan(&window_op(params)).is_empty());
		}
	}

	/// xterm defines 16 with nothing after it, so a second parameter means a sequence cmote does not
	/// fully understand — left alone rather than half-answered. An EMPTY second parameter is still a
	/// second one, which is the distinction `param_count` exists to keep (§111).
	#[test]
	fn a_second_parameter_rules_it_out() {
		assert!(scan(&window_op("16;2")).is_empty());
		assert!(scan(&window_op("16;")).is_empty());
		// A sub-parameter is excluded by the same count test, without a rule of its own.
		assert!(scan(&window_op("16:2")).is_empty());
	}

	/// An omitted parameter defaults to 1 — de-iconify, a command — so a bare `CSI t` is not this
	/// question. And the match is on the whole number rather than a prefix of it.
	#[test]
	fn only_the_whole_number_matches() {
		assert!(scan(&window_op("")).is_empty());
		assert!(scan(&window_op("1")).is_empty());
		assert!(scan(&window_op("6")).is_empty());
		assert!(scan(&window_op("160")).is_empty());
		assert!(scan(&window_op("116")).is_empty());
		// A padded 16 is still 16: the framer drops leading zeros rather than counting them (§111).
		assert_eq!(scan(&window_op("0016")).len(), 1);
	}

	/// DEC and xterm define no private marker and no intermediate for this final byte, so a sequence
	/// carrying either is a different sequence — the near-miss rule §56 wrote down.
	#[test]
	fn a_marker_or_an_intermediate_rules_it_out() {
		for marker in *b"?<=>" {
			let request = [b"\x1b[".as_slice(), &[marker], b"16t"].concat();
			assert!(
				scan(&request).is_empty(),
				"CSI {} 16 t must not be answered",
				char::from(marker)
			);
		}
		assert!(scan(b"\x1b[16 t").is_empty());
		assert!(scan(b"\x1b[16$t").is_empty());
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to carry
	/// across a boundary drawn anywhere — including between the ESC and the `[`, and inside the number.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut reports = WindowReports::default();
		assert!(reports.feed(b"\x1b").is_empty());
		assert!(reports.feed(b"[1").is_empty());
		assert!(reports.feed(b"6").is_empty());
		// The offset is into THIS chunk, which is where the interruption advance uses it.
		assert_eq!(reports.feed(b"t"), vec![1]);
	}

	/// A control byte inside a CSI is run where it sits and the sequence carries on around it, which is
	/// what the engine does — so a scanner that gave up here would leave the engine to read a sequence
	/// cmote never judged (§106). CAN and SUB are the two that really do cancel.
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert!(!scan(b"\x1b[16\x07t").is_empty(), "BEL is read through");
		assert!(scan(b"\x1b[16\x18t").is_empty(), "CAN cancels");
		assert!(scan(b"\x1b[16\x1at").is_empty(), "SUB cancels");
	}

	/// Two in one chunk, both reported, in stream order — the interruption advance walks them in the
	/// order they came, so the replies land in the buffer in the order the questions were written.
	#[test]
	fn two_requests_in_one_chunk_are_both_reported() {
		assert_eq!(scan(b"\x1b[16t\x1b[16t"), vec![5, 10]);
	}

	/// The reply, and the one thing easy to get backwards about it: xterm answers HEIGHT first, which
	/// is the opposite of the order the arguments read in.
	#[test]
	fn the_reply_states_the_height_before_the_width() {
		assert_eq!(cell_size_reply(9, 20), b"\x1b[6;20;9t".to_vec());
		assert_eq!(cell_size_reply(7, 15), b"\x1b[6;15;7t".to_vec());
	}

	/// Before the GUI has measured, the answer is zero rather than a guess — the same thing `CSI 14 t`
	/// says in that window, since both are built from the one pair `set_cell_pixels` writes.
	#[test]
	fn an_unmeasured_cell_reports_zero() {
		assert_eq!(cell_size_reply(0, 0), b"\x1b[6;0;0t".to_vec());
	}
}

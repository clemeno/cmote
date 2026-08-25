// term/presentation.rs — DECRQPSR, the two reports that describe how the page is being presented
// (PLAN §143).
//
//   CSI 1 $ w    DECRQPSR → DECCIR,   the cursor information report
//   CSI 2 $ w    DECRQPSR → DECTABSR, the tab stop report
//
// `vte`'s `csi_dispatch` matches on `(action, intermediates)` and holds no arm for `('w', [b'$'])` —
// the only `$` intermediates it knows are DECRQM's `p`, DECCARA's `r`, DECRARA's `t`, DECCRA's `v`
// and the erase family's `x` / `z` / `{`. So the sequence is parsed, logged through `unhandled!()`
// and discarded, which is the same door every scanner in this directory came through.
//
// WHAT DEC DEFINES, read off the VT510 manual rather than remembered:
//
//   DECRQPSR's Ps is 0 "Error. Request ignored", 1 for DECCIR, 2 for DECTABSR. There is no
//   "I do not report that" form — DECRQSS has one (§66) and this family does not, so an
//   unrecognised request is answered with SILENCE and that is the standard's own answer rather
//   than cmote declining to speak. Which settles the question the audit row left open.
//
//   DECCIR is `DCS 1 $ u Pr ; Pc ; Pp ; Srend ; Satt ; Sflag ; Pgl ; Pgr ; Scss ; Sdesig ST`.
//   Pr, Pc and Pp are "the number of the line the cursor is on", the column, and the page. The four
//   S-fields are single characters carrying bit flags, all built the same way: bit 8 always 0, bit 7
//   always 1, bit 6 an extension indicator, then the flags. Counting bit 1 as the low one, that is a
//   base of 0x40 with the flags added on. Pgl and Pgr name which of G0–G3 is invoked in each half.
//   Scss says which of the four slots hold a 96-character set. Sdesig is "a string of intermediate
//   and final characters indicating the character sets designated as G0 through G3".
//
//   DECTABSR is `DCS 2 $ u D...D ST`, where the data string is the column of each tab stop, and the
//   manual's own example separates them with `/`: `DCS 2 $ u 9/17/25/33/41/49/57/65/73 ST`.
//
// WHERE EVERY FIELD COMES FROM, since a report that quietly invents one of ten fields is worse than
// no report at all (§54, §60):
//
//   * Pr and Pc are the cursor DECXCPR reports (§82), one-based, and ABSOLUTE — ignoring origin mode,
//     which is the convention §74 and §82 settled for cmote's other cursor report. One cursor, one
//     convention, and Sflag carries DECOM so a reader that cares can tell.
//   * Pp is 1. cmote is a one-page terminal — DECRQCRA's page parameter is ignored for the same
//     reason (§60) and the page-positioning sequences have nowhere to go.
//   * Srend, Satt and Sflag are read from the engine's pen and modes and from cmote's own charset
//     state, one flag at a time; see [`cursor_report`].
//   * Pgl, Pgr and Sdesig are `term/charset.rs`'s, which is why that module and this one arrived
//     together: GR is written by three sequences and read by nothing else in the program, so a
//     terminal that answered `Pgr` from a constant while accepting LS2R would be reporting a state it
//     had refused to keep.
//   * The tab stops are `term/tabs.rs`'s mirror, because the engine's own table is private (§143).
//
// THE ONE FLAG THAT CANNOT BE TRUE. Srend's bit 3 is blink, and the engine's per-cell flag word has
// no bit for it: the fifteen it names cover inverse, bold, italic, dim, hidden, strikeout, five
// underline styles and the wide-character marks, and nothing blinks (§59). So the bit is always 0
// here — the same honest hole DECRQCRA's checksum has for the same attribute, and the same answer
// DECRQSS gives when it reports a blinking cursor shape as its steady twin (§123).

use super::charset::{Charsets, SLOTS};

/// DECRQPSR's intermediate byte, and the thing that tells it from `CSI Ps " w` — which is DECRPDE,
/// the DISPLAYED-EXTENT report, one intermediate away (§144).
const DOLLAR: u8 = b'$';

/// DECRQPSR's final byte.
const REQUEST: u8 = b'w';

/// The base every one of DECCIR's four flag characters is built on: bit 8 clear, bit 7 set.
///
/// Bit 6, the extension indicator, stays clear in all four — it means "another byte follows", and no
/// field here has more flags than fit in one.
const FLAG_BASE: u8 = 0x40;

/// Which report a DECRQPSR asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationRequest {
	/// `CSI 1 $ w` — DECCIR, the cursor information report.
	Cursor,
	/// `CSI 2 $ w` — DECTABSR, the tab stop report.
	TabStops,
}

/// The DECRQPSR scanner (§143). Feed it every byte of shell output; it reports where each request sat
/// and which of the two reports it asked for.
///
/// The CSI grammar is [`super::csi::Framer`]'s (§111); what is left here is this module's own
/// question — which of the family's parameters was written, and whether it is one DEC defined.
#[derive(Debug, Default)]
pub struct Presentation {
	framer: super::csi::Framer,
}

impl Presentation {
	/// Scan a chunk of shell output, returning each request and where it sat. Safe at any chunk
	/// boundary — the state machine carries over between calls, so a sequence may be split anywhere,
	/// even between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like DECXCPR's (§82). DECCIR reports the
	/// CURSOR, so the answer is only true where the question sat: answered after the chunk it would
	/// describe where the cursor ended up, which is the defect §82 built its split advance to avoid.
	/// DECTABSR would read the same either way and rides along for the reason the locator's answer
	/// does (§140) — one route through one scanner is easier to keep right than two, and the ordering
	/// against the other reports then falls out for free.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, PresentationRequest)> {
		let mut requests = Vec::new();
		self.framer.feed(bytes, |span, csi| {
			if let Some(request) = classify(csi) {
				requests.push((span.past(), request));
			}
		});
		requests
	}
}

/// Which report a finished sequence asks for, or `None` if it is not a DECRQPSR cmote answers.
///
/// The near misses this keeps out, in the order they are easy to get wrong:
///
///   * `CSI Ps " w` is DECRPDE, the reply half of DECRQDE (§144) — the same final byte under a
///     different intermediate, and cmote SENDS that one, so reading it as a question would have cmote
///     answering its own report.
///   * `CSI Ps w` with no intermediate at all is not this sequence, and the intermediate is what
///     makes it one.
///   * `CSI 0 $ w` is DEC's own "Error. Request ignored", and an omitted parameter is 0 — so a bare
///     `CSI $ w` is that same error, answered with the same silence.
///   * A SECOND parameter rules the sequence out rather than being ignored, the tightening
///     `term/dsr.rs` and `term/locator.rs` both take: DECRQPSR takes exactly one `Ps`, so
///     `CSI 1 ; 2 $ w` is a sequence cmote does not fully understand, and answering the part it
///     recognises is the generous reading this project keeps finding at the bottom of its own
///     mistakes. That test excludes a sub-parameter for free.
fn classify(csi: &super::csi::Csi<'_>) -> Option<PresentationRequest> {
	if (csi.final_byte(), csi.marker(), csi.intermediates()) != (REQUEST, None, &[DOLLAR][..]) {
		return None;
	}
	if csi.param_count() != 1 {
		return None;
	}
	match csi.param(0)? {
		1 => Some(PresentationRequest::Cursor),
		2 => Some(PresentationRequest::TabStops),
		_ => None,
	}
}

/// The visual attributes DECCIR reports, which is exactly its `Srend` byte.
///
/// Grouped as DEC groups them rather than spread across [`CursorState`], and the grouping is not
/// cosmetic: six `bool`s in one struct is six positional fields a caller can transpose in silence, and
/// splitting them the way the REPORT splits them means each group is checked against one manual table
/// instead of a reader having to hold all three at once.
#[derive(Debug, Clone, Copy)]
pub struct Rendition {
	pub bold: bool,
	pub underline: bool,
	pub reverse: bool,
}

/// The two flags DECCIR's `Sflag` byte carries that are not the single shifts — those come from the
/// character-set state, which is the only thing that knows one is pending.
#[derive(Debug, Clone, Copy)]
pub struct Modes {
	/// DECOM.
	pub origin: bool,
	/// Whether a wrap is owed: the cursor has filled the last column and the next glyph starts a new
	/// line. Both halves of it — the engine's `input_needs_wrap` and the margins' own deferred wrap
	/// (§102) — because with a right margin set it is cmote holding the flag and not the engine.
	pub pending_wrap: bool,
}

/// Everything DECCIR reports that is not the character sets — read from the engine by the caller, so
/// that the report itself stays a pure function of its inputs and can be tested without a terminal.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
	/// The cursor's row and column, ZERO-based as the engine counts them; the report adds the one.
	pub row: usize,
	pub column: usize,
	/// `Srend`.
	pub rendition: Rendition,
	/// `Satt` — DECSCA, whether the pen is arming the cells it writes against a selective erase (§56).
	pub protected: bool,
	/// `Sflag`, minus the single shifts.
	pub modes: Modes,
}

/// DECCIR — `DCS 1 $ u … ST`, the cursor and everything the page is currently being written with.
pub fn cursor_report(state: CursorState, charsets: &Charsets) -> Vec<u8> {
	let rendition = FLAG_BASE
		| u8::from(state.rendition.bold)
		| (u8::from(state.rendition.underline) << 1)
		// Bit 3 is blink, and it is always clear: the engine has no flag for it (§59).
		| (u8::from(state.rendition.reverse) << 3);
	let attributes = FLAG_BASE | u8::from(state.protected);
	let single_shift = charsets.pending_single_shift();
	let flags = FLAG_BASE
		| u8::from(state.modes.origin)
		| (u8::from(single_shift == Some(2)) << 1)
		| (u8::from(single_shift == Some(3)) << 2)
		| (u8::from(state.modes.pending_wrap) << 3);
	// Every slot cmote can designate holds a 94-character set: the twelve national sets are 94-column
	// sets by definition, ASCII and DEC line drawing are too, and every 96-character set is one of the
	// designations `term/charset.rs` refuses. So the size bits are clear, and they will stay clear for
	// as long as that refusal holds — which is why this is a constant with a reason rather than a loop
	// over four slots that can only ever answer the same thing.
	let sizes = FLAG_BASE;
	let designations: String = (0..SLOTS)
		.map(|slot| charsets.designated(slot).designation())
		.collect();
	format!(
		"\x1bP1$u{};{};1;{};{};{};{};{};{};{}\x1b\\",
		state.row + 1,
		state.column + 1,
		char::from(rendition),
		char::from(attributes),
		char::from(flags),
		charsets.gl(),
		charsets.gr(),
		char::from(sizes),
		designations,
	)
	.into_bytes()
}

/// DECTABSR — `DCS 2 $ u … ST`, the column of every tab stop, one-based and `/`-separated.
///
/// `stops` arrives zero-based, as the engine and the mirror both count columns; the report is
/// one-based, which is what the manual's own example shows (its first stop is 9 on a terminal whose
/// power-on stops are every eight columns from the left edge).
///
/// A page with NO stops on it reports an empty data string rather than nothing at all. That is the
/// truthful answer — a program has just cleared them all with `CSI 3 g` and is entitled to hear so —
/// and it keeps a sender from waiting out a timeout to learn it, which is `term/query.rs`'s founding
/// argument.
pub fn tab_stop_report(stops: impl Iterator<Item = usize>) -> Vec<u8> {
	let columns: Vec<String> = stops.map(|column| (column + 1).to_string()).collect();
	format!("\x1bP2$u{}\x1b\\", columns.join("/")).into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::charset::{Charset, CharsetRequest, Charsets, Designations};

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<(usize, PresentationRequest)> {
		Presentation::default().feed(bytes)
	}

	/// A cursor at the home corner with nothing set — the baseline every field test moves one item of.
	fn plain() -> CursorState {
		CursorState {
			row: 0,
			column: 0,
			rendition: Rendition {
				bold: false,
				underline: false,
				reverse: false,
			},
			protected: false,
			modes: Modes {
				origin: false,
				pending_wrap: false,
			},
		}
	}

	/// The baseline with one or more visual attributes set — the three that DECCIR's `Srend` carries.
	fn with_rendition(bold: bool, underline: bool, reverse: bool) -> CursorState {
		CursorState {
			rendition: Rendition {
				bold,
				underline,
				reverse,
			},
			..plain()
		}
	}

	/// The report as a string, for tests that read its fields.
	fn report(state: CursorState, charsets: &Charsets) -> String {
		String::from_utf8(cursor_report(state, charsets)).expect("the report is ASCII")
	}

	/// Field `index` of a DECCIR, counting from 0 at Pr — the payload between `DCS 1 $ u` and the ST.
	fn field(report: &str, index: usize) -> String {
		report
			.trim_start_matches("\x1bP1$u")
			.trim_end_matches("\x1b\\")
			.split(';')
			.nth(index)
			.expect("the report has ten fields")
			.to_owned()
	}

	#[test]
	fn a_request_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b[1$w"), vec![(5, PresentationRequest::Cursor)]);
		assert_eq!(
			scan(b"ab\x1b[2$w"),
			vec![(7, PresentationRequest::TabStops)]
		);
	}

	/// DEC defines 0 as "Error. Request ignored" and nothing past 2 at all, and an omitted parameter
	/// is 0 — so three shapes of request are answered with the same silence.
	#[test]
	fn only_the_two_reports_dec_defines_are_answered() {
		assert!(scan(b"\x1b[0$w").is_empty(), "DEC's own error value");
		assert!(scan(b"\x1b[$w").is_empty(), "omitted is 0");
		assert!(scan(b"\x1b[3$w").is_empty());
		assert!(scan(b"\x1b[12$w").is_empty(), "not a prefix match");
	}

	/// The near miss this module is built around: cmote SENDS `CSI Ps " w` as DECRPDE (§144), so
	/// reading the family on its final byte alone would have cmote answering its own report.
	#[test]
	fn the_displayed_extent_report_is_not_a_request() {
		assert!(scan(b"\x1b[1\"w").is_empty(), "DECRPDE's intermediate");
		assert!(scan(b"\x1b[1w").is_empty(), "no intermediate at all");
		assert!(scan(b"\x1b[1$ w").is_empty(), "a second intermediate");
	}

	/// DECRQPSR takes exactly one `Ps`, so a second parameter means a sequence this scanner does not
	/// fully understand — left alone rather than half-answered. An EMPTY second parameter is still a
	/// second one, which is the distinction `param_count` exists to keep (§111).
	#[test]
	fn a_second_parameter_rules_it_out() {
		assert!(scan(b"\x1b[1;2$w").is_empty());
		assert!(scan(b"\x1b[1;$w").is_empty());
		assert!(
			scan(b"\x1b[1:2$w").is_empty(),
			"and a sub-parameter with it"
		);
	}

	#[test]
	fn a_private_marker_rules_it_out() {
		for marker in *b"?<=>" {
			let request = [b"\x1b[".as_slice(), &[marker], b"1$w"].concat();
			assert!(scan(&request).is_empty());
		}
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scanner = Presentation::default();
		assert!(scanner.feed(b"\x1b[1").is_empty());
		assert!(scanner.feed(b"$").is_empty());
		assert_eq!(scanner.feed(b"w"), vec![(1, PresentationRequest::Cursor)]);
	}

	/// The whole report for a terminal that has just started: the cursor home, nothing set, four ASCII
	/// slots, G0 in GL and G1 in GR. Spelled out once, because the shape of the envelope and the order
	/// of the ten fields is what everything below reads a piece of.
	#[test]
	fn a_fresh_terminal_reports_its_defaults() {
		let charsets = Charsets::default();
		assert_eq!(
			report(plain(), &charsets),
			"\x1bP1$u1;1;1;@;@;@;0;1;@;BBBB\x1b\\"
		);
	}

	/// The cursor is one-based in the report and zero-based everywhere in the program, which is the
	/// off-by-one worth a test of its own.
	#[test]
	fn the_cursor_is_reported_one_based() {
		let charsets = Charsets::default();
		let state = CursorState {
			row: 4,
			column: 9,
			..plain()
		};
		let report = report(state, &charsets);
		assert_eq!(field(&report, 0), "5");
		assert_eq!(field(&report, 1), "10");
		assert_eq!(field(&report, 2), "1", "cmote is a one-page terminal");
	}

	/// Each rendition flag on its own, against DEC's bit order: bold is bit 1, underline bit 2, blink
	/// bit 3 and reverse bit 4, over a base of 0x40.
	#[test]
	fn each_rendition_flag_lands_on_its_own_bit() {
		let charsets = Charsets::default();
		let bold = with_rendition(true, false, false);
		let underline = with_rendition(false, true, false);
		let reverse = with_rendition(false, false, true);
		assert_eq!(field(&report(bold, &charsets), 3), "A", "0x40 | 1");
		assert_eq!(field(&report(underline, &charsets), 3), "B", "0x40 | 2");
		assert_eq!(field(&report(reverse, &charsets), 3), "H", "0x40 | 8");
		let all = with_rendition(true, true, true);
		assert_eq!(field(&report(all, &charsets), 3), "K", "0x40 | 1 | 2 | 8");
	}

	/// Blink is bit 3 and the engine has no flag for it (§59), so the bit can never be set. Asserted
	/// rather than left in prose, because "always 0" is the kind of claim that quietly stops being
	/// true when somebody adds a blink flag and forgets this report.
	#[test]
	fn the_blink_bit_is_never_set() {
		let charsets = Charsets::default();
		for state in [plain(), with_rendition(true, true, true)] {
			let rendition = field(&report(state, &charsets), 3).as_bytes()[0];
			assert_eq!(rendition & 0b100, 0, "bit 3 is blink");
		}
	}

	#[test]
	fn selective_erase_is_the_only_attribute_bit() {
		let charsets = Charsets::default();
		let protected = CursorState {
			protected: true,
			..plain()
		};
		assert_eq!(field(&report(protected, &charsets), 4), "A");
		assert_eq!(field(&report(plain(), &charsets), 4), "@");
	}

	/// Sflag's four bits: origin mode, SS2 pending, SS3 pending, and a wrap owed.
	#[test]
	fn each_flag_bit_lands_where_dec_puts_it() {
		let charsets = Charsets::default();
		let origin = CursorState {
			modes: Modes {
				origin: true,
				pending_wrap: false,
			},
			..plain()
		};
		let wrapping = CursorState {
			modes: Modes {
				origin: false,
				pending_wrap: true,
			},
			..plain()
		};
		assert_eq!(field(&report(origin, &charsets), 5), "A", "0x40 | 1");
		assert_eq!(field(&report(wrapping, &charsets), 5), "H", "0x40 | 8");
		let mut shifted = Charsets::default();
		shifted.single_shift(2);
		assert_eq!(field(&report(plain(), &shifted), 5), "B", "SS2 is bit 2");
		let mut shifted = Charsets::default();
		shifted.single_shift(3);
		assert_eq!(field(&report(plain(), &shifted), 5), "D", "SS3 is bit 3");
	}

	/// The three charset fields, which is the whole reason this module and `term/charset.rs` arrived
	/// together: GR is written by LS1R / LS2R / LS3R and read by nothing else in the program.
	#[test]
	fn the_charset_fields_report_what_is_designated_and_invoked() {
		let mut charsets = Charsets::default();
		charsets.designate(1, Charset::LineDrawing);
		charsets.lock(1, false);
		charsets.lock(2, true);
		let report = report(plain(), &charsets);
		assert_eq!(field(&report, 6), "1", "Pgl — G1 is in GL");
		assert_eq!(field(&report, 7), "2", "Pgr — G2 is in GR");
		assert_eq!(field(&report, 9), "B0BB", "Sdesig, G0 through G3");
	}

	/// Sdesig reports the SPELLING that was designated, which is why a set with two finals keeps both
	/// in `term/charset.rs` rather than being canonicalised to one.
	#[test]
	fn a_multi_byte_designation_survives_into_the_report() {
		// Driven through the scanner rather than built by hand, so the test reads the same path the
		// stream does — `ESC ( % 6` is Portugal, and its two-byte spelling is what has to come back.
		let mut charsets = Charsets::default();
		let found = Designations::default().feed(b"\x1b(%6");
		let CharsetRequest::Designate { slot, charset } = found[0].1 else {
			panic!("ESC ( % 6 designates a character set");
		};
		charsets.designate(slot, charset);
		assert_eq!(field(&report(plain(), &charsets), 9), "%6BBB");
	}

	/// Every slot cmote can designate holds a 94-character set, so the size bits are clear — and stay
	/// clear only for as long as that is true.
	#[test]
	fn every_designated_set_is_ninety_four_columns_wide() {
		let charsets = Charsets::default();
		assert_eq!(field(&report(plain(), &charsets), 8), "@");
	}

	/// The manual's own example, reproduced: a terminal whose stops are every eight columns from the
	/// left edge reports 9, 17, 25 and so on — which is the zero-based mirror read one-based.
	#[test]
	fn the_tab_report_matches_the_manuals_example() {
		let stops = (0..80).step_by(8);
		assert_eq!(
			tab_stop_report(stops),
			b"\x1bP2$u1/9/17/25/33/41/49/57/65/73\x1b\\".to_vec()
		);
	}

	/// A page with every stop cleared reports an empty data string rather than nothing at all: the
	/// program asked, the answer is "none", and silence would leave it waiting out a timeout to learn
	/// the same thing (§93).
	#[test]
	fn a_page_with_no_stops_still_answers() {
		assert_eq!(
			tab_stop_report(std::iter::empty()),
			b"\x1bP2$u\x1b\\".to_vec()
		);
	}
}

// term/dsr.rs — the DEC-private device status reports (PLAN §82).
//
//   CSI ? 6 n    DECXCPR — "where is the cursor?", in DEC's own spelling
//
// DSR comes in two grammars. The ANSI one (`CSI Ps n`) is the engine's: `vte` dispatches it to
// `device_status`, which answers `5` with `CSI 0 n` and `6` with `CSI <row> ; <col> R`. The DEC one
// puts a `?` in front, and `vte` has no arm for it at all — its CSI table holds `('n', [])` and no
// `('n', [b'?'])` — so every sequence in this file reached the engine and stopped there.
//
// That is §72's and §74's shape once more: a sequence with nothing behind it, in a terminal that
// already holds the answer. The row read ❌ from §67 to §82 for a reason that turned out not to be
// one — "answering it would mean inventing a page number cmote does not have". xterm's own ctlseqs
// says otherwise:
//
//     Ps = 6  =>  Report Cursor Position (DECXCPR).  The response [row;column] is returned as
//     CSI ? r ; c R  (assumes the default page, i.e., "1").
//
// No page is sent. The reply is the two numbers cmote already reports through the ANSI spelling, with
// a `?` in front of them — so the cost that kept this row shut was never being charged.
//
// WHY THE ALLOW-LIST. `CSI ? Ps n` is a family, and xterm answers nine more values of it: printer
// status (15), UDK lock (25), keyboard status (26), locator availability and type (55 / 56),
// macro space (62), a memory checksum (63), data integrity (75) and multi-session configuration (85).
// Cmote answers THREE of them — DECXCPR and the two locator questions (§93) — and the other seven
// are refused rather than unimplemented, because a reply is an advertisement (§71) and every one of
// those seven advertises a machine rather than a page:
//
//   * 15 / 25 / 62 / 63 / 75 / 85 describe hardware cmote does not have — a printer, a user-defined-key
//     store, a macro store, that store's checksum, a data-integrity check, a multi-session controller.
//     "Ready", "unlocked" or a byte count would each be a claim about equipment that is not there.
//   * 26 reports the KEYBOARD's language — xterm answers `CSI ? 27 ; 1 ; 0 ; 0 n` for a North American
//     one, the nationality sitting in the first parameter (§84). §36 fixed the rule this would break:
//     cmote's identity replies name the program and never the user's machine, which is why DA3 answers a
//     constant unit id rather than a serial number. A remote must not learn the layout in front of
//     the person at the other end off a query they never see.
//
// TWO OF THE NINE CAME BACK OFF THAT LIST IN §93, and the line they crossed is worth stating.
// `55` and `56` ask whether a DEC locator is present and what type it is — a pointing-device
// protocol cmote does not implement. xterm's answers for a terminal without one are `CSI ? 53 n`
// ("no locator") and `CSI ? 57 ; 0 n` ("cannot identify"), and those two advertise NOTHING: they
// state an absence, which is the one thing a terminal lacking the equipment can say truthfully.
// That is the whole test the other seven fail — "printer ready" or a macro-space byte count is a
// claim about hardware that is not there, while "there is no locator" is a claim about hardware that
// is not there being not there. The same shape as DECRQM's honest "not recognised" for mode 69,
// which this project already prefers to silence, and it removes a sender left waiting out a timeout.
//
// The refusal is cmote's own, in the same construction `term/iterm.rs` uses for OSC 1337 keys,
// `term/pointer.rs` for pointer shapes and `link.rs` for URI schemes: the scanner reads the whole
// parameter, compares it against a list of one, and a value that is not on the list produces nothing
// at all. The engine sees those bytes afterwards and drops them, as it always did.
//
// WHY THIS IS SPLIT-FED. A cursor report is only true where it sits. `term/query.rs` collects its
// queries and answers them once the chunk has been advanced, which is right for XTVERSION and DA3 —
// constants — and wrong for a position: a chunk carrying `CSI ? 6 n` followed by more output would
// report where the cursor ENDED UP, not where the question was asked. So this scanner reports offsets
// and `term/mod.rs` answers inside its split loop, reading the cursor with the engine advanced exactly
// to the sequence, and pushing the reply into the same buffer the engine's own replies land in
// (§60's DECRQCRA does this already). A DSR and a DECXCPR asked for in one write therefore come back
// in the order they were asked.

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// DECXCPR, the cursor position — the parameter this module was built for (§82).
const CURSOR_POSITION: u16 = 6;

/// "Is there a locator?" (§93). Answered with the honest negative below.
const LOCATOR_STATUS: u16 = 55;

/// "What kind of locator?" (§93). Answered with the other honest negative.
const LOCATOR_TYPE: u16 = 56;

/// The longest parameter run buffered inside one sequence. DECXCPR's is a single digit; anything
/// longer is malformed, and refusing to grow past this keeps a hostile stream from ballooning our
/// memory (§12).
const MAX_PARAMS: usize = 32;

/// The most intermediate bytes buffered. DECXCPR has none at all — they are collected only so that a
/// near miss carrying one is rejected rather than mistaken for it.
const MAX_INTERMEDIATES: usize = 4;

/// Which of the DEC-private status reports cmote answers (§82, §93).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
	/// `CSI ? 6 n` — DECXCPR, where the cursor is. Answered from the live cursor, which is why this
	/// whole module is split-fed.
	CursorPosition,
	/// `CSI ? 55 n` — is a locator available? Answered "no", which is a constant.
	LocatorStatus,
	/// `CSI ? 56 n` — what kind of locator? Answered "cannot identify", also a constant.
	LocatorType,
}

/// Where the scanner is in the byte stream. Only the CSI shape matters here: `ESC [`, then parameter
/// bytes, then intermediate bytes, then one final byte.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC. A CSI starts if the next byte is `[`, and nothing else here is of interest.
	Escape,
	/// Inside `ESC [ …`, collecting the sequence until its final byte.
	Csi,
}

/// The DEC-private DSR scanner (§82). Feed it every byte of shell output; it reports where each
/// cursor-position request sat, for `term/mod.rs` to answer from the live cursor.
#[derive(Debug, Default)]
pub struct Dsr {
	state: Scan,
	/// The private marker, if the sequence opened with one. DECXCPR requires `?`, and keeping the
	/// marker apart from `params` lets the digits parse the same either way.
	marker: Option<u8>,
	params: Vec<u8>,
	intermediates: Vec<u8>,
}

impl Dsr {
	/// Scan a chunk of shell output, returning where each DECXCPR sat. Safe at any chunk boundary —
	/// the state machine carries over between calls, so a sequence may be split anywhere, even between
	/// the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the rectangles (§58) and the tab-stop
	/// reset (§74). The engine has no arm for these bytes and will do nothing with them, so the side
	/// of the boundary is not about the engine's behaviour — it is about the cursor being read once
	/// the whole sequence is behind it, so that a request cannot report a position from the middle of
	/// its own final byte.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Request)> {
		let mut requests = Vec::new();
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Escape;
					}
				}
				Scan::Escape => match byte {
					b'[' => {
						self.marker = None;
						self.params.clear();
						self.intermediates.clear();
						self.state = Scan::Csi;
					}
					// ESC ESC: still waiting for the sequence's real first byte.
					ESC => {}
					_ => self.state = Scan::Text,
				},
				Scan::Csi => match byte {
					// Parameter bytes: digits and separators, plus the private markers (`< = > ?`,
					// 0x3c–0x3f) which are only legal as the very first one.
					0x30..=0x3f => {
						if self.params.is_empty() && self.marker.is_none() && byte >= 0x3c {
							self.marker = Some(byte);
						} else {
							self.params.push(byte);
							if self.params.len() > MAX_PARAMS {
								self.state = Scan::Text;
							}
						}
					}
					// Intermediate bytes. DECXCPR has none, so any of these rules it out.
					0x20..=0x2f => {
						self.intermediates.push(byte);
						if self.intermediates.len() > MAX_INTERMEDIATES {
							self.state = Scan::Text;
						}
					}
					// The final byte ends the sequence, so this is where it is judged.
					0x40..=0x7e => {
						if let Some(request) = self.request(byte) {
							requests.push((index + 1, request));
						}
						self.state = Scan::Text;
					}
					// A fresh ESC restarts the match.
					ESC => self.state = Scan::Escape,
					// A C0 control byte or DEL inside a CSI: malformed, so drop the sequence rather
					// than let a stray byte extend it indefinitely.
					_ => self.state = Scan::Text,
				},
			}
		}
		requests
	}

	/// Which of the three questions cmote answers this is, or `None` for the rest of the family.
	///
	/// `CSI ? Ps n` requires the marker and one parameter. The near misses this keeps out: `CSI 6 n`
	/// without the marker is the ANSI spelling, which is the ENGINE's and is answered there (answering
	/// it here as well would give the program two replies to one question); `CSI > 6 n` carries a
	/// marker DEC never defined for this final byte; and every other `Ps` is the allow-list doing its
	/// job.
	///
	/// A second parameter rules the sequence out rather than being ignored, which is a deliberate
	/// tightening over `term/tabs.rs`'s reading of DECST8C. DSR takes exactly one `Ps`, so `CSI ? 6 ; 1 n`
	/// is a sequence cmote does not fully understand — and answering the part it recognises would be
	/// the generous reading this project keeps finding at the bottom of its own mistakes.
	fn request(&self, final_byte: u8) -> Option<Request> {
		if (final_byte, self.marker, self.intermediates.as_slice()) != (b'n', Some(b'?'), &[][..]) {
			return None;
		}
		match self.only_param()? {
			CURSOR_POSITION => Some(Request::CursorPosition),
			LOCATOR_STATUS => Some(Request::LocatorStatus),
			LOCATOR_TYPE => Some(Request::LocatorType),
			_ => None,
		}
	}

	/// The sole parameter as a number. `None` when there is more than one, when the digits are
	/// unparseable, or when there are none at all — an omitted `Ps` is 0 to `vte`, and 0 is not a
	/// request cmote answers, so treating "absent" as "not ours" and "unreadable" as "not ours" costs
	/// nothing and keeps one shape of answer for both.
	fn only_param(&self) -> Option<u16> {
		if self.params.is_empty() || self.params.contains(&b';') {
			return None;
		}
		let mut value: u16 = 0;
		for &byte in &self.params {
			let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
			value = value.checked_mul(10)?.checked_add(u16::from(digit))?;
		}
		Some(value)
	}
}

/// The DECXCPR reply for a cursor at `row` / `col`, both **zero-based** as the engine holds them.
///
/// `CSI ? <row> ; <col> R` — xterm's two-parameter form, quoted in the module header, with no page
/// number. The arithmetic is deliberately the same `+ 1` the engine does for the ANSI spelling
/// (`device_status`, `alacritty_terminal/src/term/mod.rs:1332`), and that is the whole reason this is
/// a function of two numbers rather than a reader of terminal state: cmote is a second READER of the
/// cursor here, never a second source for it, so the two spellings of one question cannot come to
/// disagree (§71, §73).
///
/// One consequence rides along and is disclosed rather than corrected: the engine reports the cursor's
/// ABSOLUTE row, ignoring origin mode, where DEC defines both spellings as reporting a position
/// relative to the scrolling region when DECOM is set. Copying the engine's arithmetic copies that
/// too. Correcting it here would make cmote's answer disagree with the engine's for the same cursor —
/// two spellings of one question, answering differently, which is worse than one shared divergence a
/// row can name (§74 made the same call for the movement sequences).
pub fn cursor_reply(row: u16, col: u16) -> Vec<u8> {
	format!("\x1b[?{};{}R", row + 1, col + 1).into_bytes()
}

/// "There is no locator" — xterm's `CSI ? 53 n`, the answer to `CSI ? 55 n` (§93).
///
/// This and the one below are the two replies §82 disclosed and did not send. They are the exception
/// that proves the rule the rest of the family is refused under: **a reply is an advertisement**
/// (§71), and these advertise nothing. They state an ABSENCE, which is the one thing a terminal
/// without the equipment can say truthfully — the same shape as DECRQM's honest "not recognised" for
/// mode 69, which this project already prefers to silence.
///
/// The alternative was silence, and silence is not neutral: a program that asks whether a locator is
/// there and hears nothing cannot tell that from a terminal still thinking about it, so it waits out
/// its own timeout before deciding. `query.rs`'s founding argument, applied where §82 said it applied
/// and then did not act.
pub const NO_LOCATOR: &[u8] = b"\x1b[?53n";

/// "Cannot identify the locator" — xterm's `CSI ? 57 ; 0 n`, the answer to `CSI ? 56 n` (§93).
///
/// The type question's negative. xterm answers `CSI ? 57 ; 1 n` for a mouse; the `0` says there is
/// nothing to describe, which is true here and says nothing about the machine.
pub const NO_LOCATOR_TYPE: &[u8] = b"\x1b[?57;0n";

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<(usize, Request)> {
		Dsr::default().feed(bytes)
	}

	/// Just the offsets, for the tests that are about WHERE a sequence was found rather than which
	/// question it carried.
	fn offsets(bytes: &[u8]) -> Vec<usize> {
		scan(bytes).into_iter().map(|(offset, _)| offset).collect()
	}

	/// The sequence itself, and the offset it reports: ONE PAST the final byte.
	#[test]
	fn a_cursor_position_request_is_found_just_past_its_final_byte() {
		assert_eq!(offsets(b"\x1b[?6n"), vec![5]);
		assert_eq!(offsets(b"ab\x1b[?6ncd"), vec![7]);
	}

	/// The allow-list is one value wide, and it is matched on the whole number rather than a prefix of
	/// it — `66` and `60` are not `6`, the same rule `term/notify.rs` keeps for OSC 9's sub-codes.
	#[test]
	fn only_the_parameter_six_is_answered() {
		assert!(scan(b"\x1b[?n").is_empty(), "an omitted parameter is not 6");
		assert!(scan(b"\x1b[?0n").is_empty());
		assert!(scan(b"\x1b[?5n").is_empty(), "DSR 5 is the ANSI form's own");
		assert!(scan(b"\x1b[?60n").is_empty(), "not a prefix match");
		assert!(scan(b"\x1b[?66n").is_empty());
	}

	/// The two locator questions, answered with xterm's own negatives (§93). Both are constants, so
	/// what this pins is that the scanner tells them apart and that neither is mistaken for the other.
	#[test]
	fn the_locator_questions_get_their_honest_negatives() {
		assert_eq!(scan(b"\x1b[?55n"), vec![(6, Request::LocatorStatus)]);
		assert_eq!(scan(b"\x1b[?56n"), vec![(6, Request::LocatorType)]);
		assert_eq!(NO_LOCATOR, b"\x1b[?53n", "xterm's 'no locator'");
		assert_eq!(NO_LOCATOR_TYPE, b"\x1b[?57;0n", "and its 'cannot identify'");
	}

	/// The seven values xterm answers that cmote refuses by name — the module header's argument, pinned so
	/// a later hand cannot start advertising a printer, a keyboard nationality or a macro store by
	/// widening this list without a test going red.
	#[test]
	fn the_status_reports_that_would_speak_for_the_machine_are_refused() {
		for parameter in [15, 25, 26, 62, 63, 75, 85] {
			let request = format!("\x1b[?{parameter}n");
			assert!(
				scan(request.as_bytes()).is_empty(),
				"CSI ? {parameter} n must not be answered"
			);
		}
	}

	/// Without the private marker this is the ANSI spelling, which the engine answers itself. Reading
	/// it here as well would put two replies on the wire for one question.
	#[test]
	fn the_private_marker_is_required() {
		assert!(scan(b"\x1b[6n").is_empty());
		assert!(
			scan(b"\x1b[>6n").is_empty(),
			"a different marker is a different sequence"
		);
	}

	/// DSR carries exactly one `Ps`, so a second parameter means something this scanner does not
	/// understand — and an unrecognised sequence is left alone rather than half-answered.
	#[test]
	fn a_second_parameter_rules_it_out() {
		assert!(scan(b"\x1b[?6;1n").is_empty());
		assert!(scan(b"\x1b[?6;n").is_empty());
	}

	/// An intermediate byte makes it something else again, so the match tests all three of final byte,
	/// marker and intermediates — the near-miss rule §56 wrote down.
	#[test]
	fn an_intermediate_byte_rules_it_out() {
		assert!(scan(b"\x1b[?6 n").is_empty());
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to carry
	/// across a boundary drawn anywhere — including between the ESC and the `[`.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut dsr = Dsr::default();
		assert!(dsr.feed(b"\x1b").is_empty());
		assert!(dsr.feed(b"[?").is_empty());
		assert!(dsr.feed(b"6").is_empty());
		// The offset is into THIS chunk, which is where the split advance uses it.
		assert_eq!(dsr.feed(b"n"), vec![(1, Request::CursorPosition)]);
	}

	/// A control byte inside a CSI abandons the sequence rather than extending it, so the `n` that
	/// follows is not read as this sequence's final byte.
	#[test]
	fn a_control_byte_abandons_the_sequence() {
		assert!(scan(b"\x1b[?6\x07n").is_empty());
	}

	/// A hostile stream must not be able to make the scanner buffer without bound.
	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		let mut bytes = b"\x1b[?".to_vec();
		bytes.extend(std::iter::repeat_n(b'6', MAX_PARAMS + 10));
		bytes.push(b'n');
		assert!(scan(&bytes).is_empty());
	}

	/// Two in one chunk, both reported, in stream order — the split advance walks them in the order
	/// they came, so each is answered from the cursor as it stood at its own sequence.
	#[test]
	fn two_requests_in_one_chunk_are_both_reported() {
		assert_eq!(offsets(b"\x1b[?6n\x1b[?6n"), vec![5, 10]);
	}

	/// xterm's two-parameter form, one-based, with no page number — the quoted reply in the module
	/// header, and the same arithmetic the engine does for the ANSI spelling.
	#[test]
	fn the_reply_is_xterms_two_parameter_form() {
		assert_eq!(cursor_reply(0, 0), b"\x1b[?1;1R".to_vec());
		assert_eq!(cursor_reply(3, 4), b"\x1b[?4;5R".to_vec());
		assert_eq!(cursor_reply(23, 79), b"\x1b[?24;80R".to_vec());
	}
}

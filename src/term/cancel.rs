// term/cancel.rs — the sequences the engine would read as something ELSE (PLAN §57).
//
// Every other scanner under `term/` is here because the engine DROPS something: an OSC it ignores,
// a query it has no arm for, an erase it never learned. This one is here for the opposite reason.
// The engine has an arm, the arm matches, and what it does is wrong — so the sequence has to be
// stopped before it reaches the dispatch, not read out beside it.
//
// There is exactly one such sequence today, and it is a spelling collision:
//
//   CSI s           SCOSC — save the cursor position (the ANSI.SYS spelling, universal)
//   CSI Pl ; Pr s   DECSLRM — set the LEFT and RIGHT margins (VT420)
//
// Two unrelated meanings on one final byte. A real VT420 tells them apart by a mode: DECSLRM only
// means margins once DECLRMM (`CSI ? 69 h`) has been set, and means save-cursor otherwise.
//
// **§102 gave cmote that mode, and with it the real rule.** Until then the engine refused mode 69
// outright — it is not in `NamedPrivateMode`, so DECSET dropped it and DECRQM answered `0`, "not
// recognised" — and cmote had no margins to give either. So the only evidence left in the bytes was
// the parameter count, and this scanner cancelled EVERY parametrised `s` on it. That guess is now
// retired. The scanner reports the sequence and its two numbers; `Terminal::process` decides:
//
//   mode 69 SET      DECSLRM. The margins are applied and the byte is still cancelled, because the
//                    engine's arm for it would save the cursor on the way past.
//   mode 69 RESET    SCOSC. The byte is let through and the engine saves the cursor, which is what
//                    a real xterm does with it, parameters and all.
//
// Letting the byte through is not a loosening of §57. The harm §57 closed was a margin request
// costing a program its saved cursor, and the terminfo is the proof it cannot happen now: all four
// of `xterm-256color`'s margin capabilities SET MODE 69 FIRST
// (`smglr=\E[?69h\E[%i%p1%d;%p2%ds`). A program that means margins says so before it asks.
//
// The problem is the program that does not ask. `vte`'s dispatch is
//
//   ('s', []) => handler.save_cursor_position()     // vte-0.15.0/src/ansi.rs:1737
//
// which does not look at its parameters at all, so `CSI 5;70 s` **saves the cursor**. That is worse
// than doing nothing: the engine keeps ONE saved-cursor slot, shared by `CSI s` and `ESC 7`, so an
// unasked-for save overwrites whatever the program had put there. The program's own later `CSI u`
// then restores to wherever the margin request happened to sit — a cursor jump that surfaces far
// away from its cause, in the shape "my status-line update moved the cursor".
//
// So the sequence is cancelled. `mod.rs` splits its advance at the final byte, feeds the engine a
// CAN in place of it, and steps over it.
//
// Feeding NOTHING in place of it would be the bug and not the fix. The parameter bytes have already
// reached the engine's parser, which is sitting in its CSI-parameter state waiting for a final
// byte — so the next one to come along would be taken as this sequence's. `CSI 5;70 s` followed by
// the word `hello` would dispatch `('h', [])` with parameters 5 and 70: set mode 5, set mode 70,
// print `ello`. The sequence has to be ENDED, and CAN is how the ANSI state machine ends one:
//
//   0x18 in the CSI-parameter state -> `anywhere()` -> `execute(0x18)`, state = Ground
//                                      (vte-0.15.0/src/lib.rs)
//
// no dispatch, and `execute` has no arm for CAN, so it is a `debug!` line and nothing else. The
// parser's parameter buffer is left holding the digits, which is harmless: the next ESC clears it
// (`reset_params` on the way into the escape state).
//
// CAN rather than SUB (0x1a), which takes the same transition: SUB is *defined* to be displayable —
// a terminal may print a substitute glyph for it — and this engine happening to ignore it today is
// not a promise. CAN rather than a final byte the engine has no arm for (`('p', [])`, say), because
// that would rest on the absence of an arm, which is exactly the kind of thing a version bump adds.
// CAN is a cancel in the state machine itself.
//
// What this scanner still decides on its own is which sequences are CANDIDATES, and there the test
// is unchanged: a parametrised, unmarked, unintermediated `s`. The bare `CSI s` — every save-cursor
// in the wild — never reaches the decision at all.

/// The byte fed to the engine in place of a final byte cmote refuses to let it dispatch.
///
/// CAN, "cancel" — the ANSI state machine's own way to abandon a sequence in flight (see the module
/// header for why this and not one of the alternatives).
pub const CANCEL: u8 = 0x18;

/// One DECSLRM found in the stream (§57, §102).
///
/// The offset is the position of the final byte ITSELF, and the two numbers are what the sequence
/// asked for — `None` where the parameter was omitted, which DEC reads as the page edge.
///
/// The scanner reports the sequence; it does not decide what happens to it. That decision needs the
/// margin mode, which lives with the rest of the margin state, so `Terminal::process` makes it: with
/// mode 69 set this is a margin request and the byte is cancelled, and without it the byte is a
/// save-cursor and is let through (`term/margins.rs` argues why that is the right split).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CancelRequest {
	pub offset: usize,
	pub left: Option<u16>,
	pub right: Option<u16>,
}

/// The most parameters DECSLRM carries. Everything past them is somebody else's sequence — kept as a
/// named number rather than a literal `2` because the test below reads better when the bound has the
/// reason attached.
const KEPT_PARAMS: usize = 2;

/// The misparse scanner (§57). Feed it every byte of shell output; it reports the offset of each
/// final byte the engine must not be allowed to dispatch.
///
/// One field, because the CSI grammar is [`csi::Framer`]'s (§111) and the shape test is the whole of
/// what this module decides. It kept the most hand-rolled state of the ten before the move — a
/// parameter counter, a slot index, two half-read numbers and a `plain` flag — and every one of them
/// was a re-derivation of something the framer now reports.
#[derive(Debug, Default)]
pub struct Cancel {
	framer: super::csi::Framer,
}

impl Cancel {
	/// Scan a chunk of shell output, returning each DECSLRM it carried. Safe at any chunk boundary:
	/// the state machine carries over between calls, so a sequence may be split anywhere, even
	/// between the ESC and the `[`.
	///
	/// Each offset is the position of the final byte ITSELF — a third convention, next to the start
	/// of the sequence that a prompt mark reports (§34) and the byte one PAST it that a selective
	/// erase reports (§56). It is the byte that may be replaced, so it is the byte named: the engine
	/// is advanced up to it, fed a CAN instead of it, and resumed after it.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<CancelRequest> {
		let mut cancels = Vec::new();
		self.framer.feed(bytes, |offset, csi| {
			if let Some(request) = margins(offset, csi) {
				cancels.push(request);
			}
		});
		cancels
	}
}

/// The DECSLRM a finished sequence is, or `None` when its final byte is not one to refuse.
///
/// Four things rule it out, and the first three are the near-miss rule §56 wrote down: a private
/// marker makes it XTSAVE (`CSI ? Pm s`, which the engine drops harmlessly), an intermediate makes it
/// some other sequence entirely, and a sub-parameter is a spelling DECSLRM does not have. The fourth
/// is the count — one or two parameters, no more and no fewer. None at all is the bare `CSI s`, every
/// save-cursor in the wild, and three is somebody else's sequence ending in `s`.
///
/// **What is NOT a reason to give up is length.** This module used to say that in a note on a
/// deliberately unbounded counter, and the framer is where the argument belongs now: a long digit run
/// is CLAMPED and the sequence lives, because the engine saturates the number and dispatches anyway.
/// Abandoning would mean the final byte was never judged, and then the engine reads a padded DECSLRM
/// as a save-cursor — the margins silently not set AND a cursor the program never asked to save
/// overwritten, which is §57's harm exactly. A separator run past the engine's array is different:
/// there the engine drops the sequence too, so abandoning is agreement, and it also fails the count
/// test twice over.
fn margins(offset: usize, csi: &super::csi::Csi<'_>) -> Option<CancelRequest> {
	if csi.final_byte() != b's'
		|| csi.marker().is_some()
		|| !csi.intermediates().is_empty()
		|| csi.sub_parameters()
	{
		return None;
	}
	if !(1..=KEPT_PARAMS).contains(&csi.param_count()) {
		return None;
	}
	Some(CancelRequest {
		// The framer names the byte one PAST the final one, which is what the eight scanners that FEED
		// the engine need. This is the one that REPLACES a byte, so it names the final byte itself —
		// the third convention the doc above describes, derived from the framer's rather than tracked.
		offset: offset - 1,
		// `None` is an omitted parameter and `Some(0)` an explicit zero, kept apart because it is the
		// READER that decides the two mean the same thing (§102, `term/margins.rs`). That distinction
		// is `Params::finish`'s doing (§111) — before it, a written zero rendered as nothing at all.
		left: csi.param(0),
		right: csi.param(1),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read back where it found a DECSLRM. The numbers the
	/// sequence carried have their own tests below; most of these are about which sequences are
	/// candidates at all.
	fn scan(bytes: &[u8]) -> Vec<usize> {
		let mut cancel = Cancel::default();
		cancel
			.feed(bytes)
			.into_iter()
			.map(|request| request.offset)
			.collect()
	}

	/// Feed one byte slice and read back the two numbers of the single request it carried.
	fn numbers(bytes: &[u8]) -> (Option<u16>, Option<u16>) {
		let mut cancel = Cancel::default();
		let found = cancel.feed(bytes);
		assert_eq!(found.len(), 1, "expected exactly one request");
		(found[0].left, found[0].right)
	}

	#[test]
	fn a_margin_request_is_cancelled() {
		// `\x1b[5;70s` is 7 bytes and the `s` is the last of them, so the offset is 6.
		assert_eq!(scan(b"\x1b[5;70s"), vec![6]);
	}

	#[test]
	fn one_parameter_is_enough_to_make_it_a_margin_request() {
		// DECSLRM's second parameter defaults to the right edge, so `CSI 5 s` is a margin request
		// too — and one that would save the cursor just as silently.
		assert_eq!(scan(b"\x1b[5s"), vec![3]);
	}

	#[test]
	fn a_bare_save_cursor_is_left_alone() {
		// The universal spelling, and the one meaning cmote keeps. Every save-cursor in the wild is
		// this one, so cancelling it would break far more than the collision does.
		assert!(scan(b"\x1b[s").is_empty());
	}

	#[test]
	fn a_restore_cursor_is_left_alone() {
		assert!(scan(b"\x1b[u").is_empty());
	}

	#[test]
	fn saving_the_private_modes_is_left_alone() {
		// `CSI ? Pm s` is XTSAVE and `CSI ? Pm r` XTRESTORE. Both carry parameters and end in a byte
		// this scanner watches, and neither is DECSLRM — the marker is what says so. The engine has
		// no arm for either, so they are already dropped without harm.
		assert!(scan(b"\x1b[?1049s\x1b[?1049r").is_empty());
	}

	#[test]
	fn an_intermediate_byte_rules_out_a_margin_request() {
		// DECSLRM has no intermediate, so `CSI 1 SP s` is some other sequence and not ours to touch.
		assert!(scan(b"\x1b[1 s").is_empty());
	}

	#[test]
	fn the_scroll_region_is_left_alone() {
		// DECSTBM (`CSI Pt;Pb r`) is the vertical margins, which the engine DOES implement. Sharing
		// the parametrised shape with DECSLRM is exactly why the final byte is checked.
		assert!(scan(b"\x1b[1;24r").is_empty());
	}

	#[test]
	fn ordinary_output_asks_for_nothing() {
		// The common case has to cost nothing: no cancel means `process` never splits its advance.
		assert!(scan(b"\x1b[31mred\x1b[0m\x1b[2J\x1b[1;1Hhello").is_empty());
	}

	#[test]
	fn a_letter_s_in_plain_text_is_not_a_sequence() {
		assert!(scan(b"save this string").is_empty());
	}

	#[test]
	fn scroll_up_is_not_confused_with_it() {
		// SU is the capital `S`; the final byte is case-sensitive.
		assert!(scan(b"\x1b[5S").is_empty());
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_caught() {
		// Output arrives in arbitrary chunks, including between the ESC and the `[`, and the engine's
		// own parser carries the head across the boundary the same way.
		let mut cancel = Cancel::default();
		assert!(cancel.feed(b"text\x1b").is_empty());
		assert!(cancel.feed(b"[5;7").is_empty());
		// The offset counts from the start of the chunk that completed the sequence — and the numbers
		// survive the boundary, since `70` was written across it.
		let found = cancel.feed(b"0shello");
		assert_eq!(found.len(), 1);
		assert_eq!(found[0].offset, 1);
		assert_eq!((found[0].left, found[0].right), (Some(5), Some(70)));
	}

	#[test]
	fn the_two_numbers_come_out_as_the_sequence_wrote_them() {
		assert_eq!(numbers(b"\x1b[5;70s"), (Some(5), Some(70)));
	}

	#[test]
	fn an_omitted_parameter_is_not_the_same_as_a_zero() {
		// The reader treats both as "the page edge" (§102), but that is the READER's decision, so the
		// scanner keeps them apart and hands over what was actually written.
		assert_eq!(numbers(b"\x1b[5s"), (Some(5), None));
		assert_eq!(numbers(b"\x1b[;70s"), (None, Some(70)));
		assert_eq!(numbers(b"\x1b[0;70s"), (Some(0), Some(70)));
	}

	#[test]
	fn a_third_parameter_rules_the_sequence_out() {
		// DECSLRM has two. Three means some other sequence ending in `s`, and cancelling it would
		// take a save-cursor a program is entitled to.
		assert!(scan(b"\x1b[1;2;3s").is_empty());
	}

	#[test]
	fn a_sub_parameter_rules_the_sequence_out() {
		// Colons introduce sub-parameters, which DECSLRM does not have.
		assert!(scan(b"\x1b[1:2s").is_empty());
	}

	#[test]
	fn a_run_of_digits_saturates_instead_of_wrapping() {
		// A margin past the page is clamped by the reader; one that WRAPPED round would be a small
		// plausible number and would be obeyed. Six nines is already past `u16`.
		assert_eq!(numbers(b"\x1b[999999;5s"), (Some(u16::MAX), Some(5)));
	}

	#[test]
	fn two_margin_requests_in_one_chunk_come_out_in_order() {
		// Ascending offsets, because `process` merges them with the other scanners' events and
		// advances the engine forwards only.
		assert_eq!(scan(b"\x1b[5;70s\x1b[1;80s"), vec![6, 13]);
	}

	#[test]
	fn a_long_parameter_run_is_still_judged_because_the_engine_judges_it() {
		// The engine counts PARAMETERS, and it never abandons a sequence over the length of a digit
		// run: `vte` saturates the number and dispatches anyway (its params are a fixed array of 32,
		// and a digit is folded in with `saturating_mul`). So a DECSLRM padded with leading zeros still
		// reaches `('s', [])` there — and a scanner that had given up on it would leave the engine to
		// read it as a save-cursor, which is the exact harm §57 exists to prevent: the margins silently
		// not set, AND a cursor the program never asked to save overwritten.
		let mut margins = vec![b'\x1b', b'['];
		margins.extend(std::iter::repeat_n(b'0', 40));
		margins.extend_from_slice(b"1;80s");
		assert_eq!(numbers(&margins), (Some(1), Some(80)));
	}

	#[test]
	fn a_runaway_digit_run_saturates_rather_than_abandoning_the_sequence() {
		// What §12 asks of this scanner is that a hostile stream cannot grow its state, and a digit run
		// cannot: there is no buffer here, only two numbers and two counters. So the answer to a runaway
		// run is the engine's own answer — saturate — and NOT a drop, because the engine will not drop
		// it and the two have to agree about what this byte stream was.
		let mut params = vec![b'\x1b', b'['];
		params.extend(std::iter::repeat_n(b'9', 500));
		params.push(b's');
		assert_eq!(numbers(&params), (Some(u16::MAX), None));
	}

	/// The count test at its exact edge, which the shared grammar makes checkable: two parameters is
	/// DECSLRM and three is not, and the sequence in between is the one a hand-rolled slot index used
	/// to get right by counting separators instead of parameters (§111).
	#[test]
	fn the_count_test_holds_at_its_own_edge() {
		assert_eq!(scan(b"\x1b[1;80s"), vec![6], "two is DECSLRM");
		assert!(scan(b"\x1b[1;80;3s").is_empty(), "three is not");
		// An EMPTY parameter still counts as one, so a trailing separator makes a third.
		assert_eq!(numbers(b"\x1b[1;s"), (Some(1), None), "two, one omitted");
		assert!(scan(b"\x1b[1;;s").is_empty(), "three, two omitted");
	}

	/// The clamp and the engine's saturation land on the same number, which is the whole argument for
	/// clamping a digit run rather than capping it (§111). Five digits already reach past a `u16`, so
	/// every input the two could disagree about is one they both answer `u16::MAX` for.
	#[test]
	fn the_digit_clamp_and_the_engines_saturation_agree() {
		assert_eq!(numbers(b"\x1b[99999;5s"), (Some(u16::MAX), Some(5)));
		assert_eq!(numbers(b"\x1b[999999;5s"), (Some(u16::MAX), Some(5)));
		// And below the clamp nothing is lost, however the number was padded.
		assert_eq!(numbers(b"\x1b[00000000012345;5s"), (Some(12345), Some(5)));
	}

	#[test]
	fn a_runaway_separator_run_is_not_a_margin_request() {
		// The other half of the same rule: length is not what rules a sequence out, SHAPE is. Five
		// hundred separators is five hundred and one parameters, which is not DECSLRM — and the engine
		// stops counting at 32 and drops it too, so both sides leave it alone.
		let mut params = vec![b'\x1b', b'['];
		params.extend(std::iter::repeat_n(b';', 500));
		params.push(b's');
		assert!(scan(&params).is_empty());
	}

	#[test]
	fn a_control_byte_inside_a_csi_does_not_end_it() {
		// This test used to assert the opposite, and that was the disagreement (§106). The engine runs the
		// line feed where it sits and goes on reading the sequence around it, so `CSI 5;` LF `70 s` reaches
		// its save-cursor arm — and a scanner that had given up on it would be the reason the margins were
		// not applied and the saved cursor was overwritten. Same harm as a padded run, a different route.
		assert_eq!(numbers(b"\x1b[5;\n70s"), (Some(5), Some(70)));
	}

	#[test]
	fn only_can_and_sub_really_cancel_a_sequence() {
		// The two bytes the ANSI state machine defines as cancels, which is why cmote feeds one of them in
		// place of a final byte it refuses. The engine leaves the sequence for the same pair and no others.
		assert!(scan(b"\x1b[5;70\x18s").is_empty());
		assert!(scan(b"\x1b[5;70\x1as").is_empty());
		// And DEL is not one of them: the engine ignores it and keeps reading.
		assert_eq!(numbers(b"\x1b[5;70\x7fs"), (Some(5), Some(70)));
	}

	#[test]
	fn a_fresh_escape_restarts_the_match() {
		// The second sequence is the real one; the first was abandoned half-written.
		assert_eq!(scan(b"\x1b[5;\x1b[1;80s"), vec![10]);
	}
}

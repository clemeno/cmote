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
// means margins once DECLRMM (`CSI ? 69 h`) has been set, and means save-cursor otherwise. cmote
// does not have margins to give — the engine's scroll region is vertical only
// (`set_scrolling_region(top, bottom)`), so left/right margins would mean re-implementing print,
// wrap, insert, delete and scroll — and the engine refuses mode 69 outright, answering a DECRQM for
// it with `0`, "not recognised". A program that asks is therefore told the truth and will not spell
// `s` as DECSLRM.
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
// The rule for telling the two meanings apart, then: a parameter makes it DECSLRM. That is the only
// evidence in the bytes, since the mode that would settle it is one the engine never accepts. It
// costs the reading of `CSI 0 s` as a save-cursor, which nothing writes — every save-cursor in the
// wild is the bare `CSI s`, and that one is left alone and still works.

/// The byte fed to the engine in place of a final byte cmote refuses to let it dispatch.
///
/// CAN, "cancel" — the ANSI state machine's own way to abandon a sequence in flight (see the module
/// header for why this and not one of the alternatives).
pub const CANCEL: u8 = 0x18;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The longest parameter run we will count inside one sequence. DECSLRM carries two small numbers;
/// a longer run is malformed, and abandoning the sequence keeps a hostile stream from being able to
/// hold this scanner in its CSI state indefinitely (§12).
const MAX_PARAMS: usize = 32;

/// Where the scanner is in the byte stream. A CSI is `ESC [`, then parameter bytes, then
/// intermediate bytes, then one final byte — only the final byte decides anything here.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC. A CSI starts if the next byte is `[`.
	Escape,
	/// Inside `ESC [ …`, waiting for the final byte that says what this was.
	Csi,
}

/// The misparse scanner (§57). Feed it every byte of shell output; it reports the offset of each
/// final byte the engine must not be allowed to dispatch.
#[derive(Debug, Default)]
pub struct Cancel {
	state: Scan,
	/// How many parameter bytes the sequence in flight has carried. Non-zero is the whole test for
	/// DECSLRM against SCOSC.
	params: usize,
	/// Whether the sequence is still the plain `CSI <parameters> <final>` shape. A private marker
	/// (`< = > ?`) or an intermediate (0x20–0x2f) makes it a different sequence altogether —
	/// `CSI ? s` is XTSAVE, which the engine drops harmlessly — so neither is DECSLRM.
	plain: bool,
}

impl Cancel {
	/// Scan a chunk of shell output, returning the offsets of the final bytes to cancel. Safe at any
	/// chunk boundary: the state machine carries over between calls, so a sequence may be split
	/// anywhere, even between the ESC and the `[`.
	///
	/// Each offset is the position of the final byte ITSELF — a third convention, next to the start
	/// of the sequence that a prompt mark reports (§34) and the byte one PAST it that a selective
	/// erase reports (§56). It is the byte being replaced, so it is the byte named: the engine is
	/// advanced up to it, fed a CAN instead of it, and resumed after it.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<usize> {
		let mut cancels = Vec::new();
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Escape;
					}
				}
				Scan::Escape => match byte {
					b'[' => {
						self.params = 0;
						self.plain = true;
						self.state = Scan::Csi;
					}
					// ESC ESC: still waiting for the sequence's real first byte.
					ESC => {}
					_ => self.state = Scan::Text,
				},
				Scan::Csi => match byte {
					// Parameter bytes: the digits and the two separators.
					0x30..=0x3b => {
						self.params += 1;
						if self.params > MAX_PARAMS {
							self.state = Scan::Text;
						}
					}
					// A private marker. Legal only as the first parameter byte, and either way this
					// is not DECSLRM.
					0x3c..=0x3f => self.plain = false,
					// An intermediate byte. DECSLRM has none.
					0x20..=0x2f => self.plain = false,
					// The final byte ends the sequence, so this is where it is judged.
					0x40..=0x7e => {
						if byte == b's' && self.plain && self.params > 0 {
							cancels.push(index);
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
		cancels
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read what it asked to cancel.
	fn scan(bytes: &[u8]) -> Vec<usize> {
		let mut cancel = Cancel::default();
		cancel.feed(bytes)
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
		// The offset counts from the start of the chunk that completed the sequence.
		assert_eq!(cancel.feed(b"0shello"), vec![1]);
	}

	#[test]
	fn two_margin_requests_in_one_chunk_come_out_in_order() {
		// Ascending offsets, because `process` merges them with the other scanners' events and
		// advances the engine forwards only.
		assert_eq!(scan(b"\x1b[5;70s\x1b[1;80s"), vec![6, 13]);
	}

	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		// A hostile stream must not be able to hold the scanner in its CSI state for ever. Dropping
		// the sequence is the safe end: the engine then reads it as a save-cursor, which is where we
		// started, rather than cmote acting on unbounded input (§12).
		let mut params = vec![b'\x1b', b'['];
		params.extend(std::iter::repeat_n(b'1', MAX_PARAMS + 10));
		params.push(b's');
		assert!(scan(&params).is_empty());
	}

	#[test]
	fn a_control_byte_inside_a_csi_abandons_the_sequence() {
		// A newline mid-sequence means the stream is not sending what we thought.
		assert!(scan(b"\x1b[5;\n70s").is_empty());
	}

	#[test]
	fn a_fresh_escape_restarts_the_match() {
		// The second sequence is the real one; the first was abandoned half-written.
		assert_eq!(scan(b"\x1b[5;\x1b[1;80s"), vec![10]);
	}
}

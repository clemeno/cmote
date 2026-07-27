// term/compat.rs — rewrite the escape sequences our emulator does not know into the
// equivalent ones it does (PLAN §9).
//
// `vt100` implements the common CSI sequences but not their aliases. ECMA-48 gives
// several movements two spellings, and which one a program picks is pure house style:
//
//   CSI y ; x f   HVP  move to row, column      == CSI y ; x H   CUP
//   CSI n `       HPA  move to column n         == CSI n G       CHA
//   CSI n a       HPR  move n columns right     == CSI n C       CUF
//   CSI n e       VPR  move n rows down         == CSI n B       CUD
//   CSI s / CSI u      save / restore cursor    == ESC 7 / ESC 8 (DECSC / DECRC)
//
// A program that spells them the second way works; one that spells them the first way
// has every movement silently dropped, its output streams out as plain text, wraps at the
// right edge and scrolls — which is what btop does (its `Mv::to` emits the `f` form for
// every panel it draws, so nothing lands where it belongs).
//
// Rewriting the stream is the whole fix: the parser then sees only spellings it knows,
// and no other byte is touched. The scanner is a state machine rather than a search over
// a buffer because output arrives in arbitrary chunks — a sequence can be split anywhere,
// including between the ESC and the `[`.

/// ASCII escape, the lead byte of every sequence.
const ESC: u8 = 0x1b;

/// The longest CSI sequence we will hold back before deciding it is malformed. A real one
/// is a handful of bytes; anything longer is a broken or hostile stream, and it is passed
/// through untouched rather than buffered without bound (§12).
const MAX_SEQUENCE: usize = 64;

/// Where the scanner is in the byte stream.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; a CSI starts if the next byte is `[`.
	Escape,
	/// Inside a CSI's parameters, waiting for its final byte.
	Csi,
}

/// The rewriter. Feed it every byte of shell output; it writes the same stream out with
/// the alias sequences replaced by the spellings `vt100` implements.
#[derive(Debug, Default)]
pub struct Aliases {
	state: Scan,
	/// The sequence being held back: the ESC, the `[`, then its parameters. Held rather
	/// than emitted because a rewrite can change the sequence's length (`CSI s` is three
	/// bytes, `ESC 7` is two), so nothing may go out before the final byte decides.
	pending: Vec<u8>,
}

impl Aliases {
	/// Rewrite `input` into `out`. Safe at any chunk boundary — a sequence split across
	/// calls stays held until the call that completes it.
	pub fn rewrite(&mut self, input: &[u8], out: &mut Vec<u8>) {
		out.reserve(input.len());
		for &byte in input {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.pending.push(byte);
						self.state = Scan::Escape;
					} else {
						out.push(byte);
					}
				}
				Scan::Escape => {
					if byte == b'[' {
						self.pending.push(byte);
						self.state = Scan::Csi;
					} else {
						// Not a CSI: the ESC and this byte belong to some other sequence
						// (OSC, DECSC, a charset selection) and pass through untouched. An
						// ESC here starts a fresh one.
						out.append(&mut self.pending);
						if byte == ESC {
							self.pending.push(byte);
						} else {
							out.push(byte);
							self.state = Scan::Text;
						}
					}
				}
				Scan::Csi => match byte {
					// Parameter and intermediate bytes: still inside the sequence.
					0x20..=0x3f if self.pending.len() < MAX_SEQUENCE => self.pending.push(byte),
					// The final byte closes the sequence and decides what goes out.
					0x40..=0x7e => {
						self.finish(byte, out);
						self.state = Scan::Text;
					}
					// A control byte inside a sequence, or one longer than any real
					// sequence: malformed. Pass what we held through verbatim and let the
					// parser make of it what it will.
					_ => {
						out.append(&mut self.pending);
						out.push(byte);
						self.state = Scan::Text;
					}
				},
			}
		}
	}

	/// Emit the completed sequence, translated if its final byte is one of the aliases.
	/// Only a plain sequence is eligible: a private one (`CSI ? … h`) or one carrying an
	/// intermediate byte means something else entirely and goes out as it came in.
	fn finish(&mut self, final_byte: u8, out: &mut Vec<u8>) {
		let params = &self.pending[2..];
		let plain = !params
			.iter()
			.any(|byte| matches!(byte, 0x20..=0x2f | 0x3c..=0x3f));

		if plain {
			// Save and restore carry no parameters, and their equivalents are two-byte
			// sequences with no `[` at all, so they replace the whole thing.
			if params.is_empty()
				&& let Some(replacement) = simple_alias(final_byte)
			{
				out.extend_from_slice(&[ESC, replacement]);
				self.pending.clear();
				return;
			}
			if let Some(replacement) = moving_alias(final_byte) {
				out.append(&mut self.pending);
				out.push(replacement);
				return;
			}
		}

		out.append(&mut self.pending);
		out.push(final_byte);
	}
}

/// The final byte that means the same movement to `vt100`, for the aliases that differ
/// only in that byte. `None` for every other sequence, which passes through.
fn moving_alias(final_byte: u8) -> Option<u8> {
	match final_byte {
		b'f' => Some(b'H'), // HVP -> CUP: move to row;column
		b'`' => Some(b'G'), // HPA -> CHA: move to column
		b'a' => Some(b'C'), // HPR -> CUF: move right
		b'e' => Some(b'B'), // VPR -> CUD: move down
		_ => None,
	}
}

/// The byte after ESC for the two aliases that are not CSI sequences at all in the form
/// `vt100` understands: the ANSI.SYS cursor save and restore.
fn simple_alias(final_byte: u8) -> Option<u8> {
	match final_byte {
		b's' => Some(b'7'), // save cursor    -> DECSC
		b'u' => Some(b'8'), // restore cursor -> DECRC
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Rewrite one slice with a fresh scanner and read the result as bytes.
	fn rewrite(input: &[u8]) -> Vec<u8> {
		let mut aliases = Aliases::default();
		let mut out = Vec::new();
		aliases.rewrite(input, &mut out);
		out
	}

	#[test]
	fn hvp_becomes_cup() {
		// The btop bug: every panel it draws is positioned with the `f` spelling, which
		// vt100 drops on the floor. As `H` it moves the cursor where it was meant to go.
		assert_eq!(rewrite(b"\x1b[12;40fhello"), b"\x1b[12;40Hhello".to_vec());
		// No parameters at all is still a move — to the home position.
		assert_eq!(rewrite(b"\x1b[f"), b"\x1b[H".to_vec());
	}

	#[test]
	fn the_other_movement_aliases_are_translated_too() {
		assert_eq!(rewrite(b"\x1b[7`"), b"\x1b[7G".to_vec()); // HPA -> CHA
		assert_eq!(rewrite(b"\x1b[3a"), b"\x1b[3C".to_vec()); // HPR -> CUF
		assert_eq!(rewrite(b"\x1b[2e"), b"\x1b[2B".to_vec()); // VPR -> CUD
	}

	#[test]
	fn save_and_restore_become_their_two_byte_equivalents() {
		// These lose the `[` entirely, which is why the rewriter holds a sequence back
		// instead of emitting its bytes as they arrive.
		assert_eq!(rewrite(b"\x1b[s"), b"\x1b7".to_vec());
		assert_eq!(rewrite(b"\x1b[u"), b"\x1b8".to_vec());
		// With parameters, `s` is a margin-setting sequence and means something else; it
		// must be left exactly as it came in.
		assert_eq!(rewrite(b"\x1b[1;80s"), b"\x1b[1;80s".to_vec());
	}

	#[test]
	fn everything_else_passes_through_untouched() {
		// The sequences vt100 already knows, a private one, an OSC, and plain text —
		// including text with an `f` in it, which is not a final byte of anything.
		for stream in [
			&b"\x1b[12;40H"[..],
			&b"\x1b[?25l"[..],
			&b"\x1b[?1049h"[..],
			&b"\x1b[38;2;255;0;0m"[..],
			&b"\x1b]7;file://host/tmp\x07"[..],
			&b"\x1b7saved\x1b8"[..],
			&b"the quick brown fox"[..],
			&b"\x1b[2J\x1b[H"[..],
		] {
			assert_eq!(rewrite(stream), stream.to_vec());
		}
	}

	#[test]
	fn a_private_or_intermediate_sequence_is_never_translated() {
		// `CSI ? … f` and `CSI SP … f` are not HVP; the marker bytes say so, and a blind
		// final-byte swap would corrupt them.
		assert_eq!(rewrite(b"\x1b[?5f"), b"\x1b[?5f".to_vec());
		assert_eq!(rewrite(b"\x1b[ f"), b"\x1b[ f".to_vec());
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_rewritten() {
		// Output arrives in arbitrary chunks — including a split between ESC and `[`, and
		// one that leaves the final byte for the next call.
		let mut aliases = Aliases::default();
		let mut out = Vec::new();
		aliases.rewrite(b"row\x1b", &mut out);
		aliases.rewrite(b"[9;", &mut out);
		aliases.rewrite(b"3", &mut out);
		aliases.rewrite(b"ftail", &mut out);
		assert_eq!(out, b"row\x1b[9;3Htail".to_vec());
	}

	#[test]
	fn a_malformed_sequence_is_passed_through_not_swallowed() {
		// A newline in the middle of a CSI: not a sequence at all. Whatever was held must
		// come back out, or the text it was made of vanishes from the screen.
		assert_eq!(rewrite(b"\x1b[12;\nrest"), b"\x1b[12;\nrest".to_vec());
		// An ESC that starts a different sequence while one is open.
		assert_eq!(rewrite(b"\x1b\x1b[Hx"), b"\x1b\x1b[Hx".to_vec());
		// And one longer than any real sequence stops being buffered.
		let long = [b"\x1b[".as_slice(), &[b'1'; MAX_SEQUENCE], b"f"].concat();
		assert_eq!(rewrite(&long), long);
	}
}

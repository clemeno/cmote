// term/answer.rs — answer the status/identity queries our emulator leaves silent (PLAN §9).
//
// Some sequences are not commands the terminal obeys but QUESTIONS it must answer: the
// program writes them downstream and then BLOCKS reading its stdin until the reply comes
// back upstream. `vt100` has no arm for any of them — the same gap `compat.rs` papers over
// for cursor moves — so it drops them, and the program stalls until a timeout fires (vim,
// tmux, less and emacs all probe at startup; a `CSI 6n` size-probe can hang a shell script
// outright). We recognise them and reply.
//
//   CSI 5 n        DSR       are you ok?                 -> CSI 0 n
//   CSI 6 n        DSR-CPR   where is the cursor?        -> CSI row ; col R      (live cursor)
//   CSI ? 6 n      DECXCPR   cursor + page               -> CSI ? row ; col ; 1 R
//   CSI c / 0 c    DA1       what terminal are you?      -> CSI ? 62 ; 1 ; 6 c
//   CSI > c / > 0 c DA2      secondary id                -> CSI > 1 ; 10 ; 0 c
//
// This differs from `compat` in direction: compat rewrites bytes on the way INTO the
// parser, while a reply travels the other way — back to the server on the input channel,
// the same path a keystroke takes (see `Terminal::process` and app.rs). So this module
// withholds NOTHING from the parser (the queries are no-ops there anyway); it only reports
// WHERE each query ends, so the caller can split the parser feed there, read whatever live
// state the reply needs, and send the answer.
//
// The one query that needs live state is the cursor-position report: it must reflect the
// cursor WHERE THE QUERY SAT, not where later output in the same chunk left it. The classic
// size-probe is why — `ESC 7` save, `CSI 999;999 H` jump to the far corner, `CSI 6n` ask,
// `ESC 8` restore: read the cursor after the whole chunk and the restore has already undone
// the jump, so the program would misread the terminal size. Splitting the feed at the query
// keeps the answer honest.
//
// A scanner rather than a search over a buffer, because output arrives in arbitrary chunks
// and a query can split anywhere — including between the ESC and the `[`.

/// ASCII escape, the lead byte of every sequence.
const ESC: u8 = 0x1b;

/// The longest run of parameter bytes we hold before deciding a CSI is not a query. A real
/// one is a handful of bytes; anything longer is abandoned rather than buffered without
/// bound (§12) — the bytes still reach the parser, we simply stop trying to classify them.
const MAX_PARAMS: usize = 32;

/// The VT220-class primary device attributes (DA1) reply: VT220 (62) with 132-column mode
/// (1) and selective erase (6). Deliberately claims neither sixel (4) nor ReGIS (3), so a
/// program cannot take our answer as licence to send graphics the grid cannot render.
pub const PRIMARY_DA: &[u8] = b"\x1b[?62;1;6c";

/// The secondary device attributes (DA2) reply: terminal type 1 (VT220, consistent with the
/// DA1 above), firmware version 10, ROM cartridge 0. The version is what tools like tmux read
/// to gate features, so it stays low rather than impersonating a specific modern xterm.
pub const SECONDARY_DA: &[u8] = b"\x1b[>1;10;0c";

/// A query the emulator must answer. Which reply each maps to lives in `reply`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Query {
	/// DSR `CSI 5 n` — device status. The answer is a fixed "ok".
	Status,
	/// DSR-CPR `CSI 6 n` — the cursor's position. Needs the live cursor.
	CursorPosition,
	/// DECXCPR `CSI ? 6 n` — the cursor's position plus its page. Needs the live cursor.
	ExtendedCursorPosition,
	/// DA1 `CSI c` / `CSI 0 c` — primary device attributes.
	PrimaryAttributes,
	/// DA2 `CSI > c` / `CSI > 0 c` — secondary device attributes.
	SecondaryAttributes,
}

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

/// The query scanner. Feed it the same byte stream the parser gets; it names each status or
/// identity query and where it ends, and answers nothing itself — `Terminal` supplies the
/// live cursor and sends the reply.
#[derive(Debug, Default)]
pub struct Queries {
	state: Scan,
	/// The parameter and marker bytes of the CSI currently open (`?`, `>`, digits, `;`).
	/// Collected only to classify at the final byte; never re-emitted, since the parser
	/// receives every byte from the caller regardless.
	params: Vec<u8>,
}

impl Queries {
	/// Scan `input` for status/identity queries. Returns, in stream order, the byte index in
	/// `input` just PAST each query's final byte, paired with its kind. The caller feeds the
	/// parser up to that index, reads the cursor, and emits the reply (see `reply`). Safe at
	/// any chunk boundary — the state carries over, so a query split across calls is reported
	/// on the call that completes it, its earlier bytes already fed to the parser.
	pub fn scan(&mut self, input: &[u8]) -> Vec<(usize, Query)> {
		let mut cuts = Vec::new();
		for (index, &byte) in input.iter().enumerate() {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Escape;
					}
				}
				Scan::Escape => {
					self.state = match byte {
						b'[' => {
							self.params.clear();
							Scan::Csi
						}
						// ESC ESC: still waiting for the sequence's real first byte.
						ESC => Scan::Escape,
						// Some other escape (OSC, a charset selection, DECSC): not a CSI.
						_ => Scan::Text,
					};
				}
				Scan::Csi => match byte {
					// Parameter, marker and intermediate bytes: still inside the sequence.
					0x20..=0x3f if self.params.len() < MAX_PARAMS => self.params.push(byte),
					// The final byte closes the sequence; classify what we held.
					0x40..=0x7e => {
						if let Some(query) = classify(&self.params, byte) {
							cuts.push((index + 1, query));
						}
						self.state = Scan::Text;
					}
					// A control byte inside a sequence, or one longer than any real query:
					// give up on it. The bytes still reach the parser through the caller.
					_ => self.state = Scan::Text,
				},
			}
		}
		cuts
	}
}

/// Classify a completed CSI by its parameters and final byte, or `None` if it is not a query
/// we answer — every other `n`/`c` sequence (and everything else) passes through untouched.
fn classify(params: &[u8], final_byte: u8) -> Option<Query> {
	match final_byte {
		b'n' => match params {
			b"5" => Some(Query::Status),
			b"6" => Some(Query::CursorPosition),
			b"?6" => Some(Query::ExtendedCursorPosition),
			_ => None,
		},
		b'c' => match params {
			// DA1 with no parameter, or the explicit 0; anything else is not a plain DA1.
			b"" | b"0" => Some(Query::PrimaryAttributes),
			// DA2 carries the `>` marker.
			b">" | b">0" => Some(Query::SecondaryAttributes),
			_ => None,
		},
		_ => None,
	}
}

/// Write the reply for `query` into `out`, given the LIVE cursor as 1-based `(row, col)`.
/// Only the cursor-position reports use the cursor; the caller reads it once and passes it in
/// regardless, so this stays a pure function of its inputs (testable with no parser). Keeping
/// every reply's exact bytes here means the wire format lives in one file.
pub fn reply(query: Query, cursor: (u16, u16), out: &mut Vec<u8>) {
	let (row, col) = cursor;
	match query {
		Query::Status => out.extend_from_slice(b"\x1b[0n"),
		Query::CursorPosition => out.extend_from_slice(format!("\x1b[{row};{col}R").as_bytes()),
		Query::ExtendedCursorPosition => {
			out.extend_from_slice(format!("\x1b[?{row};{col};1R").as_bytes());
		}
		Query::PrimaryAttributes => out.extend_from_slice(PRIMARY_DA),
		Query::SecondaryAttributes => out.extend_from_slice(SECONDARY_DA),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan one slice with a fresh scanner and read back the queries it found.
	fn scan(input: &[u8]) -> Vec<(usize, Query)> {
		Queries::default().scan(input)
	}

	/// Build one reply with a given cursor and read it back as bytes.
	fn reply_bytes(query: Query, cursor: (u16, u16)) -> Vec<u8> {
		let mut out = Vec::new();
		reply(query, cursor, &mut out);
		out
	}

	#[test]
	fn each_query_is_recognised() {
		assert_eq!(scan(b"\x1b[5n"), vec![(4, Query::Status)]);
		assert_eq!(scan(b"\x1b[6n"), vec![(4, Query::CursorPosition)]);
		assert_eq!(scan(b"\x1b[?6n"), vec![(5, Query::ExtendedCursorPosition)]);
		// DA1 both with and without the explicit 0.
		assert_eq!(scan(b"\x1b[c"), vec![(3, Query::PrimaryAttributes)]);
		assert_eq!(scan(b"\x1b[0c"), vec![(4, Query::PrimaryAttributes)]);
		// DA2 likewise.
		assert_eq!(scan(b"\x1b[>c"), vec![(4, Query::SecondaryAttributes)]);
		assert_eq!(scan(b"\x1b[>0c"), vec![(5, Query::SecondaryAttributes)]);
	}

	#[test]
	fn the_cut_index_lands_just_past_the_final_byte() {
		// `xx␛[6nyy`: the query ends at index 6, so the caller feeds [..6] (query included,
		// a no-op in the parser) then reads the cursor, and [6..] is the trailing "yy".
		assert_eq!(scan(b"xx\x1b[6nyy"), vec![(6, Query::CursorPosition)]);
	}

	#[test]
	fn look_alike_sequences_are_left_alone() {
		// A DSR variant we do not answer, a private mode set, a clear, a DA2 with an id we do
		// not match, and plain text carrying an `n` and a `c`.
		for stream in [
			&b"\x1b[1n"[..],   // DSR 1 — not 5 or 6
			&b"\x1b[?25l"[..], // hide cursor
			&b"\x1b[2J"[..],   // clear screen
			&b"\x1b[>1c"[..],  // DA2 with a non-zero id
			&b"nice cars"[..],
		] {
			assert!(scan(stream).is_empty(), "should ignore {stream:?}");
		}
	}

	#[test]
	fn a_query_split_across_chunks_is_reported_on_completion() {
		// Output arrives in arbitrary chunks — including a split between the params and the
		// final byte. The head produces nothing; the tail reports the query at its own index.
		let mut queries = Queries::default();
		assert_eq!(queries.scan(b"\x1b[6"), vec![]);
		assert_eq!(queries.scan(b"n"), vec![(1, Query::CursorPosition)]);
	}

	#[test]
	fn a_split_between_esc_and_bracket_is_still_read() {
		let mut queries = Queries::default();
		assert_eq!(queries.scan(b"row\x1b"), vec![]);
		assert_eq!(queries.scan(b"[6n"), vec![(3, Query::CursorPosition)]);
	}

	#[test]
	fn an_overlong_parameter_run_is_abandoned() {
		// Past the cap we stop classifying; the sequence is not answered. (Its bytes still
		// reach the parser via the caller, so nothing is lost from the screen.)
		let long = [b"\x1b[".as_slice(), &[b'1'; MAX_PARAMS + 4], b"n"].concat();
		assert!(scan(&long).is_empty());
	}

	#[test]
	fn the_cursor_reports_are_one_based() {
		// vt100 hands back a 0-based cursor; the caller adds one, so a cursor at row 3,
		// column 5 reports `CSI 3;5 R`.
		assert_eq!(
			reply_bytes(Query::CursorPosition, (3, 5)),
			b"\x1b[3;5R".to_vec()
		);
		assert_eq!(
			reply_bytes(Query::ExtendedCursorPosition, (3, 5)),
			b"\x1b[?3;5;1R".to_vec()
		);
	}

	#[test]
	fn the_status_and_attribute_replies_are_fixed() {
		// These carry no live state, so the cursor passed in is irrelevant.
		assert_eq!(reply_bytes(Query::Status, (9, 9)), b"\x1b[0n".to_vec());
		assert_eq!(
			reply_bytes(Query::PrimaryAttributes, (9, 9)),
			PRIMARY_DA.to_vec()
		);
		assert_eq!(
			reply_bytes(Query::SecondaryAttributes, (9, 9)),
			SECONDARY_DA.to_vec()
		);
	}
}

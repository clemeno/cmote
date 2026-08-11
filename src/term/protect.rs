// term/protect.rs — selective erase: the cells a program asked us not to wipe (PLAN §56).
//
// A VT220 had a second, weaker kind of erase. The program marks some characters as PROTECTED, and
// then a *selective* erase wipes everything around them and leaves them standing:
//
//   CSI 1 " q     DECSCA — protect what is written from here on
//   CSI 0 " q     DECSCA — stop protecting (Ps 2 means the same)
//   CSI ? Ps J    DECSED — selective erase in the display, skipping protected cells
//   CSI ? Ps K    DECSEL — selective erase in the line, skipping protected cells
//
// It was built for data-entry forms on a serial line. The program draws the labels once
// (`Name:`, `Address:`) inside a protected run, the user types into the blanks unprotected, and the
// next record costs a single `CSI ? 2 J`: the typed fields clear and the labels stay. On a 9600-baud
// wire, not redrawing the form was the whole point. Modern full-screen programs repaint every frame
// instead, which is why almost nothing emits this today — and why the `?` spellings, unlike the
// plain `CSI J` / `CSI K`, are the ones no engine bothers with.
//
// `alacritty_terminal` is one of those engines: `vte`'s CSI dispatch has arms for `('J', [])` and
// `('K', [])` — EMPTY intermediates only — so `CSI ? 2 J` matches nothing and is dropped whole,
// and DECSCA (`('q', ['"'])`) has no arm either. Both therefore have to be cmote's own work, like
// the identity queries (§33), the prompt marks (§34) and the inline images (§41).
//
// The obvious way to do it would be a protection map beside the grid: a set of cells cmote marks as
// it watches the cursor. That way is a trap. Protection is per-CELL state, so the map would have to
// follow every scroll, every insert/delete line, every reflow on resize and every swap to the
// alternate page — which is to say, cmote would have to re-implement the grid to keep a bitmap
// aligned to it.
//
// So protection rides the engine's own PEN instead. Every cell the engine prints is stamped from
// `grid.cursor.template`, and the template carries a 16-bit flag word (`term::cell::Flags`) with
// bit 15 unused — the engine names fifteen. cmote sets that spare bit on the template while DECSCA
// is armed, and from then on the engine does all the work for free: the bit is part of each printed
// cell, so it rides scrolling, reflow, insert/delete and the alternate-screen swap exactly as bold
// or italic does, because it IS just another flag as far as the engine is concerned. Nothing in the
// engine reads it (`Cell::is_empty` tests named flags with `intersects`, so an unknown bit cannot
// make a blank cell look occupied) and nothing in cmote's renderer draws it (it matches named flags
// too), so the bit is invisible in both directions. `term/mod.rs` holds a build-time assertion that
// the engine has not since claimed bit 15.
//
// The one thing the engine does to the flag word wholesale is SGR 0: `Attr::Reset` assigns
// `Flags::empty()`, which would drop protection with it. DECSCA is independent of SGR on a real
// terminal — `CSI 0 m` inside a protected run must not unprotect it — so the scanner reports every
// SGR seen while armed and `mod.rs` re-asserts the bit after it. Re-asserting a bit that is still
// set is a no-op, so this deliberately over-reports rather than trying to work out which SGR lists
// contain a reset: over-reporting costs a split, under-reporting would silently lose protection.
//
// This module stays free of engine types on purpose — it deals in a `u16` flag word and in plain
// row/column numbers, so the region arithmetic and the whole grammar are testable without building
// a terminal, and the one file that knows the engine's names is still `term/mod.rs`.

use std::ops::Range;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The longest parameter run we will buffer inside one sequence. The real payloads here are tiny
/// (`1`, `?2`); a longer run is malformed, and refusing to grow past this keeps a hostile stream
/// from ballooning our memory (§12).
const MAX_PARAMS: usize = 32;

/// The most intermediate bytes we will buffer. A real CSI has at most one or two (`"` for DECSCA,
/// `!` for DECSTR); the cap is for the same reason as `MAX_PARAMS`.
const MAX_INTERMEDIATES: usize = 4;

/// The bit cmote borrows inside the engine's 16-bit per-cell flag word to mean "DECSCA-protected".
///
/// The engine names bits 0–14 and leaves 15 free, so this rides along in every cell the engine
/// prints, scrolls and reflows without the engine or the renderer ever noticing it (see the module
/// header). `term/mod.rs` asserts at build time that the engine has not claimed it since, so a
/// future version that adds a sixteenth flag fails the build instead of silently colliding — which
/// would show up as text that cannot be erased, the hardest kind of bug to trace back to here.
pub const PROTECTED_BIT: u16 = 1 << 15;

/// Whether a cell's flag word says the program protected it.
pub fn is_protected(flags: u16) -> bool {
	flags & PROTECTED_BIT != 0
}

/// The same flag word with protection added — for stamping the pen while DECSCA is armed.
pub fn mark(flags: u16) -> u16 {
	flags | PROTECTED_BIT
}

/// The same flag word with protection removed.
pub fn unmark(flags: u16) -> u16 {
	flags & !PROTECTED_BIT
}

/// How far a selective erase reaches, the `Ps` of DECSED and DECSEL. The same three values the
/// plain `CSI J` / `CSI K` take, so a reader who knows those knows these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extent {
	/// From the cursor to the end of the line (or of the screen). `Ps = 0`, and the default when
	/// the parameter is omitted.
	ToEnd,
	/// From the start of the line (or of the screen) up to and INCLUDING the cursor. `Ps = 1`.
	ToStart,
	/// The whole line, or the whole screen. `Ps = 2`.
	All,
}

/// Which selective erase a program asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Erase {
	/// DECSED — `CSI ? Ps J`, over the screen.
	Display(Extent),
	/// DECSEL — `CSI ? Ps K`, over the cursor's line.
	Line(Extent),
}

/// Something the stream asked cmote to do about protection, to be applied once the engine has been
/// advanced PAST the sequence that carried it (see `Protect::feed` on offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
	/// DECSCA moved the pen in or out of protecting what it writes. Also carries `false` for the
	/// two full resets (RIS `ESC c` and DECSTR `CSI ! p`), which clear the pen outright and so
	/// clear protection with it — the same as a real terminal.
	Protect(bool),
	/// An SGR passed by while the pen was armed, and SGR 0 assigns the whole flag word, so the
	/// protection bit has to be put back. Over-reported on purpose (see the module header).
	Reassert,
	/// A selective erase to perform, on the cells the engine has by now positioned the cursor in.
	Erase(Erase),
}

/// Where the scanner is in the byte stream. Only the CSI shape matters here, and a CSI is
/// `ESC [` then parameter bytes, then intermediate bytes, then one final byte.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC. A CSI starts if the next byte is `[`; `ESC c` is a full reset all on its own.
	Escape,
	/// Inside `ESC [ …`, collecting the sequence until its final byte.
	Csi,
}

/// The selective-erase scanner (§56). Feed it every byte of shell output; it tracks whether the
/// pen is protecting and reports the sequences the engine drops.
#[derive(Debug, Default)]
pub struct Protect {
	state: Scan,
	/// The private marker, if the sequence opened with one (`?` for DECSED and DECSEL). Kept apart
	/// from `params` so the parameter digits parse the same whether a marker was there or not.
	marker: Option<u8>,
	params: Vec<u8>,
	intermediates: Vec<u8>,
	/// Whether DECSCA has the pen protecting right now. The scanner keeps this because it is what
	/// decides whether an SGR is worth reporting — the common case, an unarmed stream, reports
	/// nothing at all and so costs `process` no splits.
	armed: bool,
}

impl Protect {
	/// Scan a chunk of shell output, returning what to do and where. Safe at any chunk boundary —
	/// the state machine carries over between calls, so a sequence may be split anywhere, even
	/// between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, which is the opposite of what it would be
	/// for a mark: a prompt mark (§34) is applied with the engine advanced up TO it, because the
	/// cursor is then on the line the mark names, while everything here has to be applied with the
	/// engine advanced PAST it. Re-asserting protection only makes sense once the SGR that wiped it
	/// has landed, and a selective erase reads the cursor the erase itself never moves — but the
	/// sequence still has to reach the engine first, since the engine is the thing that ignores it.
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
					// RIS. A full reset rebuilds the pen from scratch, protection included.
					b'c' => {
						self.armed = false;
						requests.push((index + 1, Request::Protect(false)));
						self.state = Scan::Text;
					}
					// ESC ESC: still waiting for the sequence's real first byte.
					ESC => {}
					_ => self.state = Scan::Text,
				},
				Scan::Csi => match byte {
					// Parameter bytes: the digits and separators, plus the private markers
					// (`< = > ?`, 0x3c–0x3f) which are only legal as the very first one.
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
					// Intermediate bytes — `"` makes DECSCA, `!` makes DECSTR.
					0x20..=0x2f => {
						self.intermediates.push(byte);
						if self.intermediates.len() > MAX_INTERMEDIATES {
							self.state = Scan::Text;
						}
					}
					// The final byte ends the sequence, so this is where it is judged.
					0x40..=0x7e => {
						if let Some(request) = self.classify(byte) {
							requests.push((index + 1, request));
						}
						self.state = Scan::Text;
					}
					// A fresh ESC restarts the match.
					ESC => self.state = Scan::Escape,
					// A C0 control byte or DEL inside a CSI: malformed, so drop the sequence
					// rather than let a stray byte extend it indefinitely.
					_ => self.state = Scan::Text,
				},
			}
		}
		requests
	}

	/// Decide what the sequence just completed means, and update the armed state if it was a DECSCA.
	/// Matching on all three of final byte, marker and intermediates together is what keeps the
	/// near-miss spellings out: `CSI 2 J` is a plain erase (no marker), `CSI > 4 ; 2 m` is
	/// XTMODKEYS rather than an SGR (marker `>`), and `CSI 1 SP q` is a cursor-style request rather
	/// than a DECSCA (intermediate ` `, not `"`).
	fn classify(&mut self, final_byte: u8) -> Option<Request> {
		match (final_byte, self.marker, self.intermediates.as_slice()) {
			// DECSCA — `CSI Ps " q`. Ps 1 protects; 0 and 2 both stop protecting, and so does an
			// omitted parameter, which means 0.
			(b'q', None, [b'"']) => {
				self.armed = self.first_param() == Some(1);
				Some(Request::Protect(self.armed))
			}
			// DECSTR soft reset — `CSI ! p`. Clears the pen the way RIS does.
			(b'p', None, [b'!']) => {
				self.armed = false;
				Some(Request::Protect(false))
			}
			// DECSED / DECSEL — the `?` spellings of erase, the two the engine drops.
			(b'J', Some(b'?'), []) => self.extent().map(Erase::Display).map(Request::Erase),
			(b'K', Some(b'?'), []) => self.extent().map(Erase::Line).map(Request::Erase),
			// An SGR while the pen is armed. Reported so the protection bit can be put back on
			// the far side of it, because SGR 0 assigns the flag word whole.
			(b'm', None, []) if self.armed => Some(Request::Reassert),
			_ => None,
		}
	}

	/// The first parameter as a number, treating an omitted one as 0 — which is what all three of
	/// DECSCA, DECSED and DECSEL default to. `None` only when the digits are unparseable, which
	/// leaves the sequence unclassified rather than guessing at it (§54's rule: malformed remote
	/// input is a no-op, never a reset).
	fn first_param(&self) -> Option<u16> {
		let digits = self
			.params
			.split(|&byte| byte == b';')
			.next()
			.unwrap_or_default();
		if digits.is_empty() {
			return Some(0);
		}
		let mut value: u16 = 0;
		for &byte in digits {
			let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
			value = value.checked_mul(10)?.checked_add(u16::from(digit))?;
		}
		Some(value)
	}

	/// How far this erase reaches. `None` for anything else, including `Ps = 3`: plain `CSI 3 J`
	/// drops the scrollback, and there is no selective version of that — protection is a property
	/// of cells on the screen, and history is not erased a cell at a time.
	fn extent(&self) -> Option<Extent> {
		match self.first_param()? {
			0 => Some(Extent::ToEnd),
			1 => Some(Extent::ToStart),
			2 => Some(Extent::All),
			_ => None,
		}
	}
}

/// The cells one selective erase covers: a row, and the half-open range of columns in it.
///
/// Pure arithmetic over the grid's size and the cursor's place in it, kept apart from the engine so
/// the six shapes can be checked without building a terminal. Rows are counted from the top of the
/// SCREEN, which is what the caller reads off the engine's cursor and passes back when indexing it.
pub fn spans(
	erase: Erase,
	row: usize,
	col: usize,
	rows: usize,
	cols: usize,
) -> Vec<(usize, Range<usize>)> {
	// A grid with no cells has nothing to erase, and saying so here keeps the `col + 1` below from
	// having to be careful about it.
	if rows == 0 || cols == 0 {
		return Vec::new();
	}
	// The cursor can legitimately sit one past the last column, waiting to wrap. An erase that
	// starts "at the cursor" then starts at the last real column, and one that ends there ends at
	// it — the same clamp serves both.
	let col = col.min(cols - 1);
	let row = row.min(rows - 1);
	let whole = |line: usize| (line, 0..cols);
	match erase {
		Erase::Line(Extent::ToEnd) => vec![(row, col..cols)],
		Erase::Line(Extent::ToStart) => vec![(row, 0..col + 1)],
		Erase::Line(Extent::All) => vec![whole(row)],
		Erase::Display(Extent::ToEnd) => std::iter::once((row, col..cols))
			.chain((row + 1..rows).map(whole))
			.collect(),
		Erase::Display(Extent::ToStart) => (0..row)
			.map(whole)
			.chain(std::iter::once((row, 0..col + 1)))
			.collect(),
		Erase::Display(Extent::All) => (0..rows).map(whole).collect(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read what it asked for.
	fn scan(bytes: &[u8]) -> Vec<(usize, Request)> {
		let mut protect = Protect::default();
		protect.feed(bytes)
	}

	/// Feed a slice to a scanner already armed, which is the state that makes SGR interesting.
	fn scan_armed(bytes: &[u8]) -> Vec<(usize, Request)> {
		let mut protect = Protect::default();
		protect.feed(b"\x1b[1\"q");
		protect.feed(bytes)
	}

	#[test]
	fn the_pen_starts_unprotected() {
		assert!(!Protect::default().armed);
	}

	#[test]
	fn decsca_one_arms_the_pen() {
		// `\x1b[1"q` is 5 bytes, so the offset is 5 — one PAST the final `q`.
		assert_eq!(scan(b"\x1b[1\"q"), vec![(5, Request::Protect(true))]);
	}

	#[test]
	fn decsca_zero_and_two_both_stop_protecting() {
		// DEC gives Ps 2 the same meaning as 0, and a program that writes either expects the
		// following text to be erasable.
		assert_eq!(scan(b"\x1b[0\"q"), vec![(5, Request::Protect(false))]);
		assert_eq!(scan(b"\x1b[2\"q"), vec![(5, Request::Protect(false))]);
	}

	#[test]
	fn decsca_with_no_parameter_stops_protecting() {
		// An omitted parameter means 0, so the shortest spelling unprotects.
		assert_eq!(scan(b"\x1b[\"q"), vec![(4, Request::Protect(false))]);
	}

	#[test]
	fn a_full_reset_stops_protecting() {
		// RIS and DECSTR both rebuild the pen, so protection cannot outlive either.
		assert_eq!(scan(b"\x1bc"), vec![(2, Request::Protect(false))]);
		assert_eq!(scan(b"\x1b[!p"), vec![(4, Request::Protect(false))]);
	}

	#[test]
	fn a_reset_disarms_the_scanner_as_well_as_reporting_it() {
		// Not just the report: the scanner must stop treating later SGRs as interesting, or every
		// coloured byte of an ordinary session would cost `process` a split.
		let mut protect = Protect::default();
		protect.feed(b"\x1b[1\"q");
		protect.feed(b"\x1bc");
		assert!(protect.feed(b"\x1b[31m").is_empty());
	}

	#[test]
	fn an_sgr_while_armed_asks_for_a_reassert() {
		// SGR 0 assigns the whole flag word, so protection has to be put back after it.
		assert_eq!(scan_armed(b"\x1b[0m"), vec![(4, Request::Reassert)]);
	}

	#[test]
	fn every_sgr_while_armed_is_reported_not_just_the_resetting_ones() {
		// Deliberate over-reporting: re-asserting a bit that is still set is a no-op, and telling
		// which SGR lists contain a reset means parsing colour specs, where a `0` can be a colour
		// index rather than a reset. Over-report and stay correct.
		assert_eq!(scan_armed(b"\x1b[38;5;0m"), vec![(9, Request::Reassert)]);
	}

	#[test]
	fn an_sgr_while_unarmed_is_not_reported() {
		// The common case. An ordinary session is full of SGR and must cost nothing here.
		assert!(scan(b"\x1b[31mred\x1b[0m").is_empty());
	}

	#[test]
	fn xtmodkeys_is_not_mistaken_for_an_sgr() {
		// `CSI > 4 ; 2 m` ends in `m` but is a keyboard-mode request, not a pen change. The
		// private marker is what tells them apart.
		assert!(scan_armed(b"\x1b[>4;2m").is_empty());
	}

	#[test]
	fn the_cursor_style_request_is_not_mistaken_for_a_decsca() {
		// `CSI 1 SP q` (DECSCUSR) shares the final byte with DECSCA and differs only in the
		// intermediate, so a scanner that ignored intermediates would arm the pen on a cursor
		// shape change.
		assert!(scan(b"\x1b[1 q").is_empty());
	}

	#[test]
	fn selective_erase_in_the_display_reads_all_three_extents() {
		assert_eq!(
			scan(b"\x1b[?J"),
			vec![(4, Request::Erase(Erase::Display(Extent::ToEnd)))]
		);
		assert_eq!(
			scan(b"\x1b[?0J"),
			vec![(5, Request::Erase(Erase::Display(Extent::ToEnd)))]
		);
		assert_eq!(
			scan(b"\x1b[?1J"),
			vec![(5, Request::Erase(Erase::Display(Extent::ToStart)))]
		);
		assert_eq!(
			scan(b"\x1b[?2J"),
			vec![(5, Request::Erase(Erase::Display(Extent::All)))]
		);
	}

	#[test]
	fn selective_erase_in_the_line_reads_all_three_extents() {
		assert_eq!(
			scan(b"\x1b[?K"),
			vec![(4, Request::Erase(Erase::Line(Extent::ToEnd)))]
		);
		assert_eq!(
			scan(b"\x1b[?1K"),
			vec![(5, Request::Erase(Erase::Line(Extent::ToStart)))]
		);
		assert_eq!(
			scan(b"\x1b[?2K"),
			vec![(5, Request::Erase(Erase::Line(Extent::All)))]
		);
	}

	#[test]
	fn a_plain_erase_is_left_to_the_engine() {
		// Without the `?` these are ordinary ED and EL, which the engine handles — and handles
		// differently: a plain `CSI 2 J` on the primary screen scrolls the viewport into history
		// rather than blanking it in place. Claiming them here would break that.
		assert!(scan(b"\x1b[2J\x1b[0J\x1b[2K\x1b[1K").is_empty());
	}

	#[test]
	fn there_is_no_selective_erase_of_the_scrollback() {
		// `CSI 3 J` drops the history; `CSI ? 3 J` is not a thing, because protection is a
		// property of cells on the screen.
		assert!(scan(b"\x1b[?3J").is_empty());
	}

	#[test]
	fn another_private_mode_sequence_is_ignored() {
		// Hide-cursor and bracketed paste share the `?` marker with DECSED, so the final byte has
		// to be checked too.
		assert!(scan(b"\x1b[?25l\x1b[?2004h\x1b[?25h").is_empty());
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks, including between the ESC and the `[`.
		let mut protect = Protect::default();
		assert!(protect.feed(b"label\x1b").is_empty());
		assert!(protect.feed(b"[1").is_empty());
		// The offset counts from the start of the chunk that completed the sequence.
		assert_eq!(protect.feed(b"\"qmore"), vec![(2, Request::Protect(true))]);
	}

	#[test]
	fn several_requests_in_one_chunk_come_out_in_order() {
		// The order is the whole point: arming, then the text, then the erase must be applied in
		// the sequence the stream put them in.
		assert_eq!(
			scan(b"\x1b[1\"qName:\x1b[0\"q\x1b[?2J"),
			vec![
				(5, Request::Protect(true)),
				(15, Request::Protect(false)),
				(20, Request::Erase(Erase::Display(Extent::All))),
			]
		);
	}

	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		// A hostile stream must not be able to grow our buffer, and dropping the sequence is the
		// safe end: an erase that never happens beats one built from unbounded input (§12).
		let mut params = vec![b'\x1b', b'['];
		params.extend(std::iter::repeat_n(b'1', MAX_PARAMS + 10));
		params.extend_from_slice(b"\"q");
		let mut protect = Protect::default();
		assert!(protect.feed(&params).is_empty());
		assert!(!protect.armed);
	}

	#[test]
	fn a_control_byte_inside_a_csi_abandons_the_sequence() {
		// A newline mid-sequence means the stream is not sending what we thought; the bytes after
		// it are ordinary output, not the rest of a DECSCA.
		assert!(scan(b"\x1b[1\n\"q").is_empty());
	}

	#[test]
	fn marking_and_unmarking_leave_the_engine_bits_alone() {
		// The neighbouring bits are bold, italic, underline and the rest — corrupting one would
		// repaint the screen wrong, so the borrowed bit has to be surgical.
		let engine = 0b0000_0000_0000_1110;
		assert!(!is_protected(engine));
		assert!(is_protected(mark(engine)));
		assert_eq!(unmark(mark(engine)), engine);
		assert_eq!(mark(engine) & !PROTECTED_BIT, engine);
	}

	#[test]
	fn unmarking_an_unprotected_word_changes_nothing() {
		assert_eq!(unmark(0b0000_0000_0000_0011), 0b0000_0000_0000_0011);
	}

	#[test]
	fn erase_in_line_to_end_starts_at_the_cursor() {
		assert_eq!(
			spans(Erase::Line(Extent::ToEnd), 2, 4, 5, 10),
			vec![(2, 4..10)]
		);
	}

	#[test]
	fn erase_in_line_to_start_includes_the_cursor_cell() {
		// The inclusive end is what the plain EL does, and a form clearing back to the margin
		// expects the cell under the cursor to go with it.
		assert_eq!(
			spans(Erase::Line(Extent::ToStart), 2, 4, 5, 10),
			vec![(2, 0..5)]
		);
	}

	#[test]
	fn erase_in_line_all_covers_the_whole_row_and_only_that_row() {
		assert_eq!(
			spans(Erase::Line(Extent::All), 2, 4, 5, 10),
			vec![(2, 0..10)]
		);
	}

	#[test]
	fn erase_in_display_to_end_runs_from_the_cursor_to_the_last_row() {
		assert_eq!(
			spans(Erase::Display(Extent::ToEnd), 2, 4, 5, 10),
			vec![(2, 4..10), (3, 0..10), (4, 0..10)]
		);
	}

	#[test]
	fn erase_in_display_to_start_runs_from_the_first_row_to_the_cursor() {
		assert_eq!(
			spans(Erase::Display(Extent::ToStart), 2, 4, 5, 10),
			vec![(0, 0..10), (1, 0..10), (2, 0..5)]
		);
	}

	#[test]
	fn erase_in_display_all_covers_every_row() {
		assert_eq!(
			spans(Erase::Display(Extent::All), 2, 4, 3, 4),
			vec![(0, 0..4), (1, 0..4), (2, 0..4)]
		);
	}

	#[test]
	fn a_cursor_waiting_to_wrap_erases_from_the_last_real_column() {
		// The cursor sits at column `cols` when it has filled the last cell and not yet wrapped.
		// Indexing there would be out of bounds, so it clamps back onto the grid.
		assert_eq!(
			spans(Erase::Line(Extent::ToEnd), 0, 10, 5, 10),
			vec![(0, 9..10)]
		);
		assert_eq!(
			spans(Erase::Line(Extent::ToStart), 0, 10, 5, 10),
			vec![(0, 0..10)]
		);
	}

	#[test]
	fn a_cursor_below_the_last_row_clamps_onto_the_grid() {
		assert_eq!(
			spans(Erase::Line(Extent::All), 99, 0, 5, 10),
			vec![(4, 0..10)]
		);
	}

	#[test]
	fn a_grid_with_no_cells_yields_no_spans() {
		assert!(spans(Erase::Display(Extent::All), 0, 0, 0, 0).is_empty());
		assert!(spans(Erase::Line(Extent::ToStart), 0, 0, 5, 0).is_empty());
	}
}

// term/tabs.rs — DECST8C, the tab stops a program asks to have put back (PLAN §74).
//
//   CSI ? 5 W    DECST8C — clear every tab stop, then set one every eight columns
//
// That is the state a terminal powers up in, and this is its one-sequence spelling. Without it a
// program has to walk the stops out by hand: `CSI 3 g` to clear the lot, a carriage return to get to
// column 0, then a run of `ESC H` eight columns apart. Which is precisely what ncurses' `reset` does
// — `clear_all_tabs`, then `init_tabs` columns of movement and `set_tab`, over and over — because
// terminfo has capabilities for the pieces and none for the whole.
//
// `vte` parses the sequence: `('W', [b'?']) if next_param_or(0) == 5 => handler.set_tabs(8)`. But
// `alacritty_terminal` never overrides `Handler::set_tabs`, whose default body in the trait is empty,
// so the sequence reached the engine and stopped there. That is §72's shape exactly — a sequence with
// no arm behind it, in a terminal that already has every ingredient the sequence is made of — and it
// gets §72's answer: cmote does not write the engine's tab table itself. The table is private, the
// engine keeps it aligned across every resize on its own, and a second writer of engine state is the
// thing §71 and §73 both refused to become. cmote FEEDS the engine the long spelling instead, out of
// sequences the engine does handle and the matrix already marks ✅: TBC (`CSI 3 g`), CR, HTS
// (`ESC H`) and CUF (`CSI Ps C`).
//
// WHICH sequences do the walking is the whole of the care taken here. `alacritty_terminal` routes
// `CSI G` (CHA), `CSI d` (VPA), `CSI A` and `CSI B` through one `goto`, which adds the scrolling
// region's top to the line it is handed — while those four hand it the line the cursor is ALREADY
// on. Under origin mode with a region that does not start at the top of the page, every one of them
// therefore drags the cursor down by the region's top, once per call. CR, CUF and CUB never go near
// `goto`: they assign the column and leave the line untouched. A walk built out of those three
// cannot move the cursor's row under any mode, so nothing in here has to know about origin mode, the
// scrolling region, or the saved cursor — and the row that comes out is the row that went in,
// without cmote having read a single piece of engine state to make that true.
//
// One thing does not survive the trip: a cursor waiting to wrap. CR, CUF and CUB all clear the
// engine's pending-wrap flag, so a program that filled the last column, asked for its tab stops
// back, and then printed one more character gets that character over the last cell instead of at the
// start of the next line. It is not detectable from outside the engine — a pending wrap looks like a
// cursor sitting in the last column — and it is the same small loss the soft reset takes (§72).

/// The interval DECST8C names: one stop every eight columns, counting from column 0.
///
/// Column 0 is a stop on a real terminal and in the engine's own power-on table
/// (`TabStops::new` fills `i % 8 == 0`), which only shows up under a BACKWARD tab: `CSI Z` from
/// column 5 has to land on 0 rather than run off the page. So the walk sets one there too.
pub const INTERVAL: u16 = 8;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The longest parameter run buffered inside one sequence. DECST8C's is a single digit; anything
/// longer is malformed, and refusing to grow past this keeps a hostile stream from ballooning our
/// memory (§12).
const MAX_PARAMS: usize = 32;

/// The most intermediate bytes buffered. DECST8C has none at all — they are collected only so that a
/// near miss carrying one is rejected rather than mistaken for it.
const MAX_INTERMEDIATES: usize = 4;

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

/// The DECST8C scanner (§74). Feed it every byte of shell output; it reports where each tab-stop
/// reset sat, for `term/mod.rs` to carry out.
#[derive(Debug, Default)]
pub struct Tabs {
	state: Scan,
	/// The private marker, if the sequence opened with one. DECST8C requires `?`, and keeping the
	/// marker apart from `params` lets the digits parse the same either way.
	marker: Option<u8>,
	params: Vec<u8>,
	intermediates: Vec<u8>,
}

impl Tabs {
	/// Scan a chunk of shell output, returning where each DECST8C sat. Safe at any chunk boundary —
	/// the state machine carries over between calls, so a sequence may be split anywhere, even
	/// between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the selective erase (§56) and the
	/// rectangles (§58) and for the same reason: the engine has to be advanced past the sequence it
	/// ignores before cmote does anything, or the fed walk would land in front of it and the engine
	/// would then parse the tail of a sequence cmote had already answered.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<usize> {
		let mut resets = Vec::new();
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
					// Intermediate bytes. DECST8C has none, so any of these rules it out.
					0x20..=0x2f => {
						self.intermediates.push(byte);
						if self.intermediates.len() > MAX_INTERMEDIATES {
							self.state = Scan::Text;
						}
					}
					// The final byte ends the sequence, so this is where it is judged.
					0x40..=0x7e => {
						if self.is_tab_reset(byte) {
							resets.push(index + 1);
						}
						self.state = Scan::Text;
					}
					// A fresh ESC restarts the match.
					ESC => self.state = Scan::Escape,
					// A byte the grammar above does not claim, but which the engine reads STRAIGHT
					// THROUGH — it runs a mid-sequence C0 where it sits, ignores DEL, and does nothing
					// with a high byte, keeping the sequence in every case (§106). Abandoning it here
					// would mean cmote and the engine disagreeing about what this byte stream even was,
					// which is how three defects reached a release.
					byte if super::csi::passes_through(byte) => {}
					// CAN and SUB, the only two bytes that really cancel a sequence in flight.
					_ => self.state = Scan::Text,
				},
			}
		}
		resets
	}

	/// Whether the sequence just completed is DECST8C — `CSI ? 5 W`, the marker and the parameter
	/// both required.
	///
	/// Read straight off `vte`'s own arm (`('W', [b'?']) if next_param_or(0) == 5`), deliberately, so
	/// that cmote and the engine agree on what the bytes are even though only one of them acts. The
	/// near misses this keeps out: `CSI 5 W` with no marker is CTC, a different sequence entirely;
	/// `CSI ? W` and `CSI ? 2 W` are DECST8C's own final byte carrying a value DEC never defined for
	/// it, and an undefined value is a no-op rather than a guess (§54).
	fn is_tab_reset(&self, final_byte: u8) -> bool {
		(final_byte, self.marker, self.intermediates.as_slice()) == (b'W', Some(b'?'), &[][..])
			&& self.first_param() == Some(5)
	}

	/// The first parameter as a number, treating an omitted one as 0 — which is what `vte` does, and
	/// 0 is not 5, so an empty `CSI ? W` matches nothing. `None` when the digits are unparseable,
	/// which leaves the sequence unclassified rather than guessing at it.
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
}

/// DECST8C written in the sequences the engine itself handles, for `term/mod.rs` to feed it (§74).
///
/// `columns` is the page width and `col` the cursor's column, both as the engine reports them. The
/// string clears every stop, walks the page setting one every `INTERVAL` columns, and returns the
/// cursor to the column it started in — by carriage return and CUF, so the walk cannot touch the
/// cursor's row (see the module header for why that matters). Nothing in it prints a glyph, so the
/// page underneath is untouched.
///
/// Pure: the arithmetic and the spelling are both testable without building a terminal, which is the
/// same split every scanner in `term/` keeps.
pub fn every_eighth_column(columns: u16, col: u16) -> Vec<u8> {
	// `CSI 3 g` is TBC "clear them all", and CR puts the cursor on column 0 — the first stop.
	let mut feed = b"\x1b[3g\r".to_vec();
	let mut stop = 0;
	while stop < columns {
		// HTS sets a stop at the column the cursor is in, which is why this is a walk at all.
		feed.extend_from_slice(b"\x1bH");
		stop += INTERVAL;
		// Step only while another stop is left to reach. CUF clamps at the last column, so a step
		// past the page would not set anything — it would just leave a no-op sequence in the feed
		// and the cursor somewhere the next line has to undo anyway.
		if stop < columns {
			feed.extend_from_slice(format!("\x1b[{INTERVAL}C").as_bytes());
		}
	}
	// Back to where the cursor started. CR first because the walk ended wherever the last stop was,
	// and counting forward from column 0 needs no arithmetic about where that was.
	feed.push(b'\r');
	if col > 0 {
		feed.extend_from_slice(format!("\x1b[{col}C").as_bytes());
	}
	feed
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<usize> {
		Tabs::default().feed(bytes)
	}

	/// The sequence itself, and the offset it reports: ONE PAST the final byte, so the engine has
	/// already ignored it by the time cmote answers.
	#[test]
	fn a_tab_reset_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b[?5W"), vec![5]);
		assert_eq!(scan(b"ab\x1b[?5Wcd"), vec![7]);
	}

	/// The parameter is the whole test, exactly as it is for the engine: `next_param_or(0)` makes an
	/// omitted parameter 0, and DEC defined no other value for this final byte.
	#[test]
	fn only_the_parameter_five_is_a_tab_reset() {
		assert!(scan(b"\x1b[?W").is_empty(), "an omitted parameter means 0");
		assert!(scan(b"\x1b[?0W").is_empty());
		assert!(scan(b"\x1b[?2W").is_empty());
		assert!(scan(b"\x1b[?50W").is_empty(), "not a prefix match");
	}

	/// Without the private marker this is CTC, a different sequence with a different meaning. The
	/// engine has no arm for that one either, but agreeing with it about what the bytes ARE is what
	/// keeps one reading of the grammar in the terminal.
	#[test]
	fn the_private_marker_is_required() {
		assert!(scan(b"\x1b[5W").is_empty());
		assert!(
			scan(b"\x1b[>5W").is_empty(),
			"a different marker is a different sequence"
		);
	}

	/// An intermediate byte makes it something else again, so the match tests all three of final
	/// byte, marker and intermediates — the near-miss rule §56 wrote down.
	#[test]
	fn an_intermediate_byte_rules_it_out() {
		assert!(scan(b"\x1b[?5 W").is_empty());
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to
	/// carry across a boundary drawn anywhere — including between the ESC and the `[`.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut tabs = Tabs::default();
		assert!(tabs.feed(b"\x1b").is_empty());
		assert!(tabs.feed(b"[?").is_empty());
		assert!(tabs.feed(b"5").is_empty());
		// The offset is into THIS chunk, which is where the split advance uses it.
		assert_eq!(tabs.feed(b"W"), vec![1]);
	}

	/// A control byte inside a CSI abandons the sequence rather than extending it, so the `W` that
	/// follows is not read as this sequence's final byte.
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		// The reverse of what this asserted before §106: the engine reads a mid-sequence control byte
		// through and keeps the sequence, so cmote does too, or the two disagree about the same bytes.
		assert!(!scan(b"\x1b[?5\x07W").is_empty());
		// CAN and SUB are the only two that really cancel one.
		assert!(scan(b"\x1b[?5\x18W").is_empty());
	}

	/// A hostile stream must not be able to make the scanner buffer without bound.
	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		let mut bytes = b"\x1b[?".to_vec();
		bytes.extend(std::iter::repeat_n(b'5', MAX_PARAMS + 10));
		bytes.push(b'W');
		assert!(scan(&bytes).is_empty());
	}

	/// Two in one chunk, both reported, in stream order — the split advance walks them in the order
	/// they came.
	#[test]
	fn two_resets_in_one_chunk_are_both_reported() {
		assert_eq!(scan(b"\x1b[?5W\x1b[?5W"), vec![5, 10]);
	}

	/// The walk itself: a stop at column 0 and every eighth column after it, and nothing past the
	/// page's width.
	#[test]
	fn the_walk_sets_a_stop_every_eighth_column() {
		// 24 columns: stops at 0, 8 and 16, so three HTS and two steps between them.
		assert_eq!(
			every_eighth_column(24, 0),
			b"\x1b[3g\r\x1bH\x1b[8C\x1bH\x1b[8C\x1bH\r".to_vec()
		);
	}

	/// A width that is not a multiple of eight ends at the last stop that fits, and sends no step
	/// after it: CUF would clamp to the last column rather than run off the page, so the sequence
	/// would set nothing and only move the cursor the restoring CR has to bring back.
	#[test]
	fn a_ragged_width_stops_at_the_last_one_that_fits() {
		// 20 columns: stops at 0, 8 and 16 — 24 is off the page, so the walk ends at the third HTS.
		assert_eq!(
			every_eighth_column(20, 0),
			b"\x1b[3g\r\x1bH\x1b[8C\x1bH\x1b[8C\x1bH\r".to_vec()
		);
	}

	/// A page narrower than one interval still has its column 0, and a zero-width one asks for
	/// nothing — neither is a shape a terminal really has, and neither may produce a stray stop.
	#[test]
	fn a_narrow_page_still_gets_its_first_stop() {
		assert_eq!(every_eighth_column(4, 0), b"\x1b[3g\r\x1bH\r".to_vec());
		assert_eq!(every_eighth_column(0, 0), b"\x1b[3g\r\r".to_vec());
	}

	/// The cursor goes back where it was, counted forward from column 0 so the walk's own end
	/// position never enters into it.
	#[test]
	fn the_walk_puts_the_cursor_back() {
		let feed = every_eighth_column(24, 13);
		assert!(
			feed.ends_with(b"\r\x1b[13C"),
			"got {:?}",
			String::from_utf8_lossy(&feed)
		);
		// Column 0 needs no CUF at all, and `CSI 0 C` would move one column on a real terminal.
		assert!(every_eighth_column(24, 0).ends_with(b"\x1bH\r"));
	}

	/// The rule the module header argues for, pinned as a rule rather than as a string: the walk is
	/// built ONLY out of sequences that cannot touch the cursor's row. `CSI G` would read the same
	/// on a page with no scrolling region and drag the cursor down one with, which is a divergence no
	/// end-to-end test would catch unless it thought to set origin mode first.
	#[test]
	fn the_walk_uses_no_sequence_that_can_move_the_row() {
		// Every spelling the walk is allowed to emit, and the CUF that restores this cursor column.
		let allowed: [&[u8]; 4] = [b"\x1b[3g", b"\x1bH", b"\x1b[8C", b"\x1b[40C"];
		let feed = every_eighth_column(120, 40);
		let mut rest = feed.as_slice();
		// Consume the whole feed from the front: at every byte it is either CR — the one plain byte
		// the walk sends, and a column move on any terminal — or the start of an allowed sequence.
		while let Some((&first, tail)) = rest.split_first() {
			if first == b'\r' {
				rest = tail;
				continue;
			}
			let spelling = allowed
				.iter()
				.find(|spelling| rest.starts_with(spelling))
				.unwrap_or_else(|| {
					panic!(
						"not a sequence the walk may use: {:?}",
						String::from_utf8_lossy(rest)
					)
				});
			rest = &rest[spelling.len()..];
		}
	}
}

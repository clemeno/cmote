// term/rect.rs — the VT420 rectangular area operations (PLAN §58).
//
// A VT420 could act on a BOX of the screen rather than a run of it. Four sequences, all sharing the
// `$` intermediate and all giving their corners as `Pt;Pl;Pb;Pr` — top, left, bottom, right, 1-based
// and inclusive:
//
//   CSI Pt;Pl;Pb;Pr $ z                          DECERA  — erase the rectangle
//   CSI Pt;Pl;Pb;Pr $ {                          DECSERA — erase it, leaving protected cells (§56)
//   CSI Pch;Pt;Pl;Pb;Pr $ x                      DECFRA  — fill it with one character
//   CSI Pts;Pls;Pbs;Prs;Pps;Ptd;Pld;Ppd $ v      DECCRA  — copy it somewhere else
//
// They were the block operations of a forms terminal: clear a field, rule a line of `-` across a box,
// scroll a sub-window by copying it up a row. A modern full-screen program repaints instead, so almost
// nothing emits these — but they are cheap here, because everything they need already exists. §56 had
// to solve the hard half: writing cells directly into the engine's grid, and knowing which of them a
// program marked as protected.
//
// `vte` has no arm for any of them. Its CSI dispatch matches `$` only in `('p', [b'$'])` and
// `('p', [b'?', b'$'])` — the two DECRQM spellings — so every one of these four falls through to the
// unhandled arm and is dropped whole. That makes them cmote's, the same way DECSCA and the `?` erases
// were.
//
// This module is the grammar and the geometry, and nothing else: it deals in plain row and column
// numbers, so every corner case of clamping, defaulting and overlap is tested without building a
// terminal. `term/mod.rs` does the cell writing, as it does for the selective erase.
//
// Three rules are worth stating where they are decided rather than where they are executed.
//
// PROTECTION. Only DECSERA respects it. DECERA, DECFRA and DECCRA go straight through a protected
// cell, exactly as the plain `CSI J` does and the `?` one does not (§56) — two verbs, the plain one
// stronger. A cell COPIED by DECCRA carries its protection with it, which comes for free from copying
// the whole cell, and is right: DECSCA marks a cell unerasable, not immovable.
//
// DECFRA's CHARACTER is an allow-list. `Pch` is a character code, and only 32–126 and 160–255 are
// accepted — printable ASCII and printable Latin-1, the same two ranges xterm allows. Everything else
// drops the whole sequence. A remote must not be able to paint the page with C0 or C1 controls, with
// DEL, or with unassigned code points, and "fill four hundred cells with U+0000" is not a request
// worth honouring on the way to finding out what the renderer does with it (§12).
//
// DECCRA's SOURCE IS READ FIRST. The two rectangles may overlap — scrolling a sub-window by one row is
// the whole point of the sequence, and that is the maximally overlapping case — so the copy is defined
// as if it went through a buffer, and cmote does exactly that rather than choosing a clever direction
// to walk in. Its two page parameters (`Pps`, `Ppd`) are ignored: cmote has one page, which is what
// clamping a page number to the number of pages the terminal has amounts to.

use std::ops::RangeInclusive;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The longest parameter run we will buffer inside one sequence. DECCRA is the big one at eight
/// parameters — `1;1;24;80;1;1;1;1` is 17 bytes — so this is generous headroom rather than a limit
/// anything real will meet, and refusing to grow past it keeps a hostile stream from ballooning our
/// memory (§12).
const MAX_PARAMS: usize = 64;

/// The most intermediate bytes we will buffer. These sequences have exactly one (`$`).
const MAX_INTERMEDIATES: usize = 4;

/// The four corners as the sequence spelled them: 1-based and inclusive, with 0 — which is also what
/// an omitted parameter means — standing for "the edge of the page".
///
/// Kept unresolved on purpose. Turning these into cells needs the page size, which is the engine's to
/// know, so the scanner reports what the program asked for and `area` below resolves it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Corners {
	pub top: u16,
	pub left: u16,
	pub bottom: u16,
	pub right: u16,
}

/// A resolved rectangle of cells: 0-based, inclusive on all four sides, and known to be inside the
/// page it was resolved against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
	pub top: usize,
	pub left: usize,
	pub bottom: usize,
	pub right: usize,
}

impl Area {
	/// The rows this rectangle covers, top to bottom.
	pub fn rows(&self) -> RangeInclusive<usize> {
		self.top..=self.bottom
	}

	/// The columns this rectangle covers, left to right.
	pub fn columns(&self) -> RangeInclusive<usize> {
		self.left..=self.right
	}

	/// How many cells wide it is — the stride a copy reads its buffer back at.
	pub fn width(&self) -> usize {
		self.right - self.left + 1
	}

	/// How many cells tall it is.
	pub fn height(&self) -> usize {
		self.bottom - self.top + 1
	}
}

/// One rectangular operation the stream asked for, to be applied once the engine has been advanced
/// past the sequence that carried it (see `Rectangles::feed` on offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
	/// DECERA — blank the rectangle. Protection does not stop it.
	Erase(Corners),
	/// DECSERA — blank the rectangle, leaving the cells DECSCA protected (§56).
	SelectiveErase(Corners),
	/// DECFRA — fill the rectangle with one character, in the pen's current attributes.
	Fill(char, Corners),
	/// DECCRA — copy the rectangle to a destination named by its top-left corner alone, since the
	/// extent comes from the source. The corner is 1-based like everything else here, with 0 meaning
	/// the first row or column.
	Copy {
		source: Corners,
		top: u16,
		left: u16,
	},
}

/// Where the scanner is in the byte stream. A CSI is `ESC [`, then parameter bytes, then intermediate
/// bytes, then one final byte — and for this family the intermediate is what matters most.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC. A CSI starts if the next byte is `[`.
	Escape,
	/// Inside `ESC [ …`, collecting the sequence until its final byte.
	Csi,
}

/// The rectangular-operations scanner (§58). Feed it every byte of shell output; it reports the four
/// sequences the engine drops, in the order the stream put them.
#[derive(Debug, Default)]
pub struct Rectangles {
	state: Scan,
	/// The private marker, if the sequence opened with one. None of these four have one, so its only
	/// job is to rule a near-miss out.
	marker: Option<u8>,
	params: Vec<u8>,
	intermediates: Vec<u8>,
}

impl Rectangles {
	/// Scan a chunk of shell output, returning what to do and where. Safe at any chunk boundary — the
	/// state machine carries over between calls, so a sequence may be split anywhere, even between the
	/// ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, as a selective erase's is (§56) and unlike a
	/// prompt mark's. These operations name their own coordinates and never read or move the cursor,
	/// so the offset is not about where the cursor will be — it is only about ordering against the
	/// text in the same chunk, and applying them on the far side of the sequence the engine is about
	/// to ignore keeps both halves in a defined state.
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
					// Intermediate bytes — `$` is the one that makes this family.
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
					// A C0 control byte or DEL inside a CSI: malformed, so drop the sequence rather
					// than let a stray byte extend it indefinitely.
					_ => self.state = Scan::Text,
				},
			}
		}
		requests
	}

	/// Decide what the sequence just completed means. All three of final byte, marker and
	/// intermediates are matched together, which is what keeps the neighbours out: `CSI Ps $ p` is
	/// DECRQM, and `CSI Pt;Pl;Pb;Pr;Ps… $ r` / `$ t` are the attribute-changing pair (DECCARA and
	/// DECRARA) this module deliberately does not claim.
	fn classify(&self, final_byte: u8) -> Option<Request> {
		let numbers = self.numbers()?;
		match (final_byte, self.marker, self.intermediates.as_slice()) {
			// DECERA / DECSERA — the same four corners, differing only in whether protection holds.
			(b'z', None, [b'$']) => Some(Request::Erase(corners(&numbers, 0))),
			(b'{', None, [b'$']) => Some(Request::SelectiveErase(corners(&numbers, 0))),
			// DECFRA — the character first, then the four corners.
			(b'x', None, [b'$']) => {
				let glyph = fill_char(number(&numbers, 0))?;
				Some(Request::Fill(glyph, corners(&numbers, 1)))
			}
			// DECCRA — the source's four corners, its page (ignored), then the destination's top-left
			// corner. The eighth parameter is the destination page, ignored for the same reason.
			(b'v', None, [b'$']) => Some(Request::Copy {
				source: corners(&numbers, 0),
				top: number(&numbers, 5),
				left: number(&numbers, 6),
			}),
			_ => None,
		}
	}

	/// Every parameter as a number, with an omitted one reading as 0 — which is what all of these
	/// spell "the default" as. `None` when any of them is unparseable, which drops the whole sequence
	/// rather than acting on half of it (§54's rule: malformed remote input is a no-op, never a
	/// guess). A rectangle built from a misread corner would erase the wrong cells.
	fn numbers(&self) -> Option<Vec<u16>> {
		self.params
			.split(|&byte| byte == b';')
			.map(|digits| {
				let mut value: u16 = 0;
				for &byte in digits {
					let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
					value = value.checked_mul(10)?.checked_add(u16::from(digit))?;
				}
				Some(value)
			})
			.collect()
	}
}

/// One parameter by position, or 0 when the sequence stopped short of it — the same thing an omitted
/// parameter means, so a program may leave any tail off.
fn number(numbers: &[u16], index: usize) -> u16 {
	numbers.get(index).copied().unwrap_or(0)
}

/// Four consecutive parameters read as corners, starting at `first`.
fn corners(numbers: &[u16], first: usize) -> Corners {
	Corners {
		top: number(numbers, first),
		left: number(numbers, first + 1),
		bottom: number(numbers, first + 2),
		right: number(numbers, first + 3),
	}
}

/// DECFRA's fill character, or `None` for a code cmote will not paint the page with.
///
/// Printable ASCII and printable Latin-1 only, the two ranges xterm allows. Latin-1 maps to the
/// Unicode code points of the same value, so the conversion is the identity and cannot fail — but it
/// is written as a conversion rather than a cast so the allow-list stays the only thing deciding.
fn fill_char(code: u16) -> Option<char> {
	match code {
		32..=126 | 160..=255 => char::from_u32(u32::from(code)),
		_ => None,
	}
}

/// Resolve four corners against a page of this size: 1-based and inclusive becomes 0-based and
/// inclusive, defaults become edges, and anything past an edge is clamped to it.
///
/// `None` when the rectangle holds no cells — an empty page, or corners the program crossed over
/// (`Pt` below `Pb`, `Pl` right of `Pr`), or a top-left that starts off the page entirely. A DEC
/// terminal does nothing for those, and so does this.
pub fn area(corners: Corners, rows: usize, cols: usize) -> Option<Area> {
	if rows == 0 || cols == 0 {
		return None;
	}
	// An omitted or zero parameter means the default, and the default is the edge in that direction:
	// the first row or column for a start, the last for an end.
	let start = |value: u16| usize::from(value).max(1);
	let end = |value: u16, last: usize| match usize::from(value) {
		0 => last,
		given => given.min(last),
	};
	let top = start(corners.top);
	let left = start(corners.left);
	let bottom = end(corners.bottom, rows);
	let right = end(corners.right, cols);
	// An END past the page is clamped to it — a program sized for a bigger screen gets the part of
	// its rectangle that exists. A START past the page is not: clamping it back onto the last row
	// would act on a row the program never named, and doing nothing is the better answer (§57).
	if top > rows || left > cols || top > bottom || left > right {
		return None;
	}
	Some(Area {
		top: top - 1,
		left: left - 1,
		bottom: bottom - 1,
		right: right - 1,
	})
}

/// Where a copy actually lands: the part of the source that fits, and the destination's 0-based
/// top-left corner.
///
/// The destination is only a corner, so the extent comes from the source — and a source that would
/// run off the bottom or the right is trimmed rather than refused, which is what a DEC terminal does
/// and the only reading that makes `CSI 2;1;24;80;1;1;1;1 $ v` (scroll a page up one row) work at all.
/// `None` when the corner itself is off the page.
pub fn copy_extent(
	source: Area,
	top: u16,
	left: u16,
	rows: usize,
	cols: usize,
) -> Option<(Area, usize, usize)> {
	if rows == 0 || cols == 0 {
		return None;
	}
	let corner = |value: u16| usize::from(value).max(1) - 1;
	let to_row = corner(top);
	let to_col = corner(left);
	if to_row >= rows || to_col >= cols {
		return None;
	}
	let height = source.height().min(rows - to_row);
	let width = source.width().min(cols - to_col);
	let trimmed = Area {
		top: source.top,
		left: source.left,
		bottom: source.top + height - 1,
		right: source.left + width - 1,
	};
	Some((trimmed, to_row, to_col))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read what it asked for.
	fn scan(bytes: &[u8]) -> Vec<(usize, Request)> {
		let mut rectangles = Rectangles::default();
		rectangles.feed(bytes)
	}

	/// The four corners, spelled out.
	fn box_of(top: u16, left: u16, bottom: u16, right: u16) -> Corners {
		Corners {
			top,
			left,
			bottom,
			right,
		}
	}

	#[test]
	fn an_erase_reads_its_four_corners() {
		// `\x1b[2;3;5;7$z` is 11 bytes, so the offset is 11 — one PAST the final `z`.
		assert_eq!(
			scan(b"\x1b[2;3;5;7$z"),
			vec![(11, Request::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn a_selective_erase_is_the_same_rectangle_by_another_verb() {
		assert_eq!(
			scan(b"\x1b[2;3;5;7${"),
			vec![(11, Request::SelectiveErase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn an_erase_with_no_parameters_is_the_whole_page() {
		// Every corner defaults to its edge, which is how a program spells "all of it".
		assert_eq!(
			scan(b"\x1b[$z"),
			vec![(4, Request::Erase(box_of(0, 0, 0, 0)))]
		);
	}

	#[test]
	fn a_fill_reads_the_character_before_the_corners() {
		// 45 is `-`: rule a line across a box.
		assert_eq!(
			scan(b"\x1b[45;2;1;2;80$x"),
			vec![(15, Request::Fill('-', box_of(2, 1, 2, 80)))]
		);
	}

	#[test]
	fn a_fill_takes_printable_latin_one_too() {
		// 176 is `°`. The high range is the other half of what xterm allows.
		assert_eq!(
			scan(b"\x1b[176;1;1;1;1$x"),
			vec![(15, Request::Fill('°', box_of(1, 1, 1, 1)))]
		);
	}

	#[test]
	fn a_fill_with_a_control_character_is_refused() {
		// The allow-list. A remote must not paint the page with C0, C1 or DEL — and refusing the
		// whole sequence is safer than filling with a substitute nobody asked for (§12).
		assert!(scan(b"\x1b[0;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[7;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[27;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[127;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[155;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[300;1;1;5;5$x").is_empty());
	}

	#[test]
	fn a_copy_reads_a_source_and_a_corner() {
		// The shape that scrolls a sub-window up by one row: copy rows 2–10 to row 1.
		assert_eq!(
			scan(b"\x1b[2;1;10;40;1;1;1;1$v"),
			vec![(
				21,
				Request::Copy {
					source: box_of(2, 1, 10, 40),
					top: 1,
					left: 1,
				}
			)]
		);
	}

	#[test]
	fn a_copy_may_leave_its_tail_off() {
		// Trailing parameters are omitted freely, and an omitted one reads as 0, which resolves to
		// the first row and column.
		assert_eq!(
			scan(b"\x1b[2;1;10;40$v"),
			vec![(
				13,
				Request::Copy {
					source: box_of(2, 1, 10, 40),
					top: 0,
					left: 0,
				}
			)]
		);
	}

	#[test]
	fn the_attribute_rectangles_are_not_claimed() {
		// DECCARA (`$ r`) and DECRARA (`$ t`) share the family's shape and change attributes rather
		// than content. Left to the engine, which drops them — a rectangle of SGR is its own job.
		assert!(scan(b"\x1b[1;1;5;5;7$r\x1b[1;1;5;5;7$t").is_empty());
	}

	#[test]
	fn a_request_mode_query_is_not_mistaken_for_one() {
		// DECRQM is the one `$` sequence the engine DOES answer, in both spellings. Claiming it here
		// would take an answer away from a program that asked a question.
		assert!(scan(b"\x1b[4$p\x1b[?69$p").is_empty());
	}

	#[test]
	fn the_intermediate_is_what_makes_the_family() {
		// Without the `$` these are ordinary sequences the engine handles: `CSI 5 x` is not a fill,
		// and `CSI 2 z` is nothing at all.
		assert!(scan(b"\x1b[45;2;1;2;80x\x1b[2;3;5;7z").is_empty());
	}

	#[test]
	fn a_private_marker_rules_the_sequence_out() {
		assert!(scan(b"\x1b[?2;3;5;7$z").is_empty());
	}

	#[test]
	fn a_malformed_parameter_drops_the_whole_sequence() {
		// A rectangle built from a misread corner erases the wrong cells, so a number that will not
		// parse is a no-op rather than a guess. `:` opens a sub-parameter, which none of these take.
		assert!(scan(b"\x1b[2:3;5;7$z").is_empty());
		assert!(scan(b"\x1b[99999;1;1;1$z").is_empty());
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		let mut rectangles = Rectangles::default();
		assert!(rectangles.feed(b"text\x1b").is_empty());
		assert!(rectangles.feed(b"[2;3;5").is_empty());
		// The offset counts from the start of the chunk that completed the sequence.
		assert_eq!(
			rectangles.feed(b";7$zmore"),
			vec![(4, Request::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn several_requests_in_one_chunk_come_out_in_order() {
		let requests = scan(b"\x1b[1;1;2;2$z\x1b[45;1;1;1;9$x");
		assert_eq!(
			requests,
			vec![
				(11, Request::Erase(box_of(1, 1, 2, 2))),
				(25, Request::Fill('-', box_of(1, 1, 1, 9))),
			]
		);
	}

	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		let mut params = vec![b'\x1b', b'['];
		params.extend(std::iter::repeat_n(b'1', MAX_PARAMS + 10));
		params.extend_from_slice(b"$z");
		assert!(scan(&params).is_empty());
	}

	#[test]
	fn a_control_byte_inside_a_csi_abandons_the_sequence() {
		assert!(scan(b"\x1b[2;3\n;5;7$z").is_empty());
	}

	#[test]
	fn ordinary_output_asks_for_nothing() {
		// The common case has to cost nothing: no request means `process` never splits its advance.
		assert!(scan(b"\x1b[31mred\x1b[0m\x1b[2J\x1b[1;1Hhello").is_empty());
	}

	#[test]
	fn corners_resolve_to_zero_based_cells() {
		assert_eq!(
			area(box_of(2, 3, 5, 7), 24, 80),
			Some(Area {
				top: 1,
				left: 2,
				bottom: 4,
				right: 6
			})
		);
	}

	#[test]
	fn omitted_corners_reach_the_edges_of_the_page() {
		assert_eq!(
			area(box_of(0, 0, 0, 0), 24, 80),
			Some(Area {
				top: 0,
				left: 0,
				bottom: 23,
				right: 79
			})
		);
	}

	#[test]
	fn corners_past_the_page_clamp_to_it() {
		// A program sized for a bigger screen gets the part of its rectangle that exists.
		assert_eq!(
			area(box_of(1, 1, 999, 999), 24, 80),
			Some(Area {
				top: 0,
				left: 0,
				bottom: 23,
				right: 79
			})
		);
	}

	#[test]
	fn a_rectangle_that_starts_off_the_page_holds_nothing() {
		assert_eq!(area(box_of(25, 1, 30, 5), 24, 80), None);
		assert_eq!(area(box_of(1, 81, 5, 90), 24, 80), None);
	}

	#[test]
	fn crossed_corners_hold_nothing() {
		// Bottom above top, or right left of left. DEC does nothing for these rather than swapping
		// them, and silently erasing a rectangle the program did not describe would be worse.
		assert_eq!(area(box_of(5, 1, 2, 9), 24, 80), None);
		assert_eq!(area(box_of(1, 9, 5, 2), 24, 80), None);
	}

	#[test]
	fn a_single_cell_is_a_rectangle() {
		assert_eq!(
			area(box_of(3, 4, 3, 4), 24, 80),
			Some(Area {
				top: 2,
				left: 3,
				bottom: 2,
				right: 3
			})
		);
	}

	#[test]
	fn a_page_with_no_cells_holds_no_rectangle() {
		assert_eq!(area(box_of(1, 1, 1, 1), 0, 80), None);
		assert_eq!(area(box_of(1, 1, 1, 1), 24, 0), None);
	}

	#[test]
	fn a_copy_that_fits_keeps_its_whole_source() {
		let source = area(box_of(2, 1, 10, 40), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 1, 1, 24, 80), Some((source, 0, 0)));
	}

	#[test]
	fn a_copy_running_off_the_page_is_trimmed_not_refused() {
		// Nine rows of source, but only three rows of room at the destination: the top three land and
		// the rest is dropped, which is what makes a scroll-by-copy work at the bottom of a page.
		let source = area(box_of(2, 1, 10, 40), 24, 80).expect("inside the page");
		let (trimmed, row, col) = copy_extent(source, 22, 79, 24, 80).expect("the corner is on it");
		assert_eq!(row, 21);
		assert_eq!(col, 78);
		assert_eq!(trimmed.height(), 3);
		assert_eq!(trimmed.width(), 2);
		assert_eq!(trimmed.top, source.top);
		assert_eq!(trimmed.left, source.left);
	}

	#[test]
	fn a_copy_to_a_corner_off_the_page_does_nothing() {
		let source = area(box_of(1, 1, 2, 2), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 25, 1, 24, 80), None);
		assert_eq!(copy_extent(source, 1, 81, 24, 80), None);
	}

	#[test]
	fn a_copy_corner_defaults_to_the_first_cell() {
		// An omitted destination reads as 0, and both 0 and 1 mean the first row or column.
		let source = area(box_of(3, 3, 4, 4), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 0, 0, 24, 80), Some((source, 0, 0)));
	}

	#[test]
	fn an_area_reports_its_own_size() {
		let source = area(box_of(2, 3, 5, 7), 24, 80).expect("inside the page");
		assert_eq!(source.height(), 4);
		assert_eq!(source.width(), 5);
		assert_eq!(source.rows().count(), 4);
		assert_eq!(source.columns().count(), 5);
	}
}

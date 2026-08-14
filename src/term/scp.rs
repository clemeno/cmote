// term/scp.rs — SCP, the direction a line's characters are laid down in (PLAN §76).
//
//   CSI Ps1 ; Ps2 SP k     SCP — select character path
//
// `Ps1` picks the path: 0 the implementation's default, 1 LEFT-TO-RIGHT, 2 RIGHT-TO-LEFT. `Ps2`
// says which of the terminal's two components drives the other — 0 implementation-dependent,
// 1 data to presentation, 2 presentation to data.
//
// That pair of components is the whole idea, and it is the reason this is buildable at all.
// ECMA-48 models a terminal as a DATA component (the characters, in the order they arrived) and a
// PRESENTATION component (where the glyphs are put). cmote already has exactly that split and did
// not know it: the engine's grid is the data, `ui/grid.rs` is the presentation, and every frame is
// already derived from the grid rather than stored. So a right-to-left character path is a rule
// about the derivation — column 0 of the data is drawn at the last column of the presentation — and
// nothing about the grid changes. The engine goes on being the only writer of its own state (§71,
// §73), the scrollback holds what the host sent, and a copy yields the characters in the order they
// arrived, which is what pasting them back somewhere else has to mean.
//
// `Ps2 = 2` is the one value refused. "Presentation to data" asks the terminal to write the drawing
// back into the grid, which is cmote writing engine state — the thing §71 and §73 both declined —
// and it would also destroy the only copy of what the host actually sent. It is a no-op here, and
// the row in the compatibility matrix says so.
//
// What this is NOT is bidi. There is no Unicode Bidirectional Algorithm in cmote and none is
// implied: ECMA-48 pairs SCP with BDSM (`CSI 8 h`, bidirectional support mode), whose default is
// EXPLICIT, and explicit means the sender has already put the characters in the order it wants them
// laid down. cmote implements the explicit half — the path — and not the implicit half, which is
// the algorithm. `vte`'s `NamedMode` names only `Insert = 4` and `LineFeedNewLine = 20`, so BDSM
// arrives as `Mode::Unknown(8)` and reaches nothing, which is consistent: cmote is in explicit mode
// and cannot be asked out of it.
//
// The path is held per LINE, keyed by the absolute document line (§40) — the scrollback-stable
// coordinate the prompt marks, the search hits and the selection all use. So a line keeps its
// direction as the screen scrolls under it, for free and for the same reason those do. Two things
// clear the store outright: RIS, which renumbers the document by dropping the history, and a swap
// on or off the alternate screen, where line numbering restarts against a page with no history.
//
// The mirror itself is one function used by both sides — `flip` below. The renderer calls it to
// decide where to draw a run, and the pointer path calls it to decide which cell was clicked, so the
// two directions cannot disagree about where a column went.

use std::collections::BTreeSet;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The longest parameter run buffered inside one sequence. SCP's is two small numbers; a longer run
/// is malformed, and refusing to grow past this keeps a hostile stream from ballooning our memory
/// (§12).
const MAX_PARAMS: usize = 32;

/// The most intermediate bytes buffered. SCP has exactly one, the space; the cap is for the same
/// reason as `MAX_PARAMS`.
const MAX_INTERMEDIATES: usize = 4;

/// How many lines may carry a remembered path at once. A bounded ring like the prompt marks' (§34),
/// and for the same reason: a remote decides how many of these arrive, so the store cannot be
/// allowed to grow with the session. Eviction takes the LOWEST line number, which is the one
/// furthest back in history and so the first to fall out of the scrollback anyway.
const MAX_LINES: usize = 4096;

/// The direction a line's characters are laid down in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Path {
	/// Column 0 of the data is drawn at the left. The power-on state, and what every line is until a
	/// program says otherwise.
	#[default]
	LeftToRight,
	/// Column 0 of the data is drawn at the RIGHT, and the line reads leftwards from there.
	RightToLeft,
}

/// Something the stream asked cmote to do about character paths, to be applied once the engine has
/// been advanced PAST the sequence that carried it (see `Scp::feed` on offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
	/// SCP — give the line the cursor is on this path.
	Select(Path),
	/// RIS (`ESC c`) — the engine rebuilds the whole terminal and drops the history with it, which
	/// renumbers every absolute line. A remembered path would then land on a different line than the
	/// one it was set for, so the store is emptied rather than carried across.
	Reset,
}

/// Where the scanner is in the byte stream. Only the CSI shape matters here, plus the one `ESC c`
/// that has to be noticed.
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

/// The character-path scanner (§76). Feed it every byte of shell output; it reports each SCP and
/// each RIS, with the offset to apply it at.
#[derive(Debug, Default)]
pub struct Scp {
	state: Scan,
	/// The private marker, if the sequence opened with one. SCP has none, so any marker rules it
	/// out; kept apart from `params` so the digits parse the same either way.
	marker: Option<u8>,
	params: Vec<u8>,
	intermediates: Vec<u8>,
}

impl Scp {
	/// Scan a chunk of shell output, returning what to do and where. Safe at any chunk boundary —
	/// the state machine carries over between calls, so a sequence may be split anywhere, even
	/// between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the selective erase (§56), the
	/// rectangles (§58) and the tab reset (§74). SCP names no coordinates of its own — it acts on
	/// "the line the cursor is on" — and the sequence moves nothing, so the cursor reads the same on
	/// either side of it; one-past keeps the rule uniform across the scanners that report something
	/// the engine ignored.
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
					// RIS. The engine rebuilds itself and drops the history, so every line this
					// store remembers is about to mean a different line.
					b'c' => {
						requests.push((index + 1, Request::Reset));
						self.state = Scan::Text;
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
					// Intermediate bytes — the space is SCP's.
					0x20..=0x2f => {
						self.intermediates.push(byte);
						if self.intermediates.len() > MAX_INTERMEDIATES {
							self.state = Scan::Text;
						}
					}
					// The final byte ends the sequence, so this is where it is judged.
					0x40..=0x7e => {
						if let Some(path) = self.path(byte) {
							requests.push((index + 1, Request::Select(path)));
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

	/// The path this sequence selects, or `None` when it is not an SCP cmote acts on.
	///
	/// Matching on final byte, marker and intermediates together is the near-miss rule §56 wrote
	/// down: `CSI Ps k` with no intermediate is something else entirely, and a private marker makes
	/// it a private sequence rather than this one.
	///
	/// Two values are refused rather than guessed at. A `Ps1` DEC never defined leaves the line
	/// alone, which is what `vte` does with its own out-of-range parameters. And `Ps2 = 2`,
	/// "presentation to data", asks cmote to write the drawing back into the grid — engine state
	/// cmote does not write (§71, §73), and the only copy of what the host actually sent.
	fn path(&self, final_byte: u8) -> Option<Path> {
		if !matches!(
			(final_byte, self.marker, self.intermediates.as_slice()),
			(b'k', None, [b' '])
		) {
			return None;
		}
		let path = match self.param(0)? {
			0 | 1 => Path::LeftToRight,
			2 => Path::RightToLeft,
			_ => return None,
		};
		match self.param(1)? {
			// Implementation-dependent, and data to presentation. Both describe what cmote does
			// every frame regardless: the drawing is derived from the grid.
			0 | 1 => Some(path),
			_ => None,
		}
	}

	/// Parameter `index` as a number, treating an omitted one as 0 — which is what `vte` does with
	/// both of SCP's, and 0 is a value each of them defines. `None` when the digits are unparseable,
	/// which leaves the sequence unclassified rather than guessing at it (§54's rule: malformed
	/// remote input is a no-op, never a reset).
	fn param(&self, index: usize) -> Option<u16> {
		let digits = self.params.split(|&byte| byte == b';').nth(index);
		let Some(digits) = digits else {
			return Some(0);
		};
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

/// Which lines are being drawn right to left, by absolute document line (§40).
///
/// Sparse on purpose: left-to-right is the default and almost every line, so the store holds only
/// the exceptions and a session that never sends SCP allocates nothing at all. Ordered rather than
/// hashed so eviction can take the lowest line number — the one furthest back in history — without
/// a scan.
#[derive(Debug, Default)]
pub struct Paths {
	rtl: BTreeSet<u64>,
}

impl Paths {
	/// Give one document line a character path.
	pub fn select(&mut self, line: u64, path: Path) {
		match path {
			Path::LeftToRight => {
				self.rtl.remove(&line);
			}
			Path::RightToLeft => {
				self.rtl.insert(line);
				// Bounded like the prompt marks' ring (§34). The lowest line number is the oldest,
				// so this drops history the viewport can no longer reach before it drops anything
				// on screen.
				while self.rtl.len() > MAX_LINES {
					self.rtl.pop_first();
				}
			}
		}
	}

	/// Whether this document line is drawn right to left.
	pub fn is_rtl(&self, line: u64) -> bool {
		self.rtl.contains(&line)
	}

	/// Forget every path. RIS renumbers the document by dropping the history, and a swap on or off
	/// the alternate screen restarts numbering against a page that keeps none — in both cases a
	/// remembered line number would now name different text.
	pub fn clear(&mut self) {
		self.rtl.clear();
	}
}

/// The mirror, in one place so the two directions cannot disagree.
///
/// Turns a data column into the presentation column it is drawn at, and — being its own inverse —
/// turns the column a pointer landed on back into the data column that was clicked. `cols` is the
/// page's width, so a right-to-left line puts column 0 hard against the right edge, which is what a
/// character path running that way means: the first character sits where the reader starts.
pub fn flip(col: u16, cols: u16) -> u16 {
	cols.saturating_sub(1).saturating_sub(col)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go.
	fn scan(bytes: &[u8]) -> Vec<(usize, Request)> {
		Scp::default().feed(bytes)
	}

	/// The three paths, and the offset each is reported at: ONE PAST the final byte.
	#[test]
	fn the_three_paths_are_recognised() {
		assert_eq!(
			scan(b"\x1b[2 k"),
			vec![(5, Request::Select(Path::RightToLeft))]
		);
		assert_eq!(
			scan(b"\x1b[1 k"),
			vec![(5, Request::Select(Path::LeftToRight))]
		);
		// An omitted or explicit 0 is the implementation's default, which here is left to right.
		assert_eq!(
			scan(b"\x1b[0 k"),
			vec![(5, Request::Select(Path::LeftToRight))]
		);
		assert_eq!(
			scan(b"\x1b[ k"),
			vec![(4, Request::Select(Path::LeftToRight))]
		);
	}

	/// The second parameter is the one cmote reads and refuses on. 0 and 1 both say the drawing is
	/// derived from the data, which is what cmote does; 2 asks for the reverse.
	#[test]
	fn the_update_mode_that_would_rewrite_the_grid_is_refused() {
		assert_eq!(
			scan(b"\x1b[2;1 k"),
			vec![(7, Request::Select(Path::RightToLeft))]
		);
		assert_eq!(
			scan(b"\x1b[2;0 k"),
			vec![(7, Request::Select(Path::RightToLeft))]
		);
		assert!(scan(b"\x1b[2;2 k").is_empty(), "presentation to data");
	}

	/// A value DEC never defined leaves the line alone rather than being rounded to one that is
	/// defined — the same thing `vte` does with its own out-of-range parameters.
	#[test]
	fn an_undefined_path_is_a_no_op() {
		assert!(scan(b"\x1b[3 k").is_empty());
		assert!(scan(b"\x1b[99 k").is_empty());
	}

	/// The intermediate is the whole test on the shape: `CSI Ps k` without it is a different
	/// sequence, and a private marker makes it a private one.
	#[test]
	fn the_space_intermediate_is_required() {
		assert!(scan(b"\x1b[2k").is_empty());
		assert!(scan(b"\x1b[?2 k").is_empty());
		assert!(scan(b"\x1b[2 q").is_empty(), "a different final byte");
	}

	/// RIS empties the store, because it drops the history and so renumbers every line.
	#[test]
	fn a_full_reset_is_reported() {
		assert_eq!(scan(b"\x1bc"), vec![(2, Request::Reset)]);
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to
	/// carry across a boundary drawn anywhere — including between the ESC and the `[`.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scp = Scp::default();
		assert!(scp.feed(b"\x1b").is_empty());
		assert!(scp.feed(b"[2").is_empty());
		assert!(scp.feed(b" ").is_empty());
		assert_eq!(
			scp.feed(b"k"),
			vec![(1, Request::Select(Path::RightToLeft))]
		);
	}

	/// A control byte inside a CSI abandons the sequence rather than extending it.
	#[test]
	fn a_control_byte_abandons_the_sequence() {
		assert!(scan(b"\x1b[2\x07 k").is_empty());
	}

	/// A hostile stream must not be able to make the scanner buffer without bound.
	#[test]
	fn a_runaway_parameter_run_is_abandoned() {
		let mut bytes = b"\x1b[".to_vec();
		bytes.extend(std::iter::repeat_n(b'2', MAX_PARAMS + 10));
		bytes.extend_from_slice(b" k");
		assert!(scan(&bytes).is_empty());
	}

	/// The store: an exception list over a left-to-right default, and setting a line back to
	/// left-to-right removes it rather than recording a second kind of entry.
	#[test]
	fn a_path_sticks_to_its_line_and_lets_go() {
		let mut paths = Paths::default();
		assert!(!paths.is_rtl(7), "left to right until told otherwise");
		paths.select(7, Path::RightToLeft);
		assert!(paths.is_rtl(7));
		assert!(!paths.is_rtl(8), "its neighbour is untouched");
		paths.select(7, Path::LeftToRight);
		assert!(!paths.is_rtl(7));
	}

	/// A remote decides how many of these arrive, so the store is a bounded ring — and what it drops
	/// is the lowest line number, which is the one furthest back in history.
	#[test]
	fn the_store_is_bounded_and_drops_the_oldest_line() {
		let mut paths = Paths::default();
		for line in 0..(MAX_LINES as u64 + 100) {
			paths.select(line, Path::RightToLeft);
		}
		assert!(!paths.is_rtl(0), "the oldest lines are the ones evicted");
		assert!(!paths.is_rtl(99));
		assert!(paths.is_rtl(100), "and the newest are all still there");
		assert!(paths.is_rtl(MAX_LINES as u64 + 99));
	}

	/// Both resets empty it, because both renumber what a line index means.
	#[test]
	fn clearing_forgets_every_line() {
		let mut paths = Paths::default();
		paths.select(3, Path::RightToLeft);
		paths.clear();
		assert!(!paths.is_rtl(3));
	}

	/// The mirror is its own inverse, which is the property that lets the renderer and the pointer
	/// share one function: whatever column a glyph is drawn at, clicking there names the column it
	/// was drawn from.
	#[test]
	fn the_mirror_is_its_own_inverse() {
		let cols = 80;
		for col in 0..cols {
			assert_eq!(flip(flip(col, cols), cols), col);
		}
		// The ends swap, which is the whole point: data column 0 draws at the right edge.
		assert_eq!(flip(0, 80), 79);
		assert_eq!(flip(79, 80), 0);
	}

	/// A degenerate page must not panic or wrap around.
	#[test]
	fn a_page_with_no_columns_flips_to_nothing() {
		assert_eq!(flip(0, 0), 0);
		assert_eq!(flip(5, 1), 0);
	}
}

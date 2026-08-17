// term/margins.rs — the left and right margins a program sets, and the arithmetic they imply (§102).
//
// DECSLRM (`CSI Pl ; Pr s`) is the horizontal half of DECSTBM. Where the vertical region walls off
// the ROWS that scrolling may move, the margins wall off the COLUMNS — and everything that follows
// from that is a consequence rather than a separate feature:
//
//   a line breaks at the right margin instead of at the screen edge
//   a carriage return goes to the left margin instead of to column 1
//   an insert or delete of characters stops at the margins
//   a scroll — SU, SD, IL, DL, IND, RI — moves only the band of columns between them
//
// The point of the sequence is a page split into columns that scroll independently, without the
// program redrawing the whole screen for every line. Its terminfo capabilities are `smglr`, `smglp`,
// `smgrp` and `mgc`, all four declared by `xterm-256color`, none of them in an init or reset string:
// margins go out when a program deliberately asks for them and never by accident (§73 checked this
// against the terminfo rather than asserting it).
//
// THE MODE IS THE WHOLE RULE, AND THAT IS NEW
//
// `CSI s` has two meanings on one final byte:
//
//   CSI s            SCOSC — save the cursor position (ANSI.SYS, universal)
//   CSI Pl ; Pr s    DECSLRM — set the margins (VT420)
//
// A real terminal tells them apart by a MODE: the byte means margins only while DECLRMM
// (`CSI ? 69 h`) is set, and means save-cursor otherwise. §57 could not use that rule, because the
// engine refuses mode 69 outright and answers a DECRQM for it with "not recognised" — so the only
// evidence left in the bytes was the parameter count, and cmote cancelled every parametrised `s` to
// keep a margin request from silently eating the program's one saved-cursor slot.
//
// cmote holds mode 69 itself now, so the real rule is back and the guess is retired:
//
//   mode 69 SET      the byte is DECSLRM. cmote applies the margins and still cancels the byte, so
//                    the engine never dispatches its half-blind `('s', []) => save_cursor_position`.
//   mode 69 RESET    the byte is SCOSC. cmote leaves it alone and the engine saves the cursor —
//                    which is what a real xterm does with it, parameters and all.
//
// That is not a loosening. §57's harm was a margin request costing a program its saved cursor, and
// the terminfo above is the proof it cannot happen: every one of the four margin capabilities SETS
// MODE 69 FIRST (`smglr=\E[?69h\E[%i%p1%d;%p2%ds`). A program that means margins says so.
//
// NARROWED, NOT MERELY ENABLED
//
// Everything below keys off `narrowed`, not off `enabled`. A program may set mode 69 and leave the
// margins at the page edges, and then the band IS the page: every operation cmote would take over
// would have to reproduce the engine's own behaviour exactly, including which rows reach the
// scrollback. Reproducing it is not the same as having it. So while the band spans the full width,
// cmote steps aside and the engine does what it has always done — no new code on the path that
// carries every ordinary session. The margins only become cmote's business once they actually
// exclude a column.
//
// WHAT A BAND SCROLL DOES NOT DO IS FILL THE SCROLLBACK
//
// A row pushed out of the top of a narrowed band is DISCARDED. It cannot go to the scrollback,
// because the scrollback holds whole lines and this row is only a slice of one — the columns
// outside the band belong to whatever else is on the page and are not scrolling at all. xterm does
// the same, and it is also the only answer that leaves the history readable: half-lines interleaved
// with whole ones would make every search, selection and copy downstream of it wrong.

/// The left and right margins, the mode that turns them on, and the deferred wrap that comes with
/// them (§102).
///
/// Both columns are zero-based and INCLUSIVE, unlike the vertical region beside it (`term/region.rs`
/// mirrors the engine's half-open range because it has to match the engine's arithmetic; this one is
/// cmote's own, so it is written the way it is used).
#[derive(Debug, Clone, Copy)]
pub struct Margins {
	/// DECLRMM, private mode 69. Nothing here means anything until a program sets it.
	enabled: bool,
	/// First column of the band, inclusive.
	left: usize,
	/// Last column of the band, inclusive.
	right: usize,
	/// The wrap that has been earned but not yet taken: the cursor is sitting ON the right margin
	/// with its cell already written, and the next graphic character goes to the left margin one row
	/// down. This is the engine's `input_needs_wrap` in cmote's hands — see `term/gate.rs` for why it
	/// cannot be left in the engine's, which is that the engine would wrap to column 0.
	pending_wrap: bool,
	/// The deferred wrap as DECSC saved it, so a save-and-restore around a wrapped line does not
	/// silently lose the wrap. The margins themselves are NOT saved by DECSC — they are not cursor
	/// state, and no terminal saves them there.
	saved_pending_wrap: bool,
}

impl Default for Margins {
	/// Power-on: no mode, no margins, no deferred wrap. The columns are meaningless while `enabled`
	/// is false and are given the degenerate band rather than a page width this type does not know.
	fn default() -> Self {
		Self {
			enabled: false,
			left: 0,
			right: 0,
			pending_wrap: false,
			saved_pending_wrap: false,
		}
	}
}

impl Margins {
	/// DECLRMM set or reset — private mode 69 (§102).
	///
	/// Turning the mode OFF throws the margins away, so a later `CSI ? 69 h` starts from the whole
	/// page rather than from whatever the last program left behind. Turning it ON sets the band to
	/// the whole page, which is what a VT420 does: the mode enables margins, DECSLRM places them, and
	/// between the two the band is everything.
	pub fn enable(&mut self, on: bool, cols: usize) {
		self.enabled = on;
		self.left = 0;
		self.right = cols.saturating_sub(1);
		self.pending_wrap = false;
	}

	/// Apply a DECSLRM. Returns whether the request was taken, since a rejected one must leave both
	/// the margins and the cursor exactly where they were.
	///
	/// `left` and `right` are the sequence's two parameters, one-based, `None` when omitted. A
	/// parameter written as an explicit `0` is read as omitted, which is how the engine reads
	/// DECSTBM's second parameter and how DEC defines a missing one: the page edge.
	///
	/// The test is xterm's — the left margin must be strictly left of the right one, and the right is
	/// clamped to the last column. A band of one column is therefore refused rather than accepted:
	/// there is no useful reading of a page whose text may occupy a single column, and a program that
	/// asks for one has miscounted.
	pub fn set(&mut self, left: Option<u16>, right: Option<u16>, cols: usize) -> bool {
		if !self.enabled || cols == 0 {
			return false;
		}
		let last = cols - 1;
		let left = left.filter(|value| *value != 0);
		let right = right.filter(|value| *value != 0);
		let left = left.map_or(0, |value| usize::from(value) - 1).min(last);
		let right = right.map_or(last, |value| usize::from(value) - 1).min(last);
		if left >= right {
			return false;
		}
		self.left = left;
		self.right = right;
		self.pending_wrap = false;
		true
	}

	/// Everything off — RIS, a soft reset, and every resize.
	///
	/// A resize is in that list because the page it was measured against is gone: a band of columns
	/// 10 to 40 means nothing on a window that is now 30 columns wide, and xterm drops margins on
	/// resize for the same reason. Reflow makes it worse than arbitrary, since the text those columns
	/// held has moved.
	pub fn reset(&mut self) {
		*self = Self::default();
	}

	/// Whether the margins actually exclude a column, which is the only state in which cmote takes
	/// any operation over from the engine. See the module header.
	pub fn narrowed(&self, cols: usize) -> bool {
		self.enabled && (self.left > 0 || self.right + 1 < cols)
	}

	/// Whether mode 69 is set, which is what DECRQM reports — a program may have the mode on with the
	/// band at the page edges, and asking is how it finds out the mode took.
	pub fn enabled(&self) -> bool {
		self.enabled
	}

	/// First column of the band, inclusive.
	pub fn left(&self) -> usize {
		self.left
	}

	/// Last column of the band, inclusive.
	pub fn right(&self) -> usize {
		self.right
	}

	/// The band an operation should act on, which is the whole page when there are no margins.
	///
	/// The gate never needs this — it steps aside entirely while the band is the page — but DECIC and
	/// DECDC do: they are defined in terms of the margins and are legal without them, so "no margins"
	/// has to resolve to a band rather than to a refusal (§102).
	pub fn band(&self, cols: usize) -> (usize, usize) {
		if self.narrowed(cols) {
			(self.left, self.right)
		} else {
			(0, cols.saturating_sub(1))
		}
	}

	/// Whether a wrap has been earned and not yet taken.
	pub fn pending_wrap(&self) -> bool {
		self.pending_wrap
	}

	/// Record, or clear, the deferred wrap.
	pub fn set_pending_wrap(&mut self, pending: bool) {
		self.pending_wrap = pending;
	}

	/// DECSC — carry the deferred wrap into the saved cursor.
	pub fn save(&mut self) {
		self.saved_pending_wrap = self.pending_wrap;
	}

	/// DECRC — take it back out again.
	pub fn restore(&mut self) {
		self.pending_wrap = self.saved_pending_wrap;
	}

	/// Where a column named by a program lands, given origin mode (§102).
	///
	/// Under DECOM the columns a program writes are counted from the LEFT MARGIN and cannot reach
	/// past the right one — the same relationship origin mode already has with the vertical region,
	/// which the engine implements for rows and, having no margins, never had to implement for
	/// columns. Without origin mode the column is absolute and the margins do not confine it, which
	/// is DEC's rule and the one that lets a program address a status area outside its own band.
	pub fn place(&self, column: usize, origin: bool, cols: usize) -> usize {
		if origin && self.narrowed(cols) {
			(self.left + column).min(self.right)
		} else {
			column.min(cols.saturating_sub(1))
		}
	}

	/// How far right a cursor at `column` may travel — CUF, and the tab stops.
	///
	/// A cursor already OUTSIDE the band to the right is not dragged back into it; the margin bounds
	/// motion that starts inside, and a program addressing a column past the band keeps the whole
	/// page. Same rule mirrored in `backstop`.
	pub fn forward_stop(&self, column: usize, cols: usize) -> usize {
		if self.narrowed(cols) && column <= self.right {
			self.right
		} else {
			cols.saturating_sub(1)
		}
	}

	/// How far left a cursor at `column` may travel — CUB, and the backspace.
	pub fn backstop(&self, column: usize, cols: usize) -> usize {
		if self.narrowed(cols) && column >= self.left {
			self.left
		} else {
			0
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The page width every test below uses.
	const COLS: usize = 80;

	/// A page with mode 69 set and the margins where the arguments say, for the tests that care about
	/// the band rather than about how it was set.
	fn band(left: u16, right: u16) -> Margins {
		let mut margins = Margins::default();
		margins.enable(true, COLS);
		assert!(margins.set(Some(left), Some(right), COLS));
		margins
	}

	#[test]
	fn margins_mean_nothing_until_the_mode_is_set() {
		// The sequence alone does not arm them. This is the rule that lets a parametrised `CSI s` go
		// on meaning save-cursor for every program that never asked for margins.
		let mut margins = Margins::default();
		assert!(!margins.set(Some(10), Some(40), COLS));
		assert!(!margins.narrowed(COLS));
	}

	#[test]
	fn the_mode_alone_opens_the_band_to_the_whole_page() {
		// Enabled but not narrowed: cmote steps aside and the engine keeps every operation.
		let mut margins = Margins::default();
		margins.enable(true, COLS);
		assert!(margins.enabled());
		assert!(!margins.narrowed(COLS));
		assert_eq!((margins.left(), margins.right()), (0, COLS - 1));
	}

	#[test]
	fn a_margin_pair_is_one_based_on_the_wire_and_zero_based_here() {
		let margins = band(11, 40);
		assert_eq!((margins.left(), margins.right()), (10, 39));
		assert!(margins.narrowed(COLS));
	}

	#[test]
	fn an_omitted_parameter_means_the_page_edge() {
		let mut margins = Margins::default();
		margins.enable(true, COLS);
		assert!(margins.set(Some(11), None, COLS));
		assert_eq!((margins.left(), margins.right()), (10, COLS - 1));
		assert!(margins.set(None, Some(40), COLS));
		assert_eq!((margins.left(), margins.right()), (0, 39));
	}

	#[test]
	fn a_parameter_written_as_zero_is_read_as_omitted() {
		// DEC's rule for a missing parameter, and the reading the engine already gives DECSTBM's
		// second one.
		let mut margins = Margins::default();
		margins.enable(true, COLS);
		assert!(margins.set(Some(0), Some(40), COLS));
		assert_eq!(margins.left(), 0);
	}

	#[test]
	fn a_backwards_or_degenerate_band_is_refused() {
		// Refused, not clamped: the margins keep whatever they had, exactly as a rejected DECSTBM
		// leaves the scrolling region alone.
		let mut margins = band(11, 40);
		assert!(!margins.set(Some(40), Some(11), COLS));
		assert!(!margins.set(Some(20), Some(20), COLS));
		assert_eq!((margins.left(), margins.right()), (10, 39));
	}

	#[test]
	fn a_right_margin_past_the_page_is_clamped_to_it() {
		let margins = band(11, 999);
		assert_eq!(margins.right(), COLS - 1);
	}

	#[test]
	fn a_band_spanning_the_whole_page_is_not_narrowed() {
		// Set explicitly to the edges, which a program may well do to cancel a previous band without
		// leaving the mode. Nothing is taken over from the engine in that state.
		let margins = band(1, COLS as u16);
		assert!(margins.enabled());
		assert!(!margins.narrowed(COLS));
	}

	#[test]
	fn turning_the_mode_off_throws_the_band_away() {
		let mut margins = band(11, 40);
		margins.enable(false, COLS);
		assert!(!margins.narrowed(COLS));
		// And back on again starts from the whole page, not from the old band.
		margins.enable(true, COLS);
		assert_eq!((margins.left(), margins.right()), (0, COLS - 1));
	}

	#[test]
	fn origin_mode_counts_columns_from_the_left_margin() {
		let margins = band(11, 40);
		assert_eq!(
			margins.place(0, true, COLS),
			10,
			"column 1 is the left margin"
		);
		assert_eq!(margins.place(5, true, COLS), 15);
		// And cannot reach past the right one, however large the number.
		assert_eq!(margins.place(500, true, COLS), 39);
	}

	#[test]
	fn without_origin_mode_a_column_is_absolute_and_not_confined() {
		// The rule that lets a program address a status area outside its own band.
		let margins = band(11, 40);
		assert_eq!(margins.place(0, false, COLS), 0);
		assert_eq!(margins.place(70, false, COLS), 70);
		assert_eq!(
			margins.place(500, false, COLS),
			COLS - 1,
			"still on the page"
		);
	}

	#[test]
	fn a_cursor_inside_the_band_stops_at_the_margins() {
		let margins = band(11, 40);
		assert_eq!(margins.forward_stop(20, COLS), 39);
		assert_eq!(margins.backstop(20, COLS), 10);
	}

	#[test]
	fn a_cursor_outside_the_band_keeps_the_whole_page() {
		// Motion that starts outside is not dragged in — the margins bound motion within them, they
		// do not capture a cursor a program deliberately parked elsewhere.
		let margins = band(11, 40);
		assert_eq!(
			margins.forward_stop(60, COLS),
			COLS - 1,
			"right of the band"
		);
		assert_eq!(margins.backstop(5, COLS), 0, "left of the band");
	}

	#[test]
	fn the_deferred_wrap_rides_the_saved_cursor() {
		let mut margins = band(11, 40);
		margins.set_pending_wrap(true);
		margins.save();
		margins.set_pending_wrap(false);
		margins.restore();
		assert!(margins.pending_wrap());
	}

	#[test]
	fn a_reset_takes_the_mode_with_it() {
		let mut margins = band(11, 40);
		margins.set_pending_wrap(true);
		margins.reset();
		assert!(!margins.enabled());
		assert!(!margins.narrowed(COLS));
		assert!(!margins.pending_wrap());
	}
}

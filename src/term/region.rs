// term/region.rs — the engine's vertical scrolling region, mirrored (PLAN §102).
//
// DECSTBM (`CSI Pt ; Pb r`) sets the band of rows that scrolling is confined to, and the engine
// implements it fully: every scroll, index and reverse-index it performs honours the band. What the
// engine does NOT do is let anyone read it back. `Term::scroll_region` is a private field, there is
// no accessor, and the only sequence that reports it — DECRQSS for `r` — reaches an arm the engine
// leaves at its empty default.
//
// That gap has cost cmote something twice already:
//
//   §100 (SL / SR)  A sideways shift ought to stop at the edges of a scrolling region. Unable to
//                   read the region, cmote used DECOM as a proxy for "a region exists" and REFUSED
//                   the shift whenever origin mode was set — a refusal aimed at the wrong thing,
//                   since SL and SR name no coordinates and origin mode is not what bounds them.
//   §102 (margins)  A left/right margin is the horizontal half of the same object. A band scroll
//                   has to know both halves, so margins could not be built at all without this.
//
// The way round is not a guess and not a heuristic. It is that **every writer of that field is
// observable**, so a mirror kept beside it cannot drift. In `alacritty_terminal-0.26.0` the field is
// assigned in exactly four places:
//
//   term/mod.rs:420   `Term::new`      — the whole page
//   term/mod.rs:701   `Term::resize`   — back to the whole page
//   term/mod.rs:1843  `reset_state`    — back to the whole page (RIS)
//   term/mod.rs:2174  `set_scrolling_region` — the sequence itself
//
// The last two are `Handler` methods, which is to say they arrive through the gate (`term/gate.rs`)
// and cmote sees them happen. The first two are calls cmote itself makes — `Terminal::new` and
// `Terminal::resize` — so cmote knows when they happen without being told. Four writers, four of
// them observed: the mirror is exact by construction rather than by care.
//
// **What would break it** is a fifth writer arriving in a version bump — a new sequence, or a reset
// path that assigns the field without passing through `Handler`. Nothing catches that at build time,
// because the field is private and its type would not change. This is the one disclosed cost of the
// mirror, and it is why the arithmetic below is a transcription of the engine's rather than an
// improvement on it: if the two ever have to be compared by hand, they should read the same.
//
// SO THE NUMBERS ARE THE ENGINE'S NUMBERS, NOT TIDIER ONES.
//
// The engine keeps `scroll_region: Range<Line>` — a half-open range of signed row indices, `start`
// inclusive and `end` exclusive. Both halves are stored here in exactly that shape, signed and
// half-open, including the case that looks like a bug and is not: `CSI 0 ; 24 r` gives the engine
// `top = 0`, and `Line(top as i32 - 1)` is then `Line(-1)`, a start ABOVE the first row. cmote
// mirrors the -1 rather than clamping it, because a clamp here would make the mirror disagree with
// the engine about which rows scroll — which is the one thing the mirror exists to get right.

/// The vertical scrolling region as the engine holds it: half-open, `start` inclusive and `end`
/// exclusive, in signed row indices where row 0 is the top of the visible page (§102).
///
/// Constructed at the page size and then fed every write the engine takes, so it always answers what
/// the engine would answer if it could be asked. See the module header for why all four writers are
/// reachable and what would have to change for that to stop being true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
	/// First row of the band, inclusive. May be negative — see the module header.
	start: i32,
	/// One past the last row of the band, matching the engine's `Range<Line>`.
	end: i32,
}

impl Region {
	/// The whole page: what the engine starts at, and what it returns to on a reset or a resize.
	pub fn full(rows: usize) -> Self {
		Self {
			start: 0,
			end: rows as i32,
		}
	}

	/// Apply a DECSTBM exactly as `Term::set_scrolling_region` applies it (`term/mod.rs:2155`).
	///
	/// `top` is the sequence's first parameter, one-based, already defaulted to 1 by the parser;
	/// `bottom` is its second, one-based, `None` when omitted or written as zero. A request whose top
	/// is not above its bottom is REJECTED and leaves the region alone — the engine logs it and
	/// returns, so the mirror must return too, or the two would part company on malformed input.
	///
	/// The engine also homes the cursor after a successful set. That is not mirrored here because it
	/// is not region state: the gate lets the engine's own `set_scrolling_region` do it, and only
	/// corrects the COLUMN afterwards when margins are on (§102).
	pub fn set(&mut self, top: usize, bottom: Option<usize>, rows: usize) {
		let bottom = bottom.unwrap_or(rows);
		if top >= bottom {
			return;
		}
		let rows = rows as i32;
		self.start = (top as i32 - 1).min(rows);
		self.end = (bottom as i32).min(rows);
	}

	/// Back to the whole page — RIS (`reset_state`) and every resize.
	pub fn reset(&mut self, rows: usize) {
		*self = Self::full(rows);
	}

	/// First row of the band, clamped into the page so a caller walking rows cannot start above it.
	///
	/// The clamp is HERE and not in `set` on purpose: the stored numbers stay the engine's, and only
	/// a caller that is about to index the grid gets a safe version of them.
	pub fn first_row(&self) -> usize {
		self.start.max(0) as usize
	}

	/// Last row of the band, inclusive and clamped into the page.
	///
	/// `end` is exclusive and never below 1 for a region the engine accepted, but a saturating
	/// subtraction keeps a hand-built `Region` from wrapping the index round.
	pub fn last_row(&self) -> usize {
		(self.end - 1).max(0) as usize
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The page size every test below uses, so the numbers in them read as rows on a normal screen.
	const ROWS: usize = 24;

	#[test]
	fn a_fresh_region_is_the_whole_page() {
		let region = Region::full(ROWS);
		assert_eq!((region.first_row(), region.last_row()), (0, ROWS - 1));
	}

	#[test]
	fn a_scrolling_region_is_stored_as_the_engine_stores_it() {
		// `CSI 5 ; 20 r` — one-based and inclusive on the wire, so rows 4..=19 zero-based.
		let mut region = Region::full(ROWS);
		region.set(5, Some(20), ROWS);
		assert_eq!(region.first_row(), 4);
		assert_eq!(region.last_row(), 19);
		assert_ne!((region.first_row(), region.last_row()), (0, ROWS - 1));
	}

	#[test]
	fn an_omitted_bottom_means_the_last_row() {
		// `CSI 5 r` — the engine fills the bottom in from the page height.
		let mut region = Region::full(ROWS);
		region.set(5, None, ROWS);
		assert_eq!(region.first_row(), 4);
		assert_eq!(region.last_row(), ROWS - 1);
	}

	#[test]
	fn a_backwards_region_is_rejected_and_changes_nothing() {
		// The engine logs and returns, leaving the previous region in place. A mirror that instead
		// took the request would think the wrong rows scroll for as long as the region lasted.
		let mut region = Region::full(ROWS);
		region.set(5, Some(20), ROWS);
		region.set(20, Some(5), ROWS);
		assert_eq!(region.first_row(), 4);
		assert_eq!(region.last_row(), 19);
	}

	#[test]
	fn a_region_with_equal_ends_is_rejected() {
		// `top >= bottom` is the engine's test, so a one-row band written this way is refused rather
		// than being read as a band of one.
		let mut region = Region::full(ROWS);
		region.set(9, Some(9), ROWS);
		assert_eq!((region.first_row(), region.last_row()), (0, ROWS - 1));
	}

	#[test]
	fn a_zero_top_is_mirrored_as_the_engine_reads_it() {
		// `CSI 0 ; 24 r` puts the engine's start at Line(-1) — above the first row. Clamping it here
		// would be tidier and would make the mirror wrong, so the clamp lives in `first_row` instead
		// and the stored number stays the engine's.
		let mut region = Region::full(ROWS);
		region.set(0, Some(10), ROWS);
		assert_eq!(region.start, -1);
		assert_eq!(region.first_row(), 0);
		assert_eq!(region.last_row(), 9);
	}

	#[test]
	fn a_bottom_past_the_page_is_clamped_to_it() {
		// The engine takes the minimum with the page height on both ends.
		let mut region = Region::full(ROWS);
		region.set(3, Some(999), ROWS);
		assert_eq!(region.last_row(), ROWS - 1);
	}

	#[test]
	fn a_top_past_the_page_is_clamped_to_it() {
		// Nonsense in, but the mirror still has to land where the engine lands rather than panicking
		// or wrapping an index round.
		let mut region = Region::full(ROWS);
		region.set(999, Some(1000), ROWS);
		assert_eq!(region.first_row(), ROWS);
		assert_eq!(region.last_row(), ROWS - 1);
	}

	#[test]
	fn a_reset_puts_the_whole_page_back() {
		let mut region = Region::full(ROWS);
		region.set(5, Some(20), ROWS);
		region.reset(ROWS);
		assert_eq!((region.first_row(), region.last_row()), (0, ROWS - 1));
	}

	#[test]
	fn a_resize_to_a_taller_page_is_the_whole_of_the_new_one() {
		// The engine assigns the full range on resize, so a region set before it does not survive —
		// and the mirror must not survive either.
		let mut region = Region::full(ROWS);
		region.set(5, Some(20), ROWS);
		region.reset(50);
		assert_eq!(region.first_row(), 0);
		assert_eq!(region.last_row(), 49);
	}

	#[test]
	fn a_band_covering_the_page_exactly_still_reads_as_the_whole_page() {
		// `CSI 1 ; 24 r` on a 24-row page is the same band as no region at all, and the operations
		// the gate bounds should take the engine's path for it rather than cmote's.
		let mut region = Region::full(ROWS);
		region.set(1, Some(ROWS), ROWS);
		assert_eq!((region.first_row(), region.last_row()), (0, ROWS - 1));
	}
}

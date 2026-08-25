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

/// The DECST8C scanner (§74). Feed it every byte of shell output; it reports where each tab-stop
/// reset sat, for `term/mod.rs` to carry out.
///
/// The CSI grammar is [`csi::Framer`]'s (§111); what is left here is the one question that is this
/// module's own — whether a finished sequence is DECST8C.
#[derive(Debug, Default)]
pub struct Tabs {
	framer: super::csi::Framer,
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
		self.framer.feed(bytes, |span, csi| {
			if is_tab_reset(csi) {
				resets.push(span.past());
			}
		});
		resets
	}
}

/// Whether a finished sequence is DECST8C — `CSI ? 5 W`, the marker and the parameter both required.
///
/// Read straight off `vte`'s own arm (`('W', [b'?']) if next_param_or(0) == 5`), deliberately, so that
/// cmote and the engine agree on what the bytes are even though only one of them acts. The near misses
/// this keeps out: `CSI 5 W` with no marker is CTC, a different sequence entirely; `CSI ? W` and
/// `CSI ? 2 W` are DECST8C's own final byte carrying a value DEC never defined for it, and an
/// undefined value is a no-op rather than a guess (§54).
///
/// The omitted parameter is 0 — `vte`'s `next_param_or(0)` — and 0 is not 5, so a bare `CSI ? W`
/// matches nothing. That default lives here, at the one site that knows what DECST8C means by an
/// absent parameter, rather than in the framer (§111).
fn is_tab_reset(csi: &super::csi::Csi<'_>) -> bool {
	(csi.final_byte(), csi.marker(), csi.intermediates()) == (b'W', Some(b'?'), &[][..])
		&& csi.param(0).unwrap_or(0) == 5
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

/// cmote's mirror of the engine's private tab-stop table (§143).
///
/// The engine implements tab stops fully — HTS sets one, TBC clears one or all, a tab moves to the
/// next, and a resize keeps the table the width of the page — and then keeps the answer entirely to
/// itself: `Term::tabs` is a private field of a private type with no accessor and no reply arm. That
/// is `term/region.rs`'s situation word for word, and it gets the same answer: a mirror, kept here,
/// written at every point the engine is TOLD.
///
/// DECTABSR is the one reader (§143). Nothing else in cmote needs to know where a tab stop is — the
/// engine does the tabbing — so this exists solely to be reported, which is why it is a mirror and
/// not a second implementation: `put_tab` is still the engine's, and if the two ever disagreed the
/// engine would still be the one moving the cursor.
///
/// WHAT WRITES IT, all four observable and all four covered:
///
///   * HTS and TBC arrive as `Handler` calls, so the gate updates the mirror on the way past.
///   * RIS rebuilds the engine's table inside `Term::reset_state`, with no sequence to watch — the
///     gate catches it because the gate is where the engine is told to reset.
///   * A resize rebuilds it inside `Term::resize`, with no sequence and no `Handler` call at all;
///     `Terminal::resize` corrects the mirror there, exactly as it corrects the scrolling region.
///   * DECST8C feeds the engine TBC, HTS and CUF (§74), which are `Handler` calls like any others, so
///     the walk lands in the mirror for free rather than needing a fifth writer.
///
/// `Handler::set_tabs` — `CSI Ps W`, "a stop every Ps columns" — is deliberately NOT a writer, and
/// the reason is agreement rather than oversight: `alacritty_terminal` never overrides it, so the
/// engine's own table does not change either (§74). A mirror that acted on it would be the only thing
/// in the program that thought the stops had moved.
#[derive(Debug, Default)]
pub struct Stops {
	/// One flag per column, the width of the page — the engine's own shape, because every rule below
	/// is copied from the engine's own code and a different shape would mean translating each of them.
	columns: Vec<bool>,
}

impl Stops {
	/// The power-on table: a stop every [`INTERVAL`] columns, counting from column 0.
	///
	/// `TabStops::new` is `(0..columns).map(|i| i % INITIAL_TABSTOPS == 0)`, and `INITIAL_TABSTOPS` is
	/// 8 — the same number [`INTERVAL`] holds. There is no way to read that constant out of the crate,
	/// so the tests below assert the number instead, with its source, the way `term/csi.rs` pins the
	/// engine's parameter width: a version bump that moved it is then a conversation rather than a
	/// mirror that silently reports stops the engine does not have.
	pub fn new(columns: usize) -> Self {
		Self {
			columns: (0..columns)
				.map(|column| column.is_multiple_of(interval()))
				.collect(),
		}
	}

	/// HTS — a stop at `column`.
	pub fn set(&mut self, column: usize) {
		if let Some(stop) = self.columns.get_mut(column) {
			*stop = true;
		}
	}

	/// TBC 0 — the stop at `column`, if there is one.
	pub fn clear(&mut self, column: usize) {
		if let Some(stop) = self.columns.get_mut(column) {
			*stop = false;
		}
	}

	/// TBC 3 — every stop on the page.
	pub fn clear_all(&mut self) {
		self.columns.fill(false);
	}

	/// RIS — the table built afresh, which is what `Term::reset_state` does to its own.
	pub fn reset(&mut self, columns: usize) {
		*self = Self::new(columns);
	}

	/// A resize — the engine's own rule, which is NOT "build a fresh table".
	///
	/// `TabStops::resize` grows the vector with `index % 8 == 0` for each column ADDED and leaves
	/// every existing column exactly as it was, so a program's hand-set stops survive a widening.
	/// Copying that rather than calling [`Stops::new`] is the difference between a mirror and a guess:
	/// a fresh table would put the default stops back over a program's own the first time the user
	/// dragged the window wider.
	pub fn resize(&mut self, columns: usize) {
		let mut column = self.columns.len();
		self.columns.resize_with(columns, || {
			let stop = column.is_multiple_of(interval());
			column += 1;
			stop
		});
	}

	/// Every column that holds a stop, ascending and zero-based — what DECTABSR reports, one-based
	/// (§143).
	pub fn columns(&self) -> impl Iterator<Item = usize> + '_ {
		self.columns
			.iter()
			.enumerate()
			.filter_map(|(column, stop)| stop.then_some(column))
	}
}

/// [`INTERVAL`] as the column arithmetic above counts in.
///
/// A function rather than a second constant so there is one number: `INTERVAL` is a `u16` because the
/// sequences it is written into are, and the columns are `usize` because the engine's are.
fn interval() -> usize {
	INTERVAL as usize
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
		// The offset is into THIS chunk, which is where the interruption advance uses it.
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

	/// A hostile stream must not be able to make the scanner buffer without bound — and the two bounds
	/// answer differently on purpose, which is what this pins now that the grammar is shared (§111).
	#[test]
	fn the_two_parameter_bounds_answer_differently() {
		// More parameters than the engine's array holds: the engine ignores the whole sequence, so the
		// scanner abandons it too. Both sides ignoring the same bytes is agreement.
		let mut many = b"\x1b[?".to_vec();
		many.extend(std::iter::repeat_n(b';', super::super::csi::MAX_PARAMS + 1));
		many.push(b'W');
		assert!(scan(&many).is_empty());

		// A runaway DIGIT run is clamped instead, and the sequence lives — because the engine
		// saturates the number rather than giving up on the sequence. It is simply not DECST8C, since
		// the clamped value is not 5.
		let mut digits = b"\x1b[?".to_vec();
		digits.extend(std::iter::repeat_n(b'5', 500));
		digits.push(b'W');
		assert!(scan(&digits).is_empty());
	}

	/// Leading zeros do not change what a parameter means, so `CSI ? 000005 W` is DECST8C — which is
	/// what the engine's own saturating fold makes of it (§111).
	#[test]
	fn leading_zeros_still_read_as_a_tab_reset() {
		assert_eq!(scan(b"\x1b[?000000000000000005W"), vec![22]);
	}

	/// Two in one chunk, both reported, in stream order — the interruption advance walks them in the order
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

	// --- the mirror of the engine's private table (§143) ----------------------------------------

	/// Which columns hold a stop, as a list — the shape every test below compares.
	fn stops(stops: &Stops) -> Vec<usize> {
		stops.columns().collect()
	}

	/// The power-on table, and the number it is built from. `INITIAL_TABSTOPS` is `pub(crate)` to
	/// nobody, so this is the one place the value is asserted — the same arrangement `term/csi.rs`
	/// keeps for the engine's parameter width, and for the same reason: a crate bump that moved it
	/// would otherwise leave the mirror reporting stops the engine does not have.
	#[test]
	fn the_power_on_table_is_the_engines_own() {
		assert_eq!(INTERVAL, 8, "alacritty_terminal term/mod.rs:51");
		assert_eq!(stops(&Stops::new(20)), vec![0, 8, 16]);
		assert_eq!(stops(&Stops::new(8)), vec![0]);
		assert_eq!(stops(&Stops::new(0)), Vec::<usize>::new(), "a page of none");
	}

	#[test]
	fn a_stop_can_be_set_and_cleared_one_column_at_a_time() {
		let mut table = Stops::new(20);
		table.set(5);
		assert_eq!(stops(&table), vec![0, 5, 8, 16]);
		table.clear(8);
		assert_eq!(stops(&table), vec![0, 5, 16]);
		table.clear_all();
		assert_eq!(stops(&table), Vec::<usize>::new());
	}

	/// A column off the end of the page is not a stop and not a panic. It cannot arrive from the gate —
	/// the cursor is always on the grid — but a mirror that indexed blindly would turn a future
	/// off-by-one into a crash in a terminal rather than a wrong report.
	#[test]
	fn a_column_past_the_page_changes_nothing() {
		let mut table = Stops::new(8);
		table.set(99);
		table.clear(99);
		assert_eq!(stops(&table), vec![0]);
	}

	/// The rule that makes this a mirror rather than a guess: `TabStops::resize` GROWS the table,
	/// keeping every stop a program set and giving each column ADDED the default every eight. Putting
	/// a fresh table back instead would wipe a program's own stops the first time the user dragged
	/// the window wider.
	#[test]
	fn a_resize_keeps_the_stops_a_program_set() {
		let mut table = Stops::new(20);
		table.clear_all();
		table.set(5);
		table.resize(40);
		assert_eq!(
			stops(&table),
			vec![5, 24, 32],
			"the hand-set stop kept; columns 20..39 get the default"
		);
	}

	/// Shrinking drops the columns that are gone, which is what `Vec::resize_with` does to the
	/// engine's own vector — so a window made narrow and wide again comes back with the default
	/// pattern in the columns that were away, on both sides of the mirror.
	#[test]
	fn a_resize_narrower_drops_the_columns_that_left() {
		let mut table = Stops::new(40);
		table.set(30);
		table.resize(20);
		assert_eq!(stops(&table), vec![0, 8, 16]);
		table.resize(40);
		assert_eq!(stops(&table), vec![0, 8, 16, 24, 32], "and not 30");
	}

	/// RIS builds the table afresh, which is what `Term::reset_state` does to its own — the one place
	/// [`Stops::reset`] and [`Stops::resize`] must NOT be the same call.
	#[test]
	fn a_reset_builds_the_table_afresh() {
		let mut table = Stops::new(20);
		table.clear_all();
		table.set(5);
		table.reset(20);
		assert_eq!(stops(&table), vec![0, 8, 16]);
	}
}

// term/search.rs — finding text in the terminal's scrollback (PLAN §35).
//
// The engine retains SCROLLBACK lines of history (§23) and cmote can scroll through them, but
// until now the only way to find something up there was to scroll and read. This module is the
// pure, testable core of a find bar over the whole document (history + the live screen): the
// geometry of one grid row flattened for searching, the match list, and which match is current.
//
// It knows nothing about the engine, the widgets or the clipboard. `Terminal::find` builds a
// `SearchRow` per grid line and collects the hits; `app` holds a `Search` while the bar is open, and
// turns the current match into an ordinary text selection so the existing highlight and Copy
// paths serve it unchanged (the same tactic §34's select-command-output uses).
//
// Two coordinate choices carry the design:
//
//   * A match's line is an ABSOLUTE line index — `history_size + row` at scan time, the same
//     scrollback-stable coordinate the OSC 133 marks use (§34) — so a hit keeps pointing at its
//     text as new output pushes the viewport down, rather than at whatever later scrolled into
//     that viewport row.
//   * A match's columns are grid COLUMNS, not byte offsets, because that is what a selection
//     addresses. A row therefore carries a byte -> column map beside its text, which is also how
//     a double-width glyph's second cell (which holds no glyph of its own) is skipped without
//     the columns after it drifting.
//
// Because the lines are absolute, the list also serves the renderer: `Search::visible` resolves the
// hits that fall on the screen as it is scrolled right now into viewport rows (§39), and the grid
// washes every one of them — so the current hit is where you are, and the washes are where else the
// query is. That resolution is the only place the two coordinate spaces meet.

/// One found occurrence: the absolute line it sits on and the inclusive column span it covers
/// (§35). Inclusive because that is what a selection's head wants — `end_col` is the last cell of
/// the match, not one past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
	pub line: u64,
	pub start_col: u16,
	pub end_col: u16,
}

/// One match placed on the screen as it is scrolled RIGHT NOW (§39): the viewport row it occupies
/// (row 0 is the top visible line) and the inclusive column span it covers. A `SearchMatch` is stored in
/// document coordinates so it survives new output; this is that coordinate resolved against the
/// viewport for one frame, which is the only form a renderer can paint — and it is deliberately a
/// separate type, so an absolute line and a screen row can never be passed for one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHighlight {
	pub row: u16,
	pub start_col: u16,
	pub end_col: u16,
}

/// One grid row, flattened for searching (§35): its text plus, for every BYTE of that text, the
/// grid column the byte came from. The two are only ever grown together by `push`, so the map
/// cannot drift out of step with the text — the invariant is the reason this is a type and not a
/// pair of loose vectors.
///
/// The text is stored ASCII-lowered, since that is the only form searched (see `find`), and every
/// glyph is lowered as it arrives.
pub struct SearchRow {
	/// The absolute line this row is, stamped onto every match it yields.
	line: u64,
	/// The row's glyphs, ASCII-lowered, one after another with no padding between cells.
	text: String,
	/// `cols[b]` is the grid column that byte `b` of `text` came from. Same length as `text`.
	cols: Vec<u16>,
}

impl SearchRow {
	/// An empty row for absolute line `line`.
	pub fn new(line: u64) -> Self {
		Self {
			line,
			text: String::new(),
			cols: Vec::new(),
		}
	}

	/// Append the glyph at grid column `col`. Columns need not be consecutive: the caller skips a
	/// wide glyph's trailing cell (it carries no glyph of its own), and the map is what keeps the
	/// following columns correct anyway.
	pub fn push(&mut self, glyph: char, col: u16) {
		self.text.push(glyph.to_ascii_lowercase());
		// One map entry per byte the glyph took, so `cols[byte]` is total over the text and a
		// multi-byte glyph maps every one of its bytes to its own single column.
		self.cols.resize(self.text.len(), col);
	}

	/// Drop the row's trailing blank cells. A terminal pads every row to the full width, so
	/// without this a query of one space would "match" thousands of times over the padding and
	/// bury the real hits — the same reason a copy trims them (§10).
	pub fn trim_end(&mut self) {
		let trimmed = self.text.trim_end_matches(' ').len();
		self.text.truncate(trimmed);
		self.cols.truncate(trimmed);
	}

	/// Every occurrence of `query` in this row, left to right (§35). The search is ASCII
	/// case-insensitive — both sides are lowered with `to_ascii_lowercase`, which touches only
	/// `A`-`Z` and so preserves every byte offset, exactly the rule the editor's find follows
	/// (§32), so a non-ASCII case pair like `é`/`É` stays distinct. Matches do not overlap: each
	/// search resumes past the last hit. An empty query matches nothing.
	pub fn find(&self, query: &str) -> Vec<SearchMatch> {
		let mut out = Vec::new();
		if query.is_empty() {
			return out;
		}
		let needle = query.to_ascii_lowercase();
		let mut from = 0;
		// `find` respects UTF-8 boundaries and `from` only ever lands on a match end (a boundary),
		// so every slice below is valid.
		while let Some(rel) = self.text[from..].find(&needle) {
			let start = from + rel;
			let end = start + needle.len();
			// The map is per byte, so the match's first byte gives its first column and its LAST
			// byte its last — `end` is one past the match, hence `end - 1`. Both are in range: the
			// hit came out of `text`, and `cols` is as long as `text`.
			if let (Some(&start_col), Some(&end_col)) =
				(self.cols.get(start), self.cols.get(end - 1))
			{
				out.push(SearchMatch {
					line: self.line,
					start_col,
					end_col,
				});
			}
			from = end;
		}
		out
	}
}

/// The scrollback search's state while the bar is open (§35): the query, every match in document
/// order (oldest line first), and which one is current. `app` holds this as `Tab::search` and
/// drops it when the bar closes; the match list is rebuilt by `Terminal::find` whenever the query
/// changes or a step is taken, so it never has to be invalidated on new output.
#[derive(Debug, Default)]
pub struct Search {
	/// The text being searched for. Empty means no matches and an idle bar.
	pub query: String,
	/// Every match, in document order — oldest line first, left to right within a line.
	matches: Vec<SearchMatch>,
	/// Which match is current: the one revealed and selected. Zero and meaningless when
	/// `matches` is empty.
	current: usize,
}

impl Search {
	/// Take the results of a fresh scan for a NEW query (§35), landing on the NEWEST match — the
	/// last in document order. A terminal search almost always means "where did that last
	/// happen", and the newest hit is also the one nearest the live prompt the user is looking at,
	/// so it is the cheapest one to show.
	pub fn set_matches(&mut self, matches: Vec<SearchMatch>) {
		self.current = matches.len().saturating_sub(1);
		self.matches = matches;
	}

	/// Take the results of a re-scan of the SAME query (§35) — run before every step, so output
	/// that arrived since the query was typed joins the list. The current match is kept by
	/// identity (same line, same columns) wherever it survived the re-scan; when it did not (its
	/// line scrolled past the retained history, or a resize reflowed it away) the newest match is
	/// the fallback, the same place a new query lands.
	pub fn refresh(&mut self, matches: Vec<SearchMatch>) {
		let previous = self.current();
		self.current = match previous.and_then(|found| matches.iter().position(|m| *m == found)) {
			Some(index) => index,
			None => matches.len().saturating_sub(1),
		};
		self.matches = matches;
	}

	/// Step to the neighbouring match and return it (§35). `newer` walks toward the live prompt
	/// (down the screen), `!newer` back into history (up). Both wrap, so stepping past either end
	/// continues from the other rather than dead-ending. `None` only when there are no matches.
	pub fn step(&mut self, newer: bool) -> Option<SearchMatch> {
		let count = self.matches.len();
		if count == 0 {
			return None;
		}
		self.current = if newer {
			(self.current + 1) % count
		} else {
			// `+ count - 1` rather than `- 1`, so stepping back from the first wraps to the last
			// instead of underflowing.
			(self.current + count - 1) % count
		};
		self.current()
	}

	/// The current match, or `None` when the query has none.
	pub fn current(&self) -> Option<SearchMatch> {
		self.matches.get(self.current).copied()
	}

	/// Every match that falls on the visible screen right now, as viewport rows (§39) — what the
	/// renderer washes so ALL the hits show at once and the eye can see how the query is spread
	/// through the output, not only where the current hit is. The mapping is `absolute -
	/// history_size + display_offset`, the same one the prompt ticks use (§34), so scrolling the
	/// history moves the washes with the text they belong to and needs no re-scan.
	///
	/// The current match is INCLUDED. Revealing it already makes it an ordinary selection (§35) and
	/// the renderer lets the selection's fill win, so the current hit stands out from the rest
	/// without this having to know which one it is — and a user who drags a new selection over the
	/// grid still sees every hit washed, the current one among them.
	///
	/// The walk starts at the first visible line rather than at the top of the document: matches
	/// arrive in document order (`Terminal::find` walks the grid from the oldest line down), so a
	/// `partition_point` skips the history above the viewport in log time and the walk stops at the
	/// viewport's bottom. That matters because this runs on every frame the grid draws, and a query
	/// of one letter over a full scrollback has tens of thousands of hits — nearly all of them off
	/// screen. (An unsorted list could only *miss* a wash, never misplace one.)
	pub fn visible(
		&self,
		history_size: u16,
		display_offset: u16,
		screen_lines: u16,
	) -> Vec<SearchHighlight> {
		// The absolute line showing at viewport row 0: the top of the live screen, less however far
		// the viewport has climbed back into the retained history.
		let top = u64::from(history_size.saturating_sub(display_offset));
		let bottom = top + u64::from(screen_lines);
		let from = self.matches.partition_point(|found| found.line < top);
		self.matches[from..]
			.iter()
			.take_while(|found| found.line < bottom)
			.filter_map(|found| {
				Some(SearchHighlight {
					// In range by the two bounds above, so this cannot fail: `line - top` is below
					// `screen_lines`, which is itself a `u16`. Spelled as a `try_from` so that a bound
					// which stopped holding would drop the highlight rather than draw it on a row
					// wrapped round into the visible page (§111).
					row: u16::try_from(found.line - top).ok()?,
					start_col: found.start_col,
					end_col: found.end_col,
				})
			})
			.collect()
	}

	/// How many matches the query has right now — the denominator the bar shows.
	pub fn count(&self) -> usize {
		self.matches.len()
	}

	/// The current match's 1-based position for display ("3 / 12"), or `0` when there are none.
	pub fn ordinal(&self) -> usize {
		if self.matches.is_empty() {
			0
		} else {
			self.current + 1
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// A row of plain ASCII on absolute line `line`, one glyph per consecutive column — the
	// ordinary case, before any wide glyph or padding enters it.
	fn row(line: u64, text: &str) -> SearchRow {
		let mut row = SearchRow::new(line);
		for (col, glyph) in text.chars().enumerate() {
			row.push(
				glyph,
				u16::try_from(col).expect("a test fixture is a short line"),
			);
		}
		row
	}

	#[test]
	fn a_row_finds_every_occurrence_case_insensitively() {
		// "Error" and "error" both match "error", and the columns are the cells the hit covers.
		let row = row(7, "Error: no error here");
		assert_eq!(
			row.find("error"),
			vec![
				SearchMatch {
					line: 7,
					start_col: 0,
					end_col: 4
				},
				SearchMatch {
					line: 7,
					start_col: 10,
					end_col: 14
				},
			]
		);
	}

	#[test]
	fn matches_do_not_overlap_and_an_empty_query_finds_nothing() {
		// "aa" in "aaaa" is two non-overlapping hits (columns 0-1 and 2-3), not three.
		let row = row(0, "aaaa");
		assert_eq!(
			row.find("aa")
				.iter()
				.map(|m| m.start_col)
				.collect::<Vec<_>>(),
			vec![0, 2]
		);
		// An idle bar is idle, not "everything matches".
		assert!(row.find("").is_empty());
	}

	#[test]
	fn a_wide_glyph_leaves_the_later_columns_correct() {
		// 世 occupies columns 1-2, and its trailing cell holds no glyph — so the caller pushes
		// nothing for column 2. The byte -> column map is what keeps "b" at column 3 rather than
		// sliding to 2, which a plain "index == column" assumption would do.
		let mut row = SearchRow::new(3);
		row.push('a', 0);
		row.push('世', 1);
		row.push('b', 3);
		assert_eq!(
			row.find("b"),
			vec![SearchMatch {
				line: 3,
				start_col: 3,
				end_col: 3
			}]
		);
		// And a search that lands ON the wide glyph reports its lead column, once.
		assert_eq!(
			row.find("世"),
			vec![SearchMatch {
				line: 3,
				start_col: 1,
				end_col: 1
			}]
		);
	}

	#[test]
	fn trailing_blank_padding_is_trimmed_before_searching() {
		// A grid row is padded to the full width; a query of one space must not match the padding.
		let mut padded = row(0, "hi        ");
		padded.trim_end();
		assert!(padded.find(" ").is_empty());
		// An interior space is still real text, so it is still findable.
		let mut interior = row(0, "a b       ");
		interior.trim_end();
		assert_eq!(
			interior.find(" "),
			vec![SearchMatch {
				line: 0,
				start_col: 1,
				end_col: 1
			}]
		);
	}

	// Three matches on ascending lines, the shape every `Search` test starts from.
	fn three() -> Vec<SearchMatch> {
		vec![
			SearchMatch {
				line: 1,
				start_col: 0,
				end_col: 2,
			},
			SearchMatch {
				line: 5,
				start_col: 4,
				end_col: 6,
			},
			SearchMatch {
				line: 9,
				start_col: 0,
				end_col: 2,
			},
		]
	}

	#[test]
	fn a_new_query_lands_on_the_newest_match() {
		// The newest hit is the last in document order — nearest the live prompt (§35).
		let mut search = Search::default();
		search.set_matches(three());
		assert_eq!(search.current().map(|m| m.line), Some(9));
		assert_eq!(search.count(), 3);
		assert_eq!(search.ordinal(), 3);
	}

	#[test]
	fn stepping_wraps_in_both_directions() {
		let mut search = Search::default();
		search.set_matches(three());
		// Back into history from the newest, then past the oldest — which wraps to the newest.
		assert_eq!(search.step(false).map(|m| m.line), Some(5));
		assert_eq!(search.step(false).map(|m| m.line), Some(1));
		assert_eq!(search.step(false).map(|m| m.line), Some(9));
		// And forward, wrapping the other way.
		assert_eq!(search.step(true).map(|m| m.line), Some(1));
	}

	#[test]
	fn stepping_an_empty_result_set_does_nothing() {
		let mut search = Search::default();
		search.set_matches(Vec::new());
		assert_eq!(search.step(true), None);
		assert_eq!(search.ordinal(), 0);
	}

	#[test]
	fn a_rescan_keeps_the_current_match_by_identity() {
		// Arrange: sitting on the middle match (line 5).
		let mut search = Search::default();
		search.set_matches(three());
		search.step(false);
		assert_eq!(search.current().map(|m| m.line), Some(5));

		// Act: new output added a fourth match at the end; the earlier three are unchanged.
		let mut grown = three();
		grown.push(SearchMatch {
			line: 12,
			start_col: 0,
			end_col: 2,
		});
		search.refresh(grown);

		// Assert: still on line 5 — its index moved nowhere here, but the identity, not the
		// index, is what was matched. The new hit is in the count, so stepping can reach it.
		assert_eq!(search.current().map(|m| m.line), Some(5));
		assert_eq!(search.count(), 4);
		assert_eq!(search.step(true).map(|m| m.line), Some(9));
	}

	#[test]
	fn a_rescan_that_lost_the_current_match_falls_back_to_the_newest() {
		// The current match's line scrolled out of the retained history, so it is gone from the
		// re-scan: the fallback is the newest match, where a fresh query would land.
		let mut search = Search::default();
		search.set_matches(three());
		search.step(false);
		search.step(false);
		assert_eq!(search.current().map(|m| m.line), Some(1));

		search.refresh(three().into_iter().skip(1).collect());
		assert_eq!(search.current().map(|m| m.line), Some(9));
	}

	#[test]
	fn only_the_matches_on_screen_are_handed_to_the_renderer() {
		// Arrange: hits on absolute lines 1, 5 and 9, and a 4-row screen with 6 lines of history —
		// so at the live bottom the screen shows absolute 6..=9 and only the last hit is on it.
		let mut search = Search::default();
		search.set_matches(three());

		// Act + assert: the hit on line 9 lands on the bottom row (9 - 6), the other two are above
		// the viewport and are simply not there — the renderer is never asked to paint off screen.
		assert_eq!(
			search.visible(6, 0, 4),
			vec![SearchHighlight {
				row: 3,
				start_col: 0,
				end_col: 2
			}]
		);
	}

	#[test]
	fn scrolling_back_moves_the_highlights_with_their_text() {
		// The same three hits, the same 4-row screen, now scrolled 5 lines back into history: the
		// screen shows absolute 1..=4, so the hit on line 1 has arrived at the top row and the one
		// that was visible at the bottom has gone off the end. No re-scan was needed for either —
		// the matches are absolute, and this is the only thing that moved.
		let mut search = Search::default();
		search.set_matches(three());
		assert_eq!(
			search.visible(6, 5, 4),
			vec![SearchHighlight {
				row: 0,
				start_col: 0,
				end_col: 2
			}]
		);
		// A viewport somewhere in the middle catches the middle hit, at the row its line sits on.
		assert_eq!(
			search.visible(6, 2, 4),
			vec![SearchHighlight {
				row: 1,
				start_col: 4,
				end_col: 6
			}]
		);
	}

	#[test]
	fn an_idle_bar_highlights_nothing() {
		// No query, so no hits, so no washes — and a screen with no history is the ordinary case
		// that must not underflow the absolute -> row mapping.
		let search = Search::default();
		assert!(search.visible(0, 0, 24).is_empty());
	}
}

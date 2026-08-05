// term/search.rs — finding text in the terminal's scrollback (PLAN §35).
//
// The engine retains SCROLLBACK lines of history (§23) and cmote can scroll through them, but
// until now the only way to find something up there was to scroll and read. This module is the
// pure, testable core of a find bar over the whole document (history + the live screen): the
// geometry of one grid row flattened for searching, the match list, and which match is current.
//
// It knows nothing about the engine, the widgets or the clipboard. `Terminal::find` builds a
// `Row` per grid line and collects the hits; `app` holds a `Search` while the bar is open, and
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

/// One found occurrence: the absolute line it sits on and the inclusive column span it covers
/// (§35). Inclusive because that is what a selection's head wants — `end_col` is the last cell of
/// the match, not one past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
	pub line: u64,
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
pub struct Row {
	/// The absolute line this row is, stamped onto every match it yields.
	line: u64,
	/// The row's glyphs, ASCII-lowered, one after another with no padding between cells.
	text: String,
	/// `cols[b]` is the grid column that byte `b` of `text` came from. Same length as `text`.
	cols: Vec<u16>,
}

impl Row {
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
	pub fn find(&self, query: &str) -> Vec<Match> {
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
				out.push(Match {
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
	matches: Vec<Match>,
	/// Which match is current: the one revealed and selected. Zero and meaningless when
	/// `matches` is empty.
	current: usize,
}

impl Search {
	/// Take the results of a fresh scan for a NEW query (§35), landing on the NEWEST match — the
	/// last in document order. A terminal search almost always means "where did that last
	/// happen", and the newest hit is also the one nearest the live prompt the user is looking at,
	/// so it is the cheapest one to show.
	pub fn set_matches(&mut self, matches: Vec<Match>) {
		self.current = matches.len().saturating_sub(1);
		self.matches = matches;
	}

	/// Take the results of a re-scan of the SAME query (§35) — run before every step, so output
	/// that arrived since the query was typed joins the list. The current match is kept by
	/// identity (same line, same columns) wherever it survived the re-scan; when it did not (its
	/// line scrolled past the retained history, or a resize reflowed it away) the newest match is
	/// the fallback, the same place a new query lands.
	pub fn refresh(&mut self, matches: Vec<Match>) {
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
	pub fn step(&mut self, newer: bool) -> Option<Match> {
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
	pub fn current(&self) -> Option<Match> {
		self.matches.get(self.current).copied()
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
	fn row(line: u64, text: &str) -> Row {
		let mut row = Row::new(line);
		for (col, glyph) in text.chars().enumerate() {
			row.push(glyph, col as u16);
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
				Match {
					line: 7,
					start_col: 0,
					end_col: 4
				},
				Match {
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
		let mut row = Row::new(3);
		row.push('a', 0);
		row.push('世', 1);
		row.push('b', 3);
		assert_eq!(
			row.find("b"),
			vec![Match {
				line: 3,
				start_col: 3,
				end_col: 3
			}]
		);
		// And a search that lands ON the wide glyph reports its lead column, once.
		assert_eq!(
			row.find("世"),
			vec![Match {
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
			vec![Match {
				line: 0,
				start_col: 1,
				end_col: 1
			}]
		);
	}

	// Three matches on ascending lines, the shape every `Search` test starts from.
	fn three() -> Vec<Match> {
		vec![
			Match {
				line: 1,
				start_col: 0,
				end_col: 2,
			},
			Match {
				line: 5,
				start_col: 4,
				end_col: 6,
			},
			Match {
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
		grown.push(Match {
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
}

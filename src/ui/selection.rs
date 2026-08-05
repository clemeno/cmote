// ui/selection.rs — mouse text selection over the terminal grid (PLAN §10).
//
// This is pure grid geometry with no widgets and no clipboard: given two cells
// (where the drag began and where it is now) it decides which cells are selected
// and turns them into the text a copy would put on the clipboard. The rendering
// (highlighting the selected cells) and the clipboard write live elsewhere; this
// module is the testable core they share.
//
// The selection is a *stream* selection, like a normal terminal: it runs in
// reading order from the start cell to the end cell, so a multi-row selection
// takes the tail of the first row, all of the middle rows, and the head of the
// last row — not a rectangular block. That matches how xterm and friends behave
// and is what users expect when dragging across wrapped output.
//
// Its endpoints are DOCUMENT positions, not screen ones (§40): an absolute line index — the
// coordinate §34's prompt marks and §35's search matches already use — plus a grid column. That one
// choice is what makes the selection behave like text rather than like a rectangle painted on glass:
//
//   * scrolling moves the highlight WITH the text it covers, instead of leaving it parked on rows
//     whose contents slid out from under it;
//   * a selection may be taller than the screen — a whole command's output (§34) — and copying it
//     reads the history directly (`Screen::line_cell`) rather than only the visible grid;
//   * the renderer resolves the other way, once per row: a viewport row becomes the line it is
//     showing (`Marks::top_line` in `ui::grid`).
//
// The pointer, of course, is still on screen. `Cell` is that on-screen position and `Cell::spot` is
// the single door between the two spaces.

use crate::term::screen::{Cell as ScreenCell, Screen};

/// A single grid position ON SCREEN. `row`/`col` are 0-based viewport cells — row 0 is the top
/// visible line — the same space `screen::Screen::cell`, the renderer and the pointer
/// (`ui::terminal::cell_at`) use. `Default` is the origin cell, which lets `App` (which owns a
/// "last hovered cell") derive `Default`. Turn one into a `Spot` before it goes anywhere near a
/// selection (§40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
	pub row: u16,
	pub col: u16,
}

impl Cell {
	/// This on-screen cell as the DOCUMENT position it is showing right now (§40) — the one crossing
	/// between the two coordinate spaces. The arithmetic itself belongs to `Screen::line_at`, so a
	/// click, a prompt tick (§34) and a search match (§35) can never disagree about which line is
	/// which.
	pub fn spot(self, screen: Screen<'_>) -> Spot {
		Spot {
			line: screen.line_at(self.row),
			col: self.col,
		}
	}
}

/// A single position in the DOCUMENT (§40): an absolute line — 0 is the oldest line the session
/// still retains, `history_size` is the top of the live screen — plus a grid column. Deliberately a
/// different type from `Cell`, so a viewport row can never be handed to something expecting a
/// document line; the same discipline `search::Match` and `search::Highlight` keep between the two
/// spaces (§39).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spot {
	pub line: u64,
	pub col: u16,
}

impl Spot {
	/// Reading-order key: lines dominate, then columns. Comparing these two keys
	/// orders any two positions the way text flows, which is all the selection math
	/// needs (no need to know the grid width).
	fn order_key(self) -> (u64, u16) {
		(self.line, self.col)
	}
}

/// A drag selection: `anchor` is the position the drag started on, `head` is where the
/// pointer is now. Either can be the visually-earlier one (dragging up/left is
/// allowed), so all queries normalize to an ordered (start, end) pair first. Both are document
/// positions (§40), so the selection keeps covering its own text however the view then scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
	anchor: Spot,
	head: Spot,
}

impl Selection {
	/// Begin a selection anchored at `anchor`, with the head on the same position. Until
	/// the pointer moves to another cell this is "empty" (see `is_empty`): a bare
	/// click selects nothing, matching a terminal.
	pub fn new(anchor: Spot) -> Self {
		Self {
			anchor,
			head: anchor,
		}
	}

	/// Move the head as the pointer drags. The anchor stays put, so dragging in any
	/// direction grows or shrinks the run between the two positions.
	pub fn with_head(self, head: Spot) -> Self {
		Self {
			anchor: self.anchor,
			head,
		}
	}

	/// True when nothing is actually selected (the head never left the anchor cell).
	/// Copy is disabled in this state and a plain click clears the selection.
	pub fn is_empty(&self) -> bool {
		self.anchor == self.head
	}

	/// The selection as an ordered `(start, end)` pair in reading order, so callers
	/// never have to care which end the drag started from.
	fn bounds(&self) -> (Spot, Spot) {
		if self.anchor.order_key() <= self.head.order_key() {
			(self.anchor, self.head)
		} else {
			(self.head, self.anchor)
		}
	}

	/// Whether the cell at document line `line`, column `col` falls inside the selected stream —
	/// used by the renderer to highlight it, which turns the viewport row it is drawing into the line
	/// that row is showing first (§40). A cell is in when it is at or after the start and at or
	/// before the end in reading order.
	pub fn contains(&self, line: u64, col: u16) -> bool {
		if self.is_empty() {
			return false;
		}
		let (start, end) = self.bounds();
		let here = (line, col);
		start.order_key() <= here && here <= end.order_key()
	}

	/// The selected cells, line by line, in reading order (§10). Each line is the column span it
	/// contributes — the tail of the first line, whole middle lines, the head of the last — with
	/// the trailing half of every wide glyph dropped (its lead cell owns the glyph) and trailing
	/// blank cells trimmed, since a terminal pads every row to the full width and copying that
	/// padding would paste a wall of spaces.
	///
	/// The lines are read straight out of the document (`Screen::line_cell`, §40), so what is copied
	/// does not depend on where the viewport is parked: a selection reaching up into the history — or
	/// taller than the screen, as a long command's output is (§34) — comes back whole.
	///
	/// This is the shared geometry behind both the plain-text copy (`extract`) and the styled
	/// HTML copy (`ui::richcopy`), so the two can never disagree on which cells a selection
	/// covers. It hands back owned cells (they are cheap and short-lived for a copy), keeping
	/// this module free of any clipboard or HTML concern.
	pub fn selected_rows(&self, screen: Screen<'_>) -> Vec<Vec<ScreenCell>> {
		if self.is_empty() {
			return Vec::new();
		}
		let (start, end) = self.bounds();
		let (_, cols) = screen.size();
		let last_col = cols.saturating_sub(1);

		let mut rows: Vec<Vec<ScreenCell>> = Vec::new();
		for line in start.line..=end.line {
			// A line the session no longer holds — the selection reached back past the scrollback cap,
			// or a reflow moved the ground under it — contributes NOTHING, not an empty row: pasting
			// blank lines in place of text that is simply gone would misreport what was copied.
			if screen.line_cell(line, 0).is_none() {
				continue;
			}
			// The column range this line contributes: clipped to the start on the first line
			// and to the end on the last, full width in between.
			let from = if line == start.line { start.col } else { 0 };
			let to = if line == end.line { end.col } else { last_col };

			let mut cells: Vec<ScreenCell> = Vec::new();
			let mut col = from;
			while col <= to {
				let Some(cell) = screen.line_cell(line, col) else {
					col += 1;
					continue;
				};
				// A wide glyph's trailing half owns no glyph of its own — skip it so the lead
				// cell's glyph is not doubled.
				if cell.is_wide_continuation() {
					col += 1;
					continue;
				}
				cells.push(cell);
				col += 1;
			}
			// Trim the row's trailing blank padding (see the doc comment).
			while cells.last().is_some_and(|cell| !cell.has_contents()) {
				cells.pop();
			}
			rows.push(cells);
		}
		rows
	}

	/// Extract the selected text from `screen` as the clipboard string (§10). Walks the shared
	/// `selected_rows` geometry, reads each cell's glyph (a blank cell is a space), and joins
	/// lines with `\n`. Trailing blanks are already trimmed by `selected_rows`, so copying never
	/// pastes the grid's width-padding.
	pub fn extract(&self, screen: Screen<'_>) -> String {
		let lines: Vec<String> = self
			.selected_rows(screen)
			.iter()
			.map(|cells| {
				let mut line = String::new();
				for cell in cells {
					if cell.has_contents() {
						line.push_str(cell.contents());
					} else {
						line.push(' ');
					}
				}
				line
			})
			.collect();
		lines.join("\n")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::{ScrollMotion, Terminal};

	// A document position. On a screen with no history yet (most tests below) line N is simply
	// viewport row N, which keeps the geometry tests readable.
	fn spot(line: u64, col: u16) -> Spot {
		Spot { line, col }
	}

	// A fresh emulator fed `input`, so tests can select over real grid contents.
	fn screen_with(rows: u16, cols: u16, input: &str) -> Terminal {
		let mut terminal = Terminal::new(rows, cols);
		terminal.process(input.as_bytes());
		terminal
	}

	#[test]
	fn a_bare_click_selects_nothing() {
		// Arrange: anchor and head on the same cell (no drag).
		let selection = Selection::new(spot(0, 3));

		// Assert
		assert!(selection.is_empty());
		assert!(!selection.contains(0, 3));
	}

	#[test]
	fn contains_is_direction_independent() {
		// Dragging right-to-left must select the same cells as left-to-right.
		let forward = Selection::new(spot(0, 2)).with_head(spot(0, 5));
		let backward = Selection::new(spot(0, 5)).with_head(spot(0, 2));

		for col in 0..8 {
			assert_eq!(
				forward.contains(0, col),
				backward.contains(0, col),
				"mismatch at col {col}"
			);
		}
		assert!(forward.contains(0, 2) && forward.contains(0, 5));
		assert!(!forward.contains(0, 1) && !forward.contains(0, 6));
	}

	#[test]
	fn multi_row_selects_tail_middle_and_head() {
		// A selection from (0,2) to (2,1) takes columns >=2 on line 0, all of line 1,
		// and columns <=1 on line 2.
		let selection = Selection::new(spot(0, 2)).with_head(spot(2, 1));

		assert!(!selection.contains(0, 1)); // before the start on the first line
		assert!(selection.contains(0, 2));
		assert!(selection.contains(1, 0)); // whole middle line
		assert!(selection.contains(1, 9));
		assert!(selection.contains(2, 1));
		assert!(!selection.contains(2, 2)); // after the end on the last line
	}

	#[test]
	fn extract_trims_trailing_blanks_on_a_single_row() {
		// "hi" then blank padding to the grid width; selecting the whole row copies
		// just "hi".
		let terminal = screen_with(1, 10, "hi");
		let selection = Selection::new(spot(0, 0)).with_head(spot(0, 9));
		assert_eq!(selection.extract(terminal.screen()), "hi");
	}

	#[test]
	fn extract_joins_rows_with_newlines() {
		// Two printed lines; select across both.
		let terminal = screen_with(2, 10, "ab\r\ncd");
		let selection = Selection::new(spot(0, 0)).with_head(spot(1, 9));
		assert_eq!(selection.extract(terminal.screen()), "ab\ncd");
	}

	#[test]
	fn extract_keeps_a_wide_glyph_once() {
		// 世 occupies two columns; selecting across it must yield the glyph once, not
		// twice, and not a stray blank for the continuation cell.
		let terminal = screen_with(1, 10, "a世b");
		let selection = Selection::new(spot(0, 0)).with_head(spot(0, 3));
		assert_eq!(selection.extract(terminal.screen()), "a世b");
	}

	#[test]
	fn empty_selection_extracts_nothing() {
		let terminal = screen_with(1, 10, "hi");
		let selection = Selection::new(spot(0, 0));
		assert_eq!(selection.extract(terminal.screen()), "");
	}

	/// A selection addresses the document, not the glass (§40): what it covers is the same text
	/// however the viewport is then scrolled — including text that has left the screen entirely.
	#[test]
	fn a_selection_reads_the_document_wherever_the_viewport_sits() {
		// A two-row screen fed five lines: three have scrolled off into history (§23), so absolute
		// line 0 ("one") is nowhere on the two visible rows.
		let mut terminal = screen_with(2, 10, "one\r\ntwo\r\nthree\r\nfour\r\nfive");
		assert_eq!(terminal.screen().history_size(), 3);

		let selection = Selection::new(spot(0, 0)).with_head(spot(0, 2));
		assert_eq!(selection.extract(terminal.screen()), "one");
		// Scrolling brings it back into view; the copy is unchanged, because the scroll never
		// entered into it. Under viewport coordinates this same selection would have been copying
		// whatever line happened to be on row 0.
		terminal.scroll(ScrollMotion::Top);
		assert_eq!(selection.extract(terminal.screen()), "one");
	}

	/// A selection may be taller than the screen and still copy whole (§40) — the limit §34's
	/// select-command-output had to live with, since it could only read the visible grid.
	#[test]
	fn a_selection_taller_than_the_screen_copies_all_of_it() {
		let terminal = screen_with(2, 10, "one\r\ntwo\r\nthree\r\nfour\r\nfive");
		let selection = Selection::new(spot(0, 0)).with_head(spot(4, 9));
		assert_eq!(
			selection.extract(terminal.screen()),
			"one\ntwo\nthree\nfour\nfive"
		);
	}

	/// Lines the session does not have are skipped, not copied as blanks (§40): a selection whose
	/// end runs past the last written line yields the lines that exist and nothing for the rest.
	#[test]
	fn lines_the_document_does_not_have_contribute_nothing() {
		let terminal = screen_with(2, 10, "one\r\ntwo");
		let selection = Selection::new(spot(0, 0)).with_head(spot(9, 9));
		assert_eq!(selection.extract(terminal.screen()), "one\ntwo");
	}
}

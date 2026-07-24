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

/// A single grid position. `row`/`col` are 0-based cell coordinates, the same
/// space `vt100::Screen::cell` and the renderer use. `Default` is the origin cell,
/// which lets `App` (which owns a "last hovered cell") derive `Default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
	pub row: u16,
	pub col: u16,
}

impl Cell {
	/// Reading-order key: rows dominate, then columns. Comparing these two keys
	/// orders any two cells the way text flows, which is all the selection math
	/// needs (no need to know the grid width).
	fn order_key(self) -> (u16, u16) {
		(self.row, self.col)
	}
}

/// A drag selection: `anchor` is the cell the drag started on, `head` is where the
/// pointer is now. Either can be the visually-earlier one (dragging up/left is
/// allowed), so all queries normalize to an ordered (start, end) pair first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
	anchor: Cell,
	head: Cell,
}

impl Selection {
	/// Begin a selection anchored at `cell`, with the head on the same cell. Until
	/// the pointer moves to another cell this is "empty" (see `is_empty`): a bare
	/// click selects nothing, matching a terminal.
	pub fn new(anchor: Cell) -> Self {
		Self {
			anchor,
			head: anchor,
		}
	}

	/// Move the head as the pointer drags. The anchor stays put, so dragging in any
	/// direction grows or shrinks the run between the two cells.
	pub fn with_head(self, head: Cell) -> Self {
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
	fn bounds(&self) -> (Cell, Cell) {
		if self.anchor.order_key() <= self.head.order_key() {
			(self.anchor, self.head)
		} else {
			(self.head, self.anchor)
		}
	}

	/// Whether the cell at `(row, col)` falls inside the selected stream — used by
	/// the renderer to highlight it. A cell is in when it is at or after the start
	/// and at or before the end in reading order.
	pub fn contains(&self, row: u16, col: u16) -> bool {
		if self.is_empty() {
			return false;
		}
		let (start, end) = self.bounds();
		let here = (row, col);
		start.order_key() <= here && here <= end.order_key()
	}

	/// Extract the selected text from `screen` as the clipboard string (§10).
	/// Walks each selected row, takes the column span that row contributes (the
	/// tail of the first row, whole middle rows, the head of the last row), reads
	/// each cell's glyph, and joins rows with `\n`. Trailing blanks on every line
	/// are trimmed — terminal cells are blank-padded to the grid width, and copying
	/// that padding would paste a wall of spaces.
	pub fn extract(&self, screen: &vt100::Screen) -> String {
		if self.is_empty() {
			return String::new();
		}
		let (start, end) = self.bounds();
		let (_, cols) = screen.size();
		let last_col = cols.saturating_sub(1);

		let mut lines: Vec<String> = Vec::new();
		for row in start.row..=end.row {
			// The column range this row contributes: clipped to the start cell on the
			// first row and the end cell on the last, full width in between.
			let from = if row == start.row { start.col } else { 0 };
			let to = if row == end.row { end.col } else { last_col };

			let mut line = String::new();
			let mut col = from;
			while col <= to {
				let cell = screen.cell(row, col);
				// A wide glyph's trailing half owns no text of its own — skip it so the
				// lead cell's glyph is not doubled.
				if cell.is_some_and(vt100::Cell::is_wide_continuation) {
					col += 1;
					continue;
				}
				match cell {
					Some(cell) if cell.has_contents() => line.push_str(cell.contents()),
					// An empty cell is a space; blank runs get trimmed off the end below.
					_ => line.push(' '),
				}
				col += 1;
			}
			lines.push(line.trim_end().to_string());
		}
		lines.join("\n")
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;

	fn cell(row: u16, col: u16) -> Cell {
		Cell { row, col }
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
		let selection = Selection::new(cell(0, 3));

		// Assert
		assert!(selection.is_empty());
		assert!(!selection.contains(0, 3));
	}

	#[test]
	fn contains_is_direction_independent() {
		// Dragging right-to-left must select the same cells as left-to-right.
		let forward = Selection::new(cell(0, 2)).with_head(cell(0, 5));
		let backward = Selection::new(cell(0, 5)).with_head(cell(0, 2));

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
		// A selection from (0,2) to (2,1) takes columns >=2 on row 0, all of row 1,
		// and columns <=1 on row 2.
		let selection = Selection::new(cell(0, 2)).with_head(cell(2, 1));

		assert!(!selection.contains(0, 1)); // before the start on the first row
		assert!(selection.contains(0, 2));
		assert!(selection.contains(1, 0)); // whole middle row
		assert!(selection.contains(1, 9));
		assert!(selection.contains(2, 1));
		assert!(!selection.contains(2, 2)); // after the end on the last row
	}

	#[test]
	fn extract_trims_trailing_blanks_on_a_single_row() {
		// "hi" then blank padding to the grid width; selecting the whole row copies
		// just "hi".
		let terminal = screen_with(1, 10, "hi");
		let selection = Selection::new(cell(0, 0)).with_head(cell(0, 9));
		assert_eq!(selection.extract(terminal.screen()), "hi");
	}

	#[test]
	fn extract_joins_rows_with_newlines() {
		// Two printed lines; select across both.
		let terminal = screen_with(2, 10, "ab\r\ncd");
		let selection = Selection::new(cell(0, 0)).with_head(cell(1, 9));
		assert_eq!(selection.extract(terminal.screen()), "ab\ncd");
	}

	#[test]
	fn extract_keeps_a_wide_glyph_once() {
		// 世 occupies two columns; selecting across it must yield the glyph once, not
		// twice, and not a stray blank for the continuation cell.
		let terminal = screen_with(1, 10, "a世b");
		let selection = Selection::new(cell(0, 0)).with_head(cell(0, 3));
		assert_eq!(selection.extract(terminal.screen()), "a世b");
	}

	#[test]
	fn empty_selection_extracts_nothing() {
		let terminal = screen_with(1, 10, "hi");
		let selection = Selection::new(cell(0, 0));
		assert_eq!(selection.extract(terminal.screen()), "");
	}
}

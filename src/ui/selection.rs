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
// The pointer, of course, is still on screen. `ScreenSpot` is that on-screen position and `ScreenSpot::spot` is
// the single door between the two spaces.
//
// A press can also select on its own, without any drag (§42): a double click takes the WORD under
// the pointer and a triple click the whole LINE. Both are the same `Selection` a drag builds — the
// grid highlights one and Copy copies one, whatever put it there, the same route §34's
// select-command-output took — so the work here is deciding which cells the word or the line covers.
// "Line" means the LOGICAL line: output that ran past the right margin occupies several rows, and a
// triple click takes all of them. That same wrap flag (`Screen::line_wrapped`) is what stops a copy
// across a wrap from pasting a newline into the middle of a path.

use std::time::{Duration, Instant};

use crate::term::screen::{Cell, Screen};

/// How long after a press a second one on the same target still counts as part of the same
/// multi-click (§42). Half a second is Windows' own default double-click time
/// (`GetDoubleClickTime`), so cmote agrees with the rest of the desktop rather than inventing its
/// own feel.
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);

/// The punctuation that counts as part of a word, on top of any alphanumeric character (§42). This
/// is the whole of the double-click rule, and it is chosen for what an SSH session actually holds: a
/// path (`/etc/ssh/sshd_config`), a URL, a `user@host:port`, a `KEY=value` and a dotted filename each
/// come back WHOLE, which is almost always what is about to be pasted back into the shell.
///
/// Deliberately absent: the shell's own separators — space, quotes, brackets, `|`, `;` and `,` — so a
/// double click inside a list or an argument takes the one item under the pointer. The trade is that
/// a sentence's trailing `.` or `:` is swept up with the word before it (xterm does the same), which
/// is a cheaper annoyance than a path arriving in pieces.
const WORD_PUNCTUATION: &str = "_-./~+=@%&#?:";

/// A single grid position ON SCREEN. `row`/`col` are 0-based viewport cells — row 0 is the top
/// visible line — the same space `screen::Screen::cell`, the renderer and the pointer
/// (`ui::terminal::cell_at`) use. `Default` is the origin cell, which lets `App` (which owns a
/// "last hovered cell") derive `Default`. Turn one into a `DocSpot` before it goes anywhere near a
/// selection (§40).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenSpot {
	pub row: u16,
	pub col: u16,
}

impl ScreenSpot {
	/// This on-screen cell as the DOCUMENT position it is showing right now (§40) — the one crossing
	/// between the two coordinate spaces. The arithmetic itself belongs to `Screen::line_at`, so a
	/// click, a prompt tick (§34) and a search match (§35) can never disagree about which line is
	/// which.
	pub fn to_doc(self, screen: Screen<'_>) -> DocSpot {
		DocSpot {
			line: screen.line_at(self.row),
			col: self.col,
		}
	}
}

/// A single position in the DOCUMENT (§40): an absolute line — 0 is the oldest line the session
/// still retains, `history_size` is the top of the live screen — plus a grid column. Deliberately a
/// different type from `ScreenSpot`, so a viewport row can never be handed to something expecting a
/// document line; the same discipline `search::Match` and `search::Highlight` keep between the two
/// spaces (§39).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DocSpot {
	pub line: u64,
	pub col: u16,
}

impl DocSpot {
	/// Reading-order key: lines dominate, then columns. Comparing these two keys
	/// orders any two positions the way text flows, which is all the selection math
	/// needs (no need to know the grid width).
	fn order_key(self) -> (u64, u16) {
		(self.line, self.col)
	}
}

/// How many presses in a row landed on one cell (§42): one selects nothing on its own, two select
/// the word under the pointer, three the whole logical line. A fourth starts the count over, so
/// leaning on the button cycles rather than escalating forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
	Single,
	Double,
	Triple,
}

/// The multi-click counter (§42, §48). The toolkit reports each press on its own and says nothing
/// about how many came before it, so cmote keeps the tally: the last press's target, when it
/// happened, and what it counted as. Kept here rather than in `app` because it is pure timing
/// arithmetic and worth a test — `press` takes the current instant instead of reading the clock
/// itself for exactly that reason.
///
/// Consecutive presses must be on the SAME TARGET, not merely within a few pixels as a
/// general-purpose widget would ask, and `T` is what "the same" means. The grid counts presses on a
/// [`ScreenSpot`] (§42): the cell IS the target there, and it is the cell a word or line then expands
/// from, so nudging the pointer inside one cell between two clicks must not break the double click
/// and crossing into the next cell must. The window's dividers count presses on a
/// `pane_grid::Split` (§48), where the seam is the target the same way — the pointer may wander
/// anywhere along a divider that is hundreds of pixels long and still be double-clicking the same
/// one.
#[derive(Debug, Clone, Copy)]
pub struct Clicks<T> {
	/// The last press: what it landed on, when, and what it counted as. `None` until the first one.
	last: Option<(T, Instant, Click)>,
}

/// Written out rather than derived because deriving would demand `T: Default`, which is a promise
/// the counter never needs: it starts having seen nothing at all, whatever it counts.
impl<T> Default for Clicks<T> {
	fn default() -> Self {
		Self { last: None }
	}
}

impl<T: Copy + PartialEq> Clicks<T> {
	/// Count a press on `target` at `now`, returning what it is (§42). A press on another target, or
	/// one that came too late, is a fresh `Single`; each press inside the window escalates from the
	/// one before it. `Instant::duration_since` saturates rather than panicking, so a clock that
	/// appears to run backwards yields a zero gap — still inside the window, which is harmless here.
	pub fn press(&mut self, target: T, now: Instant) -> Click {
		let kind = match self.last {
			Some((last_target, at, last))
				if last_target == target && now.duration_since(at) <= MULTI_CLICK_WINDOW =>
			{
				match last {
					Click::Single => Click::Double,
					Click::Double => Click::Triple,
					Click::Triple => Click::Single,
				}
			}
			_ => Click::Single,
		};
		self.last = Some((target, now, kind));
		kind
	}
}

/// What made a selection — the one thing that can tell two identical-looking ones apart (§42). A
/// DRAG whose head never left its anchor selects nothing, because a bare click deselects in every
/// terminal; a selection over a KNOWN RANGE that happens to be one cell wide — a one-letter word, a
/// one-character search hit (§35), a command whose output is a single character (§34) — really does
/// select that cell. Both have `anchor == head`, so only their origin separates them, and `is_empty`
/// reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
	Drag,
	Expanded,
}

/// A drag selection: `anchor` is the position the drag started on, `head` is where the
/// pointer is now. Either can be the visually-earlier one (dragging up/left is
/// allowed), so all queries normalize to an ordered (start, end) pair first. Both are document
/// positions (§40), so the selection keeps covering its own text however the view then scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
	anchor: DocSpot,
	head: DocSpot,
	origin: Origin,
}

impl Selection {
	/// Begin a selection anchored at `anchor`, with the head on the same position. Until
	/// the pointer moves to another cell this is "empty" (see `is_empty`): a bare
	/// click selects nothing, matching a terminal.
	pub fn new(anchor: DocSpot) -> Self {
		Self {
			anchor,
			head: anchor,
			origin: Origin::Drag,
		}
	}

	/// Move the head as the pointer drags. The anchor stays put, so dragging in any
	/// direction grows or shrinks the run between the two positions.
	pub fn with_head(self, head: DocSpot) -> Self {
		Self {
			anchor: self.anchor,
			head,
			origin: self.origin,
		}
	}

	/// A selection over a range something else already worked out, rather than one a pointer dragged:
	/// a command's output (§34), a search hit (§35), a word or a line (§42). Inclusive of both ends,
	/// and — unlike a drag — never empty, so a range one cell wide is highlighted and copyable instead
	/// of reading as "nothing selected" (see `Origin`).
	pub fn spanning(start: DocSpot, end: DocSpot) -> Self {
		Self {
			anchor: start,
			head: end,
			origin: Origin::Expanded,
		}
	}

	/// The word around `spot`, as a double click selects it (§42), or `None` when there is no word
	/// there — a blank cell, a separator, or a line the session no longer holds. Falling back to
	/// `None` rather than to a one-cell span is deliberate: a double click on empty space should leave
	/// the screen as it was, not select a space nobody asked for.
	///
	/// The run grows outward from the clicked cell over word characters (`is_word_char`) and CROSSES A
	/// WRAP: output that ran past the right margin is one logical line, so a path broken across two
	/// rows is still one word — and, because a copy re-joins wrapped rows, it comes back off the
	/// clipboard in one piece.
	pub fn word(screen: Screen<'_>, spot: DocSpot) -> Option<Self> {
		if !is_word(screen, spot) {
			return None;
		}
		let mut start = spot;
		while let Some(previous) = step_left(screen, start) {
			if !is_word(screen, previous) {
				break;
			}
			start = previous;
		}
		let mut end = spot;
		while let Some(next) = step_right(screen, end) {
			if !is_word(screen, next) {
				break;
			}
			end = next;
		}
		Some(Self::spanning(start, end))
	}

	/// The whole LOGICAL line through `spot`, as a triple click selects it (§42), or `None` for a line
	/// the session no longer holds. Logical, not physical: the run walks back to the first row of a
	/// wrapped line and on to its last, so one long command — or one long path — is taken in full
	/// however many rows it occupies.
	pub fn line(screen: Screen<'_>, spot: DocSpot) -> Option<Self> {
		// A line with no cell at column 0 is not a line the document has (§40).
		screen.line_cell(spot.line, 0)?;
		let (_, cols) = screen.size();
		// Back to the first row of the wrapped run: a line is a continuation when the one BEFORE it is
		// marked as continued. A line the session dropped reports `false`, which ends the walk.
		let mut first = spot.line;
		while first > 0 && screen.line_wrapped(first - 1) {
			first -= 1;
		}
		// And forward while this line continues into the next one the document actually has, so the
		// walk cannot run off the end of the document.
		let mut last = spot.line;
		while screen.line_wrapped(last) && screen.line_cell(last + 1, 0).is_some() {
			last += 1;
		}
		Some(Self::spanning(
			DocSpot {
				line: first,
				col: 0,
			},
			DocSpot {
				line: last,
				col: cols.saturating_sub(1),
			},
		))
	}

	/// True when nothing is actually selected — a DRAG whose head never left its anchor cell. Copy is
	/// disabled in this state and a plain click clears the selection. A word or line selection is
	/// never empty, even when it covers a single cell (see `Origin`).
	pub fn is_empty(&self) -> bool {
		self.origin == Origin::Drag && self.anchor == self.head
	}

	/// The selection as an ordered `(start, end)` pair in reading order, so callers
	/// never have to care which end the drag started from.
	fn bounds(&self) -> (DocSpot, DocSpot) {
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
	pub fn selected_rows(&self, screen: Screen<'_>) -> Vec<SelectionRow> {
		if self.is_empty() {
			return Vec::new();
		}
		let (start, end) = self.bounds();
		let (_, cols) = screen.size();
		let last_col = cols.saturating_sub(1);

		let mut rows: Vec<SelectionRow> = Vec::new();
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

			let mut cells: Vec<Cell> = Vec::new();
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
			// Trim the row's trailing blank padding (see the doc comment) — but only on a line that
			// actually ends there. A row that WRAPS into the next one has no padding to trim: every
			// column of it is real output, and any blank in it is a space in the middle of a logical
			// line, which trimming would swallow.
			let wrapped = screen.line_wrapped(line);
			if !wrapped {
				while cells.last().is_some_and(|cell| !cell.has_contents()) {
					cells.pop();
				}
			}
			rows.push(SelectionRow { cells, wrapped });
		}
		rows
	}

	/// Extract the selected text from `screen` as the clipboard string (§10). Walks the shared
	/// `selected_rows` geometry, reads each cell's glyph (a blank cell is a space), and joins
	/// lines with `\n`. Trailing blanks are already trimmed by `selected_rows`, so copying never
	/// pastes the grid's width-padding.
	///
	/// A row that WRAPS into the next one is joined to it with NO newline (§42): those two rows are one
	/// logical line the terminal happened to fold, so a copied path or URL comes back in one piece
	/// instead of arriving with a line break where the window's edge was. Every other terminal
	/// unwraps on copy for the same reason — a pasted command has to be the command that ran.
	pub fn extract(&self, screen: Screen<'_>) -> String {
		let rows = self.selected_rows(screen);
		let mut text = String::new();
		for (index, row) in rows.iter().enumerate() {
			// The break belongs to the row BEFORE this one: it gets a newline unless it wrapped.
			if index > 0 && !rows[index - 1].wrapped {
				text.push('\n');
			}
			for cell in &row.cells {
				if cell.has_contents() {
					text.push_str(cell.contents());
				} else {
					text.push(' ');
				}
			}
		}
		text
	}
}

/// One line's contribution to a selection (§10): the cells it gives up, and whether the line is
/// CONTINUED by the next one (§42). The flag is what tells a consumer whether to put a line break
/// between this row and the one after it — `extract` and the HTML copy (`ui::richcopy`) both read it,
/// so the two can never disagree about where the pasted text breaks.
#[derive(Debug, Clone)]
pub struct SelectionRow {
	pub cells: Vec<Cell>,
	pub wrapped: bool,
}

/// Whether the cell at `spot` is part of a word (§42). A blank cell, a separator, and a cell the
/// document does not have are all "no", which is what ends a double click's outward walk.
///
/// A wide glyph's trailing half carries no text of its own — the lead cell in the column before it
/// owns the glyph — so the question is passed to that cell instead. Without this every CJK word would
/// end after its first character.
fn is_word(screen: Screen<'_>, spot: DocSpot) -> bool {
	let Some(cell) = screen.line_cell(spot.line, spot.col) else {
		return false;
	};
	let cell = if cell.is_wide_continuation() && spot.col > 0 {
		match screen.line_cell(spot.line, spot.col - 1) {
			Some(lead) => lead,
			None => return false,
		}
	} else {
		cell
	};
	// The BASE character decides: a grapheme's combining marks follow whatever it is, and a blank
	// cell has no characters at all.
	cell.contents().chars().next().is_some_and(is_word_char)
}

/// Whether one character belongs to a word (§42): any alphanumeric — in any script, so CJK and
/// accented text work without a special case — plus the shell-friendly punctuation in
/// `WORD_PUNCTUATION`.
fn is_word_char(ch: char) -> bool {
	ch.is_alphanumeric() || WORD_PUNCTUATION.contains(ch)
}

/// The position one cell before `spot` within its LOGICAL line (§42), or `None` at the start of one.
/// At column 0 that means the last column of the line above — but only when the line above is marked
/// as continuing into this one, otherwise the two are separate lines and the word ends here.
fn step_left(screen: Screen<'_>, spot: DocSpot) -> Option<DocSpot> {
	if spot.col > 0 {
		return Some(DocSpot {
			line: spot.line,
			col: spot.col - 1,
		});
	}
	let previous = spot.line.checked_sub(1)?;
	if !screen.line_wrapped(previous) {
		return None;
	}
	let (_, cols) = screen.size();
	Some(DocSpot {
		line: previous,
		col: cols.saturating_sub(1),
	})
}

/// The position one cell after `spot` within its LOGICAL line (§42), or `None` at the end of one —
/// the mirror of `step_left`, crossing into column 0 of the next line only when this one wraps into
/// it and the document actually holds it.
fn step_right(screen: Screen<'_>, spot: DocSpot) -> Option<DocSpot> {
	let (_, cols) = screen.size();
	if spot.col + 1 < cols {
		return Some(DocSpot {
			line: spot.line,
			col: spot.col + 1,
		});
	}
	if !screen.line_wrapped(spot.line) {
		return None;
	}
	let next = DocSpot {
		line: spot.line + 1,
		col: 0,
	};
	screen.line_cell(next.line, next.col).map(|_| next)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::{ScrollMotion, Terminal};

	// A document position. On a screen with no history yet (most tests below) line N is simply
	// viewport row N, which keeps the geometry tests readable.
	fn spot(line: u64, col: u16) -> DocSpot {
		DocSpot { line, col }
	}

	// A fresh emulator fed `input`, so tests can select over real grid contents.
	fn screen_with(rows: u16, cols: u16, input: &str) -> Terminal {
		let mut terminal = Terminal::new(rows, cols);
		terminal.process(input.as_bytes());
		terminal
	}

	// An on-screen cell, for the multi-click tally (which counts cells, not document lines).
	fn at(row: u16, col: u16) -> ScreenSpot {
		ScreenSpot { row, col }
	}

	// The text a word selection around `col` on the first line copies, or `None` when there is no
	// word there — the double click, end to end (§42).
	fn word_at(terminal: &Terminal, line: u64, col: u16) -> Option<String> {
		Selection::word(terminal.screen(), spot(line, col))
			.map(|selection| selection.extract(terminal.screen()))
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
	fn extract_keeps_the_tab_the_grid_draws_as_a_blank() {
		// The engine stores the TAB in the first cell it skipped (`put_tab`), and copying a region
		// gives that tab back — which is what keeps a paste of columnar output (`du`, `ls -l`) lined
		// up instead of collapsed to one space.
		//
		// The counterpart to §117, and the reason that fix is draw-only: the GRID must not hand a
		// control character to the text shaper (it would displace every glyph after it in the run),
		// but the character has to stay in the cell for everything that READS the grid rather than
		// painting it. This test is the half that would catch a "fix" applied one layer too deep.
		let terminal = screen_with(1, 24, "23\t./trans_3");
		let selection = Selection::new(spot(0, 0)).with_head(spot(0, 16));
		assert_eq!(selection.extract(terminal.screen()), "23\t     ./trans_3");
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

	/// Presses on one cell escalate single → double → triple, then start over (§42) — so leaning on
	/// the button cycles instead of escalating for ever.
	#[test]
	fn presses_on_one_cell_escalate_and_then_start_over() {
		let mut clicks = Clicks::default();
		let start = Instant::now();
		let cell = at(3, 7);
		assert_eq!(clicks.press(cell, start), Click::Single);
		assert_eq!(
			clicks.press(cell, start + Duration::from_millis(100)),
			Click::Double
		);
		assert_eq!(
			clicks.press(cell, start + Duration::from_millis(200)),
			Click::Triple
		);
		assert_eq!(
			clicks.press(cell, start + Duration::from_millis(300)),
			Click::Single
		);
	}

	/// A press that came too late, or landed on another cell, is a fresh single click (§42).
	#[test]
	fn a_late_press_or_one_on_another_cell_starts_the_count_over() {
		let mut clicks = Clicks::default();
		let start = Instant::now();
		assert_eq!(clicks.press(at(0, 0), start), Click::Single);
		// Past the window — however deliberate the pair looked, it is two separate clicks.
		assert_eq!(
			clicks.press(at(0, 0), start + Duration::from_millis(501)),
			Click::Single
		);
		// Inside it, the same cell escalates …
		assert_eq!(
			clicks.press(at(0, 0), start + Duration::from_millis(600)),
			Click::Double
		);
		// … and the cell next door is another target, so it starts over.
		assert_eq!(
			clicks.press(at(0, 1), start + Duration::from_millis(650)),
			Click::Single
		);
	}

	/// A double click takes the word the pointer is in, whichever character of it was hit (§42).
	#[test]
	fn a_double_click_takes_the_word_under_the_pointer() {
		let terminal = screen_with(1, 20, "cat file.txt now");
		// The first character of the word, one in the middle, and its last: all the same word.
		assert_eq!(word_at(&terminal, 0, 4).as_deref(), Some("file.txt"));
		assert_eq!(word_at(&terminal, 0, 7).as_deref(), Some("file.txt"));
		assert_eq!(word_at(&terminal, 0, 11).as_deref(), Some("file.txt"));
	}

	/// The word rule is chosen for what a shell session holds (§42): a path, a URL, an endpoint and a
	/// `KEY=value` each come back whole, because a double click on one is nearly always about to be
	/// pasted straight back into the shell.
	#[test]
	fn a_path_a_url_and_an_endpoint_are_each_one_word() {
		let terminal = screen_with(1, 40, "vi /etc/ssh/sshd_config");
		assert_eq!(
			word_at(&terminal, 0, 8).as_deref(),
			Some("/etc/ssh/sshd_config")
		);

		let terminal = screen_with(1, 40, "see https://example.com/a?b=1");
		assert_eq!(
			word_at(&terminal, 0, 10).as_deref(),
			Some("https://example.com/a?b=1")
		);

		let terminal = screen_with(1, 40, "ssh root@10.0.0.1:22");
		assert_eq!(
			word_at(&terminal, 0, 6).as_deref(),
			Some("root@10.0.0.1:22")
		);
	}

	/// And it stops at the shell's own separators (§42) — space, quotes, brackets and commas — so a
	/// double click inside a list or an argument takes the one item under the pointer.
	#[test]
	fn a_word_stops_at_the_shells_own_separators() {
		let terminal = screen_with(1, 20, "ls 'a' (b) c,d");
		assert_eq!(word_at(&terminal, 0, 4).as_deref(), Some("a"));
		assert_eq!(word_at(&terminal, 0, 8).as_deref(), Some("b"));
		assert_eq!(word_at(&terminal, 0, 11).as_deref(), Some("c"));
		assert_eq!(word_at(&terminal, 0, 13).as_deref(), Some("d"));
	}

	/// A double click on blank space selects nothing rather than a space (§42): the screen is left as
	/// it was, which is what a click on nothing should do.
	#[test]
	fn a_double_click_on_blank_space_selects_nothing() {
		let terminal = screen_with(1, 10, "a b");
		assert!(word_at(&terminal, 0, 1).is_none(), "the gap between words");
		assert!(word_at(&terminal, 0, 6).is_none(), "the padding after them");
	}

	/// A one-character word really is a selection, unlike a drag that never left its anchor (§42) —
	/// the two look identical (`anchor == head`) and only their origin tells them apart. The same holds
	/// for any range something else worked out: a one-character search hit (§35), or a command that
	/// printed a single character (§34).
	#[test]
	fn a_one_cell_word_is_a_real_selection() {
		let terminal = screen_with(1, 10, "cd a");
		let selection = Selection::word(terminal.screen(), spot(0, 3)).expect("a word is there");
		assert!(!selection.is_empty());
		assert_eq!(selection.extract(terminal.screen()), "a");
		assert!(selection.contains(0, 3), "and it is highlighted");

		// A one-cell range built directly reads the same way …
		let hit = Selection::spanning(spot(0, 3), spot(0, 3));
		assert!(!hit.is_empty());
		assert_eq!(hit.extract(terminal.screen()), "a");

		// … while the bare click on that cell still selects nothing, so it still deselects.
		assert!(Selection::new(spot(0, 3)).is_empty());
	}

	/// A word broken across the right margin is still one word, and copies without the line break the
	/// window's edge put in it (§42) — the whole reason the wrap flag is read at all.
	#[test]
	fn a_word_crosses_a_wrap_and_copies_in_one_piece() {
		// Eight columns: "/etc/ssh" fills row 0 and "/sshd" carries on below it.
		let terminal = screen_with(2, 8, "/etc/ssh/sshd");
		assert_eq!(word_at(&terminal, 0, 2).as_deref(), Some("/etc/ssh/sshd"));
		// Reached from the far side of the wrap, it is the same word.
		assert_eq!(word_at(&terminal, 1, 3).as_deref(), Some("/etc/ssh/sshd"));
	}

	/// A triple click takes the whole LOGICAL line — every row a wrapped one occupies — and stops at
	/// the next line, whichever row it was clicked on (§42).
	#[test]
	fn a_triple_click_takes_the_whole_logical_line() {
		// "one two three" wraps over rows 0-1 on an eight-column screen; "next" is a line of its own.
		let terminal = screen_with(3, 8, "one two three\r\nnext");
		let line = |line: u64| {
			Selection::line(terminal.screen(), spot(line, 2))
				.expect("the document holds that line")
				.extract(terminal.screen())
		};
		assert_eq!(line(0), "one two three");
		assert_eq!(line(1), "one two three", "clicked on the second half");
		assert_eq!(line(2), "next");
		// A line the session does not hold selects nothing at all.
		assert!(Selection::line(terminal.screen(), spot(99, 0)).is_none());
	}

	/// An ordinary drag across a wrap unwraps on copy too (§42): the break belongs to the line, not to
	/// the row the window happened to fold it at.
	#[test]
	fn a_drag_across_a_wrap_copies_without_the_break() {
		let terminal = screen_with(3, 8, "abcdefghij\r\nnext");
		let selection = Selection::new(spot(0, 0)).with_head(spot(2, 7));
		assert_eq!(selection.extract(terminal.screen()), "abcdefghij\nnext");
	}
}

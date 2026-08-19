// term/gate.rs — the one place cmote sits BETWEEN the parser and the engine (PLAN §102).
//
// Every other module under `term/` reads the byte stream a SECOND time, beside the engine, and acts
// on what the engine dropped. That shape was chosen deliberately and it is still the right one for
// almost everything: a scanner cannot break what the engine does, because it never stands in the
// engine's way.
//
// This module is the exception, and it exists because two things cannot be done from beside the
// stream:
//
//   READING BACK WHAT THE ENGINE DECIDED. The vertical scrolling region is private, with no
//   accessor and no reply arm (`term/region.rs` says what that cost). A scanner could watch DECSTBM
//   go past on the wire — but not the RESETS, which happen inside the engine on RIS and on resize.
//   Sitting on the `Handler` boundary catches all of them, because that is where the engine is TOLD.
//
//   STANDING IN FRONT OF A DECISION. Left and right margins change what PRINTING a character does:
//   the line breaks at the margin instead of at the screen edge. There is no sequence to translate
//   that into (§72's route) and no way to repair it afterwards, because by the time the text is on
//   the grid it is at the wrong columns. The only place to be is in front of `Handler::input`.
//
// `Processor::advance<H: Handler>` is generic over the handler and `Term` merely IMPLEMENTS
// `Handler`, so a type that holds `&mut Term` and implements the trait itself can be passed in its
// place. No fork, no patch, no reimplementation: the gate answers a dozen calls itself and hands the
// engine the other sixty.
//
// WHY THIS WAS REFUSED FOR SEVEN SECTIONS, AND WHAT CHANGED
//
// TERMINAL_COMPATIBILITY_PLAN §5 costed this build twice and turned it down both times, on one
// argument: **every method of `Handler` has a default empty body**, so a method the gate forgets to
// forward — or one a future `alacritty_terminal` ADDS — compiles cleanly and silently drops a
// sequence. §5 called that "the same class of hazard as §57's borrowed flag bit, except §57's could
// be caught at build time with a `const` assertion and this one cannot: a trait growing a defaulted
// method breaks nothing."
//
// That last clause was wrong, and the attribute on the `impl` below is why:
//
//   #[deny(clippy::missing_trait_methods)]
//
// The lint reports every method an `impl` leaves to its default. Denied on this one block, a method
// missing from the list — today's oversight or tomorrow's addition — is a **build error**, and
// cmote's gate runs `clippy --all-targets -- -D warnings`. The failure mode §5 refused the build
// over is now the one thing that cannot happen quietly.
//
// It fails loud from the other end too. If a future clippy DROPS the lint, the attribute names a
// lint that no longer exists, which is an `unknown_lints` warning, which `-D warnings` turns into an
// error. There is no version of the future where this file goes quiet on its own.
//
// What the guard does NOT catch is a method that is present and forwards WRONGLY, or one whose
// meaning changes inside the engine while its signature stays. The first is ordinary code and is
// tested like ordinary code; the second is the same exposure every other module here already runs,
// since all of them read engine state.
//
// THE FORWARDS ARE GENERATED, AND THAT IS A CORRECTNESS ARGUMENT
//
// Sixty-odd forwards written out by hand would be sixty chances to pass `count` where `mode` was
// meant. The `forward!` macro below takes a name and a signature and writes the body, so the only
// thing that can be got wrong is the signature — and a wrong signature does not compile, because the
// trait's does not match. The macro is not a saving of typing. It is the removal of a class of bug.

use std::sync::{Arc, Mutex};

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
	Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
	KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb,
	ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};
use unicode_width::UnicodeWidthChar;

use super::margins::Margins;
use super::region::ScrollRegion;
use super::{Engine, ReplyBuffer};

/// DECLRMM, the private mode that turns the left and right margins on (§102).
///
/// Absent from the engine's `NamedPrivateMode`, so it arrives as an `Unknown` and would otherwise be
/// ignored — including by the DECRQM answer, which would report "not recognised" for a mode cmote
/// now implements.
const LEFT_RIGHT_MARGIN_MODE: u16 = 69;

/// Write one `Handler` method that hands its arguments straight to the engine.
///
/// Each entry is the method's name and its parameter list, spelled exactly as the trait spells it.
/// Every method of `Handler` returns `()` and takes `&mut self`, so that shape is baked in and the
/// entry carries only what varies. See the module header for why this is generated rather than
/// written out: a wrong signature fails to compile, and a hand-written body that forwards the wrong
/// argument does not.
macro_rules! forward {
	($($name:ident($($argument:ident: $type:ty),*)),* $(,)?) => {
		$(
			fn $name(&mut self $(, $argument: $type)*) {
				self.term.$name($($argument),*);
			}
		)*
	};
}

/// The handler cmote passes to `Processor::advance` in the engine's place (§102).
///
/// Borrowed for the length of one advance and thrown away, so it holds no state of its own — the
/// state it maintains lives on `Terminal` and is borrowed in alongside the engine.
pub struct Gate<'a> {
	/// The engine, which still does all the work this gate does not do itself.
	term: &'a mut Engine,
	/// cmote's mirror of the engine's private vertical scrolling region (`term/region.rs`).
	region: &'a mut ScrollRegion,
	/// The left and right margins and the deferred wrap that comes with them (`term/margins.rs`).
	margins: &'a mut Margins,
	/// Where a reply goes. The engine writes its own answers through its event listener into this
	/// same buffer, so an answer the gate writes lands in the stream in the order it was asked for
	/// rather than in a second queue that would have to be kept in step (§33).
	replies: &'a Arc<Mutex<ReplyBuffer>>,
}

impl<'a> Gate<'a> {
	/// Borrow an engine and the state kept beside it for the length of one advance.
	pub fn new(
		term: &'a mut Engine,
		region: &'a mut ScrollRegion,
		margins: &'a mut Margins,
		replies: &'a Arc<Mutex<ReplyBuffer>>,
	) -> Self {
		Self {
			term,
			region,
			margins,
			replies,
		}
	}

	/// The page width.
	fn cols(&self) -> usize {
		self.term.grid().columns()
	}

	/// The page height.
	fn rows(&self) -> usize {
		self.term.grid().screen_lines()
	}

	/// Whether the margins exclude a column, which is the whole test for "cmote handles this one".
	/// While it is false every method below hands straight over and the session runs on exactly the
	/// code it ran on before §102 (`term/margins.rs` says why that matters).
	fn narrowed(&self) -> bool {
		self.margins.narrowed(self.cols())
	}

	/// DECOM. It decides whether a column a program names is counted from the left margin.
	fn origin(&self) -> bool {
		self.term.mode().contains(TermMode::ORIGIN)
	}

	/// DECAWM. A line only breaks at the right margin while autowrap is on; with it off the cursor
	/// stays on the margin and each glyph overwrites the last.
	fn autowrap(&self) -> bool {
		self.term.mode().contains(TermMode::LINE_WRAP)
	}

	/// The cursor's column.
	fn column(&self) -> usize {
		self.term.grid().cursor.point.column.0
	}

	/// The cursor's row, in visible-page coordinates.
	fn row(&self) -> usize {
		self.term.grid().cursor.point.line.0.max(0) as usize
	}

	/// Put the cursor on a column, and cancel any deferred wrap.
	///
	/// Written straight into the grid rather than through `Term::goto_col`, which would be the
	/// natural call and cannot be used: the engine's `goto_col` routes through `goto`, and `goto`
	/// adds the scrolling region's top to the LINE it is given — so under origin mode a pure column
	/// move drags the cursor downward. That is the engine defect §74 recorded against CHA and HPA,
	/// and every margin-aware motion here would inherit it.
	fn set_column(&mut self, column: usize) {
		let column = column.min(self.cols().saturating_sub(1));
		let cursor = &mut self.term.grid_mut().cursor;
		cursor.point.column = Column(column);
		cursor.input_needs_wrap = false;
		self.margins.set_pending_wrap(false);
	}

	/// Put the cursor on a row of the visible page, leaving the column alone.
	fn set_row(&mut self, row: usize) {
		let row = row.min(self.rows().saturating_sub(1));
		self.term.grid_mut().cursor.point.line = Line(row as i32);
	}

	/// Whether the cursor is inside both bands — the test IL and DL apply before moving anything.
	fn cursor_in_band(&self) -> bool {
		let row = self.row();
		let column = self.column();
		row >= self.region.first_row()
			&& row <= self.region.last_row()
			&& column >= self.margins.left()
			&& column <= self.margins.right()
	}

	/// After a glyph: keep the cursor ON the right margin and remember that a wrap is owed (§102).
	///
	/// Two ways the engine leaves the cursor past the margin, and both end here. With a right margin
	/// short of the screen edge the engine simply advances the column past it, knowing nothing about
	/// the band. With the right margin AT the screen edge the engine instead sets its own
	/// `input_needs_wrap` — which has to be taken away from it, because the engine's wrap goes to
	/// column 0 and the band's goes to the left margin.
	fn hold_at_right_margin(&mut self) {
		let right = self.margins.right();
		let cursor = &mut self.term.grid_mut().cursor;
		let owed = if cursor.input_needs_wrap {
			cursor.input_needs_wrap = false;
			true
		} else if cursor.point.column.0 > right {
			cursor.point.column = Column(right);
			true
		} else {
			false
		};
		if owed {
			self.margins.set_pending_wrap(true);
		}
	}

	/// Take the wrap the last glyph earned, if one is owed and autowrap allows it.
	///
	/// Called at the top of everything that would otherwise act on a cursor sitting ON the right
	/// margin with its cell already written. The flag is cleared either way: a deferred wrap that
	/// autowrap forbids is not kept waiting for autowrap to come back on.
	fn take_pending_wrap(&mut self) {
		if !self.margins.pending_wrap() {
			return;
		}
		self.margins.set_pending_wrap(false);
		if self.autowrap() {
			self.wrap_to_left_margin();
		}
	}

	/// Break the line at the right margin: down one row inside the scrolling region, then back to the
	/// LEFT MARGIN rather than to column 1.
	///
	/// **No `WRAPLINE` flag is set here**, and that is a decision rather than an omission. The engine
	/// marks a row that wrapped so that search, selection and copy can read the two rows as one
	/// logical line — but the flag belongs to the whole ROW, and inside a narrow band the rest of the
	/// row belongs to some other column of the page. Joining on it would splice unrelated text into
	/// every copy taken across a margin wrap. So the rows stay separate, which costs a wrapped word
	/// being copied in two pieces and keeps the copy honest about what was on the screen (§102).
	fn wrap_to_left_margin(&mut self) {
		self.index();
		self.set_column(self.margins.left());
	}

	/// One row down, scrolling the band if the cursor is already on the last row of the region — the
	/// margin-aware half of LF, VT, FF, IND and NEL.
	fn index(&mut self) {
		let bottom = self.region.last_row();
		if self.row() == bottom {
			self.scroll_band(self.region.first_row(), bottom, 1, true);
		} else if self.row() + 1 < self.rows() {
			self.set_row(self.row() + 1);
		}
	}

	/// One row up, scrolling the band if the cursor is already on the first row of the region — RI.
	fn reverse(&mut self) {
		let top = self.region.first_row();
		if self.row() == top {
			self.scroll_band(top, self.region.last_row(), 1, false);
		} else if self.row() > 0 {
			self.set_row(self.row() - 1);
		}
	}

	/// Scroll a band of columns through a band of rows (§102).
	///
	/// The one primitive under every margin-aware scroll: SU and SD take the whole region, IL and DL
	/// take the region from the cursor's row down, and IND and RI take the region by one line. Only
	/// the columns between the margins move; everything outside them is another column of the page
	/// and is not scrolling.
	///
	/// **A row pushed out of the band is discarded, not pushed to the scrollback.** The history holds
	/// whole lines, and this row is a slice of one — the columns outside the band are not leaving.
	/// xterm does the same, and it is also the only answer that leaves the history readable, since
	/// half-lines interleaved with whole ones would make every search, selection and copy wrong.
	fn scroll_band(&mut self, top: usize, bottom: usize, lines: usize, up: bool) {
		if bottom < top || lines == 0 {
			return;
		}
		let height = bottom - top + 1;
		let lines = lines.min(height);
		let (left, right) = (self.margins.left(), self.margins.right());
		let background = self.term.grid().cursor.template.bg;
		let grid = self.term.grid_mut();
		if lines < height {
			// Two walks rather than one, and the direction matters: a scroll's source and destination
			// overlap, so writing upward while reading downward would smear a row over the rows it was
			// about to be read from.
			let destinations: Vec<usize> = if up {
				(top..=bottom - lines).collect()
			} else {
				((top + lines)..=bottom).rev().collect()
			};
			for destination in destinations {
				let source = if up {
					destination + lines
				} else {
					destination - lines
				};
				for column in left..=right {
					let cell = grid[Line(source as i32)][Column(column)].clone();
					grid[Line(destination as i32)][Column(column)] = cell;
				}
			}
		}
		let blanked: Vec<usize> = if up {
			((bottom + 1 - lines)..=bottom).collect()
		} else {
			(top..=(top + lines - 1)).collect()
		};
		for row in blanked {
			for column in left..=right {
				grid[Line(row as i32)][Column(column)] = background.into();
			}
		}
		for row in top..=bottom {
			self.mend_band_edges(row);
		}
	}

	/// Insert or delete cells within the band, at the cursor — ICH and DCH under margins.
	///
	/// The engine's own versions run to the page edge, which under margins would push a neighbouring
	/// column's text sideways. A cursor outside the band does nothing at all: the margins bound the
	/// operation, and there is no sensible band to perform it in.
	fn shift_cells(&mut self, count: usize, insert: bool) {
		let (left, right) = (self.margins.left(), self.margins.right());
		let cursor = self.column();
		if cursor < left || cursor > right || count == 0 {
			return;
		}
		let room = right - cursor + 1;
		let count = count.min(room);
		let background = self.term.grid().cursor.template.bg;
		let row = Line(self.row() as i32);
		let grid = self.term.grid_mut();
		if count < room {
			let destinations: Vec<usize> = if insert {
				((cursor + count)..=right).rev().collect()
			} else {
				(cursor..=(right - count)).collect()
			};
			for destination in destinations {
				let source = if insert {
					destination - count
				} else {
					destination + count
				};
				let cell = grid[row][Column(source)].clone();
				grid[row][Column(destination)] = cell;
			}
		}
		let blanked: Vec<usize> = if insert {
			(cursor..(cursor + count)).collect()
		} else {
			((right + 1 - count)..=right).collect()
		};
		for column in blanked {
			grid[row][Column(column)] = background.into();
		}
		self.mend_band_edges(self.row());
	}

	/// Blank the half of a wide glyph left stranded across a margin (§102).
	///
	/// A glyph two cells wide can straddle a margin: its lead inside the band and its continuation
	/// outside, or the other way round. Move the band and only one half moves, leaving a lead with no
	/// continuation or a continuation with no lead — neither a state the renderer, the reflow or the
	/// copy expects to meet, and the half that remains is not a character anybody asked for. Only the
	/// four cells either side of the two margins can be in it: every other pair moved together.
	///
	/// The same care `shift_columns` takes at the page edges for SL and SR (§100).
	fn mend_band_edges(&mut self, row: usize) {
		let (left, right) = (self.margins.left(), self.margins.right());
		let cols = self.cols();
		let background = self.term.grid().cursor.template.bg;
		let line = Line(row as i32);
		let grid = self.term.grid_mut();
		// Inside the band at each edge, and outside it at each edge: a split pair leaves one orphan on
		// each side, so both have to be looked at.
		let suspects = [
			(left, Flags::WIDE_CHAR_SPACER),
			(left.checked_sub(1).unwrap_or(left), Flags::WIDE_CHAR),
			(right, Flags::WIDE_CHAR),
			(
				(right + 1).min(cols.saturating_sub(1)),
				Flags::WIDE_CHAR_SPACER,
			),
		];
		for (column, orphaned) in suspects {
			// The left edge of the page has nothing outside it, and neither has the right — the
			// clamping above turns those into cells that are inside the band, where the flag being
			// looked for cannot be an orphan.
			let inside_band = column >= left && column <= right;
			let looking_outward = orphaned == Flags::WIDE_CHAR && column < left
				|| orphaned == Flags::WIDE_CHAR_SPACER && column > right;
			if !inside_band && !looking_outward {
				continue;
			}
			if grid[line][Column(column)].flags.contains(orphaned) {
				let partner = if orphaned == Flags::WIDE_CHAR {
					column + 1
				} else {
					column.wrapping_sub(1)
				};
				// A pair that is still whole spans the margin only if its two halves ended up on
				// opposite sides of it; one wholly inside or wholly outside moved together and is fine.
				let partner_inside = partner < cols && partner >= left && partner <= right;
				if partner < cols && partner_inside != inside_band {
					grid[line][Column(column)] = background.into();
				}
			}
		}
	}

	/// Answer a DECRQM about mode 69, which the engine would report as "not recognised" (§102).
	///
	/// `1` for set and `2` for reset, the two values DEC defines for a mode the terminal implements.
	/// The engine's own answer would be `0`, and once cmote implements the mode that answer is a lie
	/// a program acts on: a program told the mode is unknown will not ask for margins, and one told
	/// it is reset when it is set will place text in the wrong columns.
	fn report_margin_mode(&mut self) {
		let value = if self.margins.enabled() { 1 } else { 2 };
		let reply = format!("\x1b[?{LEFT_RIGHT_MARGIN_MODE};{value}$y");
		self.replies
			.lock()
			.expect("reply buffer mutex poisoned")
			.bytes
			.extend_from_slice(reply.as_bytes());
	}
}

// The forwarding table and the handful of methods cmote answers itself. The `deny` is the whole
// reason this build was possible at all — see the module header.
#[deny(clippy::missing_trait_methods)]
impl Handler for Gate<'_> {
	/// DECSTBM — mirrored on the way past, then performed by the engine as it always was (§102).
	///
	/// The mirror is updated FIRST and unconditionally, including for a request the engine will
	/// reject: `ScrollRegion::set` applies the engine's own `top >= bottom` test and leaves itself alone
	/// exactly where the engine leaves itself alone, so the two agree on malformed input as well as
	/// on good input. Then the call goes through untouched, because the engine is still the only
	/// thing that scrolls and it needs its own copy to do it with.
	fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
		self.region.set(top, bottom, self.term.screen_lines());
		self.term.set_scrolling_region(top, bottom);
	}

	/// RIS — the engine puts the region back to the whole page, so the mirror does too (§102).
	///
	/// This is one of the two reset paths a scanner beside the stream could never have seen. The
	/// bytes on the wire are `ESC c`, which says nothing about a scrolling region; that it clears one
	/// is a fact about the engine's insides, and the only place it surfaces is here.
	fn reset_state(&mut self) {
		self.term.reset_state();
		self.region.reset(self.term.screen_lines());
		self.margins.reset();
	}

	/// Print one character, breaking the line at the RIGHT MARGIN instead of at the screen edge.
	///
	/// This is the method the whole gate exists for. Everything else here could in principle have
	/// been done from beside the stream; this one could not, because by the time a glyph is on the
	/// grid it is already at the wrong columns.
	///
	/// The deferred wrap is the part worth reading twice. A terminal does NOT wrap when the last
	/// column of a line is filled — it leaves the cursor sitting on that column with the wrap owed,
	/// so a program that fills the line and then moves the cursor never wraps at all. The engine has
	/// its own flag for that (`input_needs_wrap`) and it is fired by the SCREEN edge, so with a right
	/// margin short of the edge it never fires, and with a right margin AT the edge it fires and
	/// wraps to column 0 rather than to the left margin. Either way the flag has to become cmote's:
	/// `hold_at_right_margin` takes it over after every glyph.
	fn input(&mut self, c: char) {
		if !self.narrowed() {
			self.term.input(c);
			return;
		}
		// A zero-width character combines with the cell already written and never moves the cursor,
		// so it cannot cross a margin — including while a wrap is owed, since the glyph it combines
		// with is the one sitting on the margin.
		let width = c.width().unwrap_or(0);
		if width == 0 {
			self.term.input(c);
			return;
		}
		// The wrap column is the right margin WHEREVER the cursor is, not only inside the band. That
		// is xterm's rule — `ScrnRightMargin` reads the mode, never the cursor — and it was worth
		// getting wrong once to be sure of: text starting left of the band flows rightward, meets the
		// right margin, and continues at the LEFT margin, so a program that sets margins and then
		// prints a full-width line gets it folded into the band. The tempting alternative, letting
		// text outside the band keep the whole page, is nobody's behaviour and would have been cmote
		// inventing a dialect (§57's rule, pointed the other way: where a reference implementation
		// has decided, matching it beats improving on it).
		self.take_pending_wrap();
		// A glyph too wide for what is left of the band breaks the line early. Without autowrap it is
		// dropped instead: a two-cell glyph printed at the right margin would otherwise put its
		// continuation in the next column of the page, which belongs to somebody else.
		if self.column() + width > self.margins.right() + 1 {
			if !self.autowrap() {
				return;
			}
			self.wrap_to_left_margin();
		}
		self.term.input(c);
		self.hold_at_right_margin();
	}

	/// CUP and HVP. Under origin mode the column is counted from the left margin (§102).
	fn goto(&mut self, line: i32, col: usize) {
		if !self.narrowed() {
			self.term.goto(line, col);
			return;
		}
		let column = self.margins.place(col, self.origin(), self.cols());
		self.term.goto(line, column);
		self.margins.set_pending_wrap(false);
	}

	/// VPA — the row alone, so the margins have nothing to say about it beyond cancelling a wrap the
	/// cursor is about to leave behind.
	fn goto_line(&mut self, line: i32) {
		self.term.goto_line(line);
		self.margins.set_pending_wrap(false);
	}

	/// CHA and HPA. Origin mode counts from the left margin here too.
	///
	/// Performed rather than forwarded once the margins are narrowed, which has a side effect worth
	/// naming: `set_column` writes the cursor directly and so avoids the engine's `goto_col` routing
	/// through `goto`, where origin mode drags the cursor DOWNWARD (§74). The defect is untouched on
	/// the path without margins, because fixing it there is a different change with its own row.
	fn goto_col(&mut self, col: usize) {
		if !self.narrowed() {
			self.term.goto_col(col);
			return;
		}
		let column = self.margins.place(col, self.origin(), self.cols());
		self.set_column(column);
	}

	/// ICH — insert blanks at the cursor, pushing the rest of the BAND right rather than the rest of
	/// the line.
	fn insert_blank(&mut self, count: usize) {
		if !self.narrowed() {
			self.term.insert_blank(count);
			return;
		}
		// Cleared and not taken: an insert does not move the cursor, so a wrap owed here is abandoned
		// rather than performed.
		self.margins.set_pending_wrap(false);
		self.shift_cells(count, true);
	}

	/// DCH — delete at the cursor and pull the rest of the BAND left.
	fn delete_chars(&mut self, count: usize) {
		if !self.narrowed() {
			self.term.delete_chars(count);
			return;
		}
		self.margins.set_pending_wrap(false);
		self.shift_cells(count, false);
	}

	/// CUF — forward, stopping at the right margin when the cursor started inside the band.
	fn move_forward(&mut self, cols: usize) {
		if !self.narrowed() {
			self.term.move_forward(cols);
			return;
		}
		let stop = self.margins.forward_stop(self.column(), self.cols());
		let target = self.column().saturating_add(cols).min(stop);
		self.set_column(target);
	}

	/// CUB — back, stopping at the left margin when the cursor started inside the band.
	fn move_backward(&mut self, cols: usize) {
		if !self.narrowed() {
			self.term.move_backward(cols);
			return;
		}
		let floor = self.margins.backstop(self.column(), self.cols());
		let target = self.column().saturating_sub(cols).max(floor);
		self.set_column(target);
	}

	/// CNL — down and to the LEFT MARGIN. The row is the engine's to work out; only the column moves.
	fn move_down_and_cr(&mut self, rows: usize) {
		self.term.move_down_and_cr(rows);
		if self.narrowed() {
			self.set_column(self.margins.left());
		}
	}

	/// CPL — up and to the left margin, the mirror of the one above.
	fn move_up_and_cr(&mut self, rows: usize) {
		self.term.move_up_and_cr(rows);
		if self.narrowed() {
			self.set_column(self.margins.left());
		}
	}

	/// HT — forward to the next tab stop, and no further than the right margin.
	///
	/// The stops themselves are the engine's table and are not margin-aware; a stop beyond the band
	/// simply cannot be reached from inside it, which is what a real terminal does and is why this
	/// clamps the arrival rather than filtering the table.
	fn put_tab(&mut self, count: u16) {
		if !self.narrowed() {
			self.term.put_tab(count);
			return;
		}
		self.take_pending_wrap();
		self.term.put_tab(count);
		if self.column() > self.margins.right() {
			self.set_column(self.margins.right());
		}
	}

	/// BS — back one column, stopping at the left margin.
	fn backspace(&mut self) {
		if !self.narrowed() {
			self.term.backspace();
			return;
		}
		// A backspace with a wrap owed cancels the wrap and stays put: the cursor is already sitting
		// on the last cell written, which is the cell a backspace was aiming for.
		if self.margins.pending_wrap() {
			self.margins.set_pending_wrap(false);
			return;
		}
		let floor = self.margins.backstop(self.column(), self.cols());
		if self.column() > floor {
			self.set_column(self.column() - 1);
		}
	}

	/// CR — to the LEFT MARGIN, or to column 1 when the cursor is left of the band.
	///
	/// The second half is the rule that lets a program address a status area outside its own band
	/// without being dragged back into it on the next carriage return.
	fn carriage_return(&mut self) {
		if !self.narrowed() {
			self.term.carriage_return();
			return;
		}
		let target = if self.column() >= self.margins.left() {
			self.margins.left()
		} else {
			0
		};
		self.set_column(target);
	}

	/// LF, VT, FF and IND — one row down, scrolling only the band if the cursor is already on the
	/// last row of the region.
	fn linefeed(&mut self) {
		if !self.narrowed() {
			self.term.linefeed();
			return;
		}
		// Cleared rather than taken: a line feed already moves down a row, and performing the owed
		// wrap first would move down twice.
		self.margins.set_pending_wrap(false);
		self.index();
	}

	/// NEL, and LF while LNM is set.
	///
	/// **Cannot be forwarded**, and this is the trap the whole gate has to be read for: the engine's
	/// own `newline` calls the ENGINE's `linefeed` and `carriage_return`, not the gate's. Handing it
	/// over would perform the two margin-blind versions of methods this file has just replaced. The
	/// same shape holds for `goto_line` and `goto_col`, which route through the engine's `goto`.
	fn newline(&mut self) {
		if !self.narrowed() {
			self.term.newline();
			return;
		}
		self.linefeed();
		if self.term.mode().contains(TermMode::LINE_FEED_NEW_LINE) {
			self.carriage_return();
		}
	}

	/// RI — one row up, scrolling the band down if the cursor is already on the region's first row.
	fn reverse_index(&mut self) {
		if !self.narrowed() {
			self.term.reverse_index();
			return;
		}
		self.margins.set_pending_wrap(false);
		self.reverse();
	}

	/// SU — scroll the band up through the whole scrolling region.
	fn scroll_up(&mut self, lines: usize) {
		if !self.narrowed() {
			self.term.scroll_up(lines);
			return;
		}
		self.scroll_band(self.region.first_row(), self.region.last_row(), lines, true);
	}

	/// SD — scroll the band down through the whole scrolling region.
	fn scroll_down(&mut self, lines: usize) {
		if !self.narrowed() {
			self.term.scroll_down(lines);
			return;
		}
		self.scroll_band(
			self.region.first_row(),
			self.region.last_row(),
			lines,
			false,
		);
	}

	/// IL — open blank lines at the cursor, scrolling the band below it down.
	///
	/// Refused outright when the cursor is outside the region or outside the margins, which is both
	/// the engine's own test for the region and xterm's for the margins. There is no band to open
	/// lines in from out there, and guessing one would move text the program had walled off.
	fn insert_blank_lines(&mut self, lines: usize) {
		if !self.narrowed() {
			self.term.insert_blank_lines(lines);
			return;
		}
		if !self.cursor_in_band() {
			return;
		}
		self.margins.set_pending_wrap(false);
		self.scroll_band(self.row(), self.region.last_row(), lines, false);
	}

	/// DL — close lines at the cursor, pulling the band below it up.
	fn delete_lines(&mut self, lines: usize) {
		if !self.narrowed() {
			self.term.delete_lines(lines);
			return;
		}
		if !self.cursor_in_band() {
			return;
		}
		self.margins.set_pending_wrap(false);
		self.scroll_band(self.row(), self.region.last_row(), lines, true);
	}

	/// DECSC and `CSI s` — the deferred wrap rides along with the cursor it belongs to.
	fn save_cursor_position(&mut self) {
		self.margins.save();
		self.term.save_cursor_position();
	}

	/// DECRC and `CSI u` — and back out again.
	fn restore_cursor_position(&mut self) {
		self.term.restore_cursor_position();
		self.margins.restore();
	}

	/// DECSET. Mode 69 is DECLRMM, which turns the margins on (§102).
	///
	/// The engine has no name for 69 and would drop it, which is why the sequence is answered here
	/// and not forwarded: there is nothing on the far side of this call that knows what it means.
	fn set_private_mode(&mut self, mode: PrivateMode) {
		if matches!(mode, PrivateMode::Unknown(LEFT_RIGHT_MARGIN_MODE)) {
			let cols = self.cols();
			self.margins.enable(true, cols);
			return;
		}
		self.term.set_private_mode(mode);
	}

	/// DECRST. Turning DECLRMM off throws the band away with it.
	fn unset_private_mode(&mut self, mode: PrivateMode) {
		if matches!(mode, PrivateMode::Unknown(LEFT_RIGHT_MARGIN_MODE)) {
			let cols = self.cols();
			self.margins.enable(false, cols);
			return;
		}
		self.term.unset_private_mode(mode);
	}

	/// DECRQM. The engine would answer "not recognised" for a mode cmote implements (§102).
	fn report_private_mode(&mut self, mode: PrivateMode) {
		if matches!(mode, PrivateMode::Unknown(LEFT_RIGHT_MARGIN_MODE)) {
			self.report_margin_mode();
			return;
		}
		self.term.report_private_mode(mode);
	}

	forward! {
		set_title(title: Option<String>),
		set_cursor_style(style: Option<CursorStyle>),
		set_cursor_shape(shape: CursorShape),
		move_up(lines: usize),
		move_down(lines: usize),
		identify_terminal(intermediate: Option<char>),
		device_status(argument: usize),
		bell(),
		substitute(),
		set_horizontal_tabstop(),
		erase_chars(count: usize),
		move_backward_tabs(count: u16),
		move_forward_tabs(count: u16),
		clear_line(mode: LineClearMode),
		clear_screen(mode: ClearMode),
		clear_tabs(mode: TabulationClearMode),
		set_tabs(interval: u16),
		terminal_attribute(attribute: Attr),
		set_mode(mode: Mode),
		unset_mode(mode: Mode),
		report_mode(mode: Mode),
		set_keypad_application_mode(),
		unset_keypad_application_mode(),
		set_active_charset(index: CharsetIndex),
		configure_charset(index: CharsetIndex, charset: StandardCharset),
		set_color(index: usize, color: Rgb),
		dynamic_color_sequence(prefix: String, index: usize, terminator: &str),
		reset_color(index: usize),
		clipboard_store(clipboard: u8, payload: &[u8]),
		clipboard_load(clipboard: u8, terminator: &str),
		decaln(),
		push_title(),
		pop_title(),
		text_area_size_pixels(),
		text_area_size_chars(),
		set_hyperlink(link: Option<Hyperlink>),
		set_mouse_cursor_icon(icon: CursorIcon),
		report_keyboard_mode(),
		push_keyboard_mode(mode: KeyboardModes),
		pop_keyboard_modes(to_pop: u16),
		set_keyboard_mode(mode: KeyboardModes, behavior: KeyboardModesApplyBehavior),
		set_modify_other_keys(mode: ModifyOtherKeys),
		report_modify_other_keys(),
		set_scp(char_path: ScpCharPath, update_mode: ScpUpdateMode),
	}
}

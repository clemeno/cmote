// ui/minimap.rs — the whole scrollback at a glance, beside the scrollbar (PLAN §180).
//
// The scrollbar says WHERE the view is in the history and nothing about what is there. The minimap
// is the history itself, shrunk into a column to the left of the bar: the lines as a few pixels of
// their own colours, the view as a slider over them, and a press anywhere on it scrolls the grid there.
// It is VS Code's minimap with `renderCharacters` off — blocks, never glyphs. A glyph two pixels
// tall is noise, and drawing text is the expensive part of a frame; a block per run of same-coloured
// cells is a quad, and the shape of the output is what the eye finds a place by anyway.
//
// It is a column the grid gives up, not an overlay on its cells. `ui::terminal::grid_size` takes
// `WIDTH` off the width before working out the columns, so the remote is told a narrower terminal
// and no cell is ever drawn under the map. The width is the same with or without history, on purpose:
// a column count that changed whenever the alternate screen came and went would resize the pty under
// every full-screen program as it started.
//
// Two scales, VS Code's "fit". While the whole document fits at `LINE` pixels a line, that is the
// scale, the map is drawn from the top, and the slider is the height the screen would be. Once it does
// not fit, the scale shrinks until the whole document does: lines share pixels and the map shows an
// even sample of them. So the oldest retained line is always on the map, and a press lands within a
// few lines of any line in the history — the thing a scrolling minimap gives up on a deep history.
//
// The geometry is pure and lives here; the grid calls `column` to know where the map is, `draw` to
// paint it, and `slider` / `offset` to drive it with the pointer, the same split as its own
// `scrollbar_thumb` and `scrollbar_offset`.
//
// The slider is named for VS Code's own and never "band": that word is DECSLRM's, the columns between
// the margins (§102).
//
// Under the ink, each finished shell command gets a command wash (§181): its lines tinted green, red
// or blue by the exit code its shell reported, so a failure stands out from a long way off.

use iced::advanced::renderer::{Quad, Renderer as _};
use iced::{Background, Border, Color, Rectangle};

use crate::term::screen::Screen;
use crate::ui::grid::{document_lines, to_iced_color};
use crate::ui::scrollbar;
use crate::ui::terminal::{CELL_HEIGHT, GRID_PADDING};

/// How wide the map's column is: what `ui::terminal::grid_size` takes off the grid's width, so about
/// nine cells at the grid's font size.
pub const WIDTH: f32 = 64.0;

/// The space from the map's left edge to its ink — so at least this much between the grid's last cell
/// and the ink, more where `grid_size` rounded the columns down — so the map does not read as more of
/// the line.
const GAP: f32 = 4.0;

/// How tall one document line is drawn while the whole document fits, and how far apart the painted
/// lines are once it does not.
const LINE: f32 = 2.0;

/// How wide one cell's ink is. One pixel a cell is VS Code's own scale, and it gives the map
/// `WIDTH - GAP` = 60 columns; a longer line is cut there, as VS Code cuts one at its own edge.
const CELL: f32 = 1.0;

/// How opaque the ink is: the text's own colour, faded so the map reads as a picture of the output
/// rather than as more of it.
const INK_ALPHA: f32 = 0.6;

/// The slider that marks what the grid is showing: the scrollbar's grey, faint enough that the ink under
/// it still shows (`ui::scrollbar::THUMB` is the same grey at 0.55).
const SLIDER: Color = Color::from_rgba(0.82, 0.82, 0.82, 0.15);

/// The command wash of a command that exited 0 (§181): Figma's light-theme success green.
const SUCCEEDED: Color = Color::from_rgb8(0x14, 0xae, 0x5c);

/// The wash of any other exit code, 130 (a Ctrl+C) among them: Figma's light-theme danger red.
const FAILED: Color = Color::from_rgb8(0xf2, 0x48, 0x22);

/// The wash of a command finished with no code — a bare `133;D`, or a field that is not a number:
/// Figma's light-theme brand blue.
const UNREPORTED: Color = Color::from_rgb8(0x0d, 0x99, 0xff);

/// How opaque one wash is: enough to tell green from red at a glance, faint enough that the ink over
/// it still reads. Where squeezed washes overlap they add up and come out denser.
const WASH_ALPHA: f32 = 0.3;

/// Where the map is in a grid of `bounds` showing `rows` rows: left of the scrollbar's gutter, and from
/// the first row's top to the last row's bottom — the scrollbar's own track, so the two run side by
/// side over the same height.
pub fn column(bounds: Rectangle, rows: u16) -> Rectangle {
	Rectangle {
		x: bounds.x + bounds.width - GRID_PADDING - WIDTH,
		y: bounds.y + GRID_PADDING,
		width: WIDTH,
		height: f32::from(rows) * CELL_HEIGHT,
	}
}

/// How many pixels one document line takes on a map `height` tall: `LINE` while every line fits at
/// that, then whatever fits all of them.
fn scale(height: f32, rows: u16, history: u16) -> f32 {
	(height / document_lines(rows, history)).min(LINE)
}

/// The slider over the map that marks the grid's own view: `rows` lines from the line on the grid's
/// top row, which is `history - offset` (§40). Exact, so `offset` can read it back; what is PAINTED is
/// `painted_slider`.
pub fn slider(column: Rectangle, rows: u16, history: u16, offset: u16) -> Rectangle {
	let scale = scale(column.height, rows, history);
	Rectangle {
		y: column.y + f32::from(history - offset.min(history)) * scale,
		height: f32::from(rows) * scale,
		..column
	}
}

/// The slider as painted: `slider`, never shorter than the scrollbar's own floor
/// (`scrollbar::MIN_THUMB`), centred on the exact one and kept inside the column.
///
/// The floor is the bar's reason (§23). Squeezed, the exact slider is the screen's share of the column:
/// 24 rows of a full 10 000-line history come out under one pixel tall, and at the slider's opacity
/// nothing shows where the view is. Only the paint is floored. A press still reads the exact slider, and
/// on one thinner than a pixel every press lands off it and centres the view under the pointer, which
/// is what a press on a mark that small means anyway.
fn painted_slider(column: Rectangle, rows: u16, history: u16, offset: u16) -> Rectangle {
	let exact = slider(column, rows, history, offset);
	let height = exact.height.max(scrollbar::MIN_THUMB).min(column.height);
	let middle = exact.y + exact.height / 2.0;
	Rectangle {
		y: (middle - height / 2.0).clamp(column.y, column.y + column.height - height),
		height,
		..exact
	}
}

/// The inverse of `slider`: the viewport offset that puts the slider's TOP at `top`, clamped into
/// `0..=history`, so a drag past either end stops at that end.
///
/// The slider's top and not the pointer, for the scrollbar's reason (`scrollbar_offset`): the caller
/// subtracts where in the slider the press caught it first, so the slider stays under the same part of
/// itself for the whole drag. Rounded through `lines_scrolled`, as the scrollbar's offset is: `slider`
/// puts line k's top at `k * scale`, and a top that comes back a hair under it would truncate to the
/// line above.
pub fn offset(column: Rectangle, rows: u16, history: u16, top: f32) -> u16 {
	let scale = scale(column.height, rows, history);
	// `scale` is zero only for a map with no height, which has nothing to press on.
	let top_line = if scale > 0.0 {
		crate::ui::lines_scrolled((top - column.y) / scale)
	} else {
		0
	};
	u16::try_from((i32::from(history) - top_line).clamp(0, i32::from(history)))
		.expect("clamped into 0..=history, a u16, on the line above")
}

/// The finished commands' washes in the order they are painted, each with its colour (§181): every
/// command but the failures oldest first, then the failures.
///
/// Failures last because squeezed washes overlap. Each is floored to a row (`wash`), so the short
/// commands after a failure would paint over it — three of them leave a tenth of its red — and a
/// failure is what the wash is there to show.
fn washes(
	commands: &[(std::ops::Range<u64>, Option<i32>)],
) -> impl Iterator<Item = (&std::ops::Range<u64>, Color)> {
	let failed = |exit: &Option<i32>| matches!(exit, Some(code) if *code != 0);
	let others = commands.iter().filter(move |(_, exit)| !failed(exit));
	let failures = commands.iter().filter(move |(_, exit)| failed(exit));
	others.chain(failures).map(|(lines, exit)| {
		let color = match exit {
			Some(0) => SUCCEEDED,
			Some(_) => FAILED,
			None => UNREPORTED,
		};
		(lines, color)
	})
}

/// Where one finished command's wash goes (§181): the column's whole width over the document `lines`
/// it spans, prompt to end. Never shorter than one painted row (`LINE`): squeezed, a one-line failure
/// would be about a thirtieth of a pixel (0.034 at 24 rows of a full history), and a failure is what
/// the wash is there to show. An empty span, a D on its prompt's own line, gets the same row.
fn wash(column: Rectangle, rows: u16, history: u16, lines: &std::ops::Range<u64>) -> Rectangle {
	let scale = scale(column.height, rows, history);
	let at = |line: u64| crate::ui::pixels(usize::try_from(line).unwrap_or(usize::MAX), scale);
	Rectangle {
		y: column.y + at(lines.start),
		height: (at(lines.end) - at(lines.start)).max(LINE),
		..column
	}
}

/// Which document line each painted row of the map shows, top to bottom, with the row's distance from
/// the map's top: every line while they fit, then an even sample of them, one every `LINE` pixels.
///
/// The FIRST line of each stretch a row stands for, which is a sample and not a summary. `ponytail:`
/// a lone red error line among a hundred plain ones can fall between two rows of a deep history; the
/// upgrade, if that is missed, is to paint the stretch's most-inked line, at its cost per frame.
///
/// Enough rows to COVER the document, the last one partial: a squeezed document rarely ends on a whole
/// row, and rounding down would leave the newest stretch — the latest screenful — off the map. The
/// partial row's paint runs up to `LINE` past the column's bottom, into the grid's padding. A row
/// starts inside the document, so its line is one the document has; where float rounding adds one row
/// at the very end, `line_ink` answers nothing for a line the session does not hold.
fn painted_lines(height: f32, rows: u16, history: u16) -> impl Iterator<Item = (f32, u64)> {
	let scale = scale(height, rows, history);
	let count = crate::ui::cells_covering(document_lines(rows, history) * scale, LINE);
	(0..count).map(move |row| {
		let y = crate::ui::pixels(row, LINE);
		let line = crate::ui::cells(y, scale);
		(
			y,
			u64::try_from(line).expect("a line index is far below u64::MAX"),
		)
	})
}

/// Paint the map of `screen` into `column`: the wash of each finished command in `commands` (absolute
/// lines and exit code, §181, in `washes`' order), each painted line's ink in its own colours over it,
/// then the slider over
/// the lines the grid is showing. `foreground` is the colour a cell's default foreground is drawn in,
/// which the grid knows and this file does not (DECSCNM swaps it, §149).
///
/// Nothing at all with no history, the scrollbar's rule (`scrollbar_thumb`): a map of only the screen
/// beside the screen says nothing, and on the alternate screen, which keeps none, a press on the
/// column then stays the program's.
pub fn draw(
	renderer: &mut iced::Renderer,
	screen: Screen<'_>,
	column: Rectangle,
	foreground: Color,
	commands: &[(std::ops::Range<u64>, Option<i32>)],
) {
	let (rows, _) = screen.size();
	let history = screen.history_size();
	if history == 0 {
		return;
	}
	for (lines, color) in washes(commands) {
		renderer.fill_quad(
			quad(wash(column, rows, history, lines)),
			Background::Color(Color {
				a: WASH_ALPHA,
				..color
			}),
		);
	}
	let left = column.x + GAP;
	let columns = crate::ui::cell_index(column.width - GAP, CELL);
	for (y, line) in painted_lines(column.height, rows, history) {
		for ink in screen.line_ink(line, columns) {
			renderer.fill_quad(
				quad(Rectangle {
					x: left + f32::from(ink.start) * CELL,
					y: column.y + y,
					width: f32::from(ink.width) * CELL,
					height: LINE,
				}),
				Background::Color(Color {
					a: INK_ALPHA,
					..to_iced_color(ink.fg, foreground)
				}),
			);
		}
	}
	renderer.fill_quad(
		quad(painted_slider(
			column,
			rows,
			history,
			screen.display_offset(),
		)),
		Background::Color(SLIDER),
	);
}

/// A plain fill, not snapped: once the document is squeezed the slider's top falls part-way through a
/// pixel, and snapping would move it a whole pixel at a time while the view moves a line.
fn quad(bounds: Rectangle) -> Quad {
	Quad {
		bounds,
		border: Border::default(),
		shadow: iced::Shadow::default(),
		snap: false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn close(left: f32, right: f32, what: &str) {
		assert!(
			(left - right).abs() < 0.01,
			"{what}: expected {right}, got {left}"
		);
	}

	/// A grid of 24 rows, 700 pixels wide: 86 columns beside the map.
	fn bounds() -> Rectangle {
		Rectangle {
			x: 10.0,
			y: 20.0,
			width: 700.0,
			height: 24.0 * CELL_HEIGHT + 2.0 * GRID_PADDING,
		}
	}

	#[test]
	fn the_column_takes_the_grids_freed_width_and_no_cell() {
		// `grid_size` takes WIDTH off the width, so for every width that holds the map the last cell
		// ends at or before the map's left edge — and the map ends where the scrollbar's gutter begins.
		for width in [200.0, 640.0, 641.3, 1234.5] {
			let area = iced::Size::new(width, 600.0);
			let (rows, cols) = crate::ui::terminal::grid_size(area, 0.0);
			let bounds = Rectangle {
				x: 0.0,
				y: 0.0,
				width,
				height: 600.0,
			};
			let column = column(bounds, rows);
			let last_cell = GRID_PADDING + f32::from(cols) * crate::ui::terminal::CELL_WIDTH;
			assert!(
				last_cell <= column.x + 0.01,
				"{width}: cells end at {last_cell}, the map starts at {}",
				column.x
			);
			close(
				column.x + column.width,
				width - GRID_PADDING,
				"up to the scrollbar's gutter",
			);
		}
	}

	#[test]
	fn a_short_history_is_drawn_line_for_line_from_the_top() {
		// 24 rows and 100 lines of history fit at LINE a line, so each line is LINE tall and the slider
		// is the screen's height at that scale, at the top line the view is showing.
		let column = column(bounds(), 24);
		let parked = slider(column, 24, 100, 40);
		close(
			parked.y,
			column.y + 60.0 * LINE,
			"line 60 is on the top row",
		);
		close(parked.height, 24.0 * LINE, "the screen's 24 lines");
		let lines: Vec<(f32, u64)> = painted_lines(column.height, 24, 100).collect();
		assert_eq!(lines.len(), 124, "every line, none sampled away");
		assert!(
			lines
				.iter()
				.enumerate()
				.all(|(row, &(y, line))| u64::try_from(row) == Ok(line)
					&& (y - crate::ui::pixels(row, LINE)).abs() < 0.01)
		);
	}

	#[test]
	fn a_deep_history_is_squeezed_until_all_of_it_fits() {
		// 5000 lines do not fit at LINE a line: the oldest is at the map's top and the live bottom at
		// its bottom, so every line in the history is somewhere a press can reach.
		let column = column(bounds(), 24);
		let oldest = slider(column, 24, 5000, 5000);
		close(oldest.y, column.y, "the oldest line at the top");
		let live = slider(column, 24, 5000, 0);
		close(
			live.y + live.height,
			column.y + column.height,
			"the live bottom at the bottom",
		);
		let lines: Vec<(f32, u64)> = painted_lines(column.height, 24, 5000).collect();
		assert_eq!(
			lines.len(),
			crate::ui::cells_covering(column.height, LINE),
			"one painted row every LINE pixels, enough to cover the column"
		);
		assert_eq!(lines[0].1, 0, "starting at the oldest line");
		assert!(
			lines.windows(2).all(|pair| pair[0].1 < pair[1].1),
			"in order, no line twice"
		);
		// And the sample spans the whole document rather than its oldest stretch: the last painted row
		// is within one stride of the last line, a stride being how many lines one row stands for.
		let stride = LINE / scale(column.height, 24, 5000);
		let last = lines.last().map_or(0, |&(_, line)| line);
		let last = f32::from(u16::try_from(last).expect("a line of a 5024-line document"));
		assert!(
			last < 5024.0 && last >= 5024.0 - stride.ceil(),
			"the last row shows line {last}, a stride being {stride}"
		);
	}

	#[test]
	fn offset_reads_the_slider_back_at_both_scales() {
		// `offset` is `slider` solved for the offset, so the round trip lands where it started — on
		// the short history drawn line for line, and on the deep one that is squeezed.
		let column = column(bounds(), 24);
		for history in [100, 5000] {
			for parked in [0, 1, 37, history / 2, history - 1, history] {
				let parked_slider = slider(column, 24, history, parked);
				assert_eq!(
					offset(column, 24, history, parked_slider.y),
					parked,
					"history {history}"
				);
			}
		}
	}

	#[test]
	fn a_top_most_of_the_way_to_the_next_line_reads_as_that_line() {
		// Rounded, not truncated: a slider top 0.6 of the way from line 59 to line 60 is line 60 on
		// the grid's top row, offset 40 — a truncation would say line 59, offset 41.
		let column = column(bounds(), 24);
		assert_eq!(offset(column, 24, 100, column.y + 59.6 * LINE), 40);
	}

	#[test]
	fn a_drag_past_either_end_stops_at_that_end() {
		let column = column(bounds(), 24);
		assert_eq!(offset(column, 24, 5000, column.y - 500.0), 5000);
		assert_eq!(offset(column, 24, 5000, column.y + 5000.0), 0);
		assert_eq!(offset(column, 24, 100, f32::INFINITY), 0);
		assert_eq!(offset(column, 24, 100, f32::NEG_INFINITY), 100);
	}

	#[test]
	fn a_wash_covers_its_commands_lines_and_never_less_than_a_row() {
		// Drawn line for line, a command on lines 10..13 is washed over exactly those three, across
		// the whole column. Squeezed into a 10 000-line history, a one-line command is still a row.
		let column = column(bounds(), 24);
		let short = wash(column, 24, 100, &(10..13));
		close(short.y, column.y + 10.0 * LINE, "from its prompt");
		close(short.height, 3.0 * LINE, "to its end");
		close(short.x, column.x, "the column's left edge");
		close(short.width, WIDTH, "the column's whole width");
		let deep = wash(column, 24, 10_000, &(5000..5001));
		close(deep.height, LINE, "floored at one painted row");
		// Above the floor a squeezed wash is the scale's height, not LINE a line: 1000 lines are the
		// distance the slider moves over them, about 35 pixels and not 2000.
		close(
			wash(column, 24, 10_000, &(1000..2000)).height,
			slider(column, 24, 10_000, 8000).y - slider(column, 24, 10_000, 9000).y,
			"squeezed like the slider",
		);
		close(
			deep.y,
			slider(column, 24, 10_000, 5000).y,
			"on the line the slider puts there",
		);
	}

	#[test]
	fn failures_are_washed_last_each_command_in_its_exit_codes_colour() {
		// Oldest first, but every failure after the rest, so no short command painted later can cover
		// one: green for 0, blue for no code, red for 1 and for 130.
		let commands = [
			(0..2, Some(1)),
			(2..4, Some(0)),
			(4..6, None),
			(6..8, Some(130)),
		];
		let painted: Vec<(std::ops::Range<u64>, Color)> = washes(&commands)
			.map(|(lines, color)| (lines.clone(), color))
			.collect();
		assert_eq!(
			painted,
			vec![
				(2..4, SUCCEEDED),
				(4..6, UNREPORTED),
				(0..2, FAILED),
				(6..8, FAILED),
			]
		);
	}

	#[test]
	fn a_squeezed_slider_is_painted_at_the_bars_floor_and_inside_the_map() {
		// A full history at 24 rows: the exact slider is under a pixel, the painted one is the bar's
		// floor, centred on it in the middle and held inside the column at both ends.
		let column = column(bounds(), 24);
		assert!(slider(column, 24, 10_000, 5000).height < 1.0, "the premise");
		let middle = painted_slider(column, 24, 10_000, 5000);
		let exact = slider(column, 24, 10_000, 5000);
		close(middle.height, scrollbar::MIN_THUMB, "floored");
		close(
			middle.y + middle.height / 2.0,
			exact.y + exact.height / 2.0,
			"centred on the view",
		);
		close(
			painted_slider(column, 24, 10_000, 10_000).y,
			column.y,
			"the oldest end",
		);
		let live = painted_slider(column, 24, 10_000, 0);
		close(
			live.y + live.height,
			column.y + column.height,
			"the live end",
		);
		// A slider already taller than the floor is painted as it is.
		assert_eq!(
			painted_slider(column, 24, 100, 40),
			slider(column, 24, 100, 40)
		);
	}
}

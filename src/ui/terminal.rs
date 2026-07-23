// ui/terminal.rs — render the vt100 `Screen` grid as iced widgets (PLAN §9-§10).
//
// The emulator gives us a grid of cells; this draws it. Each screen row is a
// `row` of fixed-width boxes: consecutive same-styled *narrow* cells coalesce
// into one box (its width is exactly `n × CELL_WIDTH`), while a *wide* cell (CJK,
// emoji — §11) gets its own box two cells across. Pinning each box to an exact
// multiple of the cell width is what keeps columns aligned: a wide glyph our
// bundled font can't draw falls back to a system font whose advance we don't
// control, so free-flowing text would shift everything after it — the fixed box
// reserves the two columns regardless of how wide the fallback glyph actually is.
// Background fills the whole box (so a narrow glyph in a wide box still tiles);
// foreground, bold, and underline come from each cell; the cursor cell is drawn
// inverted so it is visible.

use iced::font::Weight;
use iced::widget::text::{LineHeight, Span, Wrapping};
use iced::widget::{button, column, container, rich_text, row, span, text};
use iced::{Color, Element, Font, Length, Size};

use crate::app::Message;
use crate::term::Terminal;

/// Glyph size and line spacing. A fixed monospace metric — the whole grid shares
/// it, so columns line up and rows tile without gaps.
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.2;

/// The bundled monospace font (Fira Mono, embedded in the binary — see
/// `app::MONO_FONT`). Naming it explicitly instead of `Font::MONOSPACE` means the
/// grid looks identical on every machine AND its cell advance is known exactly,
/// which is what makes the pixel↔cell resize math below correct (§9, §11).
const TERMINAL_FONT: Font = Font::with_name("Fira Mono");

/// One monospace cell in logical pixels. Height is the line box; width uses Fira
/// Mono's exact advance ratio (600/1000 em = 0.6), so both axes are precise —
/// no per-font guesswork, which is why we bundle a known font.
const CELL_WIDTH: f32 = FONT_SIZE * 0.6;
const CELL_HEIGHT: f32 = FONT_SIZE * LINE_HEIGHT;

/// Padding between the grid and its container edge. Named so `view` (which draws
/// it) and `grid_size` (which subtracts it) can never drift apart.
const GRID_PADDING: f32 = 6.0;

/// The status bar above the grid (§10): a fixed height plus its colors, text
/// size, and side padding. The fixed height matters twice — `view` renders the
/// bar at exactly this height AND `grid_size` subtracts it, so the reflow math
/// accounts for the space the bar takes and the two can never drift (the same
/// discipline as `GRID_PADDING`).
const STATUS_BAR_HEIGHT: f32 = 34.0;
const STATUS_BAR_TEXT: f32 = 13.0;
const STATUS_BAR_BG: Color = Color::from_rgb8(0x2d, 0x2d, 0x2d);
const STATUS_BAR_FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const STATUS_BAR_PADDING: iced::Padding = iced::Padding {
	top: 0.0,
	right: 10.0,
	bottom: 0.0,
	left: 10.0,
};

/// The default foreground/background when a cell asks for the "default" color —
/// a light-on-dark scheme, and the window's backdrop behind the whole grid.
const DEFAULT_FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const DEFAULT_BG: Color = Color::from_rgb8(0x1e, 0x1e, 0x1e);

/// The 16 base ANSI colors (indices 0-15): the 8 standard colors then their
/// bright variants. Values follow the common xterm palette.
const ANSI_16: [(u8, u8, u8); 16] = [
	(0x00, 0x00, 0x00), // 0 black
	(0x80, 0x00, 0x00), // 1 red
	(0x00, 0x80, 0x00), // 2 green
	(0x80, 0x80, 0x00), // 3 yellow
	(0x00, 0x00, 0x80), // 4 blue
	(0x80, 0x00, 0x80), // 5 magenta
	(0x00, 0x80, 0x80), // 6 cyan
	(0xc0, 0xc0, 0xc0), // 7 white
	(0x80, 0x80, 0x80), // 8 bright black (gray)
	(0xff, 0x00, 0x00), // 9 bright red
	(0x00, 0xff, 0x00), // 10 bright green
	(0xff, 0xff, 0x00), // 11 bright yellow
	(0x00, 0x00, 0xff), // 12 bright blue
	(0xff, 0x00, 0xff), // 13 bright magenta
	(0x00, 0xff, 0xff), // 14 bright cyan
	(0xff, 0xff, 0xff), // 15 bright white
];

/// The six intensity steps of the 6×6×6 color cube (indices 16-231).
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// The resolved look of one cell: everything a span needs. Grouping key too —
/// consecutive cells with an equal `CellStyle` become one span.
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
	fg: Color,
	bg: Color,
	bold: bool,
	underline: bool,
}

/// Render the whole terminal screen (§10): a status bar on top, the vt100 grid
/// filling the rest. `endpoint` is the `user@host:port` shown in the bar. Owns
/// its output (glyph strings and the label are copied out), so the returned
/// element borrows nothing and is `'static`.
pub fn view(terminal: &Terminal, endpoint: &str) -> Element<'static, Message> {
	let screen = terminal.screen();
	let (rows, cols) = screen.size();
	let (cursor_row, cursor_col) = screen.cursor_position();
	let cursor_visible = !screen.hide_cursor();

	let mut lines: Vec<Element<'static, Message>> = Vec::with_capacity(rows as usize);
	for row in 0..rows {
		let on_cursor_row = cursor_visible && row == cursor_row;
		lines.push(render_row(screen, row, cols, on_cursor_row, cursor_col));
	}

	// The grid, on the dark backdrop, filling the space left under the status bar.
	let grid = container(column(lines).spacing(0))
		.style(|_theme| container::Style {
			background: Some(DEFAULT_BG.into()),
			..container::Style::default()
		})
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(GRID_PADDING);

	// Bar on top (fixed height), grid below it filling the remaining window.
	column![status_bar(endpoint), grid]
		.spacing(0)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
}

/// The status bar (§10): the `user@host:port` of the live session on the left, a
/// Disconnect button on the right. Its height is fixed to `STATUS_BAR_HEIGHT` so
/// `grid_size` can subtract it exactly. The label is copied in, so the returned
/// element owns its text and stays `'static` like the grid.
fn status_bar(endpoint: &str) -> Element<'static, Message> {
	// `width(Fill)` on the label pushes the button to the right edge.
	let info = text(endpoint.to_owned())
		.size(STATUS_BAR_TEXT)
		.color(STATUS_BAR_FG)
		.width(Length::Fill);
	let disconnect =
		button(text("Disconnect").size(STATUS_BAR_TEXT)).on_press(Message::DisconnectPressed);

	container(
		row![info, disconnect]
			.spacing(10)
			.align_y(iced::alignment::Vertical::Center),
	)
	.style(|_theme| container::Style {
		background: Some(STATUS_BAR_BG.into()),
		..container::Style::default()
	})
	.width(Length::Fill)
	.height(Length::Fixed(STATUS_BAR_HEIGHT))
	.padding(STATUS_BAR_PADDING)
	.into()
}

/// The (rows, cols) grid that fits `area` logical pixels, laid out exactly as
/// `view` draws it: the status bar takes `STATUS_BAR_HEIGHT` off the top, then
/// the grid's own padding is subtracted on both axes. Rounds down so the last
/// cell is never clipped, and clamps to at least 1×1 so the emulator always has
/// a valid size. The app calls this on a window resize to reflow both the local
/// emulator and the remote pty (§9).
pub fn grid_size(area: Size) -> (u16, u16) {
	let usable_width = area.width - 2.0 * GRID_PADDING;
	let usable_height = area.height - STATUS_BAR_HEIGHT - 2.0 * GRID_PADDING;
	let cols = (usable_width / CELL_WIDTH)
		.floor()
		.clamp(1.0, f32::from(u16::MAX)) as u16;
	let rows = (usable_height / CELL_HEIGHT)
		.floor()
		.clamp(1.0, f32::from(u16::MAX)) as u16;
	(rows, cols)
}

/// One box's worth of the grid: a string of glyphs, the look they share, and how
/// many grid columns the box spans. A narrow run spans its glyph count; a single
/// wide cell spans two. Split out from rendering so the column-packing logic can
/// be unit-tested without building any widgets.
struct Run {
	content: String,
	style: CellStyle,
	cols: u16,
}

/// Pack one screen row into boxes (§11). Walks the row left to right, growing a
/// run while cells are narrow and share a style, and sealing a wide cell into its
/// own two-column run so a following cell can never merge into it (which would
/// mis-size the box). Wide *continuation* cells are skipped — the lead already
/// reserves their column.
fn plan_runs(
	screen: &vt100::Screen,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
) -> Vec<Run> {
	let mut runs: Vec<Run> = Vec::new();
	let mut content = String::new();
	// The open run: its style, its column span so far, and whether it is a (sealed)
	// wide run. `None` means no run is open yet.
	let mut current: Option<(CellStyle, u16, bool)> = None;

	for col in 0..cols {
		let cell = screen.cell(row, col);

		// The trailing half of a wide glyph: its column was already claimed by the
		// lead cell's two-column box, so emit nothing for it.
		if cell.is_some_and(vt100::Cell::is_wide_continuation) {
			continue;
		}

		let is_wide = cell.is_some_and(vt100::Cell::is_wide);
		let glyph = match cell {
			Some(cell) if cell.has_contents() => cell.contents().to_string(),
			_ => " ".to_string(),
		};
		let is_cursor = on_cursor_row && col == cursor_col;
		let style = cell_style(cell, is_cursor);

		// Extend only when this cell is narrow AND the open run is a narrow run of
		// the same style; a wide cell (or a wide open run) always breaks the run.
		let extend =
			matches!(current, Some((run_style, _, false)) if !is_wide && run_style == style);
		if extend {
			content.push_str(&glyph);
			if let Some((_, span_cols, _)) = current.as_mut() {
				*span_cols += 1;
			}
		} else {
			if let Some((run_style, span_cols, _)) = current.take() {
				runs.push(Run {
					content: std::mem::take(&mut content),
					style: run_style,
					cols: span_cols,
				});
			}
			content.push_str(&glyph);
			current = Some((style, if is_wide { 2 } else { 1 }, is_wide));
		}
	}
	if let Some((run_style, span_cols, _)) = current {
		runs.push(Run {
			content,
			style: run_style,
			cols: span_cols,
		});
	}
	runs
}

/// Build one screen row as a `row` of fixed-width boxes (§11), one per packed run.
fn render_row(
	screen: &vt100::Screen,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
) -> Element<'static, Message> {
	let boxes: Vec<Element<'static, Message>> =
		plan_runs(screen, row, cols, on_cursor_row, cursor_col)
			.into_iter()
			.map(|run| cell_box(run.content, run.style, run.cols))
			.collect();

	// Fully qualified: the `row` parameter above shadows the `row` widget helper.
	iced::widget::row(boxes).spacing(0).into()
}

/// One fixed-width cell box: the glyph(s) drawn in a container pinned to exactly
/// `span_cols × CELL_WIDTH` (§11). The container carries the background so it fills
/// the whole box — including any slack a fallback wide glyph leaves — and clips so
/// an over-wide fallback glyph can't spill past its columns and shove the next box.
fn cell_box(content: String, style: CellStyle, span_cols: u16) -> Element<'static, Message> {
	// One span holds the run's glyphs; `Wrapping::None` keeps it on a single line
	// even when the text's measured width grazes the box width.
	let glyphs = rich_text(vec![make_span(content, style)])
		.size(FONT_SIZE)
		.line_height(LineHeight::Relative(LINE_HEIGHT))
		.wrapping(Wrapping::None);

	container(glyphs)
		.width(Length::Fixed(f32::from(span_cols) * CELL_WIDTH))
		.height(Length::Fixed(CELL_HEIGHT))
		.clip(true)
		.style(move |_theme| container::Style {
			background: Some(style.bg.into()),
			..container::Style::default()
		})
		.into()
}

/// Resolve a cell's colors and attributes into a `CellStyle`, applying inverse
/// video and the cursor highlight (each swaps fg/bg; together they cancel, which
/// matches how a real terminal draws the cursor over already-inverted text).
fn cell_style(cell: Option<&vt100::Cell>, is_cursor: bool) -> CellStyle {
	let (mut fg, mut bg, bold, underline) = match cell {
		Some(cell) => (
			resolve(cell.fgcolor(), DEFAULT_FG),
			resolve(cell.bgcolor(), DEFAULT_BG),
			cell.bold(),
			cell.underline(),
		),
		None => (DEFAULT_FG, DEFAULT_BG, false, false),
	};

	let inverse = cell.is_some_and(vt100::Cell::inverse);
	if inverse ^ is_cursor {
		std::mem::swap(&mut fg, &mut bg);
	}

	CellStyle {
		fg,
		bg,
		bold,
		underline,
	}
}

/// Map a vt100 color to an iced color. `Default` becomes the caller's default
/// (different for fg and bg); indexed colors go through the xterm-256 palette.
fn resolve(color: vt100::Color, default: Color) -> Color {
	match color {
		vt100::Color::Default => default,
		vt100::Color::Idx(index) => xterm_256(index),
		vt100::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
	}
}

/// The xterm 256-color palette: 0-15 base ANSI, 16-231 a 6×6×6 cube, 232-255 a
/// 24-step grayscale ramp.
fn xterm_256(index: u8) -> Color {
	if index < 16 {
		let (r, g, b) = ANSI_16[index as usize];
		return Color::from_rgb8(r, g, b);
	}
	if index < 232 {
		let value = index - 16;
		let r = CUBE_STEPS[(value / 36) as usize];
		let g = CUBE_STEPS[((value / 6) % 6) as usize];
		let b = CUBE_STEPS[(value % 6) as usize];
		return Color::from_rgb8(r, g, b);
	}
	let level = 8 + (index - 232) * 10;
	Color::from_rgb8(level, level, level)
}

/// Build a styled span for one run of same-styled cells. Foreground, weight, and
/// underline live here; the background is painted by the enclosing `cell_box` so it
/// fills the whole fixed-width box rather than only the glyphs' advance (§11).
fn make_span(content: String, style: CellStyle) -> Span<'static, ()> {
	// Pick the weight we actually bundled: Medium (500) for normal cells, Bold (700)
	// for bold. This MUST match a bundled weight exactly. We ship Fira Mono only at
	// 500 and 700 (no 400 "Regular"), and cosmic-text — with the whole system font
	// DB present at runtime — does NOT nearest-weight-match within a named family:
	// asking for `Weight::Normal` (400) finds no "Fira Mono" at 400 and silently
	// falls back to the platform default (a *proportional* font, e.g. Segoe UI),
	// which breaks the monospace grid. Medium/Bold both resolve to our real faces,
	// and every Fira Mono weight shares the 0.6 advance, so cells stay `CELL_WIDTH`.
	let font = Font {
		weight: if style.bold {
			Weight::Bold
		} else {
			Weight::Medium
		},
		..TERMINAL_FONT
	};
	span(content)
		.font(font)
		.size(FONT_SIZE)
		.color(style.fg)
		.underline(style.underline)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;

	#[test]
	fn grid_fits_area_minus_bar_and_padding_rounding_down() {
		// width:  (812 - 12)      / 8.4  = 95.2  -> 95 cols
		// height: (500 - 34 - 12) / 16.8 = 27.02 -> 27 rows  (34 = status bar)
		let (rows, cols) = grid_size(Size::new(812.0, 500.0));
		assert_eq!((rows, cols), (27, 95));
	}

	#[test]
	fn tiny_area_clamps_to_at_least_one_cell() {
		// Smaller than the padding would give a negative count; clamp to 1×1.
		assert_eq!(grid_size(Size::new(1.0, 1.0)), (1, 1));
	}

	// Pack row 0 of a grid after feeding `input` to a fresh emulator. The cursor is
	// left out (`on_cursor_row = false`) so the tests exercise the column packing
	// alone, not the cursor's inverse-video split.
	fn row_runs(input: &str, cols: u16) -> Vec<Run> {
		let mut terminal = Terminal::new(1, cols);
		terminal.process(input.as_bytes());
		plan_runs(terminal.screen(), 0, cols, false, 0)
	}

	#[test]
	fn narrow_cells_of_one_style_coalesce_into_a_single_box() {
		// "hello" plus trailing spaces are all the default style, so the whole row is
		// one box spanning every column.
		let runs = row_runs("hello", 20);
		assert_eq!(runs.len(), 1);
		assert!(runs[0].content.starts_with("hello"));
		assert_eq!(runs[0].cols, 20);
	}

	#[test]
	fn a_wide_glyph_gets_its_own_two_column_box() {
		// 世 is East-Asian-wide: it must be sealed into a two-column box, with the
		// narrow cells on either side kept in their own boxes.
		let cols = 10;
		let runs = row_runs("a世b", cols);
		assert_eq!(runs.len(), 3);
		assert_eq!((runs[0].content.as_str(), runs[0].cols), ("a", 1));
		assert_eq!((runs[1].content.as_str(), runs[1].cols), ("世", 2));
		assert!(runs[2].content.starts_with('b'));
		assert_eq!(runs[2].cols, cols - 3); // b + trailing spaces
	}

	#[test]
	fn packed_runs_cover_every_grid_column_exactly_once() {
		// The box widths must sum to the grid width — each wide glyph claims two
		// columns and each continuation claims none, so nothing is lost or doubled.
		let cols = 12;
		let runs = row_runs("x世y世z", cols);
		let total: u16 = runs.iter().map(|run| run.cols).sum();
		assert_eq!(total, cols);
	}
}

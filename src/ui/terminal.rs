// ui/terminal.rs — render the vt100 `Screen` grid as iced widgets (PLAN §9-§10).
//
// The emulator gives us a grid of cells; this draws it. Each screen row becomes
// one `rich_text` line, and within a row we coalesce runs of same-styled cells
// into a single `span` (fewer, wider text runs render faster and keep a
// monospace font perfectly aligned). Colors, bold, and underline come straight
// from each cell; the cursor cell is drawn inverted so it is visible.

use iced::font::Weight;
use iced::widget::text::{LineHeight, Span};
use iced::widget::{column, container, rich_text, span};
use iced::{Color, Element, Font, Length};

use crate::app::Message;
use crate::term::Terminal;

/// Glyph size and line spacing. A fixed monospace metric — the whole grid shares
/// it, so columns line up and rows tile without gaps.
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.2;

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

/// Render the whole terminal grid. Owns its output (glyph strings are copied out
/// of the grid), so the returned element borrows nothing and is `'static`.
pub fn view(terminal: &Terminal) -> Element<'static, Message> {
	let screen = terminal.screen();
	let (rows, cols) = screen.size();
	let (cursor_row, cursor_col) = screen.cursor_position();
	let cursor_visible = !screen.hide_cursor();

	let mut lines: Vec<Element<'static, Message>> = Vec::with_capacity(rows as usize);
	for row in 0..rows {
		let on_cursor_row = cursor_visible && row == cursor_row;
		lines.push(render_row(screen, row, cols, on_cursor_row, cursor_col));
	}

	// The grid, on the dark backdrop, filling the window.
	container(column(lines).spacing(0))
		.style(|_theme| container::Style {
			background: Some(DEFAULT_BG.into()),
			..container::Style::default()
		})
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(6)
		.into()
}

/// Build one screen row as a `rich_text` line, coalescing equal-styled cells.
fn render_row(
	screen: &vt100::Screen,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
) -> Element<'static, Message> {
	let mut spans: Vec<Span<'static, ()>> = Vec::new();
	let mut run = String::new();
	let mut run_style: Option<CellStyle> = None;

	for col in 0..cols {
		let cell = screen.cell(row, col);

		// A wide glyph (e.g. CJK) occupies two columns: the lead cell holds it and
		// the next is a continuation. `ponytail:` skip the continuation — the lead
		// glyph may not span two monospace cells, so wide text can misalign. Rare
		// for a shell prompt; proper wide-cell handling is a later refinement.
		if cell.is_some_and(vt100::Cell::is_wide_continuation) {
			continue;
		}

		let glyph = match cell {
			Some(cell) if cell.has_contents() => cell.contents().to_string(),
			_ => " ".to_string(),
		};
		let is_cursor = on_cursor_row && col == cursor_col;
		let style = cell_style(cell, is_cursor);

		// Extend the current run while the style matches; otherwise flush it.
		if run_style == Some(style) {
			run.push_str(&glyph);
		} else {
			if let Some(previous) = run_style.take() {
				spans.push(make_span(std::mem::take(&mut run), previous));
			}
			run.push_str(&glyph);
			run_style = Some(style);
		}
	}
	if let Some(previous) = run_style {
		spans.push(make_span(run, previous));
	}

	rich_text(spans)
		.size(FONT_SIZE)
		.line_height(LineHeight::Relative(LINE_HEIGHT))
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

/// Build a styled span for one run of same-styled cells.
fn make_span(content: String, style: CellStyle) -> Span<'static, ()> {
	let font = Font {
		weight: if style.bold {
			Weight::Bold
		} else {
			Weight::Normal
		},
		..Font::MONOSPACE
	};
	span(content)
		.font(font)
		.size(FONT_SIZE)
		.color(style.fg)
		.background(style.bg)
		.underline(style.underline)
}

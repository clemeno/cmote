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
use iced::widget::{
	button, column, container, mouse_area, rich_text, row, span, stack, text, text_editor,
};
use iced::{Color, Element, Font, Length, Point, Size};

use crate::app::Message;
use crate::term::Terminal;
use crate::ui::selection::{Cell, Selection};

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

/// The background of a selected cell (§10). A muted blue that reads clearly under
/// the default light foreground; selected cells keep their own fg, only the fill
/// changes, so text stays legible while the region is obviously highlighted.
const SELECTION_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);

/// The right-click context menu's panel background (§10) — slightly lighter than
/// the status bar so it stands out as a floating surface over the grid.
const MENU_BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);

/// The body copy for the disconnect confirmation dialog (§10). Public so `app` can
/// seed it into the selectable dialog buffer when the modal opens.
pub const DISCONNECT_DIALOG_BODY: &str = "Ends this shell and returns to the connect form. The remote program is signalled to close; what happens to any unsaved work there is up to that program.";

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
/// filling the rest. `endpoint` is the `user@host:port` shown in the bar,
/// `selection` the active text selection to highlight (if any), `menu` the
/// right-click context menu's anchor when it is open, and `dialog_body` the
/// selectable message buffer for the disconnect confirmation. The grid's own output
/// (glyph strings, the label) is copied out and so is `'static`; the returned element
/// borrows only `dialog_body`, so its lifetime is tied to that.
pub fn view<'a>(
	terminal: &Terminal,
	endpoint: &str,
	selection: Option<&Selection>,
	menu: Option<Point>,
	confirm_disconnect: bool,
	dialog_body: &'a text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	let screen = terminal.screen();
	let (rows, cols) = screen.size();
	let (cursor_row, cursor_col) = screen.cursor_position();
	let cursor_visible = !screen.hide_cursor();

	let mut lines: Vec<Element<'static, Message>> = Vec::with_capacity(rows as usize);
	for row in 0..rows {
		let on_cursor_row = cursor_visible && row == cursor_row;
		lines.push(render_row(
			screen,
			row,
			cols,
			on_cursor_row,
			cursor_col,
			selection,
		));
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

	// The grid reacts to the mouse (§10): press-drag-release drives the text
	// selection and a right-press opens the context menu. `on_move` reports a point
	// local to the grid, which `app` maps to a cell via `cell_at`.
	let interactive_grid = mouse_area(grid)
		.on_press(Message::GridPressed)
		.on_move(Message::GridMoved)
		.on_release(Message::GridReleased)
		.on_right_press(Message::GridRightPressed);

	// Copy is only meaningful with a non-empty selection; the buttons/menu key off this.
	let has_selection = selection.is_some_and(|selection| !selection.is_empty());

	// Bar on top (fixed height), grid below it filling the remaining window.
	let base = column![status_bar(endpoint, has_selection), interactive_grid]
		.spacing(0)
		.width(Length::Fill)
		.height(Length::Fill);

	// Overlays stack on top of the base, bottom-to-top: the right-click menu (with a
	// click-away dismiss layer), then the Disconnect confirmation modal. The base and
	// overlay layers are `'static`; the confirmation panel borrows `dialog_body`, so the
	// vector — and the whole view — takes that `'a` lifetime.
	let mut layers: Vec<Element<'a, Message>> = vec![base.into()];
	if let Some(point) = menu {
		layers.push(dismiss_layer());
		layers.push(context_menu(point, has_selection));
	}
	if confirm_disconnect {
		layers.push(crate::ui::dialog::backdrop(Message::DisconnectCancelled));
		layers.push(confirm_disconnect_panel(dialog_body, drag));
	}

	// A lone base needs no stack; otherwise layer the overlays over it.
	if layers.len() == 1 {
		layers.pop().expect("layers holds the base element")
	} else {
		stack(layers)
			.width(Length::Fill)
			.height(Length::Fill)
			.into()
	}
}

/// The status bar (§10): three zones — Copy / Paste on the left, the live session's
/// `user@host:port` centered, and Disconnect on the right. Its height is fixed to
/// `STATUS_BAR_HEIGHT` so `grid_size` can subtract it exactly. `has_selection`
/// enables Copy — with nothing selected the button has no `on_press` and iced
/// renders it disabled. The label is copied in, so the returned element owns its
/// text and stays `'static` like the grid.
fn status_bar(endpoint: &str, has_selection: bool) -> Element<'static, Message> {
	// `on_press_maybe(None)` disables Copy until there is a selection to copy.
	let copy = button(text("Copy").size(STATUS_BAR_TEXT))
		.on_press_maybe(has_selection.then_some(Message::CopyPressed));
	let paste = button(text("Paste").size(STATUS_BAR_TEXT)).on_press(Message::PastePressed);
	let disconnect =
		button(text("Disconnect").size(STATUS_BAR_TEXT)).on_press(Message::DisconnectPressed);

	// Three equal-width zones. Because each takes the same `Fill` share, the middle
	// zone's centered label is centered in the *window*, not merely between the side
	// groups — so the host info stays put no matter how wide Copy/Paste/Disconnect are.
	let left = container(row![copy, paste].spacing(10))
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Left);
	let center = container(
		text(endpoint.to_owned())
			.size(STATUS_BAR_TEXT)
			.color(STATUS_BAR_FG),
	)
	.width(Length::Fill)
	.align_x(iced::alignment::Horizontal::Center);
	let right = container(disconnect)
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Right);

	container(
		row![left, center, right]
			.spacing(10)
			.align_y(iced::alignment::Vertical::Center),
	)
	.style(|_theme| container::Style {
		background: Some(STATUS_BAR_BG.into()),
		..container::Style::default()
	})
	.width(Length::Fill)
	.height(Length::Fixed(STATUS_BAR_HEIGHT))
	// Centre the row within the fixed-height bar; the row's own `align_y` only aligns
	// its children to each other, not the row inside this taller container.
	.align_y(iced::alignment::Vertical::Center)
	.padding(STATUS_BAR_PADDING)
	.into()
}

/// The right-click context menu (§10): a small floating panel with Copy selection
/// and Paste, anchored at the click. Copy is disabled without a selection (same
/// rule as the status bar). `point` is local to the grid, which sits below the
/// status bar in the stack, so shift it down by the bar height to place the panel
/// under the cursor. `ponytail:` no edge clamping — near the window's right/bottom
/// the panel can run past the edge; good enough for v1.
fn context_menu(point: Point, has_selection: bool) -> Element<'static, Message> {
	let copy = button(text("Copy selection").size(STATUS_BAR_TEXT))
		.on_press_maybe(has_selection.then_some(Message::CopyPressed));
	let paste = button(text("Paste").size(STATUS_BAR_TEXT)).on_press(Message::PastePressed);

	let panel = container(column![copy, paste].spacing(2))
		.style(|_theme| container::Style {
			background: Some(MENU_BG.into()),
			..container::Style::default()
		})
		.padding(4);

	// A full-size transparent container whose padding positions the panel at the
	// click point (top-left aligned by default).
	container(panel)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(iced::Padding {
			top: point.y + STATUS_BAR_HEIGHT,
			right: 0.0,
			bottom: 0.0,
			left: point.x,
		})
		.into()
}

/// A full-window invisible layer that sits under the context menu (§10): any click
/// that misses the menu lands here and dismisses it. Right-press dismisses too, so
/// a second right-click never stacks two menus.
fn dismiss_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_press(Message::MenuDismissed)
		.on_right_press(Message::MenuDismissed)
		.into()
}

/// The Disconnect confirmation modal (§10): the shared dialog chrome
/// (`ui::dialog`) with the question in the header, a line explaining what confirming
/// does, and Cancel / Disconnect in the footer. Sits above `dim_backdrop` in the
/// stack; because Disconnect drops a live session, it takes an explicit confirm here
/// rather than acting on the status-bar button directly. The header's close (✕) and
/// the backdrop both emit `DisconnectCancelled`, so dismissing never disconnects.
fn confirm_disconnect_panel(
	dialog_body: &text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'_, Message> {
	crate::ui::dialog::dialog(
		"Disconnect from this session?".to_owned(),
		Message::DisconnectCancelled,
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel")
				.on_press(Message::DisconnectCancelled)
				.into(),
			button("Disconnect")
				.on_press(Message::DisconnectConfirmed)
				.into(),
		],
		drag,
	)
}

/// Map a pointer position (local to the grid, as `mouse_area::on_move` reports it)
/// to the grid cell under it (§10). Subtracts the grid padding, divides by the cell
/// metrics, and clamps into the grid so a drag past an edge selects the edge cell
/// rather than a phantom one off the grid.
pub fn cell_at(point: Point, rows: u16, cols: u16) -> Cell {
	let x = (point.x - GRID_PADDING).max(0.0);
	let y = (point.y - GRID_PADDING).max(0.0);
	// `as u16` truncates toward zero; x/y are non-negative, so this floors.
	let col = (x / CELL_WIDTH) as u16;
	let row = (y / CELL_HEIGHT) as u16;
	Cell {
		row: row.min(rows.saturating_sub(1)),
		col: col.min(cols.saturating_sub(1)),
	}
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

/// The window (logical) size whose content fits exactly a `cols`×`rows` grid — the
/// inverse of `grid_size`, built from the same metrics so the two never drift. Adds the
/// grid padding on both axes and the status-bar height, plus half a cell of slack so
/// float rounding in `grid_size` cannot come back a row/column short. `run` uses it to
/// open the window sized for a chosen terminal size (§10, §11).
pub fn window_size(cols: u16, rows: u16) -> Size {
	let width = f32::from(cols) * CELL_WIDTH + 2.0 * GRID_PADDING + CELL_WIDTH / 2.0;
	let height =
		f32::from(rows) * CELL_HEIGHT + STATUS_BAR_HEIGHT + 2.0 * GRID_PADDING + CELL_HEIGHT / 2.0;
	Size::new(width, height)
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
	selection: Option<&Selection>,
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
		let is_selected = selection.is_some_and(|selection| selection.contains(row, col));
		let style = cell_style(cell, is_cursor, is_selected);

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
	selection: Option<&Selection>,
) -> Element<'static, Message> {
	let boxes: Vec<Element<'static, Message>> =
		plan_runs(screen, row, cols, on_cursor_row, cursor_col, selection)
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
/// matches how a real terminal draws the cursor over already-inverted text). A
/// selected cell then takes the selection fill, keeping its foreground so the text
/// stays legible; because `CellStyle` is the run-grouping key, this also breaks the
/// selected span off from its neighbours automatically (§10).
fn cell_style(cell: Option<&vt100::Cell>, is_cursor: bool, is_selected: bool) -> CellStyle {
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

	// The selection fill wins over the resolved background so the highlight reads
	// uniformly across the run regardless of the cells' own colors.
	if is_selected {
		bg = SELECTION_BG;
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

	#[test]
	fn window_size_fits_the_requested_grid() {
		// A window opened via `window_size` must reflow back to exactly that grid, so the
		// initial window is wide enough for the intended column count (§11).
		assert_eq!(grid_size(window_size(160, 40)), (40, 160));
	}

	// Pack row 0 of a grid after feeding `input` to a fresh emulator. The cursor is
	// left out (`on_cursor_row = false`) so the tests exercise the column packing
	// alone, not the cursor's inverse-video split.
	fn row_runs(input: &str, cols: u16) -> Vec<Run> {
		let mut terminal = Terminal::new(1, cols);
		terminal.process(input.as_bytes());
		plan_runs(terminal.screen(), 0, cols, false, 0, None)
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

	#[test]
	fn cell_at_maps_pixels_to_cells_and_clamps() {
		// Just inside the padded top-left is cell (0, 0).
		let origin = cell_at(Point::new(GRID_PADDING + 1.0, GRID_PADDING + 1.0), 24, 80);
		assert_eq!((origin.row, origin.col), (0, 0));

		// One cell right and one cell down.
		let next = cell_at(
			Point::new(
				GRID_PADDING + CELL_WIDTH + 0.5,
				GRID_PADDING + CELL_HEIGHT + 0.5,
			),
			24,
			80,
		);
		assert_eq!((next.row, next.col), (1, 1));

		// Far past the grid clamps to the last cell, never off the grid.
		let clamped = cell_at(Point::new(100_000.0, 100_000.0), 24, 80);
		assert_eq!((clamped.row, clamped.col), (23, 79));
	}

	#[test]
	fn a_selection_breaks_into_its_own_highlighted_run() {
		// Selecting columns 1-2 of an all-default row splits it into three runs
		// (before / selected / after); only the middle carries the selection fill —
		// proof the highlight is both applied and isolated to the selection.
		let mut terminal = Terminal::new(1, 5);
		terminal.process(b"abcde");
		let selection = Selection::new(Cell { row: 0, col: 1 }).with_head(Cell { row: 0, col: 2 });
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, Some(&selection));

		// "a" | "bc" (selected) | "de"
		assert_eq!(runs.len(), 3);
		assert_eq!(runs[1].content, "bc");
		assert_eq!(runs[1].style.bg, SELECTION_BG);
		assert_ne!(runs[0].style.bg, SELECTION_BG);
		assert_ne!(runs[2].style.bg, SELECTION_BG);
	}
}

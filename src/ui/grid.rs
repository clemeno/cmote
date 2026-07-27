// ui/grid.rs — the vt100 screen drawn as ONE widget (PLAN §9).
//
// The emulator gives us a grid of cells; this draws it. Every glyph is placed at the
// exact pixel its column starts at, and every background is a quad of exactly the cells
// it covers. That placement rule is the whole point of the widget:
//
//   * A glyph our bundled font does not have (btop's braille graphs, a rounded box
//     corner, an emoji) falls back to a system font whose advance we do not control. Laid
//     out as flowing text it drags everything after it sideways and the row stops lining
//     up with the rest of the screen. Positioned per cell, a fallback glyph can only be
//     the wrong *shape* — never in the wrong *place*.
//   * A full-screen program repaints the whole grid many times a second, and a truecolor
//     one (btop again) gives nearly every cell its own color, so the runs of same-styled
//     cells that coalesce on a shell prompt do not coalesce at all. As widgets that is
//     tens of thousands of layout nodes per frame; as quads and cached text draws it is
//     the same work a terminal has always done.
//
// Runs of consecutive same-styled ASCII cells are still drawn as one string — that is the
// common case and the cheap one. Anything non-ASCII is sealed into its own run so its
// width can never leak into its neighbours, and braille (U+2800-U+28FF) skips text
// rendering altogether: the code point IS a 2x4 dot bitmap, so we draw the dots.
//
// The widget also answers the mouse for programs that asked for it (§9): when the remote
// has turned a mouse protocol on, a click, a release and a scroll become reports on the
// input channel instead of local text selection. Holding Shift takes the mouse back —
// the xterm convention — and a bare move is never captured, so the pane's own hover
// tracking keeps working underneath.

use iced::advanced::Renderer as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::Quad;
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, text};
use iced::keyboard::Modifiers;
use iced::{
	Background, Border, Color, Element, Font, Length, Pixels, Point, Rectangle, Size, Theme,
	alignment, mouse,
};
use vt100::MouseProtocolMode;

use crate::app::Message;
use crate::term::mouse as report;
use crate::ui::selection::{Cell, Selection};
use crate::ui::terminal::{CELL_HEIGHT, CELL_WIDTH, FONT_SIZE, GRID_PADDING, cell_at};

/// The bundled monospace font (Fira Mono, embedded in the binary — see `app::MONO_FONT`).
/// Naming it explicitly instead of `Font::MONOSPACE` means the grid looks identical on
/// every machine AND its cell advance is known exactly, which is what makes the pixel↔cell
/// math correct (§9).
const TERMINAL_FONT: Font = Font::with_name("Fira Mono");

/// The default foreground/background when a cell asks for the "default" color — a
/// light-on-dark scheme, and the window's backdrop behind the whole grid.
const DEFAULT_FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const DEFAULT_BG: Color = Color::from_rgb8(0x1e, 0x1e, 0x1e);

/// The background of a selected cell (§10). A muted blue that reads clearly under the
/// default light foreground; selected cells keep their own fg, only the fill changes, so
/// text stays legible while the region is obviously highlighted.
const SELECTION_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);

/// How thick an underlined cell's rule is, and how far above the cell's bottom edge it
/// sits. `fill_text` draws glyphs only, so the rule is a quad of our own.
const UNDERLINE_THICKNESS: f32 = 1.0;

/// The stroke of a box-drawing line we draw ourselves (the rounded corners). One logical
/// pixel — what Fira Mono's own ─ and │ come out at over the font sizes the grid uses, so
/// a drawn corner joins a shaped line without a step.
const LINE_THICKNESS: f32 = 1.0;

/// A braille cell is a 2x4 grid of dots, and the dot's diameter as a fraction of one of
/// those sub-cells. Below 1.0 the dots stay separate, which is what makes a braille graph
/// read as a graph rather than a solid block.
const BRAILLE_COLUMNS: f32 = 2.0;
const BRAILLE_ROWS: f32 = 4.0;
const BRAILLE_DOT: f32 = 0.72;

/// Where each bit of a braille code point sits, as (column, row) in that 2x4 grid. The
/// order is the standard one: bits 0-2 run down the left column, 3-5 down the right, and
/// the two low dots 6 and 7 were added underneath when braille grew from six dots to
/// eight — which is why they are not in reading order.
const BRAILLE_DOTS: [(f32, f32); 8] = [
	(0.0, 0.0),
	(0.0, 1.0),
	(0.0, 2.0),
	(1.0, 0.0),
	(1.0, 1.0),
	(1.0, 2.0),
	(0.0, 3.0),
	(1.0, 3.0),
];

/// The 16 base ANSI colors (indices 0-15): the 8 standard colors then their bright
/// variants. Values follow the common xterm palette.
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

/// The terminal grid widget. Borrows the emulator's screen for the frame rather than
/// copying it — a full screen of cells is exactly the thing not worth cloning 60 times a
/// second.
pub struct Grid<'a> {
	screen: &'a vt100::Screen,
	selection: Option<&'a Selection>,
}

/// Draw the emulator's current screen, highlighting `selection` if there is one.
pub fn grid<'a>(screen: &'a vt100::Screen, selection: Option<&'a Selection>) -> Grid<'a> {
	Grid { screen, selection }
}

/// What the widget remembers between events: the modifiers (they arrive on their own
/// event, not on the mouse ones), which button the remote program believes is down, and
/// the last cell a move was reported for, so dragging inside one cell stays quiet.
#[derive(Debug, Default)]
struct State {
	modifiers: Modifiers,
	held: Option<report::Button>,
	last: Option<Cell>,
}

impl Widget<Message, Theme, iced::Renderer> for Grid<'_> {
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<State>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(State::default())
	}

	fn size(&self) -> Size<Length> {
		Size::new(Length::Fill, Length::Fill)
	}

	fn layout(
		&mut self,
		_tree: &mut Tree,
		_renderer: &iced::Renderer,
		limits: &layout::Limits,
	) -> layout::Node {
		layout::atomic(limits, Length::Fill, Length::Fill)
	}

	fn draw(
		&self,
		_tree: &Tree,
		renderer: &mut iced::Renderer,
		_theme: &Theme,
		_style: &iced::advanced::renderer::Style,
		layout: Layout<'_>,
		_cursor: mouse::Cursor,
		viewport: &Rectangle,
	) {
		let bounds = layout.bounds();
		let Some(visible) = bounds.intersection(viewport) else {
			return;
		};

		// The backdrop, once, behind everything: every cell that keeps the default
		// background then costs no quad of its own.
		renderer.fill_quad(fill(bounds), Background::Color(DEFAULT_BG));

		let (rows, cols) = self.screen.size();
		let (cursor_row, cursor_col) = self.screen.cursor_position();
		let cursor_visible = !self.screen.hide_cursor();
		let origin = Point::new(bounds.x + GRID_PADDING, bounds.y + GRID_PADDING);

		renderer.with_layer(visible, |renderer| {
			for row in 0..rows {
				let top = origin.y + f32::from(row) * CELL_HEIGHT;
				// A glyph is clipped to its own row, never to its own cell: a fallback
				// glyph a shade too wide should lean on its neighbour rather than lose a
				// slice of itself, and either way the next cell still starts where it must.
				let row_bounds = Rectangle {
					x: origin.x,
					y: top,
					width: f32::from(cols) * CELL_WIDTH,
					height: CELL_HEIGHT,
				};
				for run in plan_runs(
					self.screen,
					row,
					cols,
					cursor_visible && row == cursor_row,
					cursor_col,
					self.selection,
				) {
					draw_run(renderer, run, origin.x, top, row_bounds);
				}
			}
		});
	}

	fn update(
		&mut self,
		tree: &mut Tree,
		event: &iced::Event,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		_renderer: &iced::Renderer,
		_clipboard: &mut dyn Clipboard,
		shell: &mut Shell<'_, Message>,
		_viewport: &Rectangle,
	) {
		let state = tree.state.downcast_mut::<State>();
		if let iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) = event {
			state.modifiers = *modifiers;
		}

		// No mouse protocol on, or Shift held: the pointer is the user's, for selecting
		// text and opening our own menu. Nothing is captured, so the layers above act as
		// they always have. A button already down is the exception — its press went to the
		// program, so its release and its drag must too, or the program is left believing
		// the button never came up.
		let mode = self.screen.mouse_protocol_mode();
		if mode == MouseProtocolMode::None || (state.modifiers.shift() && state.held.is_none()) {
			return;
		}
		let iced::Event::Mouse(pointer) = event else {
			return;
		};
		let bounds = layout.bounds();
		let Some(position) = cursor.position() else {
			return;
		};
		let inside = bounds.contains(position);
		let (rows, cols) = self.screen.size();
		let cell = cell_at(
			Point::new(position.x - bounds.x, position.y - bounds.y),
			rows,
			cols,
		);

		let pointer_event = match pointer {
			mouse::Event::ButtonPressed(button) => {
				let Some(button) = press_button(*button) else {
					return;
				};
				if !inside {
					return;
				}
				state.held = Some(button);
				report::Event::Press(button)
			}
			mouse::Event::ButtonReleased(button) => {
				let Some(button) = press_button(*button) else {
					return;
				};
				// A release outside the grid still belongs to the press that started
				// inside it, so the program is not left believing the button is down.
				if !inside && state.held != Some(button) {
					return;
				}
				state.held = None;
				report::Event::Release(button)
			}
			mouse::Event::WheelScrolled { delta } => {
				let Some(button) = wheel_button(*delta) else {
					return;
				};
				if !inside {
					return;
				}
				report::Event::Press(button)
			}
			mouse::Event::CursorMoved { .. } => {
				if !inside || state.last == Some(cell) {
					return;
				}
				state.last = Some(cell);
				report::Event::Motion(state.held)
			}
			_ => return,
		};

		let Some(bytes) = report::encode(
			mode,
			self.screen.mouse_protocol_encoding(),
			pointer_event,
			cell.row,
			cell.col,
			state.modifiers,
		) else {
			return;
		};
		shell.publish(Message::MouseReport(bytes));
		// A click belongs to the program, not to our selection or context menu, so it is
		// captured. A move is left alone: the hover tracking above us still wants it, and
		// the program has already been told.
		if !matches!(pointer_event, report::Event::Motion(_)) {
			shell.capture_event();
		}
	}
}

impl<'a> From<Grid<'a>> for Element<'a, Message> {
	fn from(grid: Grid<'a>) -> Self {
		Element::new(grid)
	}
}

/// Draw one packed run: its background, its underline rule, then its glyphs — braille as
/// dots we place ourselves, everything else as text pinned to the run's first column.
fn draw_run(
	renderer: &mut iced::Renderer,
	run: Run,
	grid_left: f32,
	top: f32,
	row_bounds: Rectangle,
) {
	let left = grid_left + f32::from(run.col) * CELL_WIDTH;
	let width = f32::from(run.cols) * CELL_WIDTH;
	let bounds = Rectangle {
		x: left,
		y: top,
		width,
		height: CELL_HEIGHT,
	};

	// The backdrop already covers every default-background cell.
	if run.style.bg != DEFAULT_BG {
		renderer.fill_quad(fill(bounds), Background::Color(run.style.bg));
	}
	if run.style.underline {
		renderer.fill_quad(
			fill(Rectangle {
				y: top + CELL_HEIGHT - UNDERLINE_THICKNESS,
				height: UNDERLINE_THICKNESS,
				..bounds
			}),
			Background::Color(run.style.fg),
		);
	}

	if let Some(dots) = braille(&run.content) {
		draw_braille(renderer, dots, bounds, run.style.fg);
		return;
	}
	if let Some(toward) = rounded_corner(&run.content) {
		draw_corner(renderer, toward, bounds, run.style.fg);
		return;
	}
	// Blank cells are most of a screen; their background is already drawn.
	if run.content.bytes().all(|byte| byte == b' ') {
		return;
	}

	// ASCII is guaranteed to be in the bundled font, so it can skip shaping and font
	// fallback entirely. Anything else gets the full path — that is what finds the glyph
	// in a system font when ours has not got it.
	let shaping = if run.content.is_ascii() {
		text::Shaping::Basic
	} else {
		text::Shaping::Advanced
	};
	renderer.fill_text(
		text::Text {
			content: run.content,
			bounds: Size::new(width, CELL_HEIGHT),
			size: Pixels(FONT_SIZE),
			line_height: text::LineHeight::Absolute(Pixels(CELL_HEIGHT)),
			font: Font {
				weight: if run.style.bold {
					iced::font::Weight::Bold
				} else {
					// Pick the weight we actually bundled: Medium (500) for normal cells,
					// Bold (700) for bold. This MUST match a bundled weight exactly. We ship
					// Fira Mono only at 500 and 700 (no 400 "Regular"), and cosmic-text — with
					// the whole system font DB present at runtime — does NOT nearest-weight-
					// match within a named family: asking for `Weight::Normal` (400) finds no
					// "Fira Mono" at 400 and silently falls back to the platform default (a
					// *proportional* font), which breaks the grid. Medium/Bold both resolve to
					// our real faces, and every Fira Mono weight shares the 0.6 advance.
					iced::font::Weight::Medium
				},
				..TERMINAL_FONT
			},
			align_x: text::Alignment::Left,
			align_y: alignment::Vertical::Top,
			shaping,
			wrapping: text::Wrapping::None,
		},
		Point::new(left, top),
		run.style.fg,
		row_bounds,
	);
}

/// Draw a braille cell as its dots. No monospace font we could bundle carries the braille
/// block, and the fallback fonts that do are proportional — but the code point spells the
/// pattern out bit by bit, so drawing it is both smaller than an extra font and exactly
/// the right size for the cell. `pattern` is the low byte of the code point.
fn draw_braille(renderer: &mut iced::Renderer, pattern: u8, bounds: Rectangle, color: Color) {
	let step_x = bounds.width / BRAILLE_COLUMNS;
	let step_y = bounds.height / BRAILLE_ROWS;
	let diameter = step_x.min(step_y) * BRAILLE_DOT;
	for (bit, (column, row)) in BRAILLE_DOTS.iter().enumerate() {
		if pattern & (1 << bit) == 0 {
			continue;
		}
		renderer.fill_quad(
			Quad {
				bounds: Rectangle {
					x: bounds.x + column * step_x + (step_x - diameter) / 2.0,
					y: bounds.y + row * step_y + (step_y - diameter) / 2.0,
					width: diameter,
					height: diameter,
				},
				border: Border::default().rounded(diameter / 2.0),
				shadow: iced::Shadow::default(),
				snap: false,
			},
			Background::Color(color),
		);
	}
}

/// Draw one rounded box corner. Fira Mono has the square corners but not these four, so a
/// fallback font would draw them at its own size and stroke in the middle of a run of our
/// own box lines — visibly the wrong shape where two boxes meet. Drawing them puts the
/// stroke exactly where the neighbouring ─ and │ are: a quarter arc, then the straight
/// tails that carry the line out to the cell's edges. `toward` says which way those two
/// lines leave the cell.
fn draw_corner(renderer: &mut iced::Renderer, toward: (f32, f32), bounds: Rectangle, color: Color) {
	let parts = corner_parts(bounds, toward);

	// The arc is one quadrant of a ring: a circular quad whose *border* is the stroke and
	// whose fill is nothing, clipped to the quarter facing the cell's centre.
	renderer.with_layer(parts.quadrant, |renderer| {
		renderer.fill_quad(
			Quad {
				bounds: parts.ring,
				border: Border {
					color,
					width: LINE_THICKNESS,
					radius: (parts.ring.width / 2.0).into(),
				},
				shadow: iced::Shadow::default(),
				snap: false,
			},
			Background::Color(Color::TRANSPARENT),
		);
	});

	for tail in [parts.across, parts.down].into_iter().flatten() {
		renderer.fill_quad(fill(tail), Background::Color(color));
	}
}

/// The pieces one rounded corner is drawn from, in absolute coordinates. Split out from
/// the drawing so the geometry — the part that can be wrong — is testable without a
/// renderer.
struct CornerParts {
	/// The full circle whose border is the stroke; only a quarter of it is ever shown.
	ring: Rectangle,
	/// The quadrant of that circle the arc lives in, used as the clip.
	quadrant: Rectangle,
	/// The straight run from where the arc leaves to the cell's left or right edge, and to
	/// its top or bottom edge. Either is `None` when the arc already reaches that edge.
	across: Option<Rectangle>,
	down: Option<Rectangle>,
}

/// Work out those pieces. The arc's radius is half the cell's short side, so it is as
/// round as the cell allows; its centre sits that far toward the corner the two lines
/// leave by, which puts the arc's two ends exactly on the cell's centre lines — where the
/// straight box characters in the neighbouring cells run.
fn corner_parts(bounds: Rectangle, (toward_x, toward_y): (f32, f32)) -> CornerParts {
	let radius = bounds.width.min(bounds.height) / 2.0;
	let mid_x = bounds.x + bounds.width / 2.0;
	let mid_y = bounds.y + bounds.height / 2.0;
	let center_x = mid_x + toward_x * radius;
	let center_y = mid_y + toward_y * radius;

	CornerParts {
		ring: Rectangle {
			x: center_x - radius,
			y: center_y - radius,
			width: radius * 2.0,
			height: radius * 2.0,
		},
		quadrant: Rectangle {
			x: if toward_x > 0.0 {
				center_x - radius
			} else {
				center_x
			},
			y: if toward_y > 0.0 {
				center_y - radius
			} else {
				center_y
			},
			width: radius,
			height: radius,
		},
		across: tail(center_x, bounds.x, bounds.x + bounds.width, toward_x).map(
			|(start, length)| Rectangle {
				x: start,
				y: mid_y - LINE_THICKNESS / 2.0,
				width: length,
				height: LINE_THICKNESS,
			},
		),
		down: tail(center_y, bounds.y, bounds.y + bounds.height, toward_y).map(
			|(start, length)| Rectangle {
				x: mid_x - LINE_THICKNESS / 2.0,
				y: start,
				width: LINE_THICKNESS,
				height: length,
			},
		),
	}
}

/// The straight run from `from` to whichever edge `toward` points at, as (start, length),
/// or `None` when there is nothing left to draw. With a cell twice as tall as it is wide
/// the arc already spans the full half-width, so the horizontal tail is always empty and
/// the vertical one never is.
fn tail(from: f32, low: f32, high: f32, toward: f32) -> Option<(f32, f32)> {
	let (start, length) = if toward > 0.0 {
		(from, high - from)
	} else {
		(low, from - low)
	};
	(length > 0.0).then_some((start, length))
}

/// Which way a rounded corner's two lines leave the cell, or `None` if the run is not one
/// of the four. As with braille, a lone cell is all that can qualify — runs are sealed at
/// every non-ASCII glyph.
fn rounded_corner(content: &str) -> Option<(f32, f32)> {
	let mut chars = content.chars();
	let first = chars.next()?;
	if chars.next().is_some() {
		return None;
	}
	match first {
		'╭' => Some((1.0, 1.0)),   // arc down and right
		'╮' => Some((-1.0, 1.0)),  // arc down and left
		'╯' => Some((-1.0, -1.0)), // arc up and left
		'╰' => Some((1.0, -1.0)),  // arc up and right
		_ => None,
	}
}

/// The braille pattern a run carries, or `None` if it is not a single braille cell. Only
/// a lone cell qualifies: runs are sealed at every non-ASCII glyph, so a braille cell is
/// always alone in its run.
fn braille(content: &str) -> Option<u8> {
	let mut chars = content.chars();
	let first = chars.next()?;
	if chars.next().is_some() {
		return None;
	}
	let code = u32::from(first).checked_sub(0x2800)?;
	(code < 0x100).then_some(code as u8)
}

/// A plain rectangle fill: no border, no shadow, snapped to the pixel grid so adjacent
/// cell backgrounds tile without a seam.
fn fill(bounds: Rectangle) -> Quad {
	Quad {
		bounds,
		border: Border::default(),
		shadow: iced::Shadow::default(),
		snap: true,
	}
}

/// The protocol button an iced mouse button maps to, or `None` for the ones the protocol
/// has no number for (back/forward and friends).
fn press_button(button: mouse::Button) -> Option<report::Button> {
	match button {
		mouse::Button::Left => Some(report::Button::Left),
		mouse::Button::Middle => Some(report::Button::Middle),
		mouse::Button::Right => Some(report::Button::Right),
		_ => None,
	}
}

/// Which way a scroll went, as the protocol's wheel button. A horizontal-only scroll
/// reports nothing — the protocol's wheel is vertical.
fn wheel_button(delta: mouse::ScrollDelta) -> Option<report::Button> {
	let y = match delta {
		mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => y,
	};
	if y > 0.0 {
		Some(report::Button::WheelUp)
	} else if y < 0.0 {
		Some(report::Button::WheelDown)
	} else {
		None
	}
}

/// The resolved look of one cell: everything a draw needs. Grouping key too — consecutive
/// cells with an equal `CellStyle` become one run.
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
	fg: Color,
	bg: Color,
	bold: bool,
	underline: bool,
}

/// One draw's worth of the grid: a string of glyphs, the look they share, the column they
/// start at, and how many columns they span. Split out from drawing so the packing logic
/// can be unit-tested without a renderer.
struct Run {
	content: String,
	style: CellStyle,
	col: u16,
	cols: u16,
}

/// Pack one screen row into runs (§9). Walks the row left to right, growing a run while
/// cells are narrow, ASCII, and share a style; anything else is sealed into a run of its
/// own so its width cannot leak into its neighbours — a wide cell claims two columns, a
/// non-ASCII one claims its own single column whatever the fallback font does with it.
/// Wide *continuation* cells are skipped: the lead already reserves their column.
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
	// The open run: its style, the column it starts at, its span so far, and whether it is
	// sealed (nothing may join it). `None` means no run is open yet.
	let mut current: Option<(CellStyle, u16, u16, bool)> = None;

	for col in 0..cols {
		let cell = screen.cell(row, col);

		// The trailing half of a wide glyph: its column was already claimed by the lead
		// cell's two-column run, so emit nothing for it.
		if cell.is_some_and(vt100::Cell::is_wide_continuation) {
			continue;
		}

		let is_wide = cell.is_some_and(vt100::Cell::is_wide);
		let glyph = match cell {
			Some(cell) if cell.has_contents() => cell.contents().to_string(),
			_ => " ".to_string(),
		};
		let seals = is_wide || !glyph.is_ascii();
		let is_cursor = on_cursor_row && col == cursor_col;
		let is_selected = selection.is_some_and(|selection| selection.contains(row, col));
		let style = cell_style(cell, is_cursor, is_selected);

		// Extend only when this cell joins freely AND the open run is an unsealed run of
		// the same style.
		let extend =
			matches!(current, Some((run_style, _, _, false)) if !seals && run_style == style);
		if extend {
			content.push_str(&glyph);
			if let Some((_, _, span_cols, _)) = current.as_mut() {
				*span_cols += 1;
			}
		} else {
			if let Some((run_style, start, span_cols, _)) = current.take() {
				runs.push(Run {
					content: std::mem::take(&mut content),
					style: run_style,
					col: start,
					cols: span_cols,
				});
			}
			content.push_str(&glyph);
			current = Some((style, col, if is_wide { 2 } else { 1 }, seals));
		}
	}
	if let Some((run_style, start, span_cols, _)) = current {
		runs.push(Run {
			content,
			style: run_style,
			col: start,
			cols: span_cols,
		});
	}
	runs
}

/// Resolve a cell's colors and attributes into a `CellStyle`, applying inverse video and
/// the cursor highlight (each swaps fg/bg; together they cancel, which matches how a real
/// terminal draws the cursor over already-inverted text). A selected cell then takes the
/// selection fill, keeping its foreground so the text stays legible; because `CellStyle`
/// is the run-grouping key, this also breaks the selected run off from its neighbours
/// automatically (§10).
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

/// Map a vt100 color to an iced color. `Default` becomes the caller's default (different
/// for fg and bg); indexed colors go through the xterm-256 palette.
fn resolve(color: vt100::Color, default: Color) -> Color {
	match color {
		vt100::Color::Default => default,
		vt100::Color::Idx(index) => xterm_256(index),
		vt100::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
	}
}

/// The xterm 256-color palette: 0-15 base ANSI, 16-231 a 6×6×6 cube, 232-255 a 24-step
/// grayscale ramp.
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;

	// Pack row 0 of a grid after feeding `input` to a fresh emulator. The cursor is left
	// out (`on_cursor_row = false`) so the tests exercise the column packing alone, not
	// the cursor's inverse-video split.
	fn row_runs(input: &str, cols: u16) -> Vec<Run> {
		let mut terminal = Terminal::new(1, cols);
		terminal.process(input.as_bytes());
		plan_runs(terminal.screen(), 0, cols, false, 0, None)
	}

	#[test]
	fn narrow_ascii_cells_of_one_style_coalesce_into_a_single_run() {
		// "hello" plus trailing spaces are all the default style and all ASCII, so the
		// whole row is one run spanning every column from column 0.
		let runs = row_runs("hello", 20);
		assert_eq!(runs.len(), 1);
		assert!(runs[0].content.starts_with("hello"));
		assert_eq!((runs[0].col, runs[0].cols), (0, 20));
	}

	#[test]
	fn a_wide_glyph_gets_its_own_two_column_run() {
		// 世 is East-Asian-wide: it must be sealed into a two-column run, with the narrow
		// cells on either side kept in their own runs.
		let cols = 10;
		let runs = row_runs("a世b", cols);
		assert_eq!(runs.len(), 3);
		assert_eq!(
			(runs[0].content.as_str(), runs[0].col, runs[0].cols),
			("a", 0, 1)
		);
		assert_eq!(
			(runs[1].content.as_str(), runs[1].col, runs[1].cols),
			("世", 1, 2)
		);
		assert!(runs[2].content.starts_with('b'));
		assert_eq!((runs[2].col, runs[2].cols), (3, cols - 3)); // b + trailing spaces
	}

	#[test]
	fn a_non_ascii_glyph_is_sealed_into_its_own_single_column_run() {
		// The placement rule that keeps a row aligned: a glyph the bundled font may not
		// have (here a box-drawing rule) never shares a run, so whatever a fallback font
		// does with its advance stays inside that one column.
		let runs = row_runs("a─b", 5);
		assert_eq!(runs.len(), 3);
		assert_eq!(
			(runs[1].content.as_str(), runs[1].col, runs[1].cols),
			("─", 1, 1)
		);
		assert_eq!(runs[2].col, 2);
	}

	#[test]
	fn braille_cells_never_merge_and_are_read_as_dot_patterns() {
		// Each braille cell is its own run — and each carries its own dot bitmap, which is
		// what the widget draws instead of asking a font for the glyph.
		let runs = row_runs("⠁⣿", 4);
		assert_eq!(runs.len(), 3); // two braille cells, then the trailing spaces
		assert_eq!(braille(&runs[0].content), Some(0b0000_0001));
		assert_eq!(braille(&runs[1].content), Some(0b1111_1111));
		assert_eq!(braille(&runs[2].content), None);
		// Plain text is not braille, however long the run.
		assert_eq!(braille("hello"), None);
		assert_eq!(braille("a"), None);
	}

	#[test]
	fn a_rounded_corner_meets_the_cell_edges_its_lines_leave_by() {
		// The join is the whole point: the arc must end on the cell's centre lines, where
		// the straight ─ and │ in the neighbouring cells run, and the tails must carry the
		// stroke from there to the edges the corner opens onto — no further, or the corner
		// draws into the cell next door.
		let cell = Rectangle {
			x: 0.0,
			y: 0.0,
			width: CELL_WIDTH,
			height: CELL_HEIGHT,
		};
		// Pixel geometry from a cell size that is itself a product of floats, so compare
		// within a fraction of a pixel rather than bit for bit.
		let close = |left: f32, right: f32| {
			assert!((left - right).abs() < 0.01, "expected {right}, got {left}");
		};

		let toward = rounded_corner("╭").unwrap();
		assert_eq!(toward, (1.0, 1.0)); // ╭ opens right and down
		let parts = corner_parts(cell, toward);

		// The arc is as round as the cell's short side allows, centred toward the corner
		// the lines leave by, and clipped to the quarter facing the cell's centre.
		let radius = CELL_WIDTH / 2.0;
		close(parts.ring.width, radius * 2.0);
		close(parts.ring.x, CELL_WIDTH / 2.0);
		close(parts.ring.y, CELL_HEIGHT / 2.0);
		close(parts.quadrant.width, radius);
		close(parts.quadrant.height, radius);
		close(parts.quadrant.x, CELL_WIDTH / 2.0);
		close(parts.quadrant.y, CELL_HEIGHT / 2.0);

		// The arc already spans the full half-width, so there is no horizontal tail; the
		// vertical one runs from where the arc ends down to the cell's bottom edge, on the
		// column's centre line.
		assert!(parts.across.is_none());
		let down = parts.down.unwrap();
		close(down.x, CELL_WIDTH / 2.0 - LINE_THICKNESS / 2.0);
		close(down.y, CELL_HEIGHT / 2.0 + radius);
		close(down.y + down.height, CELL_HEIGHT);

		// The other three are the same shape mirrored: each opens the way its glyph does.
		assert_eq!(rounded_corner("╮"), Some((-1.0, 1.0)));
		assert_eq!(rounded_corner("╯"), Some((-1.0, -1.0)));
		assert_eq!(rounded_corner("╰"), Some((1.0, -1.0)));
		// And a square corner is left to the font, which has it.
		assert_eq!(rounded_corner("┌"), None);
		assert_eq!(rounded_corner("╭╮"), None);

		// Mirrored: ╯ opens left and up, so its tail runs to the cell's top edge instead.
		let up = corner_parts(cell, rounded_corner("╯").unwrap())
			.down
			.unwrap();
		close(up.y, 0.0);
		close(up.height, CELL_HEIGHT / 2.0 - radius);
	}

	#[test]
	fn packed_runs_cover_every_grid_column_exactly_once() {
		// The run spans must sum to the grid width — each wide glyph claims two columns
		// and each continuation claims none — and each run must start where the previous
		// one ended, which is what the draw relies on to place glyphs.
		let cols = 12;
		let runs = row_runs("x世y世z", cols);
		let total: u16 = runs.iter().map(|run| run.cols).sum();
		assert_eq!(total, cols);
		let mut next = 0;
		for run in &runs {
			assert_eq!(run.col, next);
			next += run.cols;
		}
	}

	#[test]
	fn a_selection_breaks_into_its_own_highlighted_run() {
		// Selecting columns 1-2 of an all-default row splits it into three runs (before /
		// selected / after); only the middle carries the selection fill — proof the
		// highlight is both applied and isolated to the selection.
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

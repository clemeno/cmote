// ui/grid.rs — the terminal screen drawn as ONE widget (PLAN §9).
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

use std::ops::RangeInclusive;

use crate::app::Message;
use crate::palette;
use crate::term::mouse as report;
use crate::term::screen::{
	Cell as ScreenCell, Color as CellColor, CursorShape, MouseMode, Screen, UnderlineStyle,
};
use crate::term::search::Highlight;
use crate::ui::selection::{Cell, Selection};
use crate::ui::terminal::{CELL_HEIGHT, CELL_WIDTH, FONT_SIZE, GRID_PADDING, cell_at};
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

/// The bundled monospace font (Fira Mono, embedded in the binary — see `app::MONO_FONT_REGULAR`).
/// Naming it explicitly instead of `Font::MONOSPACE` means the grid looks identical on
/// every machine AND its cell advance is known exactly, which is what makes the pixel↔cell
/// math correct (§9). Upright cells (normal and bold) draw from this family.
const TERMINAL_FONT: Font = Font::with_name("Fira Mono");

/// The italic family (IBM Plex Mono — see `app::ITALIC_FONT`), used only for italic cells,
/// because Fira Mono ships no italic (§23). Its advance is the same 600/1000 em as Fira Mono,
/// so an italic run stays on the same pixel grid as the upright text around it.
const TERMINAL_FONT_ITALIC: Font = Font::with_name("IBM Plex Mono");

/// The default foreground/background when a cell asks for the "default" color — the
/// light-on-dark scheme (the single source of truth is `palette`, so the renderer and the
/// colour-query answerer never disagree, §23), and the window's backdrop behind the whole grid.
const DEFAULT_FG: Color = rgb(palette::DEFAULT_FG);
const DEFAULT_BG: Color = rgb(palette::DEFAULT_BG);

/// The background of a selected cell (§10). A muted blue that reads clearly under the
/// default light foreground; selected cells keep their own fg, only the fill changes, so
/// text stays legible while the region is obviously highlighted.
const SELECTION_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);

/// The background of a cell inside a find-bar match that is not the current one (§39). A muted
/// amber, a different HUE from the selection's blue rather than a paler shade of it, so "here is
/// the hit you are on" and "here is another hit" are told apart at a glance instead of by
/// brightness — brightness is exactly what a colour-blind eye or a dim screen loses. Like the
/// selection it changes only the fill, so the text underneath stays legible.
const MATCH_BG: Color = Color::from_rgb8(0x54, 0x46, 0x1c);

/// How thick an underlined cell's rule is, and how far above the cell's bottom edge it
/// sits. `fill_text` draws glyphs only, so the rule is a quad of our own.
const UNDERLINE_THICKNESS: f32 = 1.0;

/// The gap between the two rules of a double underline.
const UNDERLINE_GAP: f32 = 1.0;

/// A dotted underline's dot length and the gap after it, then a dashed one's dash and gap:
/// both are the same repeated-segment rule, only the segment and gap sizes differ.
const DOT_LEN: f32 = 1.0;
const DOT_GAP: f32 = 2.0;
const DASH_LEN: f32 = 3.0;
const DASH_GAP: f32 = 2.0;

/// A curly underline is approximated as a triangle wave: a segment this long, stepped this
/// far up on every other segment. A true sine is invisibly small at this cell size; a
/// two-level zigzag reads unmistakably as "curly" while staying quads we place exactly.
const CURL_STEP: f32 = 2.0;
const CURL_AMPLITUDE: f32 = 2.0;

/// How thick the crossed-out (strikeout) rule is; it sits on the cell's vertical middle.
const STRIKEOUT_THICKNESS: f32 = 1.0;

/// How far a faint (dim) cell's foreground is pulled from its background toward its full
/// colour — below 1.0 so the text reads as reduced intensity but stays legible.
const DIM_STRENGTH: f32 = 0.55;

/// The cursor's ink and the thickness of its non-block shapes (§23). A remote picks the shape
/// with DECSCUSR; the block shape inverts its cell (so a glyph under it stays legible) and is
/// drawn in the run planner, while these three are overlays drawn on top of an otherwise
/// normal cell: a `Bar` at the cell's left edge, an `Underline` along its bottom — both
/// thicker than the SGR underline rule (`UNDERLINE_THICKNESS`) so they read as a cursor and
/// not an attribute — and a `HollowBlock` one-pixel outline (what a terminal shows when its
/// window is unfocused). The ink is the default foreground, the conventional cursor colour on
/// this scheme, and every shape is drawn steady: cmote runs no animation timer, so no blink.
const CURSOR_COLOR: Color = DEFAULT_FG;
const CURSOR_BAR_THICKNESS: f32 = 2.0;
const CURSOR_UNDERLINE_THICKNESS: f32 = 2.0;
const CURSOR_OUTLINE_THICKNESS: f32 = 1.0;

/// How many lines one mouse-wheel notch scrolls the scrollback (§23). Three is the usual
/// terminal step — brisk enough to move but well short of a page. A pixel-precise (trackpad)
/// delta is converted by the cell height instead, so it scrolls a line per cell of travel.
const WHEEL_LINES: f32 = 3.0;

/// The scroll indicator (§23): a thin thumb hugging the grid's right edge, drawn only while the
/// viewport is scrolled back into history and gone the moment it returns to the live bottom —
/// "auto-hiding" without an animation timer, which cmote does not run. It is a read-only mark,
/// not a control: scrolling stays on the wheel and the Shift+PageUp/PageDown/Home/End keys. The
/// bar is `SCROLLBAR_WIDTH` wide, inset `SCROLLBAR_INSET` from the right edge so it sits in the grid's
/// own padding gutter and never over a cell; its thumb is never shorter than `SCROLLBAR_MIN_THUMB`
/// so a deep history still shows a visible mark, and it draws in a translucent light so the text
/// underneath stays readable.
const SCROLLBAR_WIDTH: f32 = 4.0;
const SCROLLBAR_INSET: f32 = 1.0;
const SCROLLBAR_MIN_THUMB: f32 = 16.0;
const SCROLLBAR_THUMB_COLOR: Color = Color::from_rgba(0.82, 0.82, 0.82, 0.55);

/// The prompt tick (§34): a small mark drawn in the LEFT padding gutter beside every shell
/// prompt on screen, from the OSC 133 marks (`Terminal::prompt_rows`). It mirrors the scroll
/// indicator on the right — a read-only mark living in the padding, never over a cell — so the
/// eye can find where each command's prompt began and a prompt jump has something visible to land
/// on. `PROMPT_TICK_WIDTH` wide, inset `PROMPT_TICK_INSET` from the left edge (the two together
/// stay inside `GRID_PADDING`, so no glyph is touched), `PROMPT_TICK_VPAD` short of the row's full
/// height so consecutive prompts read as separate ticks, and a soft cyan that stands apart from
/// the grey scrollbar without competing with the text.
const PROMPT_TICK_WIDTH: f32 = 3.0;
const PROMPT_TICK_INSET: f32 = 1.0;
const PROMPT_TICK_VPAD: f32 = 2.0;
const PROMPT_TICK_COLOR: Color = Color::from_rgba(0.42, 0.72, 0.85, 0.85);

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

/// The terminal grid widget. Borrows the emulator's screen for the frame rather than
/// copying it — a full screen of cells is exactly the thing not worth cloning 60 times a
/// second.
pub struct Grid<'a> {
	screen: Screen<'a>,
	selection: Option<&'a Selection>,
	/// Viewport rows that hold a shell prompt right now (§34), for the left-gutter ticks. Owned
	/// (a short list, at most one per visible row) rather than borrowed, since it is computed fresh
	/// each frame from the terminal's scroll position.
	prompts: Vec<u16>,
	/// The find bar's matches that fall on the visible screen right now (§39), each washed so every
	/// hit shows and not only the current one. Owned and computed fresh each frame for the same
	/// reason as `prompts`: it is a resolution of absolute document lines against wherever the
	/// viewport happens to be parked, which changes with every scroll.
	matches: Vec<Highlight>,
}

/// Draw the emulator's current screen, highlighting `selection` if there is one, washing the find
/// bar's on-screen `matches` (§39) and ticking the `prompts` rows in the left gutter (§34).
pub fn grid<'a>(
	screen: Screen<'a>,
	selection: Option<&'a Selection>,
	prompts: Vec<u16>,
	matches: Vec<Highlight>,
) -> Grid<'a> {
	Grid {
		screen,
		selection,
		prompts,
		matches,
	}
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

impl Grid<'_> {
	/// The reading-order run of the OSC 8 link under the pointer while Ctrl is held (§24), or
	/// `None` when Ctrl is up, the pointer is off the grid, or the cell there carries no link.
	/// This is only the gate — the modifier and the pointer; finding the run is `link_run_at`
	/// (pure, so it is unit-tested without a renderer). `bounds` is the grid's layout rectangle,
	/// so the pointer is made grid-local exactly as the mouse-report path does (`cell_at`).
	fn hovered_link_run(
		&self,
		modifiers: Modifiers,
		cursor: mouse::Cursor,
		bounds: Rectangle,
		rows: u16,
		cols: u16,
	) -> Option<RangeInclusive<usize>> {
		if !modifiers.control() {
			return None;
		}
		let position = cursor.position()?;
		if !bounds.contains(position) {
			return None;
		}
		let cell = cell_at(
			Point::new(position.x - bounds.x, position.y - bounds.y),
			rows,
			cols,
		);
		link_run_at(self.screen, cell, rows, cols)
	}
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
		tree: &Tree,
		renderer: &mut iced::Renderer,
		_theme: &Theme,
		_style: &iced::advanced::renderer::Style,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		viewport: &Rectangle,
	) {
		let state = tree.state.downcast_ref::<State>();
		let bounds = layout.bounds();
		let Some(visible) = bounds.intersection(viewport) else {
			return;
		};

		// The backdrop, once, behind everything: every cell that keeps the default
		// background then costs no quad of its own.
		renderer.fill_quad(fill(bounds), Background::Color(DEFAULT_BG));

		let (rows, cols) = self.screen.size();
		// The Ctrl-hover link affordance (§24): while Ctrl is held and the pointer is over an
		// OSC 8 link cell, the whole link's run of cells is underlined, so the link reveals itself
		// as one before the Ctrl+click that opens it. Computed once and consulted per cell in the
		// run planner below; the app already repaints on every hover move and modifier change, so
		// the underline follows the pointer and appears/vanishes with Ctrl without extra plumbing.
		let link_hover = self.hovered_link_run(state.modifiers, cursor, bounds, rows, cols);
		let (cursor_row, cursor_col) = self.screen.cursor_position();
		// The cursor is always on the live screen; when the viewport is scrolled back into
		// history its row on screen is that plus the display offset, and once that drops below
		// the viewport (scrolled far enough that the prompt is off the bottom) it is not drawn
		// at all (§23).
		let cursor_display_row = cursor_row.saturating_add(self.screen.display_offset());
		let cursor_on_screen = cursor_display_row < rows;
		// The cursor draws only when it is on screen, DECTCEM shows it, and the shape is not
		// `Hidden` (§23).
		let cursor_shape = (cursor_on_screen && !self.screen.hide_cursor())
			.then(|| self.screen.cursor_shape())
			.filter(|shape| *shape != CursorShape::Hidden);
		// A block cursor inverts its cell in the run planner so a glyph under it stays legible;
		// every other shape is an overlay drawn on top after the row, its cell left untouched.
		let block_cursor = cursor_shape == Some(CursorShape::Block);
		let origin = Point::new(bounds.x + GRID_PADDING, bounds.y + GRID_PADDING);
		// The find bar's on-screen matches (§39), flattened once for the whole frame into the per-cell
		// mask the run planner reads — see `match_mask` for why it is a mask and not a list. Bound
		// before `marks` so the mask outlives the borrow the planner takes of it.
		let mask = match_mask(&self.matches, rows, cols);
		let marks = Marks {
			selection: self.selection,
			matches: &mask,
		};

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
					block_cursor && row == cursor_display_row,
					cursor_col,
					marks,
					link_hover.as_ref(),
				) {
					draw_run(renderer, run, origin.x, top, row_bounds);
				}
			}

			// A shaped (non-block) cursor sits on top of its cell's glyph, once, after the grid.
			if let Some(shape) = cursor_shape.filter(|shape| *shape != CursorShape::Block) {
				draw_cursor(
					renderer,
					shape,
					Rectangle {
						x: origin.x + f32::from(cursor_col) * CELL_WIDTH,
						y: origin.y + f32::from(cursor_display_row) * CELL_HEIGHT,
						width: CELL_WIDTH,
						height: CELL_HEIGHT,
					},
				);
			}
		});

		// The scroll indicator on top of everything (§23): a thin thumb in the right padding
		// gutter while the viewport is scrolled back, and nothing at the live bottom. Read-only —
		// it reports where the view sits and how deep the history is, but the wheel and keys do the
		// moving. Drawn outside the clip above: it lives in the padding, not among the cells.
		if let Some(thumb) = scrollbar_thumb(
			bounds,
			rows,
			self.screen.history_size(),
			self.screen.display_offset(),
		) {
			renderer.fill_quad(
				Quad {
					bounds: thumb,
					border: Border::default().rounded(SCROLLBAR_WIDTH / 2.0),
					shadow: iced::Shadow::default(),
					snap: false,
				},
				Background::Color(SCROLLBAR_THUMB_COLOR),
			);
		}

		// The prompt ticks (§34): one mark in the left padding gutter for every shell prompt on
		// screen. Like the scroll indicator, they are drawn last and in the padding, so they sit
		// over no cell and never disturb the text. A row outside the visible grid is skipped — the
		// terminal only hands over on-screen rows, but the guard keeps the geometry honest.
		for &row in &self.prompts {
			if row < rows {
				renderer.fill_quad(
					Quad {
						bounds: prompt_tick_rect(bounds, row),
						border: Border::default().rounded(PROMPT_TICK_WIDTH / 2.0),
						shadow: iced::Shadow::default(),
						snap: false,
					},
					Background::Color(PROMPT_TICK_COLOR),
				);
			}
		}
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

		let iced::Event::Mouse(pointer) = event else {
			return;
		};
		let mode = self.screen.mouse_mode();
		let shift = state.modifiers.shift();

		// The wheel is the one pointer event that means something even with no mouse-aware
		// program: unless such a program has asked for it — and Shift is not taking the mouse
		// back, the xterm convention — a scroll moves cmote's own scrollback (§23). Handled
		// before the report path below so it works at a bare shell prompt, where no mouse mode
		// is on, and gated to the grid's own bounds so scrolling over a side panel is not us.
		if let mouse::Event::WheelScrolled { delta } = pointer {
			let to_program = mode != MouseMode::None && !shift;
			if !to_program {
				if cursor
					.position()
					.is_some_and(|position| layout.bounds().contains(position))
					&& let Some(lines) = wheel_lines(*delta)
				{
					shell.publish(Message::TerminalScroll(lines));
					shell.capture_event();
				}
				return;
			}
		}

		// No mouse protocol on, or Shift held: the pointer is the user's, for selecting
		// text and opening our own menu. Nothing is captured, so the layers above act as
		// they always have. A button already down is the exception — its press went to the
		// program, so its release and its drag must too, or the program is left believing
		// the button never came up.
		if mode == MouseMode::None || (shift && state.held.is_none()) {
			return;
		}
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
			self.screen.mouse_encoding(),
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
	draw_underline(
		renderer,
		run.style.underline,
		bounds,
		run.style.underline_color,
	);
	if run.style.strikeout {
		renderer.fill_quad(
			fill(Rectangle {
				y: top + (CELL_HEIGHT - STRIKEOUT_THICKNESS) / 2.0,
				height: STRIKEOUT_THICKNESS,
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
	// The text layout box is ONE cell wider than the run's own pixel width. A run's advance
	// is exactly `run.cols * CELL_WIDTH`, but cosmic-text sums each glyph's advance in f32 and
	// the total lands a sliver past that; a glyph whose right edge exceeds the layout width is
	// culled, so the run's LAST glyph would vanish whenever the run ends on a visible cell — a
	// colour change, the cell before the cursor, the last char of an `ls --color` name. The
	// slack lifts that boundary clear of the rounding (and gives an oversized fallback glyph in
	// a sealed one-column run room to shape). It paints nothing extra: the content is fixed, so
	// the empty tail draws nothing, and everything is still clipped to `row_bounds` and pinned
	// at `left` by the left alignment, so no glyph moves.
	let text_bounds_width = width + CELL_WIDTH;
	renderer.fill_text(
		text::Text {
			content: run.content,
			bounds: Size::new(text_bounds_width, CELL_HEIGHT),
			size: Pixels(FONT_SIZE),
			line_height: text::LineHeight::Absolute(Pixels(CELL_HEIGHT)),
			// The face for this run. Upright cells draw from Fira Mono, italic cells from IBM
			// Plex Mono (Fira Mono has none, §23); the weight and style pick the exact face.
			// Each MUST match a bundled face, because cosmic-text — with the whole system font
			// DB present at runtime — does NOT nearest-match within a named family: an unbundled
			// weight/style silently falls back to a *proportional* system font, breaking the
			// grid. We bundle Fira Mono at 400/700 and IBM Plex Mono italic at 400/700, which is
			// every combination this asks for.
			font: Font {
				weight: if run.style.bold {
					iced::font::Weight::Bold
				} else {
					iced::font::Weight::Normal
				},
				style: if run.style.italic {
					iced::font::Style::Italic
				} else {
					iced::font::Style::Normal
				},
				..if run.style.italic {
					TERMINAL_FONT_ITALIC
				} else {
					TERMINAL_FONT
				}
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

/// Draw the cursor as a shape overlaid on its cell (§23). Only the shapes that sit *on top*
/// of an otherwise normal cell are here — `Bar` a rule down the left edge, `Underline` one
/// along the bottom, `HollowBlock` an outline around the whole cell. The block cursor is not
/// (it inverts its cell in the run planner instead, keeping its glyph legible), and `Hidden`
/// never arrives (the caller filters both). All draw in the cursor colour, steady, no blink.
fn draw_cursor(renderer: &mut iced::Renderer, shape: CursorShape, bounds: Rectangle) {
	match shape {
		CursorShape::Bar => renderer.fill_quad(
			fill(Rectangle {
				width: CURSOR_BAR_THICKNESS,
				..bounds
			}),
			Background::Color(CURSOR_COLOR),
		),
		CursorShape::Underline => renderer.fill_quad(
			fill(Rectangle {
				y: bounds.y + CELL_HEIGHT - CURSOR_UNDERLINE_THICKNESS,
				height: CURSOR_UNDERLINE_THICKNESS,
				..bounds
			}),
			Background::Color(CURSOR_COLOR),
		),
		CursorShape::HollowBlock => renderer.fill_quad(
			Quad {
				bounds,
				border: Border {
					color: CURSOR_COLOR,
					width: CURSOR_OUTLINE_THICKNESS,
					radius: 0.0.into(),
				},
				shadow: iced::Shadow::default(),
				snap: true,
			},
			Background::Color(Color::TRANSPARENT),
		),
		CursorShape::Block | CursorShape::Hidden => {}
	}
}

/// Draw a run's underline in the requested style (§9, §23). `fill_text` draws glyphs only,
/// so every rule here is quads of our own — which is what lets the engine's five distinct
/// underlines reach the screen at all. All sit on the same baseline, one line above the
/// cell's bottom edge; `color` is the underline's own colour (SGR 58) or the foreground.
fn draw_underline(
	renderer: &mut iced::Renderer,
	style: UnderlineStyle,
	bounds: Rectangle,
	color: Color,
) {
	let base = bounds.y + CELL_HEIGHT - UNDERLINE_THICKNESS;
	match style {
		UnderlineStyle::None => {}
		UnderlineStyle::Single => rule(renderer, bounds.x, base, bounds.width, color),
		UnderlineStyle::Double => {
			rule(renderer, bounds.x, base, bounds.width, color);
			rule(
				renderer,
				bounds.x,
				base - UNDERLINE_GAP - UNDERLINE_THICKNESS,
				bounds.width,
				color,
			);
		}
		UnderlineStyle::Dotted => dashes(
			renderer,
			bounds.x,
			base,
			bounds.width,
			DOT_LEN,
			DOT_GAP,
			color,
		),
		UnderlineStyle::Dashed => dashes(
			renderer,
			bounds.x,
			base,
			bounds.width,
			DASH_LEN,
			DASH_GAP,
			color,
		),
		UnderlineStyle::Curly => curl(renderer, bounds.x, base, bounds.width, color),
	}
}

/// One solid horizontal rule, `UNDERLINE_THICKNESS` tall, from `x` across `width` at `y`.
fn rule(renderer: &mut iced::Renderer, x: f32, y: f32, width: f32, color: Color) {
	renderer.fill_quad(
		fill(Rectangle {
			x,
			y,
			width,
			height: UNDERLINE_THICKNESS,
		}),
		Background::Color(color),
	);
}

/// A run of `segment`-long marks separated by `gap`, across `width`. Dotted and dashed
/// underlines are the same shape at different sizes. The last mark is clipped to the run so
/// it never spills past the cells it belongs to.
fn dashes(
	renderer: &mut iced::Renderer,
	x: f32,
	y: f32,
	width: f32,
	segment: f32,
	gap: f32,
	color: Color,
) {
	let mut offset = 0.0;
	while offset < width {
		rule(renderer, x + offset, y, segment.min(width - offset), color);
		offset += segment + gap;
	}
}

/// A curly underline as a triangle wave: `CURL_STEP`-long marks whose baseline alternates
/// between the underline row and `CURL_AMPLITUDE` above it. Two levels, not a real sine —
/// enough to read as wavy at a 14px cell (see `CURL_STEP`).
fn curl(renderer: &mut iced::Renderer, x: f32, y: f32, width: f32, color: Color) {
	let mut offset = 0.0;
	let mut raised = false;
	while offset < width {
		let lift = if raised { CURL_AMPLITUDE } else { 0.0 };
		rule(
			renderer,
			x + offset,
			y - lift,
			CURL_STEP.min(width - offset),
			color,
		);
		offset += CURL_STEP;
		raised = !raised;
	}
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

/// How far a scroll should move cmote's scrollback, in lines — positive up into history,
/// matching the engine's delta sign (§23). A line-based wheel notch is scaled by `WHEEL_LINES`;
/// a pixel-based (trackpad) delta is divided by the cell height so it tracks the gesture. A
/// purely horizontal scroll moves nothing and yields `None`, so no message is published.
fn wheel_lines(delta: mouse::ScrollDelta) -> Option<i32> {
	let lines = match delta {
		mouse::ScrollDelta::Lines { y, .. } => (y * WHEEL_LINES).round() as i32,
		mouse::ScrollDelta::Pixels { y, .. } => (y / CELL_HEIGHT).round() as i32,
	};
	(lines != 0).then_some(lines)
}

/// The scroll indicator's thumb rectangle for a grid of `bounds`, or `None` when nothing should
/// be drawn — at the live bottom (`offset == 0`) or with no history to indicate (§23). Split from
/// the draw so the geometry, the part that can be wrong, is testable without a renderer (as with
/// `corner_parts`). The thumb tracks the text area, not the padding: its height is the viewport's
/// share of the whole document (`rows` of screen plus `history` of scrollback), floored at
/// `SCROLLBAR_MIN_THUMB` so a deep history still shows a visible mark, and its top runs from the
/// track's top at the oldest retained line (`offset == history`) down toward the bottom as the
/// view returns to the live tail — clamped so that minimum height can never push it off the end.
fn scrollbar_thumb(bounds: Rectangle, rows: u16, history: u16, offset: u16) -> Option<Rectangle> {
	if offset == 0 || history == 0 {
		return None;
	}
	let rows = f32::from(rows);
	let history = f32::from(history);
	let offset = f32::from(offset);
	let document = history + rows;

	let track_top = bounds.y + GRID_PADDING;
	let track_height = rows * CELL_HEIGHT;
	let thumb_height = (track_height * rows / document)
		.max(SCROLLBAR_MIN_THUMB)
		.min(track_height);
	// 0 at the oldest line, growing toward 1 as the view returns to the bottom.
	let position = (history - offset) / document;
	let max_top = track_top + track_height - thumb_height;
	let thumb_top = (track_top + position * track_height).min(max_top);

	Some(Rectangle {
		x: bounds.x + bounds.width - SCROLLBAR_WIDTH - SCROLLBAR_INSET,
		y: thumb_top,
		width: SCROLLBAR_WIDTH,
		height: thumb_height,
	})
}

/// The prompt tick's rectangle for a `row` of the grid `bounds` (§34): a short bar in the left
/// padding gutter, centred on the row it marks. Split from the draw for the same reason as the
/// scroll thumb — the geometry is the part that can be wrong, so it is testable without a
/// renderer. The x stays inside `GRID_PADDING` (inset + width), so the mark never overlaps the
/// first cell, which begins one `GRID_PADDING` in.
fn prompt_tick_rect(bounds: Rectangle, row: u16) -> Rectangle {
	let top = bounds.y + GRID_PADDING + f32::from(row) * CELL_HEIGHT;
	Rectangle {
		x: bounds.x + PROMPT_TICK_INSET,
		y: top + PROMPT_TICK_VPAD,
		width: PROMPT_TICK_WIDTH,
		height: CELL_HEIGHT - 2.0 * PROMPT_TICK_VPAD,
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
	italic: bool,
	strikeout: bool,
	underline: UnderlineStyle,
	/// The underline's colour, already resolved (SGR 58, or the foreground when the cell set
	/// none). Part of the key so a cell that recolours only its underline still breaks its run.
	underline_color: Color,
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

/// The contiguous reading-order run of cells sharing the OSC 8 hyperlink URI at `cell`, in
/// row-major indices (`row * cols + col`), or `None` when that cell carries no link (§24). The
/// engine lays a link's cells out contiguously — they share one `Arc<Hyperlink>` — so the run is
/// a single `[start, end]` span, which is exactly what the Ctrl-hover underline covers, across a
/// wrap and all. Pure (no renderer, no widget), so the walk is unit-tested on its own.
fn link_run_at(
	screen: Screen<'_>,
	cell: Cell,
	rows: u16,
	cols: u16,
) -> Option<RangeInclusive<usize>> {
	let uri = screen.cell(cell.row, cell.col)?.hyperlink()?.to_owned();
	let width = usize::from(cols);
	let total = usize::from(rows) * width;
	let here = usize::from(cell.row) * width + usize::from(cell.col);
	// Whether the cell at a row-major index carries the very same link URI. A fresh `Cell` is
	// built per probe, but a link is only as long as its text, so the walk is short.
	let same = |index: usize| {
		let probe_row = (index / width) as u16;
		let probe_col = (index % width) as u16;
		screen
			.cell(probe_row, probe_col)
			.and_then(|cell| cell.hyperlink().map(|link| link == uri))
			.unwrap_or(false)
	};
	let mut start = here;
	while start > 0 && same(start - 1) {
		start -= 1;
	}
	let mut end = here;
	while end + 1 < total && same(end + 1) {
		end += 1;
	}
	Some(start..=end)
}

/// What is marked on the grid over and above the cells' own styling: the mouse text selection
/// (§10) and the find bar's on-screen matches (§39). Grouped into one argument because they are
/// consulted together for every cell and resolved the same way — a fill that replaces the cell's
/// own background — and because `plan_runs` had already reached the argument count where one more
/// loose `Option` is a mistake waiting to be made at a call site.
#[derive(Default, Clone, Copy)]
struct Marks<'a> {
	selection: Option<&'a Selection>,
	/// The row-major "this cell is inside a match" mask over the visible grid (see `match_mask`).
	/// Empty when the find bar is shut, or open with no hits on screen.
	matches: &'a [bool],
}

/// Flatten the visible matches into a per-cell lookup for one frame (§39): `mask[row * cols + col]`
/// is true when that cell falls inside a hit. Row-major, the same index space the Ctrl-hover link
/// run already uses, so the run planner tests both the same cheap way.
///
/// A mask rather than a walk of the match list per cell, because the list is not small in the case
/// that matters: find-as-you-type means the FIRST letter typed is searched, and one letter over a
/// screenful of text matches hundreds of times, so `cells × matches` per frame would stall the very
/// keystroke it is meant to serve. This is `cells + matches` instead. An empty list allocates
/// nothing at all and every lookup then misses, which is the shut-bar case — the common one.
fn match_mask(matches: &[Highlight], rows: u16, cols: u16) -> Vec<bool> {
	if matches.is_empty() {
		return Vec::new();
	}
	let mut mask = vec![false; usize::from(rows) * usize::from(cols)];
	for found in matches {
		// A row or column past the grid's edge is dropped rather than wrapped onto the next row: the
		// match list is resolved against the same screen, but a resize between the scan and this
		// frame could leave a hit pointing outside it.
		if found.row >= rows {
			continue;
		}
		let base = usize::from(found.row) * usize::from(cols);
		let last = found.end_col.min(cols.saturating_sub(1));
		// An empty range when the span starts off the right edge, which is what an inclusive range
		// with a start past its end iterates to — nothing.
		for col in found.start_col..=last {
			mask[base + usize::from(col)] = true;
		}
	}
	mask
}

/// Pack one screen row into runs (§9). Walks the row left to right, growing a run while
/// cells are narrow, ASCII, and share a style; anything else is sealed into a run of its
/// own so its width cannot leak into its neighbours — a wide cell claims two columns, a
/// non-ASCII one claims its own single column whatever the fallback font does with it.
/// Wide *continuation* cells are skipped: the lead already reserves their column. `link_run`
/// is the Ctrl-hover link's reading-order span (§24): a cell inside it is underlined as the
/// affordance, which — being part of the style key — also seals it into its own run.
fn plan_runs(
	screen: Screen<'_>,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
	marks: Marks<'_>,
	link_run: Option<&RangeInclusive<usize>>,
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
		if cell
			.as_ref()
			.is_some_and(|cell| cell.is_wide_continuation())
		{
			continue;
		}

		let is_wide = cell.as_ref().is_some_and(|cell| cell.is_wide());
		let glyph = match &cell {
			Some(cell) if cell.has_contents() => cell.contents().to_string(),
			_ => " ".to_string(),
		};
		let seals = is_wide || !glyph.is_ascii();
		let is_cursor = on_cursor_row && col == cursor_col;
		let is_selected = marks
			.selection
			.is_some_and(|selection| selection.contains(row, col));
		// This cell's row-major index, matched against the find bar's match mask (§39) and the
		// Ctrl-hover link's span (§24) — both live in this one index space.
		let index = usize::from(row) * usize::from(cols) + usize::from(col);
		let is_match = marks.matches.get(index).copied().unwrap_or(false);
		let is_link_hover = link_run.is_some_and(|run| run.contains(&index));
		let style = cell_style(
			cell.as_ref(),
			is_cursor,
			is_selected,
			is_match,
			is_link_hover,
		);

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

/// Resolve a cell's colors and attributes into a `CellStyle` (§9, §23). The order matters:
/// faint fades the ink toward its own background first; then inverse video and the cursor
/// each swap fg/bg (together they cancel, matching how a real terminal draws the cursor over
/// already-inverted text); then a search match and, over it, a selection take the fill, keeping
/// the foreground so text stays legible; and conceal last, painting the glyph and its rules in
/// the final background so it holds its cell but shows nothing. Because `CellStyle` is the
/// run-grouping key, either fill (and any per-cell attribute) breaks its run off from its
/// neighbours (§10, §39).
fn cell_style(
	cell: Option<&ScreenCell>,
	is_cursor: bool,
	is_selected: bool,
	is_match: bool,
	is_link_hover: bool,
) -> CellStyle {
	let Some(cell) = cell else {
		return CellStyle {
			fg: DEFAULT_FG,
			bg: DEFAULT_BG,
			bold: false,
			italic: false,
			strikeout: false,
			underline: UnderlineStyle::None,
			underline_color: DEFAULT_FG,
		};
	};

	let mut fg = resolve(cell.fgcolor(), DEFAULT_FG);
	let mut bg = resolve(cell.bgcolor(), DEFAULT_BG);
	// Faint is a property of the ink, so fade it toward the background before any swap.
	if cell.dim() {
		fg = blend(bg, fg, DIM_STRENGTH);
	}
	// The underline's explicit colour (SGR 58), resolved now so it tracks the ink; the
	// fallback to the foreground is applied after the swap, below, so it follows inverse too.
	let explicit_underline = cell.underline_color().map(|color| resolve(color, fg));

	if cell.inverse() ^ is_cursor {
		std::mem::swap(&mut fg, &mut bg);
	}
	// The find bar's matches (§39): a wash under every hit on screen. Applied BEFORE the selection,
	// so the current hit — which revealing already turned into an ordinary selection (§35) — keeps
	// the selection's fill and stays the one the eye lands on. That ordering is the whole reason the
	// match list can include the current match and stay ignorant of which one it is.
	if is_match {
		bg = MATCH_BG;
	}
	if is_selected {
		bg = SELECTION_BG;
	}
	if cell.hidden() {
		fg = bg;
	}
	// A concealed cell shows nothing, so its underline vanishes into the background too;
	// otherwise the rule takes its explicit colour, or the (post-swap) foreground.
	let mut underline = cell.underline();
	let mut underline_color = if cell.hidden() {
		bg
	} else {
		explicit_underline.unwrap_or(fg)
	};
	// The Ctrl-hover link affordance (§24): a link cell that has no underline of its own gains a
	// single one in the foreground while it is the hover target, so the link shows as one. A cell
	// already underlined keeps its own rule — it is visibly a link already — so the hover never
	// downgrades a program's fancier underline (a spell-check curly, say) to a plain line.
	if is_link_hover && underline == UnderlineStyle::None {
		underline = UnderlineStyle::Single;
		underline_color = fg;
	}

	CellStyle {
		fg,
		bg,
		bold: cell.bold(),
		italic: cell.italic(),
		strikeout: cell.strikeout(),
		underline,
		underline_color,
	}
}

/// Blend `from` toward `to` by `t` (0 keeps `from`, 1 reaches `to`), one channel at a time.
/// Used to fade a faint cell's foreground toward its background.
fn blend(from: Color, to: Color, t: f32) -> Color {
	Color::from_rgba(
		from.r + (to.r - from.r) * t,
		from.g + (to.g - from.g) * t,
		from.b + (to.b - from.b) * t,
		from.a + (to.a - from.a) * t,
	)
}

/// Map a cell color to an iced color. `Default` becomes the caller's default (different
/// for fg and bg); indexed colors go through the shared xterm-256 palette.
fn resolve(color: CellColor, default: Color) -> Color {
	match color {
		CellColor::Default => default,
		CellColor::Indexed(index) => rgb(palette::xterm_256(index)),
		CellColor::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
	}
}

/// A `palette` RGB triple as an iced color — the one place the shared palette's plain
/// `(u8, u8, u8)` is lifted into the renderer's colour type.
const fn rgb((r, g, b): (u8, u8, u8)) -> Color {
	Color::from_rgb8(r, g, b)
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
		plan_runs(terminal.screen(), 0, cols, false, 0, Marks::default(), None)
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
		let marks = Marks {
			selection: Some(&selection),
			matches: &[],
		};
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, marks, None);

		// "a" | "bc" (selected) | "de"
		assert_eq!(runs.len(), 3);
		assert_eq!(runs[1].content, "bc");
		assert_eq!(runs[1].style.bg, SELECTION_BG);
		assert_ne!(runs[0].style.bg, SELECTION_BG);
		assert_ne!(runs[2].style.bg, SELECTION_BG);
	}

	#[test]
	fn every_on_screen_match_is_washed_and_the_current_one_keeps_the_selection_fill() {
		// Arrange: "ab ab" with both hits on row 0 (columns 0-1 and 3-4), and the SECOND one also
		// selected — which is exactly the state the find bar leaves behind, since revealing the
		// current match turns it into an ordinary selection (§35).
		let mut terminal = Terminal::new(1, 5);
		terminal.process(b"ab ab");
		let hits = [
			Highlight {
				row: 0,
				start_col: 0,
				end_col: 1,
			},
			Highlight {
				row: 0,
				start_col: 3,
				end_col: 4,
			},
		];
		let selection = Selection::new(Cell { row: 0, col: 3 }).with_head(Cell { row: 0, col: 4 });
		let mask = match_mask(&hits, 1, 5);
		let marks = Marks {
			selection: Some(&selection),
			matches: &mask,
		};

		// Act
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, marks, None);

		// Assert: "ab" (match wash) | " " (plain) | "ab" (selected, not washed) — the two fills are
		// different colours, so the current hit is told apart from the other one, and each fill
		// breaks its own run off from the space between them.
		assert_eq!(runs.len(), 3);
		assert_eq!(runs[0].content, "ab");
		assert_eq!(runs[0].style.bg, MATCH_BG);
		assert_eq!(runs[1].content, " ");
		assert_eq!(runs[1].style.bg, DEFAULT_BG);
		assert_eq!(runs[2].content, "ab");
		assert_eq!(runs[2].style.bg, SELECTION_BG);
	}

	#[test]
	fn the_match_mask_drops_what_falls_outside_the_grid() {
		// A hit on a row the grid no longer has (a resize shrank the screen between the scan and
		// this frame) is dropped, and one whose span runs past the last column is clipped — never
		// wrapped onto the row below, which is what an unclamped row-major write would do.
		let hits = [
			Highlight {
				row: 5,
				start_col: 0,
				end_col: 1,
			},
			Highlight {
				row: 0,
				start_col: 2,
				end_col: 9,
			},
		];
		let mask = match_mask(&hits, 2, 4);
		assert_eq!(
			mask,
			vec![false, false, true, true, false, false, false, false]
		);

		// And no hits means no allocation and no washes at all — the shut-bar case.
		assert!(match_mask(&[], 2, 4).is_empty());
	}

	#[test]
	fn a_faint_cell_fades_its_foreground_toward_the_background() {
		// SGR 2 dims: the resolved foreground moves from the default toward the dark
		// background, so each channel lands below the plain default — fainter, still on screen.
		let plain = row_runs("x", 1)[0].style.fg;
		let faint = row_runs("\x1b[2mx", 1)[0].style.fg;
		assert!(faint.r < plain.r && faint.g < plain.g && faint.b < plain.b);
	}

	#[test]
	fn a_concealed_cell_paints_its_glyph_in_its_background() {
		// SGR 8 conceal: foreground equals background, so the glyph draws invisibly while the
		// cell keeps its place (and its text is still there to be copied).
		let style = row_runs("\x1b[8mx", 1)[0].style;
		assert_eq!(style.fg, style.bg);
	}

	#[test]
	fn a_strikeout_cell_is_marked_and_breaks_its_run() {
		// SGR 9 crosses out: the flag reaches the run's style, and — being part of the
		// grouping key — a struck cell never merges with a plain neighbour.
		let runs = row_runs("a\x1b[9mb", 5);
		let struck: Vec<_> = runs.iter().filter(|run| run.style.strikeout).collect();
		assert_eq!(struck.len(), 1);
		assert!(struck[0].content.starts_with('b'));
		assert!(runs.len() >= 2);
	}

	#[test]
	fn an_underline_style_reaches_the_run_style() {
		// The engine's distinct underline flags survive resolution: a curly underline arrives
		// at the run as Curly, so the draw can pick the matching rule.
		assert_eq!(
			row_runs("\x1b[4:3mx", 1)[0].style.underline,
			UnderlineStyle::Curly
		);
	}

	#[test]
	fn a_wheel_notch_becomes_a_line_delta_and_a_horizontal_scroll_is_ignored() {
		// One notch up scrolls WHEEL_LINES into history (positive, the engine's sign), one notch
		// down scrolls back out (negative), and a purely horizontal scroll moves nothing (§23).
		assert_eq!(
			wheel_lines(mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 }),
			Some(3)
		);
		assert_eq!(
			wheel_lines(mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 }),
			Some(-3)
		);
		assert_eq!(
			wheel_lines(mouse::ScrollDelta::Lines { x: 2.0, y: 0.0 }),
			None
		);
		// A pixel-precise (trackpad) delta scrolls a line per cell of vertical travel.
		assert_eq!(
			wheel_lines(mouse::ScrollDelta::Pixels {
				x: 0.0,
				y: CELL_HEIGHT
			}),
			Some(1)
		);
	}

	#[test]
	fn the_scroll_indicator_hides_at_the_live_bottom_and_with_no_history() {
		// Nothing to indicate at the live tail (offset 0) or before any line has scrolled off
		// (history 0), so no thumb is drawn — "auto-hiding" without a timer (§23).
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		assert!(scrollbar_thumb(bounds, 24, 0, 0).is_none());
		assert!(scrollbar_thumb(bounds, 24, 100, 0).is_none());
		// Scrolled up at all, with history to show: a thumb appears.
		assert!(scrollbar_thumb(bounds, 24, 100, 1).is_some());
	}

	#[test]
	fn the_scroll_thumb_tracks_position_and_depth_within_the_track() {
		// The thumb sits in the right gutter, reports where the view is (top of the track at the
		// oldest line, sliding down toward the live tail) and how deep the history is (a shorter
		// thumb the more there is), and never leaves the text area's vertical span (§23).
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		let track_top = GRID_PADDING;
		let track_bottom = GRID_PADDING + 24.0 * CELL_HEIGHT;
		let close = |left: f32, right: f32| {
			assert!((left - right).abs() < 0.01, "expected {right}, got {left}");
		};

		// At the oldest retained line (offset == history) the thumb is pinned to the track top.
		let top = scrollbar_thumb(bounds, 24, 100, 100).unwrap();
		close(top.y, track_top);
		close(top.x, bounds.width - SCROLLBAR_WIDTH - SCROLLBAR_INSET);

		// Returning toward the live tail slides the thumb down the track, monotonically.
		let middle = scrollbar_thumb(bounds, 24, 100, 50).unwrap();
		let near_bottom = scrollbar_thumb(bounds, 24, 100, 1).unwrap();
		assert!(top.y < middle.y && middle.y < near_bottom.y);

		// However deep the history, the thumb stays inside the track and never shorter than the
		// floor that keeps it visible.
		let deep = scrollbar_thumb(bounds, 24, 5000, 2500).unwrap();
		close(deep.height, SCROLLBAR_MIN_THUMB);
		assert!(deep.y >= track_top - 0.01);
		assert!(deep.y + deep.height <= track_bottom + 0.01);
	}

	#[test]
	fn a_prompt_tick_sits_in_the_left_gutter_on_its_row() {
		// The tick lives in the left padding — its right edge (inset + width) stays inside
		// GRID_PADDING, so it never reaches the first cell — and it is centred vertically on the
		// row it marks (§34).
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		let tick = prompt_tick_rect(bounds, 3);
		assert!(
			tick.x + tick.width <= GRID_PADDING,
			"tick spills onto a cell"
		);
		// Row 3's top is GRID_PADDING + 3 cells down; the bar is inset by PROMPT_TICK_VPAD.
		let expected_top = GRID_PADDING + 3.0 * CELL_HEIGHT + PROMPT_TICK_VPAD;
		assert!((tick.y - expected_top).abs() < 0.01);
		assert!((tick.height - (CELL_HEIGHT - 2.0 * PROMPT_TICK_VPAD)).abs() < 0.01);
	}

	#[test]
	fn an_italic_cell_is_marked_and_breaks_its_run() {
		// SGR 3 italic: the flag reaches the run's style (so the draw can pick the italic
		// face), and — being part of the grouping key — an italic cell never merges with an
		// upright neighbour, since the two draw from different font families.
		let runs = row_runs("a\x1b[3mb", 5);
		let italic: Vec<_> = runs.iter().filter(|run| run.style.italic).collect();
		assert_eq!(italic.len(), 1);
		assert!(italic[0].content.starts_with('b'));
	}

	#[test]
	fn link_run_at_spans_the_whole_link_and_nothing_else() {
		// An OSC 8 link over "site": every one of its four cells shares the URI, so the run is the
		// contiguous span 0..=3, and it comes back whole from any cell of the link. The plain 'X'
		// after the close carries no link, so it has no run at all (§24).
		let cols = 20;
		let mut terminal = Terminal::new(1, cols);
		terminal.process(b"\x1b]8;;https://example.com\x07site\x1b]8;;\x07X");
		let screen = terminal.screen();
		assert_eq!(
			link_run_at(screen, Cell { row: 0, col: 2 }, 1, cols),
			Some(0..=3)
		);
		assert_eq!(
			link_run_at(screen, Cell { row: 0, col: 0 }, 1, cols),
			Some(0..=3)
		);
		assert_eq!(link_run_at(screen, Cell { row: 0, col: 4 }, 1, cols), None);
	}

	#[test]
	fn a_ctrl_hovered_link_cell_gains_a_single_underline() {
		// With no hover the link cells carry no underline; when the link's span is the hover run,
		// exactly those cells gain a single underline — the affordance the pointer draws on Ctrl.
		let cols = 20;
		let mut terminal = Terminal::new(1, cols);
		terminal.process(b"\x1b]8;;https://example.com\x07site\x1b]8;;\x07X");

		let plain = plan_runs(terminal.screen(), 0, cols, false, 0, Marks::default(), None);
		assert!(
			plain
				.iter()
				.all(|run| run.style.underline == UnderlineStyle::None)
		);

		let run = 0..=3usize;
		let hovered = plan_runs(
			terminal.screen(),
			0,
			cols,
			false,
			0,
			Marks::default(),
			Some(&run),
		);
		let underlined: String = hovered
			.iter()
			.filter(|run| run.style.underline == UnderlineStyle::Single)
			.map(|run| run.content.as_str())
			.collect();
		assert_eq!(underlined, "site");
	}
}

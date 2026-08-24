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

use crate::app::Message;
use crate::palette;
use crate::term::graphics::Placement;
use crate::term::mouse as report;
use crate::term::scp;
use crate::term::screen::{
	Cell, Color as CellColor, CursorShape, Link, MouseMode, Screen, UnderlineStyle,
};
use crate::term::search::SearchHighlight;
use crate::ui::scrollbar;
use crate::ui::selection::{ScreenSpot, Selection};
use crate::ui::terminal::{CELL_HEIGHT, CELL_WIDTH, FONT_SIZE, GRID_PADDING, cell_under};
use iced::advanced::Renderer as _;
use iced::advanced::image::{Image as RasterImage, Renderer as _};
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

// The scrollbar (§23, §116): a thin thumb hugging the grid's right edge, drawn whenever there is
// history to move through, and grabbable — press and drag it, or press the bare track to jump there.
// `scrollbar::WIDTH` wide, inset `scrollbar::INSET` from the right edge so it sits in the grid's own
// padding gutter and never over a cell; its thumb is never shorter than `scrollbar::MIN_THUMB` so a
// deep history still shows a findable mark, and it draws in a translucent light so the text
// underneath stays readable.
//
// Those numbers used to be four `const`s here. They are `ui::scrollbar`'s now (§118), because every
// pane's `scrollable` styles itself from the same ones and one window should have one scrollbar.
// What is still this file's is the GEOMETRY — `scrollbar_thumb` and its inverse, which speak in rows,
// history and viewport offsets, words only the terminal has. `MIN_THUMB` is likewise the terminal's
// alone in practice: iced hard-codes its own minimum scroller length, so a pane cannot be told it.

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

/// The bookmark tick (§55): the same mark in the same gutter, for a line a SCRIPT chose with
/// `OSC 1337 ; SetMark` rather than a line the shell had a prompt on. Amber against the prompts'
/// cyan, because the two answer different questions — "where did I run something" and "where did the
/// build say to look" — and a session using both wants to tell them apart at a glance. Same geometry,
/// so a bookmark on a prompt's own line simply draws over it.
const USER_TICK_COLOR: Color = Color::from_rgba(0.84, 0.66, 0.23, 0.85);

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
	/// Viewport rows holding an explicit bookmark right now (§55), for the same gutter in a different
	/// colour. Owned and recomputed each frame for the same reason as `prompts`, and normally empty —
	/// only a session whose scripts drop marks has any.
	user_marks: Vec<u16>,
	/// The find bar's matches that fall on the visible screen right now (§39), each washed so every
	/// hit shows and not only the current one. Owned and computed fresh each frame for the same
	/// reason as `prompts`: it is a resolution of absolute document lines against wherever the
	/// viewport happens to be parked, which changes with every scroll.
	matches: Vec<SearchHighlight>,
	/// The inline images the session is holding (§41), each anchored to an absolute document line.
	/// Borrowed, not owned: the pixels belong to the emulator for the whole session, and a frame only
	/// needs to know where they go.
	images: &'a [Placement],
	/// The pointer shape the remote asked for over its own grid (OSC 22, §77), already mapped to
	/// iced's vocabulary by `ui::terminal::grid_interaction`, or `None` when it has asked for nothing.
	///
	/// It is HERE, and not on the `mouse_area` outside this widget, because the gutter is not the
	/// remote's grid (§125). `mouse_area` applies its own interaction over its whole bounds — the
	/// padding included — and only when the content answered `Interaction::None`, which on Windows is
	/// exactly what this widget answers over its own scrollbar (`grab_interaction` returns `None`
	/// there so the `WM_SETCURSOR` seam can paint the hand). So a remote's shape was reaching the one
	/// strip of pixels that belongs to cmote, and which of the two got drawn depended on which
	/// mechanism painted last. Deciding it in one place is the fix; the answer is that the bar wins.
	pointer: Option<mouse::Interaction>,
}

/// Draw the emulator's current screen, highlighting `selection` if there is one, washing the find
/// bar's on-screen `matches` (§39), ticking the `prompts` and `user_marks` rows in the left gutter
/// (§34, §55) and compositing the inline `images` over the cells they reserved (§41).
pub fn grid<'a>(
	screen: Screen<'a>,
	selection: Option<&'a Selection>,
	prompts: Vec<u16>,
	user_marks: Vec<u16>,
	matches: Vec<SearchHighlight>,
	images: &'a [Placement],
	pointer: Option<mouse::Interaction>,
) -> Grid<'a> {
	Grid {
		screen,
		selection,
		prompts,
		user_marks,
		matches,
		images,
		pointer,
	}
}

/// What the widget remembers between events: the modifiers (they arrive on their own
/// event, not on the mouse ones), which button the remote program believes is down, the
/// last cell a move was reported for, so dragging inside one cell stays quiet, and the
/// scrollbar grip while the bar is being dragged.
#[derive(Debug, Default)]
struct GridState {
	modifiers: Modifiers,
	held: Option<report::Button>,
	last: Option<ScreenSpot>,
	/// Where inside the thumb the pointer caught it, in pixels from the thumb's top — `Some` for
	/// exactly as long as the button is down on the bar (§116).
	///
	/// The GRIP is what is stored, rather than the offset the drag started at, and that is what makes
	/// the bar feel attached: the thumb stays under the same part of itself for the whole drag. Storing
	/// the pointer's own y and treating it as the thumb's top would snap the thumb's top to the pointer
	/// the instant it moved, jumping the view by up to a thumb's height before the drag even began.
	scroll_grip: Option<f32>,
	/// Whether this grid last believed the pointer was on its scrollbar (§119), so the enter and the
	/// exit are raised on the CHANGE and not on every mouse move.
	///
	/// Per widget instance, which is what makes one `cursor::SCROLLBAR` name safe for every region's
	/// bar at once: a split window's other grid also sees this move, but with `on_bar` already false
	/// it says nothing, so it cannot take back a hand its neighbour has just been given.
	on_bar: bool,
}

impl Grid<'_> {
	/// The OSC 8 link under the pointer while Ctrl is held (§24, §92), or `None` when Ctrl is up,
	/// the pointer is off the grid, or the cell there carries no link. Every cell carrying this
	/// same link is then underlined as the affordance, wherever on the page it sits.
	/// This is only the gate — the modifier and the pointer; reading the link is `link_at`
	/// (pure, so it is unit-tested without a renderer). `bounds` is the grid's layout rectangle,
	/// so the pointer is made grid-local exactly as the mouse-report path does (`cell_at`).
	fn hovered_link(
		&self,
		modifiers: Modifiers,
		cursor: mouse::Cursor,
		bounds: Rectangle,
	) -> Option<Link> {
		if !modifiers.control() {
			return None;
		}
		let position = cursor.position()?;
		if !bounds.contains(position) {
			return None;
		}
		let cell = cell_under(
			&self.screen,
			Point::new(position.x - bounds.x, position.y - bounds.y),
		);
		link_at(self.screen, cell)
	}

	/// Which claim on the pointer wins, given the three facts a frame has (§125).
	///
	/// Split out of `mouse_interaction` so the ORDER — the part that can be wrong — is testable
	/// without a widget tree, the same split `scrollbar_thumb` and `corner_parts` already use.
	///
	/// `over` is whether the pointer is inside this widget at all. It has to be asked: this method is
	/// called whether or not it is, and it is the check the `mouse_area` outside used to make for the
	/// remote's shape (§77). Without it a program's chosen pointer would follow the mouse across the
	/// tab strip and the dialogs.
	fn interaction_over(&self, dragging: bool, on_bar: bool, over: bool) -> mouse::Interaction {
		if dragging || on_bar {
			// The bar is cmote's own furniture, so it beats whatever the remote asked for. WHO draws
			// the hand is `grab_interaction`'s business (§51): `None` on Windows, precisely so iced is
			// asked for nothing and `cursor`'s `WM_SETCURSOR` seam paints the bitmap.
			return crate::cursor::grab_interaction(dragging).unwrap_or(mouse::Interaction::None);
		}
		if over {
			return self.pointer.unwrap_or(mouse::Interaction::None);
		}
		mouse::Interaction::None
	}

	/// Whether the bar is under `cursor` right now — the grab zone, not the painted thumb, so the
	/// answer matches what a press would actually do (§116). `false` with no history, since then
	/// there is no bar at all.
	fn on_scrollbar(&self, bounds: Rectangle, cursor: mouse::Cursor) -> bool {
		let (rows, _) = self.screen.size();
		self.screen.history_size() > 0
			&& cursor
				.position()
				.is_some_and(|position| scrollbar_track(bounds, rows).contains(position))
	}

	/// Raise the hand's enter and exit for the bar (§119), on the change only.
	///
	/// The other two grabbable surfaces get these from their own `mouse_area`'s `on_enter` / `on_exit`
	/// (§51, §52). The bar has no widget of its own — it is quads inside this one (§116) — so there is
	/// nothing to hang those on, and the grid works the answer out from the pointer instead. Which is
	/// the better source anyway: one place computes one boolean, so the enter and the exit cannot
	/// arrive in the wrong order the way two widgets' events can.
	///
	/// Not captured. A move that changes the hand is still a move the layers above want — the pane's
	/// own hover tracking, and the selection drag — and this only reports.
	fn track_hand(
		&self,
		state: &mut GridState,
		bounds: Rectangle,
		cursor: mouse::Cursor,
		shell: &mut Shell<'_, Message>,
	) {
		let on_bar = self.on_scrollbar(bounds, cursor);
		if on_bar == state.on_bar {
			return;
		}
		state.on_bar = on_bar;
		shell.publish(if on_bar {
			Message::GrabEntered(crate::cursor::SCROLLBAR)
		} else {
			Message::GrabExited(crate::cursor::SCROLLBAR)
		});
	}

	/// Drive the scrollbar with the pointer (§116), returning whether the event was the bar's — in
	/// which case the caller is done with it.
	///
	/// This runs BEFORE both of the other things a press can mean, and that ordering is the design.
	/// Above the widget a `mouse_area` starts a text selection; inside it, a mouse-aware program gets
	/// a report. A press on the bar is neither: the bar is chrome in the padding gutter, not a cell,
	/// and it is cmote's own view control rather than anything the remote should hear about. So a press
	/// it claims is captured and neither path sees it.
	///
	/// A full-screen program does not have to be special-cased here. The alternate screen retains no
	/// history, so `history_size()` is 0, so there is no thumb, so nothing is claimed and a click in
	/// `vim`'s right-hand column reaches `vim` exactly as it did before.
	fn scroll_drag(
		&self,
		state: &mut GridState,
		pointer: &mouse::Event,
		bounds: Rectangle,
		cursor: mouse::Cursor,
		shell: &mut Shell<'_, Message>,
	) -> bool {
		let (rows, _) = self.screen.size();
		let history = self.screen.history_size();
		match pointer {
			mouse::Event::ButtonPressed(mouse::Button::Left) => {
				let Some(position) = cursor.position() else {
					return false;
				};
				let Some(thumb) =
					scrollbar_thumb(bounds, rows, history, self.screen.display_offset())
				else {
					return false;
				};
				if !scrollbar_track(bounds, rows).contains(position) {
					return false;
				}
				// On the thumb: keep hold of the point that was caught. Off it, on the bare track: the
				// view JUMPS there, and the grip becomes the thumb's middle so the thumb centres under
				// the pointer and the drag carries on from it — which is what makes a click on the
				// track and a drag from it the same gesture rather than two.
				let grip = if position.y >= thumb.y && position.y < thumb.y + thumb.height {
					position.y - thumb.y
				} else {
					thumb.height / 2.0
				};
				state.scroll_grip = Some(grip);
				// The hand closes for the whole drag (§119, §51), before the offset goes out — so a
				// frame that paints the move already has the closed hand on it.
				shell.publish(Message::ScrollbarGrabbed);
				shell.publish(Message::TerminalScrollTo(scrollbar_offset(
					bounds,
					rows,
					history,
					position.y - grip,
				)));
				shell.capture_event();
				true
			}
			mouse::Event::CursorMoved { .. } => {
				let Some(grip) = state.scroll_grip else {
					return false;
				};
				let Some(position) = cursor.position() else {
					return false;
				};
				// No bounds test on the move, on purpose: a drag that wanders off the bar sideways, or
				// past either end, keeps scrolling and pins at the end it ran out at — the offset is
				// clamped, not dropped. Letting go of the drag because the pointer strayed a few pixels
				// is the thing that makes a scrollbar feel broken.
				shell.publish(Message::TerminalScrollTo(scrollbar_offset(
					bounds,
					rows,
					history,
					position.y - grip,
				)));
				shell.capture_event();
				true
			}
			mouse::Event::ButtonReleased(mouse::Button::Left) => {
				// Wherever the release lands — the release belongs to the press that started on the
				// bar, the same rule the mouse-report path applies to a button it saw go down.
				if state.scroll_grip.take().is_none() {
					return false;
				}
				// Let go: the hand opens again if the pointer is still on the bar, and the frame's
				// own `drawn` call decides that — not this line (§119).
				shell.publish(Message::ScrollbarReleased);
				shell.capture_event();
				true
			}
			_ => false,
		}
	}
}

impl Widget<Message, Theme, iced::Renderer> for Grid<'_> {
	fn tag(&self) -> tree::Tag {
		tree::Tag::of::<GridState>()
	}

	fn state(&self) -> tree::State {
		tree::State::new(GridState::default())
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

	#[expect(
		clippy::too_many_lines,
		reason = "a widget's paint: backdrop, then rows, then the cursor and images, in order"
	)]
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
		let state = tree.state.downcast_ref::<GridState>();
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
		let link_hover = self.hovered_link(state.modifiers, cursor, bounds);
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
		// Where the viewport is parked, as a document line (§40) — the coordinate space the selection
		// and the inline images are both stored in, so a row being drawn can be resolved into the line
		// it shows and a picture onto the row its own line is at.
		let top_line = self.screen.line_at(0);
		let marks = Marks {
			selection: self.selection,
			top_line,
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

			// The inline images (§41), composited over the blank cells they reserved. After the text,
			// so a picture is never hidden by the row it sits on, and inside the same clip, so one
			// scrolled half off the top is cut at the grid's edge instead of drawn over the chrome.
			//
			// Whichever page is up, these are ITS pictures: the emulator keeps a store per screen and
			// hands over only the one being drawn (`Terminal::images`), so a full-screen program's
			// preview shows while it is up and the scrollback's plots come back untouched when it
			// quits. No check is needed here, and none is possible either — the placements are the same
			// shape on both pages, because the alternate screen keeps no history and so the absolute
			// line of row `r` there is simply `r` (§40).
			for placement in self.images {
				let (pixels, reserved) = image_bounds(placement, origin, top_line);
				// Clipped to the cells the picture reserved, so its own pixels can never bleed past
				// the box the engine is holding for it — an image drawn a shade larger than its
				// rounded-up box would otherwise creep onto the row below. A box entirely off the
				// visible grid is skipped before any texture work is asked of the renderer.
				let Some(clip) = reserved.intersection(&visible) else {
					continue;
				};
				renderer.draw_image(
					// Snapped to the pixel grid: a picture drawn at its native size on a
					// half-pixel boundary would be resampled into a blur for no reason.
					RasterImage::new(placement.handle.clone()).snap(true),
					pixels,
					clip,
				);
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
		// gutter while the viewport is scrolled back, and nothing at the live bottom. Draggable
		// since §116, so it brightens under the pointer and brighter still while it is held
		// (§125) — the same three opacities a pane's bar wears, from the same function. Drawn
		// outside the clip above: it lives in the padding, not among the cells.
		if let Some(thumb) = scrollbar_thumb(
			bounds,
			rows,
			self.screen.history_size(),
			self.screen.display_offset(),
		) {
			renderer.fill_quad(
				Quad {
					bounds: thumb,
					border: Border::default().rounded(scrollbar::radius()),
					shadow: iced::Shadow::default(),
					snap: false,
				},
				Background::Color(scrollbar::thumb(scrollbar_touch(
					state.scroll_grip.is_some(),
					self.on_scrollbar(bounds, cursor),
				))),
			);
		}

		// The prompt ticks (§34): one mark in the left padding gutter for every shell prompt on
		// screen. Like the scroll indicator, they are drawn last and in the padding, so they sit
		// over no cell and never disturb the text. A row outside the visible grid is skipped — the
		// terminal only hands over on-screen rows, but the guard keeps the geometry honest.
		// The bookmark ticks are drawn AFTER the prompt ticks (§55) so that a bookmark landing on a
		// prompt's own line — a shell hook that emits both — shows as the bookmark. That is the right
		// way round: the prompt is derivable from the shell's own marks, whereas a bookmark is
		// something a script went out of its way to say.
		for (marks, color) in [
			(&self.prompts, PROMPT_TICK_COLOR),
			(&self.user_marks, USER_TICK_COLOR),
		] {
			for &row in marks {
				if row < rows {
					renderer.fill_quad(
						Quad {
							bounds: prompt_tick_rect(bounds, row),
							border: Border::default().rounded(PROMPT_TICK_WIDTH / 2.0),
							shadow: iced::Shadow::default(),
							snap: false,
						},
						Background::Color(color),
					);
				}
			}
		}
	}

	/// The cursor over the grid: the two hands over the scrollbar, the remote's shape over the cells,
	/// and nothing anywhere else (§119, §125).
	///
	/// **The bar wins.** Both claims are answered here since §125, in this order, because the gutter
	/// the bar sits in is cmote's own furniture and not the page a remote is drawing — see the
	/// `pointer` field for what the old arrangement did instead.
	///
	/// `Interaction::None` when a remote has asked for nothing, because that is what the grid has
	/// always answered: the text cursor over cells is the `mouse_area`'s in `ui::terminal`, and
	/// handing the question back is how it keeps deciding.
	///
	/// WHO draws the hand depends on the platform (§51), and that whole question is
	/// `grab_interaction`'s: on Windows there are no hand cursors, so it answers `None` precisely so
	/// iced is asked for nothing and `cursor`'s own `WM_SETCURSOR` seam paints them from the claim the
	/// enter/exit above maintain. Everywhere else the toolkit has both hands and is simply asked.
	///
	/// Dragging is read from this widget's own state and not from `cursor`'s global, so the shape is
	/// right for the bar being dragged rather than for whatever was last picked up anywhere.
	fn mouse_interaction(
		&self,
		tree: &Tree,
		layout: Layout<'_>,
		cursor: mouse::Cursor,
		_viewport: &Rectangle,
		_renderer: &iced::Renderer,
	) -> mouse::Interaction {
		let state = tree.state.downcast_ref::<GridState>();
		self.interaction_over(
			state.scroll_grip.is_some(),
			self.on_scrollbar(layout.bounds(), cursor),
			cursor.is_over(layout.bounds()),
		)
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
		let state = tree.state.downcast_mut::<GridState>();
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
		// is on, and gated to the grid's own bounds so scrolling over a side pane is not us.
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

		// The hand over the bar (§119), before anything that might return: it is a report about where
		// the pointer IS, so it has to be right even on the events below that get consumed.
		self.track_hand(state, layout.bounds(), cursor, shell);

		// The scrollbar, which is grabbable (§116). Tried before everything below it: a press in the
		// right padding gutter is the bar's, whether or not a program has asked for the mouse, and it
		// belongs to neither the selection above nor the report below. Nothing is claimed unless there
		// is history to move through, so this is inert on the alternate screen and inert in a session
		// with no scrollback yet.
		if self.scroll_drag(state, pointer, layout.bounds(), cursor, shell) {
			return;
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
		let cell = cell_under(
			&self.screen,
			Point::new(position.x - bounds.x, position.y - bounds.y),
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
				report::MouseEvent::Press(button)
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
				report::MouseEvent::Release(button)
			}
			mouse::Event::WheelScrolled { delta } => {
				let Some(button) = wheel_button(*delta) else {
					return;
				};
				if !inside {
					return;
				}
				report::MouseEvent::Press(button)
			}
			mouse::Event::CursorMoved { .. } => {
				if !inside || state.last == Some(cell) {
					return;
				}
				state.last = Some(cell);
				report::MouseEvent::Motion(state.held)
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
		if !matches!(pointer_event, report::MouseEvent::Motion(_)) {
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
	// The braille block is 256 code points wide, so a pattern inside it fits a byte — and the
	// `try_from` is the check rather than a separate comparison that has to agree with it.
	u8::try_from(code).ok()
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
		mouse::ScrollDelta::Lines { y, .. } => super::lines_scrolled(y * WHEEL_LINES),
		mouse::ScrollDelta::Pixels { y, .. } => super::lines_scrolled(y / CELL_HEIGHT),
	};
	(lines != 0).then_some(lines)
}

/// How the terminal's bar is being touched, in the vocabulary both surfaces share (§125).
///
/// Free rather than a method because it holds no grid state: it is the mapping from this widget's
/// two facts onto `scrollbar::Touch`, and out here the precedence is testable without a renderer or
/// a widget tree — the same reason the geometry below is.
///
/// **A drag beats a hover, and that is not the same as "both".** §116's drag deliberately survives
/// the pointer straying off the bar, so while one is in flight the hover flag is not a fact about
/// the bar any more; the drag is. A pane's bar reaches the same answer from iced's own flags
/// (`scrollbar::touch_of`), which is why the rule is written down in both places.
fn scrollbar_touch(dragging: bool, hovering: bool) -> scrollbar::Touch {
	if dragging {
		scrollbar::Touch::Dragged
	} else if hovering {
		scrollbar::Touch::Hovered
	} else {
		scrollbar::Touch::Idle
	}
}

/// The scroll indicator's thumb rectangle for a grid of `bounds`, or `None` when there is no history
/// to indicate at all (§23). Split from the draw so the geometry, the part that can be wrong, is
/// testable without a renderer (as with `corner_parts`). The thumb tracks the text area, not the
/// padding: its height is the viewport's share of the whole document (`rows` of screen plus `history`
/// of scrollback), floored at `scrollbar::MIN_THUMB` so a deep history still shows a visible mark, and
/// its top runs from the track's top at the oldest retained line (`offset == history`) down toward the
/// bottom as the view returns to the live tail — clamped so that minimum height can never push it off
/// the end.
///
/// **It is drawn at the live bottom too, which it was not until §116.** §23 showed nothing there on
/// purpose: as a pure indicator it had nothing to say when the view was already at the tail. Once the
/// bar became something to GRAB, that silence was the whole problem — a bar you can only grab after
/// you have already scrolled by some other means is not a way to start scrolling. So the only `None`
/// left is a session with no scrollback, where there is genuinely nothing to move to.
fn scrollbar_thumb(bounds: Rectangle, rows: u16, history: u16, offset: u16) -> Option<Rectangle> {
	if history == 0 {
		return None;
	}
	let track = scrollbar_track(bounds, rows);
	let thumb_height = scrollbar_thumb_height(rows, history, track.height);
	// The thumb slides over the track LESS its own height, and the position scales onto that span
	// rather than onto the whole track. Until §116 it scaled onto the track and was then clamped at
	// the bottom, which drew the same picture within a pixel or two for a shallow history and made a
	// DRAG come apart for a deep one: past the clamp a range of offsets all mapped to the same
	// bottom-most thumb, so the bar stopped following the pointer while the view kept moving. Scaling
	// onto the span is what makes `scrollbar_offset` below an exact inverse instead of an approximate
	// one, and the two are only correct together.
	let span = track.height - thumb_height;
	// 0 at the oldest retained line, 1 at the live tail.
	let position = f32::from(history - offset.min(history)) / f32::from(history);
	let thumb_top = track.y + position * span.max(0.0);

	Some(Rectangle {
		x: bounds.x + bounds.width - scrollbar::WIDTH - scrollbar::INSET,
		y: thumb_top,
		width: scrollbar::WIDTH,
		height: thumb_height,
	})
}

/// How tall the whole document is in lines — the visible screen plus everything retained above it.
/// One function because the thumb's height, its position and the inverse below must all divide by the
/// same number or the bar and the pointer disagree.
fn document_lines(rows: u16, history: u16) -> f32 {
	f32::from(rows) + f32::from(history)
}

/// The thumb's height: the viewport's share of the document, floored so a deep history still leaves
/// something visible and capped so it can never exceed the track it slides in.
fn scrollbar_thumb_height(rows: u16, history: u16, track_height: f32) -> f32 {
	(track_height * f32::from(rows) / document_lines(rows, history))
		.max(scrollbar::MIN_THUMB)
		.min(track_height)
}

/// The rectangle a press is tested against to start a drag (§116), and the track the thumb slides in.
///
/// **Wider than the thumb is painted, deliberately.** The thumb is `scrollbar::WIDTH` (4px) so it reads
/// as an indicator rather than furniture, and a 4-pixel grab target is a target you miss. The zone is
/// the whole right padding gutter instead, which is the same rule the left gutter's prompt ticks use —
/// a 3px tick inside a 6px gutter, and the press tests the gutter (§34). The gutter is `GRID_PADDING`
/// and the paint is `scrollbar::WIDTH + scrollbar::INSET` from the edge, so the zone contains the thumb
/// with room either side and still touches no cell.
fn scrollbar_track(bounds: Rectangle, rows: u16) -> Rectangle {
	Rectangle {
		x: bounds.x + bounds.width - GRID_PADDING,
		y: bounds.y + GRID_PADDING,
		width: GRID_PADDING,
		height: f32::from(rows) * CELL_HEIGHT,
	}
}

/// The inverse of `scrollbar_thumb` (§116): the viewport offset that would put the thumb's TOP at
/// `thumb_top`, clamped into `0..=history`.
///
/// A drag knows where the thumb should be and needs the offset that means, which is the forward
/// mapping read backwards — `position = (history - offset) / document` solved for `offset`. Pure and
/// paired with the forward function on purpose: the two are only correct together, so the tests assert
/// the round trip rather than either one's arithmetic in isolation.
///
/// Note it takes the thumb's top and not the pointer: a grab holds the thumb wherever it was caught,
/// so the caller subtracts that grip before asking. Handing the pointer straight in would teleport the
/// thumb's top to the pointer on the first pixel of every drag.
fn scrollbar_offset(bounds: Rectangle, rows: u16, history: u16, thumb_top: f32) -> u16 {
	if history == 0 {
		return 0;
	}
	let track = scrollbar_track(bounds, rows);
	let thumb_height = scrollbar_thumb_height(rows, history, track.height);
	let span = track.height - thumb_height;
	// A thumb that fills its track has nowhere to slide, so every press means the live bottom rather
	// than a division by zero. Reachable: the height is floored at `scrollbar::MIN_THUMB`, so a grid
	// only a few rows tall has a thumb as tall as its track.
	if span <= 0.0 {
		return 0;
	}
	// The forward mapping's `position`, read backwards: 0 at the track top (oldest), 1 at the span's
	// end (live tail).
	let position = ((thumb_top - track.y) / span).clamp(0.0, 1.0);
	// Through `lines_scrolled` rather than a cast of our own: it is already the one place a pixel
	// measurement becomes a line count for §23's scrolling, and it ROUNDS. A truncating cast would
	// read the top half of every line as the line below it, so the bar would feel one row behind the
	// pointer all the way down.
	let lines = crate::ui::lines_scrolled(f32::from(history) * (1.0 - position));
	u16::try_from(lines.clamp(0, i32::from(history)))
		.expect("clamped into 0..=history, a u16, on the line above")
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

/// Where an inline image lands on this frame (§41): the rectangle its PIXELS are drawn in, and the
/// rectangle of CELLS it reserved. `origin` is the grid's first cell and `top_line` the document line
/// the top visible row is showing, so this is the reverse of the projection the run planner does —
/// a document line back onto a row (§40).
///
/// The row offset is signed: a picture anchored ABOVE the viewport gets a negative one and is drawn
/// with its top off screen, which is what lets a tall image scroll smoothly through the view rather
/// than pop into existence once its first line comes on screen. Split out from the draw for the same
/// reason as the scroll thumb and the prompt tick — the geometry is the part that can be wrong, so it
/// is worth testing without a renderer.
fn image_bounds(placement: &Placement, origin: Point, top_line: u64) -> (Rectangle, Rectangle) {
	// Signed, in i64: both lines are absolute document indices, so their difference is the row the
	// picture's top edge sits at — which is negative for one scrolled past the top of the viewport.
	let row = placement.line.cast_signed() - top_line.cast_signed();
	let x = origin.x + f32::from(placement.col) * CELL_WIDTH;
	let y = origin.y + super::signed_pixels(row, CELL_HEIGHT);
	let pixels = Rectangle {
		x,
		y,
		width: f32::from(placement.width),
		height: f32::from(placement.height),
	};
	let reserved = Rectangle {
		x,
		y,
		width: f32::from(placement.cols) * CELL_WIDTH,
		height: f32::from(placement.rows) * CELL_HEIGHT,
	};
	(pixels, reserved)
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

/// The OSC 8 hyperlink on `cell`, or `None` when that cell carries no link (§24, §92).
///
/// This used to walk outwards from the cell and return the contiguous span of cells sharing the
/// link's URI, which was wrong in both directions and §92 replaced it with the link itself. The
/// underline is decided per cell instead, by comparing this against what each cell carries.
///
/// **Contiguity was never the rule.** The specification is that "character cells that have the same
/// target URI and the same nonempty id are always underlined together on mouseover", and its own
/// worked example is a URL a program split across two runs with other text between them — exactly
/// what a contiguous walk stops at. And the **URI** was never the identity either: one address
/// written twice is two links, and the old walk joined them whenever they happened to sit next to
/// each other.
///
/// Pure (no renderer, no widget), so it stays unit-testable on its own.
fn link_at(screen: Screen<'_>, cell: ScreenSpot) -> Option<Link> {
	screen.cell(cell.row, cell.col)?.link().cloned()
}

/// What is marked on the grid over and above the cells' own styling: the mouse text selection
/// (§10) and the find bar's on-screen matches (§39). Grouped into one argument because they are
/// consulted together for every cell and resolved the same way — a fill that replaces the cell's
/// own background — and because `plan_runs` had already reached the argument count where one more
/// loose `Option` is a mistake waiting to be made at a call site.
#[derive(Default, Clone, Copy)]
struct Marks<'a> {
	selection: Option<&'a Selection>,
	/// The absolute document line the TOP visible row is showing (§40). The selection is stored in
	/// document coordinates, so this is what turns the row being drawn into the line to ask about —
	/// carried here rather than read off the screen per cell, since it is one number for the frame.
	top_line: u64,
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
fn match_mask(matches: &[SearchHighlight], rows: u16, cols: u16) -> Vec<bool> {
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

/// Whether a cell's contents carry a control character, in which case the grid draws a blank in its
/// place (§117).
///
/// `any` rather than a test of the whole string: a cell holds a grapheme cluster, so in principle a
/// base character could arrive with a control among its marks, and one control anywhere in the string
/// is enough to wreck the advance of everything after it.
///
/// The one that really happens is `\t`, which the engine stores on purpose. Deciding this by category
/// rather than by naming that one character is the point — every C0 and C1 code is something the
/// terminal is supposed to have ACTED on rather than drawn, so a cell holding any of them is a cell
/// whose glyph is not a glyph.
fn holds_control(contents: &str) -> bool {
	contents.chars().any(char::is_control)
}

/// Pack one screen row into runs (§9). Walks the row left to right, growing a run while
/// cells are narrow, ASCII, and share a style; anything else is sealed into a run of its
/// own so its width cannot leak into its neighbours — a wide cell claims two columns, a
/// non-ASCII one claims its own single column whatever the fallback font does with it.
/// Wide *continuation* cells are skipped: the lead already reserves their column. `hovered_link`
/// is the OSC 8 link under the Ctrl-hover pointer (§24, §92): every cell carrying that same link is
/// underlined as the affordance, wherever it sits — which, being part of the style key, also seals
/// each into a run of its own.
fn plan_runs(
	screen: Screen<'_>,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
	marks: Marks<'_>,
	hovered_link: Option<&Link>,
) -> Vec<Run> {
	let mut runs: Vec<Run> = Vec::new();
	let mut content = String::new();
	// The open run: its style, the column it starts at, its span so far, and whether it is
	// sealed (nothing may join it). `None` means no run is open yet.
	let mut current: Option<(CellStyle, u16, u16, bool)> = None;
	// The document line this viewport row is showing (§40). The selection lives in document
	// coordinates so that scrolling moves it with its text; resolving that here — once per row, not
	// once per cell — is the whole cost of the projection on the drawing side.
	let line = marks.top_line + u64::from(row);

	for col in 0..cols {
		let cell = screen.cell(row, col);

		// The trailing half of a wide glyph: its column was already claimed by the lead
		// cell's two-column run, so emit nothing for it.
		if cell
			.as_ref()
			.is_some_and(super::super::term::screen::Cell::is_wide_continuation)
		{
			continue;
		}

		let is_wide = cell
			.as_ref()
			.is_some_and(super::super::term::screen::Cell::is_wide);
		let glyph = match &cell {
			// A cell can legitimately HOLD a control character, and one does: the engine writes the
			// TAB into the first cell it skipped over (`put_tab`) so that copying the region gets a
			// real tab back — which `Selection::extract` reads, and which is what keeps a paste of
			// `du` output aligned. It must never reach the shaper, though. A tab is one CELL to the
			// grid and a jump to the next tab STOP to cosmic-text, so a `\t` inside a run's content
			// silently displaces every glyph after it in that run — and since the selection decides
			// where runs begin and end, selecting a substring moved characters that should have
			// stayed put (§117). Drawn as the blank the cell occupies; the character is still in the
			// cell for anything that reads the grid rather than paints it.
			Some(cell) if cell.has_contents() && !holds_control(cell.contents()) => {
				cell.contents().to_string()
			}
			_ => " ".to_string(),
		};
		let seals = is_wide || !glyph.is_ascii();
		let is_cursor = on_cursor_row && col == cursor_col;
		let is_selected = marks
			.selection
			.is_some_and(|selection| selection.contains(line, col));
		// This cell's row-major index, matched against the find bar's match mask (§39).
		let index = usize::from(row) * usize::from(cols) + usize::from(col);
		let is_match = marks.matches.get(index).copied().unwrap_or(false);
		// The Ctrl-hover underline is per cell rather than per span since §92: a cell is part of
		// the hovered link when it carries the very same link — same URI and same identifier — so a
		// link the program split into separate runs lights up whole, and one address written twice
		// stays two links.
		let is_link_hover =
			hovered_link.is_some() && cell.as_ref().and_then(Cell::link) == hovered_link;
		let style = cell_style(
			cell.as_ref(),
			CellMarks {
				cursor: is_cursor,
				selected: is_selected,
				matched: is_match,
				link_hover: is_link_hover,
			},
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
	// The one place the DATA order becomes a PRESENTATION order (§76). Everything above walks the
	// grid the host wrote; a line a program put on a right-to-left character path is drawn from the
	// other edge, and this is where that happens — after the runs are built, so the styles, the
	// selection fill, the match wash and the link underline all came out of data coordinates and
	// travel with their cells.
	if screen.line_is_rtl(line) {
		mirror(&mut runs, cols);
	}
	runs
}

/// Turn a row's runs from data order into the presentation order of a right-to-left line (§76).
///
/// Two things move. A run's START becomes the mirror of its LAST column, because the run still
/// spans rightwards from wherever it is drawn — `flip(col + cols - 1)`, not `flip(col)`. And the
/// characters inside it reverse, since each of its cells has moved to the other side of the run.
///
/// Reversing by `char` is exact here, and only here, because of how the runs above are built: a
/// cell that is not plain ASCII SEALS its run, so a run is either one cell wide or made of
/// one-byte-one-cell ASCII. A grapheme cluster — a letter with a combining mark — is never split
/// by this, because it can only ever be a run of its own. The `is_ascii` test is that invariant
/// stated where it is relied on rather than left to the reader.
fn mirror(runs: &mut [Run], cols: u16) {
	for run in runs.iter_mut() {
		run.col = scp::flip(run.col + run.cols.saturating_sub(1), cols);
		if run.content.is_ascii() {
			run.content = run.content.chars().rev().collect();
		}
	}
}

/// What the RENDERER knows about one cell that the cell itself does not (§9, §111): the cursor is on
/// it, it is selected, it is inside a search match, a Ctrl-hovered link covers it.
///
/// A struct rather than four `bool` parameters, which is what these were. All four can hold at once —
/// the cursor can sit on a selected cell inside a match under a hovered link — so they are not a state
/// to be folded into an enum; the problem with four adjacent bools is the CALL, where
/// `cell_style(cell, false, true, false, true)` is one transposition away from painting the wrong
/// thing and the compiler has nothing to say. Named fields fix exactly that.
#[expect(
	clippy::struct_excessive_bools,
	reason = "four independent marks on one cell; naming them is the point, see the doc above"
)]
#[derive(Debug, Clone, Copy)]
struct CellMarks {
	cursor: bool,
	selected: bool,
	matched: bool,
	link_hover: bool,
}

/// Resolve a cell's colors and attributes into a `CellStyle` (§9, §23). The order matters:
/// faint fades the ink toward its own background first; then inverse video and the cursor
/// each swap fg/bg (together they cancel, matching how a real terminal draws the cursor over
/// already-inverted text); then a search match and, over it, a selection take the fill, keeping
/// the foreground so text stays legible; and conceal last, painting the glyph and its rules in
/// the final background so it holds its cell but shows nothing. Because `CellStyle` is the
/// run-grouping key, either fill (and any per-cell attribute) breaks its run off from its
/// neighbours (§10, §39).
fn cell_style(cell: Option<&Cell>, marks: CellMarks) -> CellStyle {
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

	let mut fg = to_iced_color(cell.fgcolor(), DEFAULT_FG);
	let mut bg = to_iced_color(cell.bgcolor(), DEFAULT_BG);
	// Faint is a property of the ink, so fade it toward the background before any swap.
	if cell.dim() {
		fg = lerp(bg, fg, DIM_STRENGTH);
	}
	// The underline's explicit colour (SGR 58), resolved now so it tracks the ink; the
	// fallback to the foreground is applied after the swap, below, so it follows inverse too.
	let explicit_underline = cell.underline_color().map(|color| to_iced_color(color, fg));

	if cell.inverse() ^ marks.cursor {
		std::mem::swap(&mut fg, &mut bg);
	}
	// The find bar's matches (§39): a wash under every hit on screen. Applied BEFORE the selection,
	// so the current hit — which revealing already turned into an ordinary selection (§35) — keeps
	// the selection's fill and stays the one the eye lands on. That ordering is the whole reason the
	// match list can include the current match and stay ignorant of which one it is.
	if marks.matched {
		bg = MATCH_BG;
	}
	if marks.selected {
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
	if marks.link_hover && underline == UnderlineStyle::None {
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
fn lerp(from: Color, to: Color, t: f32) -> Color {
	Color::from_rgba(
		from.r + (to.r - from.r) * t,
		from.g + (to.g - from.g) * t,
		from.b + (to.b - from.b) * t,
		from.a + (to.a - from.a) * t,
	)
}

/// Map a cell color to an iced color. `Default` becomes the caller's default (different
/// for fg and bg); indexed colors go through the shared xterm-256 palette.
fn to_iced_color(color: CellColor, default: Color) -> Color {
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
	use crate::ui::selection::DocSpot;

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
		let selection =
			Selection::new(DocSpot { line: 0, col: 1 }).with_head(DocSpot { line: 0, col: 2 });
		let marks = Marks {
			selection: Some(&selection),
			..Marks::default()
		};
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, marks, None);

		// "a" | "bc" (selected) | "de"
		assert_eq!(runs.len(), 3);
		assert_eq!(runs[1].content, "bc");
		assert_eq!(runs[1].style.bg, SELECTION_BG);
		assert_ne!(runs[0].style.bg, SELECTION_BG);
		assert_ne!(runs[2].style.bg, SELECTION_BG);
	}

	/// A tab is stored IN a cell by the engine (`put_tab`, so copy gets a real tab back) and must
	/// never reach the text shaper: to the grid it is one cell, to cosmic-text it is a jump to the
	/// next tab stop, so it displaces every glyph after it in its run (§117).
	///
	/// The bug this pins was reported as "selecting a substring moves the characters", and that is
	/// exactly how it presented: the selection decides where runs split, so it decides which
	/// characters end up sharing a run with the tab and therefore which ones move. `du --inodes`
	/// output — `COUNT<TAB>PATH` — is where it was found.
	#[test]
	fn a_tab_in_a_cell_never_reaches_the_shaper_however_the_selection_falls() {
		let mut terminal = Terminal::new(1, 24);
		terminal.process(b"23\t./trans_3");
		// The tab is really there in the grid, or this test is not testing what it says it is.
		assert_eq!(terminal.screen().cell(0, 2).unwrap().contents(), "\t");

		// No selection, and then every substring selection of the path. Each one puts the run
		// boundary somewhere different, which is what made the old bug come and go.
		let mut cases: Vec<Option<(u16, u16)>> = vec![None];
		for start in 0..15_u16 {
			for end in start..15 {
				cases.push(Some((start, end)));
			}
		}
		for case in cases {
			let selection = case.map(|(start, end)| {
				Selection::new(DocSpot {
					line: 0,
					col: start,
				})
				.with_head(DocSpot { line: 0, col: end })
			});
			let marks = Marks {
				selection: selection.as_ref(),
				..Marks::default()
			};
			let runs = plan_runs(terminal.screen(), 0, 24, false, 0, marks, None);
			for run in &runs {
				assert!(
					!run.content.chars().any(char::is_control),
					"a control character reached a run: {:?} at col {} (selection {case:?})",
					run.content,
					run.col
				);
			}
			// And the path is still drawn where the grid says it is, whatever the selection did to
			// the run boundaries. Column 8 is where `.` sits — the tab at column 2 advanced the
			// cursor to the next stop, leaving columns 3..=7 blank.
			let path: String = runs
				.iter()
				.flat_map(|run| {
					run.content
						.chars()
						.enumerate()
						.map(move |(index, glyph)| (usize::from(run.col) + index, glyph))
				})
				.filter(|(column, _)| (8..=16).contains(column))
				.map(|(_, glyph)| glyph)
				.collect();
			assert_eq!(path, "./trans_3", "selection {case:?}");
		}
	}

	/// Which grid column a run draws `glyph` at: the run's own start plus how far into its content
	/// the character sits. Written once because the mirrored case is only legible this way — a run
	/// that spans the page has one column, and what moved is where each character is inside it.
	fn column_of(run: &Run, glyph: char) -> usize {
		let offset = run
			.content
			.chars()
			.position(|char| char == glyph)
			.unwrap_or_else(|| panic!("{glyph:?} is not in {:?}", run.content));
		usize::from(run.col) + offset
	}

	/// A right-to-left character path is a rule about the DRAWING, not the grid (§76): the planner
	/// walks the same cells in the same order and then mirrors what came out. Column 0 of the data
	/// lands hard against the right edge, and the run's characters reverse with it.
	#[test]
	fn a_right_to_left_line_is_drawn_from_the_other_edge() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"abc\x1b[2 k");
		let runs = plan_runs(terminal.screen(), 0, 10, false, 0, Marks::default(), None);
		// The whole row is one run — the blanks past "abc" share its style — so the assertion is
		// about where each character lands rather than where the run starts.
		assert_eq!(runs.len(), 1);
		let run = &runs[0];
		assert_eq!(
			column_of(run, 'a'),
			9,
			"data column 0 draws at the right edge"
		);
		assert_eq!(column_of(run, 'b'), 8);
		assert_eq!(
			column_of(run, 'c'),
			7,
			"and the line reads leftwards from there"
		);
	}

	/// The same screen with no path set draws in data order — the control that says the test above
	/// is measuring the mirror rather than the run planner.
	#[test]
	fn a_left_to_right_line_is_drawn_where_the_data_says() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"abc");
		let runs = plan_runs(terminal.screen(), 0, 10, false, 0, Marks::default(), None);
		assert_eq!(runs.len(), 1);
		let run = &runs[0];
		assert_eq!(column_of(run, 'a'), 0);
		assert_eq!(column_of(run, 'b'), 1);
		assert_eq!(column_of(run, 'c'), 2);
	}

	/// The selection fill travels with its cells through the mirror, because the mirror runs AFTER
	/// the styles are resolved. A highlight that stayed in data columns would land on the text at
	/// the other end of the line, which is the bug this ordering exists to avoid.
	#[test]
	fn a_selection_on_a_mirrored_line_moves_with_its_text() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"abcde\x1b[2 k");
		// Data columns 1..2 — "bc" — selected.
		let selection =
			Selection::new(DocSpot { line: 0, col: 1 }).with_head(DocSpot { line: 0, col: 2 });
		let marks = Marks {
			selection: Some(&selection),
			..Marks::default()
		};
		let runs = plan_runs(terminal.screen(), 0, 10, false, 0, marks, None);
		let filled: Vec<_> = runs
			.iter()
			.filter(|run| run.style.bg == SELECTION_BG)
			.map(|run| (run.col, run.content.as_str()))
			.collect();
		// "bc" occupies data columns 1..2, so it is drawn at presentation columns 7..8, reversed.
		let all: Vec<_> = runs
			.iter()
			.map(|run| (run.col, run.content.as_str()))
			.collect();
		assert_eq!(filled, vec![(7, "cb")], "all runs: {all:?}");
	}

	/// A run that is not plain ASCII is a single cell by construction — the planner seals it, for
	/// font fallback — so the mirror never reverses a grapheme cluster into nonsense. This pins the
	/// invariant `mirror` relies on rather than the behaviour that follows from it.
	#[test]
	fn a_non_ascii_cell_crosses_the_mirror_whole() {
		let mut terminal = Terminal::new(2, 10);
		// A letter with a combining acute: one cell, two chars.
		terminal.process("ae\u{301}i\x1b[2 k".as_bytes());
		let runs = plan_runs(terminal.screen(), 0, 10, false, 0, Marks::default(), None);
		let combined = runs
			.iter()
			.find(|run| run.content.chars().count() > 1 && !run.content.is_ascii());
		let combined = combined.expect("the combining cell should be a run of its own");
		assert_eq!(
			combined.content, "e\u{301}",
			"unreversed, so still a letter"
		);
		assert_eq!(combined.cols, 1);
	}

	/// The selection is highlighted on whichever row is showing its line right now (§40): the
	/// planner adds the row to the viewport's top line, so scrolling moves the highlight WITH the
	/// text rather than leaving it on the row it was dragged over.
	#[test]
	fn the_selection_highlight_follows_its_line_as_the_view_scrolls() {
		// A two-row screen fed four lines: two scrolled off, so the visible rows show lines 2 and 3.
		let mut terminal = Terminal::new(2, 5);
		terminal.process(b"aaaaa\r\nbbbbb\r\nccccc\r\nddddd");
		// Line 1 ("bbbbb") is selected whole — up in the history, off the screen.
		let selection =
			Selection::new(DocSpot { line: 1, col: 0 }).with_head(DocSpot { line: 1, col: 4 });

		// At the live bottom the top row shows line 2, so nothing on screen is selected.
		let marks = |terminal: &Terminal| Marks {
			selection: Some(&selection),
			top_line: terminal.screen().line_at(0),
			matches: &[],
		};
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, marks(&terminal), None);
		assert!(runs.iter().all(|run| run.style.bg != SELECTION_BG));

		// Scrolled up one line the top row shows line 1, and the whole row draws in the fill.
		terminal.scroll(crate::term::ScrollMotion::Lines(1));
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, marks(&terminal), None);
		assert_eq!(runs.len(), 1);
		assert_eq!(runs[0].content, "bbbbb");
		assert_eq!(runs[0].style.bg, SELECTION_BG);
	}

	/// An inline image is drawn at the row its own document line is showing at, at its native pixel
	/// size, inside the box of cells it reserved (§41). The projection is the reverse of the run
	/// planner's, so scrolling moves the picture with its text — including off the top, where its row
	/// goes negative and the visible part is what remains.
	#[test]
	fn an_image_is_drawn_at_the_row_its_line_is_showing_at() {
		let placement = Placement {
			line: 7,
			col: 2,
			rows: 3,
			cols: 4,
			width: 25,
			height: 40,
			handle: iced::advanced::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
		};
		let origin = Point::new(10.0, 20.0);

		// The top visible row is showing line 5, so the picture's line 7 is two rows down.
		let (pixels, reserved) = image_bounds(&placement, origin, 5);
		assert_px!(pixels.x, 10.0 + 2.0 * CELL_WIDTH);
		assert_px!(pixels.y, 20.0 + 2.0 * CELL_HEIGHT);
		// Drawn at its own size, inside a box of whole cells that is never smaller than it.
		assert_eq!((pixels.width, pixels.height), (25.0, 40.0));
		assert_px!(reserved.width, 4.0 * CELL_WIDTH);
		assert_px!(reserved.height, 3.0 * CELL_HEIGHT);
		assert!(reserved.width >= pixels.width && reserved.height >= pixels.height);

		// Scrolled down so the anchor is two rows ABOVE the viewport: the box hangs off the top and
		// the clip against the grid is what leaves only the visible slice.
		let (pixels, _) = image_bounds(&placement, origin, 9);
		assert_px!(pixels.y, 20.0 - 2.0 * CELL_HEIGHT);
	}

	#[test]
	fn every_on_screen_match_is_washed_and_the_current_one_keeps_the_selection_fill() {
		// Arrange: "ab ab" with both hits on row 0 (columns 0-1 and 3-4), and the SECOND one also
		// selected — which is exactly the state the find bar leaves behind, since revealing the
		// current match turns it into an ordinary selection (§35).
		let mut terminal = Terminal::new(1, 5);
		terminal.process(b"ab ab");
		let hits = [
			SearchHighlight {
				row: 0,
				start_col: 0,
				end_col: 1,
			},
			SearchHighlight {
				row: 0,
				start_col: 3,
				end_col: 4,
			},
		];
		let selection =
			Selection::new(DocSpot { line: 0, col: 3 }).with_head(DocSpot { line: 0, col: 4 });
		let mask = match_mask(&hits, 1, 5);
		let marks = Marks {
			selection: Some(&selection),
			top_line: 0,
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
			SearchHighlight {
				row: 5,
				start_col: 0,
				end_col: 1,
			},
			SearchHighlight {
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
	fn the_scroll_bar_shows_whenever_there_is_history_to_move_through() {
		// Before any line has scrolled off there is nowhere to go, so no bar (§23). But once there IS
		// history the bar is drawn even at the live tail, which is the §116 change and the whole point
		// of it: a bar that only appeared after you had scrolled by other means could not be the thing
		// you scroll WITH.
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		assert!(scrollbar_thumb(bounds, 24, 0, 0).is_none());
		assert!(scrollbar_thumb(bounds, 24, 100, 0).is_some());
		assert!(scrollbar_thumb(bounds, 24, 100, 1).is_some());
		// And at the tail it is parked at the very bottom of the span it slides in, so it reads as
		// "nothing below this" without needing to disappear to say so.
		let parked = scrollbar_thumb(bounds, 24, 100, 0).unwrap();
		let track = scrollbar_track(bounds, 24);
		assert!(
			(parked.y + parked.height - (track.y + track.height)).abs() < 0.01,
			"parked at the track's bottom edge, got {parked:?} in {track:?}"
		);
	}

	#[test]
	fn the_grab_zone_is_the_whole_gutter_and_contains_the_painted_thumb() {
		// The paint is 4px so it reads as an indicator; the press target is the 6px gutter, because a
		// 4px target is one you miss (§116). The zone must contain the thumb — otherwise there are
		// pixels that look grabbable and are not — and must not reach the first cell, which begins one
		// GRID_PADDING in from the left and ends one GRID_PADDING short of the right edge.
		let bounds = Rectangle {
			x: 17.0,
			y: 9.0,
			width: 200.0,
			height: 400.0,
		};
		let track = scrollbar_track(bounds, 24);
		let thumb = scrollbar_thumb(bounds, 24, 100, 50).unwrap();
		assert!(track.x <= thumb.x, "the zone starts left of the paint");
		assert!(
			thumb.x + thumb.width <= track.x + track.width,
			"and ends right of it"
		);
		// The right edge of the text area, which the zone must not cross back over.
		assert!((track.x - (bounds.x + bounds.width - GRID_PADDING)).abs() < 0.01);
		// Vertically the zone is the text rows, matching the span the thumb travels.
		assert!(track.y >= bounds.y + GRID_PADDING - 0.01);
		assert!((track.height - 24.0 * CELL_HEIGHT).abs() < 0.01);
	}

	#[test]
	fn a_press_maps_back_to_the_offset_the_thumb_was_drawn_for() {
		// `scrollbar_offset` is `scrollbar_thumb` read backwards, and the two are only correct
		// together (§116) — so the assertion is the ROUND TRIP rather than either one's arithmetic.
		// A drag hands back the thumb's top; drawing that offset must put the thumb where the drag
		// asked for it.
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		for (history, offset) in [
			(100_u16, 0_u16),
			(100, 1),
			(100, 50),
			(100, 99),
			(100, 100),
			// A deep history is the case the pre-§116 mapping got wrong: it clamped instead of
			// scaling, so a range of offsets all drew at the same bottom-most thumb and the round
			// trip could not hold.
			(5000, 0),
			(5000, 2500),
			(5000, 5000),
			(1, 0),
			(1, 1),
		] {
			let thumb = scrollbar_thumb(bounds, 24, history, offset).unwrap();
			assert_eq!(
				scrollbar_offset(bounds, 24, history, thumb.y),
				offset,
				"history {history}, offset {offset}, thumb at {}",
				thumb.y
			);
		}
	}

	#[test]
	fn a_drag_past_either_end_pins_rather_than_wrapping() {
		// A drag is not bounds-tested on the move, so the pointer routinely runs off both ends of the
		// track (§116). Off the top is the oldest retained line and off the bottom is the live tail —
		// never a wrap round to the other end, which is what an unclamped subtraction would give.
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		let track = scrollbar_track(bounds, 24);
		assert_eq!(scrollbar_offset(bounds, 24, 100, track.y - 500.0), 100);
		assert_eq!(
			scrollbar_offset(bounds, 24, 100, track.y + track.height + 500.0),
			0
		);
		// Far outside in both directions, and with a degenerate history, still answers in range.
		assert_eq!(scrollbar_offset(bounds, 24, 0, track.y), 0);
		assert_eq!(scrollbar_offset(bounds, 24, 1, f32::NEG_INFINITY), 1);
		assert_eq!(scrollbar_offset(bounds, 24, 1, f32::INFINITY), 0);
	}

	#[test]
	fn a_thumb_that_fills_its_track_reads_as_the_live_bottom() {
		// A grid only a few rows tall has a thumb floored to scrollbar::MIN_THUMB, which can be as tall
		// as the track — leaving no span to slide in. That is a division by zero waiting to happen, so
		// it answers the live bottom instead (§116).
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		let rows = 1;
		let track = scrollbar_track(bounds, rows);
		assert!(
			scrollbar_thumb_height(rows, 500, track.height) >= track.height,
			"the floor really does fill this track, or the case under test is not the case"
		);
		assert_eq!(scrollbar_offset(bounds, rows, 500, track.y), 0);
		assert_eq!(scrollbar_offset(bounds, rows, 500, track.y + 1000.0), 0);
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
		close(top.x, bounds.width - scrollbar::WIDTH - scrollbar::INSET);

		// Returning toward the live tail slides the thumb down the track, monotonically.
		let middle = scrollbar_thumb(bounds, 24, 100, 50).unwrap();
		let near_bottom = scrollbar_thumb(bounds, 24, 100, 1).unwrap();
		assert!(top.y < middle.y && middle.y < near_bottom.y);

		// However deep the history, the thumb stays inside the track and never shorter than the
		// floor that keeps it visible.
		let deep = scrollbar_thumb(bounds, 24, 5000, 2500).unwrap();
		close(deep.height, scrollbar::MIN_THUMB);
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
	fn link_at_reads_the_link_on_a_cell_and_nothing_off_it() {
		// An OSC 8 link over "site": every one of its four cells carries the same link, and the
		// plain 'X' after the close carries none (§24, §92).
		let cols = 20;
		let mut terminal = Terminal::new(1, cols);
		terminal.process(b"\x1b]8;;https://example.com\x07site\x1b]8;;\x07X");
		let screen = terminal.screen();
		let first =
			link_at(screen, ScreenSpot { row: 0, col: 0 }).expect("the first cell is linked");
		assert_eq!(first.uri(), "https://example.com");
		assert_eq!(
			link_at(screen, ScreenSpot { row: 0, col: 2 }).as_ref(),
			Some(&first),
			"every cell of one link reads back as the same link"
		);
		assert_eq!(link_at(screen, ScreenSpot { row: 0, col: 4 }), None);
	}

	#[test]
	fn one_address_written_twice_is_two_links() {
		// The identity is the whole link, not the URI (§92). The engine gives each `ESC ] 8` that
		// carries no `id=` an identifier of its own, so two separate links to one address stay two
		// links — and a hover over one must not underline the other. Written adjacent, which is
		// the arrangement the old contiguous walk joined into one.
		let cols = 20;
		let mut terminal = Terminal::new(1, cols);
		terminal.process(
			b"\x1b]8;;https://example.com\x07ab\x1b]8;;\x07\x1b]8;;https://example.com\x07cd\x1b]8;;\x07",
		);
		let screen = terminal.screen();
		let left = link_at(screen, ScreenSpot { row: 0, col: 0 }).expect("linked");
		let right = link_at(screen, ScreenSpot { row: 0, col: 2 }).expect("linked");
		assert_eq!(left.uri(), right.uri(), "the same address");
		assert_ne!(left, right, "and still not the same link");
	}

	#[test]
	fn a_link_split_into_runs_by_its_id_is_one_link() {
		// The other direction, and the one the specification's own example is about: a program may
		// split a link into separate runs and tie them together with `id=`. Both runs read back as
		// one link, which a contiguous walk could never have said (§92).
		let cols = 20;
		let mut terminal = Terminal::new(1, cols);
		terminal.process(
			b"\x1b]8;id=1;https://example.com\x07ab\x1b]8;;\x07 \x1b]8;id=1;https://example.com\x07cd\x1b]8;;\x07",
		);
		let screen = terminal.screen();
		let first = link_at(screen, ScreenSpot { row: 0, col: 0 }).expect("linked");
		let second = link_at(screen, ScreenSpot { row: 0, col: 3 }).expect("linked");
		assert_eq!(first, second, "one id, one link, across the gap");
		assert_eq!(
			link_at(screen, ScreenSpot { row: 0, col: 2 }),
			None,
			"and the gap between them is not part of it"
		);
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

		let link = link_at(terminal.screen(), ScreenSpot { row: 0, col: 0 }).expect("the link");
		let hovered = plan_runs(
			terminal.screen(),
			0,
			cols,
			false,
			0,
			Marks::default(),
			Some(&link),
		);
		let underlined: String = hovered
			.iter()
			.filter(|run| run.style.underline == UnderlineStyle::Single)
			.map(|run| run.content.as_str())
			.collect();
		assert_eq!(underlined, "site");
	}
	/// The bar's three opacities come from the two facts the widget has, and a drag outranks a hover
	/// (§125). It has to: §116's drag survives the pointer straying off the bar, so mid-drag the
	/// hover flag says nothing about the bar and the drag says everything.
	#[test]
	fn a_dragged_bar_reads_as_dragged_even_with_the_pointer_gone() {
		assert_eq!(scrollbar_touch(false, false), scrollbar::Touch::Idle);
		assert_eq!(scrollbar_touch(false, true), scrollbar::Touch::Hovered);
		assert_eq!(scrollbar_touch(true, true), scrollbar::Touch::Dragged);
		assert_eq!(
			scrollbar_touch(true, false),
			scrollbar::Touch::Dragged,
			"the pointer left the lane; the bar is still being pulled"
		);
	}

	/// The gutter is cmote's furniture, so the bar's hand beats a remote's OSC 22 shape (§77, §125).
	/// Before §125 the shape sat on the `mouse_area` outside this widget, which applies over the
	/// padding too and only when the content says `Interaction::None` — which is exactly what this
	/// widget answers over its own bar on Windows, so the remote's shape was reaching the one strip of
	/// pixels that is not its page.
	#[test]
	fn the_bars_hand_beats_the_shape_a_remote_asked_for() {
		let terminal = crate::term::Terminal::new(4, 8);
		let asked = mouse::Interaction::Crosshair;
		let grid = grid(
			terminal.screen(),
			None,
			Vec::new(),
			Vec::new(),
			Vec::new(),
			&[],
			Some(asked),
		);

		// Over the cells: the remote's shape, exactly as before.
		assert_eq!(grid.interaction_over(false, false, true), asked);
		// Over the bar, and while dragging it: NOT the remote's shape. Asserted as "not that" rather
		// than as an equality with `grab_interaction`'s own answer, which would be the test
		// recomputing the expected value the way the code does — and would pass however the two
		// claims were ordered. The shapes it may be are the hand's, or `None` on Windows once the
		// bitmaps are installed, which is the answer that hands the paint to the `WM_SETCURSOR` seam
		// (§51).
		let over_bar = grid.interaction_over(false, true, true);
		let held = grid.interaction_over(true, false, true);
		assert_ne!(over_bar, asked, "the bar wins over what a remote asked for");
		assert_ne!(held, asked, "and it keeps winning for the whole drag");
		assert!(
			matches!(
				over_bar,
				mouse::Interaction::None | mouse::Interaction::Grab
			),
			"an open hand or nothing at all, not {over_bar:?}"
		);
		assert!(
			matches!(
				held,
				mouse::Interaction::None | mouse::Interaction::Grabbing
			),
			"a closed hand or nothing at all, not {held:?}"
		);
		// Off the widget entirely: nothing, so the shape cannot follow the pointer onto the strip or
		// into a dialog.
		assert_eq!(
			grid.interaction_over(false, false, false),
			mouse::Interaction::None
		);
	}

	/// And with no OSC 22 in play the answer is what it always was: hand the question back, so the
	/// `mouse_area`'s text cursor over the cells keeps deciding (§77).
	#[test]
	fn a_grid_no_remote_has_dressed_says_nothing_over_its_cells() {
		let terminal = crate::term::Terminal::new(4, 8);
		let grid = grid(
			terminal.screen(),
			None,
			Vec::new(),
			Vec::new(),
			Vec::new(),
			&[],
			None,
		);
		assert_eq!(
			grid.interaction_over(false, false, true),
			mouse::Interaction::None
		);
	}
}

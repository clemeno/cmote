// ui/scrollbar.rs — one scrollbar look, for every surface that scrolls (PLAN §118).
//
// §23 drew the terminal a scroll indicator of its own: a thin translucent thumb in the grid's right
// padding gutter, no track behind it, and §116 made that thumb grabbable. Meanwhile the four
// `scrollable`s in the app — the folder tree (§18), the files pane (§19), the home screen's target
// list (§14) and the editor's buffer (§32) — were all on iced's default: twice as wide, a visible
// rail behind the scroller, and rounded to a different radius. Two scrollbars in one window, and the
// terminal's was the one that had been designed.
//
// So this file owns the LOOK, once, and both kinds of scrollbar read it from here.
//
// What is shared is deliberately the appearance and not the mechanism, because the mechanism cannot
// be:
//
//   * The terminal's bar is quads the grid paints itself. It has to be — the scroll position lives in
//     the emulator's display offset, not in a widget, and the grid is one widget for the whole screen
//     (§9). `ui::grid`'s `scrollbar_thumb` / `scrollbar_offset` are that geometry and stay there;
//     they speak in rows, history and viewport offsets, which are terminal words.
//   * A pane's bar is iced's, inside a `scrollable`, and iced computes the thumb from the content
//     height. Nothing here can reach into that arithmetic.
//
// The numbers below are therefore the single source of truth for both, and the two consumers differ
// in what they can do with them. One place that shows: iced's minimum scroller length is a hard-coded
// `.max(2.0)` in `iced_widget::scrollable`, so `MIN_THUMB` — the floor that keeps the terminal's thumb
// findable over ten thousand lines of scrollback — applies to the terminal only. A pane scrolling a
// very long list gets a smaller thumb than the terminal would. That is a real difference and not one
// this module can close.

use iced::widget::scrollable;
use iced::{Border, Color, Rectangle, Theme, mouse};

use crate::app::Message;

/// How wide the bar is drawn. Thin on purpose: it is a position report first and a control second, so
/// it should not read as furniture down the edge of every pane.
pub const WIDTH: f32 = 4.0;

/// How far the bar sits in from the surface's edge. With `WIDTH` this makes the lane it occupies
/// `WIDTH + 2 * INSET` = 6px, which is exactly `ui::terminal::GRID_PADDING` — so the terminal's bar
/// lands inside its own padding gutter and touches no cell, and a pane's takes the same width away
/// from its content.
pub const INSET: f32 = 1.0;

/// The shortest the thumb may be drawn, so a deep history still shows something findable.
///
/// **The terminal's floor only.** iced hard-codes its own minimum scroller length, so a `scrollable`
/// cannot be told this; see the module note.
pub const MIN_THUMB: f32 = 16.0;

/// The thumb's colour: a light grey at just over half alpha, so it reads over both a dark pane
/// (`explorer::PANEL_BG`) and the terminal's own background without either needing to know about it.
///
/// Translucent rather than a flat grey because the surfaces underneath differ — the panes, the two
/// editor themes and the terminal are all dark but not the same dark, and an alpha blend lands in the
/// right place on each of them by construction rather than by a colour per surface.
pub const THUMB: Color = Color::from_rgba(0.82, 0.82, 0.82, 0.55);

/// How a bar is being touched right now (§125) — the one vocabulary both surfaces answer in.
///
/// Neither surface's own state is usable by the other: a pane's comes from iced's
/// `scrollable::Status`, which names four flags per axis, and the terminal's from a grip field and a
/// hit test. So the shared thing is this, and each side maps its own facts onto it. Three values and
/// not a pair of bools, because "hovered while dragged" is not a state — a drag holds the bar
/// whether or not the pointer has strayed off it (§116).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Touch {
	/// Nothing is on it.
	Idle,
	/// The pointer is over its lane.
	Hovered,
	/// It is being dragged, wherever the pointer has got to.
	Dragged,
}

/// The thumb's colour for how it is being touched (§125).
///
/// Only the ALPHA moves, and that is what keeps one colour working over four different darks: the
/// grey is chosen to blend, so brightening it by opacity stays in the right place on each surface,
/// where a lighter grey would drift towards white on the darkest one and vanish on the lightest.
///
/// §118 said the look does not change with status and that a bar which brightened under the pointer
/// "would no longer be the same bar" — true, and the answer is that BOTH bars brighten now. The
/// decision this replaces was never "no feedback"; it was "not on one surface only".
pub fn thumb(touch: Touch) -> Color {
	Color {
		a: match touch {
			Touch::Idle => THUMB.a,
			Touch::Hovered => 0.75,
			Touch::Dragged => 0.95,
		},
		..THUMB
	}
}

/// The thumb's corner radius — fully rounded, which for a bar `WIDTH` across is half of it.
pub fn radius() -> f32 {
	WIDTH / 2.0
}

/// The geometry to hand a `scrollable`'s `Scrollbar`, so its bar matches the terminal's.
///
/// `scroller_width` equals `width`: iced sizes the lane as `width.max(scroller_width) + 2 * margin`,
/// so making them the same is what keeps that lane at the 6px `INSET` was chosen for. A wider rail
/// than scroller would only matter if there were a rail to see, and there is not.
pub fn bar() -> scrollable::Scrollbar {
	scrollable::Scrollbar::new()
		.width(WIDTH)
		.scroller_width(WIDTH)
		.margin(INSET)
}

/// The `scrollable` style: iced's own, with both rails replaced by the terminal's thumb.
///
/// Built ON TOP of `scrollable::default` rather than from nothing, so the parts this file has no
/// opinion about — the container, the gap between two rails, the autoscroll overlay — keep following
/// the theme instead of being frozen here at whatever they happened to be.
///
/// Each rail reads its OWN axis out of `status` (§125). iced reports the two axes separately, and a
/// style that took "hovered" to mean both would brighten the horizontal bar because the pointer was
/// on the vertical one — only the editor's buffer has both, so it would have been a bug visible in
/// exactly one place.
pub fn style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
	scrollable::Style {
		vertical_rail: rail(touch_of(status, Axis::Vertical)),
		horizontal_rail: rail(touch_of(status, Axis::Horizontal)),
		..scrollable::default(theme, status)
	}
}

/// Which of a `scrollable`'s two bars is being asked about — private, because outside this file the
/// two axes are `Axes`' named fields and never an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
	Vertical,
	Horizontal,
}

/// One rail: the terminal's thumb at this touch, over no track.
///
/// No track at all. The terminal draws the thumb over whatever is behind it and nothing else, which
/// is what keeps a bar this thin from reading as a groove cut down the edge of the pane.
fn rail(touch: Touch) -> scrollable::Rail {
	scrollable::Rail {
		background: None,
		border: Border::default(),
		scroller: scrollable::Scroller {
			background: thumb(touch).into(),
			border: Border::default().rounded(radius()),
		},
	}
}

/// One axis' [`Touch`], read out of iced's four-flags-per-axis `Status` (§125).
///
/// **The two flags cannot both be set here**, `Status` being one variant at a time, so the order of
/// the branches below is not load-bearing and no test pretends otherwise — a probe that swapped them
/// stayed green, which is how that was established. The precedence that IS real belongs to the
/// terminal's bar, where the two facts are independent booleans: see `ui::grid::scrollbar_touch`.
///
/// What the branches do carry is the per-AXIS read, and the compiler holds that one: the pattern
/// names both flags, so using one of them twice is a denied `unused_variables` rather than a wrong
/// colour on the editor's horizontal bar. Both probes were tried; that is the one that would not
/// build.
fn touch_of(status: scrollable::Status, axis: Axis) -> Touch {
	let (hovered, dragged) = match status {
		scrollable::Status::Active { .. } => (false, false),
		scrollable::Status::Hovered {
			is_vertical_scrollbar_hovered,
			is_horizontal_scrollbar_hovered,
			..
		} => match axis {
			Axis::Vertical => (is_vertical_scrollbar_hovered, false),
			Axis::Horizontal => (is_horizontal_scrollbar_hovered, false),
		},
		scrollable::Status::Dragged {
			is_vertical_scrollbar_dragged,
			is_horizontal_scrollbar_dragged,
			..
		} => match axis {
			Axis::Vertical => (false, is_vertical_scrollbar_dragged),
			Axis::Horizontal => (false, is_horizontal_scrollbar_dragged),
		},
	};
	if dragged {
		Touch::Dragged
	} else if hovered {
		Touch::Hovered
	} else {
		Touch::Idle
	}
}

/// Which axes a `scrollable` was given a bar on, so the hit test below knows which lanes can exist
/// (§120). Named fields rather than two `bool` arguments for the reason `CellMarks` gives: at a call
/// site `grabbable(inner, true, false)` is one transposition from a wrong answer and the compiler has
/// nothing to say about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axes {
	pub vertical: bool,
	pub horizontal: bool,
}

impl Axes {
	/// A `Direction::Vertical` scrollable — the three panes.
	pub const VERTICAL: Self = Self {
		vertical: true,
		horizontal: false,
	};
	/// A `Direction::Both` scrollable — the editor's buffer (§32).
	pub const BOTH: Self = Self {
		vertical: true,
		horizontal: true,
	};
}

/// The rectangles a `scrollable`'s bars occupy — vertical, then horizontal — for a widget of
/// `bounds` whose content laid out to `content` (§120). `None` for an axis with no bar: either it was
/// never asked for, or the content fits and iced draws nothing.
///
/// **This mirrors `iced_widget::scrollable`'s own arithmetic and has to.** It is the lane iced itself
/// grabs on (`Scrollbar::is_mouse_over` tests `total_bounds`), so the hand must appear over exactly
/// the pixels that would start a drag — a hand over a strip that does nothing, or no hand over one
/// that works, are both worse than no hand at all. The two asymmetries below are iced's and are
/// reproduced deliberately rather than tidied: the vertical lane shortens by the horizontal bar's
/// `width + margin`, while the horizontal lane narrows by the vertical lane's FULL
/// `width + 2 * margin`.
///
/// Pure, so the part that can silently drift from iced is the part a test can hold onto — and the
/// release it was read against is pinned by `the_mirror_names_the_iced_release_it_was_read_from`
/// below, so an upgrade cannot land quietly (§125).
pub fn lanes(
	bounds: Rectangle,
	content: Rectangle,
	axes: Axes,
) -> (Option<Rectangle>, Option<Rectangle>) {
	let lane = WIDTH + 2.0 * INSET;
	let show_y = axes.vertical && content.height > bounds.height;
	let show_x = axes.horizontal && content.width > bounds.width;

	let vertical = show_y.then(|| Rectangle {
		x: bounds.x + bounds.width - lane,
		y: bounds.y,
		width: lane,
		height: (bounds.height - if show_x { WIDTH + INSET } else { 0.0 }).max(0.0),
	});
	let horizontal = show_x.then(|| Rectangle {
		x: bounds.x,
		y: bounds.y + bounds.height - lane,
		width: (bounds.width - if show_y { lane } else { 0.0 }).max(0.0),
		height: lane,
	});
	(vertical, horizontal)
}

/// What the wrapper remembers between events (§120): whether the pointer was last on a bar, so the
/// hand's enter and exit go out on the CHANGE, and whether a drag is in flight, so the hand stays
/// closed for the whole of it.
///
/// `dragging` is tracked here rather than read from the `scrollable` because iced does not tell:
/// `State::scrollers_grabbed` is private. Tracking it is exact anyway — iced starts a drag on a press
/// inside the same lane this widget tests, so the two agree by construction.
#[derive(Debug, Default)]
struct HandState {
	on_bar: bool,
	dragging: bool,
}

/// A `scrollable` wearing the §51 hand over its bars (§120).
///
/// A decorator and not a fork. `iced_widget::scrollable` computes whether the pointer is over its own
/// bar and then answers `Interaction::None` regardless, with no hook to say otherwise — so the choice
/// was to reimplement the widget or to wrap it. This wraps: every `Widget` method forwards to the
/// scrollable untouched, `mouse_interaction` answers the hand over a lane, and `update` maintains the
/// claim `cursor` paints from on Windows. Nothing is captured, so the scrollable still sees every
/// event it saw before.
pub struct Grabbable<'a> {
	content: iced::Element<'a, Message>,
	axes: Axes,
	/// Which bar this is — a `cursor::SCROLLBAR_*`. One name per bar, for the reason `cursor` gives
	/// where they are defined: under a shared name any bar on screen keeps every other bar's claim
	/// alive, and the terminal's bar vanishing stopped letting go of the hand.
	handle: u64,
}

/// Wrap `inner` — a `scrollable` styled with `bar()` and `style()` — so its bars wear the hand
/// (§120), and assert to `cursor` that a bar of ours is on screen this frame.
///
/// The `drawn` call lives here rather than at the four call sites for two reasons. It is one place
/// instead of four to remember, and — the part that matters — this function runs during `App::view`,
/// which is the phase `cursor::frame_begin` / `frame_end` bracket. §119 learned that the hard way: the
/// same call from a `Widget::draw` lands after the frame it belongs to has been judged, and the hand
/// flickers off on the next one.
///
/// It asserts unconditionally, where the terminal's own call can ask `history_size() > 0` first. This
/// function cannot ask the equivalent — whether the content overflows is a LAYOUT fact and layout has
/// not run yet — so a pane whose list shrank to fit while the pointer sat still on its bar keeps the
/// hand until the pointer moves. Harmless in the ordinary case (`drawn` does nothing unless this
/// handle already holds the claim, and only a real lane hit ever gives it that), and the same
/// under-claim `covered()` documents in §52: a missing hand is a smaller lie than a hand over
/// something that cannot be dragged, and this errs the other way for one stationary frame.
pub fn grabbable<'a>(
	inner: impl Into<iced::Element<'a, Message>>,
	axes: Axes,
	handle: u64,
) -> iced::Element<'a, Message> {
	crate::cursor::drawn(handle);
	iced::Element::new(Grabbable {
		content: inner.into(),
		axes,
		handle,
	})
}

impl Grabbable<'_> {
	/// Whether `cursor` is over either of this scrollable's bars.
	fn on_bar(&self, layout: iced::advanced::Layout<'_>, cursor: mouse::Cursor) -> bool {
		let Some(position) = cursor.position() else {
			return false;
		};
		// The scrollable's own layout node is this widget's node — `layout` below forwards rather than
		// wrapping — so its first child is the CONTENT, exactly as iced reads it internally.
		let Some(content) = layout.children().next() else {
			return false;
		};
		let (vertical, horizontal) = lanes(layout.bounds(), content.bounds(), self.axes);
		[vertical, horizontal]
			.into_iter()
			.flatten()
			.any(|lane| lane.contains(position))
	}
}

impl iced::advanced::Widget<Message, iced::Theme, iced::Renderer> for Grabbable<'_> {
	fn tag(&self) -> iced::advanced::widget::tree::Tag {
		iced::advanced::widget::tree::Tag::of::<HandState>()
	}

	fn state(&self) -> iced::advanced::widget::tree::State {
		iced::advanced::widget::tree::State::new(HandState::default())
	}

	fn children(&self) -> Vec<iced::advanced::widget::Tree> {
		vec![iced::advanced::widget::Tree::new(&self.content)]
	}

	fn diff(&self, tree: &mut iced::advanced::widget::Tree) {
		tree.diff_children(std::slice::from_ref(&self.content));
	}

	fn size(&self) -> iced::Size<iced::Length> {
		self.content.as_widget().size()
	}

	fn layout(
		&mut self,
		tree: &mut iced::advanced::widget::Tree,
		renderer: &iced::Renderer,
		limits: &iced::advanced::layout::Limits,
	) -> iced::advanced::layout::Node {
		// The child's node verbatim, which is what keeps `on_bar`'s reading of it honest: this widget
		// adds no box of its own, so `layout` in every method here IS the scrollable's.
		self.content
			.as_widget_mut()
			.layout(&mut tree.children[0], renderer, limits)
	}

	fn operate(
		&mut self,
		tree: &mut iced::advanced::widget::Tree,
		layout: iced::advanced::Layout<'_>,
		renderer: &iced::Renderer,
		operation: &mut dyn iced::advanced::widget::Operation,
	) {
		self.content
			.as_widget_mut()
			.operate(&mut tree.children[0], layout, renderer, operation);
	}

	fn update(
		&mut self,
		tree: &mut iced::advanced::widget::Tree,
		event: &iced::Event,
		layout: iced::advanced::Layout<'_>,
		cursor: mouse::Cursor,
		renderer: &iced::Renderer,
		clipboard: &mut dyn iced::advanced::Clipboard,
		shell: &mut iced::advanced::Shell<'_, Message>,
		viewport: &Rectangle,
	) {
		// The scrollable first and unconditionally: it owns the scrolling, and nothing here may change
		// whether it hears an event.
		self.content.as_widget_mut().update(
			&mut tree.children[0],
			event,
			layout,
			cursor,
			renderer,
			clipboard,
			shell,
			viewport,
		);

		let iced::Event::Mouse(pointer) = event else {
			return;
		};
		let state = tree.state.downcast_mut::<HandState>();
		let on_bar = self.on_bar(layout, cursor);

		// The drag, so the hand stays closed for the whole of it. A press elsewhere is not ours, and a
		// release is answered only if we saw the press — otherwise letting go of a tab over a pane
		// would open the hand on this widget's behalf.
		match pointer {
			mouse::Event::ButtonPressed(mouse::Button::Left) if on_bar => {
				state.dragging = true;
				shell.publish(Message::ScrollbarGrabbed);
			}
			mouse::Event::ButtonReleased(mouse::Button::Left) if state.dragging => {
				state.dragging = false;
				shell.publish(Message::ScrollbarReleased);
			}
			_ => {}
		}

		// And the hover, on the change only — the same shape `ui::grid` uses for the terminal's bar,
		// and the same reason one `cursor::SCROLLBAR` name serves every bar in the window: a widget
		// that never had the hand never says it lost it (§119).
		if on_bar != state.on_bar {
			state.on_bar = on_bar;
			shell.publish(if on_bar {
				Message::GrabEntered(self.handle)
			} else {
				Message::GrabExited(self.handle)
			});
		}
	}

	/// The hand over a bar, and whatever the scrollable said anywhere else.
	///
	/// WHO draws it is `grab_interaction`'s business and not this file's (§51): `None` on Windows so
	/// iced is asked for nothing and `cursor`'s `WM_SETCURSOR` seam paints the bitmaps, the real
	/// interaction everywhere else.
	fn mouse_interaction(
		&self,
		tree: &iced::advanced::widget::Tree,
		layout: iced::advanced::Layout<'_>,
		cursor: mouse::Cursor,
		viewport: &Rectangle,
		renderer: &iced::Renderer,
	) -> mouse::Interaction {
		let inner = self.content.as_widget().mouse_interaction(
			&tree.children[0],
			layout,
			cursor,
			viewport,
			renderer,
		);
		let state = tree.state.downcast_ref::<HandState>();
		if !state.dragging && !self.on_bar(layout, cursor) {
			return inner;
		}
		crate::cursor::grab_interaction(state.dragging).unwrap_or(inner)
	}

	fn draw(
		&self,
		tree: &iced::advanced::widget::Tree,
		renderer: &mut iced::Renderer,
		theme: &iced::Theme,
		style: &iced::advanced::renderer::Style,
		layout: iced::advanced::Layout<'_>,
		cursor: mouse::Cursor,
		viewport: &Rectangle,
	) {
		self.content.as_widget().draw(
			&tree.children[0],
			renderer,
			theme,
			style,
			layout,
			cursor,
			viewport,
		);
	}

	fn overlay<'b>(
		&'b mut self,
		tree: &'b mut iced::advanced::widget::Tree,
		layout: iced::advanced::Layout<'b>,
		renderer: &iced::Renderer,
		viewport: &Rectangle,
		translation: iced::Vector,
	) -> Option<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
		self.content.as_widget_mut().overlay(
			&mut tree.children[0],
			layout,
			renderer,
			viewport,
			translation,
		)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Float comparison for a geometry test, the same helper `ui::grid`'s thumb tests use: these are
	/// pixel arithmetic, so an exact `==` is both what clippy's `float_cmp` objects to and the wrong
	/// question to ask.
	fn close(left: f32, right: f32, what: &str) {
		assert!(
			(left - right).abs() < 0.01,
			"{what}: expected {right}, got {left}"
		);
	}

	/// The lane a pane's bar occupies is the same width as the terminal's padding gutter, which is
	/// what makes the two bars sit at the same distance from their surface's edge (§118). iced sizes
	/// the lane as `width.max(scroller_width) + 2 * margin`; this asserts the numbers here really do
	/// come out at `GRID_PADDING` rather than merely looking about right.
	#[test]
	fn the_bar_occupies_the_same_lane_as_the_terminals_own_gutter() {
		assert!(
			(WIDTH + 2.0 * INSET - crate::ui::terminal::GRID_PADDING).abs() < f32::EPSILON,
			"the lane is {} and the terminal's gutter is {}",
			WIDTH + 2.0 * INSET,
			crate::ui::terminal::GRID_PADDING
		);
	}

	/// A viewport its content fits inside has no bars, so there is nothing to wear a hand over
	/// (§120). The hand must not appear over a 6px strip that does nothing.
	#[test]
	fn content_that_fits_has_no_lanes() {
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		// Exactly the same size counts as fitting: iced's test is a strict `>`.
		let fits = bounds;
		assert_eq!(lanes(bounds, fits, Axes::BOTH), (None, None));
		let taller = Rectangle {
			height: 4000.0,
			..bounds
		};
		// Overflowing DOWN with only a horizontal bar asked for is still no bar at all.
		assert_eq!(
			lanes(
				bounds,
				taller,
				Axes {
					vertical: false,
					horizontal: true
				}
			),
			(None, None)
		);
	}

	/// The lane is at the surface's edge, `WIDTH + 2 * INSET` across, and spans the whole of the
	/// other axis when it is the only bar (§120). These are `iced_widget::scrollable`'s own
	/// `total_bounds` numbers, mirrored — it is the rectangle iced grabs on, so the hand has to cover
	/// exactly it.
	#[test]
	fn one_lane_hugs_its_edge_and_spans_the_surface() {
		let bounds = Rectangle {
			x: 17.0,
			y: 9.0,
			width: 200.0,
			height: 400.0,
		};
		let lane = WIDTH + 2.0 * INSET;
		let (vertical, horizontal) = lanes(
			bounds,
			Rectangle {
				height: 4000.0,
				..bounds
			},
			Axes::VERTICAL,
		);
		assert_eq!(horizontal, None, "no horizontal bar was asked for");
		let vertical = vertical.expect("the content overflows downwards");
		close(vertical.width, lane, "the lane is WIDTH + 2 * INSET across");
		close(
			vertical.x,
			bounds.x + bounds.width - lane,
			"hugging the right edge",
		);
		close(vertical.y, bounds.y, "starting at the top");
		close(
			vertical.height,
			bounds.height,
			"no horizontal bar below it, so it runs the full height",
		);
	}

	/// With bars on both axes each lane gets out of the other's way — and by DIFFERENT amounts,
	/// which is iced's own asymmetry and reproduced rather than tidied (§120): the vertical lane
	/// shortens by the horizontal bar's `width + margin`, while the horizontal lane narrows by the
	/// vertical lane's full `width + 2 * margin`. Only the editor's buffer is `Both`, so this is the
	/// one place it shows.
	#[test]
	fn two_lanes_shrink_by_iceds_own_asymmetric_amounts() {
		let bounds = Rectangle {
			x: 0.0,
			y: 0.0,
			width: 200.0,
			height: 400.0,
		};
		let lane = WIDTH + 2.0 * INSET;
		let (vertical, horizontal) = lanes(
			bounds,
			Rectangle {
				width: 4000.0,
				height: 4000.0,
				..bounds
			},
			Axes::BOTH,
		);
		let vertical = vertical.expect("overflows downwards");
		let horizontal = horizontal.expect("overflows sideways");
		close(
			vertical.height,
			bounds.height - (WIDTH + INSET),
			"shortened by the horizontal bar's width + margin",
		);
		close(
			horizontal.width,
			bounds.width - lane,
			"narrowed by the vertical lane's whole width",
		);
		// The horizontal lane really is clear of the vertical one, because it subtracted the whole
		// lane width.
		close(
			horizontal.x + horizontal.width,
			vertical.x,
			"the horizontal lane stops where the vertical one starts",
		);
		// The vertical lane, however, OVERLAPS the horizontal one by exactly `INSET` — it subtracted
		// 5 where the lane below it is 6 tall. That is iced's own arithmetic and this test pins it
		// rather than wishing it away. It is harmless here: the hit test is a union of the two, so a
		// pixel that belongs to both still answers "a bar", which is the only question being asked.
		close(
			vertical.y + vertical.height - horizontal.y,
			INSET,
			"iced's asymmetry, mirrored: a one-pixel overlap in the corner",
		);
	}

	/// Both rails carry the terminal's thumb and neither carries a track, in every status (§118).
	/// Written as a loop over the statuses because that is the decision this module made — the SHAPE
	/// of the bar does not change with status, only its opacity (§125), so a later edit that reached
	/// for iced's hover colour or gave one rail a groove would pass a single-status check.
	#[test]
	fn every_status_draws_the_terminals_thumb_over_no_track() {
		let theme = Theme::Dark;
		for status in [active(), hovered(true, true), dragged(true, true)] {
			let style = style(&theme, status);
			for rail in [style.vertical_rail, style.horizontal_rail] {
				assert!(rail.background.is_none(), "no track, in {status:?}");
				let iced::Background::Color(scroller) = rail.scroller.background else {
					panic!("a flat colour, not a gradient, in {status:?}");
				};
				assert_eq!(
					scroller,
					Color {
						a: scroller.a,
						..THUMB
					},
					"the terminal's grey, whatever the opacity, in {status:?}"
				);
				assert!(
					(rail.scroller.border.radius.top_left - radius()).abs() < f32::EPSILON,
					"fully rounded, in {status:?}"
				);
			}
		}
	}

	fn active() -> scrollable::Status {
		scrollable::Status::Active {
			is_horizontal_scrollbar_disabled: false,
			is_vertical_scrollbar_disabled: false,
		}
	}

	fn hovered(vertical: bool, horizontal: bool) -> scrollable::Status {
		scrollable::Status::Hovered {
			is_horizontal_scrollbar_hovered: horizontal,
			is_vertical_scrollbar_hovered: vertical,
			is_horizontal_scrollbar_disabled: false,
			is_vertical_scrollbar_disabled: false,
		}
	}

	fn dragged(vertical: bool, horizontal: bool) -> scrollable::Status {
		scrollable::Status::Dragged {
			is_horizontal_scrollbar_dragged: horizontal,
			is_vertical_scrollbar_dragged: vertical,
			is_horizontal_scrollbar_disabled: false,
			is_vertical_scrollbar_disabled: false,
		}
	}

	/// A touched bar is more opaque and nothing else changes (§125): same grey, same radius, no
	/// track. The alpha is what the surfaces underneath can all take — see `thumb`.
	#[test]
	fn touching_a_bar_only_changes_its_opacity() {
		let idle = thumb(Touch::Idle);
		let hover = thumb(Touch::Hovered);
		let drag = thumb(Touch::Dragged);
		for touched in [hover, drag] {
			assert!(
				(touched.r - idle.r).abs() < f32::EPSILON
					&& (touched.g - idle.g).abs() < f32::EPSILON
					&& (touched.b - idle.b).abs() < f32::EPSILON,
				"the same grey"
			);
		}
		assert!(idle.a < hover.a, "hover shows");
		assert!(hover.a < drag.a, "and a drag shows more");
		assert!(drag.a <= 1.0, "still translucent enough to blend");
	}

	/// Each rail answers for its OWN axis (§125). The editor's buffer is the only surface with both
	/// bars, so a status read for the wrong axis would brighten a bar the pointer was nowhere near —
	/// and would do it in exactly one place in the app.
	#[test]
	fn one_axis_being_touched_leaves_the_other_alone() {
		assert_eq!(touch_of(active(), Axis::Vertical), Touch::Idle);
		assert_eq!(
			touch_of(hovered(true, false), Axis::Vertical),
			Touch::Hovered
		);
		assert_eq!(
			touch_of(hovered(true, false), Axis::Horizontal),
			Touch::Idle
		);
		assert_eq!(
			touch_of(dragged(false, true), Axis::Horizontal),
			Touch::Dragged
		);
		assert_eq!(touch_of(dragged(false, true), Axis::Vertical), Touch::Idle);
	}

	/// The release `lanes` mirrors. `iced_widget`, not `iced`: the arithmetic being copied is
	/// `Scrollbar::total_bounds` in that crate, and the two version-number together but are not the
	/// same number (0.14.0 of `iced` pulls 0.14.2 of `iced_widget`).
	const MIRRORED_FROM: &str = "0.14.2";

	/// The guard on the one thing in this file that can go wrong without anybody touching it (§125).
	///
	/// `lanes` reproduces private arithmetic from `iced_widget::scrollable`. Nothing in cmote breaks
	/// if that arithmetic changes — the hand simply appears over the wrong strip of pixels, which is
	/// a bug you have to be looking for to see. The only moment it can happen is a dependency bump,
	/// so the lockfile is read here and the version pinned: the bump fails this test, the message says
	/// what to re-read, and the number is updated by whoever re-read it.
	///
	/// The lockfile rather than a `build.rs`-exported version, because cmote has no build script and
	/// adding one for a string comparison would be a build step for every compile to catch a thing
	/// that changes twice a year. It is committed (the release builds from it), so it is as
	/// authoritative here as anywhere.
	#[test]
	fn the_mirror_names_the_iced_release_it_was_read_from() {
		let lock = include_str!("../../Cargo.lock");
		let version = lock
			.split("\nname = \"iced_widget\"\n")
			.nth(1)
			.and_then(|rest| rest.lines().next())
			.and_then(|line| line.strip_prefix("version = \""))
			.and_then(|rest| rest.strip_suffix('"'))
			.expect("Cargo.lock names iced_widget and its version");
		assert_eq!(
			version, MIRRORED_FROM,
			"iced_widget moved from {MIRRORED_FROM} to {version}: re-read `Scrollbar::total_bounds` \
			 in that crate against `lanes` here, then update MIRRORED_FROM. The two asymmetries \
			 pinned by the tests above are the parts that matter (§120, §125)."
		);
	}
}

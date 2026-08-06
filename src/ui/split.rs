// ui/split.rs — the window cut into regions, and the dividers between them (PLAN §48).
//
// A window used to be one strip of tabs and whatever tab was on screen. A SPLIT cuts it into
// regions, each with a strip and a tab of its own, so two shells — or a shell and the file it is
// editing — can be watched side by side instead of swapped between. This module owns only the
// frame: the tree of regions, the seams between them, and the geometry both are laid out by. What
// goes INSIDE a region is `App`'s business; it hands one element per region in.
//
// It is built on iced's `pane_grid`, and the reason is the MEASURING rather than the drawing: the
// same `Node` that lays the regions out will also report their rectangles (`regions` below), and a
// terminal cannot exist until something tells it the exact pixel box it has to fill, because that
// is what fixes its row and column count (§9). A hand-rolled tree of rows and columns would have
// had to do that arithmetic twice — once to draw and once to measure — and the two copies would
// drift the first time a constant changed.
//
// The vocabulary here is deliberately NOT iced's. A user who splits the window *horizontally* means
// "put the new region beside this one"; iced calls that `Axis::Vertical`, naming the divider rather
// than the layout. Both readings are defensible and neither is wrong, so `Way::axis` is made the
// ONE place the two meet — everything else in cmote speaks the user's sense and never has to hold
// the inversion in its head.

use std::collections::BTreeMap;

use iced::widget::{container, pane_grid};
use iced::{Color, Element, Length, Rectangle, Size};

use crate::app::Message;

/// The gap between two regions, in logical pixels. It is not a widget — it is the frame's own
/// background showing through, which is why `GAP` below is the colour of a seam. Wide enough to
/// read as a division of the window rather than as a rendering artefact.
pub const SPACING: f32 = 4.0;

/// The smallest a region may be squeezed to by a divider drag, on both axes. Below roughly this a
/// strip has no room for one chip and a terminal has none for a line worth reading, so the drag
/// stops here instead of letting a region be pushed out of existence — from which there would be
/// no way back, since the only handle on a region is the region itself.
pub const MIN_SIZE: f32 = 240.0;

/// Extra grab room around a seam, half of it either side. `SPACING` on its own is a four-pixel
/// target, which is a fussy thing to hit with a pointer; this widens what can be GRABBED to
/// `SPACING + LEEWAY` while leaving what is DRAWN at `SPACING`.
pub const LEEWAY: f32 = 8.0;

/// What shows through the gap at rest: darker than either region's chrome, so the seam reads as a
/// break in the window rather than as part of one side of it.
const GAP: Color = Color::from_rgb8(0x14, 0x14, 0x14);

/// The seam under the pointer — it is about to become a handle, so it says so.
const DIVIDER_HOVERED: Color = Color::from_rgb8(0x5c, 0x8a, 0xc8);

/// The seam being dragged right now. The same blue, brighter: the gesture is in flight, and the
/// pointer may well have wandered off the seam it is moving.
const DIVIDER_PICKED: Color = Color::from_rgb8(0x7f, 0xa8, 0xdc);

/// Which way a split cuts the window (§48).
///
/// Named after how the two regions END UP laid out, because that is what the user picks — and NOT
/// after the divider between them, which runs the other way. `axis` carries that inversion over to
/// iced; `grown` says how much window the cut asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Way {
	/// Side by side. The fresh region takes the right half, and the window grows rightwards.
	Horizontal,
	/// Stacked. The fresh region takes the bottom half, and the window grows downwards.
	Vertical,
}

impl Way {
	/// This cut as `pane_grid` names it — the axis the DIVIDER lies along, which is the opposite of
	/// the one the regions are laid out on. Two regions side by side are parted by a vertical seam.
	pub fn axis(self) -> pane_grid::Axis {
		match self {
			Way::Horizontal => pane_grid::Axis::Vertical,
			Way::Vertical => pane_grid::Axis::Horizontal,
		}
	}

	/// The window size this cut wants: the current one doubled along the way it cuts (§48).
	///
	/// A split does not divide the room the user already had — it asks for as much again, so the
	/// region being split keeps the size it had and the new one is its equal. Growing rather than
	/// halving is what makes a split cheap: the shell already on screen does not reflow to half a
	/// window, so nothing it has drawn is disturbed. The caller clamps this to the monitor before
	/// asking for it; past the screen's edge there is no more room to be had.
	pub fn grown(self, from: Size) -> Size {
		match self {
			Way::Horizontal => Size::new(from.width * 2.0, from.height),
			Way::Vertical => Size::new(from.width, from.height * 2.0),
		}
	}
}

/// The pixel rectangle each region occupies, measured exactly as `frame` lays them out (§48).
///
/// This is the whole reason `pane_grid` is under this feature rather than a hand-built tree of rows
/// and columns: the widget's own layout node answers the question, so a region's box is never a
/// second guess at what was drawn. `App` calls this on every window resize and after every split,
/// and hands each region's on-screen tab the size it comes back with — which is what lets the
/// terminal in there pick a row and column count that fits (§9).
///
/// The two constants MUST be the same ones `frame` passes to the widget, which is why they are
/// constants and not arguments: a divider drawn at one spacing and measured at another would leave
/// every grid a column short of its region, or a column over it.
pub fn regions<T>(
	state: &pane_grid::State<T>,
	window: Size,
) -> BTreeMap<pane_grid::Pane, Rectangle> {
	state.layout().pane_regions(SPACING, MIN_SIZE, window)
}

/// Build the frame from one element per region (§48).
///
/// `view` is called once per region, in `Pane` order, and returns everything that region shows —
/// its own tab strip and the tab beneath it. The frame adds the seams, the resize gesture, and the
/// press that moves the keyboard from one region to another.
///
/// A press is reported but never swallowed on the way in: `pane_grid` updates the region's own
/// widgets BEFORE it looks at the event itself, so the click that focuses a region is the same
/// click that lands in the terminal there. That ordering is what lets a first click into an
/// unfocused region do the obvious thing — put the caret where it was aimed — rather than being
/// spent on the focus alone.
pub fn frame<'a, T>(
	state: &'a pane_grid::State<T>,
	view: impl Fn(pane_grid::Pane, &'a T) -> Element<'a, Message>,
) -> Element<'a, Message> {
	// The `bool` iced offers is whether this region is MAXIMIZED, a mode cmote does not use: a
	// region is only ever the size the dividers give it, so there is no maximized state to draw
	// differently and the flag is dropped here.
	let grid = pane_grid(state, |pane, region, _maximized| {
		pane_grid::Content::new(view(pane, region))
	})
	.width(Length::Fill)
	.height(Length::Fill)
	.spacing(SPACING)
	.min_size(MIN_SIZE)
	// A left press anywhere in a region hands it the keyboard (§48). Every region is visible at
	// once, so "which one am I typing into" cannot be answered by what is on screen — it is
	// answered by the last one clicked, and the strip's own tint says which that is.
	.on_click(Message::SplitFocused)
	// Dragging a seam re-shares the room between the two regions either side of it. The ratio is
	// all that is stored; the pixels are worked out from it against the window's current size, so
	// a share survives a window resize instead of becoming a stale pixel count.
	.on_resize(LEEWAY, |event| Message::SplitResized {
		split: event.split,
		ratio: event.ratio,
	})
	.style(|_theme| pane_grid::Style {
		// The drop highlight belongs to `on_drag`, which cmote does not enable — a region is
		// defined by the tabs in it, and dragging one region onto another would have to say what
		// happens to both strips. Its style still has to be given a value, so it is given an
		// invisible one rather than a colour that could never appear.
		hovered_region: pane_grid::Highlight {
			background: Color::TRANSPARENT.into(),
			border: iced::Border::default(),
		},
		picked_split: pane_grid::Line {
			color: DIVIDER_PICKED,
			width: SPACING,
		},
		hovered_split: pane_grid::Line {
			color: DIVIDER_HOVERED,
			width: SPACING,
		},
	});

	// The seams are gaps, so something has to be behind them. Without this the window's own
	// background shows through and a split reads as a crack rather than as a division.
	container(grid)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(|_theme| container::Style {
			background: Some(GAP.into()),
			..container::Style::default()
		})
		.into()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_horizontal_cut_is_parted_by_a_vertical_seam() {
		// The one inversion this module exists to contain: regions laid out side by side are
		// divided by an upright line, which is what iced names the split after (§48).
		assert_eq!(Way::Horizontal.axis(), pane_grid::Axis::Vertical);
		assert_eq!(Way::Vertical.axis(), pane_grid::Axis::Horizontal);
	}

	#[test]
	fn a_cut_doubles_the_window_along_its_own_way_only() {
		let window = Size::new(1000.0, 600.0);
		// Growing rightwards leaves the height alone, and vice versa — so the region being split
		// keeps the size it had and nothing already drawn in it reflows (§48).
		assert_eq!(Way::Horizontal.grown(window), Size::new(2000.0, 600.0));
		assert_eq!(Way::Vertical.grown(window), Size::new(1000.0, 1200.0));
	}

	#[test]
	fn one_region_owns_the_whole_window() {
		let (state, _first) = pane_grid::State::new(());
		let window = Size::new(1000.0, 600.0);
		let regions = regions(&state, window);
		assert_eq!(regions.len(), 1);
		let only = regions.values().next().expect("one region");
		assert_eq!((only.width, only.height), (1000.0, 600.0));
	}

	#[test]
	fn a_horizontal_cut_shares_the_width_and_leaves_a_seam_between() {
		let (mut state, first) = pane_grid::State::new(());
		let (second, _split) = state
			.split(Way::Horizontal.axis(), first, ())
			.expect("the first region can always be split");
		let window = Size::new(1000.0, 600.0);
		let regions = regions(&state, window);
		let left = regions[&first];
		let right = regions[&second];
		// Full height each, and the widths account for the whole window with exactly one seam's
		// worth of gap unaccounted for — which is the gap the frame's background shows through.
		assert_eq!((left.height, right.height), (600.0, 600.0));
		assert!((left.width + right.width + SPACING - 1000.0).abs() < 0.5);
		// The new region is the RIGHT one: `pane_grid` puts a split's second child after the
		// first, which is what makes "split horizontally" grow rightwards (§48).
		assert!(right.x > left.x);
	}

	#[test]
	fn a_vertical_cut_shares_the_height_instead() {
		let (mut state, first) = pane_grid::State::new(());
		let (second, _split) = state
			.split(Way::Vertical.axis(), first, ())
			.expect("the first region can always be split");
		let regions = regions(&state, Size::new(1000.0, 600.0));
		let top = regions[&first];
		let bottom = regions[&second];
		assert_eq!((top.width, bottom.width), (1000.0, 1000.0));
		assert!((top.height + bottom.height + SPACING - 600.0).abs() < 0.5);
		assert!(bottom.y > top.y);
	}

	#[test]
	fn a_divider_cannot_squeeze_a_region_below_the_minimum() {
		let (mut state, first) = pane_grid::State::new(());
		let (second, split) = state
			.split(Way::Horizontal.axis(), first, ())
			.expect("the first region can always be split");
		// Drag the seam as far left as it will go. The measured regions must still both be usable:
		// a region dragged to nothing could never be dragged back, since the only handle on it is
		// the region itself (§48).
		state.resize(split, 0.0);
		let regions = regions(&state, Size::new(1000.0, 600.0));
		assert!(regions[&first].width >= MIN_SIZE);
		assert!(regions[&second].width >= MIN_SIZE);
	}
}

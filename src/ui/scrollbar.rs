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
use iced::{Border, Color, Theme};

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
/// The look does not change with `status`, and that is the point rather than an omission: the
/// terminal's bar is one colour whether or not it is being dragged, so a pane's that brightened under
/// the pointer would no longer be the same bar. Adding hover and drag feedback is a fine idea and it
/// belongs to BOTH surfaces at once, which means it starts here.
pub fn style(theme: &Theme, status: scrollable::Status) -> scrollable::Style {
	let rail = scrollable::Rail {
		// No track. The terminal draws the thumb over whatever is behind it and nothing else, which is
		// what keeps a bar this thin from reading as a groove cut down the edge of the pane.
		background: None,
		border: Border::default(),
		scroller: scrollable::Scroller {
			background: THUMB.into(),
			border: Border::default().rounded(radius()),
		},
	};
	scrollable::Style {
		vertical_rail: rail,
		horizontal_rail: rail,
		..scrollable::default(theme, status)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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

	/// Both rails carry the terminal's thumb and neither carries a track, in every status (§118).
	/// Written as a loop over the statuses because "the look does not change with status" is the
	/// decision this module made, so it is the thing to pin — a later edit that reaches for iced's
	/// hover colour on one rail and not the other would pass a single-status check.
	#[test]
	fn every_status_draws_the_terminals_thumb_over_no_track() {
		let theme = Theme::Dark;
		let statuses = [
			scrollable::Status::Active {
				is_horizontal_scrollbar_disabled: false,
				is_vertical_scrollbar_disabled: false,
			},
			scrollable::Status::Hovered {
				is_horizontal_scrollbar_hovered: true,
				is_vertical_scrollbar_hovered: true,
				is_horizontal_scrollbar_disabled: false,
				is_vertical_scrollbar_disabled: false,
			},
			scrollable::Status::Dragged {
				is_horizontal_scrollbar_dragged: true,
				is_vertical_scrollbar_dragged: true,
				is_horizontal_scrollbar_disabled: false,
				is_vertical_scrollbar_disabled: false,
			},
		];
		for status in statuses {
			let style = style(&theme, status);
			for rail in [style.vertical_rail, style.horizontal_rail] {
				assert!(rail.background.is_none(), "no track, in {status:?}");
				assert_eq!(
					rail.scroller.background,
					THUMB.into(),
					"the terminal's thumb, in {status:?}"
				);
				assert!(
					(rail.scroller.border.radius.top_left - radius()).abs() < f32::EPSILON,
					"fully rounded, in {status:?}"
				);
			}
		}
	}
}

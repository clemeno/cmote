// ui/terminal.rs — render the vt100 `Screen` grid as iced widgets (PLAN §9-§10).
//
// Stub for now: the walking skeleton has no live session to draw. Once the SSH
// task and the `term` module are wired, this renders the parser's cell grid in a
// monospace font, one styled span per run of same-attribute cells.

use iced::Element;
use iced::widget::text;

use crate::app::Message;

/// Render the terminal screen. Placeholder until the vt100 grid is available.
pub fn view() -> Element<'static, Message> {
	// ponytail: literal placeholder; replaced by the real grid render in §9.
	text("terminal (not yet wired)").into()
}

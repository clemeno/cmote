// ui/mod.rs — view helpers (PLAN §10).
//
// Views are pure functions from state to an iced `Element`. Keeping them out of
// app.rs stops that file from growing without bound and groups the widget code
// by screen. Each submodule owns one screen's layout.

pub mod connect; // the connection form
pub mod terminal; // the live shell grid

use iced::Element;
use iced::widget::{button, column, row, text};

use crate::app::Message;

/// The error screen (§10): a generic message plus a Back button to the form.
/// Detail is logged, not shown, so nothing sensitive leaks to the UI (§12).
pub fn error_view(message: &str) -> Element<'_, Message> {
	column![
		text("Connection failed").size(20),
		text(message),
		button("Back").on_press(Message::BackPressed),
	]
	.spacing(12)
	.padding(20)
	.into()
}

/// The first-contact host-key prompt (§8): show the fingerprint and make the
/// user explicitly accept or reject. There is intentionally no "always trust"
/// shortcut — accepting pins this exact key, and any later change is refused.
pub fn host_key_view(fingerprint: &str) -> Element<'_, Message> {
	column![
		text("Unknown host key").size(20),
		text("This is the first connection to this server. Verify the fingerprint below matches the server you expect before accepting."),
		text(fingerprint).size(16),
		row![
			button("Accept").on_press(Message::AcceptHostKey),
			button("Reject").on_press(Message::RejectHostKey),
		]
		.spacing(10),
	]
	.spacing(12)
	.padding(20)
	.max_width(560)
	.into()
}

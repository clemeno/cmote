// ui/snackbar.rs — the copy-confirmation toast (§10).
//
// A small, self-dismissing banner shown over the terminal screen right after a clipboard
// copy, so the user gets a "yes, that worked" without a permanent status line. It is pure
// chrome: `app` owns whether one is showing and for how long (a stored `Instant` polled by
// the window's frame clock — this build's iced executor is the thread-pool backend, which
// has no timer subscription, so `window::frames()` drives the dwell instead). This file
// only turns the message into a floating card.
//
// It rides as the top layer of a `stack` over the terminal view. A plain container ignores
// pointer events, so — like the context-menu overlays (§10) — it never blocks a click on
// the shell or the panels underneath it.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{container, text};
use iced::{Color, Element, Length, Padding, Shadow, Vector};

use crate::app::Message;

/// The toast's own palette: an elevated dark surface with light text, distinct from the
/// panels so it reads as floating above them rather than as part of one.
const CARD_BG: Color = Color::from_rgb8(0x2e, 0x2e, 0x2e);
const CARD_FG: Color = Color::from_rgb8(0xea, 0xea, 0xea);
const CARD_BORDER: Color = Color::from_rgb8(0x4a, 0x4a, 0x4a);

/// The card's metrics: text size, inner padding ([vertical, horizontal]), and how far off
/// the window's bottom edge it floats.
const TEXT_SIZE: f32 = 13.0;
const CARD_PADDING: [f32; 2] = [8.0, 16.0];
const BOTTOM_MARGIN: f32 = 28.0;

/// The floating confirmation card for `message`, aligned to the bottom-centre of the
/// window. Returned as a full-window layer so the caller can drop it straight onto the
/// terminal `stack`; the outer fill container is transparent and event-transparent, so
/// only the small card is drawn and nothing underneath it is blocked.
pub fn view(message: &str) -> Element<'_, Message> {
	let card = container(text(message.to_owned()).size(TEXT_SIZE).color(CARD_FG))
		.padding(Padding::from(CARD_PADDING))
		.style(|_theme| container::Style {
			background: Some(CARD_BG.into()),
			border: iced::Border {
				width: 1.0,
				radius: 6.0.into(),
				color: CARD_BORDER,
			},
			// A soft drop shadow is what sells the "floating above the panels" read; the
			// panels themselves are flat, so the card needs the lift to stand apart.
			shadow: Shadow {
				color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
				offset: Vector::new(0.0, 2.0),
				blur_radius: 8.0,
			},
			..container::Style::default()
		});

	container(card)
		.width(Length::Fill)
		.height(Length::Fill)
		.align_x(Horizontal::Center)
		.align_y(Vertical::Bottom)
		.padding(Padding {
			top: 0.0,
			right: 0.0,
			bottom: BOTTOM_MARGIN,
			left: 0.0,
		})
		.into()
}

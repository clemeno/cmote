// ui/menu.rs — the shared right-click context-menu chrome (PLAN §10).
//
// Three screens grew their own menu — the terminal grid's Copy/Paste (§10), the home
// list's Open/Rename/Delete (§14), and the folder tree's seven items (§18) — and they
// drifted into three looks: raised buttons here, flat theme buttons there, different
// paddings, different widths. This module is the one definition, the same way
// `ui::dialog` is the one definition of a modal card: a **panel** of full-width **items**
// on a dark floating surface, plus the **dismiss layer** that catches a click-away.
//
// Positioning stays with the caller, because the three anchor differently (the pointer,
// a row index, the panel's right edge) and only the caller knows its own geometry.

use iced::alignment::Vertical;
use iced::widget::{button, column, container, mouse_area, row, text};
use iced::{Background, Border, Color, Element, Length, Padding};

use crate::app::Message;

/// Every context menu is this wide. One width is most of what "consistent" means here,
/// and it is set by the longest item any of them carries ("Copy relative path", §18)
/// rather than per-menu guesswork.
pub const WIDTH: f32 = 180.0;

/// The floating surface, its hover highlight, and the item text — light on dark. Fixed
/// rather than themed: like the terminal chrome, each of these sets background *and*
/// foreground together, so the pair stays readable whichever way the system theme goes
/// (the trap §14 documents is a surface that themes only one of the two).
const BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);
const HOVER_BG: Color = Color::from_rgb8(0x4a, 0x4a, 0x4a);
const FG: Color = Color::from_rgb8(0xe0, 0xe0, 0xe0);

/// A disabled item's text. Dimmed, so an item that cannot be used says so — with a
/// transparent button that is the only signal there is.
const DISABLED_FG: Color = Color::from_rgb8(0x80, 0x80, 0x80);

const TEXT_SIZE: f32 = 13.0;
const CORNER_RADIUS: f32 = 4.0;

/// The tick column reserved at the head of a `check_item`, wide enough for the ✓ glyph. Every row
/// in a checkable menu leaves it, ticked or not, so the labels line up in one column whether or
/// not they are chosen (§19).
const CHECK_WIDTH: f32 = 16.0;

/// The hairline drawn by `separator`, dividing one group of items from the next (§19). Dim enough
/// to read as a rule rather than a disabled item.
const SEPARATOR_FG: Color = Color::from_rgb8(0x55, 0x55, 0x55);

/// One menu item. `on_press` of `None` renders it disabled — iced does that for a button
/// with no message — and dims its label so the difference is visible.
pub fn item(label: String, on_press: Option<Message>) -> Element<'static, Message> {
	button(text(label).size(TEXT_SIZE))
		.width(Length::Fill)
		.padding(Padding::from([2.0, 6.0]))
		.style(|_theme, status| button::Style {
			background: match status {
				button::Status::Hovered | button::Status::Pressed => {
					Some(Background::Color(HOVER_BG))
				}
				_ => None,
			},
			text_color: if matches!(status, button::Status::Disabled) {
				DISABLED_FG
			} else {
				FG
			},
			..button::Style::default()
		})
		.on_press_maybe(on_press)
		.into()
}

/// A menu item that carries a tick — one of a radio-style group where the chosen row is marked
/// (§19). Unlike `item` it is always live: clicking the ticked row is itself a meaningful action
/// (it toggles the sort off), so there is no disabled state. The tick sits in a fixed-width column
/// so every label in the group starts at the same x, ticked or not.
pub fn check_item(label: String, checked: bool, on_press: Message) -> Element<'static, Message> {
	let mark = if checked { "\u{2713}" } else { "" }; // ✓
	button(
		row![
			container(text(mark).size(TEXT_SIZE)).width(Length::Fixed(CHECK_WIDTH)),
			text(label).size(TEXT_SIZE),
		]
		.align_y(Vertical::Center),
	)
	.width(Length::Fill)
	.padding(Padding::from([2.0, 6.0]))
	.style(|_theme, status| button::Style {
		background: match status {
			button::Status::Hovered | button::Status::Pressed => Some(Background::Color(HOVER_BG)),
			_ => None,
		},
		text_color: FG,
		..button::Style::default()
	})
	.on_press(on_press)
	.into()
}

/// A hairline between two groups of items in one menu (§19). A padded 1px rule rather than a
/// bare line, so it sits clear of the rows above and below it instead of crowding them.
pub fn separator() -> Element<'static, Message> {
	container(
		container(text(""))
			.width(Length::Fill)
			.height(Length::Fixed(1.0))
			.style(|_theme| container::Style {
				background: Some(SEPARATOR_FG.into()),
				..container::Style::default()
			}),
	)
	.padding(Padding::from([3.0, 4.0]))
	.into()
}

/// The menu surface holding the items. Fixed width, rounded like the dialog card, and
/// clipped so a hovered item's fill respects the corners.
pub fn panel<'a>(items: Vec<Element<'a, Message>>) -> Element<'a, Message> {
	container(column(items).spacing(1))
		.width(Length::Fixed(WIDTH))
		.padding(4)
		.clip(true)
		.style(|_theme| container::Style {
			background: Some(BG.into()),
			border: Border {
				radius: CORNER_RADIUS.into(),
				..Border::default()
			},
			..container::Style::default()
		})
		.into()
}

/// The full-window invisible layer that sits *under* an open menu: any click that misses
/// the menu lands here and emits `on_dismiss`. A right-press dismisses too, so a second
/// right-click never stacks two menus.
pub fn dismiss_layer(on_dismiss: Message) -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_press(on_dismiss.clone())
		.on_right_press(on_dismiss)
		.into()
}

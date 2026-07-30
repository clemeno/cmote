// ui/tabs.rs — the tab strip across the top of the window (PLAN §26).
//
// One chip per open session, plus a trailing "+" that opens a new tab. Mouse-only (§26): a left
// click on a chip selects it, the chip's "×" closes it (a live session confirms first, in `App`),
// and "+" opens a fresh home tab. `App` draws this strip above the active tab's own view and
// swaps which tab it renders beneath when the active one changes.
//
// The strip has a fixed height (`STRIP_HEIGHT`), which the terminal below it must account for: it
// sits in a sub-region of the window that is exactly that much shorter. `App` hands each tab a
// window size already reduced by `STRIP_HEIGHT`, so every layout and pointer coordinate inside a
// tab is measured against the region it actually occupies.

use iced::alignment::Vertical;
use iced::widget::{button, container, mouse_area, row, text};
use iced::{Border, Color, Element, Length};

use crate::app::Message;

/// The strip's fixed height in pixels — the chip height plus the bar's padding top and bottom.
/// `app` subtracts this from the window height it gives each tab, so the terminal grid fits the
/// space left below the strip rather than overrunning it by a row.
pub const STRIP_HEIGHT: f32 = 38.0;

// Strip colours: a dark bar, a lighter fill for the active chip so the current session stands
// apart, and a muted vs bright foreground for the inactive vs active labels.
const BAR_BG: Color = Color::from_rgb8(0x22, 0x22, 0x22);
const ACTIVE_BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);
const INACTIVE_FG: Color = Color::from_rgb8(0xa0, 0xa0, 0xa0);
const ACTIVE_FG: Color = Color::from_rgb8(0xf0, 0xf0, 0xf0);

/// Each chip's fixed height, so the bar's own height (`STRIP_HEIGHT`) is predictable.
const CHIP_HEIGHT: f32 = 30.0;

/// The longest label a chip shows before the middle is elided (§22), so one long endpoint cannot
/// push the "+" off the strip.
const MAX_LABEL_CHARS: usize = 24;

/// One chip's data: the owning tab's id (for the close message), the label to show, and whether
/// it is the active tab (which tints its fill and brightens its text).
pub struct Chip {
	pub id: u64,
	pub label: String,
	pub active: bool,
}

/// Build the tab strip from the chips, in strip order. Returns an owned (`'static`) element —
/// every label is cloned into its widget, so nothing is borrowed from the caller.
pub fn strip(chips: &[Chip]) -> Element<'static, Message> {
	let mut items: Vec<Element<'static, Message>> = Vec::with_capacity(chips.len() + 1);
	for (index, chip) in chips.iter().enumerate() {
		items.push(chip_view(index, chip));
	}
	// The trailing "+" opens a new tab on the home screen.
	items.push(
		button(text("+").size(16).color(ACTIVE_FG))
			.on_press(Message::TabNew)
			.style(|_theme, _status| button::Style {
				background: None,
				text_color: ACTIVE_FG,
				..button::Style::default()
			})
			.into(),
	);

	container(row(items).spacing(2).align_y(Vertical::Center))
		.width(Length::Fill)
		.height(Length::Fixed(STRIP_HEIGHT))
		.padding(4)
		.style(|_theme| container::Style {
			background: Some(BAR_BG.into()),
			..container::Style::default()
		})
		.into()
}

/// One chip: the (elided) label with a "×" beside it, tinted when active. The whole chip is a
/// `mouse_area` whose left press selects the tab; the "×" is a nested button that captures its own
/// press (children handle events first), so closing does not also select.
fn chip_view(index: usize, chip: &Chip) -> Element<'static, Message> {
	let label = crate::ui::elide_middle(&chip.label, MAX_LABEL_CHARS);
	let fg = if chip.active { ACTIVE_FG } else { INACTIVE_FG };

	let name = text(label).size(13).color(fg);
	let close = button(text("×").size(14).color(fg))
		.padding([0.0, 4.0])
		.on_press(Message::TabCloseRequested(chip.id))
		.style(move |_theme, _status| button::Style {
			background: None,
			text_color: fg,
			..button::Style::default()
		});

	let active = chip.active;
	let cell = container(row![name, close].spacing(6).align_y(Vertical::Center))
		.height(Length::Fixed(CHIP_HEIGHT))
		.padding([0.0, 8.0])
		.align_y(Vertical::Center)
		.style(move |_theme| {
			let mut style = container::Style {
				border: Border {
					radius: 4.0.into(),
					..Border::default()
				},
				..container::Style::default()
			};
			if active {
				style.background = Some(ACTIVE_BG.into());
			}
			style
		});

	mouse_area(cell)
		.on_press(Message::TabSelected(index))
		.into()
}

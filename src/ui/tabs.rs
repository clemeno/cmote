// ui/tabs.rs — the tab strip across the top of the window (PLAN §26).
//
// One chip per open session, plus a trailing "+" that opens a new tab. Mouse-only (§26): a left
// click on a chip selects it, the chip's "×" closes it (a live session confirms first, in `App`),
// and "+" opens a fresh home tab. `App` draws this strip above the active tab's own view and
// swaps which tab it renders beneath when the active one changes.
//
// The same press that selects also GRABS the chip (§38), so a tab can be dragged along the strip to
// a new position with no separate handle. The gesture is reported entirely through per-chip pointer
// events — `on_press` grabs, `on_enter` names the slot under the pointer, `on_release` drops — so it
// needs no pixel arithmetic here and no knowledge of how wide any chip laid out. The strip is
// reordered once, on the drop; iced does not expose a widget's laid-out bounds, so shuffling the
// chips live under the pointer could ping-pong between two slots when their widths differ. `App`
// holds the whole gesture's state (which tab is grabbed, which slot it is over) and hands back the
// one piece the strip has to draw: a mark on the chip that would receive the drop.
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

// The command-status dot's colours (§34): amber while a command runs, green when the last one
// exited 0, red when it failed. Muted so the dot reads as a status light, not an alarm.
const STATUS_RUNNING: Color = Color::from_rgb8(0xd7, 0xa8, 0x3a);
const STATUS_OK: Color = Color::from_rgb8(0x5c, 0xb8, 0x5c);
const STATUS_FAILED: Color = Color::from_rgb8(0xe0, 0x6c, 0x6c);

/// The command-status dot a chip shows (§34), from the tab's OSC 133 shell-integration marks. A
/// tab with no live shell, or a shell that announces no integration, carries `None` — so most
/// chips show no dot at all and the strip stays quiet until a command actually runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
	/// A command is executing right now.
	Running,
	/// The last command finished and exited 0.
	Ok,
	/// The last command finished and exited non-zero.
	Failed,
}

impl Status {
	/// The dot's colour for this status.
	fn color(self) -> Color {
		match self {
			Status::Running => STATUS_RUNNING,
			Status::Ok => STATUS_OK,
			Status::Failed => STATUS_FAILED,
		}
	}
}

/// The drop mark's colour (§38): the border drawn round the chip a dragged tab would land on. A
/// muted blue, the same family as the selection fills elsewhere, so it reads as "here" rather than
/// as a warning.
const DROP_BORDER: Color = Color::from_rgb8(0x5c, 0x8a, 0xc8);

/// The longest label a chip shows before the middle is elided (§22), so one long endpoint cannot
/// push the "+" off the strip.
const MAX_LABEL_CHARS: usize = 48;

/// One chip's data: the owning tab's id (for the close message), the label to show, whether it is
/// the active tab (which tints its fill and brightens its text), its command-status dot (§34), and
/// whether a drag in flight would drop onto this chip's slot (§38), which outlines it.
pub struct Chip {
	pub id: u64,
	pub label: String,
	pub active: bool,
	pub status: Option<Status>,
	pub drop_target: bool,
}

/// Build the tab strip from the chips, in strip order. Returns an owned (`'static`) element —
/// every label is cloned into its widget, so nothing is borrowed from the caller.
///
/// `dragging` says a tab is being moved right now (§38): it only changes the cursor, which becomes
/// the "grabbing" hand over every chip, so the gesture reads as in progress wherever the pointer has
/// got to. The bar itself catches the release (a drop on the gap between chips still counts) and the
/// pointer leaving the strip, which abandons the move.
pub fn strip(chips: &[Chip], dragging: bool) -> Element<'static, Message> {
	let mut items: Vec<Element<'static, Message>> = Vec::with_capacity(chips.len() + 1);
	for (index, chip) in chips.iter().enumerate() {
		items.push(chip_view(index, chip, dragging));
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

	let bar = container(row(items).spacing(2).align_y(Vertical::Center))
		.width(Length::Fill)
		.height(Length::Fixed(STRIP_HEIGHT))
		.padding(4)
		.style(|_theme| container::Style {
			background: Some(BAR_BG.into()),
			..container::Style::default()
		});

	// The bar backs the chips up on both ends of a drag (§38). Its release catches a drop that lands
	// on the padding or the gap between two chips — the last chip hovered still wins the slot — and
	// its exit ends a gesture that wanders off the strip, so a drag can always be called off by
	// moving away. Neither event is captured by the chips above (iced's `mouse_area` only captures
	// presses), so both layers see them and the chip's own handler still fires first.
	mouse_area(bar)
		.on_release(Message::TabDropped)
		.on_exit(Message::TabDragCancelled)
		.into()
}

/// One chip: the (elided) label with a "×" beside it, tinted when active. The whole chip is a
/// `mouse_area` whose left press selects the tab AND grabs it for a drag (§38); the "×" is a nested
/// button that captures its own press (children handle events first), so closing does not also
/// select. The pointer entering the chip names it as the drop slot, and a release over it drops.
fn chip_view(index: usize, chip: &Chip, dragging: bool) -> Element<'static, Message> {
	let label = crate::ui::elide_middle(&chip.label, MAX_LABEL_CHARS);
	let fg = if chip.active { ACTIVE_FG } else { INACTIVE_FG };

	let name = text(label).size(13).color(fg);
	// A leading status dot when the tab's shell reports one (§34): a running command, or how the
	// last one exited. Its own colour whether the chip is active or not — the status matters the
	// same on a background tab, which is the whole point of showing it per tab.
	let mut contents = row![].spacing(6).align_y(Vertical::Center);
	if let Some(status) = chip.status {
		contents = contents.push(text("●").size(10).color(status.color()));
	}
	contents = contents.push(name);
	let close = button(text("×").size(14).color(fg))
		.padding([0.0, 4.0])
		.on_press(Message::TabCloseRequested(chip.id))
		.style(move |_theme, _status| button::Style {
			background: None,
			text_color: fg,
			..button::Style::default()
		});

	contents = contents.push(close);
	let active = chip.active;
	let drop_target = chip.drop_target;
	let cell = container(contents)
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
			// The drop mark (§38): an outline on the chip a release would drop onto. Drawn over the
			// active tint rather than instead of it, so the strip never stops saying which tab is on
			// screen while a tab is being moved.
			if drop_target {
				style.border.color = DROP_BORDER;
				style.border.width = 1.0;
			}
			style
		});

	// The whole chip is the drag handle (§38). `on_press` selects and grabs; `on_release` drops. The
	// cursor advertises the gesture: an open hand at rest, a closed one while a tab is in flight.
	let area = mouse_area(cell)
		.on_press(Message::TabSelected(index))
		.on_release(Message::TabDropped)
		.interaction(if dragging {
			iced::mouse::Interaction::Grabbing
		} else {
			iced::mouse::Interaction::Grab
		});

	// The hover report — which slot the pointer is over — is wired up ONLY while a tab is actually
	// being dragged. `App` would ignore it at rest anyway, but not asking for it means moving the
	// pointer across the strip publishes no messages at all when there is nothing to move.
	if dragging {
		area.on_enter(Message::TabDraggedOver(index)).into()
	} else {
		area.into()
	}
}

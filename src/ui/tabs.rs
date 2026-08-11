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
// A chip also carries a right-click menu (§52), which sends the tab to another AREA of the window —
// moving it there, or opening a second copy of it there. It is the keyboard-free counterpart to the
// drag above: a drag reorders a tab within one strip and cannot cross a seam (the other strip's
// chips report nothing to a gesture that began here), so crossing one is what the menu is for.
//
// The strip has a fixed height (`STRIP_HEIGHT`), which the terminal below it must account for: it
// sits in a sub-region of the window that is exactly that much shorter. `App` hands each tab a
// window size already reduced by `STRIP_HEIGHT`, so every layout and pointer coordinate inside a
// tab is measured against the region it actually occupies.
//
// Since §48 the window can hold MORE THAN ONE of these strips: a split gives each region its own,
// and the region a strip belongs to is not in this file at all. `App` maps every message a region's
// widgets raise so it names its own region, which is why nothing here takes a region as an
// argument — a chip press means "this strip's tab", and the wrapper says whose strip that was. The
// one thing a strip does have to be told is whether its region holds the KEYBOARD, since with
// several strips on screen "which one am I typing into" is no longer answerable from the layout.

use iced::alignment::Vertical;
use iced::widget::{button, container, mouse_area, progress_bar, row, space, stack, text};
use iced::{Border, Color, Element, Font, Length, Point, Size};

use crate::app::Message;
use crate::ui::split;

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

/// The bar's fill in a region that does NOT hold the keyboard (§48). Darker than `BAR_BG`, not
/// lighter: the region being typed into should be the one that looks lit, and with one region — the
/// only case before §48 — this colour never appears at all, so an unsplit window is unchanged.
const BAR_UNFOCUSED_BG: Color = Color::from_rgb8(0x18, 0x18, 0x18);

/// The Material Icons face, bundled in the binary (`app::ICON_FONT`) and already loaded by the
/// file panes (§19). Named again here rather than shared so the strip's chrome carries the strip's
/// own palette instead of inheriting a panel's.
const ICON_FONT: Font = Font::with_name("Material Icons");

/// The split buttons' glyphs (§48), and a second inversion to keep straight: Material Icons names
/// its two split icons after the DIVIDING LINE, so `vertical_split` (a box parted by an upright
/// line) is the picture of two regions SIDE BY SIDE. The names below say what the button does, and
/// the codepoints are the pictures that show it.
const SPLIT_BESIDE_GLYPH: char = '\u{e949}';
const SPLIT_BELOW_GLYPH: char = '\u{e947}';

/// The split buttons' glyph size — a touch under the chip height so the two read as toolbar
/// controls sitting in the bar rather than as chips of their own.
const SPLIT_ICON_SIZE: f32 = 17.0;

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

/// The progress bar's thickness and its unfilled track (§54). Three pixels along the bottom edge of
/// the chip: enough to read across the strip at a glance, thin enough that a chip with no progress
/// to report looks exactly as it did before — and one is drawn only when a command actually reports.
const PROGRESS_GIRTH: f32 = 3.0;
const PROGRESS_TRACK: Color = Color::from_rgb8(0x2e, 0x2e, 0x2e);

/// The paused/attention colour (§54). A muted blue rather than the amber of work in flight or the
/// red of failure: `st = 4` means the command is waiting for something, which is neither.
const PROGRESS_PAUSED: Color = Color::from_rgb8(0x5c, 0x8a, 0xc8);

/// The branch pill (§55): the value a remote shell announced as its git branch, drawn on the chip.
///
/// Its own fill and its own dimmer ink, deliberately. This value is chosen by the REMOTE and drawn in
/// chrome cmote owns, so it must not be able to pass for the endpoint label beside it — the label is
/// how the user knows which machine they are typing into. A pill is the cheapest honest way to say
/// "the remote said this": it reads as an annotation on the tab rather than as part of its name.
const BRANCH_BG: Color = Color::from_rgb8(0x2f, 0x3a, 0x2f);
const BRANCH_FG: Color = Color::from_rgb8(0x9d, 0xc0, 0x9d);
const BRANCH_TEXT_SIZE: f32 = 11.0;

/// The drop mark's colour (§38): the border drawn round the chip a dragged tab would land on. A
/// muted blue, the same family as the selection fills elsewhere, so it reads as "here" rather than
/// as a warning.
const DROP_BORDER: Color = Color::from_rgb8(0x5c, 0x8a, 0xc8);

/// The longest label a chip shows before the middle is elided (§22), so one long endpoint cannot
/// push the "+" off the strip.
const MAX_LABEL_CHARS: usize = 48;

/// One chip's data: the owning tab's id (for the close message), the label to show, whether it is
/// the active tab (which tints its fill and brightens its text), its command-status dot (§34), what
/// the remote command reports about its progress (§54), and whether a drag in flight would drop onto
/// this chip's slot (§38), which outlines it.
pub struct Chip {
	pub id: u64,
	pub label: String,
	pub active: bool,
	pub status: Option<Status>,
	pub progress: crate::term::progress::Progress,
	/// The branch the remote announced (§55), if it announced one. Already sanitised and capped by
	/// `term::iterm` — this file draws it and does not police it, but note that it IS remote-chosen
	/// text, which is why it gets its own pill rather than joining the label.
	pub branch: Option<String>,
	pub drop_target: bool,
}

/// The bar along a chip's bottom edge when the tab's remote command reports progress (§54), or
/// `None` when it reports nothing — which is most tabs, most of the time, so most chips are built
/// exactly as they were before this existed.
///
/// `Indeterminate` is drawn as a FULL-width bar in a dimmed amber rather than as an animated pulse.
/// Animating it would mean waking the whole window on a timer to move a few pixels on a strip, which
/// is a poor trade for a state that already reads correctly as "something is happening and nobody
/// will say how much". The dimming is what distinguishes it from a genuine 100%.
fn progress_bar_for(
	progress: crate::term::progress::Progress,
) -> Option<Element<'static, Message>> {
	use crate::term::progress::Progress;

	let (fraction, fill) = match progress {
		Progress::None => return None,
		// Full width, dimmed: working, extent unknown.
		Progress::Indeterminate => (
			1.0,
			Color {
				a: 0.45,
				..STATUS_RUNNING
			},
		),
		Progress::Working(share) => (f32::from(share) / 100.0, STATUS_RUNNING),
		Progress::Failed(share) => (f32::from(share) / 100.0, STATUS_FAILED),
		Progress::Paused(share) => (f32::from(share) / 100.0, PROGRESS_PAUSED),
	};

	let bar = progress_bar(0.0..=1.0, fraction)
		.length(Length::Fill)
		.girth(Length::Fixed(PROGRESS_GIRTH))
		.style(move |_theme| progress_bar::Style {
			background: PROGRESS_TRACK.into(),
			bar: fill.into(),
			border: Border::default(),
		});
	// Pinned to the chip's bottom edge: the bar fills the stack layer's height and pushes itself
	// down, so it rides the edge whatever the chip's own padding does above it.
	Some(
		container(bar)
			.height(Length::Fill)
			.align_y(Vertical::Bottom)
			.into(),
	)
}

/// Build the tab strip from the chips, in strip order. Returns an owned (`'static`) element —
/// every label is cloned into its widget, so nothing is borrowed from the caller.
///
/// `dragging` says a tab is being moved right now (§38): it only changes the cursor, which becomes
/// the "grabbing" hand over every chip, so the gesture reads as in progress wherever the pointer has
/// got to. The bar itself catches the release (a drop on the gap between chips still counts) and the
/// pointer leaving the strip, which abandons the move.
///
/// `focused` says this strip's region holds the keyboard (§48), which only tints the bar. A window
/// with no split has exactly one region and it is always focused, so that window looks as it always
/// did and the tint is a cost paid only once the user asks for a split.
///
/// `splittable` says this strip may OFFER a split, and only the undivided window's one strip ever
/// does (§48). One cut is all there is, so once it has been made neither strip shows the controls
/// again until the split region closes — a control that cannot do anything is worse than an absent
/// one, because the only way to find out is to press it.
pub fn strip(
	chips: &[Chip],
	dragging: bool,
	focused: bool,
	splittable: bool,
) -> Element<'static, Message> {
	let mut items: Vec<Element<'static, Message>> = Vec::with_capacity(chips.len() + 4);
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
	// The split controls are pushed to the FAR RIGHT rather than left where the "+" sits (§48).
	// Chips grow with their labels, so a strip with several long endpoints in it would otherwise
	// walk these two off the bar — and a control that can be pushed out of reach is one a user has
	// to close a tab to get at. The spacer goes in either way: without it a strip that is not
	// offering a split would let its chips spread across the bar and then pull back once one was.
	items.push(space().width(Length::Fill).into());
	if splittable {
		items.push(split_button(
			SPLIT_BESIDE_GLYPH,
			Message::Split(split::Way::Horizontal),
		));
		items.push(split_button(
			SPLIT_BELOW_GLYPH,
			Message::Split(split::Way::Vertical),
		));
	}

	let fill = if focused { BAR_BG } else { BAR_UNFOCUSED_BG };
	// The row is told to FILL, not left to shrink to its chips: the spacer above only pushes the split
	// buttons to the far end if there is a full bar's width for it to take up (§48).
	let bar = container(
		row(items)
			.spacing(2)
			.align_y(Vertical::Center)
			.width(Length::Fill),
	)
	.width(Length::Fill)
	.height(Length::Fixed(STRIP_HEIGHT))
	.padding(4)
	.style(move |_theme| container::Style {
		background: Some(fill.into()),
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

/// One of the two split controls at the right of the bar (§48): cut this region in two and put a
/// fresh one beside it or below it. Muted at rest and lit on hover, like the "×" in a chip — a
/// control that changes the shape of the whole window should not be the loudest thing on the strip.
///
/// Never disabled, only absent (§48): it is drawn on the undivided window's strip and nowhere else,
/// so whenever it is there it works. A split itself cannot fail — the window asks the OS to grow to
/// make the room, and if the screen has none left to give, the two regions share what it already
/// had. A greyed-out button would have had to explain which of those two it was.
fn split_button(glyph: char, message: Message) -> Element<'static, Message> {
	button(
		text(glyph.to_string())
			.font(ICON_FONT)
			.size(SPLIT_ICON_SIZE)
			.color(INACTIVE_FG),
	)
	.padding([0.0, 4.0])
	.on_press(message)
	.style(|_theme, status| button::Style {
		background: match status {
			button::Status::Hovered | button::Status::Pressed => Some(ACTIVE_BG.into()),
			_ => None,
		},
		text_color: INACTIVE_FG,
		border: Border {
			radius: 4.0.into(),
			..Border::default()
		},
		..button::Style::default()
	})
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
	// The branch pill (§55), after the endpoint label and before the ✕. AFTER matters: the label is
	// what says which machine this is, so the remote-chosen text can never be read as the start of it.
	if let Some(branch) = chip.branch.as_deref() {
		contents = contents.push(
			container(
				text(branch.to_owned())
					.size(BRANCH_TEXT_SIZE)
					.color(BRANCH_FG),
			)
			.padding([0.0, 5.0])
			.style(|_theme| container::Style {
				background: Some(BRANCH_BG.into()),
				border: Border {
					radius: 7.0.into(),
					..Border::default()
				},
				..container::Style::default()
			}),
		);
	}
	let close = button(text("×").size(14).color(fg))
		.padding([0.0, 4.0])
		.on_press(Message::TabCloseRequested(chip.id))
		.style(move |_theme, _status| button::Style {
			background: None,
			text_color: fg,
			..button::Style::default()
		});

	// The "×" is a button sitting on a drag handle, and it wins the cursor while it has the pointer
	// (§52): the chip's own `mouse_area` reports the pointer anywhere inside its bounds, this one
	// included, so without saying so the hand would offer to pick up the control that CLOSES the
	// tab. The wrapper sets no press handler, so it captures nothing and the button's own click is
	// untouched.
	let close = mouse_area(close)
		.on_enter(Message::GrabControlEntered(chip.id))
		.on_exit(Message::GrabControlExited(chip.id));

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

	// The progress a remote command reports (§54), laid over the chip's bottom edge rather than inside
	// its row: the bar must not shift the label or change how wide the chip is, because a bar that
	// appears and vanishes with a command would otherwise make the whole strip twitch. Nothing is
	// stacked when there is no progress, so the common chip is the bare cell it always was.
	let cell: Element<'static, Message> = match progress_bar_for(chip.progress) {
		Some(bar) => stack![cell, bar].into(),
		None => cell.into(),
	};

	// The whole chip is the drag handle (§38). `on_press` selects and grabs; `on_release` drops. The
	// cursor advertises the gesture: an open hand at rest, a closed one while a tab is in flight.
	//
	// WHO draws that hand depends on the platform (§51). Windows has no hand cursor at all, so
	// `chip_interaction` answers `None` there — iced is asked for nothing, precisely so it never
	// sets a cursor over the strip, and `cursor` paints the two hands itself from the enter/exit
	// events below. Everywhere else the toolkit has them and is simply asked.
	let area = mouse_area(cell)
		.on_press(Message::TabSelected(index))
		.on_release(Message::TabDropped);
	let area = match crate::cursor::grab_interaction(dragging) {
		Some(interaction) => area.interaction(interaction),
		None => area,
	};

	// A right press opens the chip's own menu (§52), which sends this tab to another area of the
	// window. It names the chip by INDEX, exactly as the left press does, and — unlike the left one
	// — does not select it: acting on a background tab is the point of having the menu on the chip
	// rather than on the tab already showing, so opening the menu must not change what is on screen.
	let area = area.on_right_press(Message::TabMenuOpened(index));

	// This chip is still on screen, which is what keeps a hand it is holding (§52). A chip that
	// closes, or is sent to another region, simply stops saying so — and iced publishes no `on_exit`
	// for a widget that has left the tree, so this is the only way the hand hears about it.
	crate::cursor::drawn(chip.id);

	// While a tab is in flight the pointer entering a chip means "this slot", which is the message
	// the drop needs. At rest it means "a hand goes here" (§51) — and the pair of them is why the
	// exit is only wired at rest: mid-drag the hand is closed wherever the pointer has got to, so
	// leaving a chip changes nothing.
	if dragging {
		area.on_enter(Message::TabDraggedOver(index)).into()
	} else {
		area.on_enter(Message::GrabEntered(chip.id))
			.on_exit(Message::GrabExited(chip.id))
			.into()
	}
}

/// One area the chip's menu offers to send a tab to (§52), and whether each of the two actions can
/// actually act on it. `App` works both flags out — they depend on the tree of regions, on how many
/// tabs the strip holds and on what the tab IS — and this file only draws the answer.
///
/// A destination that cannot be acted on is still LISTED, dimmed, rather than dropped: with at most
/// four rows per group, a menu whose items move about between openings is harder to use than one
/// with a greyed row in it, and the grey is itself the explanation.
#[derive(Debug, Clone, Copy)]
pub struct Destination {
	pub area: split::Area,
	/// Whether "Move to …" would do something. False for the area the tab is already in, and for a
	/// move that would empty this region into a brand-new one — a cut and a collapse in one press.
	pub can_move: bool,
	/// Whether "Duplicate to …" would do something. False unless the tab holds a session there is
	/// something to open a second copy of.
	pub can_duplicate: bool,
}

/// The name an area goes by in the menu. Deliberately not the word "region": the menu is read by
/// someone looking at the window, and what they see is a place — the main one, the one on the right,
/// the one at the bottom — rather than a node in a tree (§52).
fn area_label(area: split::Area) -> &'static str {
	match area {
		split::Area::Main => "main",
		split::Area::Right => "right",
		split::Area::Bottom => "bottom",
	}
}

/// The chip's right-click menu (§52): move this tab to an area of the window, or open a second copy
/// of it there. `at` is where the click landed, in WINDOW coordinates — the menu is drawn over the
/// whole window rather than inside the region, because a menu clipped to a region would be cut off
/// by the very seam it is offering to send the tab across.
///
/// `window` is only used to keep the panel inside the right edge: a chip near the end of a wide
/// strip would otherwise open a menu half of which is off screen. The bottom edge needs no such care
/// — a strip sits at the top of its region, so there is always room below it.
pub fn context_menu(
	at: Point,
	window: Size,
	destinations: &[Destination],
) -> Element<'static, Message> {
	let mut items: Vec<Element<'static, Message>> = Vec::with_capacity(destinations.len() * 2 + 1);
	for destination in destinations {
		items.push(crate::ui::menu::item(
			format!("Move to {} area", area_label(destination.area)),
			destination
				.can_move
				.then_some(Message::TabMoveTo(destination.area)),
		));
	}
	items.push(crate::ui::menu::separator());
	for destination in destinations {
		items.push(crate::ui::menu::item(
			format!("Duplicate to {} area", area_label(destination.area)),
			destination
				.can_duplicate
				.then_some(Message::TabDuplicateTo(destination.area)),
		));
	}

	// A full-window transparent container whose padding places the panel at the click, exactly as
	// the grid's own menu is placed (§10).
	container(crate::ui::menu::panel(items))
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(iced::Padding {
			top: at.y,
			right: 0.0,
			bottom: 0.0,
			left: at.x.min((window.width - crate::ui::menu::WIDTH).max(0.0)),
		})
		.into()
}

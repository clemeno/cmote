// ui/dialog.rs — the shared modal-dialog chrome (PLAN §10).
//
// Every dialog in the app wears the same frame so they read as one family:
//   * a header bar — the question as a title on the LEFT, a close (✕) button on the
//     RIGHT that emits the caller's "safe" action (cancel / reject / back);
//   * a body — copy explaining what confirming the action will do (plus, for the
//     passphrase prompt, its input field);
//   * a footer — the action buttons spread EVENLY across the width.
// Assembling that frame here (rather than in each view) keeps the four call sites —
// the disconnect confirmation, the host-key prompt, the passphrase prompt, and the
// error notice — consistent, and means a change to the chrome touches one function.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::{button, column, container, mouse_area, row, stack, text, text_editor};
use iced::{Background, Border, Color, Element, Length, Padding, Point};

use crate::app::Message;

// Dialog surface colours: a dark card, a slightly lighter header bar so the title
// stands apart, a faint border so the card reads as a raised surface over whatever
// sits behind it, and the foreground shared by the title and body copy.
const CARD_BG: Color = Color::from_rgb8(0x2b, 0x2b, 0x2b);
const HEADER_BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);
const BORDER_FG: Color = Color::from_rgb8(0x50, 0x50, 0x50);
const FG: Color = Color::from_rgb8(0xe0, 0xe0, 0xe0);

/// The fill painted behind selected body text (§10) — a muted blue that reads under
/// the light body colour, matching the terminal grid's own selection highlight.
const SELECTION_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);

// Type sizes for the header title and the body copy.
const TITLE_SIZE: f32 = 16.0;
const BODY_SIZE: f32 = 14.0;

/// The close (✕) button's square hit area. The glyph is centred in this box so it
/// sits dead-centre in the header instead of riding high on its own text baseline.
const CLOSE_BUTTON_SIZE: f32 = 24.0;

/// The card's fixed width and an estimate of its height (§10). Width is fixed, so
/// horizontal dragging is clamped exactly. Height varies with the message and iced does
/// not expose the laid-out size, so the estimate is used only to *centre* the card when
/// it opens; vertical dragging is bounded by `DIALOG_DRAG_MIN_VISIBLE` instead.
pub const DIALOG_WIDTH: f32 = 460.0;
pub const DIALOG_HEIGHT_ESTIMATE: f32 = 220.0;

/// How much of the card must stay on screen when dragging down (§10). Roughly the
/// header height, so the drag handle and ✕ remain reachable to move the dialog back.
/// Using this (rather than the full card height, which we cannot measure) means the
/// dialog can be dragged all the way to the window's bottom instead of stopping short.
pub const DIALOG_DRAG_MIN_VISIBLE: f32 = 44.0;

/// The card's rounded-corner radius, matched by the header bar (which rounds its own
/// top corners) so the header does not square off over the card's rounded border.
const CORNER_RADIUS: f32 = 6.0;

/// Where the dialog card sits and whether it is mid-drag (§10). `pos` is the card's
/// top-left in window coordinates (seeded to centre and clamped by `app`); `dragging`
/// switches on the pointer-capture layer that follows the drag.
#[derive(Debug, Clone, Copy)]
pub struct Drag {
	pub pos: Point,
	pub dragging: bool,
}

/// Assemble a dialog card. `title` is the question shown in the header; `on_close`
/// is emitted by the ✕ button (wire it to the safe/cancel action); `body` explains
/// what the action does; `footer` holds the action buttons, laid out evenly across
/// the width. `drag` places the card (its top-left) and, while dragging, adds a
/// pointer-capture layer. The result fills the window, so a caller overlaying a live
/// view stacks it over a dimming backdrop, while a standalone screen renders it on the
/// plain window background.
pub fn dialog<'a>(
	title: String,
	on_close: Message,
	body: Element<'a, Message>,
	footer: Vec<Element<'a, Message>>,
	drag: Drag,
) -> Element<'a, Message> {
	// Header / body / footer stacked with no gaps: each band paints its own region,
	// so the seams line up flush and the header colour meets the body cleanly. The
	// width is fixed so `app` can clamp horizontal dragging exactly.
	let card = container(column![
		header_bar(title, on_close),
		body_band(body),
		footer_bar(footer)
	])
	.width(Length::Fixed(DIALOG_WIDTH))
	// Clip so the header respects the card's rounded corners (see CORNER_RADIUS).
	.clip(true)
	.style(|_theme| container::Style {
		background: Some(CARD_BG.into()),
		border: Border {
			color: BORDER_FG,
			width: 1.0,
			radius: CORNER_RADIUS.into(),
		},
		..container::Style::default()
	});

	// Swallow clicks that land on the card so they do not fall through to a dimming
	// backdrop below and dismiss the dialog. Clicks OUTSIDE the card still reach the
	// backdrop, so clicking away can still cancel. A selectable widget inside the card
	// receives its own press first (children handle events before this wrapper), so
	// this does not block selecting the body text.
	let card = mouse_area(card)
		.on_press(Message::Ignored)
		.on_right_press(Message::Ignored);

	// Place the card's top-left at `drag.pos`. The window-filling container is
	// top-left aligned by default, so its padding acts as an absolute offset.
	let positioned = container(card)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(Padding {
			top: drag.pos.y,
			right: 0.0,
			bottom: 0.0,
			left: drag.pos.x,
		});

	// While dragging, a transparent full-window layer on top captures every pointer
	// move and the release, so tracking continues even when the pointer leaves the card
	// (its coordinates are window-local because the layer fills the window from origin).
	if drag.dragging {
		stack![positioned, drag_capture_layer()]
			.width(Length::Fill)
			.height(Length::Fill)
			.into()
	} else {
		positioned.into()
	}
}

/// A dimming full-window scrim behind a modal (§10): translucent black that darkens
/// whatever sits behind it so the dialog reads as focused, and emits `on_dismiss` when
/// clicked so a click outside the card cancels. Shared by the disconnect modal (over the
/// shell) and the connect-flow dialogs (over the form).
pub fn backdrop(on_dismiss: Message) -> Element<'static, Message> {
	mouse_area(
		container(text(""))
			.width(Length::Fill)
			.height(Length::Fill)
			.style(|_theme| container::Style {
				background: Some(
					Color {
						a: 0.55,
						..Color::BLACK
					}
					.into(),
				),
				..container::Style::default()
			}),
	)
	.on_press(on_dismiss)
	.into()
}

/// A transparent full-window layer that reports pointer moves and the release while a
/// dialog is being dragged (§10). Present only mid-drag, so it never blocks the card's
/// buttons or text at rest.
fn drag_capture_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_move(Message::DialogDragged)
		.on_release(Message::DialogReleased)
		.into()
}

/// The body message as a **read-only, selectable** editor (§10). The user can drag to
/// select the text and copy it (Ctrl+C), but not edit it — `app` performs every
/// `text_editor` action except an edit, so the buffer never changes. It is styled
/// transparent and borderless in the shared body size/colour, so it reads like the
/// plain label it replaces while gaining selection. `content` is `App::dialog_body`,
/// seeded with this dialog's message when the dialog opens. Callers needing more than
/// the message (the passphrase field, a "wrong passphrase" hint) wrap this in their
/// own column and pass that as the body.
pub fn selectable_body(content: &text_editor::Content) -> Element<'_, Message> {
	text_editor(content)
		.on_action(Message::DialogAction)
		.size(BODY_SIZE)
		.padding(0)
		.style(|_theme, _status| text_editor::Style {
			background: Background::Color(Color::TRANSPARENT),
			border: Border::default(),
			placeholder: FG,
			value: FG,
			selection: SELECTION_BG,
		})
		.into()
}

/// The header bar: the title filling the width on the left, a square close (✕) button
/// pinned to the right. The ✕ emits `on_close`, so closing the dialog is always the
/// safe choice (never the destructive one).
fn header_bar<'a>(title: String, on_close: Message) -> Element<'a, Message> {
	let label = container(text(title).size(TITLE_SIZE).color(FG))
		.width(Length::Fill)
		.align_x(Horizontal::Left);

	// Transparent so only the ✕ shows (no raised-button chrome), with the glyph
	// centred in a fixed square so it aligns to the middle of the header row. The
	// style ignores theme and status — always no fill, our foreground glyph — so it
	// reads as a plain icon.
	let glyph = container(text("✕").size(TITLE_SIZE))
		.width(Length::Fixed(CLOSE_BUTTON_SIZE))
		.height(Length::Fixed(CLOSE_BUTTON_SIZE))
		.align_x(Horizontal::Center)
		.align_y(Vertical::Center);
	let close = button(glyph)
		.padding(0)
		.on_press(on_close)
		.style(|_theme, _status| button::Style {
			background: None,
			text_color: FG,
			..button::Style::default()
		});

	let bar = container(row![label, close].spacing(10).align_y(Vertical::Center))
		.width(Length::Fill)
		.padding(10)
		.style(|_theme| container::Style {
			background: Some(HEADER_BG.into()),
			// Round the header's top corners to match the card, so its fill does not
			// square off over the card's rounded border (the card's `clip` only clips a
			// rectangle, not the radius). Bottom corners stay square — the body meets it flush.
			border: Border {
				radius: iced::border::Radius::from(0.0).top(CORNER_RADIUS),
				..Border::default()
			},
			..container::Style::default()
		});

	// The header background is the drag handle: pressing it starts a drag, releasing
	// ends one (§10). The ✕ button inside captures its own press, so closing still
	// works and does not begin a drag. The release is normally caught by the capture
	// layer, but handling it here too ends a click that never moved.
	mouse_area(bar)
		.on_press(Message::DialogGrabbed)
		.on_release(Message::DialogReleased)
		.into()
}

/// The body region: the caller's content padded away from the card edges.
fn body_band(body: Element<'_, Message>) -> Element<'_, Message> {
	container(body).width(Length::Fill).padding(20).into()
}

/// The footer: the buttons spread evenly across the width. Each button sits in its
/// own equal-`Fill` cell centred on its share, so N buttons divide the footer into N
/// even columns and stay centred regardless of their individual widths.
fn footer_bar<'a>(buttons: Vec<Element<'a, Message>>) -> Element<'a, Message> {
	let cells: Vec<Element<'a, Message>> = buttons
		.into_iter()
		.map(|content| {
			container(content)
				.width(Length::Fill)
				.align_x(Horizontal::Center)
				.into()
		})
		.collect();

	container(row(cells).spacing(10))
		.width(Length::Fill)
		.padding(iced::Padding {
			top: 0.0,
			right: 12.0,
			bottom: 14.0,
			left: 12.0,
		})
		.into()
}

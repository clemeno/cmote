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
use iced::widget::{button, column, container, mouse_area, row, text, text_editor};
use iced::{Background, Border, Color, Element, Length};

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

/// The card's maximum width, so a wide window does not stretch a short message
/// across the whole screen; the card centres within the leftover space.
const MAX_WIDTH: f32 = 480.0;

/// The card's rounded-corner radius, matched by the header bar via clipping so the
/// header's square top corners do not poke past the rounded card.
const CORNER_RADIUS: f32 = 6.0;

/// Assemble a dialog card. `title` is the question shown in the header; `on_close`
/// is emitted by the ✕ button (wire it to the safe/cancel action); `body` explains
/// what the action does; `footer` holds the action buttons, laid out evenly across
/// the width. The result fills the window and centres the card, so a caller
/// overlaying a live view stacks it straight over a dimming backdrop, while a
/// standalone screen renders it on the plain window background.
pub fn dialog<'a>(
	title: String,
	on_close: Message,
	body: Element<'a, Message>,
	footer: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
	// Header / body / footer stacked with no gaps: each band paints its own region,
	// so the seams line up flush and the header colour meets the body cleanly.
	let card = container(column![
		header_bar(title, on_close),
		body_band(body),
		footer_bar(footer)
	])
	.width(Length::Fill)
	.max_width(MAX_WIDTH)
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

	// Centre the card in the window over whatever the caller placed behind it.
	container(card)
		.width(Length::Fill)
		.height(Length::Fill)
		.align_x(Horizontal::Center)
		.align_y(Vertical::Center)
		.padding(20)
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

	container(row![label, close].spacing(10).align_y(Vertical::Center))
		.width(Length::Fill)
		.padding(10)
		.style(|_theme| container::Style {
			background: Some(HEADER_BG.into()),
			..container::Style::default()
		})
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

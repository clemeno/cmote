// ui/dialog.rs — the shared modal-dialog chrome (PLAN §10).
//
// Every dialog in the app wears the same frame so they read as one family:
//   * a header bar — the question as a title on the LEFT, a close (✕) button on the
//     RIGHT that emits the caller's "safe" action (cancel / reject / back);
//   * a body — copy explaining what confirming the action will do (plus, for the
//     passphrase prompt, its input field);
//   * a footer — the action buttons spread EVENLY across the width.
// Assembling that frame here (rather than in each view) keeps the seven call sites —
// the delete-target, disconnect, upload and overwrite confirmations, the host-key
// prompt, the passphrase prompt, and the error notice — consistent, and means a change
// to the chrome touches one function.

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

/// A floating dialog card: where it sits, and whether it is being dragged by its header (§10).
///
/// It owns the whole gesture — centring on open, the anchor-then-delta arithmetic of a drag, and
/// the clamp that keeps the header reachable — because that arithmetic used to exist TWICE, once
/// for a tab's own dialogs and once for the App-level overlay cards (§26, §30), differing only in
/// which box they were measured against. The two copies were line for line the same, and each
/// correction had to be made in both; here it is made once, and the box is a parameter.
///
/// The fields are private on purpose. A caller holds a `Card`, hands it the pointer, and passes it
/// to `dialog` — it never has to know that a drag needs an anchor to avoid jumping on its first
/// move, which is precisely the detail both copies had to get right.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Card {
	/// The card's top-left, in the coordinates of whatever box it floats over — the OS window for
	/// an App-level overlay, a region for a tab's own dialog (§48).
	pos: Point,
	/// Whether the header is being held right now. Switches on the pointer-capture layer that
	/// follows the drag past the card's own edges, and closes the hand cursor (§51).
	dragging: bool,
	/// The pointer position at the previous move of this drag, so successive positions become
	/// movement deltas. `None` between drags and before a drag's FIRST move: a press reports where
	/// the pointer is, not where inside the header it landed, so the first move can only record an
	/// anchor — applying it as a delta would snap the card's corner to the pointer.
	last: Option<Point>,
}

impl Card {
	/// A card freshly opened in `box_size`: centred, at rest, with no leftover anchor — so a spot
	/// dragged into during a previous dialog never carries across to the next (§10, §26).
	///
	/// Centring uses the card's fixed width and its ESTIMATED height (iced does not expose the
	/// laid-out size), and floors at zero so a box too small to centre in keeps the card at the
	/// origin rather than off-screen.
	pub fn opened(box_size: iced::Size) -> Self {
		Self {
			pos: Point::new(
				((box_size.width - DIALOG_WIDTH) / 2.0).max(0.0),
				((box_size.height - DIALOG_HEIGHT_ESTIMATE) / 2.0).max(0.0),
			),
			dragging: false,
			last: None,
		}
	}

	/// The header was pressed: the drag begins (§10). The anchor is cleared rather than set,
	/// because a press carries no useful offset — see `last`.
	pub fn grab(&mut self) {
		self.dragging = true;
		self.last = None;
	}

	/// The pointer moved during a drag: shift the card by the delta since the previous move and
	/// clamp it back into `box_size` (§10). The first move of a drag only records the anchor, so
	/// the card does not jump; a move while nothing is held is ignored, which is what makes the
	/// full-window capture layer harmless at rest.
	pub fn drag_to(&mut self, pointer: Point, box_size: iced::Size) {
		if !self.dragging {
			return;
		}
		if let Some(last) = self.last {
			self.pos = clamped(self.pos + (pointer - last), box_size);
		}
		self.last = Some(pointer);
	}

	/// The drag ended (§10). The anchor goes with it, so the next drag starts by re-anchoring
	/// instead of measuring against wherever this one happened to stop.
	pub fn release(&mut self) {
		self.dragging = false;
		self.last = None;
	}

	/// The box the card floats over changed size (§26): pull the card back into it, so one dragged
	/// to a far corner before the window shrank is not left stranded off-screen. Harmless — and
	/// cheap — when the card is nowhere near an edge.
	pub fn reflow(&mut self, box_size: iced::Size) {
		self.pos = clamped(self.pos, box_size);
	}

	/// Test-only: the card's top-left. Production code never needs it — `dialog` is handed the
	/// whole card and reads the field directly — so this is not part of the interface a caller
	/// learns; it exists so an assertion can say where the card ended up.
	#[cfg(test)]
	pub(crate) fn pos(self) -> Point {
		self.pos
	}

	/// Test-only, for the same reason as `pos`: whether the header is being held right now.
	#[cfg(test)]
	pub(crate) fn is_dragging(self) -> bool {
		self.dragging
	}
}

/// Keep a proposed top-left inside `box_size` so the card stays reachable (§10).
///
/// Horizontal is exact — the fixed width keeps the whole card between the side edges. Vertical
/// only keeps `DIALOG_DRAG_MIN_VISIBLE` on screen rather than the whole card, because iced does
/// not expose the card's real height; that lets the dialog be dragged right down to the bottom
/// edge instead of being blocked short of it, while the header and its ✕ stay in reach.
fn clamped(pos: Point, box_size: iced::Size) -> Point {
	let max_x = (box_size.width - DIALOG_WIDTH).max(0.0);
	let max_y = (box_size.height - DIALOG_DRAG_MIN_VISIBLE).max(0.0);
	Point::new(pos.x.clamp(0.0, max_x), pos.y.clamp(0.0, max_y))
}

/// Assemble a dialog card. `title` is the question shown in the header; `on_close`
/// is emitted by the ✕ button (wire it to the safe/cancel action); `body` explains
/// what the action does; `footer` holds the action buttons, laid out evenly across
/// the width. `card` places it (its top-left) and, while dragging, adds a
/// pointer-capture layer. The result fills the window, so a caller overlaying a live
/// view stacks it over a dimming backdrop, while a standalone screen renders it on the
/// plain window background.
pub fn dialog<'a>(
	title: String,
	on_close: Message,
	body: Element<'a, Message>,
	footer: Vec<Element<'a, Message>>,
	card: Card,
) -> Element<'a, Message> {
	// Header / body / footer stacked with no gaps: each band paints its own region,
	// so the seams line up flush and the header colour meets the body cleanly. The
	// width is fixed so the drag can be clamped horizontally to the exact edge.
	let chrome = container(column![
		header_bar(title, on_close, card.dragging),
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
	let chrome = mouse_area(chrome)
		.on_press(Message::Ignored)
		.on_right_press(Message::Ignored);

	// Place the card's top-left at `card.pos`. The window-filling container is
	// top-left aligned by default, so its padding acts as an absolute offset.
	let positioned = container(chrome)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(Padding {
			top: card.pos.y,
			right: 0.0,
			bottom: 0.0,
			left: card.pos.x,
		});

	// While dragging, a transparent full-window layer on top captures every pointer
	// move and the release, so tracking continues even when the pointer leaves the card
	// (its coordinates are window-local because the layer fills the window from origin).
	if card.dragging {
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
	// Everything under this cannot be picked up while it is there (§52): a chip behind a modal is
	// still a live widget and still reports the pointer entering it, so without this the strip would
	// wear the open hand over chips a click cannot even reach.
	crate::cursor::covered();
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
///
/// `dragging` is the card's own drag state, and reaches this far only for the cursor (§51): the
/// hand is closed for as long as the card is held, which is a property of the gesture rather than
/// of where the pointer happens to be — a card dragged out from under the pointer must not open the
/// hand halfway through the drag.
fn header_bar<'a>(title: String, on_close: Message, dragging: bool) -> Element<'a, Message> {
	let label = container(text(title).size(TITLE_SIZE).color(FG))
		.width(Length::Fill)
		.align_x(Horizontal::Left);

	// The ✕ wins the cursor while it has the pointer (§52), exactly as a chip's "×" does: the header
	// is the card's drag handle and reports the pointer anywhere inside itself, and a hand over the
	// button that DISMISSES the dialog says the wrong thing about what a press there would do.
	let close = mouse_area(close_button(on_close))
		.on_enter(Message::GrabControlEntered(crate::cursor::HEADER))
		.on_exit(Message::GrabControlExited(crate::cursor::HEADER));

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
	//
	// It wears the hand (§51), the same one a tab chip does: an open hand says the card can be
	// picked up — which nothing else on a dialog says, since the header looks like a title bar and
	// title bars are not always draggable — and a closed one from the press to the release. The
	// enter/exit pair is what tells `cursor` the pointer is on a handle; who draws the hand depends
	// on the platform, which is `grab_interaction`'s business and not this file's.
	//
	// It names itself `cursor::HEADER` (§52): a card is drawn while its dialog is open and gone the
	// instant it closes — including when the ✕ inside this very bar is what closed it — and iced
	// publishes no exit for a widget that has left the tree. So the header says it is still there
	// with every frame it is drawn into, and the hand lets go on the first frame it is not.
	crate::cursor::drawn(crate::cursor::HEADER);
	let area = mouse_area(bar)
		.on_press(Message::DialogGrabbed)
		.on_release(Message::DialogReleased)
		.on_enter(Message::GrabEntered(crate::cursor::HEADER))
		.on_exit(Message::GrabExited(crate::cursor::HEADER));
	match crate::cursor::grab_interaction(dragging) {
		Some(interaction) => area.interaction(interaction).into(),
		None => area.into(),
	}
}

/// The shared close (✕) button (§10, §32): a transparent square with the glyph centred, no
/// raised chrome, emitting `on_press`. The dialog header pins it top-right; the editor toolbar
/// reuses it for its Close, so a "close this" affordance is the same icon everywhere. The style
/// ignores theme and status — always no fill, our foreground glyph — so it reads as a plain icon.
pub fn close_button(on_press: Message) -> Element<'static, Message> {
	let glyph = container(text("✕").size(TITLE_SIZE))
		.width(Length::Fixed(CLOSE_BUTTON_SIZE))
		.height(Length::Fixed(CLOSE_BUTTON_SIZE))
		.align_x(Horizontal::Center)
		.align_y(Vertical::Center);
	button(glyph)
		.padding(0)
		.on_press(on_press)
		.style(|_theme, _status| button::Style {
			background: None,
			text_color: FG,
			..button::Style::default()
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

#[cfg(test)]
mod tests {
	use super::*;

	/// A comfortable box to float a card in, big enough that centring is not clamped.
	const ROOMY: iced::Size = iced::Size {
		width: 1000.0,
		height: 800.0,
	};

	#[test]
	fn a_card_opens_centred_and_at_rest() {
		let card = Card::opened(ROOMY);
		assert_eq!(card.pos().x, (1000.0 - DIALOG_WIDTH) / 2.0);
		assert_eq!(card.pos().y, (800.0 - DIALOG_HEIGHT_ESTIMATE) / 2.0);
		assert!(!card.is_dragging(), "a card opens ready to read, not held");
	}

	/// A box smaller than the card cannot centre it — the arithmetic would put the top-left
	/// negative, which is off-screen in the one direction nothing can drag it back from.
	#[test]
	fn a_box_too_small_to_centre_in_keeps_the_card_at_the_origin() {
		let card = Card::opened(iced::Size::new(100.0, 100.0));
		assert_eq!(card.pos(), Point::ORIGIN);
	}

	/// A press reports where the POINTER is, not where inside the header it landed, so the first
	/// move of a drag can only record an anchor. Applying it as a delta would snap the card's
	/// corner to the pointer — a visible jump at the start of every drag.
	#[test]
	fn the_first_move_of_a_drag_only_anchors_it() {
		let mut card = Card::opened(ROOMY);
		let start = card.pos();
		card.grab();
		card.drag_to(Point::new(500.0, 500.0), ROOMY);
		assert_eq!(card.pos(), start);
		card.drag_to(Point::new(520.0, 540.0), ROOMY);
		assert_eq!(card.pos(), Point::new(start.x + 20.0, start.y + 40.0));
	}

	/// The capture layer that follows a drag past the card's edges fills the whole box, so it
	/// reports moves that are none of the card's business. One that arrives with nothing held
	/// moves nothing.
	#[test]
	fn a_card_that_is_not_held_ignores_a_move() {
		let mut card = Card::opened(ROOMY);
		let start = card.pos();
		card.drag_to(Point::new(10.0, 10.0), ROOMY);
		card.drag_to(Point::new(400.0, 400.0), ROOMY);
		assert_eq!(card.pos(), start);
	}

	/// A release forgets the anchor as well as ending the drag, so the NEXT drag re-anchors
	/// instead of measuring its first move against wherever this one stopped — which would fling
	/// the card by the whole distance between the two gestures.
	#[test]
	fn a_release_forgets_the_anchor_so_the_next_drag_does_not_fling() {
		let mut card = Card::opened(ROOMY);
		card.grab();
		card.drag_to(Point::new(100.0, 100.0), ROOMY);
		card.drag_to(Point::new(140.0, 130.0), ROOMY);
		let settled = card.pos();
		card.release();
		assert!(!card.is_dragging());

		card.grab();
		card.drag_to(Point::new(900.0, 700.0), ROOMY);
		assert_eq!(
			card.pos(),
			settled,
			"the new drag anchors, it does not jump"
		);
	}

	/// Dragged at the edges, the card stays reachable: fully inside horizontally (its width is
	/// fixed, so that clamp is exact), and vertically only far enough to keep the header — and
	/// with it the ✕ and the drag handle — on screen.
	#[test]
	fn a_card_cannot_be_dragged_out_of_reach() {
		let mut card = Card::opened(ROOMY);
		card.grab();
		card.drag_to(Point::new(0.0, 0.0), ROOMY);
		card.drag_to(Point::new(5000.0, 5000.0), ROOMY);
		assert_eq!(card.pos().x, 1000.0 - DIALOG_WIDTH);
		assert_eq!(card.pos().y, 800.0 - DIALOG_DRAG_MIN_VISIBLE);

		card.drag_to(Point::new(-5000.0, -5000.0), ROOMY);
		assert_eq!(card.pos(), Point::ORIGIN);
	}

	/// The box can shrink under a card that was dragged to a far corner — a window resize, or a
	/// region losing width to a split (§48). The card is pulled back rather than stranded.
	#[test]
	fn a_shrunken_box_pulls_a_dragged_card_back_into_reach() {
		let mut card = Card::opened(ROOMY);
		card.grab();
		card.drag_to(Point::new(0.0, 0.0), ROOMY);
		card.drag_to(Point::new(5000.0, 5000.0), ROOMY);

		let small = iced::Size::new(500.0, 400.0);
		card.reflow(small);
		assert_eq!(card.pos().x, (500.0 - DIALOG_WIDTH).max(0.0));
		assert_eq!(card.pos().y, 400.0 - DIALOG_DRAG_MIN_VISIBLE);
	}
}

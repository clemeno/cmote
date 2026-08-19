// ui/forward.rs — the port-forwards management dialog (PLAN §27).
//
// One modal, opened from the terminal status bar's "Tunnels" button, that lists the session's
// forwards and lets the user add or remove them. It wears the shared dialog chrome
// (`ui::dialog`) like every other modal, but its body is richer than a line of text: a row per
// live forward (its label, a status dot, a remove ✕) above a small add form (a kind selector,
// a listen field, a target field, an Add button). The *data* it draws — the parsed spec, the
// status — is `crate::forward`; this module is only its shape on screen.

use iced::alignment::Vertical;
use iced::widget::{button, column, container, row, text, text_input};
use iced::{Border, Color, Element, Length};

use crate::app::Message;
use crate::forward::{ForwardEntry, ForwardKind, ForwardStatus};
use crate::ui::dialog::{self, Card};

/// The widget id of the add form's listen field, so `app` can focus it as the dialog opens —
/// the first thing to type when adding a forward.
pub const LISTEN_INPUT_ID: &str = "forward-listen";

// Row colours: a live forward's green dot, a failed one's red, the muted "starting" grey, and
// the tint on the selected kind button.
const ACTIVE_FG: Color = Color::from_rgb8(0x6a, 0xbf, 0x6a);
const FAILED_FG: Color = Color::from_rgb8(0xd0, 0x6a, 0x6a);
const MUTED_FG: Color = Color::from_rgb8(0x90, 0x90, 0x90);
const FG: Color = Color::from_rgb8(0xe0, 0xe0, 0xe0);
const SELECTED_BG: Color = Color::from_rgb8(0x3d, 0x55, 0x77);

const BODY_SIZE: f32 = 14.0;

/// The tunnels dialog's add form (§27): the kind selected, the two fields being typed, and the
/// last parse error to show under them.
///
/// It is OWNED STATE, not a view of some — the app holds one, exactly as it holds a
/// `ui::connect::ConnectForm`. It lives INSIDE the open modal rather than beside it, because it
/// exists only while the dialog does: dismissing a form is what throws a half-typed forward away.
/// The session's forwards themselves are not here; they outlive any number of opens and closes of
/// this dialog, so they stay on the tab.
#[derive(Debug, Default)]
pub struct ForwardForm {
	/// The currently-selected kind. Kept across a successful add — putting up several forwards of
	/// one kind is the common case — while the two fields below are cleared.
	pub kind: ForwardKind,
	/// The listen field (`port` or `host:port`).
	pub listen: String,
	/// The target field (`host:port`), unused for a Dynamic forward.
	pub to: String,
	/// The last add attempt's parse error; `None` when the form is clean.
	pub error: Option<String>,
}

/// Build the tunnels dialog card (§27). The body is the live-forwards list (or a "none yet"
/// line) above the add form; the footer is a single Close. Reuses the shared chrome, so it
/// drags, centres and dismisses like every other modal.
pub fn panel<'a>(
	entries: &'a [ForwardEntry],
	form: &'a ForwardForm,
	card: Card,
) -> Element<'a, Message> {
	let mut body = column![].spacing(12);

	if entries.is_empty() {
		body = body.push(
			text("No forwards yet. Add one below.")
				.size(BODY_SIZE)
				.color(MUTED_FG),
		);
	} else {
		let mut list = column![].spacing(6);
		for entry in entries {
			list = list.push(forward_row(entry));
		}
		body = body.push(list);
	}

	body = body.push(add_form(form));
	if let Some(error) = &form.error {
		body = body.push(text(error).size(BODY_SIZE).color(FAILED_FG));
	}

	dialog::dialog(
		"Port forwards".to_owned(),
		Message::ForwardsClosed,
		body.into(),
		vec![button("Close").on_press(Message::ForwardsClosed).into()],
		card,
	)
}

/// One forward's row: a status dot, its label, and a remove ✕ pinned right (§27).
fn forward_row(entry: &ForwardEntry) -> Element<'_, Message> {
	let (dot, dot_color, hint) = match &entry.status {
		ForwardStatus::Starting => ("○", MUTED_FG, None),
		ForwardStatus::Active => ("●", ACTIVE_FG, None),
		ForwardStatus::Failed(reason) => ("●", FAILED_FG, Some(reason.as_str())),
	};

	// The label (the entry's, not the spec's, so a `-R 0` shows the server-assigned port), plus a
	// muted sub-line: a failure reason when the forward could not start, or — for a live one — the
	// activity gauge showing the connections open now and carried in total.
	let mut labelled = column![text(entry.label()).size(BODY_SIZE).color(FG)].spacing(2);
	if let Some(reason) = hint {
		labelled = labelled.push(text(reason).size(BODY_SIZE - 2.0).color(FAILED_FG));
	} else if entry.status == ForwardStatus::Active {
		labelled = labelled.push(
			text(entry.activity_gauge())
				.size(BODY_SIZE - 2.0)
				.color(MUTED_FG),
		);
	}

	let remove = button(text("✕").size(BODY_SIZE))
		.padding(2)
		.on_press(Message::ForwardRemove(entry.id))
		.style(|_theme, _status| button::Style {
			background: None,
			text_color: MUTED_FG,
			..button::Style::default()
		});

	row![
		text(dot).size(BODY_SIZE).color(dot_color),
		container(labelled).width(Length::Fill),
		remove,
	]
	.spacing(8)
	.align_y(Vertical::Center)
	.into()
}

/// The add form (§27): a kind selector, a listen field, a target field (hidden for Dynamic, which
/// has no fixed target), and an Add button. Enter in either field also adds.
fn add_form(form: &ForwardForm) -> Element<'_, Message> {
	let selector = row![
		kind_button("Local", ForwardKind::Local, form.kind),
		kind_button("Remote", ForwardKind::Remote, form.kind),
		kind_button("Dynamic", ForwardKind::Dynamic, form.kind),
	]
	.spacing(6);

	let listen = text_input("listen (port or host:port)", &form.listen)
		.id(LISTEN_INPUT_ID)
		.on_input(Message::ForwardListenChanged)
		.on_submit(Message::ForwardAddPressed)
		.size(BODY_SIZE);

	let mut fields = column![selector, listen].spacing(8);
	// A Local/Remote forward names where the traffic goes; a Dynamic one lets each connection
	// choose, so its target field is replaced by a short note.
	if form.kind.has_target() {
		fields = fields.push(
			text_input("to (host:port)", &form.to)
				.on_input(Message::ForwardToChanged)
				.on_submit(Message::ForwardAddPressed)
				.size(BODY_SIZE),
		);
	} else {
		fields = fields.push(
			text("A SOCKS5 proxy — each connection picks its own target.")
				.size(BODY_SIZE - 2.0)
				.color(MUTED_FG),
		);
	}

	let add = button(text("Add forward").size(BODY_SIZE)).on_press(Message::ForwardAddPressed);

	// A faint rule above the form separates it from the list without a heavy divider.
	container(column![fields, add].spacing(10))
		.padding(iced::Padding {
			top: 12.0,
			right: 0.0,
			bottom: 0.0,
			left: 0.0,
		})
		.style(|_theme| container::Style {
			border: Border {
				color: Color::from_rgb8(0x50, 0x50, 0x50),
				width: 0.0,
				radius: 0.0.into(),
			},
			..container::Style::default()
		})
		.into()
}

/// One kind-selector button, tinted when it is the chosen kind (§27).
fn kind_button(label: &str, kind: ForwardKind, selected: ForwardKind) -> Element<'_, Message> {
	let is_selected = kind == selected;
	button(text(label).size(BODY_SIZE))
		.padding([4, 10])
		.on_press(Message::ForwardKindSelected(kind))
		.style(move |_theme, _status| button::Style {
			background: is_selected.then(|| SELECTED_BG.into()),
			text_color: FG,
			border: Border {
				color: MUTED_FG,
				width: 1.0,
				radius: 4.0.into(),
			},
			..button::Style::default()
		})
		.into()
}

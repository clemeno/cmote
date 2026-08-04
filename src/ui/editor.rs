// ui/editor.rs — the in-tab text editor's view (PLAN §32).
//
// A pure view over `editor::Editor`: a toolbar (path, encoding, line ending, dirty dot, Save / Save
// As / Close), then the buffer with a line-number gutter down its left. The model owns the text,
// the changed-line marks and the Save As prompt state; this file only draws them.
//
// The gutter trick (§32): iced's `text_editor` scrolls internally and hides its scroll offset, so a
// gutter placed beside it would desync the instant the text scrolled. Instead the editor is laid out
// with its height SHRUNK to the whole buffer (`text_editor` defaults to `Length::Shrink`) and
// `Wrapping::None`, so every logical line is exactly one row of `LINE_HEIGHT` and the editor never
// scrolls itself — one outer `scrollable` moves the gutter and the text together, and the numbers
// stay pixel-aligned with their lines by construction.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
	button, column, container, mouse_area, row, scrollable, stack, text, text_editor, text_input,
};
use iced::{Background, Border, Color, Element, Font, Length, Padding};

use crate::app::Message;
use crate::editor::{Editor, EditorMessage, Status};
use crate::ui::explorer::{FG, HEADER_BG, MUTED_FG, NOTICE_FG, PANEL_BG, SELECTED_BG};

/// The editor's monospace face — the terminal's bundled Fira Mono, so code lines up the same way it
/// does in the shell (§9, §32). Resolved by family name, like every bundled face.
const FONT: Font = Font::with_name("Fira Mono");

/// The text size and the pitch each line occupies. `LINE_HEIGHT` is set on BOTH the editor (as an
/// absolute line height) and every gutter row, so the numbers march in step with the text (§32).
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 20.0;

/// The toolbar's fixed height.
const TOOLBAR_HEIGHT: f32 = 34.0;

/// The gutter's changed-line bar: its width, and the amber that marks a line edited since load
/// (§32). The same amber tints the changed line's number and the dirty dot, so "unsaved" reads one
/// colour throughout.
const BAR_WIDTH: f32 = 3.0;
const CHANGED_MARK: Color = Color::from_rgb8(0xd0, 0xa0, 0x40);

/// The average advance of Fira Mono at `FONT_SIZE`, for sizing the gutter to its digit count — the
/// same estimate the files pane makes for its labels (§19).
const DIGIT_WIDTH: f32 = 8.0;
/// The gutter's padding either side of the numbers.
const GUTTER_PAD: f32 = 8.0;

/// The widget id of the Save As path field, so `app` can focus it the instant the prompt opens (the
/// same discipline as the rename field, §18).
pub const SAVE_AS_INPUT_ID: &str = "editor-save-as";

/// The whole editor screen for one tab (§32): the toolbar over the buffer (or the loading / failed
/// message), with the Save As prompt floated on top when it is open. Borrows the editor for the
/// lifetime of the returned element, since `text_editor` reads the buffer in place.
pub fn view(editor: &Editor, tab_id: u64) -> Element<'_, Message> {
	let body: Element<'_, Message> = match &editor.status {
		Status::Loading => centered(text("Loading…").size(15).color(MUTED_FG).into()),
		Status::Failed(reason) => failed_body(reason, tab_id),
		Status::Ready => buffer_body(editor),
	};

	let screen = column![toolbar(editor, tab_id), body]
		.width(Length::Fill)
		.height(Length::Fill);

	// The Save As prompt floats over the buffer with a click-away backdrop (§32).
	match &editor.save_as {
		Some(path) => stack![
			screen,
			mouse_area(dim_fill()).on_press(Message::Editor(EditorMessage::SaveAsCancel)),
			centered(save_as_card(path)),
		]
		.width(Length::Fill)
		.height(Length::Fill)
		.into(),
		None => screen.into(),
	}
}

/// The toolbar: the path (with a dirty dot when unsaved), the encoding and line ending, any notice,
/// and the Save / Save As / Close buttons (§32).
fn toolbar(editor: &Editor, tab_id: u64) -> Element<'_, Message> {
	let ready = matches!(editor.status, Status::Ready);
	let dirty = editor.is_dirty();
	let dot = if dirty { "• " } else { "" };
	let title = text(format!("{dot}{}", editor.path)).size(13).color(FG);

	// The right-hand info cluster: encoding, line ending, and whatever transient state applies.
	let mut info = row![
		badge(editor.encoding.label()),
		badge(editor.line_ending_label()),
	]
	.spacing(10)
	.align_y(Vertical::Center);
	if editor.saving {
		info = info.push(text("Saving…").size(11).color(MUTED_FG));
	}
	if editor.parent_gone {
		info = info.push(
			text("session closed — cannot save")
				.size(11)
				.color(NOTICE_FG),
		);
	} else if let Some(notice) = &editor.notice {
		info = info.push(text(notice.clone()).size(11).color(NOTICE_FG));
	}

	let can_save = dirty && !editor.saving && !editor.parent_gone && ready;
	let can_save_as = ready && !editor.parent_gone && !editor.saving;
	let buttons = row![
		tool_button("Save", Message::Editor(EditorMessage::Save), can_save),
		tool_button(
			"Save As…",
			Message::Editor(EditorMessage::SaveAsStart),
			can_save_as
		),
		// The same ✕ that closes a dialog (§10), so "close this" is one icon app-wide.
		crate::ui::dialog::close_button(Message::TabCloseRequested(tab_id)),
	]
	.spacing(6)
	.align_y(Vertical::Center);

	container(
		row![container(title).width(Length::Fill), info, buttons,]
			.spacing(14)
			.align_y(Vertical::Center),
	)
	.width(Length::Fill)
	.height(Length::Fixed(TOOLBAR_HEIGHT))
	.align_y(Vertical::Center)
	.padding(Padding::from([0.0, 10.0]))
	.style(|_theme| container::Style {
		background: Some(HEADER_BG.into()),
		..container::Style::default()
	})
	.into()
}

/// The buffer with its line-number gutter, both inside one vertical `scrollable` so they scroll in
/// lockstep (§32).
fn buffer_body(editor: &Editor) -> Element<'_, Message> {
	let editor_widget = text_editor(&editor.content)
		.on_action(|action| Message::Editor(EditorMessage::Action(action)))
		.font(FONT)
		.size(FONT_SIZE)
		.line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
		.wrapping(Wrapping::None)
		.padding(Padding::from([0.0, 8.0]))
		.height(Length::Shrink)
		.style(|_theme, _status| text_editor::Style {
			background: Background::Color(PANEL_BG),
			border: Border::default(),
			placeholder: MUTED_FG,
			value: FG,
			selection: SELECTED_BG,
		});

	let content = row![gutter(editor), editor_widget]
		.width(Length::Fill)
		.height(Length::Shrink);

	container(scrollable(content).width(Length::Fill).height(Length::Fill))
		.style(|_theme| container::Style {
			background: Some(PANEL_BG.into()),
			..container::Style::default()
		})
		.into()
}

/// The gutter: one right-aligned number per line, an amber bar and amber number on lines changed
/// since load (§32). Each row is exactly `LINE_HEIGHT`, matching the editor's absolute line height so
/// the two stay aligned. (`ponytail:` one widget per line — fine for the config-and-script files this
/// is for; a many-thousand-line file would want a drawn gutter, the same bound the buffer's
/// laid-out-every-frame layout already carries, §32.)
fn gutter(editor: &Editor) -> Element<'_, Message> {
	let count = editor.content.line_count().max(1);
	let changed = editor.changed();
	let width = (count.to_string().len() as f32) * DIGIT_WIDTH + BAR_WIDTH + GUTTER_PAD * 2.0;

	let mut rows: Vec<Element<'_, Message>> = Vec::with_capacity(count);
	for index in 0..count {
		let is_changed = changed.get(index).copied().unwrap_or(false);
		let number = text(format!("{}", index + 1))
			.font(FONT)
			.size(FONT_SIZE)
			.color(if is_changed { CHANGED_MARK } else { MUTED_FG });
		let bar = container(text(""))
			.width(Length::Fixed(BAR_WIDTH))
			.height(Length::Fill)
			.style(move |_theme| container::Style {
				background: is_changed.then(|| CHANGED_MARK.into()),
				..container::Style::default()
			});
		let line = row![
			bar,
			container(number)
				.width(Length::Fill)
				.align_x(Horizontal::Right),
		]
		.spacing(GUTTER_PAD)
		.align_y(Vertical::Center);
		rows.push(
			container(line)
				.width(Length::Fill)
				.height(Length::Fixed(LINE_HEIGHT))
				.padding(Padding {
					top: 0.0,
					right: GUTTER_PAD,
					bottom: 0.0,
					left: 0.0,
				})
				.into(),
		);
	}

	container(column(rows))
		.width(Length::Fixed(width))
		.style(|_theme| container::Style {
			background: Some(HEADER_BG.into()),
			..container::Style::default()
		})
		.into()
}

/// The message shown in place of the buffer when the file cannot be opened — too big, binary, or an
/// unsupported encoding (§32) — with only a Close.
fn failed_body(reason: &str, tab_id: u64) -> Element<'_, Message> {
	centered(
		column![
			text("This file cannot be opened in the editor.")
				.size(15)
				.color(NOTICE_FG),
			text(reason.to_owned()).size(13).color(FG),
			tool_button("Close", Message::TabCloseRequested(tab_id), true),
		]
		.spacing(14)
		.align_x(Horizontal::Center)
		.into(),
	)
}

/// The Save As prompt card: a remote-path field pre-filled with the current path, and Save / Cancel
/// (§32). Enter in the field confirms, so it saves without reaching for the mouse.
fn save_as_card(path: &str) -> Element<'static, Message> {
	let field = text_input("remote path", path)
		.id(SAVE_AS_INPUT_ID)
		.on_input(|value| Message::Editor(EditorMessage::SaveAsChanged(value)))
		.on_submit(Message::Editor(EditorMessage::SaveAsConfirm))
		.padding(6)
		.size(13);
	let footer = row![
		tool_button("Cancel", Message::Editor(EditorMessage::SaveAsCancel), true),
		tool_button("Save", Message::Editor(EditorMessage::SaveAsConfirm), true),
	]
	.spacing(8);

	let card = container(
		column![
			text("Save as — remote path").size(14).color(FG),
			field,
			footer,
		]
		.spacing(12),
	)
	.width(Length::Fixed(420.0))
	.padding(16)
	.style(|_theme| container::Style {
		background: Some(HEADER_BG.into()),
		border: Border {
			radius: 6.0.into(),
			width: 1.0,
			color: SELECTED_BG,
		},
		..container::Style::default()
	});

	// Swallow clicks on the card so they do not reach the dismiss backdrop beneath it.
	mouse_area(card).on_press(Message::Ignored).into()
}

/// A small dimmed toolbar chip for the encoding / line-ending readout.
fn badge(label: &str) -> Element<'static, Message> {
	text(label.to_owned()).size(11).color(MUTED_FG).into()
}

/// One toolbar button, enabled or greyed. A disabled button carries no `on_press`, so iced makes it
/// inert — the same idiom the panels use for an inapplicable action (§19).
fn tool_button(label: &str, message: Message, enabled: bool) -> Element<'static, Message> {
	let mut widget =
		button(
			text(label.to_owned())
				.size(12)
				.color(if enabled { FG } else { MUTED_FG }),
		)
		.padding(Padding::from([4.0, 10.0]))
		.style(|_theme, _status| button::Style {
			background: Some(PANEL_BG.into()),
			text_color: FG,
			border: Border {
				radius: 4.0.into(),
				..Border::default()
			},
			..button::Style::default()
		});
	if enabled {
		widget = widget.on_press(message);
	}
	widget.into()
}

/// Centre a single element in the remaining space — the loading / failed message and the Save As
/// card. The wrapping container captures no clicks (it is not a `mouse_area`), so a press that
/// misses the card still reaches the backdrop below it in the stack.
fn centered(inner: Element<'_, Message>) -> Element<'_, Message> {
	container(inner)
		.width(Length::Fill)
		.height(Length::Fill)
		.align_x(Horizontal::Center)
		.align_y(Vertical::Center)
		.into()
}

/// A full-area dimming layer for the Save As backdrop.
fn dim_fill() -> Element<'static, Message> {
	container(text(""))
		.width(Length::Fill)
		.height(Length::Fill)
		.style(|_theme| container::Style {
			background: Some(Color::from_rgba(0.0, 0.0, 0.0, 0.45).into()),
			..container::Style::default()
		})
		.into()
}

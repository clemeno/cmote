// ui/editor.rs — the in-tab text editor's view (PLAN §32).
//
// A pure view over `editor::Editor`: a toolbar (path, encoding, line ending, dirty dot, a theme
// select, Save / Save As / Close), then the buffer with a line-number gutter down its left. The
// model owns the text, the changed-line marks, the Save As prompt state and the chosen theme; this
// file only draws them.
//
// Theme (§32): the editor paints in one of two schemes, chosen per tab and remembered per file
// extension by `App`. A tab's `EditorTheme` resolves here to a small `Palette`, and every drawing
// helper takes a `&Palette` rather than reaching for a global colour — so two editor tabs can wear
// different schemes at once. "Default" is cmote's own dark panels; "CME" is the user's VS Code theme
// (Themer My Color Set Dark), ported from its `editor.*` colours so a file reads much as it does
// there.
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
use crate::editor::{Editor, EditorMessage, EditorTheme, Status};
use crate::ui::explorer::{FG, HEADER_BG, MUTED_FG, NOTICE_FG, PANEL_BG, SELECTED_BG};

/// The editor's monospace face — the terminal's bundled Fira Mono, so code lines up the same way it
/// does in the shell (§9, §32). Resolved by family name, like every bundled face.
const FONT: Font = Font::with_name("Fira Mono");

/// The text size and the pitch each line occupies. `LINE_HEIGHT` is set on BOTH the editor (as an
/// absolute line height) and every gutter row, so the numbers march in step with the text (§32). It
/// is `pub` because `App`'s cursor-follow multiplies it by the cursor line to find that line's top.
const FONT_SIZE: f32 = 13.0;
pub const LINE_HEIGHT: f32 = 20.0;

/// The widget id of the buffer's outer scrollable, so `App` can scroll the cursor line into view
/// after a move (§32) — the same discipline the files pane and folder tree use for their scrollables.
pub const BUFFER_SCROLL_ID: &str = "editor-buffer";

/// The Y offset of a line's top within the buffer (§32): every logical line is exactly `LINE_HEIGHT`
/// tall (the editor is laid out at `Wrapping::None`), so a line's top is just its index times the
/// pitch. `App`'s cursor-follow feeds this to `keep_visible`.
pub fn line_top(line: usize) -> f32 {
	line as f32 * LINE_HEIGHT
}

/// The toolbar's fixed height.
const TOOLBAR_HEIGHT: f32 = 34.0;

/// The gutter's changed-line bar width (§32). Its colour is the palette's `changed`.
const BAR_WIDTH: f32 = 3.0;

/// The amber that marks a changed line in the Default scheme (§32) — the palette's `changed` there.
/// The CME scheme swaps in its own change colour.
const CHANGED_MARK: Color = Color::from_rgb8(0xd0, 0xa0, 0x40);

/// The average advance of Fira Mono at `FONT_SIZE`, for sizing the gutter to its digit count — the
/// same estimate the files pane makes for its labels (§19).
const DIGIT_WIDTH: f32 = 8.0;
/// The gutter's padding either side of the numbers.
const GUTTER_PAD: f32 = 8.0;

/// The widget id of the Save As path field, so `app` can focus it the instant the prompt opens (the
/// same discipline as the rename field, §18).
pub const SAVE_AS_INPUT_ID: &str = "editor-save-as";

/// The widget id of the find bar's query field, so Ctrl+F can focus it the instant the bar opens
/// (§32) — the same discipline as the Save As field.
pub const FIND_INPUT_ID: &str = "editor-find";

/// The widget id of the find bar's replace field (§32).
pub const REPLACE_INPUT_ID: &str = "editor-replace";

/// The colours one editor paints with (§32), resolved from the tab's `EditorTheme`. Every drawing
/// helper below takes a `&Palette` rather than a global, so a tab can differ from its neighbour.
struct Palette {
	/// The toolbar and gutter fill.
	chrome_bg: Color,
	/// The editing surface behind the text.
	buffer_bg: Color,
	/// The main foreground — the path and the buffer text.
	fg: Color,
	/// Dimmed foreground — badges, line numbers, a disabled button, an inactive theme option.
	muted: Color,
	/// The highlight behind selected text.
	selection: Color,
	/// A changed-since-load line's bar and number.
	changed: Color,
	/// A transient warning — a save failure, the closed-session note, the "cannot open" heading.
	notice: Color,
	/// A toolbar button's fill.
	button_bg: Color,
	/// The fill of the theme select's active option and the Save As card's border.
	accent: Color,
	/// The translucent band behind the current find match's line, and the fill of that line's gutter
	/// cell (§32) — so a match shows even while the find field, not the buffer, holds focus.
	match_line: Color,
}

/// Resolve a theme to its palette (§32).
fn palette(theme: EditorTheme) -> Palette {
	match theme {
		// cmote's own dark panels — the same family as the files pane and the dialogs.
		EditorTheme::Default => Palette {
			chrome_bg: HEADER_BG,
			buffer_bg: PANEL_BG,
			fg: FG,
			muted: MUTED_FG,
			selection: SELECTED_BG,
			changed: CHANGED_MARK,
			notice: NOTICE_FG,
			button_bg: PANEL_BG,
			accent: SELECTED_BG,
			match_line: Color::from_rgba8(0x4a, 0x90, 0xd0, 0.20), // a soft blue find-line wash
		},
		// "CME": Themer My Color Set Dark. Each value is that theme's own `editor.*` / `editorGutter.*`
		// colour, so the buffer reads like the same file in VS Code — dark-teal ground, white text, a
		// light-blue change marker, a faint cyan selection, an orange warning tint.
		EditorTheme::Cme => Palette {
			chrome_bg: Color::from_rgb8(0x20, 0x30, 0x3c), // editorGutter.background
			buffer_bg: Color::from_rgb8(0x1a, 0x2a, 0x30), // editor.background
			fg: Color::from_rgb8(0xff, 0xff, 0xff),        // editor.foreground
			muted: Color::from_rgb8(0x60, 0x6b, 0x74),     // editorLineNumber.foreground
			selection: Color::from_rgba8(0x00, 0xcc, 0xff, 0.2), // editor.selectionBackground #00CCFF33
			changed: Color::from_rgb8(0xaa, 0xdd, 0xff),   // editorGutter.modifiedBackground
			notice: Color::from_rgb8(0xff, 0x66, 0x00),    // errorForeground
			button_bg: Color::from_rgb8(0x40, 0x4e, 0x58), // editorWidget.background
			accent: Color::from_rgb8(0x00, 0x66, 0x88),    // the user's teal accent (bracket guide)
			match_line: Color::from_rgba8(0x00, 0xcc, 0xff, 0.16), // a faint cyan find-line wash
		},
	}
}

/// The whole editor screen for one tab (§32): the toolbar over the buffer (or the loading / failed
/// message), with the Save As prompt floated on top when it is open. Borrows the editor for the
/// lifetime of the returned element, since `text_editor` reads the buffer in place.
pub fn view(editor: &Editor, tab_id: u64) -> Element<'_, Message> {
	let p = palette(editor.theme);
	let body: Element<'_, Message> = match &editor.status {
		Status::Loading => centered(text("Loading…").size(15).color(p.muted).into()),
		Status::Failed(reason) => failed_body(reason, tab_id, &p),
		Status::Ready => buffer_body(editor, &p),
	};

	let screen = column![toolbar(editor, tab_id, &p), body]
		.width(Length::Fill)
		.height(Length::Fill);

	// The Save As prompt floats over the buffer with a click-away backdrop (§32).
	match &editor.save_as {
		Some(path) => stack![
			screen,
			mouse_area(dim_fill()).on_press(Message::Editor(EditorMessage::SaveAsCancel)),
			centered(save_as_card(path, &p)),
		]
		.width(Length::Fill)
		.height(Length::Fill)
		.into(),
		None => screen.into(),
	}
}

/// The toolbar: the path (with a dirty dot when unsaved), the encoding and line ending, any notice,
/// the theme select, and the Save / Save As / Close buttons (§32).
fn toolbar<'a>(editor: &'a Editor, tab_id: u64, p: &Palette) -> Element<'a, Message> {
	let ready = matches!(editor.status, Status::Ready);
	let dirty = editor.is_dirty();
	let dot = if dirty { "• " } else { "" };
	let title = text(format!("{dot}{}", editor.path)).size(13).color(p.fg);

	// The right-hand info cluster: encoding, line ending, and whatever transient state applies.
	let mut info = row![
		badge(editor.encoding.label(), p),
		badge(editor.line_ending_label(), p),
	]
	.spacing(10)
	.align_y(Vertical::Center);
	if editor.saving {
		info = info.push(text("Saving…").size(11).color(p.muted));
	}
	if editor.parent_gone {
		info = info.push(
			text("session closed — cannot save")
				.size(11)
				.color(p.notice),
		);
	} else if let Some(notice) = &editor.notice {
		info = info.push(text(notice.clone()).size(11).color(p.notice));
	}

	let can_save = dirty && !editor.saving && !editor.parent_gone && ready;
	let can_save_as = ready && !editor.parent_gone && !editor.saving;
	let buttons = row![
		tool_button("Save", Message::Editor(EditorMessage::Save), can_save, p),
		tool_button(
			"Save As…",
			Message::Editor(EditorMessage::SaveAsStart),
			can_save_as,
			p
		),
		// The same ✕ that closes a dialog (§10), so "close this" is one icon app-wide.
		crate::ui::dialog::close_button(Message::TabCloseRequested(tab_id)),
	]
	.spacing(6)
	.align_y(Vertical::Center);

	let bg = p.chrome_bg;
	container(
		row![
			container(title).width(Length::Fill),
			info,
			theme_select(editor.theme, p),
			buttons,
		]
		.spacing(14)
		.align_y(Vertical::Center),
	)
	.width(Length::Fill)
	.height(Length::Fixed(TOOLBAR_HEIGHT))
	.align_y(Vertical::Center)
	.padding(Padding::from([0.0, 10.0]))
	.style(move |_theme| container::Style {
		background: Some(bg.into()),
		..container::Style::default()
	})
	.into()
}

/// The toolbar's two-option theme select (§32): a tiny segmented control, the active scheme filled
/// with the palette's accent. Each option posts an App-level `EditorThemeSelected`, which repaints
/// this editor and remembers the choice for the file's extension.
fn theme_select(current: EditorTheme, p: &Palette) -> Element<'static, Message> {
	row![
		theme_option(EditorTheme::Default, current, p),
		theme_option(EditorTheme::Cme, current, p),
	]
	.spacing(4)
	.align_y(Vertical::Center)
	.into()
}

/// One option in the theme select — filled with the accent and brightened when it is the current
/// scheme, dim and un-filled otherwise (§32).
fn theme_option(
	theme: EditorTheme,
	current: EditorTheme,
	p: &Palette,
) -> Element<'static, Message> {
	let active = theme == current;
	let bg = if active { p.accent } else { p.button_bg };
	let fg = if active { p.fg } else { p.muted };
	button(text(theme.label().to_owned()).size(11).color(fg))
		.padding(Padding::from([3.0, 8.0]))
		.on_press(Message::EditorThemeSelected(theme))
		.style(move |_theme, _status| button::Style {
			background: Some(bg.into()),
			text_color: fg,
			border: Border {
				radius: 3.0.into(),
				..Border::default()
			},
			..button::Style::default()
		})
		.into()
}

/// The buffer with its line-number gutter, both inside one vertical `scrollable` so they scroll in
/// lockstep (§32).
fn buffer_body<'a>(editor: &'a Editor, p: &Palette) -> Element<'a, Message> {
	let bg = p.buffer_bg;
	let fg = p.fg;
	let muted = p.muted;
	let selection = p.selection;
	let editor_widget = text_editor(&editor.content)
		.on_action(|action| Message::Editor(EditorMessage::Action(action)))
		.font(FONT)
		.size(FONT_SIZE)
		.line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
		.wrapping(Wrapping::None)
		.padding(Padding::from([0.0, 8.0]))
		.height(Length::Shrink)
		.style(move |_theme, _status| text_editor::Style {
			// Transparent, so the current-match band drawn behind the text shows through (§32). The
			// buffer's own fill is the enclosing container's `buffer_bg`.
			background: Background::Color(Color::TRANSPARENT),
			border: Border::default(),
			placeholder: muted,
			value: fg,
			selection,
		});

	// CME turns on syntax highlighting (§32): syntect parses each line and our CME-derived theme
	// colours the scopes. A token the CME theme leaves alone keeps the buffer's own `value` colour,
	// so the highlight sits over the flat scheme rather than replacing it. Default stays plain.
	let editor_element: Element<'a, Message> = if matches!(editor.theme, EditorTheme::Cme) {
		editor_widget
			.highlight_with::<crate::ui::syntax::Highlighter>(
				crate::ui::syntax::Settings {
					// Widen past the bare extension so a whole-name file (Makefile, .bashrc) or an
					// extensionless shebang script highlights too (§32); the resolved grammar NAME is the
					// identity, so a normal file's highlighter is not rebuilt when its first line is edited.
					grammar: crate::ui::syntax::resolve_syntax(
						crate::editor::file_name(&editor.path),
						&editor.first_line(),
					)
					.name
					.clone(),
				},
				|highlight, _theme| highlight.to_format(),
			)
			.into()
	} else {
		editor_widget.into()
	};

	let body = row![gutter(editor, p), editor_element]
		.width(Length::Fill)
		.height(Length::Shrink);

	// A translucent band behind the current find match's line (§32), so the match is visible even while
	// the find field holds focus (iced paints the buffer's own selection only when the buffer itself is
	// focused). It rides BEHIND the text in a `stack` so the glyphs draw over it; the gutter, being
	// opaque, hides the band on its side, leaving it to wash only the text column. Three fixed spacers,
	// not one widget per line — the band is a single row, wherever it sits.
	let line_count = editor.content.line_count().max(1);
	let content: Element<'a, Message> = match editor.find_match_line() {
		Some(line) if line < line_count => {
			stack![line_band(line_count, line, p.match_line), body].into()
		}
		_ => body.into(),
	};

	// One outer vertical scrollable moves the gutter and the text together (§32). It reports its
	// offset and visible height on every scroll and on the first frame, which is what lets `App`
	// scroll the cursor line into view after a move without tracking the widget's hidden offset.
	let scroller = scrollable(content)
		.id(BUFFER_SCROLL_ID)
		.width(Length::Fill)
		.height(Length::Fill)
		.on_scroll(|viewport| {
			Message::Editor(EditorMessage::Scrolled {
				offset: viewport.absolute_offset().y,
				view_height: viewport.bounds().height,
			})
		});
	let buffer: Element<'a, Message> = container(scroller)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(move |_theme| container::Style {
			background: Some(bg.into()),
			..container::Style::default()
		})
		.into();

	// The find bar rides above the buffer while it is open (§32), pushing the text down rather than
	// floating over it — so a match near the top is never hidden behind the bar.
	match &editor.find {
		Some(find) => column![find_bar(find, p), buffer]
			.width(Length::Fill)
			.height(Length::Fill)
			.into(),
		None => buffer,
	}
}

/// The translucent band behind one line of the buffer (§32) — the current find match's line. Built
/// from three fixed spacers (the lines above, the band itself, the lines below), so it is three
/// widgets whatever the file's length, and its total height is `count × LINE_HEIGHT`, matching the
/// gutter and the text exactly so the band lands on its line by construction.
fn line_band<'a>(count: usize, line: usize, color: Color) -> Element<'a, Message> {
	let above = line as f32 * LINE_HEIGHT;
	let below = count.saturating_sub(line + 1) as f32 * LINE_HEIGHT;
	let spacer = |height: f32| {
		container(text(""))
			.width(Length::Fill)
			.height(Length::Fixed(height))
	};
	column![
		spacer(above),
		spacer(LINE_HEIGHT).style(move |_theme| container::Style {
			background: Some(color.into()),
			..container::Style::default()
		}),
		spacer(below),
	]
	.width(Length::Fill)
	.into()
}

/// The find / replace bar (§32): a query field with a live match count and prev / next steppers, a
/// toggle for the replace row, and a close ✕. When the replace row is shown it adds a replacement
/// field with Replace (this match) and All. Enter in the query steps to the next match; Enter in the
/// replacement replaces the current one — so the common flow needs no mouse.
fn find_bar<'a>(find: &'a crate::editor::Find, p: &Palette) -> Element<'a, Message> {
	let query = text_input("Find", &find.query)
		.id(FIND_INPUT_ID)
		.on_input(|value| Message::Editor(EditorMessage::FindQueryChanged(value)))
		.on_submit(Message::Editor(EditorMessage::FindStep(true)))
		.padding(Padding::from([3.0, 8.0]))
		.size(FONT_SIZE)
		.width(Length::Fixed(220.0));

	// The count: "3 / 12", "No results" once a query has none, or blank while the bar is idle.
	let has_hits = find.count() > 0;
	let count_label = if find.query.is_empty() {
		String::new()
	} else if has_hits {
		format!("{} / {}", find.ordinal(), find.count())
	} else {
		"No results".to_owned()
	};
	let count = text(count_label)
		.size(11)
		.color(p.muted)
		.width(Length::Fixed(72.0));

	let first_row = row![
		query,
		count,
		tool_button(
			"‹",
			Message::Editor(EditorMessage::FindStep(false)),
			has_hits,
			p
		),
		tool_button(
			"›",
			Message::Editor(EditorMessage::FindStep(true)),
			has_hits,
			p
		),
		tool_button(
			if find.replace_open {
				"Replace ▲"
			} else {
				"Replace ▼"
			},
			Message::Editor(EditorMessage::ReplaceToggle),
			true,
			p,
		),
		container(text("")).width(Length::Fill),
		crate::ui::dialog::close_button(Message::Editor(EditorMessage::FindClose)),
	]
	.spacing(6)
	.align_y(Vertical::Center);

	let mut stack = column![first_row].spacing(6);
	if find.replace_open {
		let replace = text_input("Replace with", &find.replace)
			.id(REPLACE_INPUT_ID)
			.on_input(|value| Message::Editor(EditorMessage::ReplaceChanged(value)))
			.on_submit(Message::Editor(EditorMessage::ReplaceOne))
			.padding(Padding::from([3.0, 8.0]))
			.size(FONT_SIZE)
			.width(Length::Fixed(220.0));
		stack = stack.push(
			row![
				replace,
				tool_button(
					"Replace",
					Message::Editor(EditorMessage::ReplaceOne),
					has_hits,
					p
				),
				tool_button(
					"All",
					Message::Editor(EditorMessage::ReplaceAll),
					has_hits,
					p
				),
			]
			.spacing(6)
			.align_y(Vertical::Center),
		);
	}

	let bg = p.chrome_bg;
	container(stack)
		.width(Length::Fill)
		.padding(Padding::from([6.0, 10.0]))
		.style(move |_theme| container::Style {
			background: Some(bg.into()),
			..container::Style::default()
		})
		.into()
}

/// The gutter: one right-aligned number per line, a bar and the palette's change colour on lines
/// changed since load (§32). Each row is exactly `LINE_HEIGHT`, matching the editor's absolute line
/// height so the two stay aligned. (`ponytail:` one widget per line — fine for the config-and-script
/// files this is for; a many-thousand-line file would want a drawn gutter, the same bound the
/// buffer's laid-out-every-frame layout already carries, §32.)
fn gutter<'a>(editor: &'a Editor, p: &Palette) -> Element<'a, Message> {
	let count = editor.content.line_count().max(1);
	let changed = editor.changed();
	let match_line = editor.find_match_line();
	let width = (count.to_string().len() as f32) * DIGIT_WIDTH + BAR_WIDTH + GUTTER_PAD * 2.0;
	let mark = p.changed;
	let muted = p.muted;
	let match_fg = p.fg;
	let match_bg = p.match_line;

	let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(count);
	for index in 0..count {
		let is_changed = changed.get(index).copied().unwrap_or(false);
		let is_match = Some(index) == match_line;
		// The current match's number is the bright foreground on the same wash the buffer line wears;
		// a changed line keeps its amber; every other number is dimmed.
		let number_color = if is_match {
			match_fg
		} else if is_changed {
			mark
		} else {
			muted
		};
		let number = text(format!("{}", index + 1))
			.font(FONT)
			.size(FONT_SIZE)
			.color(number_color);
		let bar = container(text(""))
			.width(Length::Fixed(BAR_WIDTH))
			.height(Length::Fill)
			.style(move |_theme| container::Style {
				background: is_changed.then(|| mark.into()),
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
				.style(move |_theme| container::Style {
					background: is_match.then(|| match_bg.into()),
					..container::Style::default()
				})
				.into(),
		);
	}

	let bg = p.chrome_bg;
	container(column(rows))
		.width(Length::Fixed(width))
		.style(move |_theme| container::Style {
			background: Some(bg.into()),
			..container::Style::default()
		})
		.into()
}

/// The message shown in place of the buffer when the file cannot be opened — too big, binary, or an
/// unsupported encoding (§32) — with only a Close.
fn failed_body(reason: &str, tab_id: u64, p: &Palette) -> Element<'static, Message> {
	centered(
		column![
			text("This file cannot be opened in the editor.")
				.size(15)
				.color(p.notice),
			text(reason.to_owned()).size(13).color(p.fg),
			tool_button("Close", Message::TabCloseRequested(tab_id), true, p),
		]
		.spacing(14)
		.align_x(Horizontal::Center)
		.into(),
	)
}

/// The Save As prompt card: a remote-path field pre-filled with the current path, and Save / Cancel
/// (§32). Enter in the field confirms, so it saves without reaching for the mouse.
fn save_as_card(path: &str, p: &Palette) -> Element<'static, Message> {
	let field = text_input("remote path", path)
		.id(SAVE_AS_INPUT_ID)
		.on_input(|value| Message::Editor(EditorMessage::SaveAsChanged(value)))
		.on_submit(Message::Editor(EditorMessage::SaveAsConfirm))
		.padding(6)
		.size(13);
	let footer = row![
		tool_button(
			"Cancel",
			Message::Editor(EditorMessage::SaveAsCancel),
			true,
			p
		),
		tool_button(
			"Save",
			Message::Editor(EditorMessage::SaveAsConfirm),
			true,
			p
		),
	]
	.spacing(8);

	let bg = p.chrome_bg;
	let border = p.accent;
	let card = container(
		column![
			text("Save as — remote path").size(14).color(p.fg),
			field,
			footer,
		]
		.spacing(12),
	)
	.width(Length::Fixed(420.0))
	.padding(16)
	.style(move |_theme| container::Style {
		background: Some(bg.into()),
		border: Border {
			radius: 6.0.into(),
			width: 1.0,
			color: border,
		},
		..container::Style::default()
	});

	// Swallow clicks on the card so they do not reach the dismiss backdrop beneath it.
	mouse_area(card).on_press(Message::Ignored).into()
}

/// A small dimmed toolbar chip for the encoding / line-ending readout.
fn badge(label: &str, p: &Palette) -> Element<'static, Message> {
	text(label.to_owned()).size(11).color(p.muted).into()
}

/// One toolbar button, enabled or greyed. A disabled button carries no `on_press`, so iced makes it
/// inert — the same idiom the panels use for an inapplicable action (§19).
fn tool_button(
	label: &str,
	message: Message,
	enabled: bool,
	p: &Palette,
) -> Element<'static, Message> {
	let fg = p.fg;
	let muted = p.muted;
	let bg = p.button_bg;
	let mut widget =
		button(
			text(label.to_owned())
				.size(12)
				.color(if enabled { fg } else { muted }),
		)
		.padding(Padding::from([4.0, 10.0]))
		.style(move |_theme, _status| button::Style {
			background: Some(bg.into()),
			text_color: fg,
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

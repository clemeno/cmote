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
// scrolls itself vertically — a `Direction::Both` `scrollable` moves the text, and the gutter, drawn
// beside it, is not in that scrollable at all: it is a `pin` translated by the reported scroll offset,
// so its numbers stay pixel-aligned with their lines by construction, with zero sync lag.
//
// The horizontal mirror (§32): the same widget hides its HORIZONTAL offset too, so a scrollbar synced
// to it is impossible for the same reason. So the editor is given an explicit fixed WIDTH — its widest
// line, from `content_columns` × the character advance — exactly as its height is the whole buffer, so
// it never scrolls itself horizontally either; the `Both` scrollable supplies the visible horizontal
// bar and the wheel, and a horizontal cursor-follow (driven by `App`, the mirror of the vertical one)
// keeps the cursor column on screen after a move — the last long-line gap closed.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::scrollable::{Direction, Scrollbar};
use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
	button, column, container, mouse_area, pin, row, scrollable, stack, text, text_editor,
	text_input,
};
use iced::{Background, Border, Color, Element, Font, Length, Padding};

use crate::app::Message;
use crate::editor::{Editor, EditorMessage, EditorStatus, EditorTheme};
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

/// Fira Mono's fixed advance at `FONT_SIZE` — 0.6 em, the exact monospace pitch the terminal grid also
/// relies on (§9, §11). The buffer's horizontal scroll extent and the horizontal cursor-follow both
/// scale display columns by it. `pub` because `App`'s cursor-follow uses it as the cursor's "width".
pub const CHAR_ADVANCE: f32 = FONT_SIZE * 0.6;

/// The buffer text's left/right padding — the `[0.0, 8.0]` set on the `text_editor` (§32). The content
/// width adds it on both sides; the cursor-follow adds the left one to a column's x.
const TEXT_PAD_X: f32 = 8.0;

/// The buffer's natural pixel width for a given widest-line column count (§32) — the width the
/// `text_editor` is laid out at so it never scrolls itself horizontally. Both paddings plus one extra
/// advance of slack, so the cursor sitting just past the last glyph of the longest line is reachable.
pub fn content_width(cols: usize) -> f32 {
	cols as f32 * CHAR_ADVANCE + TEXT_PAD_X * 2.0 + CHAR_ADVANCE
}

/// The X offset of a display column's left edge within the buffer widget (§32) — the horizontal
/// counterpart of `line_top`. `App`'s horizontal cursor-follow feeds this to `keep_visible`.
pub fn col_x(col: usize) -> f32 {
	col as f32 * CHAR_ADVANCE + TEXT_PAD_X
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

/// How many extra rows the virtualised gutter materialises above and below the visible window (§32).
/// A small cushion so a scroll that lands between two frames never flashes an un-numbered edge; `view`
/// reruns every frame with the fresh offset, so a few rows are plenty.
const GUTTER_OVERSCAN: usize = 4;

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
		EditorStatus::Loading => centered(text("Loading…").size(15).color(p.muted).into()),
		EditorStatus::Failed(reason) => failed_body(reason, tab_id, &p),
		EditorStatus::Ready => buffer_body(editor, &p),
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
	let ready = matches!(editor.status, EditorStatus::Ready);
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

/// The buffer with its pinned line-number gutter beside a horizontally- and vertically-scrollable
/// text column (§32). The gutter rides the reported vertical offset (a `pin`, not the scrollable), so
/// it stays in lockstep with the text; the text is laid out at its full content width so the `Both`
/// scrollable supplies the horizontal bar.
fn buffer_body<'a>(editor: &'a Editor, p: &Palette) -> Element<'a, Message> {
	let bg = p.buffer_bg;
	let fg = p.fg;
	let muted = p.muted;
	let selection = p.selection;
	// Lay the editor out exactly as wide as its widest line, but never narrower than the viewport so a
	// short file still fills the pane and a click past a line's end still lands on it (§32). At its
	// content width the widget never scrolls itself — the outer `Both` scrollable does.
	let content_px = content_width(editor.content_columns()).max(editor.view_width());
	let editor_widget = text_editor(&editor.content)
		.on_action(|action| Message::Editor(EditorMessage::Action(action)))
		.font(FONT)
		.size(FONT_SIZE)
		.line_height(LineHeight::Absolute(LINE_HEIGHT.into()))
		.wrapping(Wrapping::None)
		.padding(Padding::from([0.0, TEXT_PAD_X]))
		// `text_editor::width` takes an absolute pixel width (not a `Length`) — exactly what we want: the
		// widget is laid out at its content width so it never scrolls itself horizontally (§32).
		.width(content_px)
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

	// A translucent band behind the current find match's line (§32), so the match is visible even while
	// the find field holds focus (iced paints the buffer's own selection only when the buffer itself is
	// focused). It rides BEHIND the text in a `stack` so the glyphs draw over it, and is content-wide so
	// it washes the whole line at any horizontal offset. Three fixed spacers, not one widget per line —
	// the band is a single row, wherever it sits. The gutter lights the match row separately (§32).
	let line_count = editor.content.line_count().max(1);
	let text_layer: Element<'a, Message> = match editor.find_match_line() {
		Some(line) if line < line_count => {
			stack![line_band(line_count, line, p.match_line), editor_element].into()
		}
		_ => editor_element,
	};

	// The `Both` scrollable moves the text on both axes (§32) and reports its offset and visible size —
	// the four numbers that let `App` follow the cursor's line AND column without reading the widget's
	// hidden offsets. The gutter is NOT inside it: it pins to the reported vertical offset instead.
	let scroller = scrollable(text_layer)
		.id(BUFFER_SCROLL_ID)
		.width(Length::Fill)
		.height(Length::Fill)
		.direction(Direction::Both {
			vertical: Scrollbar::default(),
			horizontal: Scrollbar::default(),
		})
		.on_scroll(|viewport| {
			Message::Editor(EditorMessage::Scrolled {
				offset_x: viewport.absolute_offset().x,
				offset_y: viewport.absolute_offset().y,
				view_width: viewport.bounds().width,
				view_height: viewport.bounds().height,
			})
		});
	let text_pane: Element<'a, Message> = container(scroller)
		.width(Length::Fill)
		.height(Length::Fill)
		.style(move |_theme| container::Style {
			background: Some(bg.into()),
			..container::Style::default()
		})
		.into();

	let buffer: Element<'a, Message> = row![gutter(editor, p), text_pane]
		.width(Length::Fill)
		.height(Length::Fill)
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
	// The band itself carries a fill, so it keeps its own styled container; the plain pads above and
	// below share the gutter's spacer helper.
	let band = container(text(""))
		.width(Length::Fill)
		.height(Length::Fixed(LINE_HEIGHT))
		.style(move |_theme| container::Style {
			background: Some(color.into()),
			..container::Style::default()
		});
	column![fixed_spacer(above), band, fixed_spacer(below)]
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
/// height so the two stay aligned.
///
/// Pinned, not scrolled (§32): the gutter is NOT inside the buffer's scrollable — a `Both` scrollable's
/// horizontal bar could not coexist with a shared vertical scroll while keeping the gutter still. So
/// the full-height column of rows is placed in a `pin` translated up by the reported vertical offset
/// (`pin` clips its child to its own bounds), which lands each number on its line as a pure function of
/// that offset — zero sync lag, the old lockstep preserved without sharing a scrollable.
///
/// Virtualised (§32): iced rebuilds and lays out the WHOLE view tree every frame, so one widget per
/// line made a many-thousand-line file's gutter the dominant per-frame cost — while the buffer beside
/// it is a single `text_editor` whose off-screen glyphs the renderer clips. So the gutter materialises
/// only the rows the buffer scrollable currently shows (plus a small overscan) and preserves the total
/// height with one spacer above and one below — the same three-piece trick `line_band` uses. The
/// window comes from the offset and visible height the scrollable already reports (§32); until the
/// first frame measures the viewport every row is drawn, that pre-virtualisation cost paid just once.
fn gutter<'a>(editor: &'a Editor, p: &Palette) -> Element<'a, Message> {
	let count = editor.content.line_count().max(1);
	let changed = editor.changed();
	let match_line = editor.find_match_line();
	// The width is sized to the LARGEST number (the last line), not the visible ones, so the column
	// never jitters as different-length numbers scroll through.
	let width = (count.to_string().len() as f32) * DIGIT_WIDTH + BAR_WIDTH + GUTTER_PAD * 2.0;
	let mark = p.changed;
	let muted = p.muted;
	let match_fg = p.fg;
	let match_bg = p.match_line;

	let (first, last) = visible_lines(editor.scroll(), editor.view_height(), count);
	let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(last - first + 2);
	// The lines above the window, collapsed to one spacer so their height is kept without their widgets.
	rows.push(fixed_spacer(first as f32 * LINE_HEIGHT));
	for index in first..last {
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
	// The lines below the window, likewise collapsed — the two spacers plus the drawn rows sum to
	// exactly `count × LINE_HEIGHT`, so the column matches the buffer's height and the find-line band.
	rows.push(fixed_spacer((count - last) as f32 * LINE_HEIGHT));

	// Pin the full-height column up by the reported offset so line `first_visible` sits at the top of
	// the gutter, then clip it to the viewport (§32). `pin` clips its child to its own bounds already;
	// the container's `clip` is belt-and-braces and carries the gutter fill behind the whole column.
	let bg = p.chrome_bg;
	let pinned = pin(column(rows).width(Length::Fill))
		.width(Length::Fill)
		.height(Length::Fill)
		.y(-editor.scroll());
	container(pinned)
		.width(Length::Fixed(width))
		.height(Length::Fill)
		.clip(true)
		.style(move |_theme| container::Style {
			background: Some(bg.into()),
			..container::Style::default()
		})
		.into()
}

/// The half-open range of line indices the gutter must draw for a scroll offset and visible height
/// (§32): the rows on screen, widened by `GUTTER_OVERSCAN` either side and clamped to the file. A zero
/// (unmeasured) height means the first frame has not reported the viewport yet, so draw every line —
/// the pre-virtualisation cost, paid once until the real height arrives. Pure arithmetic, so it is
/// unit-tested without a widget.
fn visible_lines(offset: f32, view_height: f32, count: usize) -> (usize, usize) {
	if view_height <= 0.0 {
		return (0, count);
	}
	let first_visible = (offset.max(0.0) / LINE_HEIGHT).floor() as usize;
	let rows_shown = (view_height / LINE_HEIGHT).ceil() as usize;
	let first = first_visible.saturating_sub(GUTTER_OVERSCAN);
	let last = (first_visible + rows_shown + GUTTER_OVERSCAN + 1).min(count);
	// Guard the pathological case where the offset overshoots the content (the scrollable clamps it,
	// but keep the window valid regardless): first never past last, so `last - first` cannot underflow.
	(first.min(last), last)
}

/// A fixed-height, full-width filler that occupies vertical space without drawing anything — the plain
/// spacer the gutter's virtualisation and the find-line band build their off-screen padding from
/// (§32). The enclosing container supplies the background behind it.
fn fixed_spacer<'a>(height: f32) -> Element<'a, Message> {
	container(text(""))
		.width(Length::Fill)
		.height(Length::Fixed(height))
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

#[cfg(test)]
mod tests {
	use super::*;

	// The gutter's virtualisation window (§32). Only the arithmetic is tested — the drawn rows are a
	// widget tree — but that arithmetic is what keeps the gutter aligned and bounded, so it is the part
	// worth pinning.

	#[test]
	fn an_unmeasured_viewport_draws_every_line() {
		// Before the first frame reports a height, the whole file is drawn — the old cost, paid once.
		assert_eq!(visible_lines(0.0, 0.0, 5000), (0, 5000));
	}

	#[test]
	fn the_top_of_a_long_file_starts_at_the_first_line() {
		// A 20-row viewport at the top: from line 0, plus the below overscan, never past the file.
		assert_eq!(visible_lines(0.0, 400.0, 5000), (0, 25));
	}

	#[test]
	fn a_scrolled_window_brackets_the_visible_rows() {
		// Scrolled to line 100 (2000 px): the window is widened by the overscan either side, and the
		// visible band [100, 120) sits inside it.
		let (first, last) = visible_lines(2000.0, 400.0, 5000);
		assert_eq!((first, last), (96, 125));
		assert!(first <= 100 && last >= 120);
	}

	#[test]
	fn the_window_clamps_to_the_last_line() {
		// Near the foot of the file the window stops at the last line rather than running past it.
		assert_eq!(visible_lines(99_600.0, 400.0, 5000), (4976, 5000));
	}

	#[test]
	fn an_overshooting_offset_keeps_the_window_valid() {
		// The scrollable clamps the offset, but even a wild one must leave `first <= last` so the row
		// count cannot underflow — the window collapses to empty at the foot, its geometry still exact.
		let (first, last) = visible_lines(10_000_000.0, 400.0, 100);
		assert!(first <= last);
		assert_eq!((first, last), (100, 100));
	}

	#[test]
	fn a_short_file_draws_all_its_lines() {
		// When the file is shorter than the viewport, every line is in the window.
		assert_eq!(visible_lines(0.0, 400.0, 10), (0, 10));
	}

	#[test]
	fn the_content_width_leaves_room_past_the_last_column() {
		// The widest-line extent must reach past the last glyph, so the cursor sitting after it is
		// scrollable into view — content_width is strictly wider than the last column's left x.
		let cols = 80;
		assert!(
			content_width(cols) > col_x(cols),
			"extent reaches past the final column"
		);
		// A column's x advances by exactly one character each step, offset by the left padding.
		assert_eq!(col_x(1) - col_x(0), CHAR_ADVANCE);
		assert_eq!(col_x(0), TEXT_PAD_X);
	}
}

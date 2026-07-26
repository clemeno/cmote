// ui/files.rs — the remote file grid under the terminal (PLAN §19).
//
// A pure view over `files::Files`: the model decides which entries exist and in what
// order, this file turns them into an icon grid. Four pieces, mirroring the folder
// tree's (§18):
//
//   * `panel`        — the full-width pane: header, the wrapping grid, a notice line.
//   * `splitter`     — the grab bar above it; dragging it resizes the pane.
//   * `context_menu` — the right-click menu, drawn by the caller as a full-window overlay.
//   * `drag_layer` / `dismiss_layer` — the pointer-capture and click-away layers.
//
// The icons come from Material Icons, bundled in the binary (`app::ICON_FONT`). A font
// is how you get a folder glyph that scales and colours like text; the bundled monospace
// face has no such glyph, and drawing them by hand would be a canvas per cell — hundreds
// of them in a crowded directory.
//
// The palette is the tree's, imported rather than copied: the two panels sit against
// each other and must read as one region.

use iced::alignment::{Horizontal, Vertical};
use iced::widget::text::Wrapping;
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input};
use iced::{Color, Element, Font, Length, Padding};

use crate::app::Message;
use crate::explorer;
use crate::files::{Category, Entry, Files, FilesMessage, Kind, Rename};
use crate::ui::explorer::{
	FG, HEADER_BG, MENU_INSET, MUTED_FG, NOTICE_FG, PANEL_BG, SELECTED_BG, SPLITTER_BG, TEXT_SIZE,
	hidden_toggle,
};
use crate::ui::menu;

/// The widget id of the inline rename field, so `app` can focus it the instant the
/// rename starts — the user types straight away, no click needed (§14, §18).
pub const RENAME_INPUT_ID: &str = "files-rename";

/// The bundled icon face (Material Icons, Apache-2.0 — see assets/). Named exactly as
/// the font declares itself, the same discipline as the terminal's Fira Mono: iced
/// resolves faces by family name, and a name that does not match falls back to a system
/// font that has none of these glyphs.
const ICON_FONT: Font = Font::with_name("Material Icons");

/// One cell of the grid, and the pieces inside it. Fixed so the wrapping layout tiles
/// evenly and a long name cannot push its neighbours around.
const CELL_WIDTH: f32 = 96.0;
const CELL_HEIGHT: f32 = 78.0;
const CELL_SPACING: f32 = 4.0;
const ICON_SIZE: f32 = 30.0;
const LABEL_SIZE: f32 = 11.0;

/// The pane header's height, matching the tree's so the two headers line up.
const HEADER_HEIGHT: f32 = 28.0;

/// The "up one folder" button in the header: Material Icons' `arrow_upward`, sized to sit
/// with the header text rather than with the grid's icons.
const UP_GLYPH: char = '\u{e5d8}';
const UP_ICON_SIZE: f32 = 16.0;

/// Icon colours by category (§19). Muted enough to sit on the dark panel, distinct
/// enough that a directory of mixed content is scannable.
const FOLDER_COLOR: Color = Color::from_rgb8(0xe0, 0xb0, 0x60);
const LINK_COLOR: Color = Color::from_rgb8(0x80, 0xc0, 0xe0);
const IMAGE_COLOR: Color = Color::from_rgb8(0x90, 0xc8, 0x90);
const CODE_COLOR: Color = Color::from_rgb8(0x88, 0xb0, 0xe8);
const ARCHIVE_COLOR: Color = Color::from_rgb8(0xc0, 0xa0, 0xd0);
const DOCUMENT_COLOR: Color = Color::from_rgb8(0xc8, 0xc8, 0xc8);
const AUDIO_COLOR: Color = Color::from_rgb8(0xe0, 0x98, 0xb0);
const VIDEO_COLOR: Color = Color::from_rgb8(0xd8, 0x98, 0x78);
const PLAIN_COLOR: Color = Color::from_rgb8(0xa8, 0xa8, 0xa8);

/// The files pane: a header (the directory, the entry count, the shared `.*` toggle), the
/// icon grid, and — when something went wrong — a notice line under it. Fixed to the
/// model's current height so `grid_size` can subtract exactly that (§19).
///
/// `show_hidden` is the folder tree's flag (§18): one toggle filters both panels, which
/// is why it is passed in rather than owned here.
///
/// The whole pane is wrapped in a `mouse_area` that reports the pointer, because a
/// right-press carries no coordinates of its own — the same trick the tree uses (§18).
pub fn panel(files: &Files, show_hidden: bool) -> Element<'_, Message> {
	let mut content = column![header(files, show_hidden), grid(files, show_hidden)].spacing(0);
	if let Some(notice) = files.notice() {
		content = content.push(
			container(text(notice.to_owned()).size(TEXT_SIZE).color(NOTICE_FG))
				.width(Length::Fill)
				.padding(Padding::from([4.0, 8.0])),
		);
	}

	mouse_area(
		container(content)
			.width(Length::Fill)
			.height(Length::Fixed(files.height()))
			.style(|_theme| container::Style {
				background: Some(PANEL_BG.into()),
				..container::Style::default()
			}),
	)
	.on_move(|point| Message::Files(FilesMessage::PointerMoved(point)))
	.into()
}

/// The pane header: which directory is on show, how it is getting on, and the shared
/// dot-entry toggle. The count is the pane's only progress indicator while a big listing
/// streams in — it climbs a batch at a time (§19).
fn header(files: &Files, show_hidden: bool) -> Element<'_, Message> {
	let path = files.path().unwrap_or("no directory yet").to_owned();
	let status = if files.loading() {
		format!("{} so far…", files.count())
	} else {
		format!("{} items", files.count())
	};

	container(
		row![
			up_button(files.path().and_then(explorer::parent).is_some()),
			text(path).size(TEXT_SIZE).color(FG),
			text(status)
				.size(TEXT_SIZE)
				.color(MUTED_FG)
				.width(Length::Fill)
				.align_x(Horizontal::Right),
			hidden_toggle(show_hidden),
		]
		.spacing(12)
		.align_y(Vertical::Center),
	)
	.width(Length::Fill)
	.height(Length::Fixed(HEADER_HEIGHT))
	.align_y(Vertical::Center)
	.padding(Padding::from([0.0, 8.0]))
	.style(|_theme| container::Style {
		background: Some(HEADER_BG.into()),
		..container::Style::default()
	})
	.into()
}

/// The toolbar's "up" button, first in the header so it sits where every file manager
/// puts it. `on_press_maybe(None)` is what disables it — iced dims and deadens a button
/// with no message — which is the state at the root and before the first listing, the two
/// cases with no directory above the one on show.
fn up_button(enabled: bool) -> Element<'static, Message> {
	button(
		text(UP_GLYPH.to_string())
			.font(ICON_FONT)
			.size(UP_ICON_SIZE)
			.color(if enabled { FG } else { MUTED_FG }),
	)
	.padding(Padding::from([0.0, 4.0]))
	.style(|_theme, status| button::Style {
		background: match status {
			button::Status::Hovered | button::Status::Pressed => Some(SELECTED_BG.into()),
			_ => None,
		},
		..button::Style::default()
	})
	.on_press_maybe(enabled.then_some(Message::Files(FilesMessage::ParentOpened)))
	.into()
}

/// The scrollable icon grid. `Row::wrap` flows the cells and breaks the line whenever
/// the next one would not fit, so the column count follows the window's width without
/// this view ever being told what that width is.
fn grid(files: &Files, show_hidden: bool) -> Element<'_, Message> {
	let directory = files.path().unwrap_or(explorer::ROOT);
	let selected = files.selected();
	let editing = files.editing();

	let cells = files
		.rows(show_hidden)
		.into_iter()
		.map(|entry| cell(entry, directory, selected, editing));

	scrollable(
		container(
			row(cells)
				.spacing(CELL_SPACING)
				.width(Length::Fill)
				.wrap()
				.vertical_spacing(CELL_SPACING),
		)
		.padding(CELL_SPACING),
	)
	.width(Length::Fill)
	.height(Length::Fill)
	.into()
}

/// One entry: its icon with its name under it. A left click selects, a double click
/// enters a directory, a right click opens the menu on it. An entry being renamed shows
/// its edit field in place of the label.
fn cell<'a>(
	entry: &Entry,
	directory: &str,
	selected: Option<&str>,
	editing: Option<&'a Rename>,
) -> Element<'a, Message> {
	let path = explorer::join(directory, &entry.name);
	let category = crate::files::category(entry);

	let icon = text(glyph(category).to_string())
		.font(ICON_FONT)
		.size(ICON_SIZE)
		.color(icon_color(category));

	// The label wraps by GLYPH, not by word: file names rarely contain spaces, so
	// word wrapping would leave `some_very_long_name.tar.gz` as one unbreakable line
	// running out of its cell. The cell clips whatever still does not fit.
	let label: Element<'a, Message> = match editing.filter(|rename| rename.path == path) {
		Some(rename) => text_input("Name", &rename.text)
			.id(RENAME_INPUT_ID)
			.size(LABEL_SIZE)
			.padding(Padding::from([0.0, 2.0]))
			.on_input(|value| Message::Files(FilesMessage::RenameEdited(value)))
			.on_submit(Message::Files(FilesMessage::RenameCommitted))
			.into(),
		None => text(entry.name.clone())
			.size(LABEL_SIZE)
			.color(FG)
			.wrapping(Wrapping::Glyph)
			.align_x(Horizontal::Center)
			.width(Length::Fill)
			.into(),
	};

	let is_selected = selected == Some(path.as_str());
	let cell = container(
		column![icon, label]
			.spacing(2)
			.align_x(Horizontal::Center)
			.width(Length::Fill),
	)
	.width(Length::Fixed(CELL_WIDTH))
	.height(Length::Fixed(CELL_HEIGHT))
	.padding(4)
	.align_x(Horizontal::Center)
	.clip(true)
	.style(move |_theme| container::Style {
		background: is_selected.then(|| SELECTED_BG.into()),
		border: iced::Border {
			radius: 4.0.into(),
			..iced::Border::default()
		},
		..container::Style::default()
	});

	mouse_area(cell)
		.on_press(Message::Files(FilesMessage::EntryClicked(path.clone())))
		.on_double_click(Message::Files(FilesMessage::EntryOpened(path.clone())))
		.on_right_press(Message::Files(FilesMessage::EntryRightClicked(path)))
		.into()
}

/// The Material Icons code point for a category (§19). The names are the font's own:
/// folder, link, image, code, folder_zip, description, audiotrack, movie,
/// insert_drive_file.
fn glyph(category: Category) -> char {
	match category {
		Category::Folder => '\u{e2c7}',
		Category::Link => '\u{e157}',
		Category::Image => '\u{e3f4}',
		Category::Code => '\u{e86f}',
		Category::Archive => '\u{eb2c}',
		Category::Document => '\u{e873}',
		Category::Audio => '\u{e3a1}',
		Category::Video => '\u{e02c}',
		Category::Plain => '\u{e24d}',
	}
}

/// The colour that goes with a category's glyph.
fn icon_color(category: Category) -> Color {
	match category {
		Category::Folder => FOLDER_COLOR,
		Category::Link => LINK_COLOR,
		Category::Image => IMAGE_COLOR,
		Category::Code => CODE_COLOR,
		Category::Archive => ARCHIVE_COLOR,
		Category::Document => DOCUMENT_COLOR,
		Category::Audio => AUDIO_COLOR,
		Category::Video => VIDEO_COLOR,
		Category::Plain => PLAIN_COLOR,
	}
}

/// The grab bar between the terminal row and the files pane (§19). Pressing it starts a
/// resize; the pointer-capture layer added while dragging reports the moves and the
/// release, so tracking survives the pointer leaving the bar.
pub fn splitter() -> Element<'static, Message> {
	mouse_area(
		container(text(""))
			.width(Length::Fill)
			.height(Length::Fixed(crate::files::SPLITTER_HEIGHT))
			.style(|_theme| container::Style {
				background: Some(SPLITTER_BG.into()),
				..container::Style::default()
			}),
	)
	.on_press(Message::Files(FilesMessage::SplitterGrabbed))
	.on_release(Message::Files(FilesMessage::SplitterReleased))
	.into()
}

/// The transparent full-window layer present only while the splitter is being dragged.
pub fn drag_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_move(|point| Message::Files(FilesMessage::SplitterDragged(point)))
		.on_release(Message::Files(FilesMessage::SplitterReleased))
		.into()
}

/// The right-click menu for the entry it is open on, as a full-window overlay (§19).
/// Returns `None` when no menu is open.
///
/// `cwd` is the shell's working directory: "Copy relative path" needs one to be relative
/// *to*, so the item is disabled without it (§17). "Open in terminal" is for directories
/// and "Download" for files — each is disabled on the other, rather than hidden, so the
/// menu keeps one shape.
pub fn context_menu<'a>(files: &'a Files, cwd: Option<&str>) -> Option<Element<'a, Message>> {
	let open = files.menu()?;
	// Frozen when the menu opened, so it stays put while the pointer travels to an item.
	let anchor = open.at;
	let path = open.path.clone();
	let is_dir = open.kind == Kind::Dir;

	let item = |label: &str, message: FilesMessage| {
		menu::item(label.to_owned(), Some(Message::Files(message)))
	};

	let panel = menu::panel(vec![
		menu::item(
			"Open in terminal".to_owned(),
			is_dir.then(|| Message::Files(FilesMessage::EntryOpened(path.clone()))),
		),
		menu::item(
			"Download…".to_owned(),
			(!is_dir).then(|| Message::Files(FilesMessage::Download(path.clone()))),
		),
		item("Rename…", FilesMessage::RenameStarted(path.clone())),
		item("Copy name", FilesMessage::CopyName(path.clone())),
		menu::item(
			"Copy relative path".to_owned(),
			cwd.map(|_| Message::Files(FilesMessage::CopyRelative(path.clone()))),
		),
		item("Copy full path", FilesMessage::CopyPath(path)),
		item("Refresh", FilesMessage::Refresh),
	]);

	// Placed from the pointer, anchored to the window's BOTTOM edge — which is also the
	// pane's. `pane height - pointer.y` is the pointer's distance from that edge, so the
	// menu's bottom lands under the cursor and the panel grows *upwards*. That is the
	// right direction here: this pane sits at the bottom of the window, and a menu
	// dropping downwards would fall off it. Aligning to the bottom also means the view
	// never needs to know how tall the window is.
	let bottom = (files.height() - anchor.y).max(MENU_INSET);
	Some(
		container(panel)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_y(Vertical::Bottom)
			.padding(Padding {
				top: 0.0,
				right: 0.0,
				bottom,
				left: anchor.x,
			})
			.into(),
	)
}

/// The click-away layer that sits under this menu (shared chrome, §10).
pub fn dismiss_layer() -> Element<'static, Message> {
	menu::dismiss_layer(Message::Files(FilesMessage::MenuDismissed))
}

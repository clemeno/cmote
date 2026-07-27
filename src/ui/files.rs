// ui/files.rs — the remote file grid under the terminal (PLAN §19).
//
// A pure view over `files::Files`: the model decides which entries exist and in what
// order, this file turns them into an icon grid. Five pieces, mirroring the folder
// tree's (§18):
//
//   * `panel`        — the full-width pane: header, the wrapping grid, a notice line.
//   * `details`      — the popup beside the selected cell: type, time, size, owner (§20).
//   * `splitter`     — the grab bar above it; dragging it resizes the pane.
//   * `context_menu` — the right-click menu, drawn by the caller as a full-window overlay.
//   * `drag_layer` / `dismiss_layer` — the pointer-capture and click-away layers.
//
// It also owns the grid's *geometry* (`columns`, `row_top`, `grid_height`): iced never
// says where a laid-out cell ended up, so the popup places itself with the same
// arithmetic the layout wraps with — and `app` borrows it to move the selection a whole
// row with the arrow keys and to scroll it back into view (§20).
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
use iced::widget::{
	button, column, container, mouse_area, row, scrollable, stack, text, text_input,
};
use iced::{Color, Element, Font, Length, Padding};

use crate::app::Message;
use crate::explorer;
use crate::files::{Category, Entry, Files, FilesMessage, Kind, Rename};
use crate::ui::explorer::{
	FG, HEADER_BG, MENU_INSET, MUTED_FG, NOTICE_FG, NOTICE_HEIGHT, PANEL_BG, SELECTED_BG,
	SPLITTER_BG, TEXT_SIZE, focus_border, hidden_toggle,
};
use crate::ui::menu;

/// The widget id of the inline rename field, so `app` can focus it the instant the
/// rename starts — the user types straight away, no click needed (§14, §18).
pub const RENAME_INPUT_ID: &str = "files-rename";

/// The widget id of the icon grid's scrollable, so `app` can scroll a keyboard-moved
/// selection back into view (§20).
pub const GRID_ID: &str = "files-grid";

/// The bundled icon face (Material Icons, Apache-2.0 — see assets/). Named exactly as
/// the font declares itself, the same discipline as the terminal's Fira Mono: iced
/// resolves faces by family name, and a name that does not match falls back to a system
/// font that has none of these glyphs.
const ICON_FONT: Font = Font::with_name("Material Icons");

/// One cell of the grid, and the pieces inside it. The cell is a wide, short row — a small
/// icon in front of a left-aligned name — so it reads like a list line while the grid still
/// wraps into columns. Fixed so the wrapping layout tiles evenly and a long name cannot push
/// its neighbours around; the height fits two wrapped lines and the cell clips a name that
/// runs longer (the details popup always shows it in full, §20). Every cell is the same
/// height on purpose: the selection, keyboard row-nav and popup placement all step by a
/// uniform pitch (`row_top`, `band_hits`), which a per-name height would break.
const CELL_WIDTH: f32 = 350.0;
pub const CELL_HEIGHT: f32 = 56.0;
const CELL_SPACING: f32 = 4.0;
/// The padding inside a cell, kept as a constant because the name-fitting estimate below has
/// to subtract the very same value the container lays the cell out with.
const CELL_PADDING: f32 = 4.0;
/// The gap between a cell's icon and the name that follows it.
const ICON_LABEL_GAP: f32 = 6.0;
const ICON_SIZE: f32 = 18.0;
const LABEL_SIZE: f32 = 11.0;
/// The cell's second line — a file's size and modified date under its name (§19). Smaller and
/// (via `MUTED_FG`) dimmer than the name, so the name still reads first; `META_GAP` is the
/// space between the two lines.
const META_SIZE: f32 = 10.0;
const META_GAP: f32 = 2.0;

/// Sizing the middle-ellipsis so a name always fits the cell's two lines (§19). The label
/// sits to the right of the icon, so its width is the cell minus the padding, the icon and
/// the gap; `LABEL_CHAR` is the average advance of the interface face at `LABEL_SIZE` — the
/// same kind of estimate the details popup wraps with. A name longer than two lines' worth of
/// glyphs is shortened with `…` in its MIDDLE, so both the start of the name and its extension
/// survive the cut.
///
/// `ponytail:` an estimate, not a measurement, and deliberately a touch pessimistic (a fatter
/// `LABEL_CHAR` than the face truly has) so a name never spills past the two lines the cell
/// clips at. Ask iced's text shaper for the real extent if a name ever wraps to a third line.
const LABEL_WIDTH: f32 = CELL_WIDTH - 2.0 * CELL_PADDING - ICON_SIZE - ICON_LABEL_GAP;
const LABEL_CHAR: f32 = 6.2;
const LABEL_LINES: usize = 2;

/// The pane header's height, matching the tree's so the two headers line up. Public
/// because it is also the grid's top edge, which is what says whether a press landed in
/// the grid — and so whether it starts a rubber band (§21).
pub const HEADER_HEIGHT: f32 = 28.0;

/// The rubber band's fill and edge (§21). Translucent, so the cells it is being dragged
/// over stay readable underneath it.
const BAND_BG: Color = Color::from_rgba(0.38, 0.56, 0.82, 0.25);
const BAND_EDGE: Color = Color::from_rgb(0.45, 0.65, 0.90);

/// The details popup beside the selection (§20): its width, the gap between it and the
/// cell it belongs to, and the metrics of the lines inside it. Fixed, because the popup
/// is placed by arithmetic — iced does not report where a laid-out widget ended up, so
/// the view works out the cell's position itself and needs to know its own size to keep
/// the card inside the pane.
const POPUP_WIDTH: f32 = 250.0;
const POPUP_GAP: f32 = 6.0;
const POPUP_LINE: f32 = 15.0;
const POPUP_PADDING: f32 = 6.0;
const POPUP_CHAR: f32 = 6.6;
const POPUP_BG: Color = Color::from_rgb8(0x1c, 0x1c, 0x1c);
/// The copy button pinned to the popup's top-right corner (§20): the size of its glyph and
/// the height of the bar it sits on. The bar is added to the card's computed height so a
/// button row never eats into the lines below it.
const POPUP_ICON_SIZE: f32 = 14.0;
const POPUP_BUTTON_ROW: f32 = 16.0;

/// The header buttons: Material Icons' `arrow_upward` (up one folder) and `content_copy`
/// (copy the path on show), both sized to sit with the header text rather than with the
/// grid's icons.
const UP_GLYPH: char = '\u{e5d8}';
const COPY_GLYPH: char = '\u{e14d}';
const HEADER_ICON_SIZE: f32 = 16.0;

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
/// `width` is the window's, which is also the pane's: the grid wraps at it, so it is what
/// says how many columns there are and therefore where the selected cell — and the
/// details popup beside it — sit (§20). `focused` draws the ring that says the keyboard
/// is here.
pub fn panel(files: &Files, show_hidden: bool, width: f32, focused: bool) -> Element<'_, Message> {
	let mut content = column![header(files, show_hidden), grid(files, show_hidden)].spacing(0);
	if let Some(notice) = files.notice() {
		content = content.push(
			container(text(notice.to_owned()).size(TEXT_SIZE).color(NOTICE_FG))
				.width(Length::Fill)
				.height(Length::Fixed(NOTICE_HEIGHT))
				.padding(Padding::from([0.0, 8.0])),
		);
	}

	// The band and the popup float over the grid rather than in it: either one in the flow
	// would reshuffle the cells every time the selection moved.
	let mut layers: Vec<Element<'_, Message>> = vec![content.into()];
	if let Some(band) = files.band() {
		layers.push(band_layer(band.rect(), files, width));
	}
	if let Some(popup) = details(files, show_hidden, width) {
		layers.push(popup);
	}

	mouse_area(
		container(stack(layers))
			.width(Length::Fill)
			.height(Length::Fixed(files.height()))
			.style(move |_theme| container::Style {
				background: Some(PANEL_BG.into()),
				border: focus_border(focused),
				..container::Style::default()
			}),
	)
	.on_move(|point| Message::Files(FilesMessage::PointerMoved(point)))
	// A press anywhere in the pane — a cell or the empty space beside one — is what
	// gives it the keyboard (§20); one on the grid's empty space also starts a band (§21).
	.on_press(Message::Files(FilesMessage::PanelPressed))
	.on_release(Message::Files(FilesMessage::PanelReleased))
	.into()
}

/// The rubber band itself (§21): a translucent rectangle over the grid, clipped to the
/// grid's own bounds so a drag that runs past the pane never paints over the header, the
/// notice line or the panel next door.
fn band_layer<'a>(rect: iced::Rectangle, files: &Files, width: f32) -> Element<'a, Message> {
	let top = rect.y.max(HEADER_HEIGHT);
	let bottom = (rect.y + rect.height).min(HEADER_HEIGHT + grid_height(files));
	let left = rect.x.max(0.0);
	let right = (rect.x + rect.width).min(width);

	container(
		container(text(""))
			.width(Length::Fixed((right - left).max(0.0)))
			.height(Length::Fixed((bottom - top).max(0.0)))
			.style(|_theme| container::Style {
				background: Some(BAND_BG.into()),
				border: iced::Border {
					width: 1.0,
					color: BAND_EDGE,
					..iced::Border::default()
				},
				..container::Style::default()
			}),
	)
	.width(Length::Fill)
	.height(Length::Fill)
	.padding(Padding {
		top,
		right: 0.0,
		bottom: 0.0,
		left,
	})
	.into()
}

/// Which grid entries a rubber band covers (§21), as indices into the rows on show.
///
/// `rect` is in PANE coordinates, which is what the pointer reports: the header sits above
/// the grid and the grid may be scrolled, so both are taken out before any cell is tested.
/// Only the rows the band actually spans are walked, so a band over a directory of
/// thousands costs the same as one over a directory of ten.
pub fn band_hits(rect: iced::Rectangle, columns: usize, count: usize, scroll: f32) -> Vec<usize> {
	let top = rect.y - HEADER_HEIGHT + scroll;
	let bottom = top + rect.height;
	let pitch = CELL_HEIGHT + CELL_SPACING;
	let first = ((top - CELL_SPACING) / pitch).floor().max(0.0) as usize;
	let last = ((bottom - CELL_SPACING) / pitch).floor().max(0.0) as usize;

	let mut hits = Vec::new();
	for row in first..=last {
		for column in 0..columns {
			let index = row * columns + column;
			if index >= count {
				return hits;
			}
			let x = CELL_SPACING + column as f32 * (CELL_WIDTH + CELL_SPACING);
			let y = row_top(row);
			// Touching counts, the way every file manager's band works: a cell is in as soon
			// as the rectangle overlaps it at all.
			if x < rect.x + rect.width
				&& x + CELL_WIDTH > rect.x
				&& y < bottom
				&& y + CELL_HEIGHT > top
			{
				hits.push(index);
			}
		}
	}
	hits
}

/// How many cells fit across a pane `width` wide (§20). `Row::wrap` breaks the line when
/// the next cell would not fit, so this is that same sum done ahead of time: the app needs
/// it to move the selection a whole row with the up/down arrows, and the popup to know
/// which row and column the selection landed on. At least one, however narrow the window.
pub fn columns(width: f32) -> usize {
	let usable = width - 2.0 * CELL_SPACING + CELL_SPACING;
	((usable / (CELL_WIDTH + CELL_SPACING)) as usize).max(1)
}

/// The top edge of grid row `row`, in the scrollable's own coordinates (§20).
pub fn row_top(row: usize) -> f32 {
	CELL_SPACING + row as f32 * (CELL_HEIGHT + CELL_SPACING)
}

/// How tall the scrollable part of the pane is (§20) — the pane minus its header and,
/// when one is showing, its notice line. What "on screen" means when the app scrolls a
/// keyboard-moved selection back into view.
pub fn grid_height(files: &Files) -> f32 {
	let notice = if files.notice().is_some() {
		NOTICE_HEIGHT
	} else {
		0.0
	};
	(files.height() - HEADER_HEIGHT - notice).max(0.0)
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
			copy_button(
				files.path().is_some(),
				Message::Files(FilesMessage::CopyCurrentPath),
			),
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
			.size(HEADER_ICON_SIZE)
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

/// A header "copy path" button, sitting right after the directory it copies. Styled like
/// the up button beside it so the two read as one toolbar; `on_press_maybe(None)` dims and
/// deadens it before the first listing, when there is no path to put on the clipboard.
///
/// `pub(crate)` and message-agnostic because both panel headers wear one — the pane below
/// and the tree beside it (§22) — each copying its own path. The face is the same one the
/// file icons use, so the two panels' chrome stays of a piece.
pub(crate) fn copy_button(enabled: bool, message: Message) -> Element<'static, Message> {
	button(
		text(COPY_GLYPH.to_string())
			.font(ICON_FONT)
			.size(HEADER_ICON_SIZE)
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
	.on_press_maybe(enabled.then_some(message))
	.into()
}

/// The details popup's copy button (§20): copies the card's whole description in one press.
/// `'static` because it owns everything it needs — the joined `description` moves into its
/// message, and the glyph and styling are constants — so it outlives the borrow of `files`
/// the surrounding popup holds.
fn copy_details_button(description: String) -> Element<'static, Message> {
	button(
		text(COPY_GLYPH.to_string())
			.font(ICON_FONT)
			.size(POPUP_ICON_SIZE)
			.color(MUTED_FG),
	)
	.padding(Padding::from([0.0, 2.0]))
	.style(|_theme, status| button::Style {
		background: match status {
			button::Status::Hovered | button::Status::Pressed => Some(SELECTED_BG.into()),
			_ => None,
		},
		..button::Style::default()
	})
	.on_press(Message::Files(FilesMessage::CopyDetails(description)))
	.into()
}

/// The scrollable icon grid. `Row::wrap` flows the cells and breaks the line whenever
/// the next one would not fit, so the column count follows the window's width without
/// this view ever being told what that width is.
fn grid(files: &Files, show_hidden: bool) -> Element<'_, Message> {
	let directory = files.path().unwrap_or(explorer::ROOT);
	let editing = files.editing();

	let cells = files
		.rows(show_hidden)
		.into_iter()
		.map(|entry| cell(entry, directory, files, editing));

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
	.id(GRID_ID)
	// Reported so the popup can be placed against a scrolled grid, and so keyboard
	// navigation knows what is already on screen before it scrolls (§20).
	.on_scroll(|viewport| Message::Files(FilesMessage::Scrolled(viewport.absolute_offset().y)))
	.width(Length::Fill)
	.height(Length::Fill)
	.into()
}

/// The details popup for the selection (§20): for one entry, its full name, where it points
/// if it is a symlink, then its type, when it was last modified in the server's own
/// timezone, its size, and its `owner:group`; for several, how many they are and what they
/// come to (§21). `None` when nothing is selected, or when the cursor is filtered out by
/// the `.*` toggle.
///
/// Placed beside the cell rather than under the pointer, because the selection moves by
/// keyboard as well as by click. iced does not say where a laid-out cell ended up, so the
/// position is computed from the same geometry the grid is laid out with — the index, the
/// column count and the scroll offset — and flipped to the cell's left when the card would
/// hang off the right edge.
fn details<'a>(files: &'a Files, show_hidden: bool, width: f32) -> Option<Element<'a, Message>> {
	let index = files.selected_index(show_hidden)?;
	let rows = files.rows(show_hidden);
	let entry = *rows.get(index)?;

	let lines = if files.selected_count() > 1 {
		summary(files, show_hidden)
	} else {
		entry_lines(files, entry)
	};

	// A name or a link target can outrun the card's width, so those lines wrap — and the
	// card has to be as tall as what they wrap to, since its height is what keeps it inside
	// the pane. iced measures text only while laying it out, far too late for that, so the
	// rows are counted here from the average glyph width of the bundled face.
	// ponytail: an estimate, not a measurement — a line of all `W`s can still clip. Ask
	// iced's text shaper for the real extent if that ever shows.
	let rows: usize = lines.iter().map(|line| wrapped_rows(line)).sum();
	let height = rows as f32 * POPUP_LINE + 2.0 * POPUP_PADDING;

	// One press copies the whole card. The text is joined here, from the same lines drawn
	// below, so the model never has to rebuild what the view already has (§20).
	let description = lines.join("\n");
	let body = column(lines.into_iter().map(|line| {
		text(line)
			.size(TEXT_SIZE - 1.0)
			.color(FG)
			.line_height(iced::widget::text::LineHeight::Absolute(POPUP_LINE.into()))
			.wrapping(Wrapping::Glyph)
			.into()
	}))
	.spacing(0);

	// The copy button sits on its own bar at the top, pinned right. A dedicated row rather
	// than an overlay so it never paints over a long name's first line — showing that name
	// in full is the whole reason the card exists.
	let top_bar = container(copy_details_button(description))
		.width(Length::Fill)
		.align_x(Horizontal::Right);

	let card_height = height + POPUP_BUTTON_ROW;
	let card = container(column![top_bar, body].spacing(0))
		.width(Length::Fixed(POPUP_WIDTH))
		.height(Length::Fixed(card_height))
		.padding(POPUP_PADDING)
		.clip(true)
		.style(|_theme| container::Style {
			background: Some(POPUP_BG.into()),
			border: iced::Border {
				width: 1.0,
				radius: 4.0.into(),
				color: SELECTED_BG,
			},
			..container::Style::default()
		});

	let columns = columns(width);
	let cell_x = CELL_SPACING + (index % columns) as f32 * (CELL_WIDTH + CELL_SPACING);
	let cell_y = HEADER_HEIGHT + row_top(index / columns) - files.scroll();
	let right_of = cell_x + CELL_WIDTH + POPUP_GAP;
	let left = if right_of + POPUP_WIDTH + MENU_INSET <= width {
		right_of
	} else {
		(cell_x - POPUP_GAP - POPUP_WIDTH).max(MENU_INSET)
	};
	// Kept inside the pane: a cell near the bottom would otherwise hang its card off it.
	let top = cell_y.clamp(
		HEADER_HEIGHT,
		(files.height() - card_height).max(HEADER_HEIGHT),
	);

	Some(
		container(card)
			.width(Length::Fill)
			.height(Length::Fill)
			.padding(Padding {
				top,
				right: 0.0,
				bottom: 0.0,
				left,
			})
			.into(),
	)
}

/// The popup's lines for a single entry (§20).
fn entry_lines(files: &Files, entry: &Entry) -> Vec<String> {
	// The name heads the card because it is the one thing the cell below may have shortened:
	// the grid middle-ellipsises a name too long for its two lines, this is what the entry
	// is actually called.
	let mut lines = vec![entry.name.clone()];
	if entry.kind == Kind::Link {
		// The target costs a round trip, so it lands a moment after the rest (§20).
		lines.push(format!("→ {}", files.link_target().unwrap_or("resolving…")));
	}
	lines.extend([
		match entry.kind {
			Kind::Dir => "Folder".to_owned(),
			Kind::Link => "Symlink".to_owned(),
			Kind::File => crate::files::mime(&entry.name).to_owned(),
		},
		// Absent facts show as a dash rather than vanishing: the `ls` fallback knows none
		// of them (§19), and a card that changed shape per entry would be harder to read.
		entry.meta.mtime.map_or_else(
			|| "—".to_owned(),
			|mtime| crate::files::format_mtime(mtime, files.zone()),
		),
		entry.meta.size.map_or_else(|| "—".to_owned(), human_size),
		crate::files::owner_group(&entry.meta).unwrap_or_else(|| "—".to_owned()),
	]);
	lines
}

/// The popup's lines for a multiple selection (§21): how many entries, split into folders
/// and files, and what the files come to. A folder's own size is the size of its directory
/// entry, not of what is inside it, so it is left out of the total rather than making it
/// wrong.
fn summary(files: &Files, show_hidden: bool) -> Vec<String> {
	let rows = files.selected_rows(show_hidden);
	let folders = rows
		.iter()
		.filter(|(_, entry)| entry.kind == Kind::Dir)
		.count();
	let total: u64 = rows
		.iter()
		.filter(|(_, entry)| entry.kind != Kind::Dir)
		.filter_map(|(_, entry)| entry.meta.size)
		.sum();

	vec![
		format!("{} items selected", rows.len()),
		match folders {
			0 => format!("{} files", rows.len()),
			_ => format!("{folders} folders, {} files", rows.len() - folders),
		},
		human_size(total),
	]
}

/// How many rows a popup line takes once it wraps at the card's width — at least one, so
/// an empty line still occupies its own. `POPUP_CHAR` is the average advance of the
/// interface face at the popup's text size, measured off the glyphs a path is made of.
fn wrapped_rows(line: &str) -> usize {
	let per_row = ((POPUP_WIDTH - 2.0 * POPUP_PADDING) / POPUP_CHAR) as usize;
	line.chars().count().div_ceil(per_row.max(1)).max(1)
}

/// A size for the popup: the human reading, with the exact byte count behind it once the
/// two differ (§20). "4.2 KiB" is what you scan for; "4311 B" is what you need when the
/// difference matters.
fn human_size(size: u64) -> String {
	let human = crate::ui::terminal::human_bytes(size);
	if size < 1024 {
		human
	} else {
		format!("{human} ({size} B)")
	}
}

/// One entry: a small icon in front of its name. A left click selects, a double click
/// enters a directory, a right click opens the menu on it. An entry being renamed shows
/// its edit field in place of the label.
fn cell<'a>(
	entry: &Entry,
	directory: &str,
	files: &Files,
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
	// running out of its cell. `elide_middle` has already trimmed a name to the two lines
	// the cell shows; `clip(true)` is only a backstop for that estimate, and the details
	// popup is where the whole name always shows (§20).
	let label: Element<'a, Message> = match editing.filter(|rename| rename.path == path) {
		Some(rename) => text_input("Name", &rename.text)
			.id(RENAME_INPUT_ID)
			.size(LABEL_SIZE)
			.padding(Padding::from([0.0, 2.0]))
			.on_input(|value| Message::Files(FilesMessage::RenameEdited(value)))
			.on_submit(Message::Files(FilesMessage::RenameCommitted))
			.into(),
		None => text(elide_middle(&entry.name))
			.size(LABEL_SIZE)
			.color(FG)
			.wrapping(Wrapping::Glyph)
			.align_x(Horizontal::Left)
			.width(Length::Fill)
			.into(),
	};

	let is_selected = files.is_selected(&path);
	let name_row = row![icon, label]
		.spacing(ICON_LABEL_GAP)
		.align_y(Vertical::Center)
		.width(Length::Fill);
	// The second line, under the name: the file's size and when it was last modified (§19).
	// Muted so the name still reads first; it wraps nothing, and the cell clips it if a wide
	// date and zone ever outrun the column.
	let meta = text(meta_line(entry, files))
		.size(META_SIZE)
		.color(MUTED_FG)
		.width(Length::Fill);
	let cell = container(
		column![name_row, meta]
			.spacing(META_GAP)
			.width(Length::Fill),
	)
	.width(Length::Fixed(CELL_WIDTH))
	.height(Length::Fixed(CELL_HEIGHT))
	.padding(CELL_PADDING)
	.align_y(Vertical::Center)
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

/// Shorten a name to the two lines a cell shows, dropping the MIDDLE and marking the cut with
/// `…`, so a name too long to fit keeps both its start and its extension rather than being
/// clipped mid-word (§19). A name that already fits passes through untouched. The details
/// popup still holds the whole name (§20); this only spares the grid a name it cannot draw.
///
/// Splits by CHARACTERS, not bytes, so a multi-byte name is never cut through a glyph. The
/// tail keeps the odd character when the budget is uneven, since the extension is usually the
/// end and the more worth-showing half.
fn elide_middle(name: &str) -> String {
	let per_line = (LABEL_WIDTH / LABEL_CHAR).floor().max(1.0) as usize;
	let max = per_line * LABEL_LINES;

	let chars: Vec<char> = name.chars().collect();
	if chars.len() <= max {
		return name.to_owned();
	}

	// One glyph goes to the `…`; the rest is split head/tail.
	let budget = max.saturating_sub(1);
	let head = budget / 2;
	let tail = budget - head;
	let start: String = chars[..head].iter().collect();
	let end: String = chars[chars.len() - tail..].iter().collect();
	format!("{start}…{end}")
}

/// A cell's second line: a file's size and its last-modified date, compact, under the name
/// (§19). A folder shows only the date — a directory entry's own size is not the size of what
/// is inside it (§21), so a number there would mislead. Facts the `ls` fallback never learns
/// (§19) show as a dash rather than leaving the line lopsided; the details popup carries the
/// exact size and the full timestamp (§20).
fn meta_line(entry: &Entry, files: &Files) -> String {
	let date = entry.meta.mtime.map_or_else(
		|| "—".to_owned(),
		|mtime| crate::files::format_mtime_short(mtime, files.zone()),
	);
	if entry.kind == Kind::Dir {
		return date;
	}
	let size = entry
		.meta
		.size
		.map_or_else(|| "—".to_owned(), crate::ui::terminal::human_bytes);
	format!("{size} · {date}")
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

/// The same, for a rubber band (§21). A `mouse_area` reports a release only while the
/// pointer is over it, so a band dragged up out of the pane and let go over the terminal
/// would never end — and the next move back over the pane would carry on selecting with
/// the button up. This layer catches both the moves and the release wherever they happen;
/// its points are window coordinates, which the app maps back onto the pane.
pub fn band_drag_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_move(|point| Message::Files(FilesMessage::BandMoved(point)))
		.on_release(Message::Files(FilesMessage::PanelReleased))
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
	// A menu opened ON the selection acts on all of it (§21); one opened elsewhere has
	// already collapsed the selection onto that single entry, so this reads false.
	let many = files.selected_count() > 1 && files.is_selected(&path);

	let item = |label: &str, message: FilesMessage| {
		menu::item(label.to_owned(), Some(Message::Files(message)))
	};
	// The copy items say how many they will take, so a batch is never a surprise; the
	// single-target ones are disabled, since there is no sane "rename these nine".
	let count = files.selected_count();
	let suffix = |label: &str| {
		if many {
			format!("{label} ({count})")
		} else {
			label.to_owned()
		}
	};

	let panel = menu::panel(vec![
		menu::item(
			"Open in terminal".to_owned(),
			(is_dir && !many).then(|| Message::Files(FilesMessage::OpenInTerminal(path.clone()))),
		),
		menu::item(
			suffix("Download…"),
			// A folder cannot be downloaded, but a batch that merely CONTAINS one still can:
			// the app pulls the files out of it and leaves the folders where they are.
			(!is_dir || many).then(|| Message::Files(FilesMessage::Download(path.clone()))),
		),
		menu::item(
			"Rename…".to_owned(),
			(!many).then(|| Message::Files(FilesMessage::RenameStarted(path.clone()))),
		),
		item(&suffix("Copy name"), FilesMessage::CopyName(path.clone())),
		menu::item(
			suffix("Copy relative path"),
			cwd.map(|_| Message::Files(FilesMessage::CopyRelative(path.clone()))),
		),
		item(&suffix("Copy full path"), FilesMessage::CopyPath(path)),
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_long_name_is_shortened_in_the_middle_to_fit_the_cell() {
		// Arrange: a name well past the two lines a cell can draw — sized off the cell's own
		// budget so it stays "too long" whatever the cell width is set to — with a distinct
		// head and tail so the split is checkable.
		let max = (LABEL_WIDTH / LABEL_CHAR).floor() as usize * LABEL_LINES;
		let len = max + 20;
		let half = len / 2;
		let name = format!("{}{}", "a".repeat(half), "b".repeat(len - half));

		// Act
		let shown = elide_middle(&name);

		// Assert: the middle is gone, both ends survive, and it now fits the cell's budget.
		assert!(shown.contains('…'));
		assert!(shown.starts_with('a'));
		assert!(shown.ends_with('b'));
		assert_eq!(shown.chars().count(), max);
	}

	#[test]
	fn a_short_name_is_left_untouched() {
		// Arrange
		let name = "notes.txt";

		// Act
		let shown = elide_middle(name);

		// Assert
		assert_eq!(shown, name);
		assert!(!shown.contains('…'));
	}

	/// A band in pane coordinates: the same numbers `mouse_area::on_move` reports.
	fn band(x: f32, y: f32, width: f32, height: f32) -> iced::Rectangle {
		iced::Rectangle {
			x,
			y,
			width,
			height,
		}
	}

	#[test]
	fn a_band_catches_every_cell_it_touches_and_no_others() {
		// Three columns, seven entries: rows of 3, 3 and 1.
		let columns = 3;
		let count = 7;
		let first_row = HEADER_HEIGHT + row_top(0);
		let second_row = HEADER_HEIGHT + row_top(1);

		// Over the first two cells of the top row, not the third.
		assert_eq!(
			band_hits(
				band(0.0, first_row, 2.0 * CELL_WIDTH, 10.0),
				columns,
				count,
				0.0
			),
			[0, 1]
		);

		// Down the left-hand column, through both full rows.
		assert_eq!(
			band_hits(
				band(0.0, first_row, 10.0, second_row - first_row + 10.0),
				columns,
				count,
				0.0
			),
			[0, 3]
		);

		// Past the end of the listing: the band stops at the last entry rather than
		// selecting cells that are not there.
		assert_eq!(
			band_hits(band(0.0, first_row, 1000.0, 1000.0), columns, count, 0.0),
			[0, 1, 2, 3, 4, 5, 6]
		);

		// A scrolled grid: the same rectangle on screen now covers the row below.
		assert_eq!(
			band_hits(
				band(0.0, first_row, 2.0 * CELL_WIDTH, 10.0),
				columns,
				count,
				CELL_HEIGHT + CELL_SPACING
			),
			[3, 4]
		);

		// In the gap between two rows: touching nothing selects nothing.
		let gap = HEADER_HEIGHT + row_top(1) - CELL_SPACING / 2.0;
		assert!(band_hits(band(0.0, gap, 1000.0, 0.0), columns, count, 0.0).is_empty());
	}
}

// ui/explorer.rs — the remote folder tree beside the terminal (PLAN §18).
//
// A pure view over `explorer::Explorer`: the model decides which rows exist and in
// what order (`Explorer::rows`), this file turns them into widgets. Three pieces:
//
//   * `panel`     — the fixed-width column: header, the scrollable tree, a notice line.
//   * `splitter`  — the grab bar between grid and panel; dragging it resizes the panel.
//   * `menu`      — the right-click menu, drawn by the caller as a full-window overlay
//                   (the same stacking trick the terminal's own menu uses, §10).
//
// The palette is fixed rather than themed, like the terminal grid next to it: every
// surface here sets its own background *and* foreground, so contrast never depends on
// the system light/dark preference (the trap §14 documents).

use iced::alignment::Vertical;
use iced::widget::text::Wrapping;
use iced::widget::{checkbox, column, container, mouse_area, row, scrollable, text, text_input};
use iced::{Border, Color, Element, Length, Padding, mouse};

use crate::app::Message;
use crate::explorer::{Explorer, ExplorerMessage, Rename, Row};
use crate::ui::menu;

/// The widget id of the inline rename field, so `app` can focus it the instant the
/// rename starts — the user types straight away, no click needed (§14, §18).
pub const RENAME_INPUT_ID: &str = "explorer-rename";

/// The widget id of the tree's scrollable, so `app` can scroll a keyboard-moved selection
/// back into view (§20).
pub const TREE_ID: &str = "explorer-tree";

/// Panel surfaces: a touch darker than the status bar so the tree reads as its own
/// region, with the selected row taking the same blue the grid's selection uses. Shared
/// with the files pane below (§19) — the two panels are one region visually, so the
/// palette has exactly one definition.
pub(crate) const PANEL_BG: Color = Color::from_rgb8(0x25, 0x25, 0x25);
pub(crate) const HEADER_BG: Color = Color::from_rgb8(0x2d, 0x2d, 0x2d);
pub(crate) const SPLITTER_BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);
/// The splitter bar's colour while it is the active handle — hovered or being dragged (§18,
/// §19). A clearly brighter grey than its resting `SPLITTER_BG`, so the bar answers the
/// pointer the way the resize cursor does: this is the thing you can grab. Shared by both
/// splitters, like `SPLITTER_BG`, so the two handles feel identical.
pub(crate) const SPLITTER_HOVER: Color = Color::from_rgb8(0x5a, 0x5a, 0x5a);
pub(crate) const FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
pub(crate) const MUTED_FG: Color = Color::from_rgb8(0x90, 0x90, 0x90);
pub(crate) const SELECTED_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);
/// The notice line's colour — a warm red that stays readable on the panel's dark fill.
pub(crate) const NOTICE_FG: Color = Color::from_rgb8(0xe0, 0x80, 0x70);
/// The ring drawn round whichever panel currently owns the keyboard (§20).
pub(crate) const FOCUS_FG: Color = Color::from_rgb8(0x5a, 0x8a, 0xd0);

/// Type size and row geometry. `ROW_HEIGHT` is fixed for the same reason the home
/// screen's is (§14): the context menu is placed from a row *index*, because iced does
/// not expose where a laid-out widget ended up — and, for the same reason, so is the
/// keyboard's scroll-into-view (§20).
pub(crate) const TEXT_SIZE: f32 = 13.0;
pub const ROW_HEIGHT: f32 = 22.0;
const INDENT: f32 = 12.0;

/// The header's padding and the height of one wrapped line of the path (§22). The path can
/// be any length, so the header is no longer a fixed height — it grows a line at a time as
/// the path wraps. At a single line it still comes to the tree row's own height, so a short
/// path lines the two panels' headers up as before.
const HEADER_PAD_V: f32 = 6.0;
const HEADER_PAD_H: f32 = 8.0;
const PATH_LINE_HEIGHT: f32 = 16.0;
/// The most lines the path is allowed to wrap to before the shared middle-ellipsis (§22)
/// trims it — kept to the same two the file grid's names use, so a deep path can no longer
/// grow the header without bound and crowd the tree beneath it.
const PATH_LINES: usize = 2;
/// The room the copy button, the refresh button, the collapse-all button and the `.*` toggle take
/// along the header's first line, and a glyph advance at `TEXT_SIZE`. These size the path's
/// middle-ellipsis (`path_per_line`) as well as the wrapped-line count `header_height` feeds the
/// scroll math, so they are deliberately PESSIMISTIC — a fatter glyph and more control room than
/// the face and buttons truly take — so the char budget lands under what two lines really hold and
/// the `…` trims a deep path with margin to spare rather than letting it spill onto a third line.
const TOGGLE_WIDTH: f32 = 44.0;
const COPY_BUTTON_WIDTH: f32 = 28.0;
const REFRESH_BUTTON_WIDTH: f32 = 28.0;
const COLLAPSE_BUTTON_WIDTH: f32 = 28.0;
const AVG_CHAR_WIDTH: f32 = 8.0;
/// The notice line's height, fixed so both panels can subtract it from their scrollable
/// area exactly rather than guessing at a padded line of text (§20).
pub(crate) const NOTICE_HEIGHT: f32 = 21.0;

/// How close to the window's edge a context menu may come when the pointer sits too
/// close to it to fit. Its width is the shared one (`ui::menu::WIDTH`). Shared with the
/// files pane (§19).
pub(crate) const MENU_INSET: f32 = 8.0;

/// The tree panel: a header (title plus the hidden-folder toggle), the rows, and — when
/// something went wrong — a notice line pinned under them. Fixed to the model's current
/// width so `grid_size` can subtract exactly that (§18).
///
/// The whole panel is wrapped in a `mouse_area` that reports the pointer, because a
/// right-press carries no coordinates of its own — the same trick the terminal grid uses
/// to place its own menu (§10). The rows inside handle their own presses, so this only
/// picks up the moves they ignore.
/// `focused` draws the ring that says the keyboard is here (§20). `path` is the files
/// view's directory (`Files::path`), shown in the header so this panel names the same
/// location as the pane beneath it (§22).
pub fn panel<'a>(
	explorer: &'a Explorer,
	path: Option<&str>,
	focused: bool,
) -> Element<'a, Message> {
	let mut content = column![header(explorer, path), tree(explorer)].spacing(0);
	if let Some(notice) = explorer.notice() {
		content = content.push(
			container(text(notice.to_owned()).size(TEXT_SIZE).color(NOTICE_FG))
				.width(Length::Fill)
				.height(Length::Fixed(NOTICE_HEIGHT))
				.padding(Padding::from([0.0, 8.0])),
		);
	}

	mouse_area(
		container(content)
			.width(Length::Fixed(explorer.width()))
			.height(Length::Fill)
			.style(move |_theme| container::Style {
				background: Some(PANEL_BG.into()),
				border: focus_border(focused),
				..container::Style::default()
			}),
	)
	.on_move(|point| Message::Explorer(ExplorerMessage::PointerMoved(point)))
	// A press anywhere in the panel gives it the keyboard (§20).
	.on_press(Message::Explorer(ExplorerMessage::PanelPressed))
	.into()
}

/// The border a panel wears while it owns the keyboard (§20) — and no border at all
/// otherwise, so the two panels only ever differ by the one that has the focus. Shared
/// with the files pane, which is the other end of the same Ctrl+Tab ring.
pub(crate) fn focus_border(focused: bool) -> Border {
	Border {
		width: if focused { 1.0 } else { 0.0 },
		radius: 0.0.into(),
		color: if focused {
			FOCUS_FG
		} else {
			Color::TRANSPARENT
		},
	}
}

/// How tall the scrollable part of the tree is (§20): the browser strip's height — which the
/// tree now shares with the files pane beside it (§18, §19) — less this panel's own header.
/// What "on screen" means when the app scrolls a keyboard-moved row back into view.
///
/// `ponytail:` the notice line, when one is showing, is not subtracted — the estimate is
/// then one line generous and the tree scrolls very slightly further than it had to. The
/// pane's own `grid_height` is exact because it owns its height; this one is derived from it.
pub fn tree_height(pane_height: f32, path: Option<&str>, width: f32) -> f32 {
	(pane_height - header_height(path, width)).max(0.0)
}

/// How many characters of the path fit on one header line in a panel `width` wide — the
/// usable width (less the `.*` toggle, the copy button and the padding) over an average glyph
/// advance. Shared by `header`, which middle-ellipsises the path to `PATH_LINES` of these, and
/// `header_height`, which counts the wrapped lines, so the two agree on what "a line" holds.
fn path_per_line(width: f32) -> f32 {
	let usable = (width
		- TOGGLE_WIDTH
		- COPY_BUTTON_WIDTH
		- REFRESH_BUTTON_WIDTH
		- COLLAPSE_BUTTON_WIDTH
		- 2.0 * HEADER_PAD_H)
		.max(1.0);
	(usable / AVG_CHAR_WIDTH).floor().max(1.0)
}

/// Roughly how tall the header is for `path` in a panel `width` wide (§20, §22). The path
/// wraps a line at a time but no further than `PATH_LINES` — beyond that `header` trims it with
/// a middle `…` — so the header grows to at most two lines and `tree_height` subtracts that.
///
/// `ponytail:` the average-advance guess makes this approximate for a proportional font — a
/// long path may make the tree scroll a line more than it strictly must, the same tolerance
/// the notice line already carries. The clamp mirrors the cap `header` draws to; the header is
/// still `Shrink`, so a short path shrinks it back to one line.
pub fn header_height(path: Option<&str>, width: f32) -> f32 {
	let per_line = path_per_line(width);
	let chars = path.map_or(0, |path| path.chars().count()) as f32;
	let lines = (chars / per_line).ceil().clamp(1.0, PATH_LINES as f32);
	2.0 * HEADER_PAD_V + lines * PATH_LINE_HEIGHT
}

/// The dot-entry toggle, shared by this panel's header and the files pane's (§19) —
/// there is ONE flag (`Explorer::show_hidden`) and it filters both, so both checkboxes
/// show and flip the same state. A real `checkbox`: its tick comes from iced's built-in
/// icon font, so it needs no glyph from the system fonts. Its colours are spelled out
/// rather than themed, like everything else here.
pub(crate) fn hidden_toggle(shown: bool) -> Element<'static, Message> {
	checkbox(shown)
		.label(".*")
		.size(TEXT_SIZE)
		.text_size(TEXT_SIZE)
		.spacing(6.0)
		.style(move |_theme, _status| checkbox::Style {
			background: if shown { SELECTED_BG } else { PANEL_BG }.into(),
			icon_color: FG,
			border: Border {
				width: 1.0,
				radius: 3.0.into(),
				color: MUTED_FG,
			},
			text_color: Some(FG),
		})
		.on_toggle(|_| Message::Explorer(ExplorerMessage::HiddenToggled))
		.into()
}

/// The panel header: the current directory (§22), wrapped across as many lines as it needs
/// so the whole path stays legible in this narrow column, with the dot-folder toggle pinned
/// to its top-right. `path` is the files view's directory, passed in so the two views name
/// the SAME location — the tree can be scrolled or its selection can sit elsewhere, but this
/// header tracks the pane beneath it. Empty before the first listing, like the pane's own.
fn header(explorer: &Explorer, path: Option<&str>) -> Element<'static, Message> {
	// The button reads live from `Files::path` (§22), so it needs nothing but whether one
	// exists yet — before the first listing there is nothing to copy and it dims.
	let has_path = path.is_some();
	// Trimmed to two lines' worth of glyphs so a deep path stays legible in this narrow column
	// without pushing the tree off the bottom (§22); the copy button holds the whole path.
	let per_line = path_per_line(explorer.width()) as usize;
	let path = crate::ui::elide_middle(path.unwrap_or("no directory yet"), per_line * PATH_LINES);
	container(
		row![
			text(path)
				.size(TEXT_SIZE)
				.color(FG)
				.width(Length::Fill)
				// Break inside a long run too: a path is mostly one unbroken string with no
				// spaces to wrap at, so word-wrapping alone would just overflow the column.
				.wrapping(Wrapping::WordOrGlyph),
			// Right after the path it copies, the twin of the files pane's own button (§22).
			crate::ui::files::copy_button(
				has_path,
				Message::Explorer(ExplorerMessage::CopyCurrentPath),
			),
			// Re-list every open folder in one press: the tree's header refresh, matched by
			// the pane's own below (§18).
			crate::ui::files::refresh_button(Message::Explorer(ExplorerMessage::RefreshTree)),
			// Close every branch back to the root's children — the tree's own control, no pane twin.
			crate::ui::files::collapse_all_button(Message::Explorer(ExplorerMessage::CollapseAll)),
			hidden_toggle(explorer.show_hidden()),
		]
		.spacing(6)
		// The toggle sits with the path's first line; the wrapped continuation lines fall
		// below it, so both align to the top rather than to a growing block's centre.
		.align_y(Vertical::Top),
	)
	.width(Length::Fill)
	.height(Length::Shrink)
	.padding(Padding::from([HEADER_PAD_V, HEADER_PAD_H]))
	.style(|_theme| container::Style {
		background: Some(HEADER_BG.into()),
		..container::Style::default()
	})
	.into()
}

/// The scrollable list of folder rows.
fn tree(explorer: &Explorer) -> Element<'_, Message> {
	let selected = explorer.selected();
	let editing = explorer.editing();
	let rows = explorer
		.rows()
		.into_iter()
		.map(|row| row_view(row, selected, editing));

	scrollable(column(rows).spacing(0))
		.id(TREE_ID)
		// Reported so keyboard navigation knows which rows are already on screen (§20).
		.on_scroll(|viewport| {
			Message::Explorer(ExplorerMessage::Scrolled(viewport.absolute_offset().y))
		})
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
}

/// One folder row: an indent for its depth, a disclosure marker, then its name. The
/// whole row is clickable — a left click selects and opens/closes it, a right click
/// opens the menu on it. A row being renamed shows its edit field instead.
fn row_view<'a>(
	row: Row,
	selected: Option<&str>,
	editing: Option<&'a Rename>,
) -> Element<'a, Message> {
	let indent = Length::Fixed(f32::from(row.depth) * INDENT);
	if let Some(rename) = editing.filter(|rename| rename.path == row.path) {
		return container(
			iced::widget::row![
				container(text("")).width(indent),
				text_input("Name", &rename.text)
					.id(RENAME_INPUT_ID)
					.size(TEXT_SIZE)
					.padding(Padding::from([0.0, 4.0]))
					.on_input(|value| Message::Explorer(ExplorerMessage::RenameEdited(value)))
					.on_submit(Message::Explorer(ExplorerMessage::RenameCommitted)),
			]
			.align_y(Vertical::Center),
		)
		.height(Length::Fixed(ROW_HEIGHT))
		.padding(Padding::from([0.0, 6.0]))
		.align_y(Vertical::Center)
		.into();
	}

	// A folder whose listing is in flight shows a dot rather than an arrow, so a slow
	// server reads as "working" instead of "empty".
	let marker = match (row.loading, row.open) {
		(true, _) => "·",
		(false, true) => "v",
		(false, false) => ">",
	};
	let is_selected = selected == Some(row.path.as_str());

	let label = iced::widget::row![
		container(text("")).width(indent),
		text(marker).size(TEXT_SIZE).color(MUTED_FG),
		text(row.name).size(TEXT_SIZE).color(FG),
	]
	.spacing(6)
	.align_y(Vertical::Center);

	let cell = container(label)
		.width(Length::Fill)
		.height(Length::Fixed(ROW_HEIGHT))
		.padding(Padding::from([0.0, 6.0]))
		.align_y(Vertical::Center)
		.style(move |_theme| container::Style {
			background: is_selected.then(|| SELECTED_BG.into()),
			..container::Style::default()
		});

	let path = row.path;
	mouse_area(cell)
		.on_press(Message::Explorer(ExplorerMessage::RowClicked(path.clone())))
		.on_right_press(Message::Explorer(ExplorerMessage::RowRightClicked(path)))
		.into()
}

/// The grab bar between the grid and the panel (§18). Pressing it starts a resize; the
/// pointer-capture layer added while dragging reports the moves and the release, so
/// tracking survives the pointer leaving the bar — the same construction a dragged
/// dialog uses (§10).
///
/// `active` lights the bar (hovered or dragging, from `Explorer::splitter_active`); the
/// `ResizingHorizontally` cursor (a ↔) shows over it because it resizes the panel's WIDTH,
/// and `on_enter`/`on_exit` feed the hover half of that highlight back to the model.
pub fn splitter(active: bool) -> Element<'static, Message> {
	let fill = if active { SPLITTER_HOVER } else { SPLITTER_BG };
	mouse_area(
		container(text(""))
			.width(Length::Fixed(crate::explorer::SPLITTER_WIDTH))
			.height(Length::Fill)
			.style(move |_theme| container::Style {
				background: Some(fill.into()),
				..container::Style::default()
			}),
	)
	.interaction(mouse::Interaction::ResizingHorizontally)
	.on_press(Message::Explorer(ExplorerMessage::SplitterGrabbed))
	.on_release(Message::Explorer(ExplorerMessage::SplitterReleased))
	.on_enter(Message::Explorer(ExplorerMessage::SplitterEntered))
	.on_exit(Message::Explorer(ExplorerMessage::SplitterExited))
	.into()
}

/// The transparent full-window layer present only while the splitter is being dragged:
/// it reports every pointer move and the release, wherever the pointer has wandered to. It
/// wears the same `ResizingHorizontally` cursor as the bar, so the ↔ stays put for the whole
/// drag even as the pointer leaves the thin handle.
pub fn drag_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.interaction(mouse::Interaction::ResizingHorizontally)
		.on_move(|point| Message::Explorer(ExplorerMessage::SplitterDragged(point)))
		.on_release(Message::Explorer(ExplorerMessage::SplitterReleased))
		.into()
}

/// The right-click menu for the folder it is open on, as a full-window overlay (§18).
/// Returns `None` when no menu is open.
///
/// `cwd` is the shell's working directory: "Copy relative path" is only meaningful
/// against one, so the item is disabled when the shell has never announced it (§17).
/// `top` is where the panel starts in window coordinates (the status bar's height); the
/// rest of the placement comes from the last pointer position over the panel, so the menu
/// opens under the cursor whatever the panel's width and however far the tree is
/// scrolled.
pub fn context_menu<'a>(
	explorer: &'a Explorer,
	cwd: Option<&str>,
	top: f32,
) -> Option<Element<'a, Message>> {
	let open = explorer.menu()?;
	// The anchor was frozen when the menu opened, so it stays put while the pointer
	// travels down to an item.
	let anchor = open.at;

	let item = |label: &str, message: ExplorerMessage| {
		menu::item(label.to_owned(), Some(Message::Explorer(message)))
	};

	let path = open.path.clone();
	let panel = menu::panel(vec![
		item("Open in terminal", ExplorerMessage::Cd(path.clone())),
		// Create a subfolder inside this one (§18).
		item("New folder…", ExplorerMessage::NewFolderHere(path.clone())),
		// Send local files, or a whole local folder, into this one (§17).
		item("Upload…", ExplorerMessage::UploadHere(path.clone())),
		item(
			"Upload folder…",
			ExplorerMessage::UploadFolderHere(path.clone()),
		),
		item("Rename…", ExplorerMessage::RenameStarted(path.clone())),
		// Remove this folder and everything inside it, once confirmed (§18).
		item("Delete…", ExplorerMessage::DeleteStarted(path.clone())),
		item("Copy name", ExplorerMessage::CopyName(path.clone())),
		// Disabled without a cwd: there is nothing to be relative *to*.
		menu::item(
			"Copy relative path".to_owned(),
			cwd.map(|_| Message::Explorer(ExplorerMessage::CopyRelative(path.clone()))),
		),
		item("Copy full path", ExplorerMessage::CopyPath(path.clone())),
		// "Refresh", not "Expand": re-list this folder AND its parent, so its contents, its name
		// and its very existence are all checked — the word a user hunts for when the tree has
		// gone stale under a shell command. Collapsing a single folder is the row click or ←; the
		// header's collapse-all handles the whole tree, so neither needs a menu item.
		item("Refresh", ExplorerMessage::RefreshDir(path)),
	]);

	// Placed from the pointer, right-aligned. The panel's right edge IS the window's
	// right edge, so `panel width - pointer.x` is the pointer's distance from that edge;
	// taking the menu's own width off that puts its LEFT edge under the cursor. Clamping
	// at `MENU_INSET` is what keeps a menu opened near the right edge inside the window
	// (it then slides left instead of hanging off). Aligning right also means the view
	// never needs to know how wide the window is.
	let right = (explorer.width() - anchor.x - menu::WIDTH).max(MENU_INSET);
	Some(
		container(panel)
			.width(Length::Fill)
			.height(Length::Fill)
			.align_x(iced::alignment::Horizontal::Right)
			.padding(Padding {
				top: top + anchor.y,
				right,
				bottom: 0.0,
				left: 0.0,
			})
			.into(),
	)
}

/// The click-away layer that sits under this menu (shared chrome, §10).
pub fn dismiss_layer() -> Element<'static, Message> {
	menu::dismiss_layer(Message::Explorer(ExplorerMessage::MenuDismissed))
}

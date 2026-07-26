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
use iced::widget::{button, column, container, mouse_area, row, scrollable, text, text_input};
use iced::{Color, Element, Length, Padding};

use crate::app::Message;
use crate::explorer::{Explorer, ExplorerMessage, Rename, Row};
use crate::ui::menu;

/// The widget id of the inline rename field, so `app` can focus it the instant the
/// rename starts — the user types straight away, no click needed (§14, §18).
pub const RENAME_INPUT_ID: &str = "explorer-rename";

/// Panel surfaces: a touch darker than the status bar so the tree reads as its own
/// region, with the selected row taking the same blue the grid's selection uses.
const PANEL_BG: Color = Color::from_rgb8(0x25, 0x25, 0x25);
const HEADER_BG: Color = Color::from_rgb8(0x2d, 0x2d, 0x2d);
const SPLITTER_BG: Color = Color::from_rgb8(0x3a, 0x3a, 0x3a);
const FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const MUTED_FG: Color = Color::from_rgb8(0x90, 0x90, 0x90);
const SELECTED_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);
/// The notice line's colour — a warm red that stays readable on the panel's dark fill.
const NOTICE_FG: Color = Color::from_rgb8(0xe0, 0x80, 0x70);

/// Type size and row geometry. `ROW_HEIGHT` is fixed for the same reason the home
/// screen's is (§14): the context menu is placed from a row *index*, because iced does
/// not expose where a laid-out widget ended up.
const TEXT_SIZE: f32 = 13.0;
const ROW_HEIGHT: f32 = 22.0;
const HEADER_HEIGHT: f32 = 28.0;
const INDENT: f32 = 12.0;

/// How close to the window's right edge the context menu may come when the pointer sits
/// too far right for it to fit. Its width is the shared one (`ui::menu::WIDTH`).
const MENU_INSET: f32 = 8.0;

/// The tree panel: a header (title plus the hidden-folder toggle), the rows, and — when
/// something went wrong — a notice line pinned under them. Fixed to the model's current
/// width so `grid_size` can subtract exactly that (§18).
///
/// The whole panel is wrapped in a `mouse_area` that reports the pointer, because a
/// right-press carries no coordinates of its own — the same trick the terminal grid uses
/// to place its own menu (§10). The rows inside handle their own presses, so this only
/// picks up the moves they ignore.
pub fn panel(explorer: &Explorer) -> Element<'_, Message> {
	let mut content = column![header(explorer), tree(explorer)].spacing(0);
	if let Some(notice) = explorer.notice() {
		content = content.push(
			container(text(notice.to_owned()).size(TEXT_SIZE).color(NOTICE_FG))
				.width(Length::Fill)
				.padding(Padding::from([4.0, 8.0])),
		);
	}

	mouse_area(
		container(content)
			.width(Length::Fixed(explorer.width()))
			.height(Length::Fill)
			.style(|_theme| container::Style {
				background: Some(PANEL_BG.into()),
				..container::Style::default()
			}),
	)
	.on_move(|point| Message::Explorer(ExplorerMessage::PointerMoved(point)))
	.into()
}

/// The panel header: a label and the dot-folder toggle. The toggle is a plain button
/// whose label carries its own state (`[x]` / `[ ]`) — ASCII, so it renders in the
/// bundled font on both platforms without hunting for a checkbox glyph.
fn header(explorer: &Explorer) -> Element<'_, Message> {
	let mark = if explorer.show_hidden() { "[x]" } else { "[ ]" };
	let toggle = button(text(format!("{mark} .*")).size(TEXT_SIZE).color(FG))
		.padding(Padding::from([1.0, 6.0]))
		.style(|_theme, _status| button::Style {
			background: None,
			text_color: FG,
			..button::Style::default()
		})
		.on_press(Message::Explorer(ExplorerMessage::HiddenToggled));

	container(
		row![
			text("Remote folders")
				.size(TEXT_SIZE)
				.color(MUTED_FG)
				.width(Length::Fill),
			toggle,
		]
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

/// The scrollable list of folder rows.
fn tree(explorer: &Explorer) -> Element<'_, Message> {
	let selected = explorer.selected();
	let editing = explorer.editing();
	let rows = explorer
		.rows()
		.into_iter()
		.map(|row| row_view(row, selected, editing));

	scrollable(column(rows).spacing(0))
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
pub fn splitter() -> Element<'static, Message> {
	mouse_area(
		container(text(""))
			.width(Length::Fixed(crate::explorer::SPLITTER_WIDTH))
			.height(Length::Fill)
			.style(|_theme| container::Style {
				background: Some(SPLITTER_BG.into()),
				..container::Style::default()
			}),
	)
	.on_press(Message::Explorer(ExplorerMessage::SplitterGrabbed))
	.on_release(Message::Explorer(ExplorerMessage::SplitterReleased))
	.into()
}

/// The transparent full-window layer present only while the splitter is being dragged:
/// it reports every pointer move and the release, wherever the pointer has wandered to.
pub fn drag_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
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
		item("Rename…", ExplorerMessage::RenameStarted(path.clone())),
		item("Copy name", ExplorerMessage::CopyName(path.clone())),
		// Disabled without a cwd: there is nothing to be relative *to*.
		menu::item(
			"Copy relative path".to_owned(),
			cwd.map(|_| Message::Explorer(ExplorerMessage::CopyRelative(path.clone()))),
		),
		item("Copy full path", ExplorerMessage::CopyPath(path.clone())),
		item("Expand (refresh)", ExplorerMessage::Expand(path.clone())),
		item("Collapse", ExplorerMessage::Collapse(path)),
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

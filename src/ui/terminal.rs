// ui/terminal.rs — render the vt100 `Screen` grid as iced widgets (PLAN §9-§10).
//
// The emulator gives us a grid of cells; this draws it. Each screen row is a
// `row` of fixed-width boxes: consecutive same-styled *narrow* cells coalesce
// into one box (its width is exactly `n × CELL_WIDTH`), while a *wide* cell (CJK,
// emoji — §11) gets its own box two cells across. Pinning each box to an exact
// multiple of the cell width is what keeps columns aligned: a wide glyph our
// bundled font can't draw falls back to a system font whose advance we don't
// control, so free-flowing text would shift everything after it — the fixed box
// reserves the two columns regardless of how wide the fallback glyph actually is.
// Background fills the whole box (so a narrow glyph in a wide box still tiles);
// foreground, bold, and underline come from each cell; the cursor cell is drawn
// inverted so it is visible.

use iced::font::Weight;
use iced::widget::text::{LineHeight, Span, Wrapping};
use iced::widget::{
	button, column, container, mouse_area, progress_bar, rich_text, row, span, stack, text,
	text_editor, text_input,
};
use iced::{Color, Element, Font, Length, Point, Size};

use crate::app::{Message, TransferState};
use crate::explorer::Explorer;
use crate::files::Files;
use crate::term::Terminal;
use crate::ui::selection::{Cell, Selection};

/// Glyph size and line spacing. A fixed monospace metric — the whole grid shares
/// it, so columns line up and rows tile without gaps.
const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.2;

/// The bundled monospace font (Fira Mono, embedded in the binary — see
/// `app::MONO_FONT`). Naming it explicitly instead of `Font::MONOSPACE` means the
/// grid looks identical on every machine AND its cell advance is known exactly,
/// which is what makes the pixel↔cell resize math below correct (§9, §11).
const TERMINAL_FONT: Font = Font::with_name("Fira Mono");

/// One monospace cell in logical pixels. Height is the line box; width uses Fira
/// Mono's exact advance ratio (600/1000 em = 0.6), so both axes are precise —
/// no per-font guesswork, which is why we bundle a known font.
const CELL_WIDTH: f32 = FONT_SIZE * 0.6;
const CELL_HEIGHT: f32 = FONT_SIZE * LINE_HEIGHT;

/// Padding between the grid and its container edge. Named so `view` (which draws
/// it) and `grid_size` (which subtracts it) can never drift apart.
const GRID_PADDING: f32 = 6.0;

/// The status bar above the grid (§10): a fixed height plus its colors, text
/// size, and side padding. The fixed height matters twice — `view` renders the
/// bar at exactly this height AND `grid_size` subtracts it, so the reflow math
/// accounts for the space the bar takes and the two can never drift (the same
/// discipline as `GRID_PADDING`).
pub(crate) const STATUS_BAR_HEIGHT: f32 = 34.0;
const STATUS_BAR_TEXT: f32 = 13.0;
const STATUS_BAR_BG: Color = Color::from_rgb8(0x2d, 0x2d, 0x2d);
const STATUS_BAR_FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const STATUS_BAR_PADDING: iced::Padding = iced::Padding {
	top: 0.0,
	right: 10.0,
	bottom: 0.0,
	left: 10.0,
};

/// The default foreground/background when a cell asks for the "default" color —
/// a light-on-dark scheme, and the window's backdrop behind the whole grid.
const DEFAULT_FG: Color = Color::from_rgb8(0xd0, 0xd0, 0xd0);
const DEFAULT_BG: Color = Color::from_rgb8(0x1e, 0x1e, 0x1e);

/// The background of a selected cell (§10). A muted blue that reads clearly under
/// the default light foreground; selected cells keep their own fg, only the fill
/// changes, so text stays legible while the region is obviously highlighted.
const SELECTION_BG: Color = Color::from_rgb8(0x2f, 0x4f, 0x7a);

/// The body copy for the disconnect confirmation dialog (§10). Public so `app` can
/// seed it into the selectable dialog buffer when the modal opens.
pub const DISCONNECT_DIALOG_BODY: &str = "Ends this shell and returns to the connect form. The remote program is signalled to close; what happens to any unsaved work there is up to that program.";

/// The body copy for the upload confirmation (§17), used when the remote working
/// directory is known — the destination below is that directory. `app` appends the local
/// file and its size.
pub const UPLOAD_DIALOG_BODY: &str = "Sends this file to the directory the shell is currently in, over SFTP. Check the destination below — you can edit it before sending.";

/// The same confirmation when the shell has never announced its directory (§17): the
/// destination is then a bare file name, which the server resolves against the login
/// directory, so the user is told to make it explicit if that is not what they want.
pub const UPLOAD_DIALOG_BODY_NO_CWD: &str = "This shell does not report its working directory, so cmote cannot tell where it is. The file goes to the path below — a bare name lands in your login directory. Edit it to send the file somewhere else.";

/// The widget id of the upload dialog's destination field, so `app` can focus it as the
/// dialog opens (§17) — the path is the one thing the user may want to change, and
/// Enter in the field sends. Same trick as the passphrase prompt (§7).
pub const UPLOAD_INPUT_ID: &str = "upload-dest";

/// The body copy for the overwrite confirmation (§17). `app` appends the remote path.
/// Nothing has been written when this appears: the transfer stopped at the check.
pub const UPLOAD_EXISTS_BODY: &str = "A file already exists at this path on the server. Uploading replaces its contents — the old file is not recoverable. Nothing has been sent yet.";

/// The body of the multi-file download's collision question (§21), followed by the names
/// that clash. Nothing has been downloaded when it is asked, so every answer is safe to
/// give — including cancelling the batch outright.
pub const DOWNLOAD_EXISTS_BODY: &str = "These files are already in the folder you picked. Skipping leaves the local copies alone, saving alongside adds a -1 to the name, and replacing overwrites them — replaced files are not recoverable. Nothing has been downloaded yet.";

/// The 16 base ANSI colors (indices 0-15): the 8 standard colors then their
/// bright variants. Values follow the common xterm palette.
const ANSI_16: [(u8, u8, u8); 16] = [
	(0x00, 0x00, 0x00), // 0 black
	(0x80, 0x00, 0x00), // 1 red
	(0x00, 0x80, 0x00), // 2 green
	(0x80, 0x80, 0x00), // 3 yellow
	(0x00, 0x00, 0x80), // 4 blue
	(0x80, 0x00, 0x80), // 5 magenta
	(0x00, 0x80, 0x80), // 6 cyan
	(0xc0, 0xc0, 0xc0), // 7 white
	(0x80, 0x80, 0x80), // 8 bright black (gray)
	(0xff, 0x00, 0x00), // 9 bright red
	(0x00, 0xff, 0x00), // 10 bright green
	(0xff, 0xff, 0x00), // 11 bright yellow
	(0x00, 0x00, 0xff), // 12 bright blue
	(0xff, 0x00, 0xff), // 13 bright magenta
	(0x00, 0xff, 0xff), // 14 bright cyan
	(0xff, 0xff, 0xff), // 15 bright white
];

/// The six intensity steps of the 6×6×6 color cube (indices 16-231).
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Everything the status bar and the modals need to know about the upload feature
/// (§17), grouped so `view` keeps a readable signature. `file` is the selected local
/// file's name (`None` disables Upload), `dest` the destination path being confirmed,
/// `state` the flow's current step, and `notice` the last outcome to show in the bar.
#[derive(Debug, Clone, Copy)]
pub struct UploadView<'a> {
	pub file: Option<&'a str>,
	pub dest: &'a str,
	pub state: Option<TransferState>,
	pub notice: Option<&'a str>,
}

/// The resolved look of one cell: everything a span needs. Grouping key too —
/// consecutive cells with an equal `CellStyle` become one span.
#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
	fg: Color,
	bg: Color,
	bold: bool,
	underline: bool,
}

/// The two browser panels beside and under the grid (§18, §19), grouped so `view` keeps
/// a readable signature — the same reason `Modals` and `UploadView` exist. They travel
/// together: both take room from the grid, both draw overlays, and the tree owns the
/// dot-entry toggle that filters the pane.
#[derive(Debug, Clone, Copy)]
pub struct Panels<'a> {
	pub explorer: &'a Explorer,
	pub files: &'a Files,
	/// Which of the three the keyboard belongs to (§20): the panels draw a ring when it is
	/// theirs, and the files pane places its details popup from the window's width.
	pub focus: crate::app::Focus,
	pub width: f32,
}

/// What every modal on this screen needs (§10): whether the Disconnect confirmation is
/// open, the shared selectable body buffer whichever dialog is showing, and where the
/// card sits. Grouped because they always travel together — and because each dialog
/// added to this screen would otherwise widen `view`'s signature again.
#[derive(Debug, Clone, Copy)]
pub struct Modals<'a> {
	pub confirm_disconnect: bool,
	/// Whether the "some of these files are already there" question is open (§21).
	pub clash: bool,
	pub body: &'a text_editor::Content,
	pub drag: crate::ui::dialog::Drag,
}

/// Render the whole terminal screen (§10): a status bar on top, the vt100 grid
/// filling the rest. `endpoint` is the `user@host:port` shown in the bar,
/// `selection` the active text selection to highlight (if any), `menu` the
/// right-click context menu's anchor when it is open, `modals` whichever dialog is over
/// the shell, and `upload` the file-transfer state the bar and its dialogs show (§17).
/// The grid's own output (glyph strings, the label) is copied out and so is `'static`;
/// the returned element borrows the dialog body, so its lifetime is tied to that.
pub fn view<'a>(
	terminal: &Terminal,
	endpoint: &str,
	selection: Option<&Selection>,
	menu: Option<Point>,
	modals: Modals<'a>,
	upload: UploadView<'a>,
	panels: Panels<'a>,
) -> Element<'a, Message> {
	let Panels {
		explorer,
		files,
		focus,
		width,
	} = panels;
	let Modals {
		confirm_disconnect,
		clash,
		body: dialog_body,
		drag,
	} = modals;
	let screen = terminal.screen();
	let (rows, cols) = screen.size();
	let (cursor_row, cursor_col) = screen.cursor_position();
	let cursor_visible = !screen.hide_cursor();

	let mut lines: Vec<Element<'static, Message>> = Vec::with_capacity(rows as usize);
	for row in 0..rows {
		let on_cursor_row = cursor_visible && row == cursor_row;
		lines.push(render_row(
			screen,
			row,
			cols,
			on_cursor_row,
			cursor_col,
			selection,
		));
	}

	// The grid, on the dark backdrop, filling the space left under the status bar.
	let grid = container(column(lines).spacing(0))
		.style(|_theme| container::Style {
			background: Some(DEFAULT_BG.into()),
			..container::Style::default()
		})
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(GRID_PADDING);

	// The grid reacts to the mouse (§10): press-drag-release drives the text
	// selection and a right-press opens the context menu. `on_move` reports a point
	// local to the grid, which `app` maps to a cell via `cell_at`.
	let interactive_grid = mouse_area(grid)
		.on_press(Message::GridPressed)
		.on_move(Message::GridMoved)
		.on_release(Message::GridReleased)
		.on_right_press(Message::GridRightPressed);

	// Copy is only meaningful with a non-empty selection; the buttons/menu key off this.
	let has_selection = selection.is_some_and(|selection| !selection.is_empty());

	// Under the bar: the grid takes what is left after the explorer panel and its
	// splitter (§18). The grid is `Fill`, so hiding the panel hands its width straight
	// back — `grid_size` subtracts the same `Explorer::reserved`, which is what keeps the
	// reflow math in step with this layout.
	let body: Element<'a, Message> = if explorer.visible() {
		iced::widget::row![
			interactive_grid,
			crate::ui::explorer::splitter(),
			crate::ui::explorer::panel(explorer, files.path(), focus == crate::app::Focus::Tree),
		]
		.spacing(0)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
	} else {
		interactive_grid.into()
	};

	// Bar on top (fixed height), then the terminal row, then — full width, under both —
	// the files pane and its own horizontal splitter (§19). The bar borrows the upload
	// labels, which is why the base takes the view's `'a` lifetime.
	let mut stacked = column![
		status_bar(
			endpoint,
			has_selection,
			upload,
			explorer.visible(),
			files.visible()
		),
		body
	]
	.spacing(0);
	if files.visible() {
		stacked = stacked
			.push(crate::ui::files::splitter())
			.push(crate::ui::files::panel(
				files,
				explorer.show_hidden(),
				width,
				focus == crate::app::Focus::Files,
			));
	}
	let base: Element<'a, Message> = stacked.width(Length::Fill).height(Length::Fill).into();

	// Overlays stack on top of the base, bottom-to-top: the right-click menu (with a
	// click-away dismiss layer), then the Disconnect confirmation modal. The base and
	// overlay layers are `'static`; the confirmation panel borrows `dialog_body`, so the
	// vector — and the whole view — takes that `'a` lifetime.
	let mut layers: Vec<Element<'a, Message>> = vec![base];
	if let Some(point) = menu {
		layers.push(crate::ui::menu::dismiss_layer(Message::MenuDismissed));
		layers.push(context_menu(point, has_selection));
	}
	// The explorer's own right-click menu (§18), placed against the panel rather than
	// the pointer, and its click-away dismiss layer.
	if let Some(panel_menu) =
		crate::ui::explorer::context_menu(explorer, terminal.cwd(), STATUS_BAR_HEIGHT)
	{
		layers.push(crate::ui::explorer::dismiss_layer());
		layers.push(panel_menu);
	}
	// The files pane's own right-click menu (§19), anchored the same way.
	if let Some(pane_menu) = crate::ui::files::context_menu(files, terminal.cwd()) {
		layers.push(crate::ui::files::dismiss_layer());
		layers.push(pane_menu);
	}
	// While a splitter is being dragged, a transparent layer on top follows the pointer
	// everywhere — so the resize keeps tracking outside the bar (§18, §19).
	if explorer.dragging() {
		layers.push(crate::ui::explorer::drag_layer());
	}
	if files.dragging() {
		layers.push(crate::ui::files::drag_layer());
	}
	// And while a rubber band is being pulled, for the same reason (§21).
	if files.band().is_some() {
		layers.push(crate::ui::files::band_drag_layer());
	}
	if confirm_disconnect {
		layers.push(crate::ui::dialog::backdrop(Message::DisconnectCancelled));
		layers.push(confirm_disconnect_panel(dialog_body, drag));
	}
	// The upload confirmations (§17) use the same chrome. A running transfer shows no
	// modal — its progress lives in the status bar, so the shell stays usable.
	match upload.state {
		Some(TransferState::ConfirmPath) => {
			layers.push(crate::ui::dialog::backdrop(Message::UploadCancelled));
			layers.push(confirm_upload_panel(dialog_body, upload.dest, drag));
		}
		Some(TransferState::ConfirmOverwrite) => {
			layers.push(crate::ui::dialog::backdrop(Message::UploadCancelled));
			layers.push(confirm_overwrite_panel(dialog_body, drag));
		}
		Some(TransferState::Running { .. }) | None => {}
	}
	// The multi-file download's name-collision question (§21), same chrome again. Nothing
	// has been written when it opens: the whole batch is waiting on the answer.
	if clash {
		layers.push(crate::ui::dialog::backdrop(Message::DownloadClash(
			crate::app::ClashChoice::Cancel,
		)));
		layers.push(download_clash_panel(dialog_body, drag));
	}

	// ALWAYS a stack, even with nothing overlaid. iced keeps a widget's internal state
	// (a scrollable's offset, here the folder tree's) against its position in the widget
	// tree, so returning the bare base when there are no overlays and a `stack` when
	// there is one changes the tree's shape — and the tree scrolled itself back to the
	// top every time a menu or a dialog opened. A one-child stack costs a layout node
	// and keeps the base at the same position throughout (§18).
	stack(layers)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
}

/// The status bar (§10, §17): three zones — Copy / Paste / File… / Upload on the left,
/// the live session's `user@host:port` centered, and Disconnect on the right. Its height
/// is fixed to `STATUS_BAR_HEIGHT` so `grid_size` can subtract it exactly.
/// `has_selection` enables Copy and a picked file enables Upload — a button with no
/// `on_press` is rendered disabled by iced. While a transfer runs the centre zone shows
/// its progress instead of the endpoint, and afterwards the outcome notice until the next
/// upload.
fn status_bar<'a>(
	endpoint: &str,
	has_selection: bool,
	upload: UploadView<'a>,
	explorer_visible: bool,
	files_visible: bool,
) -> Element<'a, Message> {
	// `on_press_maybe(None)` disables Copy until there is a selection to copy.
	let copy = button(text("Copy").size(STATUS_BAR_TEXT))
		.on_press_maybe(has_selection.then_some(Message::CopyPressed));
	let paste = button(text("Paste").size(STATUS_BAR_TEXT)).on_press(Message::PastePressed);
	// Picking a file is always available (it also replaces an earlier pick); sending it
	// needs both a file and no transfer already in flight (§17).
	let pick = button(text("File…").size(STATUS_BAR_TEXT)).on_press(Message::UploadPickPressed);
	let idle = upload.state.is_none();
	let send = button(text("Upload").size(STATUS_BAR_TEXT))
		.on_press_maybe((idle && upload.file.is_some()).then_some(Message::UploadPressed));
	// The explorer toggle (§18): its label says what the panel currently is, so the
	// button reads as a state rather than a command.
	let tree = button(
		text(if explorer_visible {
			"Folders ▸"
		} else {
			"Folders ◂"
		})
		.size(STATUS_BAR_TEXT),
	)
	.on_press(Message::Explorer(crate::explorer::ExplorerMessage::Toggled));
	// The files pane's toggle (§19), reading the same way: the arrow says which way it
	// would move.
	let pane = button(
		text(if files_visible {
			"Files ▾"
		} else {
			"Files ▴"
		})
		.size(STATUS_BAR_TEXT),
	)
	.on_press(Message::Files(crate::files::FilesMessage::Toggled));
	let disconnect =
		button(text("Disconnect").size(STATUS_BAR_TEXT)).on_press(Message::DisconnectPressed);

	// Three equal-width zones. Because each takes the same `Fill` share, the middle
	// zone's centered label is centered in the *window*, not merely between the side
	// groups — so the host info stays put no matter how wide the buttons are.
	let mut buttons = row![copy, paste, pick, send].spacing(10);
	// Name the picked file next to the buttons, so Upload never sends a mystery.
	if let Some(name) = upload.file {
		buttons = buttons.push(
			text(name)
				.size(STATUS_BAR_TEXT)
				.color(STATUS_BAR_FG)
				.align_y(iced::alignment::Vertical::Center)
				.height(Length::Fill),
		);
	}
	let left = container(buttons)
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Left);
	let center = container(center_zone(endpoint, upload))
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Center);
	let right = container(row![tree, pane, disconnect].spacing(10))
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Right);

	container(
		row![left, center, right]
			.spacing(10)
			.align_y(iced::alignment::Vertical::Center),
	)
	.style(|_theme| container::Style {
		background: Some(STATUS_BAR_BG.into()),
		..container::Style::default()
	})
	.width(Length::Fill)
	.height(Length::Fixed(STATUS_BAR_HEIGHT))
	// Centre the row within the fixed-height bar; the row's own `align_y` only aligns
	// its children to each other, not the row inside this taller container.
	.align_y(iced::alignment::Vertical::Center)
	.padding(STATUS_BAR_PADDING)
	.into()
}

/// What the middle of the status bar shows (§17). A running transfer takes priority — a
/// progress bar with the byte count — then the last upload's outcome, and otherwise the
/// session's `user@host:port`, which is what the bar shows all the rest of the time.
fn center_zone<'a>(endpoint: &str, upload: UploadView<'a>) -> Element<'a, Message> {
	if let Some(TransferState::Running { sent, total }) = upload.state {
		// A total of zero has nothing to divide by. That is a download that has not yet
		// heard the file's size (§19) — or a zero-byte file — so the bar stays empty and
		// the label shows only what has actually moved.
		let (fraction, label) = if total == 0 {
			(0.0, human_bytes(sent))
		} else {
			(
				sent as f32 / total as f32,
				format!("{} / {}", human_bytes(sent), human_bytes(total)),
			)
		};
		return row![
			// `length` is the bar's long axis and `girth` its thickness — a horizontal
			// bar's width and height respectively.
			progress_bar(0.0..=1.0, fraction)
				.length(Length::Fixed(160.0))
				.girth(Length::Fixed(10.0)),
			text(label).size(STATUS_BAR_TEXT).color(STATUS_BAR_FG),
		]
		.spacing(10)
		.align_y(iced::alignment::Vertical::Center)
		.into();
	}

	let label = upload.notice.unwrap_or(endpoint).to_owned();
	text(label)
		.size(STATUS_BAR_TEXT)
		.color(STATUS_BAR_FG)
		.into()
}

/// A byte count in the units a person reads (§17). Binary units, one decimal above a
/// kibibyte — enough precision for a progress readout, no rounding surprises at the
/// boundaries.
pub fn human_bytes(bytes: u64) -> String {
	const KIB: f64 = 1024.0;
	let value = bytes as f64;
	if value < KIB {
		return format!("{bytes} B");
	}
	for (limit, unit) in [
		(KIB * KIB, "KiB"),
		(KIB * KIB * KIB, "MiB"),
		(KIB * KIB * KIB * KIB, "GiB"),
	] {
		if value < limit {
			return format!("{:.1} {unit}", value / (limit / KIB));
		}
	}
	format!("{:.1} TiB", value / (KIB * KIB * KIB * KIB))
}

/// The right-click context menu (§10): Copy selection and Paste in the shared menu
/// chrome (`ui::menu`), anchored at the click. Copy is disabled without a selection (same
/// rule as the status bar), which the chrome dims. `point` is local to the grid, which
/// sits below the status bar in the stack, so shift it down by the bar height to place the
/// panel under the cursor. `ponytail:` no edge clamping — near the window's right/bottom
/// the panel can run past the edge; good enough for v1.
fn context_menu(point: Point, has_selection: bool) -> Element<'static, Message> {
	let panel = crate::ui::menu::panel(vec![
		crate::ui::menu::item(
			"Copy selection".to_owned(),
			has_selection.then_some(Message::CopyPressed),
		),
		crate::ui::menu::item("Paste".to_owned(), Some(Message::PastePressed)),
	]);

	// A full-size transparent container whose padding positions the panel at the
	// click point (top-left aligned by default).
	container(panel)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(iced::Padding {
			top: point.y + STATUS_BAR_HEIGHT,
			right: 0.0,
			bottom: 0.0,
			left: point.x,
		})
		.into()
}

/// The Disconnect confirmation modal (§10): the shared dialog chrome
/// (`ui::dialog`) with the question in the header, a line explaining what confirming
/// does, and Cancel / Disconnect in the footer. Sits above `dim_backdrop` in the
/// stack; because Disconnect drops a live session, it takes an explicit confirm here
/// rather than acting on the status-bar button directly. The header's close (✕) and
/// the backdrop both emit `DisconnectCancelled`, so dismissing never disconnects.
fn confirm_disconnect_panel(
	dialog_body: &text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'_, Message> {
	crate::ui::dialog::dialog(
		"Disconnect from this session?".to_owned(),
		Message::DisconnectCancelled,
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel")
				.on_press(Message::DisconnectCancelled)
				.into(),
			button("Disconnect")
				.on_press(Message::DisconnectConfirmed)
				.into(),
		],
		drag,
	)
}

/// The upload confirmation (§17), in the shared dialog chrome: what the upload does and
/// which local file it sends in the (selectable) body, then the destination path in an
/// editable field — Enter in the field sends, as does the Upload button. Every dismissal
/// route emits `UploadCancelled`, so backing out never sends anything.
fn confirm_upload_panel<'a>(
	dialog_body: &'a text_editor::Content,
	dest: &'a str,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	let content = column![
		crate::ui::dialog::selectable_body(dialog_body),
		text_input("Remote path", dest)
			.id(UPLOAD_INPUT_ID)
			.on_input(Message::UploadDestChanged)
			.on_submit(Message::UploadConfirmed),
	]
	.spacing(12);

	crate::ui::dialog::dialog(
		"Upload this file?".to_owned(),
		Message::UploadCancelled,
		content.into(),
		vec![
			button("Cancel").on_press(Message::UploadCancelled).into(),
			button("Upload").on_press(Message::UploadConfirmed).into(),
		],
		drag,
	)
}

/// The overwrite confirmation (§17): the destination already holds a file. Reached only
/// after the SSH task checked and stopped, so cancelling here leaves the remote file
/// exactly as it was — nothing has been written.
fn confirm_overwrite_panel(
	dialog_body: &text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'_, Message> {
	crate::ui::dialog::dialog(
		"Replace the file on the server?".to_owned(),
		Message::UploadCancelled,
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel").on_press(Message::UploadCancelled).into(),
			button("Replace")
				.on_press(Message::UploadOverwriteConfirmed)
				.into(),
		],
		drag,
	)
}

/// The multi-file download's collision question (§21): the batch is going into one folder
/// and some of those names are already in it. Asked once for the whole batch rather than
/// once per file — twenty files with twenty collisions is one decision, not twenty. Every
/// dismissal route cancels, so backing out downloads nothing.
fn download_clash_panel<'a>(
	dialog_body: &'a text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	use crate::app::ClashChoice;

	crate::ui::dialog::dialog(
		"Some of these files are already there".to_owned(),
		Message::DownloadClash(ClashChoice::Cancel),
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel")
				.on_press(Message::DownloadClash(ClashChoice::Cancel))
				.into(),
			button("Skip them")
				.on_press(Message::DownloadClash(ClashChoice::Skip))
				.into(),
			button("Save alongside")
				.on_press(Message::DownloadClash(ClashChoice::KeepBoth))
				.into(),
			button("Replace")
				.on_press(Message::DownloadClash(ClashChoice::Replace))
				.into(),
		],
		drag,
	)
}

/// Map a pointer position (local to the grid, as `mouse_area::on_move` reports it)
/// to the grid cell under it (§10). Subtracts the grid padding, divides by the cell
/// metrics, and clamps into the grid so a drag past an edge selects the edge cell
/// rather than a phantom one off the grid.
pub fn cell_at(point: Point, rows: u16, cols: u16) -> Cell {
	let x = (point.x - GRID_PADDING).max(0.0);
	let y = (point.y - GRID_PADDING).max(0.0);
	// `as u16` truncates toward zero; x/y are non-negative, so this floors.
	let col = (x / CELL_WIDTH) as u16;
	let row = (y / CELL_HEIGHT) as u16;
	Cell {
		row: row.min(rows.saturating_sub(1)),
		col: col.min(cols.saturating_sub(1)),
	}
}

/// The (rows, cols) grid that fits `area` logical pixels, laid out exactly as
/// `view` draws it: the status bar takes `STATUS_BAR_HEIGHT` off the top, the explorer
/// panel and its splitter take `reserved_width` off the width (§18), the files pane and
/// its splitter take `reserved_height` off the height (§19) — each zero when that panel
/// is hidden — then the grid's own padding is subtracted on both axes. Rounds down so the last
/// cell is never clipped, and clamps to at least 1×1 so the emulator always has
/// a valid size. The app calls this on a window resize — and on a panel resize — to
/// reflow both the local emulator and the remote pty (§9).
pub fn grid_size(area: Size, reserved_width: f32, reserved_height: f32) -> (u16, u16) {
	let usable_width = area.width - reserved_width - 2.0 * GRID_PADDING;
	let usable_height = area.height - STATUS_BAR_HEIGHT - reserved_height - 2.0 * GRID_PADDING;
	let cols = (usable_width / CELL_WIDTH)
		.floor()
		.clamp(1.0, f32::from(u16::MAX)) as u16;
	let rows = (usable_height / CELL_HEIGHT)
		.floor()
		.clamp(1.0, f32::from(u16::MAX)) as u16;
	(rows, cols)
}

/// The window (logical) size whose content fits exactly a `cols`×`rows` grid — the
/// inverse of `grid_size`, built from the same metrics so the two never drift. Adds the
/// grid padding on both axes, the status-bar height and the space the explorer panel
/// reserves (§18), plus half a cell of slack so float rounding in `grid_size` cannot come
/// back a row/column short. `run` uses it to open the window sized for a chosen terminal
/// size *and* the panel beside it (§10, §11).
pub fn window_size(cols: u16, rows: u16, reserved_width: f32, reserved_height: f32) -> Size {
	let width =
		f32::from(cols) * CELL_WIDTH + reserved_width + 2.0 * GRID_PADDING + CELL_WIDTH / 2.0;
	let height = f32::from(rows) * CELL_HEIGHT
		+ STATUS_BAR_HEIGHT
		+ reserved_height
		+ 2.0 * GRID_PADDING
		+ CELL_HEIGHT / 2.0;
	Size::new(width, height)
}

/// One box's worth of the grid: a string of glyphs, the look they share, and how
/// many grid columns the box spans. A narrow run spans its glyph count; a single
/// wide cell spans two. Split out from rendering so the column-packing logic can
/// be unit-tested without building any widgets.
struct Run {
	content: String,
	style: CellStyle,
	cols: u16,
}

/// Pack one screen row into boxes (§11). Walks the row left to right, growing a
/// run while cells are narrow and share a style, and sealing a wide cell into its
/// own two-column run so a following cell can never merge into it (which would
/// mis-size the box). Wide *continuation* cells are skipped — the lead already
/// reserves their column.
fn plan_runs(
	screen: &vt100::Screen,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
	selection: Option<&Selection>,
) -> Vec<Run> {
	let mut runs: Vec<Run> = Vec::new();
	let mut content = String::new();
	// The open run: its style, its column span so far, and whether it is a (sealed)
	// wide run. `None` means no run is open yet.
	let mut current: Option<(CellStyle, u16, bool)> = None;

	for col in 0..cols {
		let cell = screen.cell(row, col);

		// The trailing half of a wide glyph: its column was already claimed by the
		// lead cell's two-column box, so emit nothing for it.
		if cell.is_some_and(vt100::Cell::is_wide_continuation) {
			continue;
		}

		let is_wide = cell.is_some_and(vt100::Cell::is_wide);
		let glyph = match cell {
			Some(cell) if cell.has_contents() => cell.contents().to_string(),
			_ => " ".to_string(),
		};
		let is_cursor = on_cursor_row && col == cursor_col;
		let is_selected = selection.is_some_and(|selection| selection.contains(row, col));
		let style = cell_style(cell, is_cursor, is_selected);

		// Extend only when this cell is narrow AND the open run is a narrow run of
		// the same style; a wide cell (or a wide open run) always breaks the run.
		let extend =
			matches!(current, Some((run_style, _, false)) if !is_wide && run_style == style);
		if extend {
			content.push_str(&glyph);
			if let Some((_, span_cols, _)) = current.as_mut() {
				*span_cols += 1;
			}
		} else {
			if let Some((run_style, span_cols, _)) = current.take() {
				runs.push(Run {
					content: std::mem::take(&mut content),
					style: run_style,
					cols: span_cols,
				});
			}
			content.push_str(&glyph);
			current = Some((style, if is_wide { 2 } else { 1 }, is_wide));
		}
	}
	if let Some((run_style, span_cols, _)) = current {
		runs.push(Run {
			content,
			style: run_style,
			cols: span_cols,
		});
	}
	runs
}

/// Build one screen row as a `row` of fixed-width boxes (§11), one per packed run.
fn render_row(
	screen: &vt100::Screen,
	row: u16,
	cols: u16,
	on_cursor_row: bool,
	cursor_col: u16,
	selection: Option<&Selection>,
) -> Element<'static, Message> {
	let boxes: Vec<Element<'static, Message>> =
		plan_runs(screen, row, cols, on_cursor_row, cursor_col, selection)
			.into_iter()
			.map(|run| cell_box(run.content, run.style, run.cols))
			.collect();

	// Fully qualified: the `row` parameter above shadows the `row` widget helper.
	iced::widget::row(boxes).spacing(0).into()
}

/// One fixed-width cell box: the glyph(s) drawn in a container pinned to exactly
/// `span_cols × CELL_WIDTH` (§11). The container carries the background so it fills
/// the whole box — including any slack a fallback wide glyph leaves — and clips so
/// an over-wide fallback glyph can't spill past its columns and shove the next box.
fn cell_box(content: String, style: CellStyle, span_cols: u16) -> Element<'static, Message> {
	// One span holds the run's glyphs; `Wrapping::None` keeps it on a single line
	// even when the text's measured width grazes the box width.
	let glyphs = rich_text(vec![make_span(content, style)])
		.size(FONT_SIZE)
		.line_height(LineHeight::Relative(LINE_HEIGHT))
		.wrapping(Wrapping::None);

	container(glyphs)
		.width(Length::Fixed(f32::from(span_cols) * CELL_WIDTH))
		.height(Length::Fixed(CELL_HEIGHT))
		.clip(true)
		.style(move |_theme| container::Style {
			background: Some(style.bg.into()),
			..container::Style::default()
		})
		.into()
}

/// Resolve a cell's colors and attributes into a `CellStyle`, applying inverse
/// video and the cursor highlight (each swaps fg/bg; together they cancel, which
/// matches how a real terminal draws the cursor over already-inverted text). A
/// selected cell then takes the selection fill, keeping its foreground so the text
/// stays legible; because `CellStyle` is the run-grouping key, this also breaks the
/// selected span off from its neighbours automatically (§10).
fn cell_style(cell: Option<&vt100::Cell>, is_cursor: bool, is_selected: bool) -> CellStyle {
	let (mut fg, mut bg, bold, underline) = match cell {
		Some(cell) => (
			resolve(cell.fgcolor(), DEFAULT_FG),
			resolve(cell.bgcolor(), DEFAULT_BG),
			cell.bold(),
			cell.underline(),
		),
		None => (DEFAULT_FG, DEFAULT_BG, false, false),
	};

	let inverse = cell.is_some_and(vt100::Cell::inverse);
	if inverse ^ is_cursor {
		std::mem::swap(&mut fg, &mut bg);
	}

	// The selection fill wins over the resolved background so the highlight reads
	// uniformly across the run regardless of the cells' own colors.
	if is_selected {
		bg = SELECTION_BG;
	}

	CellStyle {
		fg,
		bg,
		bold,
		underline,
	}
}

/// Map a vt100 color to an iced color. `Default` becomes the caller's default
/// (different for fg and bg); indexed colors go through the xterm-256 palette.
fn resolve(color: vt100::Color, default: Color) -> Color {
	match color {
		vt100::Color::Default => default,
		vt100::Color::Idx(index) => xterm_256(index),
		vt100::Color::Rgb(r, g, b) => Color::from_rgb8(r, g, b),
	}
}

/// The xterm 256-color palette: 0-15 base ANSI, 16-231 a 6×6×6 cube, 232-255 a
/// 24-step grayscale ramp.
fn xterm_256(index: u8) -> Color {
	if index < 16 {
		let (r, g, b) = ANSI_16[index as usize];
		return Color::from_rgb8(r, g, b);
	}
	if index < 232 {
		let value = index - 16;
		let r = CUBE_STEPS[(value / 36) as usize];
		let g = CUBE_STEPS[((value / 6) % 6) as usize];
		let b = CUBE_STEPS[(value % 6) as usize];
		return Color::from_rgb8(r, g, b);
	}
	let level = 8 + (index - 232) * 10;
	Color::from_rgb8(level, level, level)
}

/// Build a styled span for one run of same-styled cells. Foreground, weight, and
/// underline live here; the background is painted by the enclosing `cell_box` so it
/// fills the whole fixed-width box rather than only the glyphs' advance (§11).
fn make_span(content: String, style: CellStyle) -> Span<'static, ()> {
	// Pick the weight we actually bundled: Medium (500) for normal cells, Bold (700)
	// for bold. This MUST match a bundled weight exactly. We ship Fira Mono only at
	// 500 and 700 (no 400 "Regular"), and cosmic-text — with the whole system font
	// DB present at runtime — does NOT nearest-weight-match within a named family:
	// asking for `Weight::Normal` (400) finds no "Fira Mono" at 400 and silently
	// falls back to the platform default (a *proportional* font, e.g. Segoe UI),
	// which breaks the monospace grid. Medium/Bold both resolve to our real faces,
	// and every Fira Mono weight shares the 0.6 advance, so cells stay `CELL_WIDTH`.
	let font = Font {
		weight: if style.bold {
			Weight::Bold
		} else {
			Weight::Medium
		},
		..TERMINAL_FONT
	};
	span(content)
		.font(font)
		.size(FONT_SIZE)
		.color(style.fg)
		.underline(style.underline)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;

	#[test]
	fn grid_fits_area_minus_bar_and_padding_rounding_down() {
		// width:  (812 - 12)      / 8.4  = 95.2  -> 95 cols
		// height: (500 - 34 - 12) / 16.8 = 27.02 -> 27 rows  (34 = status bar)
		let (rows, cols) = grid_size(Size::new(812.0, 500.0), 0.0, 0.0);
		assert_eq!((rows, cols), (27, 95));
	}

	#[test]
	fn the_explorer_panel_takes_its_width_off_the_grid() {
		// The panel is laid out beside the grid, so the columns it costs must come out of
		// the same arithmetic the reflow uses — otherwise the pty and the view disagree
		// by exactly the panel's width (§18).
		let area = Size::new(812.0, 500.0);
		let (_, wide) = grid_size(area, 0.0, 0.0);
		let (_, narrow) = grid_size(area, 168.0, 0.0); // 168 / 8.4 = 20 columns exactly
		assert_eq!(wide - narrow, 20);
	}

	#[test]
	fn the_files_pane_takes_its_height_off_the_grid() {
		// Same discipline on the other axis (§19): the rows the pane costs must come out
		// of the arithmetic the reflow uses, or the pty and the view disagree by exactly
		// the pane's height.
		let area = Size::new(812.0, 500.0);
		let (tall, _) = grid_size(area, 0.0, 0.0);
		let (short, _) = grid_size(area, 0.0, 168.0); // 168 / 16.8 = 10 rows exactly
		assert_eq!(tall - short, 10);
	}

	#[test]
	fn tiny_area_clamps_to_at_least_one_cell() {
		// Smaller than the padding would give a negative count; clamp to 1×1.
		assert_eq!(grid_size(Size::new(1.0, 1.0), 0.0, 0.0), (1, 1));
		// A panel dragged wider than the window itself must not produce a zero or
		// negative column count — only the width is squeezed, so the rows still fit.
		let (rows, cols) = grid_size(Size::new(200.0, 200.0), 400.0, 400.0);
		assert_eq!((rows, cols), (1, 1));
	}

	#[test]
	fn window_size_fits_the_requested_grid() {
		// A window opened via `window_size` must reflow back to exactly that grid, so the
		// initial window is big enough for the intended cell count (§11) — with and
		// without the two browser panels around it (§18, §19).
		assert_eq!(
			grid_size(window_size(160, 40, 0.0, 0.0), 0.0, 0.0),
			(40, 160)
		);
		let wide = crate::explorer::DEFAULT_WIDTH + crate::explorer::SPLITTER_WIDTH;
		let tall = crate::files::DEFAULT_HEIGHT + crate::files::SPLITTER_HEIGHT;
		assert_eq!(
			grid_size(window_size(160, 40, wide, tall), wide, tall),
			(40, 160)
		);
	}

	// Pack row 0 of a grid after feeding `input` to a fresh emulator. The cursor is
	// left out (`on_cursor_row = false`) so the tests exercise the column packing
	// alone, not the cursor's inverse-video split.
	fn row_runs(input: &str, cols: u16) -> Vec<Run> {
		let mut terminal = Terminal::new(1, cols);
		terminal.process(input.as_bytes());
		plan_runs(terminal.screen(), 0, cols, false, 0, None)
	}

	#[test]
	fn narrow_cells_of_one_style_coalesce_into_a_single_box() {
		// "hello" plus trailing spaces are all the default style, so the whole row is
		// one box spanning every column.
		let runs = row_runs("hello", 20);
		assert_eq!(runs.len(), 1);
		assert!(runs[0].content.starts_with("hello"));
		assert_eq!(runs[0].cols, 20);
	}

	#[test]
	fn a_wide_glyph_gets_its_own_two_column_box() {
		// 世 is East-Asian-wide: it must be sealed into a two-column box, with the
		// narrow cells on either side kept in their own boxes.
		let cols = 10;
		let runs = row_runs("a世b", cols);
		assert_eq!(runs.len(), 3);
		assert_eq!((runs[0].content.as_str(), runs[0].cols), ("a", 1));
		assert_eq!((runs[1].content.as_str(), runs[1].cols), ("世", 2));
		assert!(runs[2].content.starts_with('b'));
		assert_eq!(runs[2].cols, cols - 3); // b + trailing spaces
	}

	#[test]
	fn packed_runs_cover_every_grid_column_exactly_once() {
		// The box widths must sum to the grid width — each wide glyph claims two
		// columns and each continuation claims none, so nothing is lost or doubled.
		let cols = 12;
		let runs = row_runs("x世y世z", cols);
		let total: u16 = runs.iter().map(|run| run.cols).sum();
		assert_eq!(total, cols);
	}

	#[test]
	fn cell_at_maps_pixels_to_cells_and_clamps() {
		// Just inside the padded top-left is cell (0, 0).
		let origin = cell_at(Point::new(GRID_PADDING + 1.0, GRID_PADDING + 1.0), 24, 80);
		assert_eq!((origin.row, origin.col), (0, 0));

		// One cell right and one cell down.
		let next = cell_at(
			Point::new(
				GRID_PADDING + CELL_WIDTH + 0.5,
				GRID_PADDING + CELL_HEIGHT + 0.5,
			),
			24,
			80,
		);
		assert_eq!((next.row, next.col), (1, 1));

		// Far past the grid clamps to the last cell, never off the grid.
		let clamped = cell_at(Point::new(100_000.0, 100_000.0), 24, 80);
		assert_eq!((clamped.row, clamped.col), (23, 79));
	}

	#[test]
	fn byte_counts_read_in_binary_units() {
		// Under a kibibyte stays exact; above it switches unit at each 1024 boundary,
		// which is what the upload progress readout shows (§17).
		assert_eq!(human_bytes(0), "0 B");
		assert_eq!(human_bytes(1023), "1023 B");
		assert_eq!(human_bytes(1024), "1.0 KiB");
		assert_eq!(human_bytes(1024 * 1024), "1.0 MiB");
		assert_eq!(human_bytes(3 * 1024 * 1024 / 2), "1.5 MiB");
		assert_eq!(human_bytes(5 * 1024 * 1024 * 1024), "5.0 GiB");
	}

	#[test]
	fn a_selection_breaks_into_its_own_highlighted_run() {
		// Selecting columns 1-2 of an all-default row splits it into three runs
		// (before / selected / after); only the middle carries the selection fill —
		// proof the highlight is both applied and isolated to the selection.
		let mut terminal = Terminal::new(1, 5);
		terminal.process(b"abcde");
		let selection = Selection::new(Cell { row: 0, col: 1 }).with_head(Cell { row: 0, col: 2 });
		let runs = plan_runs(terminal.screen(), 0, 5, false, 0, Some(&selection));

		// "a" | "bc" (selected) | "de"
		assert_eq!(runs.len(), 3);
		assert_eq!(runs[1].content, "bc");
		assert_eq!(runs[1].style.bg, SELECTION_BG);
		assert_ne!(runs[0].style.bg, SELECTION_BG);
		assert_ne!(runs[2].style.bg, SELECTION_BG);
	}
}

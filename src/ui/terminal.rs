// ui/terminal.rs — the terminal screen: status bar, grid, panels, modals (PLAN §9-§10).
//
// This lays the screen out and owns its chrome. The grid itself is one widget of its own
// (`ui::grid`), which draws every cell at an exact pixel position; the metrics both sides
// need — the cell size, the padding, the status bar's height — live here, because this is
// the module that must subtract them to work out how big the grid is.

use iced::widget::{
	button, column, container, mouse_area, progress_bar, row, stack, text, text_editor, text_input,
};
use iced::{Color, Element, Length, Point, Size};

use crate::app::{Message, TransferState};
use crate::explorer::Explorer;
use crate::files::Files;
use crate::term::Terminal;
use crate::ui::selection::{Cell, Selection};

/// Glyph size and line spacing. A fixed monospace metric — the whole grid shares
/// it, so columns line up and rows tile without gaps.
pub(crate) const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT: f32 = 1.2;

/// One monospace cell in logical pixels. Height is the line box; width uses Fira
/// Mono's exact advance ratio (600/1000 em = 0.6), so both axes are precise —
/// no per-font guesswork, which is why we bundle a known font.
pub(crate) const CELL_WIDTH: f32 = FONT_SIZE * 0.6;
pub(crate) const CELL_HEIGHT: f32 = FONT_SIZE * LINE_HEIGHT;

/// Padding between the grid and the edge of the area it is given. Named so `ui::grid`
/// (which draws inside it) and `grid_size` (which subtracts it) can never drift apart.
pub(crate) const GRID_PADDING: f32 = 6.0;

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

/// The body copy for the disconnect confirmation dialog (§10). Public so `app` can
/// seed it into the selectable dialog buffer when the modal opens.
pub const DISCONNECT_DIALOG_BODY: &str = "Ends this shell and returns to the connect form. The remote program is signalled to close; what happens to any unsaved work there is up to that program.";

/// The body copy for the upload confirmation (§17). `app` appends the names of the files
/// being sent. The destination below is a FOLDER — each file keeps its own name inside it —
/// so one file or many read the same, and the batch is confirmed once for the whole lot.
pub const UPLOAD_DIALOG_BODY: &str = "These files are sent over SFTP into the remote folder below. Edit the folder to send them somewhere else; leave it empty to use your login directory. Each file keeps its own name.";

/// The widget id of the upload dialog's destination field, so `app` can focus it as the
/// dialog opens (§17) — the folder is the one thing the user may want to change, and Enter
/// in the field sends. Same trick as the passphrase prompt (§7).
pub const UPLOAD_INPUT_ID: &str = "upload-dest";

/// The body of the upload batch's collision question (§17), followed by the names already in
/// the folder. Asked once for the whole batch after the server pre-scan, never per file — the
/// mirror of the download side (§21). Nothing has been sent when it appears, so every answer,
/// cancelling included, is safe to give.
pub const UPLOAD_CLASH_BODY: &str = "Some of these files are already in the destination folder. Skipping leaves those on the server as they are, keeping both adds a -1 to the name, and replacing overwrites them — replaced files are not recoverable. Nothing has been sent yet.";

/// The body of the multi-file download's collision question (§21), followed by the names
/// that clash. Nothing has been downloaded when it is asked, so every answer is safe to
/// give — including cancelling the batch outright.
pub const DOWNLOAD_EXISTS_BODY: &str = "These files are already in the folder you picked. Skipping leaves the local copies alone, saving alongside adds a -1 to the name, and replacing overwrites them — replaced files are not recoverable. Nothing has been downloaded yet.";

/// The body of the "new folder" dialog (§18). `app` appends the folder it will be made in. The
/// name goes in the field below; Enter there, or the Create button, sends it.
pub const NEW_FOLDER_DIALOG_BODY: &str = "A new folder is created inside the directory below. Type its name — no slashes, since that would put it somewhere else.";

/// The widget id of the "new folder" dialog's name field, so `app` can focus it as the dialog
/// opens (§18) — the name is the one thing to type, and Enter in it creates the folder.
pub const NEW_FOLDER_INPUT_ID: &str = "new-folder-name";

/// The body of the delete confirmation (§18), followed by the names being removed. Deleting is
/// not undoable and a folder takes everything inside it, so the warning is stated plainly before
/// the list — the same caution the home list's own delete carries (§14).
pub const DELETE_DIALOG_BODY: &str = "Delete these from the server? This cannot be undone — a folder is removed with everything inside it.";

/// The body of a recursive transfer's file-collision prompt (§17, §19), followed by the name of
/// the file already there. Asked one file at a time as the tree is walked: overwrite or skip just
/// this one, keep both (a -1 copy beside it), settle every later collision the same way at once,
/// or cancel the whole transfer — files already copied stay.
pub const CONFLICT_DIALOG_BODY: &str = "A file with this name is already at the destination. Choose what to do — replaced files are not recoverable. This applies as you go; \"all\" settles every remaining collision the same way.";

/// Everything the status bar and the modals need to know about the upload feature
/// (§17), grouped so `view` keeps a readable signature. `file_count` is how many local
/// files are picked (zero disables Upload) and `first_file` the first one's name, so the
/// bar can label a lone pick by name and a batch by count; `dest` is the destination folder
/// being confirmed, `state` the flow's current step, and `notice` the last outcome.
#[derive(Debug, Clone, Copy)]
pub struct UploadView<'a> {
	pub file_count: usize,
	pub first_file: Option<&'a str>,
	pub dest: &'a str,
	pub state: Option<TransferState>,
	pub notice: Option<&'a str>,
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
	/// Whether a download's "some of these files are already there" question is open (§21).
	pub clash: bool,
	/// The same, for an upload batch (§17). Separate flag because the two dialogs word the
	/// question and wire their answers differently, even though the chrome is shared.
	pub upload_clash: bool,
	/// The "new folder" dialog's typed name when it is open, `None` when closed (§18). Carries the
	/// name rather than a bare bool because the dialog's field draws from it.
	pub new_folder: Option<&'a str>,
	/// Whether the delete confirmation is open (§18).
	pub pending_delete: bool,
	/// Whether a recursive transfer's file-collision prompt is open (§17, §19).
	pub transfer_conflict: bool,
	/// The port-forwards manager and its list/add-form state (§27). Grouped in with the other
	/// modals because it is one — an overlay with the shared chrome — and it keeps `view` under
	/// the argument limit.
	pub forwards: crate::ui::forward::ForwardsView<'a>,
	pub body: &'a text_editor::Content,
	pub drag: crate::ui::dialog::Drag,
}

/// Render the whole terminal screen (§10): a status bar on top, the vt100 grid
/// filling the rest. `endpoint` is the `user@host:port` shown in the bar,
/// `selection` the active text selection to highlight (if any), `menu` the
/// right-click context menu's anchor when it is open, `modals` whichever dialog is over
/// the shell, and `upload` the file-transfer state the bar and its dialogs show (§17).
/// The grid widget borrows the emulator's screen for the frame, so the terminal, the
/// selection and the dialog body all share the returned element's lifetime.
pub fn view<'a>(
	terminal: &'a Terminal,
	endpoint: &str,
	selection: Option<&'a Selection>,
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
		upload_clash,
		new_folder,
		pending_delete,
		transfer_conflict,
		forwards,
		body: dialog_body,
		drag,
	} = modals;
	// The grid itself: one widget filling the space left under the status bar (§9).
	let grid = crate::ui::grid::grid(terminal.screen(), selection);

	// It reacts to the mouse (§10): press-drag-release drives the text selection and a
	// right-press opens the context menu. `on_move` reports a point local to the grid,
	// which `app` maps to a cell via `cell_at`. A full-screen program that has asked for
	// the mouse itself takes those events first (the grid widget captures them), so this
	// layer only ever sees the clicks that are still the user's own.
	let interactive_grid = mouse_area(grid)
		.on_press(Message::GridPressed)
		.on_move(Message::GridMoved)
		.on_release(Message::GridReleased)
		.on_right_press(Message::GridRightPressed);

	// Copy is only meaningful with a non-empty selection; the buttons/menu key off this.
	let has_selection = selection.is_some_and(|selection| !selection.is_empty());

	// Sync (§19) has something to do only when the pane names a directory the shell is not
	// already in. An exact string compare is deliberately conservative: an unknown cwd
	// (`None`, the shell never announced) never matches, so Sync stays live and a redundant
	// `cd` is a harmless no-op — far better than dimming a button that would in fact move.
	let can_sync = files.path().is_some() && files.path() != terminal.cwd();

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
			can_sync,
			upload,
			explorer.visible(),
			files.visible(),
			forwards.entries.len(),
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
		// A right-click on an OSC 8 link cell adds Open/Copy link to the menu (§24). The
		// anchor is where the click landed, so the cell — and its link, if any — is read
		// straight from it; a non-link cell leaves the menu with only its usual items.
		let link = link_at(terminal, point);
		layers.push(crate::ui::menu::dismiss_layer(Message::MenuDismissed));
		layers.push(context_menu(point, has_selection, link.as_deref()));
	}
	// The explorer's own right-click menu (§18), placed against the panel rather than
	// the pointer, and its click-away dismiss layer.
	if let Some(panel_menu) =
		crate::ui::explorer::context_menu(explorer, terminal.cwd(), STATUS_BAR_HEIGHT)
	{
		layers.push(crate::ui::explorer::dismiss_layer());
		layers.push(panel_menu);
	}
	// The files pane's own right-click menu (§19), anchored the same way — `width` so a menu
	// near the right edge slides back inside instead of spilling off it.
	if let Some(pane_menu) = crate::ui::files::context_menu(files, terminal.cwd(), width) {
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
	// The upload confirmation (§17) uses the same chrome. A running transfer shows no
	// modal — its progress lives in the status bar, so the shell stays usable.
	if let Some(TransferState::ConfirmPath) = upload.state {
		layers.push(crate::ui::dialog::backdrop(Message::UploadCancelled));
		layers.push(confirm_upload_panel(dialog_body, upload.dest, drag));
	}
	// The batch collision questions, same chrome again — a download's (§21) and an upload's
	// (§17). Nothing has been written when either opens: the whole batch waits on the answer.
	if clash {
		layers.push(crate::ui::dialog::backdrop(Message::DownloadClash(
			crate::app::ClashChoice::Cancel,
		)));
		layers.push(download_clash_panel(dialog_body, drag));
	}
	if upload_clash {
		layers.push(crate::ui::dialog::backdrop(Message::UploadClashResolved(
			crate::app::ClashChoice::Cancel,
		)));
		layers.push(upload_clash_panel(dialog_body, drag));
	}
	// The "new folder" dialog (§18): the body plus a name field. Every dismissal route cancels,
	// so backing out creates nothing.
	if let Some(name) = new_folder {
		layers.push(crate::ui::dialog::backdrop(Message::NewFolderCancelled));
		layers.push(new_folder_panel(dialog_body, name, drag));
	}
	// The delete confirmation (§18): the ✕ and the backdrop keep the entries, so dismissing never
	// deletes — the destructive action is only ever the explicit button.
	if pending_delete {
		layers.push(crate::ui::dialog::backdrop(Message::DeleteCancelled));
		layers.push(delete_panel(dialog_body, drag));
	}
	// A recursive transfer's file-collision prompt (§17, §19): six answers, the whole transfer
	// parked behind it. The ✕ and backdrop both cancel the transfer — the safe choice, since
	// resuming would need an explicit decision about the file.
	if transfer_conflict {
		layers.push(crate::ui::dialog::backdrop(
			Message::TransferConflictResolved(crate::bridge::ConflictChoice::Cancel),
		));
		layers.push(transfer_conflict_panel(dialog_body, drag));
	}
	// The port-forwards manager (§27): its own list + add form in the shared chrome. The ✕ and
	// the backdrop both just close it — nothing here is destructive, forwards are removed by
	// their own ✕ — so dismissing leaves every tunnel exactly as it was.
	if forwards.open {
		layers.push(crate::ui::dialog::backdrop(Message::ForwardsClosed));
		layers.push(crate::ui::forward::panel(forwards, drag));
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

/// The status bar (§10, §17, §19): three zones — Copy / Paste / File… / Upload / Sync on
/// the left, the live session's `user@host:port` centered, and Disconnect on the right. Its
/// height is fixed to `STATUS_BAR_HEIGHT` so `grid_size` can subtract it exactly.
/// `has_selection` enables Copy, a picked file enables Upload, and `can_sync` enables Sync —
/// a button with no `on_press` is rendered disabled by iced. While a transfer runs the centre
/// zone shows its progress instead of the endpoint, and afterwards the outcome notice until
/// the next upload.
fn status_bar<'a>(
	endpoint: &str,
	has_selection: bool,
	can_sync: bool,
	upload: UploadView<'a>,
	explorer_visible: bool,
	files_visible: bool,
	forward_count: usize,
) -> Element<'a, Message> {
	// `on_press_maybe(None)` disables Copy until there is a selection to copy.
	let copy = button(text("Copy").size(STATUS_BAR_TEXT))
		.on_press_maybe(has_selection.then_some(Message::CopyPressed));
	let paste = button(text("Paste").size(STATUS_BAR_TEXT)).on_press(Message::PastePressed);
	// Picking a file is always available (it also replaces an earlier pick); sending it
	// needs both a file and no transfer already in flight (§17).
	let pick = button(text("Files…").size(STATUS_BAR_TEXT)).on_press(Message::UploadPickPressed);
	let idle = upload.state.is_none();
	let send = button(text("Upload").size(STATUS_BAR_TEXT))
		.on_press_maybe((idle && upload.file_count > 0).then_some(Message::UploadPressed));
	// Sync (§19): type a `cd` into the shell so it follows the pane. Disabled until the pane
	// names a directory the shell is not already in — dimmed, it doubles as a tell that the
	// two are in step. It carries no path; `app` reads `Files::path` live when the press
	// arrives, so the button can never move the shell somewhere the pane has since left.
	let sync = button(text("Sync").size(STATUS_BAR_TEXT))
		.on_press_maybe(can_sync.then_some(Message::SyncPressed));
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
	// The tunnels manager (§27): opens the port-forwards dialog. The label carries the live
	// count so the bar shows at a glance how many are up without opening it.
	let tunnels_label = if forward_count > 0 {
		format!("Tunnels ({forward_count})")
	} else {
		"Tunnels".to_owned()
	};
	let tunnels =
		button(text(tunnels_label).size(STATUS_BAR_TEXT)).on_press(Message::ForwardsPressed);
	let disconnect =
		button(text("Disconnect").size(STATUS_BAR_TEXT)).on_press(Message::DisconnectPressed);

	// Three equal-width zones. Because each takes the same `Fill` share, the middle
	// zone's centered label is centered in the *window*, not merely between the side
	// groups — so the host info stays put no matter how wide the buttons are.
	let mut buttons = row![copy, paste, pick, send].spacing(10);
	// Say what is picked right after Upload — the button it belongs to — so Upload never
	// sends a mystery: a lone file by name, a batch by count, and nothing when none is picked.
	let picked = match upload.file_count {
		0 => None,
		1 => upload.first_file.map(str::to_owned),
		count => Some(format!("{count} files")),
	};
	if let Some(picked) = picked {
		buttons = buttons.push(
			text(picked)
				.size(STATUS_BAR_TEXT)
				.color(STATUS_BAR_FG)
				.align_y(iced::alignment::Vertical::Center)
				.height(Length::Fill),
		);
	}
	// Sync closes the left group, after the upload controls it is unrelated to.
	buttons = buttons.push(sync);
	let left = container(buttons)
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Left);
	let center = container(center_zone(endpoint, upload))
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Center);
	let right = container(row![tree, pane, tunnels, disconnect].spacing(10))
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

/// The URI of the OSC 8 hyperlink under a grid-local point, if the cell there is part of a
/// link (§24). The point is where a right-click landed; `cell_at` maps it to a cell and the
/// seam reads back the link. `None` for a cell with no link, so the menu shows its usual
/// items only. Returned owned because the resulting menu item must carry the URI in a
/// `'static` message.
fn link_at(terminal: &Terminal, point: Point) -> Option<String> {
	let screen = terminal.screen();
	let (rows, cols) = screen.size();
	let cell = cell_at(point, rows, cols);
	screen
		.cell(cell.row, cell.col)?
		.hyperlink()
		.map(str::to_owned)
}

/// The right-click context menu (§10): Copy selection and Paste in the shared menu
/// chrome (`ui::menu`), anchored at the click. Copy is disabled without a selection (same
/// rule as the status bar), which the chrome dims. When the clicked cell is an OSC 8 link,
/// `link` carries its URI and two more items — Open link and Copy link — are added (§24).
/// `point` is local to the grid, which sits below the status bar in the stack, so shift it
/// down by the bar height to place the panel under the cursor. `ponytail:` no edge clamping
/// — near the window's right/bottom the panel can run past the edge; good enough for v1.
fn context_menu(
	point: Point,
	has_selection: bool,
	link: Option<&str>,
) -> Element<'static, Message> {
	let mut items = vec![
		crate::ui::menu::item(
			"Copy selection".to_owned(),
			has_selection.then_some(Message::CopyPressed),
		),
		crate::ui::menu::item("Paste".to_owned(), Some(Message::PastePressed)),
		// Send local files into the shell's own working directory (§17): the picker opens,
		// then the confirmation with that folder already filled in.
		crate::ui::menu::item("Upload…".to_owned(), Some(Message::TerminalUploadPressed)),
	];
	// On a link cell, follow or copy the link too (§24). Both carry the URI, so the menu is
	// the one place the whole address is offered — handy when a link's visible text hides it.
	if let Some(uri) = link {
		items.push(crate::ui::menu::item(
			"Open link".to_owned(),
			Some(Message::LinkOpen(uri.to_owned())),
		));
		items.push(crate::ui::menu::item(
			"Copy link".to_owned(),
			Some(Message::LinkCopy(uri.to_owned())),
		));
	}
	let panel = crate::ui::menu::panel(items);

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

/// The upload confirmation (§17), in the shared dialog chrome: what the upload does and the
/// files it sends in the (selectable) body, then the destination FOLDER in an editable field
/// — Enter in the field sends, as does the Upload button. Every dismissal route emits
/// `UploadCancelled`, so backing out never sends anything. One file or many read the same:
/// the folder is the destination and each file keeps its own name inside it.
fn confirm_upload_panel<'a>(
	dialog_body: &'a text_editor::Content,
	dest: &'a str,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	let content = column![
		crate::ui::dialog::selectable_body(dialog_body),
		text_input("Remote folder", dest)
			.id(UPLOAD_INPUT_ID)
			.on_input(Message::UploadDestChanged)
			.on_submit(Message::UploadConfirmed),
	]
	.spacing(12);

	crate::ui::dialog::dialog(
		"Upload these files?".to_owned(),
		Message::UploadCancelled,
		content.into(),
		vec![
			button("Cancel").on_press(Message::UploadCancelled).into(),
			button("Upload").on_press(Message::UploadConfirmed).into(),
		],
		drag,
	)
}

/// The upload batch's collision question (§17): the batch is going into one folder and some
/// of those names are already in it, found by the server pre-scan. Asked once for the whole
/// batch, the twin of the download's `download_clash_panel` (§21) — same chrome and same four
/// answers, but each wired to `UploadClashResolved`. Every dismissal route cancels, so backing
/// out sends nothing.
fn upload_clash_panel<'a>(
	dialog_body: &'a text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	use crate::app::ClashChoice;

	crate::ui::dialog::dialog(
		"Some of these files are already there".to_owned(),
		Message::UploadClashResolved(ClashChoice::Cancel),
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel")
				.on_press(Message::UploadClashResolved(ClashChoice::Cancel))
				.into(),
			button("Skip them")
				.on_press(Message::UploadClashResolved(ClashChoice::Skip))
				.into(),
			button("Keep both")
				.on_press(Message::UploadClashResolved(ClashChoice::KeepBoth))
				.into(),
			button("Replace")
				.on_press(Message::UploadClashResolved(ClashChoice::Replace))
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

/// The "new folder" dialog (§18), in the shared dialog chrome: what it does and where in the
/// (selectable) body, then the name in an editable field — Enter there creates, as does the
/// Create button. Every dismissal route emits `NewFolderCancelled`, so backing out makes nothing.
fn new_folder_panel<'a>(
	dialog_body: &'a text_editor::Content,
	name: &'a str,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	let content = column![
		crate::ui::dialog::selectable_body(dialog_body),
		text_input("Folder name", name)
			.id(NEW_FOLDER_INPUT_ID)
			.on_input(Message::NewFolderNameChanged)
			.on_submit(Message::NewFolderConfirmed),
	]
	.spacing(12);

	crate::ui::dialog::dialog(
		"New folder".to_owned(),
		Message::NewFolderCancelled,
		content.into(),
		vec![
			button("Cancel")
				.on_press(Message::NewFolderCancelled)
				.into(),
			button("Create")
				.on_press(Message::NewFolderConfirmed)
				.into(),
		],
		drag,
	)
}

/// The delete confirmation (§18), in the shared dialog chrome: the warning and the names in the
/// (selectable) body, then Cancel / Delete. The ✕ and the backdrop both keep the entries, so
/// dismissing never deletes — the destructive action is only ever the explicit button.
fn delete_panel<'a>(
	dialog_body: &'a text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	crate::ui::dialog::dialog(
		"Delete from the server?".to_owned(),
		Message::DeleteCancelled,
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel").on_press(Message::DeleteCancelled).into(),
			button("Delete").on_press(Message::DeleteConfirmed).into(),
		],
		drag,
	)
}

/// A recursive transfer's file-collision prompt (§17, §19). Six answers is more than the shared
/// footer's one row holds, so they sit in the BODY as two rows — the three per-file answers on
/// top, the two sweeping "…all" ones and Cancel below — each button `Fill`-wide so a row divides
/// evenly. The ✕ and backdrop cancel the whole transfer, the safe default when the file's fate is
/// still undecided.
fn transfer_conflict_panel<'a>(
	dialog_body: &'a text_editor::Content,
	drag: crate::ui::dialog::Drag,
) -> Element<'a, Message> {
	use crate::bridge::ConflictChoice;

	let choice = |label: &'a str, answer: ConflictChoice| -> Element<'a, Message> {
		button(label)
			.width(Length::Fill)
			.on_press(Message::TransferConflictResolved(answer))
			.into()
	};

	let content = column![
		crate::ui::dialog::selectable_body(dialog_body),
		row![
			choice("Overwrite", ConflictChoice::Overwrite),
			choice("Keep both", ConflictChoice::KeepBoth),
			choice("Skip", ConflictChoice::Skip),
		]
		.spacing(8),
		row![
			choice("Overwrite all", ConflictChoice::OverwriteAll),
			choice("Skip all", ConflictChoice::SkipAll),
			choice("Cancel", ConflictChoice::Cancel),
		]
		.spacing(8),
	]
	.spacing(12);

	crate::ui::dialog::dialog(
		"A file is already there".to_owned(),
		Message::TransferConflictResolved(ConflictChoice::Cancel),
		content.into(),
		// The answers live in the body, so the footer carries none of its own.
		Vec::new(),
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

#[cfg(test)]
mod tests {
	use super::*;

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
}

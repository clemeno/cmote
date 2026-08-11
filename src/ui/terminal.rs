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

use crate::app::Message;
use crate::explorer::Explorer;
use crate::files::Files;
use crate::term::Terminal;
use crate::transfer::{ClashChoice, Progress, Question, Queue};
use crate::ui::selection::{Cell, Selection};

/// Glyph size and line spacing. A fixed monospace metric — the whole grid shares
/// it, so columns line up and rows tile without gaps.
pub(crate) const FONT_SIZE: f32 = 12.0;
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

/// The widget id of the scrollback find bar's query field, so Ctrl+Shift+F can focus it the instant
/// the bar opens (§35) — the same discipline as the editor's find field (§32).
pub const SEARCH_INPUT_ID: &str = "term-find";

/// The find bar's outline, and how far its card sits in from the grid's top-right corner (§35). A
/// hairline is what separates the bar from whatever output happens to be behind it, since both are
/// dark; the inset keeps it clear of the corner rather than flush against it.
const SEARCH_BAR_BORDER: Color = Color::from_rgb8(0x55, 0x55, 0x55);
const SEARCH_BAR_INSET: f32 = 8.0;

/// The body copy for the disconnect confirmation dialog (§10). Public so `app` can
/// seed it into the selectable dialog buffer when the modal opens.
pub const DISCONNECT_DIALOG_BODY: &str = "Ends this shell and returns to the connect form. The remote program is signalled to close; what happens to any unsaved work there is up to that program.";

/// The body copy for the shell-integration dialog while the server is being asked, and while it is
/// being written to (§17). Two waits, said differently, because the second one is a WRITE to the
/// user's own config file and a dialog that goes quiet mid-write is the one moment they will wonder
/// what is happening to it.
pub const INTEGRATION_ASKING_BODY: &str =
	"Looking at the login shell's configuration on the server…";
pub const INTEGRATION_WRITING_BODY: &str = "Writing…";

/// What the dialog says once the probe has answered (§17): what cmote found, what it would write,
/// and the block itself so it is read before it is installed and not after.
///
/// The block goes in the body rather than behind a "show me" toggle on purpose. This is a change to
/// a file the user's every future login reads, on a machine cmote does not own; the honest way to
/// ask for that is to put the exact text in front of them. The body is selectable and copyable
/// (§10), so it doubles as the answer for anyone who would rather paste it in by hand.
pub fn integration_found_body(
	shell: Option<crate::integration::Shell>,
	path: &str,
	installed: bool,
) -> String {
	let Some(shell) = shell else {
		return format!(
			"cmote could not tell which shell this account logs into, so there is nothing it can \
			 safely add. It looked in /etc/passwd and for a .zshrc or .bashrc under {path}.\n\n\
			 A shell that announces its directory does it with an OSC 7 escape sequence from its \
			 prompt; adding that by hand to whichever file this account reads at login has the \
			 same effect as this dialog would."
		);
	};
	if !shell.installable() {
		return format!(
			"This account logs into {}, which announces its working directory by itself — there \
			 is nothing for cmote to add.\n\n\
			 If the directory still is not showing, the shell is older than the version that \
			 started sending it (fish 3.1).",
			shell.label()
		);
	}
	if installed {
		return format!(
			"{} is already set up: cmote's block is in {path}.\n\n\
			 Removing it takes out exactly what was added — the block is bounded by its own \
			 markers — and leaves the rest of the file alone. The shell stops announcing its \
			 directory at the next login, and this session is unaffected either way.",
			shell.label()
		);
	}
	let block = crate::integration::block(shell).unwrap_or_default();
	format!(
		"This account logs into {}, which says nothing about where it is — so cmote cannot show \
		 the directory, follow a cd, or resume a reconnect where the last session left off.\n\n\
		 Installing appends this to {path}. It is typed nowhere, so it never reaches the shell's \
		 command history, and it takes effect at the NEXT login — this session is unchanged. The \
		 sequences are the ones every modern terminal reads, so other terminals benefit too, and \
		 any that do not read them ignore them.\n\n\
		 {block}",
		shell.label()
	)
}

/// What the dialog says once the file has been written (§17). It names the file, and says plainly
/// that nothing has changed in the session in front of the user — a shell reads its configuration
/// at login, and this one started before the file did.
pub fn integration_done_body(path: &str, installed: bool) -> String {
	if installed {
		format!(
			"Installed in {path}.\n\n\
			 This session is unchanged — a shell reads its configuration when it starts. The next \
			 connection to this server will announce its directory, and from then on the title, \
			 Sync, Reveal and the reconnect resume all follow the shell."
		)
	} else {
		format!(
			"Removed from {path}.\n\n\
			 This session is unchanged; the shell stops announcing its directory at the next login."
		)
	}
}

/// What the dialog says when the server refused (§17): its own words, and what that means. The
/// reason is the remote's, not a translation of it — a permissions error on a config file is
/// exactly the kind of thing the user can act on once they can read it.
pub fn integration_failed_body(reason: &str) -> String {
	format!(
		"{reason}\n\n\
		 Nothing was changed on the server. The shell's directory stays unknown, which costs the \
		 title, Sync, Reveal and the reconnect resume — everything else works as it did."
	)
}

/// The widget id of the upload dialog's destination field, so `app` can focus it as the
/// dialog opens (§17) — the folder is the one thing the user may want to change, and Enter
/// in the field sends. Same trick as the passphrase prompt (§7).
///
/// The dialog's WORDS are not here: each question the transfer flow asks is worded by the module
/// that raises it (`transfer`), since only that module knows when it is asked. What stays here is
/// the widget id, which is the view's own business.
pub const UPLOAD_INPUT_ID: &str = "upload-dest";

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

/// Which way the shell and the panes can still be brought together (§19), for the two buttons that
/// do it. One struct because they are one idea read in two directions — and because `status_bar`
/// is at its argument limit, so the pair would otherwise cost it its eighth.
///
/// `sync` types a `cd` and moves the SHELL to the pane; `reveal` moves the PANES to the shell and
/// sends nothing. Both are false when the two already agree, which is how a dimmed pair says "in
/// step" without a label for it.
#[derive(Debug, Clone, Copy)]
struct Follow {
	sync: bool,
	reveal: bool,
}

/// The two browser panels in the strip under the grid (§18, §19), grouped so `view` keeps
/// a readable signature — the same reason `Modals` exists. They travel
/// together: the files pane takes the strip's height off the grid and the tree sits in it
/// on the pane's right, both draw overlays, and the tree owns the dot-entry toggle that
/// filters the pane.
#[derive(Debug, Clone, Copy)]
pub struct Panels<'a> {
	pub explorer: &'a Explorer,
	pub files: &'a Files,
	/// Which of the three the keyboard belongs to (§20): the panels draw a ring when it is
	/// theirs, and the files pane places its details popup from the pane's width.
	pub focus: crate::app::Focus,
	/// The files pane's width — the window less the folder tree's column beside it (§18, §19).
	/// The pane's grid wraps at it, so it is what says how many columns there are and therefore
	/// where the selected cell — and the details popup beside it — sit (§20); the tree no longer
	/// takes width from the terminal, only from the pane.
	pub width: f32,
	/// The window's height (§19): the files pane's sort menu is a full-window overlay, so it needs
	/// this to convert the pane's own top edge — the pane sits at the window's bottom, so its top
	/// is `window height − pane height` — into the window space its placement is measured in.
	pub height: f32,
}

/// What every overlay on this screen needs (§10): whether the Disconnect confirmation is
/// open, the shared selectable body buffer whichever dialog is showing, and where the
/// card sits. Grouped because they always travel together — and because each dialog
/// added to this screen would otherwise widen `view`'s signature again. The scrollback
/// find bar rides along for that second reason: it is not a modal, but it is an overlay
/// over the grid, and it would otherwise be `view`'s eighth argument.
#[derive(Debug, Clone, Copy)]
pub struct Modals<'a> {
	/// Which dialog is open over this screen, `None` when none is (§10). ONE field, because one
	/// dialog: they share the body buffer below and the card beside it, which only works because
	/// only one of them can be on screen.
	pub open: Option<&'a crate::app::Modal>,
	/// The session's port forwards (§27) — the rows the manager lists when it is the open modal.
	/// Session state rather than the dialog's, so it outlives any number of opens and closes.
	pub forwards: &'a [crate::forward::ForwardEntry],
	/// The scrollback find bar's state while it is open, `None` when closed (§35). Floats over the
	/// grid rather than pushing it down, so opening it never reflows the remote pty.
	pub search: Option<&'a crate::term::search::Search>,
	pub body: &'a text_editor::Content,
	pub card: crate::ui::dialog::Card,
}

/// Render the whole terminal screen (§10): a status bar on top, the vt100 grid
/// filling the rest. `endpoint` is the `user@host:port` shown in the bar,
/// `selection` the active text selection to highlight (if any), `menu` the
/// right-click context menu's anchor when it is open, `modals` whichever dialog is over
/// the shell, and `transfers` the transfer queue the bar, the drop highlight and three of the
/// dialogs read (§17).
/// The grid widget borrows the emulator's screen for the frame, so the terminal, the
/// selection and the dialog body all share the returned element's lifetime.
pub fn view<'a>(
	terminal: &'a Terminal,
	endpoint: &'a str,
	selection: Option<&'a Selection>,
	menu: Option<Point>,
	modals: Modals<'a>,
	transfers: &'a Queue,
	panels: Panels<'a>,
) -> Element<'a, Message> {
	let Panels {
		explorer,
		files,
		focus,
		width,
		height,
	} = panels;
	let Modals {
		open: modal,
		forwards,
		search,
		body: dialog_body,
		card,
	} = modals;
	// The grid itself: one widget filling the space left under the status bar (§9). It is handed
	// the rows a shell prompt sits on (§34) so it can tick them in the left gutter, and the find
	// bar's hits that land on the screen as it is scrolled right now (§39) so it can wash every one
	// of them — resolved here, where both the bar's state and the viewport's numbers are already to
	// hand, and empty whenever the bar is shut.
	let screen = terminal.screen();
	let matches = search
		.map(|search| {
			search.visible(
				screen.history_size(),
				screen.display_offset(),
				screen.size().0,
			)
		})
		.unwrap_or_default();
	let grid = crate::ui::grid::grid(
		screen,
		selection,
		terminal.prompt_rows(),
		terminal.user_mark_rows(),
		matches,
		terminal.images(),
	);

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

	// Reveal (§19) is the same question the other way round: it has something to do when the shell
	// names a directory the panes are not both already showing. "Both" is why this is three terms
	// and not the mirror image of one — the pane can be there while the tree is not, which is
	// exactly what happens when a branch is collapsed under a selection that never moved.
	// `selected_index` is `None` for a selection inside a collapsed branch, so it is what says the
	// cwd's row is on screen rather than merely remembered.
	//
	// The strip has to be on screen at all: with the pane hidden the tree goes with it, so there
	// is nothing for a press to move in front of the user. The Files toggle is one button away,
	// and a control that answers with a change nobody can see reads as a broken one.
	let can_reveal = files.visible()
		&& match terminal.cwd() {
			Some(cwd) => {
				files.path() != Some(cwd)
					|| explorer.selected() != Some(cwd)
					|| explorer.selected_index().is_none()
			}
			// Never announced (§17): there is no directory to go to, and unlike Sync's harmless
			// duplicate `cd` there is nothing to guess at either.
			None => false,
		};

	// The terminal now has the whole area under the bar to itself — full width, alone in its
	// section (§18). The folder tree used to share this row; it moved down beside the files
	// pane, so the grid is always `Fill` here and reserves width for nothing.
	let body: Element<'a, Message> = interactive_grid.into();

	// Whether the folder tree is actually on screen: it lives inside the files strip now, so
	// it is only ever drawn beside a visible files pane (§18). Hiding the pane takes the tree
	// with it — the strip is one region.
	let tree_shown = files.visible() && explorer.visible();

	// Bar on top (fixed height), then the full-width terminal, then — the browser strip: the
	// files pane and, on its right, the folder tree (§18, §19). The strip is one fixed-height
	// row governed by the files splitter above it; the tree keeps its own splitter, now between
	// the pane and itself. The bar borrows the upload labels, which is why the base takes the
	// view's `'a` lifetime.
	let mut stacked = column![
		status_bar(
			endpoint,
			has_selection,
			Follow {
				sync: can_sync,
				reveal: can_reveal,
			},
			transfers,
			explorer.visible(),
			files.visible(),
			forwards.len(),
		),
		body
	]
	.spacing(0);
	if files.visible() {
		let pane = crate::ui::files::panel(
			files,
			explorer.show_hidden(),
			width,
			focus == crate::app::Focus::Files,
			transfers.hovering(),
		);
		// The pane fills the strip's width; the tree, when shown, takes a fixed column on its
		// right — the very width `width` was already reduced by (§18), so the pane's grid math
		// and this layout agree on where the pane ends.
		let strip: Element<'a, Message> = if tree_shown {
			iced::widget::row![
				pane,
				crate::ui::explorer::splitter(explorer.splitter_active()),
				crate::ui::explorer::panel(
					explorer,
					files.path(),
					focus == crate::app::Focus::Tree
				),
			]
			.spacing(0)
			.width(Length::Fill)
			.height(Length::Fixed(files.height()))
			.into()
		} else {
			pane
		};
		stacked = stacked
			.push(crate::ui::files::splitter(files.splitter_active()))
			.push(strip);
	}
	let base: Element<'a, Message> = stacked.width(Length::Fill).height(Length::Fill).into();

	// Overlays stack on top of the base, bottom-to-top: the right-click menu (with a
	// click-away dismiss layer), then the Disconnect confirmation modal. The base and
	// overlay layers are `'static`; the confirmation panel borrows `dialog_body`, so the
	// vector — and the whole view — takes that `'a` lifetime.
	let mut layers: Vec<Element<'a, Message>> = vec![base];
	// The scrollback find bar (§35), floating over the grid's top-right corner. Pushed first of the
	// overlays, so a context menu or a dialog opened while it is up still draws over it — and low
	// enough that the grid underneath keeps every event the bar's own widgets do not take.
	if let Some(search) = search {
		layers.push(search_bar(search));
	}
	if let Some(point) = menu {
		// A right-click on an OSC 8 link cell adds Open/Copy link to the menu (§24). The
		// anchor is where the click landed, so the cell — and its link, if any — is read
		// straight from it; a non-link cell leaves the menu with only its usual items.
		let link = link_at(terminal, point);
		layers.push(crate::ui::menu::dismiss_layer(Message::MenuDismissed));
		layers.push(context_menu(point, has_selection, link.as_deref()));
	}
	// The explorer's own right-click menu (§18), placed against the panel rather than the
	// pointer, and its click-away dismiss layer. The tree sits in the browser strip now, so its
	// top in window coordinates is the strip's top — the window height less the pane's own
	// height — not the status bar. Only drawn while the tree is actually shown.
	if tree_shown
		&& let Some(panel_menu) =
			crate::ui::explorer::context_menu(explorer, terminal.cwd(), height - files.height())
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
	// The files pane's sort menu (§19), dropped from its header button, with its own click-away
	// layer beneath. Separate from the context menus above — only ever one of the three is up.
	if let Some(sort_menu) = crate::ui::files::sort_menu(files, height, width) {
		layers.push(crate::ui::files::sort_dismiss_layer());
		layers.push(sort_menu);
	}
	// While a splitter is being dragged, a transparent layer on top follows the pointer
	// everywhere — so the resize keeps tracking outside the bar (§18, §19).
	if tree_shown && explorer.dragging() {
		layers.push(crate::ui::explorer::drag_layer());
	}
	if files.dragging() {
		layers.push(crate::ui::files::drag_layer());
	}
	// And while a rubber band is being pulled, for the same reason (§21).
	if files.band().is_some() {
		layers.push(crate::ui::files::band_drag_layer());
	}
	// The one dialog this screen has open, in the shared chrome (§10, §18, §27). One match, because
	// one dialog: they share the body buffer and the card, so two at once was never drawable — the
	// type says so now, where four independent fields only ever implied it. Every arm's backdrop
	// carries the same message its ✕ does, and none of them acts on being dismissed.
	match modal {
		Some(crate::app::Modal::Disconnect) => {
			layers.push(crate::ui::dialog::backdrop(Message::DisconnectCancelled));
			layers.push(confirm_disconnect_panel(dialog_body, card));
		}
		// The "new folder" dialog (§18): the body plus a name field, so backing out creates nothing.
		Some(crate::app::Modal::NewFolder { name, .. }) => {
			layers.push(crate::ui::dialog::backdrop(Message::NewFolderCancelled));
			layers.push(new_folder_panel(dialog_body, name, card));
		}
		// The delete confirmation (§18): the ✕ and the backdrop keep the entries, so dismissing
		// never deletes — the destructive action is only ever the explicit button.
		Some(crate::app::Modal::Delete(_)) => {
			layers.push(crate::ui::dialog::backdrop(Message::DeleteCancelled));
			layers.push(delete_panel(dialog_body, card));
		}
		// The port-forwards manager (§27): its own list + add form in the shared chrome. Nothing
		// here is destructive — forwards are removed by their own ✕ — so dismissing leaves every
		// tunnel exactly as it was.
		Some(crate::app::Modal::Forwards(form)) => {
			layers.push(crate::ui::dialog::backdrop(Message::ForwardsClosed));
			layers.push(crate::ui::forward::panel(forwards, form, card));
		}
		// Setting the remote's shell up to announce its directory (§17). Dismissing writes
		// nothing — only the explicit Install / Remove button sends anything — so the ✕ and the
		// backdrop are safe at every stage, including while a probe is still out.
		Some(crate::app::Modal::Integration(state)) => {
			layers.push(crate::ui::dialog::backdrop(Message::IntegrationClosed));
			layers.push(integration_panel(dialog_body, state, card));
		}
		None => {}
	}
	// Whichever question the transfer flow is holding, all in the same chrome (§17, §19, §21).
	// Only one is ever open — the queue raises them all off its single slot — so this is one
	// match rather than three flags that could in principle disagree. A running transfer shows no
	// modal at all: its progress lives in the status bar, so the shell stays usable.
	match transfers.asking() {
		Some(Question::Dest) => {
			layers.push(crate::ui::dialog::backdrop(Message::UploadCancelled));
			layers.push(confirm_upload_panel(dialog_body, transfers.dest(), card));
		}
		// Nothing has been written when either batch question opens: the whole batch waits on
		// the answer, so every dismissal route is safe.
		Some(Question::DownloadClash) => {
			layers.push(crate::ui::dialog::backdrop(Message::DownloadClash(
				ClashChoice::Cancel,
			)));
			layers.push(download_clash_panel(dialog_body, card));
		}
		Some(Question::UploadClash) => {
			layers.push(crate::ui::dialog::backdrop(Message::UploadClashResolved(
				ClashChoice::Cancel,
			)));
			layers.push(upload_clash_panel(dialog_body, card));
		}
		// A recursive transfer's file-collision prompt (§17, §19): six answers, the whole
		// transfer parked behind it. The ✕ and backdrop both cancel the transfer — the safe
		// choice, since resuming would need an explicit decision about the file.
		Some(Question::Conflict) => {
			layers.push(crate::ui::dialog::backdrop(
				Message::TransferConflictResolved(crate::bridge::ConflictChoice::Cancel),
			));
			layers.push(transfer_conflict_panel(dialog_body, card));
		}
		None => {}
	}
	// Becoming another account (§45). Last of the overlays, so it sits above anything else that
	// happened to be open when it was asked for: it holds a secret field, and a field the user is
	// typing a password into must not be the thing that is half covered.

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

/// The status bar (§10, §17, §19): three zones — Sync / Reveal / Copy / Paste / Files… / Upload
/// on the left, the live session's `user@host:port` centered, and Disconnect on the right. Its
/// height is fixed to `STATUS_BAR_HEIGHT` so `grid_size` can subtract it exactly.
/// `has_selection` enables Copy, a picked file enables Upload, and `follow` enables Sync and
/// Reveal — a button with no `on_press` is rendered disabled by iced. While a transfer runs the
/// centre zone shows its progress instead of the endpoint, and afterwards the outcome notice until
/// the next upload.
///
/// The account is said ONCE, by the centred endpoint. A separate account label used to sit at the
/// head of the right group — what was left of §45's withdrawn switcher — repeating on the same line
/// the `user@` the centre already carries; a bar that says the same thing twice reads as though the
/// two could differ.
fn status_bar<'a>(
	endpoint: &'a str,
	has_selection: bool,
	follow: Follow,
	transfers: &'a Queue,
	explorer_visible: bool,
	files_visible: bool,
	forward_count: usize,
) -> Element<'a, Message> {
	// Sync (§19): type a `cd` into the shell so it follows the pane. Disabled until the pane
	// names a directory the shell is not already in — dimmed, it doubles as a tell that the
	// two are in step. It carries no path; `app` reads `Files::path` live when the press
	// arrives, so the button can never move the shell somewhere the pane has since left.
	let sync = button(text("Sync").size(STATUS_BAR_TEXT))
		.on_press_maybe(follow.sync.then_some(Message::SyncPressed));
	// Reveal (§19): the same closing of the gap, read the other way — the panes come to the shell.
	// It types nothing and sends nothing; `app` reads the announced cwd when the press arrives, so
	// like Sync it can only ever mean where the other side is NOW.
	let reveal = button(text("Reveal").size(STATUS_BAR_TEXT))
		.on_press_maybe(follow.reveal.then_some(Message::RevealPressed));
	// `on_press_maybe(None)` disables Copy until there is a selection to copy.
	let copy = button(text("Copy").size(STATUS_BAR_TEXT))
		.on_press_maybe(has_selection.then_some(Message::CopyPressed));
	let paste = button(text("Paste").size(STATUS_BAR_TEXT)).on_press(Message::PastePressed);
	// Picking a file is always available (it also replaces an earlier pick); sending it
	// needs both a file and no transfer already in flight (§17).
	let pick = button(text("Files…").size(STATUS_BAR_TEXT)).on_press(Message::UploadPickPressed);
	let send = button(text("Upload").size(STATUS_BAR_TEXT))
		.on_press_maybe(transfers.can_send().then_some(Message::UploadPressed));
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
	// Sync and Reveal open the group, ahead of the clipboard and upload buttons. They are the two
	// controls here that answer "where am I", one in each direction, and the eye goes to them first
	// while working the panes below — so they sit at the edge, where a group's first items are
	// easiest to hit, rather than trailing an upload run they have nothing to do with. Adjacent
	// because they are a pair: whichever way the two sides have drifted, the answer is one of these
	// two buttons, and the one that is lit says which way it will move.
	let mut buttons = row![sync, reveal, copy, paste, pick, send]
		.spacing(10)
		.align_y(iced::alignment::Vertical::Center);
	// Say what is picked right after Upload — the button it belongs to — so Upload never
	// sends a mystery: a lone file by name, a batch by count, and nothing when none is picked.
	let picked = match transfers.picked_count() {
		0 => None,
		1 => transfers.first_picked().map(str::to_owned),
		count => Some(format!("{count} files")),
	};
	if let Some(picked) = picked {
		// Plain text, its own height: the row's `align_y` centres it against the buttons.
		buttons = buttons.push(text(picked).size(STATUS_BAR_TEXT).color(STATUS_BAR_FG));
	}
	let left = container(buttons)
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Left);
	let center = container(center_zone(endpoint, transfers))
		.width(Length::Fill)
		.align_x(iced::alignment::Horizontal::Center);
	// `align_y` on each group, not only on the row of groups: a row aligns its own children to each
	// other, so without it these settle at the top of whichever child is tallest. That is invisible
	// while a group is all buttons of one height, and shows the moment a label or a select joins them.
	let right = container(
		row![tree, pane, tunnels, disconnect]
			.spacing(10)
			.align_y(iced::alignment::Vertical::Center),
	)
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
fn center_zone<'a>(endpoint: &str, transfers: &'a Queue) -> Element<'a, Message> {
	if let Some(Progress { sent, total }) = transfers.progress() {
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
			// The ✕ stops the transfer (§16): the worker deletes the partial and winds down. A
			// deliberate cancel is final, so there is no confirmation — the same weight as
			// dismissing any status-bar action.
			button(text("✕").size(STATUS_BAR_TEXT)).on_press(Message::TransferCancelPressed),
		]
		.spacing(10)
		.align_y(iced::alignment::Vertical::Center)
		.into();
	}

	// Nothing running: the last outcome (or the endpoint), and — when a failure left something to
	// pick up — a Resume beside it that re-sends only the bytes still missing (§16).
	let label = transfers.notice().unwrap_or(endpoint).to_owned();
	let notice = text(label).size(STATUS_BAR_TEXT).color(STATUS_BAR_FG);
	if transfers.can_resume() {
		return row![
			notice,
			button(text("Resume").size(STATUS_BAR_TEXT)).on_press(Message::TransferResumePressed),
		]
		.spacing(10)
		.align_y(iced::alignment::Vertical::Center)
		.into();
	}
	notice.into()
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

/// The scrollback find bar (§35): a query field with a live `n / total` count, ↑ / ↓ steppers and
/// the shared close ✕. The arrows are drawn as directions rather than as the editor's ‹ / › because
/// in a scrollback the direction IS the meaning — ↑ walks back into history, ↓ forward toward the
/// live prompt. Enter in the field steps ↑, since a new query already lands on the newest match, so
/// back into history is where anything is left to find; Esc closes the bar (`app` watches for it,
/// as the field has no close of its own).
///
/// It floats over the grid's top-right corner instead of pushing the grid down: the grid's row
/// count is the remote pty's, so a bar that took height would resize the remote every time it
/// opened. The full-window container around it paints and captures nothing, so a click or a wheel
/// anywhere outside the bar itself still reaches the grid below.
fn search_bar(search: &crate::term::search::Search) -> Element<'_, Message> {
	let query = text_input("Find in scrollback", &search.query)
		.id(SEARCH_INPUT_ID)
		.on_input(Message::TermFindQuery)
		.on_submit(Message::TermFindStep(false))
		.padding(iced::Padding::from([3.0, 8.0]))
		.size(STATUS_BAR_TEXT)
		.width(Length::Fixed(200.0));

	// "3 / 12" once there are hits, an explicit "No results" once a query has none, and blank
	// while the bar is still idle — the same three states the editor's find bar shows (§32).
	let has_hits = search.count() > 0;
	let count_label = if search.query.is_empty() {
		String::new()
	} else if has_hits {
		format!("{} / {}", search.ordinal(), search.count())
	} else {
		"No results".to_owned()
	};

	let bar = row![
		query,
		text(count_label)
			.size(11)
			.color(STATUS_BAR_FG)
			.width(Length::Fixed(76.0)),
		step_button("\u{2191}", Message::TermFindStep(false), has_hits), // ↑ older
		step_button("\u{2193}", Message::TermFindStep(true), has_hits),  // ↓ newer
		crate::ui::dialog::close_button(Message::TermFindClose),
	]
	.spacing(6)
	.align_y(iced::alignment::Vertical::Center);

	let card = container(bar)
		.padding(iced::Padding::from([5.0, 8.0]))
		.style(|_theme| container::Style {
			background: Some(STATUS_BAR_BG.into()),
			border: iced::Border {
				radius: 4.0.into(),
				width: 1.0,
				color: SEARCH_BAR_BORDER,
			},
			..container::Style::default()
		});

	// Placed by a full-size transparent container, the same trick the context menu uses — aligned
	// right, and pushed down past the status bar so it sits inside the grid's own area.
	container(card)
		.width(Length::Fill)
		.height(Length::Fill)
		.align_x(iced::alignment::Horizontal::Right)
		.padding(iced::Padding {
			top: STATUS_BAR_HEIGHT + SEARCH_BAR_INSET,
			right: SEARCH_BAR_INSET,
			bottom: 0.0,
			left: 0.0,
		})
		.into()
}

/// One of the find bar's steppers (§35): a small square button, disabled (and dimmed by the chrome)
/// while the query has no matches to step between.
fn step_button(label: &str, message: Message, enabled: bool) -> Element<'_, Message> {
	button(text(label).size(STATUS_BAR_TEXT))
		.padding(iced::Padding::from([1.0, 6.0]))
		.on_press_maybe(enabled.then_some(message))
		.into()
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
		// Teach this remote's shell to announce its directory (§17). It lives in the menu rather
		// than on the status bar because it is a once-per-server act, not a per-moment one — and
		// the bar's Sync and Reveal, dimmed for want of exactly this, are the tell that sends
		// people looking for it.
		crate::ui::menu::item(
			"Shell integration…".to_owned(),
			Some(Message::IntegrationPressed),
		),
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
	card: crate::ui::dialog::Card,
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
		card,
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
	card: crate::ui::dialog::Card,
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
		card,
	)
}

/// The upload batch's collision question (§17): the batch is going into one folder and some
/// of those names are already in it, found by the server pre-scan. Asked once for the whole
/// batch, the twin of the download's `download_clash_panel` (§21) — same chrome and same four
/// answers, but each wired to `UploadClashResolved`. Every dismissal route cancels, so backing
/// out sends nothing.
fn upload_clash_panel<'a>(
	dialog_body: &'a text_editor::Content,
	card: crate::ui::dialog::Card,
) -> Element<'a, Message> {
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
		card,
	)
}

/// The multi-file download's collision question (§21): the batch is going into one folder
/// and some of those names are already in it. Asked once for the whole batch rather than
/// once per file — twenty files with twenty collisions is one decision, not twenty. Every
/// dismissal route cancels, so backing out downloads nothing.
fn download_clash_panel<'a>(
	dialog_body: &'a text_editor::Content,
	card: crate::ui::dialog::Card,
) -> Element<'a, Message> {
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
		card,
	)
}

/// The "new folder" dialog (§18), in the shared dialog chrome: what it does and where in the
/// (selectable) body, then the name in an editable field — Enter there creates, as does the
/// Create button. Every dismissal route emits `NewFolderCancelled`, so backing out makes nothing.
fn new_folder_panel<'a>(
	dialog_body: &'a text_editor::Content,
	name: &'a str,
	card: crate::ui::dialog::Card,
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
		card,
	)
}

/// The shell-integration dialog (§17), in the shared dialog chrome. One panel for the whole
/// errand — asking, deciding, writing, done — because it is one conversation, and a dialog that
/// closed and reopened between its steps would lose the card's position and the user's place in the
/// text they were reading.
///
/// The state decides only the FOOTER. Only `Found` with a shell cmote has a block for offers an
/// action, and it offers exactly one: Install when the block is absent, Remove when it is there.
/// Every other state offers Close alone, which is honest — there is nothing to decide while the
/// server is being asked, and nothing left to do once it has answered.
fn integration_panel<'a>(
	dialog_body: &'a text_editor::Content,
	state: &'a crate::app::Integration,
	card: crate::ui::dialog::Card,
) -> Element<'a, Message> {
	use crate::app::Integration;

	let mut buttons: Vec<Element<'a, Message>> =
		vec![button("Close").on_press(Message::IntegrationClosed).into()];
	if let Integration::Found {
		shell: Some(shell),
		installed,
		..
	} = state
		&& shell.installable()
	{
		buttons.push(if *installed {
			button("Remove").on_press(Message::IntegrationRemove).into()
		} else {
			button("Install")
				.on_press(Message::IntegrationInstall)
				.into()
		});
	}

	crate::ui::dialog::dialog(
		"Shell integration".to_owned(),
		Message::IntegrationClosed,
		crate::ui::dialog::selectable_body(dialog_body),
		buttons,
		card,
	)
}

/// The delete confirmation (§18), in the shared dialog chrome: the warning and the names in the
/// (selectable) body, then Cancel / Delete. The ✕ and the backdrop both keep the entries, so
/// dismissing never deletes — the destructive action is only ever the explicit button.
fn delete_panel<'a>(
	dialog_body: &'a text_editor::Content,
	card: crate::ui::dialog::Card,
) -> Element<'a, Message> {
	crate::ui::dialog::dialog(
		"Delete from the server?".to_owned(),
		Message::DeleteCancelled,
		crate::ui::dialog::selectable_body(dialog_body),
		vec![
			button("Cancel").on_press(Message::DeleteCancelled).into(),
			button("Delete").on_press(Message::DeleteConfirmed).into(),
		],
		card,
	)
}

/// A recursive transfer's file-collision prompt (§17, §19). Six answers is more than the shared
/// footer's one row holds, so they sit in the BODY as two rows — the three per-file answers on
/// top, the two sweeping "…all" ones and Cancel below — each button `Fill`-wide so a row divides
/// evenly. The ✕ and backdrop cancel the whole transfer, the safe default when the file's fate is
/// still undecided.
fn transfer_conflict_panel<'a>(
	dialog_body: &'a text_editor::Content,
	card: crate::ui::dialog::Card,
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
		card,
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
/// `view` draws it: the status bar takes `STATUS_BAR_HEIGHT` off the top and the files strip
/// and its splitter take `reserved_height` off the height (§19) — zero when the pane is hidden
/// — then the grid's own padding is subtracted on both axes. The terminal is full width now
/// (§18): the folder tree moved down into the files strip, so nothing reserves horizontal room
/// any more. Rounds down so the last cell is never clipped, and clamps to at least 1×1 so the
/// emulator always has a valid size. The app calls this on a window resize — and on a pane
/// resize — to reflow both the local emulator and the remote pty (§9).
pub fn grid_size(area: Size, reserved_height: f32) -> (u16, u16) {
	let usable_width = area.width - 2.0 * GRID_PADDING;
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
/// grid padding on both axes, the status-bar height and the space the files strip reserves
/// at the bottom (§19), plus half a cell of slack so float rounding in `grid_size` cannot come
/// back a row/column short. The terminal spans the full width now (§18), so only the height
/// carries a reserve. `run` uses it to open the window sized for a chosen terminal size *and*
/// the strip under it (§10, §11).
pub fn window_size(cols: u16, rows: u16, reserved_height: f32) -> Size {
	let width = f32::from(cols) * CELL_WIDTH + 2.0 * GRID_PADDING + CELL_WIDTH / 2.0;
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
		// width:  (812 - 12)      / 7.2  = 111.1 -> 111 cols
		// height: (500 - 34 - 12) / 14.4 = 31.5  -> 31 rows  (34 = status bar)
		let (rows, cols) = grid_size(Size::new(812.0, 500.0), 0.0);
		assert_eq!((rows, cols), (31, 111));
	}

	#[test]
	fn the_files_pane_takes_its_height_off_the_grid() {
		// The pane is laid out under the grid, so the rows it costs must come out of the same
		// arithmetic the reflow uses (§19), or the pty and the view disagree by exactly the
		// pane's height. It is the only reserve left: the terminal spans the full width (§18).
		let area = Size::new(812.0, 500.0);
		let (tall, _) = grid_size(area, 0.0);
		let (short, _) = grid_size(area, 144.0); // 144 / 14.4 = 10 rows exactly
		assert_eq!(tall - short, 10);
	}

	#[test]
	fn tiny_area_clamps_to_at_least_one_cell() {
		// Smaller than the padding would give a negative count; clamp to 1×1.
		assert_eq!(grid_size(Size::new(1.0, 1.0), 0.0), (1, 1));
		// A strip dragged taller than the window itself must not produce a zero or negative
		// row count — only the height is squeezed, so the columns still fit.
		let (rows, _) = grid_size(Size::new(200.0, 200.0), 400.0);
		assert_eq!(rows, 1);
	}

	#[test]
	fn window_size_fits_the_requested_grid() {
		// A window opened via `window_size` must reflow back to exactly that grid, so the
		// initial window is big enough for the intended cell count (§11) — with and without the
		// files strip reserved under it (§19).
		assert_eq!(grid_size(window_size(160, 40, 0.0), 0.0), (40, 160));
		let tall = crate::files::DEFAULT_HEIGHT + crate::files::SPLITTER_HEIGHT;
		assert_eq!(grid_size(window_size(160, 40, tall), tall), (40, 160));
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

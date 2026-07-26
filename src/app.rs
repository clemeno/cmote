// app.rs — the iced application, written in the Elm architecture (PLAN §10).
//
// Three pieces make up an iced app and they are all pure/explicit:
//   * State   — one struct (`App`) owns EVERYTHING the UI can show.
//   * Message — one enum listing every event that can happen.
//   * update  — `fn(&mut State, Message)`: the ONLY place state changes.
//   * view    — `fn(&State) -> Element`: a pure render of the current state.
//
// There is no hidden widget tree and no global mutable state. Every change
// flows through `update`, and the compiler forces us to handle each `Message`.

use std::path::PathBuf;

use iced::Element;
use iced::widget::{text, text_editor};
use tokio::sync::mpsc;

use crate::bridge::{self, SshCommand, SshEvent};
use crate::explorer::{self, ExplorerMessage};
use crate::secret::Secret;
use crate::term;
use crate::ui;
use crate::ui::connect::AuthKind;

/// The monospace font embedded in the binary (Fira Mono, OFL 1.1 — see
/// assets/FiraMono-LICENSE.txt). Bundling it keeps the terminal identical on
/// every machine and gives the grid a known cell advance, which the resize math
/// relies on (§9, §11). Registered with iced in `run` and selected by name in
/// `ui::terminal`.
const MONO_FONT: &[u8] = include_bytes!("../assets/FiraMono-Medium.ttf");

/// The bold weight of the same family (Fira Mono Bold, weight 700 — same OFL
/// licence, same Mozilla Fira release as `MONO_FONT`). Bundled so a cell the shell
/// marks bold renders in a genuinely heavier face rather than the normal one:
/// `ui::terminal` asks for `Weight::Bold`, and with only the medium weight loaded
/// iced had no 700 face to resolve to, so bold text looked identical (§11). Every
/// Fira Mono weight shares the exact 600/1000-em advance, so bundling bold does not
/// disturb the fixed cell metric the resize math depends on. Both faces share the
/// family name "Fira Mono"; iced picks the medium (500) for normal cells and the
/// bold (700) for bold ones purely by the requested weight.
const MONO_FONT_BOLD: &[u8] = include_bytes!("../assets/FiraMono-Bold.ttf");

/// The terminal size the main window opens sized for (§10, §11): wide enough for a
/// 180-column grid, with a comfortable default height. `run` converts this to a window
/// size via `ui::terminal::window_size` so it tracks the grid metrics.
const INITIAL_COLS: u16 = 180;
const INITIAL_ROWS: u16 = 40;

/// The most of the window the explorer panel may be dragged to (§18). A splitter with
/// no ceiling can push the terminal grid down to a single column, which is a state the
/// user then has to drag their way back out of.
const MAX_PANEL_FRACTION: f32 = 0.6;

/// Build and start the iced runtime. Called from `main`.
pub fn run() -> iced::Result {
	// The functional builder (iced 0.14): the first argument is the "boot"
	// function that produces the initial `(State, Task)` — here `App::new`. Then
	// the update and view functions. `.title` / `.window` / `.subscription` are builder
	// methods, and `.run()` starts the event loop.
	iced::application(App::new, App::update, App::view)
		// The title is a function of the state, not a constant: while a shell is open it
		// shows the session and the remote working directory it is sitting in (§17).
		.title(App::title)
		.font(MONO_FONT)
		.font(MONO_FONT_BOLD)
		// Open wide enough for a 180-column terminal *and* the explorer panel beside it
		// (the size is derived from the grid metrics so it stays in step with
		// `grid_size`, §18).
		.window(iced::window::Settings {
			size: ui::terminal::window_size(
				INITIAL_COLS,
				INITIAL_ROWS,
				explorer::DEFAULT_WIDTH + explorer::SPLITTER_WIDTH,
			),
			..iced::window::Settings::default()
		})
		.subscription(App::subscription)
		.run()
}

/// Which screen the single window is currently showing. This is the small state
/// machine from PLAN §10 — every transition happens in `update`.
#[derive(Debug, Default)]
pub enum Screen {
	/// The home screen: the list of saved connection targets (§14). This is where we
	/// start; picking a target pre-fills the connect form, "New connection" opens a
	/// blank one.
	#[default]
	Home,
	/// The connection form (host / port / user / auth), reached from the home screen.
	Connect,
	/// Handshake and authentication in progress; `status` is a human-readable
	/// step for the UI ("connecting", "verifying host key", "authenticating").
	Connecting { status: String },
	/// First contact with an unknown host: the server's key fingerprint is shown
	/// and the user must accept or reject before the handshake continues (§8). The
	/// fingerprint text itself lives in `App::dialog_body` (the selectable message),
	/// seeded when this state is entered — the variant is just the marker.
	ConfirmHostKey,
	/// The chosen private key is encrypted: prompt for its passphrase (§7). The
	/// text the user types lives in `App::passphrase_input`.
	NeedPassphrase,
	/// A live shell: the vt100 grid fills the window.
	Terminal,
	/// A terminal failure. The generic, non-leaking message (§12) lives in
	/// `App::dialog_body` so it can be selected and copied; this variant just marks
	/// that the error screen is showing.
	Error,
}

/// The whole application state. Owned in one place; nothing else mutates it.
#[derive(Debug, Default)]
pub struct App {
	/// Which screen is visible.
	pub screen: Screen,
	/// The saved connection targets shown on the home screen (§14). Loaded from disk
	/// at startup, kept sorted, and re-saved whenever it changes (a rename, a delete,
	/// or a successful connect). Profiles only — never any secret material (§12).
	targets: crate::profiles::Targets,
	/// The endpoint key (`user@host:port`) of the highlighted target on the home
	/// screen, if any. Drives the row highlight and is what the right-click menu and
	/// the F2/Enter/Delete shortcuts act on.
	home_selected: Option<String>,
	/// Whether the home screen's right-click menu is open (it acts on `home_selected`).
	home_menu_open: bool,
	/// Whether the delete confirmation is open for `home_selected` (§14). Deleting a
	/// target is not undoable, so — like Disconnect — the menu item and the Delete key
	/// only raise this prompt; the removal happens on an explicit confirm.
	confirm_delete: bool,
	/// The in-progress inline rename on the home screen, if any (§14).
	home_rename: Option<ui::home::RenameState>,
	/// The profile (no secret) captured when a connect is dialed, saved to `targets`
	/// once the session actually opens (§14). `None` between attempts so a failed or
	/// abandoned connect never persists a target.
	pending_target: Option<crate::profiles::Target>,
	/// The connect form's field contents. Lives here so it survives navigating
	/// to an error screen and back without losing what the user typed.
	pub form: ui::connect::ConnectForm,
	/// The connect form's current keyboard-focus stop (§10). iced can only focus text
	/// inputs, so this bespoke ring also covers the radios and the Connect button: Tab /
	/// Shift+Tab move it, Enter/Space activate it, and the view highlights the active
	/// radio/button. Text stops additionally take native focus so typing lands there.
	form_focus: ui::connect::FormStop,
	/// Channel to the SSH task. `None` until the worker starts and delivers it
	/// via `SshEvent::Ready`; `update` sends `SshCommand`s through it.
	command_tx: Option<mpsc::Sender<SshCommand>>,
	/// The terminal emulator, alive only while a shell is open. `Some` from
	/// `Connected` until `Disconnected`; output bytes are fed into it and the
	/// Terminal screen renders its grid.
	terminal: Option<term::Terminal>,
	/// The passphrase being typed on the `NeedPassphrase` screen. Kept here rather
	/// than in the form so it never lingers there; it is moved into a `Secret` on
	/// submit and the field is cleared (§12).
	passphrase_input: String,
	/// Whether a passphrase has already been submitted this connection. The SSH task
	/// re-emits `NeedPassphrase` for both the first ask and a wrong-passphrase re-ask,
	/// so this flag is how the passphrase screen knows to show its "incorrect" hint:
	/// if it is set when the prompt appears, the previous attempt was rejected (§7).
	/// Reset at the start of each connection attempt.
	passphrase_failed: bool,
	/// The `user@host:port` of the current session, shown in the terminal's status
	/// bar (§10). Set when a connection is dialed and cleared when it ends. Holds no
	/// secret, so it is safe in `Debug`.
	connection: Option<String>,
	/// The active text selection over the terminal grid, if any (§10). Drives both
	/// the on-screen highlight and what Copy puts on the clipboard; `None` when
	/// nothing is selected.
	selection: Option<ui::selection::Selection>,
	/// True while the left mouse button is held on the grid — a drag in progress.
	/// `on_move` fires on any hover, so this flag is how a drag is told from a plain
	/// move (only a drag extends the selection).
	selecting: bool,
	/// The grid cell currently under the pointer (§10). Updated on every pointer
	/// move so a press can anchor the selection here.
	hover_cell: ui::selection::Cell,
	/// The last pointer position, local to the grid, used to place the right-click
	/// context menu — a right-press carries no coordinates of its own (§10).
	pointer: iced::Point,
	/// The context menu's anchor when it is open, `None` when closed (§10).
	menu: Option<iced::Point>,
	/// Whether the Disconnect confirmation modal is open (§10). Set by the Disconnect
	/// button and cleared on confirm or cancel — it guards a live session against an
	/// accidental click.
	confirm_disconnect: bool,
	/// The body message of whatever dialog is currently open, held as `text_editor`
	/// content so the user can *select* it and copy the selection (§10). It is
	/// read-only in practice — `update` performs every action except an edit — and is
	/// reseeded each time a dialog opens. Only one dialog is ever visible, so a single
	/// buffer serves all seven (delete-target, disconnect, upload, overwrite, host-key,
	/// passphrase, error).
	dialog_body: text_editor::Content,
	/// The open dialog card's top-left position in the window (§10). Seeded to centre
	/// when a dialog opens and updated as the user drags the header, clamped so the card
	/// stays within the window.
	dialog_pos: iced::Point,
	/// Whether the open dialog is currently being dragged by its header (§10).
	dialog_dragging: bool,
	/// The pointer position at the previous drag update (§10), so successive positions
	/// become movement deltas. `None` between drags and before a drag's first move.
	dialog_drag_last: Option<iced::Point>,
	/// The last known window size (§10), tracked from resize events so a dragged dialog
	/// can be centred and clamped within the window bounds.
	window_size: iced::Size,
	/// The local file picked for upload (§17), or `None` when nothing is selected —
	/// which is also what disables the Upload button. Cleared on a successful upload, so
	/// the same file is never sent twice by a stray second click.
	upload_file: Option<PathBuf>,
	/// Where the upload is in its little flow (§17): confirming the path, confirming an
	/// overwrite, or transferring. `None` when no upload is in progress.
	upload: Option<UploadState>,
	/// The destination path being confirmed (§17). Seeded from the remote working
	/// directory plus the file's name, and editable — that is what makes the feature
	/// work on a shell that never announces its directory.
	upload_dest: String,
	/// The last upload outcome, shown in the status bar until the next upload starts
	/// (§17). `ponytail:` no timed fade — that would need a timer subscription for a
	/// line of text.
	upload_notice: Option<String>,
	/// The remote folder tree shown beside the grid (§18). It owns its own visibility,
	/// width, expansion state and selection; `app` only relays its events and turns the
	/// paths it asks for into `SshCommand::ListDir`.
	explorer: explorer::Explorer,
}

/// Where an upload has got to (§17). Only one upload runs at a time, so this is a plain
/// state, not a queue.
#[derive(Debug, Clone, Copy)]
pub enum UploadState {
	/// Showing the destination path for confirmation, before anything is sent.
	ConfirmPath,
	/// The destination already holds a file; asking whether to overwrite it.
	ConfirmOverwrite,
	/// Transferring, with the bytes written so far out of the file's size.
	Running { sent: u64, total: u64 },
}

/// Every event the app can react to. UI events come from widgets; `Ssh` events
/// are surfaced from the background tokio task via a subscription (§4).
#[derive(Debug, Clone)]
pub enum Message {
	// --- home screen: saved targets (§14) ---
	/// Open a blank connect form for a brand-new connection.
	HomeNewPressed,
	/// A target row was left-clicked — select it (payload: its endpoint key).
	HomeTargetClicked(String),
	/// A target row was right-clicked — select it and open the context menu.
	HomeTargetRightClicked(String),
	/// Dismiss the home context menu without choosing an item.
	HomeMenuDismissed,
	/// Context-menu "Open": pre-fill the form with the selected target and go there.
	HomeMenuOpen,
	/// Context-menu "Rename" (or F2): begin the inline rename of the selected target.
	HomeMenuRename,
	/// Context-menu "Delete" (or the Delete key): ask whether to remove the selected
	/// target — the confirmation, not the removal (§14).
	HomeMenuDelete,
	/// The user confirmed the delete prompt — remove the target from the store.
	HomeDeleteConfirmed,
	/// The user backed out of the delete prompt (Cancel / ✕ / backdrop / Esc) — keep it.
	HomeDeleteCancelled,
	/// The inline rename field changed.
	HomeRenameEdited(String),
	/// The inline rename was submitted (Enter) — commit it and re-sort.
	HomeRenameCommitted,
	/// A key press on the home screen (F2 rename, Enter open, Delete remove, Esc cancel).
	HomeKey(iced::keyboard::Event),
	/// Leave the connect form and return to the home list (the form's Back / Esc).
	HomePressed,
	// --- connect form field edits ---
	HostChanged(String),
	PortChanged(String),
	UserChanged(String),
	PasswordChanged(String),
	/// The optional key-passphrase field on the form changed (§14).
	KeyPassphraseChanged(String),
	// --- auth method selection (§7) ---
	/// The user switched between password and key auth.
	AuthKindChanged(AuthKind),
	/// The user clicked "Browse…" — open the native key-file picker.
	BrowseKeyPressed,
	/// The picker closed: `Some(path)` if a file was chosen, `None` if cancelled.
	KeyFilePicked(Option<PathBuf>),
	// --- form actions ---
	ConnectPressed,
	BackPressed,
	/// A key press on the connect form, used to move focus between inputs with
	/// Tab / Shift+Tab (§10). Wired only on the Connect screen; non-Tab keys are
	/// ignored here and still reach the focused input through the widget tree.
	FormKey(iced::keyboard::Event),
	// --- host-key confirmation (§8) ---
	AcceptHostKey,
	RejectHostKey,
	// --- key passphrase prompt (§7), shown only when the key is encrypted ---
	/// The user edited the passphrase prompt field.
	PassphraseChanged(String),
	/// The user submitted the typed passphrase.
	PassphraseSubmitted,
	/// The user dismissed the prompt — abort the connection.
	PassphraseCancelled,
	// --- terminal input: a raw key press, forwarded only while a shell is open (§9) ---
	Key(iced::keyboard::Event),
	/// The window changed size — refit the terminal grid to it (§9).
	WindowResized(iced::Size),
	/// The user clicked Disconnect in the terminal status bar — ask to confirm (§10).
	DisconnectPressed,
	/// The user confirmed Disconnect in the modal — tear the session down.
	DisconnectConfirmed,
	/// The user cancelled the Disconnect modal — keep the session.
	DisconnectCancelled,
	// --- terminal mouse: text selection + clipboard (§10) ---
	/// The pointer moved over the grid; the payload is its grid-local position.
	GridMoved(iced::Point),
	/// The left button went down on the grid — begin a selection at the hovered cell.
	GridPressed,
	/// The left button came back up — finish the selection (a bare click clears it).
	GridReleased,
	/// The right button went down on the grid — open the context menu at the pointer.
	GridRightPressed,
	/// Copy the current selection to the system clipboard.
	CopyPressed,
	/// Read the system clipboard, then paste it into the shell.
	PastePressed,
	/// The async clipboard read finished: `Some(text)` to paste, `None` if empty.
	Pasted(Option<String>),
	/// Dismiss the open context menu without choosing an item.
	MenuDismissed,
	// --- file upload to the remote (§17) ---
	/// The user clicked the file button in the status bar — open the native picker.
	UploadPickPressed,
	/// The picker closed: `Some(path)` selects that file, `None` (cancelled) keeps the
	/// current selection.
	UploadFilePicked(Option<PathBuf>),
	/// The user clicked Upload — show the destination confirmation.
	UploadPressed,
	/// The destination path field in the confirmation changed.
	UploadDestChanged(String),
	/// The destination was confirmed — start the transfer.
	UploadConfirmed,
	/// The destination already holds a file and the user chose to replace it.
	UploadOverwriteConfirmed,
	/// The user backed out of an upload confirmation (Cancel / ✕ / backdrop / Esc).
	UploadCancelled,
	/// Something happened in the remote folder tree (§18). Nested rather than flattened
	/// — the panel has a dozen interactions of its own, and burying them in this enum
	/// would drown the screens that only have two or three.
	Explorer(ExplorerMessage),
	/// A click that landed on a dialog card itself (not a button, not the backdrop).
	/// It carries no intent — its only job is to be *captured* so the click does not
	/// fall through to the dimming backdrop below and dismiss the dialog (§10).
	Ignored,
	/// A text-selection action inside the open dialog's body message (§10). Applied
	/// read-only — every action but an edit — so the message can be selected and
	/// copied yet never changed.
	DialogAction(text_editor::Action),
	/// The dialog header was pressed — begin dragging the dialog (§10).
	DialogGrabbed,
	/// The pointer moved while dragging a dialog; the payload is its window position.
	DialogDragged(iced::Point),
	/// The drag ended (pointer released) (§10).
	DialogReleased,
	// --- events bubbled up from the SSH task via the subscription (§4) ---
	Ssh(SshEvent),
}

impl App {
	/// Construct the initial state and the first `Task`. iced calls this once at
	/// startup. We start on the Connect screen with no work to do, so the task
	/// is empty.
	fn new() -> (Self, iced::Task<Message>) {
		// Load the saved targets so the home list is populated on the first paint (§14).
		// We start on the home screen, not the form, so there is no field to focus yet.
		// Fetch the window size right away so a dialog opened before the first resize
		// event can still be centred and clamped (§10).
		let app = Self {
			targets: crate::profiles::Targets::load(),
			..Self::default()
		};
		let size = iced::window::latest()
			.and_then(|id| iced::window::size(id).map(Message::WindowResized));
		(app, size)
	}

	/// The heart of the Elm loop: apply one `Message` to the state. Returns a
	/// `Task` for any async follow-up work (none yet in the skeleton).
	fn update(&mut self, message: Message) -> iced::Task<Message> {
		match message {
			// --- home screen (§14) ---
			Message::HomeNewPressed => return self.open_form_new(),
			Message::HomeTargetClicked(key) => {
				self.home_menu_open = false;
				// First click selects (so F2 / rename / delete have a target); clicking
				// the already-selected row again opens it — the "pick pre-fills the form"
				// action, kept distinct from selection so both can coexist (§14).
				if self.home_selected.as_deref() == Some(key.as_str()) {
					return self.open_selected_target();
				}
				self.home_selected = Some(key);
			}
			Message::HomeTargetRightClicked(key) => {
				self.home_selected = Some(key);
				self.home_menu_open = true;
			}
			Message::HomeMenuDismissed => self.home_menu_open = false,
			Message::HomeMenuOpen => return self.open_selected_target(),
			Message::HomeMenuRename => return self.start_rename(),
			Message::HomeMenuDelete => self.ask_delete_selected_target(),
			Message::HomeDeleteConfirmed => self.delete_selected_target(),
			Message::HomeDeleteCancelled => self.confirm_delete = false,
			Message::HomeRenameEdited(value) => {
				if let Some(rename) = self.home_rename.as_mut() {
					rename.text = value;
				}
			}
			Message::HomeRenameCommitted => self.commit_rename(),
			Message::HomeKey(event) => return self.on_home_key(event),
			Message::HomePressed => return self.go_home(),
			// --- connect form field edits ---
			Message::HostChanged(value) => self.form.host = value,
			Message::PortChanged(value) => self.form.port = value,
			Message::UserChanged(value) => self.form.user = value,
			Message::PasswordChanged(value) => self.form.password = value,
			Message::KeyPassphraseChanged(value) => self.form.passphrase = value,
			Message::AuthKindChanged(kind) => self.form.auth_kind = kind,
			// Opening the picker is async work, so it returns a `Task` and we
			// short-circuit the default `Task::none()` below.
			Message::BrowseKeyPressed => return browse_key(),
			// A cancelled picker (`None`) keeps whatever was already chosen.
			Message::KeyFilePicked(path) => {
				if path.is_some() {
					self.form.key_path = path;
				}
			}
			Message::ConnectPressed => self.on_connect_pressed(),
			Message::BackPressed => return self.go_to_form(),
			Message::FormKey(event) => return self.on_form_key(event),
			Message::AcceptHostKey => self.on_host_key_decision(true),
			Message::RejectHostKey => self.on_host_key_decision(false),
			Message::PassphraseChanged(value) => self.passphrase_input = value,
			Message::PassphraseSubmitted => self.on_passphrase_submitted(),
			Message::PassphraseCancelled => return self.on_passphrase_cancelled(),
			Message::Key(event) => self.on_key(event),
			Message::WindowResized(size) => self.on_window_resized(size),
			Message::DisconnectPressed => self.on_disconnect_pressed(),
			Message::DisconnectConfirmed => return self.on_disconnect_confirmed(),
			Message::DisconnectCancelled => self.confirm_disconnect = false,
			Message::GridMoved(point) => self.on_grid_moved(point),
			Message::GridPressed => self.on_grid_pressed(),
			Message::GridReleased => self.on_grid_released(),
			Message::GridRightPressed => self.menu = Some(self.pointer),
			Message::CopyPressed => return self.on_copy(),
			Message::PastePressed => return self.on_paste(),
			Message::Pasted(text) => self.on_pasted(text),
			Message::MenuDismissed => self.menu = None,
			Message::UploadPickPressed => return browse_upload(),
			// A cancelled picker (`None`) keeps whatever was already chosen — same rule
			// as the key-file picker on the form.
			Message::UploadFilePicked(path) => {
				if path.is_some() {
					self.upload_file = path;
					self.upload_notice = None;
				}
			}
			Message::UploadPressed => return self.on_upload_pressed(),
			Message::UploadDestChanged(value) => self.upload_dest = value,
			Message::UploadConfirmed => self.start_upload(false),
			Message::UploadOverwriteConfirmed => self.start_upload(true),
			Message::UploadCancelled => self.upload = None,
			Message::Explorer(message) => return self.on_explorer(message),
			// A click swallowed by a dialog card: nothing to do — capturing it is the
			// whole point (it stops the click reaching the backdrop, §10).
			Message::Ignored => {}
			// Apply a selection/cursor action to the dialog body, but never an edit:
			// that keeps the message read-only while still selectable and copyable (§10).
			Message::DialogAction(action) => {
				if !action.is_edit() {
					self.dialog_body.perform(action);
				}
			}
			Message::DialogGrabbed => {
				self.dialog_dragging = true;
				self.dialog_drag_last = None;
			}
			Message::DialogDragged(pointer) => self.on_dialog_dragged(pointer),
			Message::DialogReleased => {
				self.dialog_dragging = false;
				self.dialog_drag_last = None;
			}
			Message::Ssh(event) => return self.on_ssh_event(event),
		}
		iced::Task::none()
	}

	/// Validate the form, then send a `Connect` command to the SSH task. Cheap
	/// validation fails fast to the error screen.
	fn on_connect_pressed(&mut self) {
		let params = match self.form.validate() {
			Ok(params) => params,
			Err(reason) => {
				self.show_error(&reason);
				return;
			}
		};

		// Fresh attempt: no passphrase has been tried yet, so any upcoming prompt is
		// a first ask (no "incorrect" hint) until the user submits one (§7).
		self.passphrase_failed = false;

		// Capture the profile (no secret) to save if this connect succeeds (§14). The
		// key path is only meaningful for key auth; the name here is a placeholder —
		// `upsert_on_connect` keeps an existing target's custom name.
		let key_path = if self.form.auth_kind == ui::connect::AuthKind::Key {
			self.form.key_path.clone()
		} else {
			None
		};
		self.pending_target = Some(crate::profiles::Target {
			name: crate::profiles::endpoint_of(&params.user, &params.host, params.port),
			host: params.host.clone(),
			port: params.port,
			user: params.user.clone(),
			auth_kind: self.form.auth_kind,
			key_path,
		});

		let status = format!("connecting to {}:{}…", params.host, params.port);
		// The label the terminal status bar will show once the shell is open (§10);
		// capture it now, before `params` moves into the command.
		let endpoint = format!("{}@{}:{}", params.user, params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			self.connection = Some(endpoint);
			self.screen = Screen::Connecting { status };
		} else {
			// The command never left: do not leave a pending target to save later.
			self.pending_target = None;
		}
	}

	/// Relay the user's host-key accept/reject to the SSH task (§8). On accept we
	/// go back to a connecting status; on reject the refused handshake will
	/// surface its own error.
	fn on_host_key_decision(&mut self, accept: bool) {
		if self.send_command(SshCommand::HostKeyResponse(accept)) && accept {
			self.screen = Screen::Connecting {
				status: "authenticating…".to_string(),
			};
		}
	}

	/// Send the typed passphrase to the SSH task (§7) and return to a connecting
	/// status. The text is moved straight into a `Secret` and the input field
	/// cleared, so no plain copy of the passphrase lingers in app state (§12).
	fn on_passphrase_submitted(&mut self) {
		let secret = Secret::new(std::mem::take(&mut self.passphrase_input));
		if self.send_command(SshCommand::Passphrase(secret)) {
			// An attempt is now in flight. If the key does not unlock, the SSH task
			// re-asks and this flag makes the next prompt show its "incorrect" hint (§7).
			self.passphrase_failed = true;
			self.screen = Screen::Connecting {
				status: "authenticating…".to_string(),
			};
		}
	}

	/// Dismiss the passphrase prompt: tell the task to tear down and go back to
	/// the form. Clearing the field first means the discarded text does not linger.
	fn on_passphrase_cancelled(&mut self) -> iced::Task<Message> {
		self.passphrase_input.clear();
		self.send_command(SshCommand::Disconnect);
		self.go_to_form()
	}

	/// Send one command to the SSH task. Returns whether it was sent; a
	/// missing/closed channel becomes a visible error rather than a silent drop.
	/// `try_send` is non-blocking, so it is safe on the synchronous GUI thread.
	fn send_command(&mut self, command: SshCommand) -> bool {
		match &self.command_tx {
			Some(sender) => match sender.try_send(command) {
				Ok(()) => true,
				Err(error) => {
					self.show_error(&format!("Could not reach the SSH worker: {error}"));
					false
				}
			},
			None => {
				self.show_error("SSH worker is not ready yet.");
				false
			}
		}
	}

	/// Load `text` into the dialog body buffer so the dialog about to open shows it as
	/// selectable, copyable content (§10). Called at each dialog-open transition; a
	/// fresh `Content` also resets any selection left from a previous dialog.
	fn set_dialog_body(&mut self, text: &str) {
		self.dialog_body = text_editor::Content::with_text(text);
		// A freshly opened dialog starts centred and not being dragged, so a position
		// left over from a previous dialog never carries across (§10).
		self.dialog_pos = self.centered_dialog_pos();
		self.dialog_dragging = false;
		self.dialog_drag_last = None;
	}

	/// The card's centred top-left for the current window size (§10). Uses the dialog's
	/// fixed width and estimated height; clamped to non-negative so a tiny window keeps
	/// the card at the origin rather than off-screen.
	fn centered_dialog_pos(&self) -> iced::Point {
		iced::Point::new(
			((self.window_size.width - ui::dialog::DIALOG_WIDTH) / 2.0).max(0.0),
			((self.window_size.height - ui::dialog::DIALOG_HEIGHT_ESTIMATE) / 2.0).max(0.0),
		)
	}

	/// Clamp a proposed card top-left so the dialog stays reachable (§10). Horizontal is
	/// exact — the fixed width keeps the card fully between the side edges. Vertical only
	/// keeps the header on screen (`DIALOG_DRAG_MIN_VISIBLE`) rather than the whole card,
	/// because iced does not expose the card's real height; this lets the dialog be
	/// dragged right down to the window's bottom instead of being blocked short of it.
	fn clamp_dialog_pos(&self, pos: iced::Point) -> iced::Point {
		let max_x = (self.window_size.width - ui::dialog::DIALOG_WIDTH).max(0.0);
		let max_y = (self.window_size.height - ui::dialog::DIALOG_DRAG_MIN_VISIBLE).max(0.0);
		iced::Point::new(pos.x.clamp(0.0, max_x), pos.y.clamp(0.0, max_y))
	}

	/// Update the dragged dialog's position from a new pointer location (§10). The first
	/// move of a drag only records the anchor; later moves apply the delta so the card
	/// tracks the pointer without jumping, then clamp it into the window.
	fn on_dialog_dragged(&mut self, pointer: iced::Point) {
		if !self.dialog_dragging {
			return;
		}
		if let Some(last) = self.dialog_drag_last {
			self.dialog_pos = self.clamp_dialog_pos(self.dialog_pos + (pointer - last));
		}
		self.dialog_drag_last = Some(pointer);
	}

	/// Show the error screen with `message`, also seeding it as the dialog's selectable
	/// body so the user can copy the failure text (§10, §12). Central so every error
	/// path (validation, a dead worker channel, a session failure) stays consistent.
	fn show_error(&mut self, message: &str) {
		self.set_dialog_body(message);
		self.screen = Screen::Error;
	}

	/// React to an event from the SSH task. Returns a `Task` for any follow-up
	/// work — most events have none, but a freshly opened shell fetches the window
	/// size to fit its grid right away (§9).
	fn on_ssh_event(&mut self, event: SshEvent) -> iced::Task<Message> {
		match event {
			SshEvent::Ready(sender) => self.command_tx = Some(sender),
			SshEvent::Connecting => {
				self.screen = Screen::Connecting {
					status: "connecting…".to_string(),
				}
			}
			SshEvent::HostKey(fingerprint) => {
				// Seed the selectable body with the explanation plus the fingerprint on
				// its own line, so the whole message — the fingerprint included — can be
				// selected and copied for out-of-band comparison (§8, §10).
				self.set_dialog_body(&format!("{}\n\n{fingerprint}", ui::HOST_KEY_DIALOG_BODY));
				self.screen = Screen::ConfirmHostKey;
			}
			SshEvent::NeedPassphrase => {
				// Start from an empty field each time we ask (including a re-ask
				// after a wrong passphrase), so a stale attempt is never resent.
				self.passphrase_input.clear();
				self.set_dialog_body(ui::PASSPHRASE_DIALOG_BODY);
				self.screen = Screen::NeedPassphrase;
				// Focus the field so the user can type at once — the re-ask path
				// lands here too, refocusing on every prompt (§7).
				return iced::widget::operation::focus(ui::PASSPHRASE_INPUT_ID);
			}
			SshEvent::Connected => {
				// The session is real: persist the target now (§14) — profiles only, no
				// secret. `upsert_on_connect` adds it (or refreshes an existing endpoint,
				// keeping its custom name) and returns its key so we pre-select the row
				// for when the user returns to the home list.
				if let Some(target) = self.pending_target.take() {
					let key = self.targets.upsert_on_connect(
						&target.host,
						target.port,
						&target.user,
						target.auth_kind,
						target.key_path,
					);
					self.home_selected = Some(key);
					if let Err(error) = self.targets.save() {
						eprintln!("could not save targets: {error:#}");
					}
				}
				// A shell is open: spin up an emulator at the pty size we asked for,
				// show the terminal, then immediately refit it to the real window
				// rather than waiting for the first resize event.
				self.terminal = Some(term::Terminal::new(term::DEFAULT_ROWS, term::DEFAULT_COLS));
				self.clear_grid_interaction();
				self.screen = Screen::Terminal;
				// Open the tree at the root so the panel has something in it before the
				// shell has said anything (§18).
				if let Some(path) = self.explorer.expand(explorer::ROOT, false) {
					self.send_command(SshCommand::ListDir(path));
				}
				return fit_terminal();
			}
			SshEvent::Output(bytes) => {
				// Feed raw shell output into the emulator; the next render draws it. The
				// same bytes may carry a cwd announcement, so read the (possibly new)
				// directory out before the borrow ends and let the tree follow it (§18).
				let cwd = match self.terminal.as_mut() {
					Some(terminal) => {
						terminal.process(&bytes);
						terminal.cwd().map(str::to_owned)
					}
					None => None,
				};
				if let Some(cwd) = cwd {
					let needed = self.explorer.reveal_if_new(&cwd);
					self.list_dirs(needed);
				}
			}
			SshEvent::DirListed { path, dirs } => self.explorer.listed(&path, dirs),
			SshEvent::DirFailed { path, reason } => self.explorer.failed(&path, reason),
			SshEvent::RenameDone { from, to } => {
				// The folder moved: re-list its parent so the row reappears under the new
				// name, in the right sort position.
				if let Some(parent) = self.explorer.renamed(&from, &to) {
					self.send_command(SshCommand::ListDir(parent));
				}
			}
			SshEvent::RenameFailed(reason) => self.explorer.set_notice(reason),
			SshEvent::UploadExists(path) => {
				// Nothing has been written yet: the task checked first and stopped. Ask,
				// and only a confirmed answer re-sends with `overwrite` set (§17).
				self.set_dialog_body(&format!("{}\n\n{path}", ui::terminal::UPLOAD_EXISTS_BODY));
				self.upload = Some(UploadState::ConfirmOverwrite);
			}
			// Progress only means something while a transfer is running; a late event
			// after a failure must not revive the bar.
			SshEvent::UploadProgress { sent, total } => {
				if matches!(self.upload, Some(UploadState::Running { .. })) {
					self.upload = Some(UploadState::Running { sent, total });
				}
			}
			SshEvent::UploadDone(path) => {
				// Success deselects the file, which disables the Upload button again —
				// so a stray second click cannot re-send what just landed (§17).
				self.upload = None;
				self.upload_file = None;
				self.upload_notice = Some(format!("Uploaded to {path}"));
			}
			SshEvent::UploadFailed(message) => {
				// The file stays selected so the user can fix the path and retry. The
				// failure shows in the status bar rather than the error screen — that
				// screen would tear down the shell for a file that never left (§17).
				self.upload = None;
				self.upload_notice = Some(message);
			}
			SshEvent::Disconnected => {
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				return self.go_home();
			}
			SshEvent::Error(message) => {
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				self.show_error(&message);
			}
		}
		iced::Task::none()
	}

	/// Refit the terminal grid after the window changed size (§9). Acts only on
	/// the Terminal screen with a live emulator, and only when the cell dimensions
	/// actually change — so dragging the window doesn't spam identical resizes.
	/// Reflows the local view and tells the remote pty to match.
	fn on_window_resized(&mut self, size: iced::Size) {
		// Remember the window size on every screen so a dialog (which can appear before a
		// terminal exists) can be centred and its dragging clamped (§10).
		self.window_size = size;
		// The explorer panel takes its width out of the grid, so the same call serves a
		// window resize and a panel resize (§18).
		let (rows, cols) = ui::terminal::grid_size(size, self.explorer.reserved());
		let changed = match self.terminal.as_mut() {
			Some(terminal) if terminal.screen().size() != (rows, cols) => {
				terminal.resize(rows, cols);
				true
			}
			_ => false,
		};
		if changed {
			self.send_command(SshCommand::Resize { cols, rows });
		}
	}

	/// The Disconnect button (§10): open the confirmation modal instead of dropping
	/// the session immediately, so an accidental click cannot end a live shell. Also
	/// closes any open context menu so only the modal is shown. The teardown happens
	/// in `on_disconnect_confirmed` once the user confirms.
	fn on_disconnect_pressed(&mut self) {
		self.menu = None;
		self.set_dialog_body(ui::terminal::DISCONNECT_DIALOG_BODY);
		self.confirm_disconnect = true;
	}

	/// Confirmed Disconnect (§10): tell the SSH task to tear down, then drop the local
	/// emulator and return to the form right away — the `Disconnected` event that
	/// follows just confirms what we have already done. Mirrors the passphrase-cancel
	/// path, which also acts immediately rather than waiting.
	fn on_disconnect_confirmed(&mut self) -> iced::Task<Message> {
		self.send_command(SshCommand::Disconnect);
		self.terminal = None;
		self.connection = None;
		self.clear_grid_interaction();
		self.go_home()
	}

	/// Return to the connect form: reset the keyboard focus to the first field and
	/// focus it natively, so the form is ready for typing and its highlight ring is
	/// aligned (§10). Used by the paths that keep the user on the form to retry
	/// (error Back, passphrase cancel) — a full return to the list uses `go_home`.
	fn go_to_form(&mut self) -> iced::Task<Message> {
		self.screen = Screen::Connect;
		self.form_focus = ui::connect::FormStop::Host;
		self.apply_form_focus()
	}

	/// Return to the home screen (§14). Closes any open menu / rename, drops a pending
	/// (unsaved) target, and clears the typed secrets out of the form so they do not
	/// linger once we leave it (§12). The saved-target selection is kept so the list
	/// re-opens on the last-used row.
	fn go_home(&mut self) -> iced::Task<Message> {
		self.screen = Screen::Home;
		self.home_menu_open = false;
		self.home_rename = None;
		self.confirm_delete = false;
		self.pending_target = None;
		self.form.password.clear();
		self.form.passphrase.clear();
		iced::Task::none()
	}

	/// Open a blank connect form for a brand-new connection (§14): reset every field,
	/// focus the first, and switch to the form.
	fn open_form_new(&mut self) -> iced::Task<Message> {
		self.home_menu_open = false;
		self.form = ui::connect::ConnectForm::default();
		self.go_to_form()
	}

	/// Open the connect form pre-filled from the selected target (§14): its host / port
	/// / user / auth / key path are copied in; the secret fields start empty so the user
	/// enters them here (never persisted, §12). A stale/missing selection is a no-op.
	fn open_selected_target(&mut self) -> iced::Task<Message> {
		self.home_menu_open = false;
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		let Some(target) = self.targets.find(&key) else {
			return iced::Task::none();
		};
		self.form = ui::connect::ConnectForm {
			host: target.host.clone(),
			port: target.port.to_string(),
			user: target.user.clone(),
			auth_kind: target.auth_kind,
			password: String::new(),
			key_path: target.key_path.clone(),
			passphrase: String::new(),
		};
		self.go_to_form()
	}

	/// Begin an inline rename of the selected target (§14): seed the edit with its
	/// current name and focus the field so the user types straight away. No selection
	/// (or a stale one) is a no-op.
	fn start_rename(&mut self) -> iced::Task<Message> {
		self.home_menu_open = false;
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		let Some(target) = self.targets.find(&key) else {
			return iced::Task::none();
		};
		self.home_rename = Some(ui::home::RenameState {
			key,
			text: target.name.clone(),
		});
		iced::widget::operation::focus(ui::home::RENAME_INPUT_ID)
	}

	/// Commit the in-progress rename (§14): apply it (which re-sorts the list) and save.
	/// A blank name is rejected by the store, so committing one just discards the edit.
	fn commit_rename(&mut self) {
		if let Some(rename) = self.home_rename.take()
			&& self.targets.rename(&rename.key, &rename.text)
			&& let Err(error) = self.targets.save()
		{
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Ask before deleting the selected target (§14). Seeds the dialog body with what
	/// deleting does *and* which target it hits — the list is only a click away from the
	/// wrong row — then opens the confirmation. No selection (or a stale one) is a no-op.
	fn ask_delete_selected_target(&mut self) {
		self.home_menu_open = false;
		let Some(key) = self.home_selected.clone() else {
			return;
		};
		let Some(target) = self.targets.find(&key) else {
			return;
		};
		let body = format!(
			"{}\n\n{}  ({key})",
			ui::home::DELETE_DIALOG_BODY,
			target.name
		);
		self.set_dialog_body(&body);
		self.confirm_delete = true;
	}

	/// Delete the selected target (§14) and save — only reached from a confirmed prompt.
	/// Clears the selection so the menu and the shortcuts no longer point at a gone row.
	fn delete_selected_target(&mut self) {
		self.home_menu_open = false;
		self.confirm_delete = false;
		if let Some(key) = self.home_selected.take()
			&& self.targets.remove(&key)
			&& let Err(error) = self.targets.save()
		{
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Handle a key on the home screen (§14). While the delete prompt is up the list
	/// shortcuts are inert and only Esc is handled (it cancels, keeping the target) — a
	/// stray Enter must not open a connection behind the modal. While renaming, only Esc
	/// (cancel) is handled here — the field's own `on_submit` commits on Enter. Otherwise
	/// F2 renames the selection, Enter opens it, Delete asks to remove it; all are no-ops
	/// without a selection. Other keys fall through.
	fn on_home_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		let iced::keyboard::Event::KeyPressed { key, .. } = event else {
			return iced::Task::none();
		};

		if self.confirm_delete {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.confirm_delete = false;
			}
			return iced::Task::none();
		}

		if self.home_rename.is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.home_rename = None;
			}
			return iced::Task::none();
		}

		match key {
			iced::keyboard::Key::Named(Named::F2) => self.start_rename(),
			iced::keyboard::Key::Named(Named::Enter) => self.open_selected_target(),
			iced::keyboard::Key::Named(Named::Delete) => {
				self.ask_delete_selected_target();
				iced::Task::none()
			}
			_ => iced::Task::none(),
		}
	}

	/// Move native focus to match the current form stop: focus the stop's text input,
	/// or — for a radio/button stop — focus a non-existent id, which unfocuses every
	/// input so no field keeps the caret behind the highlight ring (§10).
	fn apply_form_focus(&self) -> iced::Task<Message> {
		let id = self
			.form_focus
			.input_id(self.form.auth_kind)
			.unwrap_or(ui::connect::NO_FOCUS_ID);
		iced::widget::operation::focus(id)
	}

	/// Handle a key on the connect form (§10): Tab / Shift+Tab move the focus ring
	/// (skipping stops that do not apply to the current auth method, §14), Enter / Space
	/// activate the current stop when it is a radio or button, and Esc returns to the
	/// home list. On a text stop, Enter/Space are left to the focused field
	/// (typing/submit). Anything else is ignored here; the focused input still receives
	/// it through the widget tree.
	fn on_form_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};

		match key {
			iced::keyboard::Key::Named(Named::Tab) => {
				let auth = self.form.auth_kind;
				self.form_focus = if modifiers.shift() {
					self.form_focus.previous(auth)
				} else {
					self.form_focus.next(auth)
				};
				self.apply_form_focus()
			}
			iced::keyboard::Key::Named(Named::Enter | Named::Space) => {
				// A text stop keeps these keys (space types, Enter is the field's own);
				// a radio/button stop turns them into its activation message.
				if self.form_focus.input_id(self.form.auth_kind).is_some() {
					iced::Task::none()
				} else if let Some(message) = self.form_focus.activation(self.form.auth_kind) {
					iced::Task::done(message)
				} else {
					iced::Task::none()
				}
			}
			// Esc backs out of the form to the home list (matches the "← Targets" button).
			iced::keyboard::Key::Named(Named::Escape) => self.go_home(),
			_ => iced::Task::none(),
		}
	}

	/// Forward a key press to the shell, but only while the terminal is open.
	/// Non-input keys (bare modifiers, unmapped keys) encode to nothing and are
	/// dropped. Keyboard events only reach here on the Terminal screen (the
	/// subscription is added only there), so no extra screen check is needed.
	fn on_key(&mut self, event: iced::keyboard::Event) {
		use iced::keyboard::key::Named;

		// While the Disconnect confirmation modal is open, keystrokes belong to the
		// dialog (notably Ctrl+C to copy the selected message text), not the remote
		// shell — the `keyboard::listen` subscription fires independently of widget
		// focus, so without this guard Ctrl+C would also send ETX to the session. The
		// dialog's own widgets still receive the keys through the widget tree (§10).
		if self.confirm_disconnect {
			return;
		}

		let iced::keyboard::Event::KeyPressed {
			key,
			physical_key,
			text,
			modifiers,
			..
		} = event
		else {
			return; // ignore key releases and other keyboard events
		};

		// Same rule for the upload dialogs (§17): while one is open the keyboard belongs
		// to it — the destination field types through the widget tree — so nothing here
		// reaches the shell. Esc backs out of a confirmation; a running transfer has
		// nothing to back out of, so it just swallows the key.
		if let Some(state) = self.upload {
			if matches!(
				state,
				UploadState::ConfirmPath | UploadState::ConfirmOverwrite
			) && matches!(key, iced::keyboard::Key::Named(Named::Escape))
			{
				self.upload = None;
			}
			return;
		}

		// And the same for the folder tree's inline rename (§18): the field types through
		// the widget tree, Esc abandons the edit, and nothing reaches the shell meanwhile
		// — otherwise renaming a folder would also be typing at the remote prompt.
		if self.explorer.editing().is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.explorer.cancel_rename();
			}
			return;
		}

		// Full-screen apps (vim, less, nano) enable DECCKM to get the SS3 arrow-key
		// form; read that mode off the emulator so `encode` sends the sequences the
		// remote program actually listens for. No terminal means no session — treat
		// it as the default (CSI) mode, though this path only runs on the Terminal screen.
		let application_cursor = self
			.terminal
			.as_ref()
			.is_some_and(|terminal| terminal.screen().application_cursor());

		if let Some(bytes) = term::keymap::encode(
			&key,
			physical_key,
			text.as_deref(),
			modifiers,
			application_cursor,
		) {
			self.send_command(SshCommand::Input(bytes));
		}
	}

	/// Track the pointer over the grid (§10): remember its position (so the context
	/// menu can anchor there) and the cell under it, and — while a drag is in
	/// progress — extend the selection's head to that cell.
	fn on_grid_moved(&mut self, point: iced::Point) {
		self.pointer = point;
		let Some(terminal) = self.terminal.as_ref() else {
			return;
		};
		let (rows, cols) = terminal.screen().size();
		self.hover_cell = ui::terminal::cell_at(point, rows, cols);
		if self.selecting
			&& let Some(selection) = self.selection
		{
			self.selection = Some(selection.with_head(self.hover_cell));
		}
	}

	/// Begin a selection at the hovered cell (§10). Also closes any open context
	/// menu — a fresh press on the grid dismisses it.
	fn on_grid_pressed(&mut self) {
		self.menu = None;
		if self.terminal.is_some() {
			self.selection = Some(ui::selection::Selection::new(self.hover_cell));
			self.selecting = true;
		}
	}

	/// Finish a drag (§10). A press-release with no movement leaves an empty
	/// selection (anchor == head), which we clear so a plain click deselects.
	fn on_grid_released(&mut self) {
		self.selecting = false;
		if self.selection.is_some_and(|selection| selection.is_empty()) {
			self.selection = None;
		}
	}

	/// Copy the current selection to the system clipboard (§10). Extracts the
	/// selected cells' text and hands it to iced's async clipboard write. The
	/// highlight is left in place — copying does not deselect. Nothing selected (or
	/// an empty extract) is a no-op.
	fn on_copy(&mut self) -> iced::Task<Message> {
		self.menu = None;
		let (Some(selection), Some(terminal)) = (self.selection, self.terminal.as_ref()) else {
			return iced::Task::none();
		};
		let text = selection.extract(terminal.screen());
		if text.is_empty() {
			return iced::Task::none();
		}
		iced::clipboard::write(text)
	}

	/// Start a paste (§10): read the system clipboard. The read is async, so this
	/// returns a task whose result comes back as `Message::Pasted`.
	fn on_paste(&mut self) -> iced::Task<Message> {
		self.menu = None;
		iced::clipboard::read().map(Message::Pasted)
	}

	/// Send pasted clipboard text to the shell (§9, §10). Wraps it for bracketed
	/// paste when the remote enabled that mode (the encoder also strips any embedded
	/// terminator, the paste-injection guard). An empty clipboard (`None`) sends
	/// nothing. The selection/highlight is deliberately kept — pasting does not clear
	/// it, so the user can still copy what they had selected.
	fn on_pasted(&mut self, text: Option<String>) {
		let (Some(text), Some(terminal)) = (text, self.terminal.as_ref()) else {
			return;
		};
		let bracketed = terminal.screen().bracketed_paste();
		let bytes = term::keymap::encode_paste(&text, bracketed);
		self.send_command(SshCommand::Input(bytes));
	}

	/// Drop all grid-interaction state — the selection, any in-progress drag, an open
	/// context menu, the Disconnect modal, the upload flow, and everything the folder
	/// tree learned. Called whenever a shell opens or closes so nothing (a stale
	/// highlight, a half-finished drag, an open overlay, a file picked for the previous
	/// session, one server's directories) carries across sessions (§10, §17, §18).
	fn clear_grid_interaction(&mut self) {
		self.selection = None;
		self.selecting = false;
		self.menu = None;
		self.confirm_disconnect = false;
		self.upload = None;
		self.upload_file = None;
		self.upload_dest.clear();
		self.upload_notice = None;
		// The panel's own width and visibility are user preferences, not session state,
		// so `reset` deliberately leaves them alone.
		self.explorer.reset();
	}

	/// The Upload button (§17): show the destination before sending anything. The path
	/// is the remote working directory (tracked from the shell's own announcements)
	/// joined with the file's name — and it is editable, so a shell that never announces
	/// its directory still works: the field then holds the bare file name, which the
	/// server resolves against the login directory.
	fn on_upload_pressed(&mut self) -> iced::Task<Message> {
		self.menu = None;
		let Some(local) = self.upload_file.clone() else {
			return iced::Task::none();
		};
		let name = file_name_of(&local).to_owned();
		let cwd = self.terminal.as_ref().and_then(term::Terminal::cwd);
		self.upload_dest = match cwd {
			Some(directory) => term::cwd::join(directory, &name),
			None => name.clone(),
		};

		let size = std::fs::metadata(&local)
			.map(|meta| ui::terminal::human_bytes(meta.len()))
			.unwrap_or_else(|_| "unknown size".to_owned());
		let where_to = match cwd {
			Some(_) => ui::terminal::UPLOAD_DIALOG_BODY,
			None => ui::terminal::UPLOAD_DIALOG_BODY_NO_CWD,
		};
		let body = format!("{where_to}\n\n{}  ({size})", local.display());
		self.set_dialog_body(&body);
		self.upload = Some(UploadState::ConfirmPath);
		// Focus the destination field, so the path can be corrected — or simply
		// confirmed with Enter — without reaching for the mouse.
		iced::widget::operation::focus(ui::terminal::UPLOAD_INPUT_ID)
	}

	/// Send the upload command and switch the status bar over to its progress bar (§17).
	/// `overwrite` is false for the first attempt — the SSH task answers with
	/// `UploadExists` rather than replacing a file — and true only after the user has
	/// confirmed that prompt. An empty destination keeps the dialog open.
	fn start_upload(&mut self, overwrite: bool) {
		let Some(local) = self.upload_file.clone() else {
			self.upload = None;
			return;
		};
		let remote = self.upload_dest.trim().to_owned();
		if remote.is_empty() {
			return;
		}
		let total = std::fs::metadata(&local)
			.map(|meta| meta.len())
			.unwrap_or(0);

		if self.send_command(SshCommand::Upload {
			local,
			remote,
			overwrite,
		}) {
			self.upload_notice = None;
			self.upload = Some(UploadState::Running { sent: 0, total });
		} else {
			self.upload = None;
		}
	}

	/// Handle one event from the remote folder tree (§18). The model decides what the
	/// action means; this only relays the network side of it — the listings it asks for,
	/// the `cd` it types into the shell, the clipboard writes — and refits the grid when
	/// the panel's footprint changes.
	fn on_explorer(&mut self, message: ExplorerMessage) -> iced::Task<Message> {
		match message {
			ExplorerMessage::Toggled => {
				self.explorer.toggle();
				// The panel's width just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new column count.
				self.refit_grid();
			}
			ExplorerMessage::HiddenToggled => self.explorer.toggle_hidden(),
			ExplorerMessage::RowClicked(path) => {
				if let Some(fetch) = self.explorer.toggle_node(&path) {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::RowRightClicked(path) => {
				self.explorer.select(&path);
				self.explorer.open_menu(path);
			}
			ExplorerMessage::PointerMoved(point) => self.explorer.set_pointer(point),
			ExplorerMessage::MenuDismissed => self.explorer.close_menu(),
			ExplorerMessage::Expand(path) => {
				self.explorer.close_menu();
				// Forced, so the menu item doubles as the refresh for a directory that
				// changed under us (a `mkdir` typed in the shell).
				if let Some(fetch) = self.explorer.expand(&path, true) {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::Collapse(path) => {
				self.explorer.close_menu();
				self.explorer.collapse(&path);
			}
			ExplorerMessage::Cd(path) => {
				self.explorer.close_menu();
				// Typed into the shell exactly as the user would type it, quoted so a
				// folder name carrying a quote stays one argument (§18). `ponytail:` a
				// POSIX shell is assumed, and if a full-screen program (vim, less) is
				// running these characters go to that program instead — cmote cannot tell
				// a prompt from an editor. Upgrade path: only offer this between prompts,
				// which the OSC announcements could mark.
				let line = format!("cd {}\r", explorer::shell_quote(&path));
				self.send_command(SshCommand::Input(line.into_bytes()));
			}
			ExplorerMessage::RenameStarted(path) => {
				self.explorer.start_rename(path);
				// The root has no parent, so it declines to be renamed; only focus the
				// field when an edit actually opened.
				if self.explorer.editing().is_some() {
					return iced::widget::operation::focus(ui::explorer::RENAME_INPUT_ID);
				}
			}
			ExplorerMessage::RenameEdited(text) => self.explorer.edit_rename(text),
			ExplorerMessage::RenameCommitted => {
				if let Some((from, to)) = self.explorer.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			ExplorerMessage::CopyName(path) => {
				self.explorer.close_menu();
				return iced::clipboard::write(explorer::name(&path).to_owned());
			}
			ExplorerMessage::CopyRelative(path) => {
				self.explorer.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				return iced::clipboard::write(explorer::relative(cwd, &path));
			}
			ExplorerMessage::CopyPath(path) => {
				self.explorer.close_menu();
				return iced::clipboard::write(path);
			}
			ExplorerMessage::SplitterGrabbed => self.explorer.set_dragging(true),
			ExplorerMessage::SplitterDragged(pointer) => {
				if self.explorer.dragging() {
					// The splitter sits at the panel's left edge and the panel runs to the
					// window's right edge, so the pointer's distance from that edge IS the
					// width — no drag anchor to track.
					let max = self.window_size.width * MAX_PANEL_FRACTION;
					self.explorer
						.set_width(self.window_size.width - pointer.x, max);
					self.refit_grid();
				}
			}
			ExplorerMessage::SplitterReleased => self.explorer.set_dragging(false),
		}
		iced::Task::none()
	}

	/// Reflow the terminal to the current window *and* panel footprint (§18). The panel
	/// takes its width out of the grid, so showing, hiding or resizing it changes the
	/// column count exactly as a window resize would — and goes through the same path.
	fn refit_grid(&mut self) {
		self.on_window_resized(self.window_size);
	}

	/// Ask the SSH task for each folder listing the tree still needs (§18). Stops at the
	/// first send failure, which has already surfaced its own error.
	fn list_dirs(&mut self, paths: Vec<String>) {
		for path in paths {
			if !self.send_command(SshCommand::ListDir(path)) {
				return;
			}
		}
	}

	/// The window title (§17). Off-session it is just the app name; with a shell open it
	/// carries the session and — as soon as the shell announces one — the remote working
	/// directory, so the directory is visible without stealing room from the grid.
	fn title(&self) -> String {
		let connected = matches!(self.screen, Screen::Terminal);
		match (connected, self.connection.as_deref()) {
			(true, Some(endpoint)) => match self.terminal.as_ref().and_then(term::Terminal::cwd) {
				Some(cwd) => format!("cmote — {endpoint} — {cwd}"),
				None => format!("cmote — {endpoint}"),
			},
			_ => "cmote".to_owned(),
		}
	}

	/// Render the current screen. Pure: it only reads state and returns widgets.
	fn view(&self) -> Element<'_, Message> {
		// Position/drag state shared by every dialog (§10); only the dialog arms use it.
		let drag = ui::dialog::Drag {
			pos: self.dialog_pos,
			dragging: self.dialog_dragging,
		};
		match &self.screen {
			Screen::Home => ui::home::view(
				self.targets.items(),
				self.home_selected.as_deref(),
				self.home_rename.as_ref(),
				self.home_menu_open,
				self.confirm_delete,
				&self.dialog_body,
				drag,
			),
			Screen::Connect => ui::connect::view(&self.form, self.form_focus),
			Screen::Connecting { status } => text(status).into(),
			// The connect-flow dialogs float over the (dimmed) form rather than replacing
			// it, so the page stays in view behind them (§10). A click on the backdrop
			// dismisses with the dialog's own safe action (reject / cancel / back).
			Screen::ConfirmHostKey => self.form_with_dialog(
				ui::host_key_view(&self.dialog_body, drag),
				Message::RejectHostKey,
			),
			Screen::NeedPassphrase => self.form_with_dialog(
				ui::passphrase_view(
					&self.passphrase_input,
					self.passphrase_failed,
					&self.dialog_body,
					drag,
				),
				Message::PassphraseCancelled,
			),
			Screen::Terminal => match &self.terminal {
				Some(terminal) => ui::terminal::view(
					terminal,
					self.connection.as_deref().unwrap_or(""),
					self.selection.as_ref(),
					self.menu,
					ui::terminal::Modals {
						confirm_disconnect: self.confirm_disconnect,
						body: &self.dialog_body,
						drag,
					},
					ui::terminal::UploadView {
						file: self.upload_file.as_deref().map(file_name_of),
						dest: &self.upload_dest,
						state: self.upload,
						notice: self.upload_notice.as_deref(),
					},
					&self.explorer,
				),
				None => text("terminal starting…").into(),
			},
			Screen::Error => self.form_with_dialog(
				ui::error_view(&self.dialog_body, drag),
				Message::BackPressed,
			),
		}
	}

	/// Overlay a connect-flow dialog on the (dimmed) connect form (§10): the form as the
	/// base, a dimming backdrop that dismisses with `on_dismiss` on a click-away, then the
	/// dialog card on top. The form stays visible behind the dialog rather than being
	/// replaced, so the prompt reads as a modal over the page.
	fn form_with_dialog<'a>(
		&'a self,
		dialog: Element<'a, Message>,
		on_dismiss: Message,
	) -> Element<'a, Message> {
		iced::widget::stack![
			ui::connect::view(&self.form, self.form_focus),
			ui::dialog::backdrop(on_dismiss),
			dialog,
		]
		.width(iced::Length::Fill)
		.height(iced::Length::Fill)
		.into()
	}

	/// Streams the app listens to. The SSH worker's outbound events (§4) are
	/// always mapped into `Message::Ssh(..)`. While a shell is open we also listen
	/// for key presses and window resizes (§9) — turned into `Message::Key(..)` and
	/// `Message::WindowResized(..)`; limiting those to the Terminal screen means the
	/// connect form's text inputs keep the keyboard to themselves and the form does
	/// not react to resizes it does not care about.
	fn subscription(&self) -> iced::Subscription<Message> {
		let ssh = bridge::subscription().map(Message::Ssh);
		// Track window size on every screen so a dialog can be centred/clamped even
		// before a terminal exists (§10).
		let resizes = iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size));
		match self.screen {
			Screen::Terminal => iced::Subscription::batch([
				ssh,
				resizes,
				iced::keyboard::listen().map(Message::Key),
			]),
			// On the connect form, listen for key presses so Tab / Shift+Tab can move
			// focus between the inputs (`on_form_key`); typing still reaches the fields
			// through the widget tree, so this only adds the focus shortcuts.
			Screen::Connect => iced::Subscription::batch([
				ssh,
				resizes,
				iced::keyboard::listen().map(Message::FormKey),
			]),
			// On the home screen, listen for the F2 / Enter / Delete / Esc shortcuts (§14);
			// the rename field still receives its own typing through the widget tree.
			Screen::Home => iced::Subscription::batch([
				ssh,
				resizes,
				iced::keyboard::listen().map(Message::HomeKey),
			]),
			_ => iced::Subscription::batch([ssh, resizes]),
		}
	}
}

/// Fetch the current window size and turn it into a `WindowResized`, so a newly
/// opened terminal fits the window immediately instead of waiting for the first
/// resize event (§9). `latest()` yields the most-recently-opened window and
/// `and_then` unwraps it — if there is somehow no window, this is a no-op.
fn fit_terminal() -> iced::Task<Message> {
	iced::window::latest().and_then(|id| iced::window::size(id).map(Message::WindowResized))
}

/// Open the native file picker for a private-key file (§7). The dialog is modal
/// and would block the GUI thread, so it runs as an async `Task` instead; its
/// result arrives back through the Elm loop as `Message::KeyFilePicked`. We keep
/// only the path — the `FileHandle` itself is not needed past selection.
fn browse_key() -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select a private key")
			.pick_file(),
		|handle| Message::KeyFilePicked(handle.map(|handle| handle.path().to_path_buf())),
	)
}

/// Open the native file picker for a file to upload (§17). Same async-`Task` shape as
/// `browse_key` — the dialog is modal and would otherwise block the GUI thread.
fn browse_upload() -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select a file to upload")
			.pick_file(),
		|handle| Message::UploadFilePicked(handle.map(|handle| handle.path().to_path_buf())),
	)
}

/// A path's own file name, which is what the status bar shows and what the remote
/// destination is built from (§17). A path with no final component (a bare root) falls
/// back to a placeholder rather than an empty label.
fn file_name_of(path: &std::path::Path) -> &str {
	path.file_name()
		.and_then(std::ffi::OsStr::to_str)
		.unwrap_or("file")
}

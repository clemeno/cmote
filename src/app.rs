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

/// Build and start the iced runtime. Called from `main`.
pub fn run() -> iced::Result {
	// The functional builder (iced 0.14): the first argument is the "boot"
	// function that produces the initial `(State, Task)` — here `App::new`. Then
	// the update and view functions. `.title` / `.subscription` are builder
	// methods, and `.run()` starts the event loop.
	iced::application(App::new, App::update, App::view)
		.title("cmote")
		.font(MONO_FONT)
		.font(MONO_FONT_BOLD)
		.subscription(App::subscription)
		.run()
}

/// Which screen the single window is currently showing. This is the small state
/// machine from PLAN §10 — every transition happens in `update`.
#[derive(Debug, Default)]
pub enum Screen {
	/// The connection form (host / port / user / auth). This is where we start.
	#[default]
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
	/// The connect form's field contents. Lives here so it survives navigating
	/// to an error screen and back without losing what the user typed.
	pub form: ui::connect::ConnectForm,
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
	/// buffer serves all four (disconnect, host-key, passphrase, error).
	dialog_body: text_editor::Content,
}

/// Every event the app can react to. UI events come from widgets; `Ssh` events
/// are surfaced from the background tokio task via a subscription (§4).
#[derive(Debug, Clone)]
pub enum Message {
	// --- connect form field edits ---
	HostChanged(String),
	PortChanged(String),
	UserChanged(String),
	PasswordChanged(String),
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
	/// A click that landed on a dialog card itself (not a button, not the backdrop).
	/// It carries no intent — its only job is to be *captured* so the click does not
	/// fall through to the dimming backdrop below and dismiss the dialog (§10).
	Ignored,
	/// A text-selection action inside the open dialog's body message (§10). Applied
	/// read-only — every action but an edit — so the message can be selected and
	/// copied yet never changed.
	DialogAction(text_editor::Action),
	// --- events bubbled up from the SSH task via the subscription (§4) ---
	Ssh(SshEvent),
}

impl App {
	/// Construct the initial state and the first `Task`. iced calls this once at
	/// startup. We start on the Connect screen with no work to do, so the task
	/// is empty.
	fn new() -> (Self, iced::Task<Message>) {
		(Self::default(), iced::Task::none())
	}

	/// The heart of the Elm loop: apply one `Message` to the state. Returns a
	/// `Task` for any async follow-up work (none yet in the skeleton).
	fn update(&mut self, message: Message) -> iced::Task<Message> {
		match message {
			Message::HostChanged(value) => self.form.host = value,
			Message::PortChanged(value) => self.form.port = value,
			Message::UserChanged(value) => self.form.user = value,
			Message::PasswordChanged(value) => self.form.password = value,
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
			Message::BackPressed => self.screen = Screen::Connect,
			Message::FormKey(event) => return Self::on_form_key(&event),
			Message::AcceptHostKey => self.on_host_key_decision(true),
			Message::RejectHostKey => self.on_host_key_decision(false),
			Message::PassphraseChanged(value) => self.passphrase_input = value,
			Message::PassphraseSubmitted => self.on_passphrase_submitted(),
			Message::PassphraseCancelled => self.on_passphrase_cancelled(),
			Message::Key(event) => self.on_key(event),
			Message::WindowResized(size) => self.on_window_resized(size),
			Message::DisconnectPressed => self.on_disconnect_pressed(),
			Message::DisconnectConfirmed => self.on_disconnect_confirmed(),
			Message::DisconnectCancelled => self.confirm_disconnect = false,
			Message::GridMoved(point) => self.on_grid_moved(point),
			Message::GridPressed => self.on_grid_pressed(),
			Message::GridReleased => self.on_grid_released(),
			Message::GridRightPressed => self.menu = Some(self.pointer),
			Message::CopyPressed => return self.on_copy(),
			Message::PastePressed => return self.on_paste(),
			Message::Pasted(text) => self.on_pasted(text),
			Message::MenuDismissed => self.menu = None,
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

		let status = format!("connecting to {}:{}…", params.host, params.port);
		// The label the terminal status bar will show once the shell is open (§10);
		// capture it now, before `params` moves into the command.
		let endpoint = format!("{}@{}:{}", params.user, params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			self.connection = Some(endpoint);
			self.screen = Screen::Connecting { status };
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
	fn on_passphrase_cancelled(&mut self) {
		self.passphrase_input.clear();
		self.send_command(SshCommand::Disconnect);
		self.screen = Screen::Connect;
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
				// A shell is open: spin up an emulator at the pty size we asked for,
				// show the terminal, then immediately refit it to the real window
				// rather than waiting for the first resize event.
				self.terminal = Some(term::Terminal::new(term::DEFAULT_ROWS, term::DEFAULT_COLS));
				self.clear_grid_interaction();
				self.screen = Screen::Terminal;
				return fit_terminal();
			}
			SshEvent::Output(bytes) => {
				// Feed raw shell output into the emulator; the next render draws it.
				if let Some(terminal) = self.terminal.as_mut() {
					terminal.process(&bytes);
				}
			}
			SshEvent::Disconnected => {
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				self.screen = Screen::Connect;
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
		let (rows, cols) = ui::terminal::grid_size(size);
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
	fn on_disconnect_confirmed(&mut self) {
		self.send_command(SshCommand::Disconnect);
		self.terminal = None;
		self.connection = None;
		self.clear_grid_interaction();
		self.screen = Screen::Connect;
	}

	/// Move focus between the connect form's inputs (§10): Tab to the next focusable
	/// widget, Shift+Tab to the previous. Any other key is left alone — the focused
	/// input receives it through the widget tree. Static: it reads no state, only turns
	/// a Tab press into a focus operation.
	fn on_form_key(event: &iced::keyboard::Event) -> iced::Task<Message> {
		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};
		if let iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab) = key {
			return if modifiers.shift() {
				iced::widget::operation::focus_previous()
			} else {
				iced::widget::operation::focus_next()
			};
		}
		iced::Task::none()
	}

	/// Forward a key press to the shell, but only while the terminal is open.
	/// Non-input keys (bare modifiers, unmapped keys) encode to nothing and are
	/// dropped. Keyboard events only reach here on the Terminal screen (the
	/// subscription is added only there), so no extra screen check is needed.
	fn on_key(&mut self, event: iced::keyboard::Event) {
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
			text,
			modifiers,
			..
		} = event
		else {
			return; // ignore key releases and other keyboard events
		};

		// Full-screen apps (vim, less, nano) enable DECCKM to get the SS3 arrow-key
		// form; read that mode off the emulator so `encode` sends the sequences the
		// remote program actually listens for. No terminal means no session — treat
		// it as the default (CSI) mode, though this path only runs on the Terminal screen.
		let application_cursor = self
			.terminal
			.as_ref()
			.is_some_and(|terminal| terminal.screen().application_cursor());

		if let Some(bytes) =
			term::keymap::encode(&key, text.as_deref(), modifiers, application_cursor)
		{
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
	/// context menu, and the Disconnect modal. Called whenever a shell opens or closes
	/// so nothing (a stale highlight, a half-finished drag, an open overlay) carries
	/// across sessions (§10).
	fn clear_grid_interaction(&mut self) {
		self.selection = None;
		self.selecting = false;
		self.menu = None;
		self.confirm_disconnect = false;
	}

	/// Render the current screen. Pure: it only reads state and returns widgets.
	fn view(&self) -> Element<'_, Message> {
		match &self.screen {
			Screen::Connect => ui::connect::view(&self.form),
			Screen::Connecting { status } => text(status).into(),
			Screen::ConfirmHostKey => ui::host_key_view(&self.dialog_body),
			Screen::NeedPassphrase => ui::passphrase_view(
				&self.passphrase_input,
				self.passphrase_failed,
				&self.dialog_body,
			),
			Screen::Terminal => match &self.terminal {
				Some(terminal) => ui::terminal::view(
					terminal,
					self.connection.as_deref().unwrap_or(""),
					self.selection.as_ref(),
					self.menu,
					self.confirm_disconnect,
					&self.dialog_body,
				),
				None => text("terminal starting…").into(),
			},
			Screen::Error => ui::error_view(&self.dialog_body),
		}
	}

	/// Streams the app listens to. The SSH worker's outbound events (§4) are
	/// always mapped into `Message::Ssh(..)`. While a shell is open we also listen
	/// for key presses and window resizes (§9) — turned into `Message::Key(..)` and
	/// `Message::WindowResized(..)`; limiting those to the Terminal screen means the
	/// connect form's text inputs keep the keyboard to themselves and the form does
	/// not react to resizes it does not care about.
	fn subscription(&self) -> iced::Subscription<Message> {
		let ssh = bridge::subscription().map(Message::Ssh);
		match self.screen {
			Screen::Terminal => iced::Subscription::batch([
				ssh,
				iced::keyboard::listen().map(Message::Key),
				iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size)),
			]),
			// On the connect form, listen for key presses so Tab / Shift+Tab can move
			// focus between the inputs (`on_form_key`); typing still reaches the fields
			// through the widget tree, so this only adds the focus shortcuts.
			Screen::Connect => {
				iced::Subscription::batch([ssh, iced::keyboard::listen().map(Message::FormKey)])
			}
			_ => ssh,
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

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

use std::path::{Path, PathBuf};

use iced::Element;
use iced::widget::{text, text_editor};
use tokio::sync::mpsc;

use crate::bridge::{self, SshCommand, SshEvent};
use crate::explorer::{self, ExplorerMessage};
use crate::files::{self, FilesMessage};
use crate::link;
use crate::secret::Secret;
use crate::term;
use crate::ui;
use crate::ui::connect::AuthKind;

/// Fira Mono, the terminal's monospace family (OFL 1.1 — see assets/FiraMono-LICENSE.txt),
/// bundled in every weight Mozilla ships so the terminal looks identical on every machine and
/// the grid has a known cell advance the resize math relies on (§9). All three weights share
/// the exact 600/1000-em advance, so which one a cell uses never disturbs the fixed metric.
/// They share the family name "Fira Mono"; `ui::grid` picks a weight by name, drawing normal
/// cells in Regular (400) and bold cells in Bold (700). Medium (500) is bundled for family
/// completeness — Fira Mono ships it, so it is here to be resolved to if ever asked for.
/// Registered with iced in `run`.
const MONO_FONT_REGULAR: &[u8] = include_bytes!("../assets/FiraMono-Regular.ttf");
const MONO_FONT_MEDIUM: &[u8] = include_bytes!("../assets/FiraMono-Medium.ttf");
const MONO_FONT_BOLD: &[u8] = include_bytes!("../assets/FiraMono-Bold.ttf");

/// The italic faces Fira Mono lacks — it ships no italic at all — supplied by IBM Plex Mono
/// (OFL 1.1 — see assets/IBMPlexMono-LICENSE.txt), the closest humanist monospace whose advance
/// is the same 600/1000 em, so an italic cell keeps the grid's pixel↔cell contract exactly
/// (§9, §23). Only italic (and bold-italic) cells use this family; upright and bold stay Fira
/// Mono. `ui::grid` asks for the family "IBM Plex Mono" with `Style::Italic` at weight 400 or
/// 700, which resolve to these two faces.
const ITALIC_FONT: &[u8] = include_bytes!("../assets/IBMPlexMono-Italic.ttf");
const ITALIC_FONT_BOLD: &[u8] = include_bytes!("../assets/IBMPlexMono-BoldItalic.ttf");

/// The icon face the files pane draws with (Material Icons, Apache-2.0 — see
/// assets/MaterialIcons-LICENSE.txt). Bundled for the same reason the monospace face is:
/// a folder glyph that is there on every machine. It is only ever asked for by name
/// (`ui::files::ICON_FONT`), so it never touches the terminal grid's metrics (§19).
const ICON_FONT: &[u8] = include_bytes!("../assets/MaterialIcons-Regular.ttf");

/// The terminal size the main window opens sized for (§10, §11): wide enough for a
/// 180-column grid, with a comfortable default height. `run` converts this to a window
/// size via `ui::terminal::window_size` so it tracks the grid metrics.
const INITIAL_COLS: u16 = 180;
const INITIAL_ROWS: u16 = 40;

/// The most of the window the explorer panel — and, on the other axis, the files pane —
/// may be dragged to (§18, §19). A splitter with no ceiling can push the terminal grid
/// down to a single cell, which is a state the user then has to drag their way back out
/// of.
const MAX_PANEL_FRACTION: f32 = 0.6;

/// How long a copy-confirmation toast stays before it clears itself (§10). Long enough to
/// register, short enough not to linger over the shell.
const SNACKBAR_DWELL: std::time::Duration = std::time::Duration::from_secs(3);

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
		.font(MONO_FONT_REGULAR)
		.font(MONO_FONT_MEDIUM)
		.font(MONO_FONT_BOLD)
		.font(ITALIC_FONT)
		.font(ITALIC_FONT_BOLD)
		.font(ICON_FONT)
		// Open wide enough for a 180-column terminal *and* the explorer panel beside it
		// (the size is derived from the grid metrics so it stays in step with
		// `grid_size`, §18).
		.window(iced::window::Settings {
			size: ui::terminal::window_size(
				INITIAL_COLS,
				INITIAL_ROWS,
				explorer::DEFAULT_WIDTH + explorer::SPLITTER_WIDTH,
				files::DEFAULT_HEIGHT + files::SPLITTER_HEIGHT,
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
	/// The server posed a keyboard-interactive challenge (§7): 2FA / OTP or any
	/// challenge-response scheme. The request's fields live in `App::interactive_prompts` and
	/// the user's in-progress answers in `App::interactive_answers`; submitting sends them back
	/// and the server drives what comes next — another prompt, success, or a generic failure.
	Interactive,
	/// The master-passphrase prompt for the portable secret vault (§16), shown over the
	/// connect form: CREATE it (first time, typed twice) or UNLOCK it. The typed values live
	/// in `App::vault_input` / `vault_confirm`; on success the pending action (`vault_pending`)
	/// — a deferred connect, or a form pre-fill — resumes.
	VaultUnlock,
	/// A live shell: the vt100 grid fills the window.
	Terminal,
	/// A terminal failure. The generic, non-leaking message (§12) lives in
	/// `App::dialog_body` so it can be selected and copied; this variant just marks
	/// that the error screen is showing.
	Error,
}

/// Which part of the terminal screen the keyboard is talking to (§20).
///
/// The shell is not the only thing on this screen any more: two panels sit beside it, and
/// both want the arrow keys. Rather than guess from the pointer, the window has one focus
/// at a time — the terminal to begin with, a click moves it to whatever was clicked, and
/// Ctrl+Tab cycles. While a panel holds it, no key reaches the shell: a panel that
/// swallowed only the arrows would still leave Tab completing paths at a prompt the user
/// is not looking at.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Focus {
	/// The remote shell: every key is encoded and sent down the channel (§9).
	#[default]
	Terminal,
	/// The folder tree (§18).
	Tree,
	/// The files pane (§19).
	Files,
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
	/// The current keyboard-interactive request's fields (§7), one per prompt with its echo
	/// hint. Empty unless the Interactive screen is showing; set from `SshEvent::Interactive`
	/// and cleared once the prompt is answered or cancelled.
	interactive_prompts: Vec<bridge::InteractivePrompt>,
	/// The user's in-progress answers to `interactive_prompts` (§7), one `String` per prompt in
	/// the same order. Moved into `Secret`s on submit and then cleared, so no plain copy of an
	/// OTP or password lingers in app state (§12).
	interactive_answers: Vec<String>,
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
	/// The local files picked for the current upload batch (§17), empty when none is
	/// pending — which is also what disables the status bar's Upload button. Cleared once
	/// the batch drains, so the same files are never re-sent by a stray click. One file or
	/// many: the flow is the same, and the confirmation lists them.
	upload_files: Vec<PathBuf>,
	/// Where the file transfer in progress has got to (§17, §19): confirming the path or
	/// moving bytes. `None` when nothing is being transferred. One state for both directions
	/// — only one transfer runs at a time, and an upload's progress bar and a download's read
	/// the same.
	transfer: Option<TransferState>,
	/// The destination FOLDER the batch goes into (§17), editable in the confirmation.
	/// Seeded from wherever the upload was started — the shell's cwd, the files pane's
	/// directory, or a folder right-clicked in the tree — and normalised to `.` (the login
	/// directory) when left empty. Each file keeps its own name inside it.
	upload_dir: String,
	/// The batch waiting to send, a (local file, remote path) pair each (§17). One transfer
	/// runs at a time, so the files queue here and every `UploadDone` starts the next — the
	/// mirror of the download queue (§21).
	uploads: std::collections::VecDeque<(PathBuf, String)>,
	/// How many of the batch have landed, for its closing notice (§17).
	uploaded: usize,
	/// Whether the batch sends with overwrite set — true only when the user answered the
	/// collision question with "replace" (§17). Decided once, applied to every file; a
	/// free or "keep both" destination is written with it off and its own name.
	upload_overwrite: bool,
	/// A batch held at the "some are already there" question (§17): the clashing names the
	/// server found, each paired with a free `name-1` path for the "keep both" answer.
	/// `Some` while the question is open, which is what draws the dialog; `None` otherwise.
	upload_clash: Option<Vec<(String, String)>>,
	/// The last transfer outcome, shown in the status bar until the next one starts
	/// (§17, §19). `ponytail:` no timed fade — that would need a timer subscription for a
	/// line of text.
	transfer_notice: Option<String>,
	/// The remote folder tree shown beside the grid (§18). It owns its own visibility,
	/// width, expansion state and selection; `app` only relays its events and turns the
	/// paths it asks for into `SshCommand::ListDir`.
	explorer: explorer::Explorer,
	/// The remote file grid shown under the grid and the tree (§19). Same division of
	/// labour: it owns what it shows, `app` turns its requests into `SshCommand::ListFiles`
	/// / `Download` and follows the shell's directory into it.
	files: files::Files,
	/// Which of the three — shell, tree, files pane — the keyboard belongs to (§20).
	focus: Focus,
	/// Whether the OS window currently has focus (§23). Half of what "the shell is focused"
	/// means for focus reporting — the other half is `focus == Focus::Terminal`. Started `true`
	/// by `new` (a window opens focused); the first `Unfocused` event corrects it if not.
	window_focused: bool,
	/// The last shell-focus state cmote told the remote, for focus reporting (§23). Only a
	/// change from this reaches the wire, so a steady state is never re-sent and a program that
	/// enables `?1004` hears nothing until focus actually moves. Started `true` — the state a
	/// program assumes on enabling the mode — and re-baselined to `true` at each session start.
	shell_focus_reported: bool,
	/// Which modifier keys are down right now (§21). Tracked from the keyboard
	/// subscription because a mouse press reports none of its own, and Ctrl+click,
	/// Shift+click and Ctrl+drag all need to know.
	modifiers: iced::keyboard::Modifiers,
	/// Downloads waiting their turn (§21) — remote path and where it is being saved. One
	/// transfer runs at a time, so a multi-file download queues here and each completion
	/// starts the next.
	downloads: std::collections::VecDeque<(String, PathBuf)>,
	/// How many of the current batch have landed, for its closing notice.
	downloaded: usize,
	/// A multi-file download held at the "some of these are already there" question (§21).
	clash: Option<Clash>,
	/// The "new folder" dialog's target and typed name (§18), `Some` while it is open. The
	/// parent is where the folder will be made — a tree folder or the pane's directory — and
	/// `name` is what the user is typing; `None` the rest of the time, which hides the dialog.
	new_folder: Option<NewFolder>,
	/// The remote entries a delete confirmation is holding (§18): the paths that will be removed
	/// once the user confirms. `Some` while the confirmation is up, `None` otherwise — deleting is
	/// not undoable, so nothing is sent until this is confirmed.
	pending_delete: Option<Vec<String>>,
	/// The file a recursive transfer is currently asking about (§17, §19): its name, shown in the
	/// six-way conflict dialog. `Some` parks the transfer behind the prompt; answering clears it
	/// and sends the choice back down the wire.
	transfer_conflict: Option<String>,
	/// The copy-confirmation toast currently showing, if any (§10). Set on every clipboard
	/// write and cleared once its dwell elapses; `None` the rest of the time. The timestamp
	/// inside it is the dwell clock — see `Snackbar`.
	snackbar: Option<Snackbar>,
	/// The shell cwd a reconnect is waiting to settle at (§22), or `None` when not resuming.
	/// Set on connect when a remembered terminal path is replayed as a `cd`: until the shell
	/// announces this exact directory, the files pane is pinned to its own remembered path so
	/// the login-then-`cd` announcements do not drag it off. Cleared the moment the shell
	/// reaches it, or when the user moves the shell themselves.
	resume_cwd: Option<String>,
	/// The unlocked secret vault for this session (§16), or `None` until the user unlocks it.
	/// Held so repeated stores/reads need no re-prompt; dropped when the app exits, wiping the
	/// decrypted secrets it carries. Lazy: a user who never opts in never has one.
	vault: Option<crate::vault::Vault>,
	/// The master passphrase being typed in the vault prompt, and its confirm field (create
	/// mode). Kept out of the vault itself so a cancelled prompt leaves nothing behind; cleared
	/// on submit or cancel (§16, §12).
	vault_input: String,
	vault_confirm: String,
	/// Whether the vault prompt is CREATING a passphrase (no vault file yet, two fields) rather
	/// than unlocking an existing one (a single field). Fixed when the prompt opens.
	vault_creating: bool,
	/// Whether the vault prompt should show its "wrong / do not match" hint — set on a failed
	/// unlock or a mismatched create, cleared when the prompt reopens (§16).
	vault_failed: bool,
	/// What a successful vault unlock should resume (§16): a deferred connect, or a form
	/// pre-fill. `None` when no vault prompt is pending.
	vault_pending: Option<VaultPending>,
	/// The secret captured at dial time to store once the connect succeeds (§16), with its
	/// endpoint. Set only when "Remember" is on and the secret is non-empty; taken and written
	/// on `Connected`, cleared if the connect never leaves. Persisting only on success means a
	/// wrong password is never saved.
	pending_remember: Option<(String, Secret)>,
}

/// What a successful vault unlock should resume (§16). The master-passphrase prompt can
/// interrupt two flows, so it records which to return to once the vault is open: continuing a
/// connection set to remember its secret, or pre-filling the form from a secret already stored
/// for a target the user opened.
#[derive(Debug)]
enum VaultPending {
	/// Continue dialing this connection; its secret is stored on a successful connect.
	Connect(bridge::ConnectParams),
	/// Pre-fill the connect form's masked field from the stored secret for this endpoint.
	Prefill(String),
}

/// A copy-confirmation toast (§10): the message it shows and when it appeared. The
/// timestamp is the whole timer — `update` compares its age against `SNACKBAR_DWELL` on
/// each frame tick and clears the toast once it is older, so a fresh copy that overwrites
/// this always gets its full dwell rather than inheriting the previous one's remaining time.
#[derive(Debug, Clone)]
struct Snackbar {
	message: String,
	shown_at: std::time::Instant,
}

/// A queued batch of downloads waiting on the name-collision answer (§21). The names that
/// collide are not kept: the answer is applied by looking again, so a folder that changed
/// while the dialog was open is still handled correctly.
#[derive(Debug, Clone)]
struct Clash {
	remotes: Vec<String>,
	dir: PathBuf,
}

/// The in-progress "new folder" dialog (§18): where the folder will be made, and the name typed
/// so far. A small owned struct, like the home screen's rename, because it is the same shape of
/// interaction — a name being entered against a fixed target.
#[derive(Debug, Clone)]
struct NewFolder {
	parent: String,
	name: String,
}

/// What to do about local files a multi-file download would land on top of (§21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClashChoice {
	/// Leave the local copies alone and download only the rest.
	Skip,
	/// Overwrite them.
	Replace,
	/// Save alongside, as `name-1.ext`.
	KeepBoth,
	/// Download nothing at all.
	Cancel,
}

/// Where a file transfer has got to (§17, §19). Only one runs at a time, so this is a
/// plain state, not a queue. `ConfirmPath` is upload-only: a download's destination comes
/// from the native save dialog, which asks its own overwrite question, and an upload's
/// overwrite question is settled up front by the batch pre-scan (§17), not by a per-file state.
#[derive(Debug, Clone, Copy)]
pub enum TransferState {
	/// Showing the destination folder for confirmation, before anything is sent.
	ConfirmPath,
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
	// --- keyboard-interactive prompt (§7): 2FA / OTP and challenge-response ---
	/// A keyboard-interactive answer field changed: which prompt (index) and its new text.
	InteractiveAnswerChanged(usize, String),
	/// The keyboard-interactive prompt was submitted (Submit button, or Enter in a field).
	InteractiveSubmitted,
	/// The keyboard-interactive prompt was dismissed — abort the connection.
	InteractiveCancelled,
	// --- remembered secrets: the "Remember" tick + the master-passphrase vault (§16) ---
	/// The connect form's "Remember" checkbox was toggled (mouse click or Enter/Space on the
	/// Remember stop). Carries no state — `update` flips the flag.
	RememberToggled,
	/// The vault prompt's master-passphrase field changed.
	VaultInputChanged(String),
	/// The vault prompt's confirm field changed (create mode only).
	VaultConfirmChanged(String),
	/// The vault prompt was submitted (the Unlock / Create button, or Enter in a field).
	VaultSubmitted,
	/// The vault prompt was dismissed (Cancel / ✕ / backdrop).
	VaultCancelled,
	// --- terminal input: a raw key press, forwarded only while a shell is open (§9) ---
	Key(iced::keyboard::Event),
	/// The window changed size — refit the terminal grid to it (§9).
	WindowResized(iced::Size),
	/// The OS window gained (`true`) or lost (`false`) focus — reported to the remote as
	/// `CSI I` / `CSI O` if it enabled focus reporting (§23).
	WindowFocus(bool),
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
	/// A pointer event a full-screen program asked to hear about (§9), already encoded as
	/// the report it expects. Only raised while the remote has a mouse protocol on and the
	/// user is not holding Shift, so it never competes with the selection above.
	MouseReport(Vec<u8>),
	/// The wheel scrolled cmote's own scrollback (§23); the payload is a signed line count,
	/// positive up into history. Raised by the grid only when no mouse-aware program wants the
	/// wheel, so it never competes with the mouse report above.
	TerminalScroll(i32),
	/// Copy the current selection to the system clipboard.
	CopyPressed,
	/// Open an OSC 8 hyperlink from the terminal's context menu (§24). Carries the URI, so
	/// the menu item stands alone; the Ctrl+click path opens straight from `on_grid_pressed`
	/// and raises no message.
	LinkOpen(String),
	/// Copy an OSC 8 hyperlink's URI to the clipboard, from the same context menu (§24).
	LinkCopy(String),
	/// Read the system clipboard, then paste it into the shell.
	PastePressed,
	/// The async clipboard read finished: `Some(text)` to paste, `None` if empty.
	Pasted(Option<String>),
	/// The status bar's "Sync" button (§19): move the shell into the directory the files
	/// pane is showing. Carries no path — the pane's own is the only thing it can mean, and
	/// reading it when the press arrives keeps it from being a directory the pane has since
	/// left (same discipline as `Files(CopyCurrentPath)` and `Files(ParentOpened)`).
	SyncPressed,
	/// Dismiss the open context menu without choosing an item.
	MenuDismissed,
	/// A window-frame tick while a copy-confirmation toast is showing (§10). Carries no
	/// payload: `update` reads the toast's own age to decide whether its dwell has elapsed.
	/// Only subscribed to while a toast is up, so it costs nothing the rest of the time.
	SnackbarTick,
	// --- file upload to the remote (§17) ---
	/// The status bar's File… button — open the native multi-file picker.
	UploadPickPressed,
	/// The picker closed on the status-bar path: the files to send, empty if cancelled. The
	/// destination is chosen later, on the Upload button, from the shell's working directory.
	UploadFilesPicked(Vec<PathBuf>),
	/// The picker closed for an "Upload…" started from a right-click surface: the files plus
	/// the folder they go into — the shell cwd (terminal menu), the pane's directory (files
	/// pane), or the folder itself (tree). Opens the confirmation straight away.
	UploadFilesPickedInto {
		files: Vec<PathBuf>,
		dir: String,
	},
	/// The status bar's Upload button — confirm the picked batch into the shell's cwd.
	UploadPressed,
	/// The terminal grid's right-click "Upload…" — pick files to send into the shell's cwd.
	TerminalUploadPressed,
	/// The destination folder field in the confirmation changed.
	UploadDestChanged(String),
	/// The destination folder was confirmed — pre-scan the server for collisions (§17).
	UploadConfirmed,
	/// The answer to "some of these are already there" for an upload batch (§17).
	UploadClashResolved(ClashChoice),
	/// The user backed out of an upload confirmation or its collision question (Cancel / ✕ /
	/// backdrop / Esc) — nothing is sent.
	UploadCancelled,
	/// Something happened in the files pane (§19). Nested for the same reason the tree's
	/// messages are.
	Files(FilesMessage),
	/// The save dialog for a download closed: `local` is where to put the file, or `None`
	/// if the user cancelled (§19). `remote` is what they asked to download.
	DownloadTargetPicked {
		remote: String,
		local: Option<PathBuf>,
	},
	/// The folder picker for a multi-file download closed (§21): `dir` is where the batch
	/// is going, or `None` if the user cancelled.
	DownloadFolderPicked {
		remotes: Vec<String>,
		dir: Option<PathBuf>,
	},
	/// The answer to "some of these files are already there" (§21).
	DownloadClash(ClashChoice),
	// --- create / delete / recursive transfer (§18, §17, §19) ---
	/// The "new folder" dialog's name field changed.
	NewFolderNameChanged(String),
	/// The "new folder" dialog was submitted (the Create button, or Enter in the field).
	NewFolderConfirmed,
	/// The "new folder" dialog was dismissed (Cancel / ✕ / backdrop / Esc) — nothing is made.
	NewFolderCancelled,
	/// The delete confirmation was confirmed — remove the held entries from the server (§18).
	DeleteConfirmed,
	/// The delete confirmation was dismissed — keep the entries.
	DeleteCancelled,
	/// The user's answer to a recursive transfer's file-collision prompt (§17, §19). Carries the
	/// six-way choice; `update` sends it down and clears the dialog so the transfer resumes.
	TransferConflictResolved(bridge::ConflictChoice),
	/// The folder picker for a recursive UPLOAD closed (§17): `local` is the folder to send (or
	/// `None` if cancelled), `dir` the remote directory it goes into.
	UploadFolderPicked {
		local: Option<PathBuf>,
		dir: String,
	},
	/// The folder picker for a recursive DOWNLOAD closed (§19): `remote` is the folder to fetch,
	/// `local` where to recreate it (or `None` if cancelled).
	DownloadFolderTargetPicked {
		remote: String,
		local: Option<PathBuf>,
	},
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
			// A window opens focused, and a program that enables focus reporting assumes the
			// same (§23); both are corrected by the first real change if the platform disagrees.
			window_focused: true,
			shell_focus_reported: true,
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
			Message::ConnectPressed => return self.on_connect_pressed(),
			Message::BackPressed => return self.go_to_form(),
			Message::FormKey(event) => return self.on_form_key(event),
			Message::AcceptHostKey => self.on_host_key_decision(true),
			Message::RejectHostKey => self.on_host_key_decision(false),
			Message::PassphraseChanged(value) => self.passphrase_input = value,
			Message::PassphraseSubmitted => self.on_passphrase_submitted(),
			Message::PassphraseCancelled => return self.on_passphrase_cancelled(),
			Message::InteractiveAnswerChanged(index, value) => {
				if let Some(slot) = self.interactive_answers.get_mut(index) {
					*slot = value;
				}
			}
			Message::InteractiveSubmitted => return self.on_interactive_submitted(),
			Message::InteractiveCancelled => return self.on_interactive_cancelled(),
			Message::RememberToggled => self.form.remember = !self.form.remember,
			Message::VaultInputChanged(value) => self.vault_input = value,
			Message::VaultConfirmChanged(value) => self.vault_confirm = value,
			Message::VaultSubmitted => return self.on_vault_submitted(),
			Message::VaultCancelled => return self.on_vault_cancelled(),
			Message::Key(event) => return self.on_key(event),
			Message::WindowResized(size) => self.on_window_resized(size),
			Message::WindowFocus(focused) => self.on_window_focus(focused),
			Message::DisconnectPressed => self.on_disconnect_pressed(),
			Message::DisconnectConfirmed => return self.on_disconnect_confirmed(),
			Message::DisconnectCancelled => self.confirm_disconnect = false,
			Message::GridMoved(point) => self.on_grid_moved(point),
			Message::GridPressed => self.on_grid_pressed(),
			Message::GridReleased => self.on_grid_released(),
			Message::GridRightPressed => self.menu = Some(self.pointer),
			Message::MouseReport(bytes) => self.on_mouse_report(bytes),
			Message::TerminalScroll(lines) => self.on_terminal_scroll(lines),
			Message::CopyPressed => return self.on_copy_rich(),
			Message::LinkOpen(uri) => {
				self.menu = None;
				self.follow_link(&uri);
			}
			Message::LinkCopy(uri) => {
				self.menu = None;
				return self.copy_to_clipboard(uri);
			}
			Message::PastePressed => return self.on_paste(),
			Message::Pasted(text) => self.on_pasted(text),
			Message::SyncPressed => self.on_sync(),
			Message::MenuDismissed => self.menu = None,
			// A frame tick while the toast is up (§10): drop it once it has outlived its
			// dwell. Clearing it removes the `frames()` subscription next diff, so the
			// ticking stops on its own — no timer to cancel.
			Message::SnackbarTick => {
				if self
					.snackbar
					.as_ref()
					.is_some_and(|snackbar| snackbar.shown_at.elapsed() >= SNACKBAR_DWELL)
				{
					self.snackbar = None;
				}
			}
			Message::UploadPickPressed => return browse_upload(),
			// A cancelled picker yields no files, which keeps whatever was already chosen —
			// the same rule the key-file picker on the form uses.
			Message::UploadFilesPicked(files) => {
				if !files.is_empty() {
					self.upload_files = files;
					self.transfer_notice = None;
				}
			}
			// Started from a right-click surface: the folder is already known, so pick the
			// files and go straight to the confirmation.
			Message::UploadFilesPickedInto { files, dir } => {
				if !files.is_empty() {
					self.upload_files = files;
					self.upload_dir = dir;
					self.transfer_notice = None;
					return self.open_upload_confirm();
				}
			}
			Message::UploadPressed => {
				self.upload_dir = self
					.terminal
					.as_ref()
					.and_then(term::Terminal::cwd)
					.unwrap_or_default()
					.to_owned();
				return self.open_upload_confirm();
			}
			Message::TerminalUploadPressed => {
				// The grid's right-click "Upload…": pick files for the shell's own directory.
				self.menu = None;
				let dir = self
					.terminal
					.as_ref()
					.and_then(term::Terminal::cwd)
					.unwrap_or_default()
					.to_owned();
				return browse_upload_into(dir);
			}
			Message::UploadDestChanged(value) => self.upload_dir = value,
			Message::UploadConfirmed => return self.on_upload_confirmed(),
			Message::UploadClashResolved(choice) => self.on_upload_clash(choice),
			Message::UploadCancelled => self.cancel_upload(),
			Message::Explorer(message) => return self.on_explorer(message),
			Message::Files(message) => return self.on_files(message),
			Message::DownloadTargetPicked { remote, local } => self.start_download(remote, local),
			Message::DownloadFolderPicked { remotes, dir } => self.on_download_folder(remotes, dir),
			Message::DownloadClash(choice) => {
				// Taking it closes the dialog whichever way the question was answered.
				if let Some(clash) = self.clash.take()
					&& choice != ClashChoice::Cancel
				{
					self.queue_downloads(&clash.remotes, &clash.dir, choice);
				}
			}
			// Create / delete / recursive transfer (§18, §17, §19).
			Message::NewFolderNameChanged(value) => {
				if let Some(new_folder) = self.new_folder.as_mut() {
					new_folder.name = value;
				}
			}
			Message::NewFolderConfirmed => self.confirm_new_folder(),
			Message::NewFolderCancelled => self.new_folder = None,
			Message::DeleteConfirmed => self.confirm_remote_delete(),
			Message::DeleteCancelled => self.pending_delete = None,
			Message::TransferConflictResolved(choice) => self.on_conflict_resolved(choice),
			Message::UploadFolderPicked { local, dir } => self.start_upload_tree(local, dir),
			Message::DownloadFolderTargetPicked { remote, local } => {
				self.start_download_tree(remote, local);
			}
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

	/// Validate the form, then begin connecting (§10). Cheap validation fails fast to the error
	/// screen. When "Remember" is ticked and a non-empty secret is in play (§16), the secret is
	/// captured to store on success; if the vault is not yet unlocked the whole connect is
	/// deferred behind the master-passphrase prompt and resumed on unlock.
	fn on_connect_pressed(&mut self) -> iced::Task<Message> {
		let params = match self.form.validate() {
			Ok(params) => params,
			Err(reason) => {
				self.show_error(&reason);
				return iced::Task::none();
			}
		};

		// Decide, before `params` moves into the dial, whether this connect should remember its
		// secret — and capture it now. Only a non-empty secret is worth storing (§16).
		if self.form.remember
			&& let Some(secret) = extract_secret(&params.auth)
		{
			let endpoint = crate::profiles::endpoint_of(&params.user, &params.host, params.port);
			self.pending_remember = Some((endpoint, secret));
			// A secret is in play, so the vault must be unlocked to store it. If it is not yet,
			// defer the connect behind the master-passphrase prompt and resume it on unlock.
			if self.vault.is_none() {
				return self.open_vault_modal(VaultPending::Connect(params));
			}
		}

		self.dial(params)
	}

	/// Send a validated `Connect` to the SSH task and move to the connecting screen (§10). Split
	/// from `on_connect_pressed` so the deferred-vault path can resume straight here once the
	/// master passphrase is entered (§16). Records the profile (no secret) to save if the
	/// session opens (§14).
	fn dial(&mut self, params: bridge::ConnectParams) -> iced::Task<Message> {
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
			// Placeholder like `name`: the stored preference wins on connect, and a
			// brand-new target takes the default `upsert_on_connect` gives it (§14).
			show_hidden: self.explorer.show_hidden(),
			// The pending target only carries auth into `upsert_on_connect`; the remembered
			// session (§22) and the remember flag (§16) live with the *stored* target, which the
			// upsert leaves untouched, so these placeholders are never read.
			terminal_path: None,
			files_path: None,
			explorer_width: None,
			files_height: None,
			remember_secret: false,
		});

		let status = format!("connecting to {}:{}…", params.host, params.port);
		// The label the terminal status bar will show once the shell is open (§10);
		// capture it now, before `params` moves into the command.
		let endpoint = format!("{}@{}:{}", params.user, params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			self.connection = Some(endpoint);
			self.screen = Screen::Connecting { status };
		} else {
			// The command never left: do not leave a pending target — or a secret to save — behind.
			self.pending_target = None;
			self.pending_remember = None;
		}
		iced::Task::none()
	}

	/// Open the master-passphrase prompt for the secret vault (§16), recording what to resume
	/// once it unlocks. The prompt is in CREATE mode (two fields) when no vault file exists yet,
	/// UNLOCK mode (one field) when it does — fixed here so the view need not re-check the disk.
	/// It shows over the connect form, so the caller has already put the form on screen.
	fn open_vault_modal(&mut self, pending: VaultPending) -> iced::Task<Message> {
		self.vault_creating = !crate::vault::Vault::exists();
		self.vault_input.clear();
		self.vault_confirm.clear();
		self.vault_failed = false;
		self.vault_pending = Some(pending);
		self.set_dialog_body(if self.vault_creating {
			ui::VAULT_CREATE_BODY
		} else {
			ui::VAULT_UNLOCK_BODY
		});
		self.screen = Screen::VaultUnlock;
		iced::widget::operation::focus(ui::VAULT_INPUT_ID)
	}

	/// Handle the vault prompt's submit (§16). Creating: the passphrase must be non-empty and
	/// match its confirmation, else re-ask with the mismatch hint. Unlocking: a wrong passphrase
	/// (or an unreadable file) re-asks with the "not correct" hint — no oracle beyond that
	/// (§12). On success the unlocked vault is kept for the session and the pending action
	/// resumes. The typed values are taken (not copied) out of the fields so nothing lingers.
	fn on_vault_submitted(&mut self) -> iced::Task<Message> {
		let entered = std::mem::take(&mut self.vault_input);

		let opened = if self.vault_creating {
			let confirm = std::mem::take(&mut self.vault_confirm);
			// A new master passphrase must be non-empty and typed identically twice, so the one
			// value that protects everything can never be a typo the user cannot reproduce.
			if entered.is_empty() || entered != confirm {
				self.vault_failed = true;
				return iced::widget::operation::focus(ui::VAULT_INPUT_ID);
			}
			crate::vault::Vault::create(entered)
		} else {
			crate::vault::Vault::unlock(entered)
		};

		match opened {
			Ok(vault) => {
				self.vault = Some(vault);
				self.vault_confirm.clear();
				self.vault_failed = false;
				self.resume_vault_pending()
			}
			Err(error) => {
				// Wrong passphrase, or a damaged / unresolvable file: re-ask. The detail is
				// logged, never shown (§12).
				eprintln!("could not open the vault: {error:#}");
				self.vault_failed = true;
				iced::widget::operation::focus(ui::VAULT_INPUT_ID)
			}
		}
	}

	/// Resume whatever the vault unlock was blocking (§16): continue the deferred connect, or
	/// pre-fill the form's masked field from the now-readable secret. A `Prefill` whose entry is
	/// missing (the flag out of step with the vault) simply leaves the field blank.
	fn resume_vault_pending(&mut self) -> iced::Task<Message> {
		match self.vault_pending.take() {
			Some(VaultPending::Connect(params)) => self.dial(params),
			Some(VaultPending::Prefill(endpoint)) => {
				if let Some(vault) = &self.vault
					&& let Some(secret) = vault.get(&endpoint).cloned()
				{
					self.fill_secret_field(&secret);
				}
				self.go_to_form()
			}
			None => iced::Task::none(),
		}
	}

	/// Dismiss the vault prompt (§16): clear the typed values and the pending secret, and drop
	/// back to the connect form (populated behind the prompt in both flows). Cancelling never
	/// stores anything — the deferred connect and the pre-fill are simply abandoned; the user
	/// can still type the secret by hand.
	fn on_vault_cancelled(&mut self) -> iced::Task<Message> {
		self.vault_input.clear();
		self.vault_confirm.clear();
		self.vault_failed = false;
		self.vault_pending = None;
		self.pending_remember = None;
		self.screen = Screen::Connect;
		iced::Task::none()
	}

	/// Put a decrypted secret into the masked form field its auth method uses (§16): the
	/// password under password auth, the key passphrase under key auth. One endpoint has one
	/// stored secret and one auth kind, so the destination is unambiguous.
	fn fill_secret_field(&mut self, secret: &Secret) {
		match self.form.auth_kind {
			AuthKind::Password => self.form.password = secret.expose().to_owned(),
			AuthKind::Key => self.form.passphrase = secret.expose().to_owned(),
			// Interactive auth has no stored secret to fill — every factor is typed live (§7).
			// A remembered target is never interactive, so this arm is not reached in practice.
			AuthKind::Interactive => {}
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

	/// Send the typed keyboard-interactive answers to the SSH task (§7) and return to a
	/// connecting status. Each answer is moved straight into a `Secret` and the buffers cleared,
	/// so no plain copy of an OTP or password lingers in app state (§12). The server drives what
	/// happens next: another prompt (the dialog reappears), success, or a generic failure.
	fn on_interactive_submitted(&mut self) -> iced::Task<Message> {
		let answers: Vec<Secret> = std::mem::take(&mut self.interactive_answers)
			.into_iter()
			.map(Secret::new)
			.collect();
		self.interactive_prompts.clear();
		if self.send_command(SshCommand::Interactive(answers)) {
			self.screen = Screen::Connecting {
				status: "authenticating…".to_string(),
			};
		}
		iced::Task::none()
	}

	/// Dismiss the keyboard-interactive prompt: tear the connection down and go back to the form
	/// (§7). Clearing the buffers first means the discarded answers do not linger (§12).
	fn on_interactive_cancelled(&mut self) -> iced::Task<Message> {
		self.interactive_answers.clear();
		self.interactive_prompts.clear();
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
			SshEvent::Interactive {
				name,
				instructions,
				prompts,
			} => {
				// Seed the selectable body with a fixed intro plus the server's heading and
				// blurb — either may be empty — so the whole message is one selectable, copyable
				// block (§7, §10). One blank line separates each part that is present.
				let mut body = ui::INTERACTIVE_DIALOG_BODY.to_owned();
				for extra in [name.trim(), instructions.trim()] {
					if !extra.is_empty() {
						body.push_str("\n\n");
						body.push_str(extra);
					}
				}
				self.set_dialog_body(&body);
				// Start every field blank, one per prompt, and show the dialog. The server only
				// sends a request with at least one prompt here (an empty, message-only request
				// is answered by the SSH task itself), so focusing the first field is always apt.
				self.interactive_answers = vec![String::new(); prompts.len()];
				self.interactive_prompts = prompts;
				self.screen = Screen::Interactive;
				return iced::widget::operation::focus(ui::interactive_field_id(0));
			}
			SshEvent::Connected => {
				// The session is real: persist the target now (§14) — profiles only, no
				// secret. `upsert_on_connect` adds it (or refreshes an existing endpoint,
				// keeping its custom name) and returns its key so we pre-select the row
				// for when the user returns to the home list.
				let mut resume_terminal = None;
				let mut resume_files = None;
				if let Some(target) = self.pending_target.take() {
					let key = self.targets.upsert_on_connect(
						&target.host,
						target.port,
						&target.user,
						target.auth_kind,
						target.key_path,
					);
					// Restore this target's remembered session before the panels list anything
					// (§22): the `.*` filter and panel sizes go on now, and the resume paths
					// come back to drive the cd / pane / tree restore below. `upsert_on_connect`
					// leaves a known endpoint's saved state untouched, so it is still here to
					// read; taking an owned snapshot ends the borrow before the panels change.
					if let Some(session) = self
						.targets
						.find(&key)
						.map(crate::profiles::Target::session)
					{
						(resume_terminal, resume_files) = self.restore_session(session);
					}
					// Remembered-secret bookkeeping (§16). A successful connect is the ONLY place
					// a secret is persisted — the credentials are now known good, so a wrong
					// password was never stored. With "Remember" on, store what dial captured;
					// with it off, forget any secret the vault held for this endpoint. The
					// target's flag is then synced to what the vault actually holds, so the home
					// list never promises a pre-fill that is not there. All of this needs the
					// vault unlocked, which the dial / open flow already ensured whenever a secret
					// was in play; if it is locked (the user never engaged it) the flag is left
					// as stored.
					if let Some(vault) = self.vault.as_mut() {
						if let Some((endpoint, secret)) = self.pending_remember.take() {
							if let Err(error) = vault.store(&endpoint, secret) {
								eprintln!("could not save the vault: {error:#}");
							}
						} else if !self.form.remember
							&& let Err(error) = vault.forget(&key)
						{
							eprintln!("could not update the vault: {error:#}");
						}
						self.targets.set_remembered(&key, vault.get(&key).is_some());
					}
					self.pending_remember = None;
					self.home_selected = Some(key);
					if let Err(error) = self.targets.save() {
						eprintln!("could not save targets: {error:#}");
					}
				}
				// A shell is open: spin up an emulator at the pty size we asked for,
				// show the terminal, then immediately refit it to the real window
				// rather than waiting for the first resize event.
				let mut terminal = term::Terminal::new(term::DEFAULT_ROWS, term::DEFAULT_COLS);
				// Hand the emulator the cell's pixel size (the GUI owns the metrics, §9) so it
				// can answer a program that asks its text area in pixels (CSI 14t, §23).
				terminal.set_cell_pixels(
					ui::terminal::CELL_WIDTH.round() as u16,
					ui::terminal::CELL_HEIGHT.round() as u16,
				);
				self.terminal = Some(terminal);
				self.clear_grid_interaction();
				self.screen = Screen::Terminal;

				// Resume where the last session left off (§22), falling back to the root for a
				// first connection or a shell that never announced a cwd — the previous
				// behaviour. The pane opens at its own remembered directory; the tree opens the
				// chain down to it and selects it, so both panels start on the resume point.
				let files_start = resume_files.unwrap_or_else(|| explorer::ROOT.to_owned());
				let needed = self.explorer.reveal_if_new(&files_start);
				self.list_dirs(needed);
				if let Some(request) = self.files.show(&files_start) {
					self.list_files(request);
				}

				// Replay the remembered shell directory as a `cd` so the shell itself resumes
				// there, and pin the pane against the resulting announcements until the shell
				// settles (§22) — otherwise its login-then-`cd` prompts would drag the pane off
				// a *different* remembered files directory. Nothing to replay leaves the shell
				// at its login directory, exactly as before.
				if let Some(cwd) = resume_terminal {
					let line = format!("cd {}\r", explorer::shell_quote(&cwd));
					self.send_command(SshCommand::Input(line.into_bytes()));
					self.resume_cwd = Some(cwd);
				}
				return fit_terminal();
			}
			SshEvent::Output(bytes) => {
				// Feed raw shell output into the emulator; the next render draws it.
				// `process` also returns the engine's replies to the status/identity queries
				// it carried (§9, §23): a program that sent one blocks reading its stdin until
				// the reply reaches it, so send the returned bytes straight back on the input
				// channel, the same path a keystroke takes. The same bytes may carry a cwd
				// announcement, so read the (possibly new) directory out before the borrow
				// ends and let the tree follow it (§18).
				let (cwd, replies) = match self.terminal.as_mut() {
					Some(terminal) => {
						let replies = terminal.process(&bytes);
						(terminal.cwd().map(str::to_owned), replies)
					}
					None => (None, Vec::new()),
				};
				if !replies.is_empty() {
					self.send_command(SshCommand::Input(replies));
				}
				// That chunk may have turned focus reporting on or off (§23); reconcile the
				// remote to the shell's true focus, so a program enabling `?1004` while a side
				// panel holds the keyboard is not left believing the shell is focused.
				self.report_focus();
				if let Some(cwd) = cwd {
					let needed = self.explorer.reveal_if_new(&cwd);
					self.list_dirs(needed);
					// While a reconnect is resuming (§22) the pane is pinned to its own
					// remembered directory: the shell's login-then-`cd` announcements must not
					// drag it off until the shell has settled at the cwd we replayed. Once it
					// has, seed the follow-guard — so the pane does not jump now but *does*
					// follow the next real `cd` — and stop pinning. Off the resume path the
					// pane follows the shell as usual (§19): only a real move re-lists.
					match self.resume_cwd.as_deref() {
						Some(target) if target == cwd.as_str() => {
							self.files.set_followed(&cwd);
							self.resume_cwd = None;
						}
						Some(_) => {}
						None => {
							if let Some(request) = self.files.follow(&cwd) {
								self.list_files(request);
							}
						}
					}
				}
			}
			SshEvent::FilesChunk {
				request,
				entries,
				done,
			} => self.files.chunk(request, entries, done),
			SshEvent::FilesFailed { request, reason } => self.files.failed(request, reason),
			// The server's own timezone and one resolved symlink, both for the details
			// popup beside the selection (§20).
			SshEvent::Zone(zone) => self.files.set_zone(zone),
			SshEvent::LinkTarget { path, target } => self.files.set_link_target(path, target),
			SshEvent::DownloadDone(path) => {
				self.transfer = None;
				self.downloaded += 1;
				self.transfer_notice = Some(format!("Saved to {path}"));
				// A batch keeps going, and says how it went once the last file lands (§21).
				self.pump_downloads();
				if self.transfer.is_none() && self.downloaded > 1 {
					self.transfer_notice = Some(format!("Saved {} files", self.downloaded));
				}
			}
			SshEvent::DownloadFailed(message) => {
				self.transfer = None;
				self.transfer_notice = Some(message);
				// One file failing does not abandon the rest of the batch — the notice says
				// which one it was, and the queue moves on.
				self.pump_downloads();
			}
			SshEvent::DirListed { path, dirs } => self.explorer.listed(&path, dirs),
			SshEvent::DirFailed { path, reason } => self.explorer.failed(&path, reason),
			SshEvent::RenameDone { from, to } => {
				// The entry moved: re-list its parent so the row reappears under the new
				// name, in the right sort position. Both panels may be showing it (§19).
				if let Some(parent) = self.explorer.renamed(&from, &to) {
					self.send_command(SshCommand::ListDir(parent));
				}
				if let Some(request) = self.files.renamed(&from) {
					self.list_files(request);
				}
			}
			SshEvent::RenameFailed(reason) => {
				self.explorer.set_notice(reason.clone());
				self.files.set_notice(reason);
			}
			SshEvent::MakeDirDone(path) => {
				// The new folder appeared inside its parent: re-list the parent in both panels so
				// it shows in the right sort position (§18). Take an owned parent to end the borrow.
				if let Some(parent) = explorer::parent(&path).map(str::to_owned) {
					self.refresh_remote_dir(&parent);
				}
			}
			SshEvent::MakeDirFailed(reason) => {
				self.explorer.set_notice(reason.clone());
				self.files.set_notice(reason);
			}
			SshEvent::DeleteDone(paths) => self.on_deleted(paths),
			SshEvent::DeleteFailed(reason) => {
				self.explorer.set_notice(reason.clone());
				self.files.set_notice(reason);
			}
			SshEvent::TransferConflict { name } => {
				// Park the transfer behind the six-way question, naming the file it is about (§17,
				// §19). The shared dialog body carries a fixed intro plus that name.
				self.set_dialog_body(&format!("{}\n\n{name}", ui::terminal::CONFLICT_DIALOG_BODY));
				self.transfer_conflict = Some(name);
			}
			SshEvent::UploadExists(path) => {
				// The batch pre-scan already settled every collision it knew about (§17), so
				// reaching here means this file appeared on the server AFTER the scan. Skip it
				// rather than reopening the question mid-batch, and move the queue on.
				self.transfer = None;
				self.transfer_notice = Some(format!(
					"Skipped {} — it appeared on the server",
					explorer::name(&path)
				));
				self.pump_uploads();
				self.finish_batch_if_drained();
			}
			SshEvent::UploadPrescan { collisions } => self.on_upload_prescan(collisions),
			// Progress only means something while a transfer is running; a late event
			// after a failure must not revive the bar.
			SshEvent::TransferProgress { sent, total } => {
				if matches!(self.transfer, Some(TransferState::Running { .. })) {
					self.transfer = Some(TransferState::Running { sent, total });
				}
			}
			SshEvent::UploadDone(path) => {
				// One file landed; count it and start the next. The closing notice, and
				// clearing the picked files, wait until the whole batch has drained (§17).
				self.transfer = None;
				self.uploaded += 1;
				self.pump_uploads();
				if self.transfer.is_none() && self.uploads.is_empty() {
					self.transfer_notice = Some(if self.uploaded > 1 {
						format!("Uploaded {} files", self.uploaded)
					} else {
						format!("Uploaded to {path}")
					});
					self.finish_batch();
				}
			}
			SshEvent::UploadFailed(message) => {
				// One file failing does not abandon the rest of the batch — the notice says
				// what went wrong, and the queue moves on (§17). The failure shows in the
				// status bar rather than the error screen, which would tear the shell down for
				// a file that never left.
				self.transfer = None;
				self.transfer_notice = Some(message);
				self.pump_uploads();
				self.finish_batch_if_drained();
			}
			SshEvent::Disconnected => {
				// A remote hangup ends a live session too: remember where it was (§22).
				self.persist_session();
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				return self.go_home();
			}
			SshEvent::Error(message) => {
				// Only saves when a shell had actually opened — an auth/handshake failure
				// reaches here with no terminal, and `persist_session` then does nothing (§22).
				self.persist_session();
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
		// The explorer panel takes its width out of the grid and the files pane its height,
		// so the same call serves a window resize and either panel's resize (§18, §19).
		let (rows, cols) =
			ui::terminal::grid_size(size, self.explorer.reserved(), self.files.reserved());
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
		// Save where the shell and pane were before any of it is torn down (§22).
		self.persist_session();
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

	/// Open the connect form pre-filled from the selected target (§14): its host / port / user /
	/// auth / key path are copied in. The secret field starts empty UNLESS the target has a
	/// remembered secret (§16), in which case it is pre-filled from the vault — unlocking it via
	/// the master-passphrase prompt first if the vault is not yet open. A stale/missing
	/// selection is a no-op.
	fn open_selected_target(&mut self) -> iced::Task<Message> {
		self.home_menu_open = false;
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		// Copy out the fields before touching `self.form`, so the borrow of `self.targets` ends
		// first (assigning the form mutably borrows `self`).
		let Some((host, port, user, auth_kind, key_path, remember)) =
			self.targets.find(&key).map(|target| {
				(
					target.host.clone(),
					target.port,
					target.user.clone(),
					target.auth_kind,
					target.key_path.clone(),
					target.remember_secret,
				)
			})
		else {
			return iced::Task::none();
		};
		self.form = ui::connect::ConnectForm {
			host,
			port: port.to_string(),
			user,
			auth_kind,
			password: String::new(),
			key_path,
			passphrase: String::new(),
			// A remembered target opens with the box already ticked (§16); untick to stop
			// remembering it, which forgets the stored secret on the next connect.
			remember,
		};

		if remember {
			match &self.vault {
				// Vault already open this session: pull the secret straight into the field.
				Some(vault) => {
					if let Some(secret) = vault.get(&key).cloned() {
						self.fill_secret_field(&secret);
					}
				}
				// Vault locked: show the (now populated) form as the backdrop and prompt to
				// unlock; the pre-fill resumes on success.
				None => {
					self.screen = Screen::Connect;
					return self.open_vault_modal(VaultPending::Prefill(key));
				}
			}
		}
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
	/// Clears the selection so the menu and the shortcuts no longer point at a gone row. Also
	/// forgets any remembered secret for this endpoint (§16) when the vault is unlocked; if it
	/// is locked the encrypted entry is left orphaned in `secrets.age` — harmless (it is
	/// unreachable without its target and still encrypted) and pruned only when next unlocked.
	fn delete_selected_target(&mut self) {
		self.home_menu_open = false;
		self.confirm_delete = false;
		if let Some(key) = self.home_selected.take() {
			if let Some(vault) = self.vault.as_mut()
				&& let Err(error) = vault.forget(&key)
			{
				eprintln!("could not update the vault: {error:#}");
			}
			if self.targets.remove(&key)
				&& let Err(error) = self.targets.save()
			{
				eprintln!("could not save targets: {error:#}");
			}
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
	/// activate the current stop, and Esc returns to the home list. What "activate" means
	/// depends on the stop: a radio/button runs its own callback (switch auth, Browse, or —
	/// on the Connect stop — submit); a TEXT stop has no callback of its own, so Enter there
	/// submits the whole form while Space is left to type a space in the field. Anything else
	/// is ignored here; the focused input still receives it through the widget tree.
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
			iced::keyboard::Key::Named(named @ (Named::Enter | Named::Space)) => {
				if self.form_focus.input_id(self.form.auth_kind).is_some() {
					// A text stop: Enter submits the form (the field has no submit of its
					// own), Space types a space and is left to the field.
					if named == Named::Enter {
						iced::Task::done(Message::ConnectPressed)
					} else {
						iced::Task::none()
					}
				} else if let Some(message) = self.form_focus.activation(self.form.auth_kind) {
					// A radio/button stop turns the key into its own activation message.
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

	/// Route a key press on the terminal screen (§20): to the focused panel, or — when the
	/// shell has the focus, which is where every session starts — down the channel.
	/// Non-input keys (bare modifiers, unmapped keys) encode to nothing and are
	/// dropped. Keyboard events only reach here on the Terminal screen (the
	/// subscription is added only there), so no extra screen check is needed.
	fn on_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::{Code, Named, Physical};

		// While the Disconnect confirmation modal is open, keystrokes belong to the
		// dialog (notably Ctrl+C to copy the selected message text), not the remote
		// shell — the `keyboard::listen` subscription fires independently of widget
		// focus, so without this guard Ctrl+C would also send ETX to the session. The
		// dialog's own widgets still receive the keys through the widget tree (§10).
		if self.confirm_disconnect {
			return iced::Task::none();
		}

		// The one place the modifier state is kept (§21): a mouse press carries none of its
		// own, so Ctrl+click, Shift+click and Ctrl+drag all read it from here.
		if let iced::keyboard::Event::ModifiersChanged(modifiers) = event {
			self.modifiers = modifiers;
			return iced::Task::none();
		}

		// Split the event into the pieces the shell encoder needs, plus which transition it is: a
		// press flags whether it is an auto-repeat, and a release carries no produced text (§25).
		// Other keyboard events (a bare modifier change is handled above) carry no key.
		let (key, physical_key, text, modifiers, key_event) = match event {
			iced::keyboard::Event::KeyPressed {
				key,
				physical_key,
				text,
				modifiers,
				repeat,
				..
			} => {
				let kind = if repeat {
					term::kitty::KeyEvent::Repeat
				} else {
					term::kitty::KeyEvent::Press
				};
				(key, physical_key, text, modifiers, kind)
			}
			iced::keyboard::Event::KeyReleased {
				key,
				physical_key,
				modifiers,
				..
			} => (
				key,
				physical_key,
				None,
				modifiers,
				term::kitty::KeyEvent::Release,
			),
			_ => return iced::Task::none(),
		};
		self.modifiers = modifiers;

		// A release never drives cmote's own shortcuts (closing a modal, cycling focus, scrolling
		// history) — those all fire on the press. It matters only as a key-up the shell itself may
		// want, and only under the kitty event-types flag (§25); so it skips the whole interaction
		// pipeline below and goes straight to the shell, but solely when the shell owns the
		// keyboard right now. In every legacy case the encoder returns nothing, so this is inert.
		if key_event == term::kitty::KeyEvent::Release {
			if self.shell_owns_keyboard() {
				return self.forward_to_shell(&key, physical_key, None, modifiers, key_event);
			}
			return iced::Task::none();
		}

		// The collision questions (§17, §21) are modal: Esc backs out of the whole batch,
		// everything else waits for a button. The download's and the upload's read the same.
		if self.clash.is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.clash = None;
			}
			return iced::Task::none();
		}
		if self.upload_clash.is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.cancel_upload();
			}
			return iced::Task::none();
		}

		// Same rule for the upload confirmation (§17): while it is open the keyboard belongs
		// to it — the destination field types through the widget tree — so nothing here
		// reaches the shell. Esc backs out; a running transfer has nothing to back out of, so
		// it just swallows the key.
		if let Some(state) = self.transfer {
			if matches!(state, TransferState::ConfirmPath)
				&& matches!(key, iced::keyboard::Key::Named(Named::Escape))
			{
				self.cancel_upload();
			}
			return iced::Task::none();
		}

		// And the same for the folder tree's inline rename (§18): the field types through
		// the widget tree, Esc abandons the edit, and nothing reaches the shell meanwhile
		// — otherwise renaming a folder would also be typing at the remote prompt.
		if self.explorer.editing().is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.explorer.cancel_rename();
			}
			return iced::Task::none();
		}

		// And the files pane's inline rename (§19), for the same reason.
		if self.files.editing().is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.files.cancel_rename();
			}
			return iced::Task::none();
		}

		// Ctrl+Tab hands the keyboard on to the next panel, Ctrl+Shift+Tab to the previous
		// one (§20). Taken before anything else on this screen: it is the way *out* of a
		// panel that is swallowing keys, so nothing may shadow it.
		if modifiers.control() && matches!(key, iced::keyboard::Key::Named(Named::Tab)) {
			self.cycle_focus(modifiers.shift());
			return iced::Task::none();
		}

		// A focused panel keeps the key; only the shell's own focus reaches the channel.
		match self.focus {
			Focus::Tree => return self.on_tree_key(&key),
			Focus::Files => return self.on_files_key(&key, modifiers),
			Focus::Terminal => {}
		}

		// Copy / paste keyboard shortcuts, with the shell focused (§10). Taken before the key is
		// encoded for the remote, so a terminal binding wins over the program — the way xterm and
		// kitty keep these for the terminal itself. Matched on the PHYSICAL key, so the shortcut
		// holds on any layout (AZERTY, Dvorak, …), not only where C / V sit on QWERTY. Alt / Logo
		// held means it is some other combination, so leave those for the shell.
		if modifiers.control() && !modifiers.alt() && !modifiers.logo() {
			match physical_key {
				// Ctrl+C copies the selection as rich HTML (colour + attributes), but ONLY when
				// something is selected; with no selection it must fall through to the shell as the
				// interrupt (ETX / SIGINT). Ctrl+Shift+C always copies, as plain text only. A rich
				// copy then clears the selection, so an immediate second Ctrl+C is the interrupt,
				// not a re-copy — a stale highlight can never silently swallow an intended Ctrl+C.
				Physical::Code(Code::KeyC) => {
					if modifiers.shift() {
						return self.on_copy();
					}
					if self
						.selection
						.is_some_and(|selection| !selection.is_empty())
					{
						let task = self.on_copy_rich();
						self.selection = None;
						return task;
					}
					// no selection: fall through so Ctrl+C reaches the shell as the interrupt
				}
				// Ctrl+V and Ctrl+Shift+V both paste plain text: a terminal takes bytes for the
				// remote shell, so there is no styled paste to distinguish (pasting escape codes
				// would be a paste-injection hazard, the one the bracketed-paste strip guards).
				Physical::Code(Code::KeyV) => return self.on_paste(),
				_ => {}
			}
		}

		// Shift + PageUp / PageDown page through the shell's own scrollback, and Shift + Home /
		// End jump to its ends, rather than reaching the remote (§23). Shift-guarded so the bare
		// keys still send their CSI sequences to a full-screen program; reached only with the
		// shell focused, since a focused panel has already claimed the arrows and their neighbours.
		if modifiers.shift()
			&& let iced::keyboard::Key::Named(named) = &key
			&& let Some(motion) = scroll_motion(named)
			&& let Some(terminal) = self.terminal.as_mut()
		{
			terminal.scroll(motion);
			return iced::Task::none();
		}

		// A press/repeat with the shell focused: hand it to the encoder and the channel.
		self.forward_to_shell(&key, physical_key, text.as_deref(), modifiers, key_event)
	}

	/// Encode a key event for the focused shell and send it down the channel (§9, §25). Shared by a
	/// press/repeat (the tail of `on_key`) and a release (which only reaches here when the shell
	/// owns the keyboard). Reads the three input modes the encoder needs off the terminal — DECCKM
	/// for the arrow-key form (full-screen apps such as vim/less/nano enable it and then expect the
	/// SS3 arrows), the modifyOtherKeys level, and the active kitty flag set — then snaps the
	/// scrollback to the live bottom whenever the key produced bytes, so a keystroke sent while
	/// scrolled up lands where it echoes, not off-screen above (§23). A release that produces
	/// nothing leaves the viewport where it is. No terminal means no session, so the modes read as
	/// their defaults; this path only runs on the Terminal screen anyway.
	fn forward_to_shell(
		&mut self,
		key: &iced::keyboard::Key,
		physical: iced::keyboard::key::Physical,
		text: Option<&str>,
		modifiers: iced::keyboard::Modifiers,
		event: term::kitty::KeyEvent,
	) -> iced::Task<Message> {
		// modifyOtherKeys is read off the terminal, not the screen view: the engine does not track
		// that mode, so cmote scans the stream for it (§9). DECCKM and the kitty flags, by contrast,
		// the engine does track, so they come off the screen seam (§25).
		let modes = self
			.terminal
			.as_ref()
			.map(|terminal| term::keymap::Modes {
				application_cursor: terminal.screen().application_cursor(),
				modify_other_keys: terminal.modify_other_keys(),
				kitty: terminal.screen().kitty_flags(),
			})
			.unwrap_or_default();

		if let Some(bytes) = term::keymap::encode(key, physical, text, modifiers, modes, event) {
			if let Some(terminal) = self.terminal.as_mut() {
				terminal.scroll(term::ScrollMotion::Bottom);
			}
			self.send_command(SshCommand::Input(bytes));
		}
		iced::Task::none()
	}

	/// Whether the remote shell is the keyboard's target right now (§9, §20). False while a modal
	/// (the disconnect confirmation, a file-collision or upload question, an inline rename) is up or
	/// a side panel holds the focus — in every such case a keystroke belongs to cmote's own UI, not
	/// the session. Used to decide whether a key *release* should reach the shell; a press is routed
	/// by the fuller guard chain in `on_key`, which this mirrors.
	fn shell_owns_keyboard(&self) -> bool {
		!self.confirm_disconnect
			&& self.clash.is_none()
			&& self.upload_clash.is_none()
			&& self.transfer.is_none()
			&& self.explorer.editing().is_none()
			&& self.files.editing().is_none()
			&& matches!(self.focus, Focus::Terminal)
	}

	/// The focus ring (§20): shell, tree, files pane, and round again — skipping whichever
	/// panels are hidden, since a stop you cannot see is a dead press of Ctrl+Tab. The
	/// shell is always in the ring; it is the one thing always on this screen.
	fn cycle_focus(&mut self, backwards: bool) {
		let mut ring = vec![Focus::Terminal];
		if self.explorer.visible() {
			ring.push(Focus::Tree);
		}
		if self.files.visible() {
			ring.push(Focus::Files);
		}
		let at = ring
			.iter()
			.position(|stop| *stop == self.focus)
			.unwrap_or(0);
		// Backwards is a forward step of len-1, which keeps the wrap-around in one place.
		let step = if backwards { ring.len() - 1 } else { 1 };
		self.set_focus(ring[(at + step) % ring.len()]);
	}

	/// Give the keyboard to a panel because it was clicked (§20). Also closes the OTHER
	/// panel's context menu — clicking into a panel is as much a click-away from the menu
	/// next door as clicking the grid is.
	fn focus_pane(&mut self, focus: Focus) {
		self.set_focus(focus);
		self.menu = None;
	}

	/// Move cmote's keyboard ring to `focus`, the single funnel for every internal focus move
	/// (§20, §23). Routing them all through here means focus reporting sees each one: a switch
	/// off the shell to a panel reads as the shell losing focus, and back as regaining it. Only
	/// a live-session move belongs here — the lifecycle reset in `clear_grid_interaction` sets
	/// the field straight, since a session opening or closing is not a focus event to report.
	fn set_focus(&mut self, focus: Focus) {
		self.focus = focus;
		self.report_focus();
	}

	/// The OS window gained or lost focus (§23). Remember it and let the remote know if it
	/// asked: the shell is focused only while the window is AND the ring is on it, so window
	/// focus and every pane switch feed the one reporter.
	fn on_window_focus(&mut self, focused: bool) {
		self.window_focused = focused;
		self.report_focus();
	}

	/// Tell the remote the shell gained (`CSI I`) or lost (`CSI O`) focus, when the state it
	/// asked to hear about actually flips (focus reporting, DECSET 1004, §23). The shell counts
	/// as focused only while the OS window is focused AND cmote's keyboard ring is on the
	/// terminal — so alt-tabbing away and switching to a side panel both read as a focus-out,
	/// per the reading that the remote, blind to cmote's panels, should hear about either.
	///
	/// Silent unless a shell is live and the program turned reporting on. The last reported
	/// state is kept so only transitions reach the wire — a steady state is never re-sent, and
	/// a program merely enabling the mode hears nothing until focus moves. Because this also
	/// runs after each chunk of shell output, a program that toggles `?1004` mid-session is
	/// reconciled to the true state on its next output rather than left believing the wrong one.
	fn report_focus(&mut self) {
		let Some(terminal) = self.terminal.as_ref() else {
			return;
		};
		if !terminal.screen().focus_reporting() {
			return;
		}
		let focused = self.window_focused && self.focus == Focus::Terminal;
		if focused == self.shell_focus_reported {
			return;
		}
		self.shell_focus_reported = focused;
		let report: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
		self.send_command(SshCommand::Input(report.to_vec()));
	}

	/// Keys while the folder tree has the focus (§20). Up/Down walk the visible rows,
	/// Right opens a folder and Left shuts it, Tab/Shift+Tab step like the arrows, Enter
	/// sends the shell there, F2 renames, and Esc hands the keyboard back to the shell.
	fn on_tree_key(&mut self, key: &iced::keyboard::Key) -> iced::Task<Message> {
		use iced::keyboard::key::Named;
		let iced::keyboard::Key::Named(named) = key else {
			return iced::Task::none();
		};

		let step = match named {
			Named::ArrowDown | Named::Tab => 1,
			Named::ArrowUp => -1,
			Named::ArrowRight => {
				// Open the folder — the same call the row click makes, so a folder never
				// listed is fetched here too.
				if let Some(path) = self.explorer.selected().map(str::to_owned)
					&& let Some(fetch) = self.explorer.expand(&path, false)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				return iced::Task::none();
			}
			Named::ArrowLeft => {
				if let Some(path) = self.explorer.selected().map(str::to_owned) {
					self.explorer.collapse(&path);
				}
				return iced::Task::none();
			}
			Named::Enter => {
				let Some(path) = self.explorer.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::Cd(path));
			}
			Named::F2 => {
				let Some(path) = self.explorer.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::RenameStarted(path));
			}
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		self.explorer.step(step);
		self.scroll_tree_into_view()
	}

	/// Keys while the files pane has the focus (§20). Left/Right step one cell and Up/Down
	/// a whole row — the grid wraps at the window's width, so how many cells that is comes
	/// from the same arithmetic the layout uses. Tab/Shift+Tab are next/previous, Enter
	/// opens a folder, F2 renames, and Esc hands the keyboard back to the shell.
	fn on_files_key(
		&mut self,
		key: &iced::keyboard::Key,
		modifiers: iced::keyboard::Modifiers,
	) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		// Ctrl+A takes the whole listing (§21). Checked before the named-key gate below,
		// since it is the pane's only shortcut on a character key.
		if modifiers.control()
			&& matches!(key, iced::keyboard::Key::Character(character)
				if character.as_str().eq_ignore_ascii_case("a"))
		{
			self.files.select_all(self.explorer.show_hidden());
			return iced::Task::none();
		}

		let iced::keyboard::Key::Named(named) = key else {
			return iced::Task::none();
		};

		let columns = ui::files::columns(self.window_size.width) as isize;
		// Shift held on an arrow extends the selection instead of moving it (§21). Not on
		// Tab: there, Shift already means "the other way".
		let extend = modifiers.shift();
		let (step, extend) = match named {
			Named::ArrowRight => (1, extend),
			Named::ArrowLeft => (-1, extend),
			Named::ArrowDown => (columns, extend),
			Named::ArrowUp => (-columns, extend),
			Named::Tab if modifiers.shift() => (-1, false),
			Named::Tab => (1, false),
			Named::Enter => {
				let Some(path) = self.files.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				// Straight through the double-click's own handler, which is where "only a
				// directory can be entered" is decided.
				return self.on_files(FilesMessage::EntryOpened(path));
			}
			Named::F2 => {
				let Some(path) = self.files.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_files(FilesMessage::RenameStarted(path));
			}
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		self.files.step(self.explorer.show_hidden(), step, extend);
		self.resolve_selected_link();
		// Only the keyboard scrolls: a click is already on a cell the user can see, and
		// scrolling under their cursor would move the thing they just aimed at.
		self.scroll_files_into_view()
	}

	/// Select whatever the rubber band now covers (§21). The grid's geometry belongs to the
	/// view, so the band is turned into cell indices there and back into paths here — the
	/// same split the arrow keys already use.
	fn apply_band(&mut self) {
		let Some(rect) = self.files.band().map(files::Band::rect) else {
			return;
		};
		let Some(directory) = self.files.path().map(str::to_owned) else {
			return;
		};
		let rows = self.files.rows(self.explorer.show_hidden());
		let paths: Vec<String> = ui::files::band_hits(
			rect,
			ui::files::columns(self.window_size.width),
			rows.len(),
			self.files.scroll(),
		)
		.into_iter()
		.filter_map(|index| Some(explorer::join(&directory, &rows.get(index)?.name)))
		.collect();
		self.files.set_band_selection(paths);
	}

	/// Which entries a context-menu item acts on (§21): the whole selection when the menu
	/// was opened on part of it, that one entry otherwise. In grid order, since that is the
	/// order a list of copied names should come out in.
	fn action_targets(&self, path: &str) -> Vec<String> {
		if self.files.selected_count() > 1 && self.files.is_selected(path) {
			self.files
				.selected_rows(self.explorer.show_hidden())
				.into_iter()
				.map(|(path, _)| path)
				.collect()
		} else {
			vec![path.to_owned()]
		}
	}

	/// Ask the server where the selected entry points, when it is a symlink (§20) — the
	/// details popup shows a link's target, and only the server can resolve it.
	///
	/// One `readlink` per *selected* link, not one per link in the listing: resolving them
	/// all is the round-trip-per-entry cost the pane is built to avoid (§19).
	fn resolve_selected_link(&mut self) {
		if let Some(path) = self.files.cursor().map(str::to_owned)
			&& self.files.kind_of(&path) == Some(files::Kind::Link)
			&& self.files.link_target().is_none()
		{
			self.send_command(SshCommand::ReadLink(path));
		}
	}

	/// Scroll the files pane so the selected cell is on screen (§20). The grid's geometry
	/// is the view's (`ui::files`), so the same arithmetic that lays the cells out is what
	/// works out where the selected one sits. The model is told the new offset as well as
	/// the widget, because the details popup is placed against it on this very frame.
	fn scroll_files_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.files.selected_index(self.explorer.show_hidden()) else {
			return iced::Task::none();
		};
		let row = index / ui::files::columns(self.window_size.width);
		let offset = keep_visible(
			self.files.scroll(),
			ui::files::grid_height(&self.files),
			ui::files::row_top(row),
			ui::files::CELL_HEIGHT,
		);
		self.files.set_scroll(offset);
		iced::widget::operation::scroll_to(
			ui::files::GRID_ID,
			iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
		)
	}

	/// The same, for the folder tree — one fixed-height row rather than a wrapping grid.
	fn scroll_tree_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.explorer.selected_index() else {
			return iced::Task::none();
		};
		let offset = keep_visible(
			self.explorer.scroll(),
			ui::explorer::tree_height(
				self.window_size.height,
				self.files.reserved(),
				self.files.path(),
				self.explorer.width(),
			),
			index as f32 * ui::explorer::ROW_HEIGHT,
			ui::explorer::ROW_HEIGHT,
		);
		self.explorer.set_scroll(offset);
		iced::widget::operation::scroll_to(
			ui::explorer::TREE_ID,
			iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
		)
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
		// A click on the grid is also how the keyboard comes back to the shell (§20).
		self.set_focus(Focus::Terminal);
		if self.terminal.is_none() {
			return;
		}
		// Ctrl+click follows an OSC 8 hyperlink instead of selecting (§24): the modifier is
		// what most terminals use, and it keeps a plain click free to select the link's text.
		// A cell with no link falls through to the ordinary selection, so Ctrl+click on
		// unlinked text still just selects.
		if self.modifiers.control()
			&& let Some(uri) = self.link_at(self.hover_cell)
		{
			self.follow_link(&uri);
			return;
		}
		self.selection = Some(ui::selection::Selection::new(self.hover_cell));
		self.selecting = true;
	}

	/// The URI of the OSC 8 hyperlink on a grid cell, if any (§24). `None` with no session,
	/// an out-of-bounds cell, or a cell that is not part of a link. Returned owned so the
	/// short-lived screen borrow is dropped before the caller acts on it.
	fn link_at(&self, cell: ui::selection::Cell) -> Option<String> {
		self.terminal
			.as_ref()?
			.screen()
			.cell(cell.row, cell.col)?
			.hyperlink()
			.map(str::to_owned)
	}

	/// Open an OSC 8 hyperlink (§24), or note it when its scheme is refused. Web and mail
	/// links open in the OS's default browser; anything else is blocked with a toast, since
	/// the URI is the remote's to choose (`link::open` is the policy). Shared by Ctrl+click
	/// and the context menu's "Open link".
	fn follow_link(&mut self, uri: &str) {
		if !link::open(uri) {
			self.snackbar = Some(Snackbar {
				message: "Link blocked — cmote opens only http, https and mailto.".to_owned(),
				shown_at: std::time::Instant::now(),
			});
		}
	}

	/// Forward a pointer report to a full-screen program that asked for the mouse (§9).
	/// The grid widget has already decided the event is the program's — it encodes and
	/// captures it, so nothing here competes with the local selection. A click into such a
	/// program is still a click into the shell, so it takes the keyboard the way a click on
	/// the grid does (§20) and dismisses any menu left open.
	fn on_mouse_report(&mut self, bytes: Vec<u8>) {
		self.menu = None;
		self.set_focus(Focus::Terminal);
		self.send_command(SshCommand::Input(bytes));
	}

	/// Scroll the shell's own scrollback by the wheel (§23). Positive lines move up into
	/// history, negative back toward the live bottom; the grid reads the new offset next frame.
	/// A missing terminal (no session) is a no-op, as is any scroll on the alternate screen —
	/// there the engine keeps no history, so the motion clamps to nothing. Scrolling is a purely
	/// local view change: nothing is sent to the remote, and the focus is left where it is.
	fn on_terminal_scroll(&mut self, lines: i32) {
		if let Some(terminal) = self.terminal.as_mut() {
			terminal.scroll(term::ScrollMotion::Lines(lines));
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
		self.copy_to_clipboard(text)
	}

	/// Copy the current selection to the clipboard as styled HTML with a plain-text fallback
	/// (§10). The HTML carries each cell's colour and attributes, so a paste into a rich editor
	/// keeps the terminal's look; the plain text rides alongside for editors — and the shell
	/// itself — that read only text. Bound to Ctrl+C (with a selection) and the context menu's
	/// Copy. If the rich write fails (the OS clipboard was briefly held by another app), it falls
	/// back to iced's plain-text write so a copy is never silently lost.
	fn on_copy_rich(&mut self) -> iced::Task<Message> {
		self.menu = None;
		let (Some(selection), Some(terminal)) = (self.selection, self.terminal.as_ref()) else {
			return iced::Task::none();
		};
		let plain = selection.extract(terminal.screen());
		if plain.is_empty() {
			return iced::Task::none();
		}
		let html = crate::ui::richcopy::to_html(&selection, terminal.screen());

		self.snackbar = Some(Snackbar {
			message: "Copied to clipboard.".to_owned(),
			shown_at: std::time::Instant::now(),
		});

		// A fresh arboard handle per copy writes the HTML and its plain-text alternate together,
		// and holds no clipboard open between copies (a held clipboard would block other apps). On
		// any error, fall back to a plain-text write so the copy still lands on the clipboard.
		let written = arboard::Clipboard::new()
			.and_then(|mut clipboard| clipboard.set_html(html, Some(plain.clone())));
		match written {
			Ok(()) => iced::Task::none(),
			Err(_) => iced::clipboard::write(plain),
		}
	}

	/// Put `text` on the system clipboard and raise the copy-confirmation toast (§10).
	/// Every copy action funnels through here, so the confirmation and the write can never
	/// drift apart, and each copy resets the dwell by stamping the toast afresh.
	fn copy_to_clipboard(&mut self, text: String) -> iced::Task<Message> {
		self.snackbar = Some(Snackbar {
			message: "Copied to clipboard.".to_owned(),
			shown_at: std::time::Instant::now(),
		});
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
		let Some(text) = text else {
			return;
		};
		let Some(terminal) = self.terminal.as_ref() else {
			return;
		};
		let bracketed = terminal.screen().bracketed_paste();
		let bytes = term::keymap::encode_paste(&text, bracketed);
		// A paste is input too, so it returns the view to the live bottom the way a keystroke
		// does (§23) — the pasted text lands where it echoes, not above a scrolled-up viewport.
		if let Some(terminal) = self.terminal.as_mut() {
			terminal.scroll(term::ScrollMotion::Bottom);
		}
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
		self.transfer = None;
		self.upload_files.clear();
		self.upload_dir.clear();
		self.uploads.clear();
		self.uploaded = 0;
		self.upload_overwrite = false;
		self.upload_clash = None;
		self.transfer_notice = None;
		// A queued batch belongs to the session that asked for it (§17, §21).
		self.downloads.clear();
		self.downloaded = 0;
		self.clash = None;
		// Every session starts with the keyboard at the shell (§20), and none is mid-resume:
		// a torn-down session has nothing to settle, and a fresh one sets this itself once it
		// knows whether it has a shell directory to replay (§22). Set straight rather than
		// through `set_focus`: opening or closing a session is not a focus move to report, and
		// the new session's remote starts out believing the shell is focused (§23), so the
		// reported baseline is reset to match — the window's own focus is left as it is.
		self.focus = Focus::Terminal;
		self.shell_focus_reported = true;
		self.resume_cwd = None;
		// The panels' own size and visibility are user preferences, not session state,
		// so `reset` deliberately leaves those alone.
		self.explorer.reset();
		self.files.reset();
	}

	/// Open the upload confirmation for the picked batch (§17): list the files in the body,
	/// show the destination folder in the editable field, and focus it so the folder can be
	/// corrected — or the batch confirmed with Enter — without reaching for the mouse. No-op
	/// with nothing picked, and refused while another transfer is running, since the status
	/// bar has one progress bar and two transfers would fight over it.
	fn open_upload_confirm(&mut self) -> iced::Task<Message> {
		self.menu = None;
		if self.upload_files.is_empty() {
			return iced::Task::none();
		}
		if self.transfer.is_some() || !self.uploads.is_empty() {
			self.transfer_notice = Some("A transfer is already running.".to_owned());
			return iced::Task::none();
		}
		let names: Vec<String> = self
			.upload_files
			.iter()
			.map(|local| file_name_of(local).to_owned())
			.collect();
		let body = format!(
			"{}\n\n{}",
			ui::terminal::UPLOAD_DIALOG_BODY,
			names.join("\n")
		);
		self.set_dialog_body(&body);
		self.transfer = Some(TransferState::ConfirmPath);
		iced::widget::operation::focus(ui::terminal::UPLOAD_INPUT_ID)
	}

	/// The destination folder was confirmed (§17): pre-scan the server for names already in
	/// it, so the "some are already there" question is asked once for the whole batch before
	/// a single byte is sent. An empty folder normalises to `.` — the login directory — so a
	/// shell that never announced its cwd still has somewhere to send to. The confirmation
	/// closes while the scan runs; `UploadPrescan` reopens as either the collision question
	/// or the transfer itself.
	fn on_upload_confirmed(&mut self) -> iced::Task<Message> {
		if self.upload_files.is_empty() {
			self.cancel_upload();
			return iced::Task::none();
		}
		let dir = self.upload_dir.trim();
		// A relative `.` resolves against the login directory server-side, and `join` keeps
		// it in front rather than turning a bare name into an absolute `/name`.
		self.upload_dir = if dir.is_empty() {
			".".to_owned()
		} else {
			dir.to_owned()
		};
		let names: Vec<String> = self
			.upload_files
			.iter()
			.map(|local| file_name_of(local).to_owned())
			.collect();
		self.transfer = None;
		self.transfer_notice = Some("Checking the destination…".to_owned());
		if !self.send_command(SshCommand::CheckUploads {
			dir: self.upload_dir.clone(),
			names,
		}) {
			self.cancel_upload();
		}
		iced::Task::none()
	}

	/// The batch pre-scan came back (§17). Nothing clashing → queue every file and start
	/// sending. Some clashing → hold the batch on the collision question, the names it found
	/// listed in the (shared) dialog body. A batch cancelled while the scan was in flight
	/// leaves nothing to do.
	fn on_upload_prescan(&mut self, collisions: Vec<(String, String)>) {
		self.transfer_notice = None;
		if self.upload_files.is_empty() {
			return;
		}
		if collisions.is_empty() {
			// The choice is irrelevant when nothing collides — every file writes to its own
			// free name — so `Skip` (which touches only clashing names) does for all of them.
			self.queue_uploads(&[], ClashChoice::Skip);
			return;
		}
		let names: Vec<String> = collisions.iter().map(|(name, _)| name.clone()).collect();
		self.set_dialog_body(&format!(
			"{}\n\n{}",
			ui::terminal::UPLOAD_CLASH_BODY,
			names.join("\n")
		));
		self.upload_clash = Some(collisions);
	}

	/// The collision question was answered (§17): build the queue under that choice and start
	/// it, or drop the whole batch on Cancel. `Replace` sends every file with overwrite set;
	/// `Skip` drops the clashing ones; `KeepBoth` sends them to the server-checked `name-1`
	/// path the pre-scan proposed. The non-clashing files always go, whatever the answer.
	fn on_upload_clash(&mut self, choice: ClashChoice) {
		let Some(collisions) = self.upload_clash.take() else {
			return;
		};
		if choice == ClashChoice::Cancel {
			self.cancel_upload();
			return;
		}
		self.queue_uploads(&collisions, choice);
	}

	/// Turn the picked files, the destination folder and the collision answer into the upload
	/// queue (§17), then start it. The mapping is `plan_uploads` (pure, so it is tested on its
	/// own); this only records the batch-wide overwrite flag and pumps the queue, one file at a
	/// time, the way the download side does (§21).
	fn queue_uploads(&mut self, collisions: &[(String, String)], choice: ClashChoice) {
		self.uploads =
			plan_uploads(&self.upload_files, &self.upload_dir, collisions, choice).into();
		self.uploaded = 0;
		self.upload_overwrite = choice == ClashChoice::Replace;
		self.pump_uploads();
		// Every file may have been skipped — a Skip answer to an all-clashing batch — so there
		// is nothing to send and nothing to wait for. Close it out rather than leaving the
		// picked files hanging.
		self.finish_batch_if_drained();
	}

	/// Start the next queued upload if the one transfer slot is free (§17). Called when a
	/// batch begins and again as each file finishes, which is what walks the queue — the
	/// mirror of `pump_downloads` (§21).
	fn pump_uploads(&mut self) {
		if self.transfer.is_some() {
			return;
		}
		if let Some((local, remote)) = self.uploads.pop_front() {
			let total = std::fs::metadata(&local)
				.map(|meta| meta.len())
				.unwrap_or(0);
			if self.send_command(SshCommand::Upload {
				local,
				remote,
				overwrite: self.upload_overwrite,
			}) {
				self.transfer = Some(TransferState::Running { sent: 0, total });
			} else {
				self.transfer = None;
			}
		}
	}

	/// Close a batch once it has fully drained (§17): no transfer running and nothing left in
	/// the queue. Clears the picked files (which disables the Upload button) and the folder,
	/// so a stray click cannot re-send what just landed. The closing notice is set by the
	/// caller that noticed the last file land.
	fn finish_batch_if_drained(&mut self) {
		if self.transfer.is_none() && self.uploads.is_empty() {
			self.finish_batch();
		}
	}

	/// Drop the finished batch's leftovers (§17), keeping whatever notice is showing.
	fn finish_batch(&mut self) {
		self.upload_files.clear();
		self.upload_dir.clear();
		self.uploads.clear();
		self.uploaded = 0;
		self.upload_overwrite = false;
	}

	/// Back out of the upload flow before or during a batch (§17): a cancelled confirmation
	/// or collision question, or Esc. Drops everything pending so nothing is sent; a transfer
	/// already in flight is left to finish, since its bytes are already on the wire.
	fn cancel_upload(&mut self) {
		self.upload_clash = None;
		self.uploads.clear();
		self.uploaded = 0;
		self.upload_overwrite = false;
		self.upload_files.clear();
		self.upload_dir.clear();
		if matches!(self.transfer, Some(TransferState::ConfirmPath)) {
			self.transfer = None;
		}
	}

	/// A snapshot of this session's per-target UI state (§22): where the shell and files pane
	/// are, the `.*` filter, and the two panel sizes. One place names everything worth
	/// remembering — `persist_session` writes it, `restore_session` reads it back — so adding
	/// another value is one field here (and one on `Target`). The shell cwd is `None` on a
	/// server that announces none (§17); `set_session` treats a `None` as "leave it", so a
	/// silent session never erases what an earlier one recorded.
	fn capture_session(&self) -> crate::profiles::SessionState {
		crate::profiles::SessionState {
			terminal_path: self
				.terminal
				.as_ref()
				.and_then(term::Terminal::cwd)
				.map(str::to_owned),
			files_path: self.files.path().map(str::to_owned),
			show_hidden: Some(self.explorer.show_hidden()),
			explorer_width: Some(self.explorer.width()),
			files_height: Some(self.files.height()),
		}
	}

	/// Fold the current session snapshot into the connected target and save (§22). Called at
	/// every teardown of a live session — clean disconnect, remote hangup, error — and again
	/// whenever a remembered value changes mid-session (the `.*` toggle), so a later hard exit
	/// still keeps what was set. Guarded on a live terminal so a connect that failed before a
	/// shell opened writes nothing: `connection` is set at dial time, so it alone would not
	/// tell an aborted attempt from a real session. `set_session` reports whether anything
	/// actually moved, so an unchanged snapshot skips the disk write.
	fn persist_session(&mut self) {
		if self.terminal.is_none() {
			return;
		}
		let Some(endpoint) = self.connection.clone() else {
			return;
		};
		let session = self.capture_session();
		if self.targets.set_session(&endpoint, session)
			&& let Err(error) = self.targets.save()
		{
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Apply a target's remembered session state to the panels before the first listing (§22):
	/// the `.*` filter and the two panel sizes go straight onto the models, and the resume
	/// paths (shell, pane) are handed back for the caller to drive the `cd` / pane / tree
	/// restore — coordination that belongs in `update`, not here. Each size is clamped to the
	/// same window fraction a splitter drag is, and applied only once the window size is known,
	/// so a restore before the first resize event cannot shrink a panel to its minimum.
	fn restore_session(
		&mut self,
		session: crate::profiles::SessionState,
	) -> (Option<String>, Option<String>) {
		if let Some(show_hidden) = session.show_hidden {
			self.explorer.set_hidden(show_hidden);
		}
		if let Some(width) = session.explorer_width
			&& self.window_size.width > 1.0
		{
			self.explorer
				.set_width(width, self.window_size.width * MAX_PANEL_FRACTION);
		}
		if let Some(height) = session.files_height
			&& self.window_size.height > 1.0
		{
			self.files
				.set_height(height, self.window_size.height * MAX_PANEL_FRACTION);
		}
		(session.terminal_path, session.files_path)
	}

	/// Handle one event from the remote folder tree (§18). The model decides what the
	/// action means; this only relays the network side of it — the listings it asks for,
	/// the `cd` it types into the shell, the clipboard writes — and refits the grid when
	/// the panel's footprint changes.
	fn on_explorer(&mut self, message: ExplorerMessage) -> iced::Task<Message> {
		match message {
			ExplorerMessage::Toggled => {
				self.explorer.toggle();
				// A hidden panel cannot hold the keyboard: hand it back to the shell (§20).
				if !self.explorer.visible() && self.focus == Focus::Tree {
					self.set_focus(Focus::Terminal);
				}
				// The panel's width just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new column count.
				self.refit_grid();
			}
			ExplorerMessage::HiddenToggled => {
				self.explorer.toggle_hidden();
				// Persist the flip now (§14, §22): the toggle folds into the same per-target
				// snapshot as the paths and panel sizes, so it survives even a later hard exit.
				self.persist_session();
			}
			ExplorerMessage::PanelPressed => self.focus_pane(Focus::Tree),
			ExplorerMessage::Scrolled(offset) => self.explorer.set_scroll(offset),
			ExplorerMessage::RowClicked(path) => {
				self.focus_pane(Focus::Tree);
				if let Some(fetch) = self.explorer.toggle_node(&path) {
					self.send_command(SshCommand::ListDir(fetch));
				}
				// Clicking a folder in the tree also points the files pane at it, WITHOUT
				// moving the shell — that is what makes the pane usable to look inside a
				// folder you are not in (§19).
				if let Some(request) = self.files.show(&path) {
					self.list_files(request);
				}
			}
			ExplorerMessage::RowRightClicked(path) => {
				self.focus_pane(Focus::Tree);
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
				// The tree's "Open in terminal" and its Enter key: a deliberate console move,
				// quoted so a folder name carrying a quote stays one argument (§18). The pane
				// then follows the `cd` it can see, the same as any other console move.
				self.explorer.close_menu();
				self.move_shell_to(&path);
			}
			ExplorerMessage::UploadHere(path) => {
				// The tree's "Upload…": pick local files to send into this folder (§17),
				// whichever directory the shell itself is in.
				self.explorer.close_menu();
				return browse_upload_into(path);
			}
			ExplorerMessage::UploadFolderHere(path) => {
				// The tree's "Upload folder…": pick a local folder to send whole into this one (§17).
				self.explorer.close_menu();
				return browse_upload_folder_into(path);
			}
			ExplorerMessage::NewFolderHere(path) => {
				// The tree's "New folder…": create a subfolder inside the right-clicked one (§18).
				self.explorer.close_menu();
				return self.begin_new_folder(path);
			}
			ExplorerMessage::DeleteStarted(path) => {
				// The tree's "Delete…": remove this folder and its whole subtree, once confirmed (§18).
				self.explorer.close_menu();
				self.begin_delete(vec![path]);
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
				let text = explorer::name(&path).to_owned();
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyRelative(path) => {
				self.explorer.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				let text = explorer::relative(cwd, &path);
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyPath(path) => {
				self.explorer.close_menu();
				return self.copy_to_clipboard(path);
			}
			ExplorerMessage::CopyCurrentPath => {
				// The header path, not a tree selection: copy the one directory the header
				// names — the files view's — verbatim, the twin of the pane's own button.
				if let Some(path) = self.files.path() {
					let text = path.to_owned();
					return self.copy_to_clipboard(text);
				}
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

	/// Type a quoted `cd` into the shell so the console moves to `path` (§19). The single
	/// way cmote moves the console on the user's behalf: the Sync button, the tree's and the
	/// pane's "Open in terminal" items, and the tree's Enter key all land here. Browsing —
	/// a pane double-click, the "up" button, a tree row click — no longer drags the console;
	/// it only ever follows a `cd` it can see (its own, or one of these), which is what keeps
	/// "who moved the console" answerable. An explicit move also ends any reconnect resume
	/// (§22): the pin that held the pane against the shell's login announcements has done its
	/// job, so the pane is free to follow this move and later ones again.
	///
	/// `ponytail:` a POSIX shell is assumed and the line is typed blind — if a full-screen
	/// program (vim, less) is running these bytes go to it instead, since cmote cannot tell a
	/// prompt from an editor. Upgrade path: only offer it between prompts, which the OSC
	/// announcements could mark.
	fn move_shell_to(&mut self, path: &str) {
		self.resume_cwd = None;
		let line = format!("cd {}\r", explorer::shell_quote(path));
		self.send_command(SshCommand::Input(line.into_bytes()));
	}

	/// The status bar's "Sync" button (§19): move the console into the directory the files
	/// pane is showing. Browsing the pane or the tree leaves the console where it is, so the
	/// two drift apart on purpose; this is the deliberate, manual way to bring the console
	/// (and with it the tree and the title, which follow it) to the folder on show. A no-op
	/// with no shell or no directory on show; the button dims in those cases and when the two
	/// already agree, so pressing it always has something to do.
	fn on_sync(&mut self) {
		let Some(path) = self.files.path().map(str::to_owned) else {
			return;
		};
		self.move_shell_to(&path);
	}

	/// Browse the files pane into a directory (§19): a double-clicked folder, the toolbar's
	/// "up" button, or Enter on the keyboard. This points the PANE only — the console stays
	/// put, so you can look inside a folder you are not in without disturbing the shell. The
	/// console is moved separately and on purpose, by Sync or "Open in terminal"
	/// (`move_shell_to`); a real `cd` there is what brings the pane back into step, via the
	/// shell-follow (§19 "last one wins").
	fn browse_to(&mut self, path: &str) {
		if let Some(request) = self.files.show(path) {
			self.list_files(request);
		}
	}

	/// Handle one event from the files pane (§19). Same division of labour as the tree's
	/// handler: the model decides what an action means, this relays the network side of
	/// it — the listings, the `cd`, the clipboard writes, the download — and refits the
	/// grid when the pane's footprint changes.
	fn on_files(&mut self, message: FilesMessage) -> iced::Task<Message> {
		match message {
			FilesMessage::Toggled => {
				self.files.toggle();
				// A hidden pane cannot hold the keyboard: hand it back to the shell (§20).
				if !self.files.visible() && self.focus == Focus::Files {
					self.set_focus(Focus::Terminal);
				}
				// The pane's height just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new row count.
				self.refit_grid();
			}
			FilesMessage::PanelPressed => {
				self.focus_pane(Focus::Files);
				// A cell's own `mouse_area` swallows the press that lands on it, so one that
				// reaches the pane missed them all. On the grid that starts a rubber band
				// (§21) — which also clears the selection, as every file manager's empty
				// space does; on the header or the notice line it only clears it.
				let pointer = self.files.pointer();
				let grid = pointer.y >= ui::files::HEADER_HEIGHT
					&& pointer.y <= ui::files::HEADER_HEIGHT + ui::files::grid_height(&self.files);
				if grid {
					self.files.begin_band(pointer, self.modifiers.control());
				} else if !self.modifiers.control() {
					self.files.deselect();
				}
			}
			FilesMessage::PanelReleased => self.files.end_band(),
			FilesMessage::PanelRightPressed => {
				// A right-press that reached the pane missed every cell, so it landed on the
				// empty grid: open the pane's own menu there (§17). The keyboard follows too,
				// as a left-press would.
				self.focus_pane(Focus::Files);
				self.files.open_pane_menu();
			}
			FilesMessage::PaneUploadHere => {
				// "Upload… here": send local files into the directory the pane is showing.
				self.files.close_menu();
				let dir = self.files.path().unwrap_or("").to_owned();
				return browse_upload_into(dir);
			}
			FilesMessage::PaneUploadFolderHere => {
				// "Upload folder… here": send a whole local folder into the directory on show (§17).
				self.files.close_menu();
				let dir = self.files.path().unwrap_or("").to_owned();
				return browse_upload_folder_into(dir);
			}
			FilesMessage::NewFolderHere => {
				// "New folder…": create a folder in the directory the pane is showing (§18).
				self.files.close_menu();
				let dir = self.files.path().unwrap_or("").to_owned();
				return self.begin_new_folder(dir);
			}
			FilesMessage::DeleteStarted(path) => {
				// "Delete…": remove the whole selection once confirmed (§18). A right-click inside
				// the selection kept it; one outside has already collapsed onto the clicked entry.
				self.files.close_menu();
				let targets = self.action_targets(&path);
				self.begin_delete(targets);
			}
			FilesMessage::DownloadFolder(path) => {
				// "Download folder…": recreate this remote directory's tree locally (§19). One
				// transfer at a time, like every other, so a running one blocks it.
				self.files.close_menu();
				if self.transfer.is_some() {
					self.files
						.set_notice("A transfer is already running.".to_owned());
					return iced::Task::none();
				}
				return pick_download_tree_target(path);
			}
			FilesMessage::BandMoved(point) => {
				// Window coordinates from the capture layer: the pane is full width along the
				// bottom of the window, so only the vertical origin has to come off.
				let local = iced::Point::new(
					point.x,
					point.y - (self.window_size.height - self.files.height()),
				);
				self.files.set_pointer(local);
				if self.files.drag_band(local) {
					self.apply_band();
				}
			}
			FilesMessage::Scrolled(offset) => self.files.set_scroll(offset),
			FilesMessage::EntryClicked(path) => {
				self.focus_pane(Focus::Files);
				self.files.close_menu();
				let show_hidden = self.explorer.show_hidden();
				// Shift runs a range from the anchor, Ctrl adds or removes this one, a plain
				// click takes it alone (§21).
				if self.modifiers.shift() {
					self.files.extend_selection(show_hidden, &path);
				} else if self.modifiers.control() {
					self.files.toggle_selection(&path);
				} else {
					self.files.select(&path);
				}
				// A clicked link is resolved the same way a walked-to one is (§20).
				self.resolve_selected_link();
			}
			FilesMessage::EntryOpened(path) => {
				self.files.close_menu();
				// Only a directory can be entered, and entering it browses the PANE there —
				// the console stays put (§19). The console is moved on purpose, by Sync or
				// "Open in terminal", not as a side effect of looking in a folder.
				if self.files.kind_of(&path) != Some(files::Kind::Dir) {
					return iced::Task::none();
				}
				self.browse_to(&path);
			}
			FilesMessage::OpenInTerminal(path) => {
				// The pane's own "Open in terminal": the deliberate console move that a
				// double-click no longer is (§19). Same landing as the tree's item.
				self.files.close_menu();
				self.move_shell_to(&path);
			}
			FilesMessage::ParentOpened => {
				self.files.close_menu();
				// The toolbar disables the button at the root and before the first listing,
				// so this is belt and braces — and the parent is read HERE, from the
				// directory actually on show, rather than carried in the message. Browses the
				// PANE up; the console is left where it is (§19).
				let Some(parent) = self.files.path().and_then(explorer::parent) else {
					return iced::Task::none();
				};
				let parent = parent.to_owned();
				self.browse_to(&parent);
			}
			FilesMessage::EntryRightClicked(path) => {
				self.focus_pane(Focus::Files);
				// A right-click INSIDE the selection keeps it — that is how a menu comes to
				// act on all of it (§21); one outside collapses onto the entry clicked, so
				// the menu never acts on entries the user has looked away from.
				if !self.files.is_selected(&path) {
					self.files.select(&path);
				}
				self.files.open_menu(path);
				self.resolve_selected_link();
			}
			FilesMessage::PointerMoved(point) => {
				self.files.set_pointer(point);
				// A move with the button down is a band being stretched (§21).
				if self.files.drag_band(point) {
					self.apply_band();
				}
			}
			FilesMessage::MenuDismissed => self.files.close_menu(),
			FilesMessage::Refresh => {
				self.files.close_menu();
				if let Some(request) = self.files.refresh() {
					self.list_files(request);
				}
			}
			FilesMessage::CopyName(path) => {
				self.files.close_menu();
				let names = self.action_targets(&path);
				let text = join_lines(names.iter().map(|path| explorer::name(path).to_owned()));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyRelative(path) => {
				self.files.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				let cwd = cwd.to_owned();
				let targets = self.action_targets(&path);
				let text = join_lines(targets.iter().map(|path| explorer::relative(&cwd, path)));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyPath(path) => {
				self.files.close_menu();
				let text = join_lines(self.action_targets(&path));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyCurrentPath => {
				// The header path, not a selection: copy the one directory verbatim, with no
				// `action_targets` detour and no line-joining — there is only ever the one.
				if let Some(path) = self.files.path() {
					let text = path.to_owned();
					return self.copy_to_clipboard(text);
				}
			}
			FilesMessage::CopyDetails(text) => {
				// Already joined in the view (§20): the popup owns the exact lines shown, so
				// this just writes them and raises the shared confirmation toast.
				return self.copy_to_clipboard(text);
			}
			FilesMessage::RenameStarted(path) => {
				self.files.start_rename(path);
				return iced::widget::operation::focus(ui::files::RENAME_INPUT_ID);
			}
			FilesMessage::RenameEdited(text) => self.files.edit_rename(text),
			FilesMessage::RenameCommitted => {
				if let Some((from, to)) = self.files.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			FilesMessage::Download(path) => {
				self.files.close_menu();
				// One transfer at a time — the status bar has one progress bar, and two
				// concurrent transfers would fight over it (§17). A batch respects that by
				// queueing; a batch started while something else runs still has to wait.
				if self.transfer.is_some() {
					self.files
						.set_notice("A transfer is already running.".to_owned());
					return iced::Task::none();
				}
				// Folders are dropped rather than refused: a band that swept up a directory
				// alongside nine files should still fetch the nine (§21).
				let mut targets = self.action_targets(&path);
				targets.retain(|path| self.files.kind_of(path) != Some(files::Kind::Dir));
				return match targets.len() {
					0 => iced::Task::none(),
					// One file keeps the save dialog, which asks its own overwrite question.
					1 => pick_download_target(targets.remove(0)),
					_ => pick_download_folder(targets),
				};
			}
			FilesMessage::SplitterGrabbed => self.files.set_dragging(true),
			FilesMessage::SplitterDragged(pointer) => {
				if self.files.dragging() {
					// The splitter sits at the pane's top edge and the pane runs to the
					// window's bottom edge, so the pointer's distance from that edge IS the
					// height — no drag anchor to track.
					let max = self.window_size.height * MAX_PANEL_FRACTION;
					self.files
						.set_height(self.window_size.height - pointer.y, max);
					self.refit_grid();
				}
			}
			FilesMessage::SplitterReleased => self.files.set_dragging(false),
		}
		iced::Task::none()
	}

	/// Start the download the save dialog just picked a destination for (§19). A
	/// cancelled dialog (`None`) sends nothing. The progress bar starts at zero of an
	/// unknown total; the first progress event from the task fills the real size in.
	fn start_download(&mut self, remote: String, local: Option<PathBuf>) {
		let Some(local) = local else {
			return;
		};
		if self.send_command(SshCommand::Download { remote, local }) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		}
	}

	/// The folder picker for a multi-file download closed (§21). Nothing is written yet:
	/// the local names that are already taken are looked up first, and if there are any the
	/// batch waits on the dialog that asks what to do about them.
	fn on_download_folder(&mut self, remotes: Vec<String>, dir: Option<PathBuf>) {
		let Some(dir) = dir else {
			return;
		};
		let taken: Vec<String> = remotes
			.iter()
			.map(|remote| explorer::name(remote).to_owned())
			.filter(|name| dir.join(name).exists())
			.collect();
		if taken.is_empty() {
			// Nothing to lose: the choice cannot apply to anything, so any of them will do.
			self.queue_downloads(&remotes, &dir, ClashChoice::Skip);
			return;
		}
		self.set_dialog_body(&format!(
			"{}\n\n{}",
			ui::terminal::DOWNLOAD_EXISTS_BODY,
			taken.join("\n")
		));
		self.clash = Some(Clash { remotes, dir });
	}

	/// Turn a picked folder and a batch of remote files into the download queue (§21),
	/// applying the answer to the "already there" question. Only the queue is built here;
	/// `pump_downloads` is what starts them, one at a time.
	fn queue_downloads(&mut self, remotes: &[String], dir: &Path, choice: ClashChoice) {
		self.downloads.clear();
		self.downloaded = 0;
		for remote in remotes {
			let name = explorer::name(remote);
			let local = dir.join(name);
			let local = match choice {
				_ if !local.exists() => local,
				ClashChoice::Replace => local,
				ClashChoice::KeepBoth => free_name(dir, name),
				// Cancel never gets this far — `DownloadClash` drops the batch instead.
				ClashChoice::Skip | ClashChoice::Cancel => continue,
			};
			self.downloads.push_back((remote.clone(), local));
		}
		self.pump_downloads();
	}

	/// Start the next queued download, if the one transfer slot is free (§21). Called when
	/// a batch begins and again as each file finishes, which is what walks the queue.
	fn pump_downloads(&mut self) {
		if self.transfer.is_some() {
			return;
		}
		if let Some((remote, local)) = self.downloads.pop_front() {
			self.start_download(remote, Some(local));
		}
	}

	/// Open the "new folder" dialog for a folder to be created inside `parent` (§18): the tree
	/// folder that was right-clicked, or the directory the files pane is showing. Seeds the body
	/// with what it does and where, then focuses the name field so the user types straight away.
	/// An empty parent (the pane has shown nothing yet) asks nothing.
	fn begin_new_folder(&mut self, parent: String) -> iced::Task<Message> {
		if parent.is_empty() {
			return iced::Task::none();
		}
		self.set_dialog_body(&format!(
			"{}\n\n{parent}",
			ui::terminal::NEW_FOLDER_DIALOG_BODY
		));
		self.new_folder = Some(NewFolder {
			parent,
			name: String::new(),
		});
		iced::widget::operation::focus(ui::terminal::NEW_FOLDER_INPUT_ID)
	}

	/// Ask the server to create the folder the dialog is holding (§18). A blank name, or one
	/// carrying a path separator (which would put the folder somewhere other than asked), is not
	/// submittable — the dialog stays open rather than closing on nothing, the same rule the
	/// inline rename follows. A good name closes the dialog and sends the request.
	fn confirm_new_folder(&mut self) {
		let Some(new_folder) = self.new_folder.as_ref() else {
			return;
		};
		if !explorer::is_plain_name(&new_folder.name) {
			return;
		}
		let path = explorer::join(&new_folder.parent, new_folder.name.trim());
		self.new_folder = None;
		self.send_command(SshCommand::MakeDir(path));
	}

	/// Open the delete confirmation for `paths` (§18): name each target, warn that a folder goes
	/// with everything inside it, and hold the paths until the user confirms. Nothing to delete is
	/// a no-op. Deleting is not undoable, so this only ever raises the question — the removal
	/// happens on an explicit confirm, the same discipline as Disconnect and the home list (§14).
	fn begin_delete(&mut self, paths: Vec<String>) {
		if paths.is_empty() {
			return;
		}
		let names = join_lines(paths.iter().map(|path| explorer::name(path).to_owned()));
		self.set_dialog_body(&format!("{}\n\n{names}", ui::terminal::DELETE_DIALOG_BODY));
		self.pending_delete = Some(paths);
	}

	/// Delete the held entries (§18) — only reached from a confirmed prompt. The panels re-list
	/// when the server reports it done (`on_deleted`), so nothing is dropped from the view on a
	/// hopeful guess.
	fn confirm_remote_delete(&mut self) {
		if let Some(paths) = self.pending_delete.take() {
			self.send_command(SshCommand::Delete(paths));
		}
	}

	/// A recursive transfer's collision prompt was answered (§17, §19): clear the dialog and send
	/// the choice to the transfer parked on it, which resumes — or, on Cancel, winds down and
	/// reports back through the usual terminal event.
	fn on_conflict_resolved(&mut self, choice: bridge::ConflictChoice) {
		self.transfer_conflict = None;
		self.send_command(SshCommand::ResolveConflict(choice));
	}

	/// Start a recursive folder upload the picker chose a source for (§17). A cancelled picker
	/// (`None`) sends nothing; a transfer already running, or a batch still queued, blocks it —
	/// the one progress bar serves them all. The bar starts at an unknown total the first progress
	/// event fills in.
	fn start_upload_tree(&mut self, local: Option<PathBuf>, dir: String) {
		let Some(local) = local else {
			return;
		};
		if self.transfer.is_some() || !self.uploads.is_empty() {
			self.transfer_notice = Some("A transfer is already running.".to_owned());
			return;
		}
		if self.send_command(SshCommand::UploadTree { local, remote: dir }) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		}
	}

	/// Start a recursive folder download the picker chose a destination for (§19). The mirror of
	/// `start_upload_tree`: a cancelled picker sends nothing, a running transfer blocks it.
	fn start_download_tree(&mut self, remote: String, local: Option<PathBuf>) {
		let Some(local) = local else {
			return;
		};
		if self.transfer.is_some() {
			self.files
				.set_notice("A transfer is already running.".to_owned());
			return;
		}
		if self.send_command(SshCommand::DownloadTree { remote, local }) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		}
	}

	/// Re-list a remote directory in whichever panel is showing it (§18): the tree, if it knows
	/// the folder, and the files pane, if that is the directory on show. The refresh a create or a
	/// delete triggers, so a new row appears — or a gone one vanishes — in place.
	fn refresh_remote_dir(&mut self, dir: &str) {
		if let Some(fetch) = self.explorer.refresh_dir(dir) {
			self.send_command(SshCommand::ListDir(fetch));
		}
		if self.files.path() == Some(dir)
			&& let Some(request) = self.files.refresh()
		{
			self.list_files(request);
		}
	}

	/// Entries were deleted (§18): step the files pane out of any folder that is now gone, drop
	/// the deleted subtrees from the tree, and re-list each parent they vanished from so the rows
	/// update in place. Done here rather than in a model because it spans both panels and the
	/// pane's own idea of where it is.
	fn on_deleted(&mut self, paths: Vec<String>) {
		// If the pane sits inside a deleted subtree, move it up to a folder that still exists
		// before anything re-lists — otherwise it would try to list a directory that is gone.
		if let Some(pane) = self.files.path().map(str::to_owned) {
			for deleted in &paths {
				if is_within(&pane, deleted) {
					let up = explorer::parent(deleted)
						.unwrap_or(explorer::ROOT)
						.to_owned();
					self.browse_to(&up);
					break;
				}
			}
		}
		let mut parents: Vec<String> = Vec::new();
		for path in &paths {
			self.explorer.forget(path);
			if let Some(parent) = explorer::parent(path).map(str::to_owned)
				&& !parents.contains(&parent)
			{
				parents.push(parent);
			}
		}
		for parent in parents {
			self.refresh_remote_dir(&parent);
		}
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

	/// Ask the SSH task for the directory the files pane wants (§19). One command per
	/// listing; the batches come back tagged with this same request number.
	fn list_files(&mut self, request: u64) {
		let Some(path) = self.files.path().map(str::to_owned) else {
			return;
		};
		self.send_command(SshCommand::ListFiles { path, request });
	}

	/// The window title (§17). Off-session it is just the app name; with a shell open it
	/// carries the session and — as soon as the shell announces one — the remote working
	/// directory, so the directory is visible without stealing room from the grid.
	fn title(&self) -> String {
		let connected = matches!(self.screen, Screen::Terminal);
		let (true, Some(endpoint)) = (connected, self.connection.as_deref()) else {
			return "cmote".to_owned();
		};
		// The third slot describes what the shell is doing: the remote-set window title if a
		// program set one (§23), otherwise the working directory it announced (§17). The endpoint
		// always stays, so a window is identifiable by host even while a program owns the title.
		// An empty title (a program cleared it) counts as none, so the cwd shows through again.
		let terminal = self.terminal.as_ref();
		let detail = terminal
			.and_then(term::Terminal::title)
			.filter(|title| !title.is_empty())
			.or_else(|| terminal.and_then(term::Terminal::cwd).map(str::to_owned));
		match detail {
			Some(detail) => format!("cmote — {endpoint} — {detail}"),
			None => format!("cmote — {endpoint}"),
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
			Screen::Interactive => self.form_with_dialog(
				ui::interactive_view(
					&self.interactive_prompts,
					&self.interactive_answers,
					&self.dialog_body,
					drag,
				),
				Message::InteractiveCancelled,
			),
			Screen::VaultUnlock => self.form_with_dialog(
				ui::vault_view(
					&self.vault_input,
					&self.vault_confirm,
					self.vault_creating,
					self.vault_failed,
					&self.dialog_body,
					drag,
				),
				Message::VaultCancelled,
			),
			Screen::Terminal => match &self.terminal {
				Some(terminal) => {
					let base = ui::terminal::view(
						terminal,
						self.connection.as_deref().unwrap_or(""),
						self.selection.as_ref(),
						self.menu,
						ui::terminal::Modals {
							confirm_disconnect: self.confirm_disconnect,
							clash: self.clash.is_some(),
							upload_clash: self.upload_clash.is_some(),
							new_folder: self
								.new_folder
								.as_ref()
								.map(|new_folder| new_folder.name.as_str()),
							pending_delete: self.pending_delete.is_some(),
							transfer_conflict: self.transfer_conflict.is_some(),
							body: &self.dialog_body,
							drag,
						},
						ui::terminal::UploadView {
							file_count: self.upload_files.len(),
							first_file: self.upload_files.first().map(|local| file_name_of(local)),
							dest: &self.upload_dir,
							state: self.transfer,
							notice: self.transfer_notice.as_deref(),
						},
						ui::terminal::Panels {
							explorer: &self.explorer,
							files: &self.files,
							focus: self.focus,
							width: self.window_size.width,
						},
					);
					// The copy toast floats over the whole terminal screen as the top layer
					// (§10). It is added only while showing, so the common case pays nothing.
					match &self.snackbar {
						Some(snackbar) => {
							iced::widget::stack![base, ui::snackbar::view(&snackbar.message)]
								.width(iced::Length::Fill)
								.height(iced::Length::Fill)
								.into()
						}
						None => base,
					}
				}
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
		// Track window focus on every screen too, so focus reporting (§23) sees a change that
		// happens while a dialog or the form is up and the shell still reflects it on return.
		let mut subs = vec![ssh, resizes, focus_events()];

		// While a copy toast is up, ride the window's frame clock so it can dismiss itself
		// once its dwell elapses (§10). This build's iced executor is the thread-pool
		// backend, which has no `time::every`; `frames()` needs no async runtime — it
		// listens for the redraws the runtime already keeps requesting while a message is
		// flowing, so the tick self-sustains and stops the moment the toast clears.
		if self.snackbar.is_some() {
			subs.push(iced::window::frames().map(|_instant| Message::SnackbarTick));
		}

		// Per-screen keyboard listeners. On the terminal every key is forwarded to the
		// shell; on the connect form Tab / Shift+Tab move focus (typing still reaches the
		// fields through the widget tree); on the home screen the F2 / Enter / Delete / Esc
		// shortcuts (§14). Other screens need no keyboard subscription.
		match self.screen {
			Screen::Terminal => subs.push(iced::keyboard::listen().map(Message::Key)),
			Screen::Connect => subs.push(iced::keyboard::listen().map(Message::FormKey)),
			Screen::Home => subs.push(iced::keyboard::listen().map(Message::HomeKey)),
			_ => {}
		}

		iced::Subscription::batch(subs)
	}
}

/// Fetch the current window size and turn it into a `WindowResized`, so a newly
/// opened terminal fits the window immediately instead of waiting for the first
/// resize event (§9). `latest()` yields the most-recently-opened window and
/// `and_then` unwraps it — if there is somehow no window, this is a no-op.
fn fit_terminal() -> iced::Task<Message> {
	iced::window::latest().and_then(|id| iced::window::size(id).map(Message::WindowResized))
}

/// Window focus changes, as `Message::WindowFocus(bool)` for focus reporting (§23). iced
/// ships no dedicated focus-event subscription, so this filters the raw event stream down to
/// the two window events that matter and drops the rest — so the shell is not woken on every
/// frame the way subscribing to all window events would.
fn focus_events() -> iced::Subscription<Message> {
	iced::event::listen_with(|event, _status, _window| match event {
		iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocus(true)),
		iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocus(false)),
		_ => None,
	})
}

/// The scrollback motion a Shift+navigation key asks for, or `None` for a key that does not
/// scroll (§23). PageUp/PageDown page through history, Home/End jump to the oldest retained
/// line and back to the live bottom — the xterm shifted-navigation set the terminal owns.
fn scroll_motion(named: &iced::keyboard::key::Named) -> Option<term::ScrollMotion> {
	use iced::keyboard::key::Named;
	match named {
		Named::PageUp => Some(term::ScrollMotion::PageUp),
		Named::PageDown => Some(term::ScrollMotion::PageDown),
		Named::Home => Some(term::ScrollMotion::Top),
		Named::End => Some(term::ScrollMotion::Bottom),
		_ => None,
	}
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

/// Open the native picker for the files to upload (§17), from the status bar's File… button.
/// Multi-select: one file or many, the flow is the same. Same async-`Task` shape as
/// `browse_key` — the dialog is modal and would otherwise block the GUI thread. The
/// destination is chosen afterwards, on the Upload button, from the shell's cwd.
fn browse_upload() -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select files to upload")
			.pick_files(),
		|handles| Message::UploadFilesPicked(handles_to_paths(handles)),
	)
}

/// The same picker, but for an "Upload…" started from a right-click surface (§17): the
/// destination folder — the shell cwd, the files pane's directory, or a tree folder — is
/// already known, so the picked files go straight to the confirmation with it filled in.
fn browse_upload_into(dir: String) -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select files to upload")
			.pick_files(),
		move |handles| Message::UploadFilesPickedInto {
			files: handles_to_paths(handles),
			dir: dir.clone(),
		},
	)
}

/// Flatten the multi-file picker's result into owned paths (§17): a cancelled dialog
/// (`None`) becomes an empty list, which every caller reads as "nothing picked".
fn handles_to_paths(handles: Option<Vec<rfd::FileHandle>>) -> Vec<PathBuf> {
	handles
		.unwrap_or_default()
		.iter()
		.map(|handle| handle.path().to_path_buf())
		.collect()
}

/// Open the native save dialog for a file being downloaded (§19), pre-filled with the
/// remote name. Async, like the other pickers, so the modal dialog never blocks the GUI
/// thread. The dialog is also what asks about replacing an existing local file, which is
/// why `download` itself has no overwrite prompt.
fn pick_download_target(remote: String) -> iced::Task<Message> {
	let name = explorer::name(&remote).to_owned();
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Save the remote file as")
			.set_file_name(name)
			.save_file(),
		move |handle| Message::DownloadTargetPicked {
			remote: remote.clone(),
			local: handle.map(|handle| handle.path().to_path_buf()),
		},
	)
}

/// One clipboard write out of many entries (§21): one per line, which is what a shell, an
/// editor and every other file manager expect a multi-selection paste to be.
fn join_lines(items: impl IntoIterator<Item = String>) -> String {
	items.into_iter().collect::<Vec<_>>().join("\n")
}

/// Open the native folder picker for a multi-file download (§21). One folder for the whole
/// batch: a save dialog per file would be a dialog storm, and the names are the remote
/// ones anyway.
fn pick_download_folder(remotes: Vec<String>) -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Save the remote files into")
			.pick_folder(),
		move |handle| Message::DownloadFolderPicked {
			remotes: remotes.clone(),
			dir: handle.map(|handle| handle.path().to_path_buf()),
		},
	)
}

/// Open the native folder picker for a recursive upload (§17): one local folder to send into the
/// already-known remote destination. Async like the other pickers, so the modal never blocks the
/// GUI thread. The folder keeps its own name inside the destination.
fn browse_upload_folder_into(dir: String) -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select a folder to upload")
			.pick_folder(),
		move |handle| Message::UploadFolderPicked {
			local: handle.map(|handle| handle.path().to_path_buf()),
			dir: dir.clone(),
		},
	)
}

/// Open the native folder picker for a recursive download (§19): where to recreate the remote
/// folder on this machine. The folder keeps its own name inside the picked directory, the mirror
/// of the upload side.
fn pick_download_tree_target(remote: String) -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Save the remote folder into")
			.pick_folder(),
		move |handle| Message::DownloadFolderTargetPicked {
			remote: remote.clone(),
			local: handle.map(|handle| handle.path().to_path_buf()),
		},
	)
}

/// Whether `path` is `ancestor` itself or sits somewhere beneath it (§18) — the test for a files
/// pane showing a directory that a delete just removed. The trailing slash is normalised so `/a`
/// matches `/a/b` but not the unrelated `/ab`.
fn is_within(path: &str, ancestor: &str) -> bool {
	let ancestor = ancestor.trim_end_matches('/');
	path == ancestor || path.starts_with(&format!("{ancestor}/"))
}

/// The first free `name-1.ext`, `name-2.ext`… beside a local name already taken (§21) —
/// the "save alongside" answer to the collision question. Bounded: after a hundred tries
/// the folder is telling us something, and the last candidate is returned rather than
/// spinning. Writing it is the download's problem, not this function's.
fn free_name(dir: &Path, name: &str) -> PathBuf {
	let (stem, extension) = match name.rsplit_once('.') {
		Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
		// A dot-file (`.bashrc`) or a name with no dot at all: the whole thing is the stem.
		_ => (name, String::new()),
	};
	let mut candidate = dir.join(format!("{stem}-1{extension}"));
	for attempt in 2..=100 {
		if !candidate.exists() {
			break;
		}
		candidate = dir.join(format!("{stem}-{attempt}{extension}"));
	}
	candidate
}

/// A path's own file name, which is what the status bar shows and what the remote
/// destination is built from (§17). A path with no final component (a bare root) falls
/// back to a placeholder rather than an empty label.
fn file_name_of(path: &std::path::Path) -> &str {
	path.file_name()
		.and_then(std::ffi::OsStr::to_str)
		.unwrap_or("file")
}

/// The secret a "Remember" tick should persist for this auth method (§16): the password, or a
/// non-empty pre-seeded key passphrase. An empty secret is nothing worth storing, so it maps to
/// `None` — the target flag then stays off and the vault keeps no empty entry. A key relying on
/// the interactive passphrase prompt (§7) has no form secret to capture here, so it is `None`
/// too; remembering a key passphrase means typing it on the form.
fn extract_secret(auth: &bridge::AuthMethod) -> Option<Secret> {
	let secret = match auth {
		bridge::AuthMethod::Password(secret) => secret,
		bridge::AuthMethod::Key {
			passphrase: Some(secret),
			..
		} => secret,
		bridge::AuthMethod::Key {
			passphrase: None, ..
		} => return None,
		// Interactive auth carries no secret — every factor is answered live (§7) — so there is
		// nothing to remember.
		bridge::AuthMethod::Interactive => return None,
	};
	if secret.expose().is_empty() {
		None
	} else {
		Some(secret.clone())
	}
}

/// Build an upload batch's queue from the picked files, the destination folder and the
/// answer to the collision question (§17). `collisions` maps a name already in the folder to
/// the free `name-1` path the server pre-scan proposed; a file not in it is free and takes its
/// own name. `Replace` overwrites in place, `KeepBoth` writes to the free path, `Skip` drops
/// the clashing file (`Cancel` never reaches here — the batch is dropped before this). Pure, so
/// the collision logic is tested without an `App` or a server.
fn plan_uploads(
	files: &[PathBuf],
	dir: &str,
	collisions: &[(String, String)],
	choice: ClashChoice,
) -> Vec<(PathBuf, String)> {
	let mut queue = Vec::new();
	for local in files {
		let name = file_name_of(local).to_owned();
		let remote = match collisions.iter().find(|(clash, _)| *clash == name) {
			// Free: its own name in the folder.
			None => explorer::join(dir, &name),
			Some((_, free)) => match choice {
				ClashChoice::Replace => explorer::join(dir, &name),
				ClashChoice::KeepBoth => free.clone(),
				ClashChoice::Skip | ClashChoice::Cancel => continue,
			},
		};
		queue.push((local.clone(), remote));
	}
	queue
}

/// The scroll offset that brings the band `top..top + height` into a `view`-tall window
/// currently scrolled to `offset` (§20) — shared by both panels, since "keep the thing
/// the arrow keys just selected on screen" is the same question for a row and a cell.
///
/// Already visible means *do not move*: a keyboard walk across a screenful of entries
/// should scroll only when it reaches an edge, not re-centre on every press.
fn keep_visible(offset: f32, view: f32, top: f32, height: f32) -> f32 {
	if top < offset {
		top
	} else if top + height > offset + view {
		// Park it against the bottom edge — but never past its own top, or an item taller
		// than the window (a cell in a pane dragged short) would be shown headless.
		(top + height - view).max(0.0).min(top)
	} else {
		offset
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// An `App` with a live emulator and an open command channel, so `send_command` succeeds
	// and its bytes can be read back off `rx`. The window starts focused with the ring on the
	// shell — the baseline a program assumes — so a focus change is measured against it.
	fn app_with_terminal(rx_cap: usize) -> (App, mpsc::Receiver<SshCommand>) {
		let (tx, rx) = mpsc::channel(rx_cap);
		let app = App {
			command_tx: Some(tx),
			terminal: Some(term::Terminal::new(24, 80)),
			window_focused: true,
			shell_focus_reported: true,
			focus: Focus::Terminal,
			..App::default()
		};
		(app, rx)
	}

	// The next input queued for the shell, or `None` if nothing was sent.
	fn next_input(rx: &mut mpsc::Receiver<SshCommand>) -> Option<Vec<u8>> {
		match rx.try_recv() {
			Ok(SshCommand::Input(bytes)) => Some(bytes),
			_ => None,
		}
	}

	/// A program that enabled focus reporting (`?1004`) hears `CSI I` / `CSI O` when the shell
	/// gains or loses focus — from the window losing OS focus AND from the keyboard ring moving
	/// off the shell to a side panel (§23) — and hears each edge only once.
	#[test]
	fn focus_reporting_answers_window_and_pane_changes() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(b"\x1b[?1004h");

		// The window loses, then regains, OS focus.
		app.on_window_focus(false);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[O"[..]));
		app.on_window_focus(true);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[I"[..]));

		// The keyboard ring moving off the shell to a side panel is a focus-out to the remote,
		// which knows nothing of cmote's panels.
		app.set_focus(Focus::Files);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[O"[..]));

		// Moving between two panels never restores the shell's focus, so nothing more is sent.
		app.set_focus(Focus::Tree);
		assert_eq!(next_input(&mut rx), None);

		// Returning the ring to the shell is the matching focus-in.
		app.set_focus(Focus::Terminal);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[I"[..]));

		// Re-asserting a state it already holds — window focused, ring on the shell — is silent.
		app.on_window_focus(true);
		assert_eq!(next_input(&mut rx), None);
	}

	/// Until a program asks for focus reporting, a focus change is cmote's own business and
	/// nothing reaches the wire (§23).
	#[test]
	fn focus_changes_are_silent_until_the_program_asks() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.on_window_focus(false);
		app.set_focus(Focus::Files);
		assert_eq!(next_input(&mut rx), None);
	}

	// A key-press event for the terminal handler. `text: None` is fine for the named keys these
	// tests use (Enter / PageUp encode from the key itself), and the physical code is non-numpad
	// so it never trips the NumLock special-case in `keymap`.
	fn key_press(
		named: iced::keyboard::key::Named,
		code: iced::keyboard::key::Code,
		modifiers: iced::keyboard::Modifiers,
	) -> iced::keyboard::Event {
		iced::keyboard::Event::KeyPressed {
			key: iced::keyboard::Key::Named(named),
			modified_key: iced::keyboard::Key::Named(named),
			physical_key: iced::keyboard::key::Physical::Code(code),
			location: iced::keyboard::Location::Standard,
			modifiers,
			text: None,
			repeat: false,
		}
	}

	// Forty lines of output over the 24-row screen, so there is history to scroll into.
	fn with_history(app: &mut App) {
		let output: Vec<u8> = (0..40).flat_map(|_| b"x\r\n".to_vec()).collect();
		app.terminal.as_mut().unwrap().process(&output);
	}

	// The current scrollback offset off the live emulator.
	fn offset(app: &App) -> u16 {
		app.terminal.as_ref().unwrap().screen().display_offset()
	}

	/// Typing while scrolled back into history snaps the view to the live bottom, and the key
	/// still reaches the shell (§23) — so what is typed lands where it will be echoed.
	#[test]
	fn typing_returns_the_scrollback_to_the_live_bottom() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = app_with_terminal(16);
		with_history(&mut app);

		app.on_terminal_scroll(5);
		assert!(offset(&app) > 0, "scrolled up into history");

		let _ = app.on_key(key_press(Named::Enter, Code::Enter, Modifiers::empty()));
		assert_eq!(offset(&app), 0, "snapped back to the bottom");
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\r"[..]));
	}

	/// Shift+PageUp scrolls cmote's own scrollback and sends nothing to the remote, while bare
	/// PageUp stays the shell's key and sends its CSI sequence (§23) — the two never collide.
	#[test]
	fn shift_page_up_scrolls_history_while_bare_page_up_reaches_the_shell() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = app_with_terminal(16);
		with_history(&mut app);

		let _ = app.on_key(key_press(Named::PageUp, Code::PageUp, Modifiers::SHIFT));
		assert!(offset(&app) > 0, "the terminal's own scrollback moved");
		assert_eq!(next_input(&mut rx), None, "nothing reached the shell");

		// Bare PageUp is the shell's: it sends the CSI "~" sequence (snapping the view back on
		// the way, since it is a keystroke to the remote).
		let _ = app.on_key(key_press(Named::PageUp, Code::PageUp, Modifiers::empty()));
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[5~"[..]));
		assert_eq!(offset(&app), 0, "typing snapped it back to the bottom");
	}

	#[test]
	fn scrolling_a_selection_into_view_moves_only_at_the_edges() {
		// A 100-tall window over 20-tall rows, scrolled to the top.
		assert_eq!(keep_visible(0.0, 100.0, 40.0, 20.0), 0.0, "already visible");
		// Off the bottom: scroll just far enough that its bottom edge lands on the
		// window's, not far enough to re-centre it.
		assert_eq!(keep_visible(0.0, 100.0, 120.0, 20.0), 40.0);
		// Off the top: its own top becomes the offset.
		assert_eq!(keep_visible(200.0, 100.0, 60.0, 20.0), 60.0);
		// A row taller than the window is shown from its top rather than its bottom.
		assert_eq!(keep_visible(0.0, 30.0, 10.0, 50.0), 10.0);
		assert_eq!(keep_visible(0.0, 30.0, 0.0, 50.0), 0.0);
	}

	/// A reconnect resumes the shell and the pane where the last session left them (§22), and
	/// — crucially — the pane stays on its OWN remembered directory through the shell's
	/// login-then-`cd` announcements, following the shell again only once it has settled. This
	/// walks that whole lifecycle through `on_ssh_event`, the one path that wires the pin.
	#[test]
	fn a_reconnect_resumes_both_paths_and_pins_the_pane_until_the_shell_settles() {
		use crate::ui::connect::AuthKind;

		// A command channel so `send_command` (the `cd` and the listings) succeeds rather
		// than tripping the "worker not ready" error; the receiver is kept alive so the
		// channel stays open.
		let (tx, _rx) = mpsc::channel(64);
		let mut app = App {
			command_tx: Some(tx),
			..App::default()
		};

		// A target connected to before, remembered at a shell directory and a *different*
		// pane directory — the divergent case a tree-click peek leaves behind.
		app.targets
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None);
		app.targets.set_session(
			"u@h:22",
			crate::profiles::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::profiles::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.find("u@h:22").unwrap().clone());

		// One OSC 7 cwd announcement, as the shell emits on each prompt (§17).
		let announce =
			|dir: &str| SshEvent::Output(format!("\x1b]7;file://host{dir}\x07").into_bytes());

		// Connect: the pane opens at its remembered directory, and the shell is set to resume
		// at its own — so the pane is pinned to `/etc` until the shell reaches `/var/log`.
		let _ = app.on_ssh_event(SshEvent::Connected);
		assert!(matches!(app.screen, Screen::Terminal));
		assert_eq!(app.files.path(), Some("/etc"));
		assert_eq!(app.resume_cwd.as_deref(), Some("/var/log"));

		// The login prompt announces the login directory first. The pane must NOT follow it
		// off `/etc` while the resume is still pending.
		let _ = app.on_ssh_event(announce("/home/u"));
		assert_eq!(
			app.files.path(),
			Some("/etc"),
			"pinned through the login prompt"
		);
		assert_eq!(
			app.resume_cwd.as_deref(),
			Some("/var/log"),
			"still settling"
		);

		// The replayed `cd` lands: the shell has settled, so the pin lifts — but the pane is
		// left where the restore put it rather than dragged onto the shell's cwd.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.files.path(), Some("/etc"), "kept, not clobbered");
		assert_eq!(app.resume_cwd, None, "no longer pinned");

		// A real move afterwards follows normally: the pane tracks the shell again.
		let _ = app.on_ssh_event(announce("/var/log/nginx"));
		assert_eq!(
			app.files.path(),
			Some("/var/log/nginx"),
			"following resumed"
		);
	}

	/// Shift+click and Shift+arrow through the app's own handlers (§21) — the model's rules
	/// are tested next door in `files`, but only this path proves the wiring: the modifier
	/// state comes off the keyboard subscription, and a mouse press carries none of its own.
	#[test]
	fn shift_click_and_shift_arrow_reach_the_selection() {
		use iced::keyboard::{Event, Modifiers};

		let mut app = App::default();
		let request = app
			.files
			.show("/home")
			.expect("a new directory needs listing");
		app.files.chunk(
			request,
			["a", "b", "c", "d"]
				.into_iter()
				.map(|name| files::Entry {
					name: name.to_owned(),
					kind: files::Kind::File,
					meta: files::Meta::default(),
				})
				.collect(),
			true,
		);
		let chosen = |app: &App| {
			app.files
				.selected_rows(app.explorer.show_hidden())
				.into_iter()
				.map(|(path, _)| path)
				.collect::<Vec<_>>()
		};

		let _ = app.on_files(FilesMessage::EntryClicked("/home/a".to_owned()));
		assert_eq!(chosen(&app), ["/home/a"]);

		// Shift goes down, then the second click lands: everything between comes with it.
		let _ = app.on_key(Event::ModifiersChanged(Modifiers::SHIFT));
		let _ = app.on_files(FilesMessage::EntryClicked("/home/c".to_owned()));
		assert_eq!(chosen(&app), ["/home/a", "/home/b", "/home/c"]);

		// Still held: the arrow key extends rather than moving.
		let _ = app.on_key(Event::KeyPressed {
			key: iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight),
			modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight),
			physical_key: iced::keyboard::key::Physical::Code(
				iced::keyboard::key::Code::ArrowRight,
			),
			location: iced::keyboard::Location::Standard,
			modifiers: Modifiers::SHIFT,
			text: None,
			repeat: false,
		});
		assert_eq!(chosen(&app), ["/home/a", "/home/b", "/home/c", "/home/d"]);

		// Shift released, plain click: back to one.
		let _ = app.on_key(Event::ModifiersChanged(Modifiers::empty()));
		let _ = app.on_files(FilesMessage::EntryClicked("/home/b".to_owned()));
		assert_eq!(chosen(&app), ["/home/b"]);
	}

	#[test]
	fn an_upload_batch_with_no_collisions_queues_every_file_by_name() {
		// Arrange: two files, an empty collision list — nothing is already there.
		let files = vec![PathBuf::from("/local/a.txt"), PathBuf::from("/local/b.txt")];

		// Act: the choice is irrelevant with no collisions, so any of them plans the same.
		let queue = plan_uploads(&files, "/remote/dir", &[], ClashChoice::Skip);

		// Assert: each file goes to the folder under its own name.
		assert_eq!(
			queue,
			vec![
				(
					PathBuf::from("/local/a.txt"),
					"/remote/dir/a.txt".to_owned()
				),
				(
					PathBuf::from("/local/b.txt"),
					"/remote/dir/b.txt".to_owned()
				),
			]
		);
	}

	#[test]
	fn the_collision_answer_decides_each_clashing_file() {
		// Arrange: three files; `b.txt` already exists, and the server proposed `b-1.txt` for
		// "keep both". `a.txt` is free, so it is unaffected by the answer.
		let files = vec![
			PathBuf::from("/local/a.txt"),
			PathBuf::from("/local/b.txt"),
			PathBuf::from("/local/c.txt"),
		];
		let clashing = [("b.txt".to_owned(), "/remote/dir/b-1.txt".to_owned())];

		// Replace: the clashing file keeps its name (it is overwritten in place).
		assert_eq!(
			plan_uploads(&files, "/remote/dir", &clashing, ClashChoice::Replace),
			vec![
				(
					PathBuf::from("/local/a.txt"),
					"/remote/dir/a.txt".to_owned()
				),
				(
					PathBuf::from("/local/b.txt"),
					"/remote/dir/b.txt".to_owned()
				),
				(
					PathBuf::from("/local/c.txt"),
					"/remote/dir/c.txt".to_owned()
				),
			]
		);

		// Keep both: the clashing file takes the free `-1` path; the others are untouched.
		assert_eq!(
			plan_uploads(&files, "/remote/dir", &clashing, ClashChoice::KeepBoth),
			vec![
				(
					PathBuf::from("/local/a.txt"),
					"/remote/dir/a.txt".to_owned()
				),
				(
					PathBuf::from("/local/b.txt"),
					"/remote/dir/b-1.txt".to_owned()
				),
				(
					PathBuf::from("/local/c.txt"),
					"/remote/dir/c.txt".to_owned()
				),
			]
		);

		// Skip: the clashing file is dropped from the queue; the free ones still go.
		assert_eq!(
			plan_uploads(&files, "/remote/dir", &clashing, ClashChoice::Skip),
			vec![
				(
					PathBuf::from("/local/a.txt"),
					"/remote/dir/a.txt".to_owned()
				),
				(
					PathBuf::from("/local/c.txt"),
					"/remote/dir/c.txt".to_owned()
				),
			]
		);
	}
}

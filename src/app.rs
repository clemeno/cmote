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
//
// Tabs (§26), then splits (§48): the state is THREE layers, and each one was added without
// disturbing the layer below it.
//   * `Tab`    — ONE session's whole state, everything a single-session app once was, with its own
//                `update`/`view`/`title`.
//   * `Region` — one split region of the window: a strip of tabs and which of them is on screen.
//                This is the layer `App` itself used to be, lifted out whole when a window stopped
//                being a single strip.
//   * `App`    — the tree of regions, which one holds the keyboard, the window, and the things
//                there is genuinely one of: the target list, the vault, the id counter, the quit
//                flow. Its `update`/`view`/`subscription` pick the region a message came from,
//                delegate into it, and route each session's SSH events to the tab that owns them
//                wherever that tab now sits.
//
// The routing is worth reading before the rest (§48). Every element a region draws is `map`ped so
// the messages it raises name their own region (`Message::In`), which means an event is applied
// WHERE IT HAPPENED rather than wherever the keyboard is. Without that, the first click into an
// unfocused split would land in the focused split's terminal — the click and the focus change
// arrive as two messages, and the click comes first.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use iced::Element;
use iced::widget::{pane_grid, text, text_editor};
use tokio::sync::mpsc;

use crate::bridge::{self, HostKeyChoice, SshCommand, SshEvent};
use crate::explorer::{self, ExplorerMessage};
use crate::files::{self, FilesMessage};
use crate::link;
use crate::secret::Secret;
use crate::term;
use crate::transfer;
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

/// How long a copy-confirmation toast stays before it clears itself (§10). Long enough to
/// register, short enough not to linger over the shell.
const SNACKBAR_DWELL: std::time::Duration = std::time::Duration::from_secs(3);

/// The safety net on a clean quit (§30): once the user confirms, cmote waits for every live
/// session to report it has disconnected before the process exits, so no remote connection is
/// cut mid-flight. A session that never acknowledges (a wedged transport) must not wedge quit
/// with it, so after this long the app leaves anyway. In practice the drain finishes in
/// milliseconds — a local channel EOF, not a network round-trip — so this bound is never hit.
///
/// `pub(crate)` for one reason: a LOCAL session's teardown holds its own short window open for the shell
/// to leave on its own (§104), and that window has to stay well inside this budget or a quit with a local
/// tab open would always wait for this timeout. `local::session` checks the relationship at compile time
/// rather than restating it in a comment.
pub(crate) const QUIT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How a Windows interpreter says it has no use for a control byte: it echoes it onto its input line,
/// rendered as a caret and a letter (§104). Measured on all three — `pwsh` and `powershell` wrap it in
/// PSReadLine's colour codes, `cmd` prints the two characters bare — and it is what tells "nothing
/// consumed the Ctrl+D" from "a program did, and quit".
///
/// Two characters is a short needle, so it is only ever looked for in the answer to a Ctrl+D cmote itself
/// just sent, and never in output at large.
const EOF_ECHO: &[u8] = b"^D";

/// How many bytes of a shell's answer to a Ctrl+D are examined before the probe gives up (§104).
///
/// It has to cover whatever the shell says BEFORE the echo, and the measured answers are six bytes
/// (`ESC[?25l`, both PowerShells, in a read of its own) and none at all (`cmd`). Sixty-four is an order of
/// magnitude of room for a shell that clears more, or repaints a right-hand prompt, on its way to echoing.
///
/// It is also the window in which a `^D` printed by something else would be mistaken for the echo, so it is
/// deliberately small rather than generous: one round trip's worth of output, not a screenful.
const EOF_ANSWER_CAP: usize = 64;

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
		// Open at the size the last run left the window (§31), or — on a first run, or after a
		// settings file that could not be trusted — wide enough for a full-width 180-column
		// terminal and tall enough to also show the browser strip under it (§18, §19). The tree
		// shares that strip rather than sitting beside the grid, so the fallback reserves height
		// only, and it is derived from the grid metrics so it stays in step with `grid_size`.
		.window(iced::window::Settings {
			size: crate::settings::Settings::load()
				.window_size()
				.unwrap_or_else(|| {
					ui::terminal::window_size(
						INITIAL_COLS,
						INITIAL_ROWS,
						files::DEFAULT_HEIGHT + files::SPLITTER_HEIGHT,
					)
				}),
			..iced::window::Settings::default()
		})
		// Keep the OS window's title-bar × from tearing the process down on its own (§30): with
		// this false the close request arrives as an event instead, so cmote can confirm the quit
		// and disconnect every session cleanly before it exits, rather than dropping them abruptly.
		.exit_on_close_request(false)
		.subscription(App::subscription)
		.run()
}

/// One split region of the window (§48): its own strip of tabs, whichever of them is on screen, and
/// the strip gesture in flight there.
///
/// This is the state that used to sit directly on `App`, because a window WAS a strip of tabs. A
/// split makes several strips, and everything in here is what has to exist once per REGION rather
/// than once per window: the tabs are different tabs, the one on screen is a different tab, and a
/// drag along one strip must not move a chip on another. What stayed on `App` is what there is
/// genuinely one of — the window, the target list, the vault, the id counter, the quit flow.
///
/// Nothing in here knows which region it is. `App` holds the tree and stamps every message a region
/// raises with the region it came from, so a `Region` never has to carry its own name around.
#[derive(Debug)]
struct Region {
	/// The open tabs, in strip order. Never empty: a region whose last tab closes is closed with it
	/// (§48), and the last tab of the LAST region is a quit (§30).
	tabs: Vec<Tab>,
	/// Index into `tabs` of the tab this region shows — and, while the region holds the keyboard,
	/// the tab the keyboard is talking to.
	active: usize,
	/// The order this region's tabs were last activated in (§37), keyed by tab id — the strip's own
	/// left-to-right order says where a tab *sits*, this one says where the user has *been*. Kept up
	/// to date by every path that changes `active`, and consulted when a tab closes so the region
	/// falls back to the tab the user was on before it rather than to a strip neighbour they may
	/// never have opened.
	recent: crate::mru::Mru,
	/// The strip drag in flight (§38), or `None` when the pointer is not moving a tab. Armed by a
	/// press on any chip — the same press that selects it — so a plain click leaves it holding a
	/// grabbed tab with no target, which drops nothing. Per-region since §48: two strips can be
	/// dragged along one after the other, and neither gesture may reach into the other's tabs.
	tab_drag: Option<TabDrag>,
}

/// The application: the window's split regions and the state they share (§26, §48). Each `Region` is
/// a strip of tabs; each `Tab` in one is a whole session (its own screen, terminal, panes and
/// dialogs). `App` owns the tree of regions, which one holds the keyboard, the OS window's size, and
/// the single target list and secret vault every tab's home screen and connect flow act on. Its
/// `update`/`view`/`subscription` pick the region a message came from and delegate into it, route
/// each session's SSH events to the tab that owns them wherever it sits, and draw the frame round it
/// all.
struct App {
	/// The window's split regions and the dividers between them (§48). Never empty: `pane_grid`
	/// refuses to close the last region, and the last tab of the last region raises a quit instead.
	regions: pane_grid::State<Region>,
	/// The region that holds the keyboard (§48). This cannot be read off the layout the way "which
	/// tab is on screen" can, because EVERY region is on screen at once — it is the region last
	/// clicked in, and its strip is tinted so the window says which that is.
	focus: pane_grid::Pane,
	/// The OS window's logical size (§48). Held here rather than derived from a region, because it
	/// is the other way round: `ui::split::regions` divides this up and hands each region's
	/// on-screen tab the box it fills. It is also what the App-level overlay cards are centred and
	/// clamped against, since those float over the whole window, splits and all.
	window: iced::Size,
	/// The next tab id to hand out (§26). Monotonic and never reused, so a closed tab's id can
	/// never collide with a worker still shutting down. App-wide, not per-region, so an id still
	/// names exactly one tab however the window is split (§48).
	next_id: u64,
	/// The one app-wide saved-target list, shared into every tab (§14) — one file on disk.
	targets: Rc<RefCell<crate::targets::Targets>>,
	/// The one app-wide secret vault, shared into every tab (§16) — unlocking it anywhere unlocks
	/// it everywhere.
	vault: Rc<RefCell<Option<crate::vault::Vault>>>,
	/// The id of a live tab whose "×" is waiting on the close confirmation (§26), or `None` when
	/// no confirmation is up. A live session is torn down only once the user confirms.
	pending_close: Option<u64>,
	/// The id of a DIRTY editor tab whose "×" is waiting on the unsaved-changes prompt (§32), or
	/// `None`. Distinct from `pending_close` because its prompt is three-way — Save & close /
	/// Discard / Cancel — where a live session's is two.
	pending_editor_close: Option<u64>,
	/// The app-wide quit flow (§30): `None` in normal use; `Confirming` while the "Quit cmote?"
	/// dialog is up; `Draining` once the user accepts, holding the sessions whose clean disconnect
	/// is still outstanding. Reached from the OS window's × or from closing the last tab.
	quit: Option<QuitPhase>,
	/// The App-level overlay card — the live-tab close confirmation (§26) and the quit dialog
	/// (§30). Unlike a tab's own dialogs, these float over the WHOLE window (every split and strip
	/// included), so their position cannot live on any one tab; it lives here. Their messages
	/// therefore arrive UNWRAPPED, which is what tells them apart from the identically named ones a
	/// tab's own dialog raises inside a region (§48).
	///
	/// It is the very same `ui::dialog::Card` a tab holds. The two used to be two field triples and
	/// two sets of methods that differed only in the box they measured against — this one the OS
	/// window, a tab's its region — so the box is now an argument and the arithmetic is written
	/// once (§10, §26).
	overlay: ui::dialog::Card,
	/// The app-wide layout remembered between runs (§31) — the OS window's size. Held here
	/// rather than per-tab because there is one window whatever tab is on show; updated on
	/// every resize and written to `settings.json` on the way out (`exit_app`).
	settings: crate::settings::Settings,
	/// Where the pointer is in the window, tracked ONLY while the window is split (§48). A divider
	/// double-click has to know where the press landed, and a press on a divider is the one click in
	/// the window that reaches no widget: `pane_grid` swallows it to start its own drag. So the raw
	/// event stream supplies both halves — this position, and the press that reads it — and the
	/// subscription that fills it is asked for only when there is a seam to hit (`divider_events`).
	/// Stale between splits, and harmless: nothing reads it while `regions` holds one region.
	pointer: iced::Point,
	/// The multi-click tally over the window's dividers (§48), the twin of a tab's `clicks` over the
	/// grid (§42) — same counter, and the seam is the target instead of the cell.
	seam_clicks: ui::selection::Clicks<pane_grid::Split>,
	/// The chip menu that is open, `None` when none is (§52). App-wide rather than per region: only
	/// one can be open at a time, and it is drawn over the whole window.
	strip_menu: Option<StripMenu>,
}

/// A tab being dragged along the strip (§38). Both halves are **ids, not strip positions**: the
/// grabbed tab keeps its identity across a reorder, and a tab could in principle close mid-gesture
/// (its own "×", a session ending), which would renumber every position after it. Resolving ids to
/// positions only at the moment of the drop means a stale index can never move the wrong tab.
#[derive(Debug)]
struct TabDrag {
	/// The tab the press grabbed.
	grabbed: u64,
	/// The tab last hovered — the slot the grabbed tab drops into — or `None` while the pointer has
	/// not left the grabbed chip, which is what makes an ordinary click reorder nothing.
	over: Option<u64>,
}

/// Where the app is in the quit flow (§30). Distinct from a single tab's close confirmation
/// (`pending_close`): this one closes ALL tabs and ends the process, so it also has to wait for
/// every remote connection to come down cleanly first.
enum QuitPhase {
	/// The "Quit cmote?" confirmation is on screen, waiting for Cancel or Quit.
	Confirming,
	/// Quit is confirmed and cmote is disconnecting: `pending` is the ids of the live sessions
	/// not yet reported down, and `since` clocks the drain against `QUIT_DRAIN_TIMEOUT` so a
	/// session that never acknowledges cannot hold the process open for ever.
	Draining {
		pending: Vec<u64>,
		since: std::time::Instant,
	},
}

impl Region {
	/// A region holding one tab, which is the only way a region is ever born (§48): the app's very
	/// first one, and each fresh half a split opens.
	fn new(first: Tab) -> Self {
		// The activation order starts with the tab already on screen (§37), so the very first close
		// has a trail to walk back along rather than an empty one.
		let recent = crate::mru::Mru::new(first.id);
		Self {
			tabs: vec![first],
			active: 0,
			recent,
			tab_drag: None,
		}
	}

	/// The tab this region shows (there is always one).
	fn active(&self) -> &Tab {
		&self.tabs[self.active]
	}

	/// The tab this region shows, mutably.
	fn active_mut(&mut self) -> &mut Tab {
		&mut self.tabs[self.active]
	}

	/// The window geometry and focus to stamp onto a tab arriving in this region — the tab on screen
	/// holds the freshest copy of all three (§26).
	///
	/// The `None` arm is not defensive padding: this is called mid-removal, with the region's strip
	/// momentarily empty, and once with a brand-new region whose only tab is being built right now.
	/// Defaults there are corrected by the relayout that follows on the same turn.
	fn carried(&self) -> (iced::Size, bool, iced::keyboard::Modifiers) {
		match self.tabs.get(self.active) {
			Some(tab) => (tab.window_size, tab.window_focused, tab.modifiers),
			None => (
				iced::Size::default(),
				true,
				iced::keyboard::Modifiers::default(),
			),
		}
	}

	/// The pointer moved over the chip at `index` (§38). While a drag is armed that chip becomes the
	/// drop slot; hovering the grabbed chip itself clears the target again, so dragging out and back
	/// leaves the strip alone. With no drag in flight this is a no-op — the strip only reacts to a
	/// hover during a gesture, so ordinary mousing across the chips costs nothing but the message.
	fn hover_tab(&mut self, index: usize) {
		if self.tab_drag.is_none() {
			return;
		}
		// Read the hovered tab's id before touching the drag, so the shared borrow of `tabs` is over
		// before the mutable borrow of `tab_drag` begins.
		let Some(hovered) = self.tabs.get(index).map(|tab| tab.id) else {
			return;
		};
		if let Some(drag) = self.tab_drag.as_mut() {
			drag.over = (hovered != drag.grabbed).then_some(hovered);
		}
	}

	/// The button was released over the strip (§38): move the grabbed tab into the hovered tab's
	/// slot. `remove` + `insert` gives the familiar feel in both directions — dragged right, the tab
	/// lands where the hovered chip was and that chip shuffles left; dragged left, the reverse.
	///
	/// Every early return is a real case, not a guard against the impossible: no drag armed (a
	/// release with nothing grabbed), no target (a plain click), and either id no longer in the strip
	/// (its tab closed mid-gesture — the "×" is inside the chip being dragged).
	///
	/// A drag never leaves the region it started in (§48). The gesture is reported by the chips'
	/// own pointer events, and a chip belongs to exactly one strip, so a pointer that wanders into
	/// another region's strip reports nothing there and the release drops the tab where it was.
	fn drop_tab(&mut self) {
		let Some(drag) = self.tab_drag.take() else {
			return;
		};
		let Some(target) = drag.over else {
			return;
		};
		let Some(from) = self.tabs.iter().position(|tab| tab.id == drag.grabbed) else {
			return;
		};
		let Some(to) = self.tabs.iter().position(|tab| tab.id == target) else {
			return;
		};
		if from == to {
			return;
		}
		// `active` is a strip POSITION, so it has to be re-found after the move. Following the active
		// tab's id rather than assuming it is the grabbed one keeps this right even where the two are
		// not the same tab — a close confirmation, say, can leave another tab on screen.
		let active_id = self.active().id;
		let tab = self.tabs.remove(from);
		self.tabs.insert(to, tab);
		if let Some(index) = self.tabs.iter().position(|tab| tab.id == active_id) {
			self.active = index;
		}
		// Nothing about the window changed — same tab on screen, same strip height — so there is no
		// refit to do. The activation order (§37) is keyed by id, so it is untouched by a reorder.
	}

	/// The strip position a new editor tab for `session` should take (§38): just past that session's
	/// own chip, and past any editor tabs already grouped there — so opening three files in a row
	/// reads left to right in the order they were opened, rather than stacking up backwards.
	///
	/// Only a run of editor tabs belonging to THIS session is skipped, so the group ends at the first
	/// chip that belongs to something else: another session's tab, or an editor the user has dragged
	/// in (§38). A session whose tab has gone — closed while a load was in flight — has nothing to sit
	/// beside, so its editor goes to the end of the strip as it used to.
	fn editor_slot(&self, session: u64) -> usize {
		let Some(parent) = self.tabs.iter().position(|tab| tab.id == session) else {
			return self.tabs.len();
		};
		let mut slot = parent + 1;
		while self
			.tabs
			.get(slot)
			.and_then(Tab::editor)
			.is_some_and(|editor| editor.session == session)
		{
			slot += 1;
		}
		slot
	}
}

impl App {
	/// Construct the initial state and the first `Task`. iced calls this once at startup: load the
	/// shared target list, start with one region holding one home tab, and fetch the window size
	/// right away so a dialog opened before the first resize is still centred (§10, §26, §48).
	fn new() -> (Self, iced::Task<Message>) {
		let targets = Rc::new(RefCell::new(crate::targets::Targets::load()));
		let vault = Rc::new(RefCell::new(None));
		let first = Tab::home(targets.clone(), vault.clone(), 0, iced::Size::default());
		// One region, undivided — which is the whole window until the user asks for a split (§48).
		let (regions, focus) = pane_grid::State::new(Region::new(first));
		let app = Self {
			regions,
			focus,
			// Corrected by the resize fetched below, which is the first thing this task does.
			window: iced::Size::default(),
			next_id: 1,
			targets,
			vault,
			pending_close: None,
			pending_editor_close: None,
			quit: None,
			// Re-seeded to centre each time an overlay opens; the origin is only a placeholder.
			overlay: ui::dialog::Card::default(),
			// The same file `run` sized the window from (§31). Loaded again here — a tiny read
			// that cannot fail — so the app owns a copy to update on resize and save on quit; the
			// first (synthetic) resize event overwrites `window` with the size actually granted.
			settings: crate::settings::Settings::load(),
			// No seam to hit until the window is cut, so nothing reads these until then (§48).
			pointer: iced::Point::ORIGIN,
			seam_clicks: ui::selection::Clicks::default(),
			strip_menu: None,
		};
		let size = iced::window::latest()
			.and_then(|id| iced::window::size(id).map(Message::WindowResized));
		(app, iced::Task::batch([size, install_hand_cursors()]))
	}

	/// The region holding the keyboard (§48). `focus` is kept valid by every path that changes the
	/// tree — a closed region hands the keyboard to the sibling that takes its room — and `pane_grid`
	/// never lets the tree empty, so the fallback exists only so a bug cannot become a panic.
	fn region(&self) -> &Region {
		self.regions
			.get(self.focus)
			.or_else(|| self.regions.iter().next().map(|(_, region)| region))
			.expect("the window always has at least one region")
	}

	/// The pane `focus` names, corrected to a real one if it ever went stale (§48).
	fn focused_pane(&self) -> pane_grid::Pane {
		if self.regions.get(self.focus).is_some() {
			return self.focus;
		}
		self.regions
			.iter()
			.next()
			.map(|(pane, _)| *pane)
			.expect("the window always has at least one region")
	}

	/// The tab on screen in the region holding the keyboard — the one a keystroke is for (§48).
	fn active(&self) -> &Tab {
		self.region().active()
	}

	/// Every open tab in the window, in region then strip order (§48). Used by everything that has
	/// to reach a tab by IDENTITY rather than by position: routing a session's events, counting live
	/// sessions for the quit drain, starting one SSH worker per tab.
	fn tabs(&self) -> impl Iterator<Item = &Tab> {
		self.regions
			.iter()
			.flat_map(|(_, region)| region.tabs.iter())
	}

	/// Every open tab in the window, mutably (§48).
	fn tabs_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
		self.regions
			.iter_mut()
			.flat_map(|(_, region)| region.tabs.iter_mut())
	}

	/// The tab with this id, mutably, wherever in the window it sits (§48).
	fn tab_mut(&mut self, id: u64) -> Option<&mut Tab> {
		self.tabs_mut().find(|tab| tab.id == id)
	}

	/// Where the tab with this id sits: which region, and its position in that region's strip (§48).
	/// Tab ids are app-wide and never reused (§26), so at most one region can answer.
	fn locate(&self, id: u64) -> Option<(pane_grid::Pane, usize)> {
		self.regions.iter().find_map(|(pane, region)| {
			region
				.tabs
				.iter()
				.position(|tab| tab.id == id)
				.map(|index| (*pane, index))
		})
	}

	/// Apply one message, deciding first WHICH REGION it is for (§48).
	///
	/// Three kinds of message arrive here and they are told apart by their shape, not by a flag:
	///
	///  * `Message::In(pane, …)` — raised by one region's own widgets, which `view` stamped with the
	///    region they belong to. It is applied THERE. This is what makes a click in an unfocused
	///    split land where it was aimed: the click and the focus change are two separate messages
	///    and the click arrives first, so routing by focus would spend it on the wrong terminal.
	///  * The App's own — a session's SSH events, the OS window's geometry and focus, the quit flow,
	///    the split gestures, and the overlay cards that float outside every region. These come
	///    unwrapped, and being unwrapped is precisely what tells an overlay card's `DialogGrabbed`
	///    apart from the identical message a tab's own dialog raises inside a region.
	///  * Everything else unwrapped — the keyboard, chiefly, which comes from a subscription and
	///    therefore has no region of its own. It goes to the region holding the keyboard.
	fn update(&mut self, message: Message) -> iced::Task<Message> {
		let task = self.dispatch(message);
		// Mirror the active tab's command progress onto the taskbar button (§54). Done HERE, once,
		// rather than at each of the places that could change it — shell output arriving, a tab
		// switch, a split's focus moving, a tab closing — because that list is long, it would grow
		// with every future feature, and a missed one leaves a stale bar on the taskbar. `show` drops
		// a reading equal to the one already up, so paying for this on every message costs a mutex
		// and a comparison.
		crate::taskbar::show(self.active().command_progress());
		task
	}

	/// Apply one message. `update` wraps this to mirror the taskbar afterwards; everything that
	/// actually decides what a message DOES is here.
	fn dispatch(&mut self, message: Message) -> iced::Task<Message> {
		// The quit confirmation is modal app-wide (§30): while it is up, Esc cancels and Enter
		// confirms, and every other keystroke is swallowed so none reaches the shell beneath it.
		// Non-key messages (button presses, SSH events, ticks, resizes) pass straight through.
		if self.quit.is_some()
			&& let Some(task) = self.quit_key_intercept(&message)
		{
			return task;
		}
		match message {
			// A region's own widgets named their region; honour it (§48).
			Message::In(pane, inner) => self.update_in(pane, *inner),
			// Route a session's event to the tab that owns it — maybe a tab in another region, or a
			// background one, so its shell keeps drawing off-screen. An event for a tab already
			// closed is dropped.
			Message::Ssh(id, event) => self.route_ssh(id, event),
			// A resize is the whole window's, and every region has to be re-measured against it (§48).
			Message::WindowResized(size) => self.on_window_resized(size),
			// The OS window gaining or losing focus is every VISIBLE shell's business, and since §48
			// that is one per region rather than one in total.
			Message::WindowFocus(focused) => self.broadcast(&Message::WindowFocus(focused)),
			// A frame tick is a clock, not a gesture (§10, §44): it belongs to every region that
			// asked for one, not to whichever region happens to hold the keyboard.
			Message::SnackbarTick => self.broadcast(&Message::SnackbarTick),
			Message::TermFindRescan => self.broadcast(&Message::TermFindRescan),
			// This clock reaches further than the two above it (§122). `broadcast` hands a message to
			// each region's ON-SCREEN tab, which is right for a toast and for a find bar — both are
			// things you can see. A held frame is the opposite: the tab you cannot see is the one
			// whose held frame would sit there unnoticed until you came back to it.
			Message::HeldUpdateExpired => self.release_held_updates(),
			// The split gestures (§48). `Split` itself arrives wrapped — it is a strip button, so it
			// names its own region — and is handled in `update_in`; these three do not.
			Message::SplitSized {
				pane,
				way,
				window,
				size,
				seed,
			} => self.apply_split(pane, way, window, size, seed),
			// The chip menu's own items (§52). Raised by an overlay drawn over the whole window, so
			// they arrive unwrapped — the region they act on is the one the menu remembers, not the
			// one holding the keyboard.
			Message::TabMenuDismissed => {
				self.strip_menu = None;
				iced::Task::none()
			}
			Message::TabMoveTo(area) => self.move_tab_to(area),
			Message::TabDuplicateTo(area) => self.duplicate_tab_to(area),
			Message::SplitFocused(pane) => {
				self.focus = pane;
				iced::Task::none()
			}
			Message::SplitResized { split, ratio } => self.on_divider_dragged(split, ratio),
			// The two halves of a divider double-click (§48). Both come off the raw event stream
			// rather than from a widget, because a press on a seam is the one click in the window
			// that reaches no widget at all — `pane_grid` swallows it to start its own drag.
			Message::PointerMoved(pointer) => {
				self.pointer = pointer;
				iced::Task::none()
			}
			Message::PointerPressed => self.on_pointer_pressed(),
			// The overlay dialogs (a live tab's close confirmation, §26; the quit card, §30) float
			// over the WHOLE window, outside every region, so their messages arrive unwrapped and
			// their header drag is the App's to track — the very same DialogGrabbed / DialogDragged /
			// DialogReleased a tab's dialog uses, but caught here so the floating card moves. The
			// guard is belt and braces now that the wrapper distinguishes them: with no overlay up
			// these fall through to the focused region, which drives its own dialogs.
			Message::DialogGrabbed if self.overlay_open() => {
				self.overlay.grab();
				// The card is held: the hand closes and stays closed until the release, wherever
				// the pointer goes meanwhile (§51).
				crate::cursor::set_dragging(true);
				iced::Task::none()
			}
			Message::DialogDragged(pointer) if self.overlay_open() => {
				// Measured against the OS WINDOW, since an overlay floats over the whole of it —
				// the one thing that differs from a tab's identical arm below (§48).
				self.overlay.drag_to(pointer, self.window);
				iced::Task::none()
			}
			Message::DialogReleased if self.overlay_open() => {
				self.overlay.release();
				crate::cursor::set_dragging(false);
				iced::Task::none()
			}
			// Everything else has no region of its own — the keyboard, above all — so it is for the
			// region holding the keyboard.
			other => {
				let pane = self.focused_pane();
				self.update_in(pane, other)
			}
		}
	}

	/// Apply one message that belongs to the region `pane` (§48).
	///
	/// This is the match `update` used to be: tab-strip management, the editor's cross-tab work and
	/// the quit flow are handled here at the App, and everything left is for the tab that region has
	/// on screen. The only difference is that "the active tab" now means "the active tab OF THIS
	/// REGION", so the region is threaded through rather than read from `focus`.
	fn update_in(&mut self, pane: pane_grid::Pane, message: Message) -> iced::Task<Message> {
		match message {
			// Already unwrapped once; a region's widgets never wrap twice, so this cannot arrive.
			Message::In(_, inner) => self.update_in(pane, *inner),
			Message::Ssh(id, event) => self.route_ssh(id, event),
			// The strip's split buttons (§48): cut THIS region, whichever one the button was on.
			Message::Split(way) => self.request_split(pane, way, SplitSeed::Home),
			Message::TabNew => self.open_tab(pane),
			Message::TabSelected(index) => self.grab_tab(pane, index),
			Message::TabDraggedOver(index) => {
				if let Some(region) = self.regions.get_mut(pane) {
					region.hover_tab(index);
				}
				iced::Task::none()
			}
			// Something grabbable has the pointer with nothing in flight (§51): a tab chip, a dialog
			// header. Caught here rather than in the tab because the cursor is the WINDOW's, not any
			// one region's — the answer is the same whichever region raised it, and a card dragged
			// across a split does not change hands halfway.
			Message::GrabEntered(handle) => {
				crate::cursor::hover_entered(handle);
				iced::Task::none()
			}
			Message::GrabExited(handle) => {
				crate::cursor::hover_exited(handle);
				iced::Task::none()
			}
			// A button ON a handle taking or giving back the pointer (§52). Same reasoning as
			// above — the cursor is the window's — and the same place to answer it.
			Message::GrabControlEntered(handle) => {
				crate::cursor::control_entered(handle);
				iced::Task::none()
			}
			Message::GrabControlExited(handle) => {
				crate::cursor::control_exited(handle);
				iced::Task::none()
			}
			Message::TabDropped => {
				if let Some(region) = self.regions.get_mut(pane) {
					region.drop_tab();
				}
				// Released: the hand opens again if the pointer is still on a chip (§51).
				crate::cursor::set_dragging(false);
				iced::Task::none()
			}
			// The pointer left the strip: the gesture is over and nothing moves. Also fires when the
			// pointer merely wanders off the strip with no drag armed, which is a harmless no-op.
			Message::TabDragCancelled => {
				if let Some(region) = self.regions.get_mut(pane) {
					region.tab_drag = None;
				}
				// Off the strip, so no chip can still hold the pointer whatever the enter/exit
				// events added up to — the boundary that heals a count a closed chip left standing
				// (§51).
				crate::cursor::set_dragging(false);
				crate::cursor::hover_reset();
				iced::Task::none()
			}
			// A chip was right-clicked (§52). The strip names the chip, the wrapper names the strip,
			// and between them that is the whole target — nothing is selected, so the menu can act on
			// a tab the user is not looking at without first putting it on screen.
			Message::TabMenuOpened(index) => {
				self.strip_menu = Some(StripMenu { pane, index });
				iced::Task::none()
			}
			Message::TabCloseRequested(id) => self.request_close(id),
			Message::TabCloseConfirmed => self.close_confirmed(),
			Message::TabCloseCancelled => {
				self.pending_close = None;
				iced::Task::none()
			}
			// The in-tab editor (§32). Opening, saving and closing all need cross-tab reach — an
			// editor saves through the session it was opened from — so they are handled here, at the
			// App, not delegated to the tab. In-buffer editing (`Message::Editor`) and the editor
			// shortcuts (`Message::EditorKey`) fall through to the active tab below.
			Message::ViewerOpen { session, path } => self.open_viewer(pane, session, path),
			Message::EditorFlush(id) => self.flush_editor_save(id),
			Message::EditorCloseSave => self.editor_close_save(),
			Message::EditorCloseDiscard => self.editor_close_discard(),
			Message::EditorCloseCancelled => {
				self.pending_editor_close = None;
				iced::Task::none()
			}
			// A picture finished decoding off the thread pool (§121). The tab may have been closed
			// while it ran, or have moved on; `viewer_mut` answering `None`, or a picture that is no
			// longer loading, is the ordinary way that ends — so neither is treated as an error.
			Message::PictureDecoded {
				viewer_id,
				bytes,
				decoded,
			} => {
				if let Some(picture) = self.viewer_mut(viewer_id).and_then(Viewer::picture_mut)
					&& picture.load_progress().is_some()
				{
					match decoded {
						Ok(image) => picture.set_loaded(image, bytes),
						Err(reason) => picture.load_failed(reason),
					}
				}
				iced::Task::none()
			}
			Message::EditorCloseNow(id) => self.force_close(id),
			Message::EditorThemeSelected(theme) => self.set_editor_theme(pane, theme),
			// The quit flow (§30): the OS window's × or the last tab's close raises the request;
			// confirming drains every session cleanly, then the process exits.
			Message::QuitRequested => self.request_quit(),
			Message::QuitConfirmed => self.quit_confirmed(),
			Message::QuitCancelled => self.quit_cancelled(),
			Message::QuitTick => self.quit_tick(),
			// A window resize reaches `update` unwrapped and never gets here; if one ever did, it
			// would still be the whole window's rather than this region's, so it is handled the same
			// way (§48).
			Message::WindowResized(size) => self.on_window_resized(size),
			// Everything else is for the tab this region has on screen. A region that has just gone
			// — its last tab closed while a message for it was still in the queue — has no tab to
			// take it, which is the one case this `Option` covers (§48).
			other => match self.regions.get_mut(pane) {
				Some(region) => region.active_mut().update(other),
				None => iced::Task::none(),
			},
		}
	}

	/// Route one session's event to the tab that owns it, wherever in the window that tab now sits
	/// (§26, §48). Lifted out of `update` so both the wrapped and unwrapped paths share it.
	fn route_ssh(&mut self, id: u64, event: SshEvent) -> iced::Task<Message> {
		// A viewer's load/save reply rides the SESSION's stream (a viewer has no channel of its own)
		// but belongs to the VIEWER tab that asked — an editor or a picture preview — so it is routed
		// there by viewer id (§32, §53).
		if let Some(viewer_id) = event.viewer_target() {
			return match self.tab_mut(viewer_id) {
				Some(tab) => tab.on_viewer_event(event),
				None => iced::Task::none(),
			};
		}
		// A session going down is what a quit drain waits for: note it BEFORE the event is consumed,
		// then let the owning tab do its own clean-up (persist, back to home).
		let ended = matches!(event, SshEvent::Disconnected | SshEvent::Error(_));
		let task = match self.tab_mut(id) {
			Some(tab) => tab.on_ssh_event(event),
			None => iced::Task::none(),
		};
		if ended {
			// Any viewers opened from this session lost the channel they read and write through —
			// tell them so (§32, §53).
			self.orphan_viewers(id);
			// One fewer session to wait on; once the last is down the process exits (§30).
			if let Some(exit) = self.note_drained(id) {
				return exit;
			}
		}
		task
	}

	/// Hand the same message to the on-screen tab of EVERY region (§48).
	///
	/// For the messages that are not gestures and so have no one region they belong to: the OS
	/// window's focus, which every visible shell has to hear about because focus reporting is a
	/// promise made to the program in it (§23), and the frame clocks, which are the toast's dwell
	/// (§10) and the find bar's re-scan (§44) — a region left un-ticked would keep a toast on screen
	/// for ever. Before §48 there was one visible tab and "the active tab" was the whole answer.
	fn broadcast(&mut self, message: &Message) -> iced::Task<Message> {
		let tasks: Vec<iced::Task<Message>> = self
			.regions
			.iter_mut()
			.map(|(_, region)| region.active_mut().update(message.clone()))
			.collect();
		iced::Task::batch(tasks)
	}

	/// A frame tick while some terminal is holding a synchronized update (§122): let go of every
	/// held frame whose 150 ms has run out.
	///
	/// EVERY tab is asked, not just the on-screen ones, which is why this is not a `broadcast`. A
	/// tab in the background is still being fed by its shell (§26), so it can be holding a frame
	/// too — and its held frame is the more dangerous of the two, because nothing on screen hints
	/// that the tab you are about to switch to is showing a screen from a minute ago. The decision
	/// about WHETHER a given frame is due belongs to the terminal itself (`release_held_update`), so
	/// this walk is unconditional and cheap: a terminal holding nothing answers `None`.
	fn release_held_updates(&mut self) -> iced::Task<Message> {
		let tasks: Vec<iced::Task<Message>> =
			self.tabs_mut().map(Tab::release_held_updates).collect();
		iced::Task::batch(tasks)
	}

	/// The OS window was resized (§26, §48): remember it, then re-measure every region against it.
	fn on_window_resized(&mut self, size: iced::Size) -> iced::Task<Message> {
		self.window = size;
		// Remember the whole OS window's size for the next run (§31); saved on the way out.
		self.settings.set_window(size.width, size.height);
		let task = self.relayout();
		// A card dragged before the window shrank could fall off-screen; pull it back into the new
		// bounds so its header stays reachable (§26). Harmless when none is open.
		if self.overlay_open() {
			self.overlay.reflow(size);
		}
		task
	}

	/// Hand every region's on-screen tab the box it now fills (§48).
	///
	/// This is the one place the window's size becomes a terminal's row and column count, and it runs
	/// after anything that can change a region's shape: a window resize, a divider drag, a split, a
	/// region closing. Each tab is given its region MINUS the strip above it, exactly as the single
	/// tab used to be given the window minus the strip — so every layout and pointer coordinate
	/// inside a tab is still measured against the space that tab actually occupies.
	///
	/// A degenerate width or height (a window too small to divide) is floored at zero rather than
	/// passed on negative; `ui::terminal::grid_size` clamps to at least one cell from there, which
	/// is the same thing that already happened to a window dragged down to nothing.
	fn relayout(&mut self) -> iced::Task<Message> {
		let boxes = ui::split::regions(&self.regions, self.window);
		let mut tasks: Vec<iced::Task<Message>> = Vec::with_capacity(boxes.len());
		for (pane, rect) in boxes {
			let inner = iced::Size {
				width: rect.width.max(0.0),
				height: (rect.height - ui::tabs::STRIP_HEIGHT).max(0.0),
			};
			if let Some(region) = self.regions.get_mut(pane) {
				tasks.push(region.active_mut().update(Message::WindowResized(inner)));
			}
		}
		iced::Task::batch(tasks)
	}

	/// Whether the window may be cut right now (§48) — true only while it is whole.
	///
	/// This is one rule serving two purposes, which is why it is a function and not two conditions.
	/// A strip asks it to decide whether to draw the split controls at all, and `apply_split` asks it
	/// again to refuse a cut that got past them. The user's two rules — the controls belong to the
	/// first region, and there is at most one split — meet in the same count: a window with one region
	/// has only the original to offer them from, and a window with two has had its cut.
	///
	/// Counting the tree rather than remembering a flag is what keeps this honest through the other
	/// end of the feature. Closing a region's last tab closes the region (§48), and the window is
	/// whole again the instant that happens — including when it is the ORIGINAL region that goes and
	/// the split one inherits the whole window. It is the top-left region then, and it may split.
	fn splittable(&self) -> bool {
		self.regions.len() == 1
	}

	/// The strip's split button (§48): find out how much screen there is, then cut `pane` in two.
	///
	/// Two steps, because the first thing the split needs is an answer that only arrives
	/// asynchronously — how big the monitor is. Doubling blind would put most of the new region past
	/// the edge of the screen, where there is no way to reach it and, since a region's only handle is
	/// the region itself, no way to drag the divider back either.
	///
	/// A screen that cannot be measured is not a reason to refuse the split; it is a reason not to
	/// clamp against a number we do not have.
	/// `seed` rides along to say what the fresh region opens with: the target list for the strip's
	/// own buttons, or — since §52 — the tab the chip menu is sending across.
	fn request_split(
		&mut self,
		pane: pane_grid::Pane,
		way: ui::split::Way,
		seed: SplitSeed,
	) -> iced::Task<Message> {
		let wanted = way.grown(self.window);
		iced::window::latest().and_then(move |window| {
			iced::window::monitor_size(window).then(move |screen| {
				let size = match screen {
					Some(screen) => iced::Size::new(
						wanted.width.min(screen.width),
						wanted.height.min(screen.height),
					),
					None => wanted,
				};
				iced::Task::done(Message::SplitSized {
					pane,
					way,
					window,
					size,
					seed,
				})
			})
		})
	}

	/// Cut `pane` in two now the window size the split will run at is known (§48).
	///
	/// The fresh region opens on the target list, which is what makes a split useful straight away:
	/// the point of asking for one is almost always to connect somewhere else, and the list is where
	/// that starts. It also takes the keyboard, for the same reason. Since §52 the chip menu can ask
	/// for the same cut with a different `seed`, and then the new region opens holding the tab that
	/// was sent to it — or a fresh copy of it — instead.
	///
	/// The window is asked to grow in the same turn, and the regions are measured against the size it
	/// was ASKED for rather than the size it has. If the OS grants less — the screen was already
	/// full — the resize event that follows corrects every region; if it grants nothing at all (a
	/// window already filling the monitor) there is no event, and measuring against the asked-for
	/// size would have been wrong, which is why `size` is clamped to the screen before it gets here.
	fn apply_split(
		&mut self,
		pane: pane_grid::Pane,
		way: ui::split::Way,
		window: iced::window::Id,
		size: iced::Size,
		seed: SplitSeed,
	) -> iced::Task<Message> {
		// The region may have gone while the monitor was being asked — its last tab closed, which
		// closes the region (§48). Nothing to cut, and the window must not grow for a split that is
		// not going to happen.
		if self.regions.get(pane).is_none() {
			return iced::Task::none();
		}
		// One cut, and only from the undivided window (§48). The strip stops offering the controls the
		// moment a split lands, but that is not enough on its own: the monitor is measured
		// asynchronously, so two quick presses both leave while the window is still whole and the
		// second arrives here to find it is not. Refusing on arrival is what makes the rule hold —
		// checking it in `request_split` would check it before the race, not after.
		if !self.splittable() {
			return iced::Task::none();
		}
		// The new tab inherits the window focus and modifier state from the region being split, so
		// its first paint agrees with the rest of the window. Its SIZE is left at the default on
		// purpose: the region it will live in does not exist yet, and the relayout below is what
		// hands it the box it actually fills.
		let (_, focused, modifiers) = self.region_at(pane).carried();
		// What goes in the fresh region (§52). A move takes the tab out of the region being cut, so
		// it is checked against the same rule the menu greys the entry by: a region cannot be
		// emptied into a split, because the cut and the collapse would cancel out and leave nothing
		// but a resized window. The check is repeated HERE rather than trusted from the menu for the
		// reason the `splittable` one above is — the monitor was measured in between, and a tab can
		// close in that time.
		let mut opening = None;
		let mut tab = match seed {
			SplitSeed::Home => {
				let id = self.next_id;
				self.next_id += 1;
				Tab::home(
					self.targets.clone(),
					self.vault.clone(),
					id,
					iced::Size::default(),
				)
			}
			SplitSeed::Move(index) => {
				if self.region_at(pane).tabs.len() < 2 {
					return iced::Task::none();
				}
				let Some(tab) = self.take_tab(pane, index) else {
					return iced::Task::none();
				};
				tab
			}
			SplitSeed::Duplicate(index) => {
				let Some((key, cwd)) = self.copy_source(pane, index) else {
					return iced::Task::none();
				};
				let id = self.next_id;
				self.next_id += 1;
				let mut tab = Tab::home(
					self.targets.clone(),
					self.vault.clone(),
					id,
					iced::Size::default(),
				);
				// Held until the tab is in the tree: the connect it starts can put a dialog on
				// screen, and a dialog belonging to a tab that is not yet anywhere would be drawn
				// nowhere.
				opening = Some(tab.open_copy_of(&key, cwd));
				tab
			}
		};
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		// `split` only fails on a pane that is not in the tree, which was just ruled out. If it ever
		// did, the id above is simply never used — ids are monotonic and skipping one is harmless.
		let Some((fresh, _split)) = self.regions.split(way.axis(), pane, Region::new(tab)) else {
			return iced::Task::none();
		};
		self.focus = fresh;
		self.window = size;
		self.settings.set_window(size.width, size.height);
		// Both halves are new shapes, so both terminals have to be re-measured — including the one
		// that was already there, which now has a divider beside it (§48).
		let relayout = self.relayout();
		let opening = opening.unwrap_or_else(iced::Task::none);
		iced::Task::batch([relayout, opening, iced::window::resize(window, size)])
	}

	/// A divider was dragged (§48): the two regions either side of it re-share their room.
	///
	/// Only the RATIO is stored, never a pixel count, so the share survives a window resize instead
	/// of becoming a stale measurement of a window that is no longer that size.
	fn on_divider_dragged(&mut self, split: pane_grid::Split, ratio: f32) -> iced::Task<Message> {
		// The press holding this drag can no longer be half of a double click (§48): a drag ends with
		// the pointer still on the seam, so two quick nudges of a divider — an ordinary way to place
		// one — would otherwise read as a double click and throw away the share just being set.
		// Forgetting the press rather than blocking the next one is what keeps the gesture available:
		// the double click AFTER a drag is two fresh presses, and it works.
		self.seam_clicks = ui::selection::Clicks::default();
		self.regions.resize(split, ratio);
		// Both regions changed size, so both grids have to be told: a terminal is only ever as big as
		// it was last told to be (§9). `Tab::on_window_resized` reflows only when the row/column count
		// actually changes, which is what keeps a drag from resizing the remote pty every frame.
		self.relayout()
	}

	/// A left press landed somewhere in the window — even the shares if it was the second one on a
	/// divider (§48).
	///
	/// A dragged divider is a share the user placed by hand, and there is no way back to the middle
	/// once it has moved: the window is grown by a split, never divided, so nothing else ever
	/// re-centres a seam. A double click on it is that way back, and it is the gesture every desktop
	/// already uses for "reset this handle".
	///
	/// The press has to be caught here, off the raw event stream, because a press on a seam is the
	/// one click in the window that reaches no widget: `pane_grid` captures it to start its own
	/// resize gesture and publishes nothing, so there is no `on_double_click` to hang this on. What
	/// arrives is therefore EVERY left press in the window, and the geometry decides — a press that
	/// is not on a seam breaks the run, exactly as a press on another cell breaks the grid's (§42).
	///
	/// A drag cannot be half of this gesture, and `on_divider_dragged` is where that is enforced: it
	/// forgets the press holding it, so the nudge-nudge of placing a divider by hand never reads as a
	/// double click, while a real double click after a drag — two fresh presses — still does.
	fn on_pointer_pressed(&mut self) -> iced::Task<Message> {
		let Some(split) = ui::split::seam_at(&self.regions, self.window, self.pointer) else {
			// Somewhere else in the window: whatever was being counted on a seam is over.
			self.seam_clicks = ui::selection::Clicks::default();
			return iced::Task::none();
		};
		// A third press is `Triple` and does nothing: the shares are already even, and leaning on the
		// button should not keep re-doing it (§42's counter cycles for the same reason).
		if self.seam_clicks.press(split, std::time::Instant::now()) != ui::selection::Click::Double
		{
			return iced::Task::none();
		}
		self.regions.resize(split, ui::split::EVEN);
		// Both regions changed size, so both terminals have to be re-measured — the same thing a drag
		// does, since this IS a drag, straight to the middle (§9, §48).
		self.relayout()
	}

	/// A region closed because its last tab did (§48). The room it held goes back to the region
	/// beside it, the keyboard follows if it was here, and the window gives the OS back the space the
	/// split asked it for.
	///
	/// The shrink is the exact mirror of the grow, and by the same rule: **the surviving region keeps
	/// the box it already has.** A split hands the region being cut its own size back and adds an
	/// equal one beside it, so nothing already on screen reflows; a close takes the departing
	/// region's share and the seam away again, and nothing reflows either. The survivor's rectangle
	/// IS the new window size, with no axis test needed to work that out: with two regions the
	/// survivor already spans the whole window along the axis they share, so its own box differs from
	/// the window on exactly the axis the split was made along.
	///
	/// That is also why this is not "halve the window". The window may have been resized by hand and
	/// the divider dragged well off centre since the split, and halving would be arithmetic on a
	/// number the user chose. Measuring the survivor respects both.
	///
	/// `pane_grid` refuses to close the LAST region, and closing the last tab of the last region is a
	/// quit (§30), so the `None` arm is unreachable through the UI — it opens a fresh home tab rather
	/// than leave the window showing nothing.
	fn close_region(&mut self, pane: pane_grid::Pane) -> iced::Task<Message> {
		// Measured BEFORE the close, while the region being closed still has its share: afterwards the
		// survivor's rectangle is the whole window and there is nothing left to read the shrink off.
		let survivor = ui::split::regions(&self.regions, self.window);
		match self.regions.close(pane) {
			Some((_closed, sibling)) => {
				if self.focus == pane {
					self.focus = sibling;
				}
				let shrunk = survivor.get(&sibling).map(|rect| {
					// Clamped to the floor the settings file holds a remembered size to: a divider
					// dragged near the end of its travel can leave a survivor narrower than the
					// smallest window cmote will reopen, and a window it refuses to remember is one
					// that jumps back to its old size on the next run.
					iced::Size::new(
						rect.width.max(crate::settings::MIN_WINDOW),
						rect.height.max(crate::settings::MIN_WINDOW),
					)
				});
				let Some(shrunk) = shrunk else {
					// Unreachable in practice — the sibling was in the tree a line ago. Leaving the
					// window as it is costs a stretched region, which the relayout below still fits.
					return self.relayout();
				};
				self.window = shrunk;
				self.settings.set_window(shrunk.width, shrunk.height);
				// Measured against the size the window is ASKED for, exactly as a split is: if the OS
				// grants something else the resize event that follows corrects every region, and if it
				// grants it silently there is no event to correct anything with.
				let relayout = self.relayout();
				let resize = iced::window::latest()
					.and_then(move |window| iced::window::resize(window, shrunk));
				iced::Task::batch([relayout, resize])
			}
			None => self.open_tab(pane),
		}
	}

	/// The region `pane`, or the focused one if it has gone (§48). Reading a region that a message
	/// still names but that has since closed is a real case — the split flow asks the OS a question
	/// and comes back a turn later — and this keeps every caller from having to say so again.
	fn region_at(&self, pane: pane_grid::Pane) -> &Region {
		self.regions.get(pane).unwrap_or_else(|| self.region())
	}

	/// Open a new tab on the home screen in `pane` and make it the one on screen (§26, §48). It
	/// inherits the window geometry / focus so its first paint is sized right.
	fn open_tab(&mut self, pane: pane_grid::Pane) -> iced::Task<Message> {
		let id = self.next_id;
		self.next_id += 1;
		// Inherit the window geometry / focus from the tab on screen if there is one; an empty strip
		// (the last tab was just closed) starts from defaults, corrected by the next resize (§26).
		let (size, focused, modifiers) = self.region_at(pane).carried();
		let mut tab = Tab::home(self.targets.clone(), self.vault.clone(), id, size);
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		let Some(region) = self.regions.get_mut(pane) else {
			return iced::Task::none();
		};
		region.tabs.push(tab);
		region.active = region.tabs.len() - 1;
		// A new tab opens active, which counts as a visit: it goes to the top of the order, so
		// closing it straight away comes back to the tab it was opened from (§37).
		region.recent.touch(id);
		// A "+" press also means "I am working here" (§48): the strip that was clicked takes the
		// keyboard, so typing goes into the tab that just opened rather than into another region.
		self.focus = pane;
		iced::Task::none()
	}

	/// Switch `pane` to the tab at `index` (§26). Carry the window geometry / focus onto it (the
	/// outgoing tab held the latest) and refit its terminal, in case the window resized while it was
	/// in the background.
	fn select_tab(&mut self, pane: pane_grid::Pane, index: usize) -> iced::Task<Message> {
		// The strip that was clicked takes the keyboard (§48), whether or not the click changes which
		// tab is on screen — clicking the chip already showing is how a user says "type in here".
		self.focus = pane;
		let Some(region) = self.regions.get_mut(pane) else {
			return iced::Task::none();
		};
		if index >= region.tabs.len() || index == region.active {
			return iced::Task::none();
		}
		let (size, focused, modifiers) = region.carried();
		region.active = index;
		let tab = region.active_mut();
		tab.window_size = size;
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		// This is the visit the whole order is built from: whichever tab is on screen is the top of
		// it, so the tab now being left becomes the one a close falls back to (§37).
		let id = tab.id;
		region.recent.touch(id);
		// Re-measure rather than trust the carried size: the tab arriving may have been in the
		// background across a window resize or a divider drag (§48).
		self.relayout()
	}

	/// A press on a chip (§38): make that tab active — a press IS a selection, as it always was —
	/// and arm a drag on it. The drag starts with no target, so a press-and-release on the same chip
	/// reorders nothing; only travelling to another chip gives the gesture somewhere to drop.
	fn grab_tab(&mut self, pane: pane_grid::Pane, index: usize) -> iced::Task<Message> {
		if let Some(region) = self.regions.get_mut(pane)
			&& let Some(tab) = region.tabs.get(index)
		{
			region.tab_drag = Some(TabDrag {
				grabbed: tab.id,
				over: None,
			});
			// The hand closes on the press and stays closed until the release (§51), which is why
			// this is set here rather than when the pointer first MOVES with the button down: a
			// press that never moves still holds the chip.
			crate::cursor::set_dragging(true);
		}
		self.select_tab(pane, index)
	}

	/// A tab's "×" (§26): confirm first if it holds a live session — like the Disconnect button,
	/// closing is not undoable — otherwise drop it at once. The tab being closed is brought to the
	/// front so the confirmation reads against the session it dismisses.
	fn request_close(&mut self, id: u64) -> iced::Task<Message> {
		let Some((pane, index)) = self.locate(id) else {
			return iced::Task::none();
		};
		let Some(region) = self.regions.get(pane) else {
			return iced::Task::none();
		};
		// Closing the last tab of the ONLY region would empty the window — that is really a request to
		// quit cmote, so it takes the quit confirmation (which also disconnects every session cleanly)
		// rather than silently reopening a fresh home tab as it used to (§30). With a split open it is
		// instead a request to close that region (§48), which `remove_tab` does once the strip empties,
		// so the confirmations below still guard a live session or unsaved edits on the way.
		if self.regions.len() == 1 && region.tabs.len() == 1 {
			return self.request_quit();
		}
		let (live, dirty, on_screen) = (
			region.tabs[index].is_live(),
			region.tabs[index].is_dirty_editor(),
			region.active,
		);
		if live {
			self.pending_close = Some(id);
			self.overlay = ui::dialog::Card::opened(self.window);
			if index != on_screen {
				return self.select_tab(pane, index);
			}
			iced::Task::none()
		} else if dirty {
			// A dirty editor is as protected as a live session (§32): its "×" raises the
			// unsaved-changes prompt (Save & close / Discard / Cancel) rather than dropping the edits.
			self.pending_editor_close = Some(id);
			self.overlay = ui::dialog::Card::opened(self.window);
			if index != on_screen {
				return self.select_tab(pane, index);
			}
			iced::Task::none()
		} else {
			self.remove_tab(pane, index)
		}
	}

	/// Begin the quit flow (§30): raised by the OS window's × or by closing the last tab. The
	/// "Quit cmote?" confirmation goes up over everything; nothing is torn down until the user
	/// accepts. A no-op if a quit is already in flight, and it supersedes any single-tab close
	/// confirmation, since quitting closes every tab anyway.
	fn request_quit(&mut self) -> iced::Task<Message> {
		if self.quit.is_none() {
			self.pending_close = None;
			self.quit = Some(QuitPhase::Confirming);
			self.overlay = ui::dialog::Card::opened(self.window);
		}
		iced::Task::none()
	}

	/// The "Quit cmote?" confirmation was accepted (§30): send every live session a clean
	/// Disconnect and wait for each to report it is down before the process exits — so no remote
	/// connection is cut mid-flight. With nothing live there is nothing to drain, so exit at once;
	/// otherwise the frame clock (subscribed while draining) polls the timeout as a safety net.
	fn quit_confirmed(&mut self) -> iced::Task<Message> {
		// Every live session in the WINDOW, splits and all (§48) — a quit closes the process, so a
		// session in a region the user is not looking at has to come down as cleanly as the rest.
		let pending: Vec<u64> = self
			.tabs()
			.filter(|tab| tab.is_live())
			.map(|tab| tab.id)
			.collect();
		if pending.is_empty() {
			return self.exit_app();
		}
		for tab in self.tabs_mut().filter(|tab| tab.is_live()) {
			// Saves each session before it goes (§22), the same snapshot a disconnect writes.
			tab.persist_session();
			// A local shell is asked to leave on its own first, which is why this is not a plain
			// Disconnect (§104). The window it costs is a fraction of `QUIT_DRAIN_TIMEOUT`.
			tab.end_session();
		}
		self.quit = Some(QuitPhase::Draining {
			pending,
			since: std::time::Instant::now(),
		});
		iced::Task::none()
	}

	/// The "Quit cmote?" confirmation was dismissed (§30). Only backs out while still asking —
	/// once draining has begun there is nothing to cancel, so a stray backdrop click is inert.
	fn quit_cancelled(&mut self) -> iced::Task<Message> {
		if matches!(self.quit, Some(QuitPhase::Confirming)) {
			self.quit = None;
		}
		iced::Task::none()
	}

	/// A frame tick while draining (§30): exit once the timeout is up even if a session never
	/// acknowledged, so a wedged transport can never hold the process open. The common case — every
	/// session already down — has exited via `note_drained` long before this fires.
	fn quit_tick(&mut self) -> iced::Task<Message> {
		if let Some(QuitPhase::Draining { since, .. }) = &self.quit
			&& since.elapsed() >= QUIT_DRAIN_TIMEOUT
		{
			return self.exit_app();
		}
		iced::Task::none()
	}

	/// Record that tab `id`'s session has finished its clean teardown during a quit drain (§30).
	/// Returns the exit task once none remain — so the process leaves only after every remote
	/// connection has closed — and `None` when not draining or others are still outstanding.
	fn note_drained(&mut self, id: u64) -> Option<iced::Task<Message>> {
		// Scope the mutable borrow of `self.quit` so `exit_app` (which borrows `self` shared)
		// can run once the drain is done, without overlapping it.
		let done = {
			let QuitPhase::Draining { pending, .. } = self.quit.as_mut()? else {
				return None;
			};
			pending.retain(|&waiting| waiting != id);
			pending.is_empty()
		};
		done.then(|| self.exit_app())
	}

	/// The single way out of the process (§30, §31): write the app-wide layout — the window size
	/// and the per-extension editor themes (§32) — to `settings.json`, then hand iced the exit
	/// task. Every quit path funnels through here (the confirm with nothing live, the drain
	/// finishing, the drain timing out), so the layout is saved exactly once however the app comes
	/// down. The save runs synchronously before the returned task is processed, so the file is on
	/// disk before the runtime leaves.
	fn exit_app(&self) -> iced::Task<Message> {
		self.settings.save();
		iced::exit()
	}

	/// While the quit dialog is up, decide the fate of one message (§30). A keystroke is consumed:
	/// on the confirmation, Esc cancels and Enter accepts; anything else is swallowed so it cannot
	/// reach the shell, and while draining every key is swallowed. A non-key message returns `None`
	/// to flow on to `update` untouched — the Quit/Cancel buttons, SSH events and the drain tick.
	fn quit_key_intercept(&mut self, message: &Message) -> Option<iced::Task<Message>> {
		use iced::keyboard::key::Named;
		let (Message::Key(event) | Message::HomeKey(event) | Message::FormKey(event)) = message
		else {
			return None;
		};
		if matches!(self.quit, Some(QuitPhase::Confirming))
			&& let iced::keyboard::Event::KeyPressed { key, .. } = event
		{
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				return Some(self.quit_cancelled());
			}
			if matches!(key, iced::keyboard::Key::Named(Named::Enter)) {
				return Some(self.quit_confirmed());
			}
		}
		Some(iced::Task::none())
	}

	/// The close confirmation was accepted (§26): disconnect the tab's session and drop it.
	fn close_confirmed(&mut self) -> iced::Task<Message> {
		let Some(id) = self.pending_close.take() else {
			return iced::Task::none();
		};
		let Some((pane, index)) = self.locate(id) else {
			return iced::Task::none();
		};
		// Tear the session down cleanly first: the Disconnect closes the remote side — a local shell is
		// asked to leave on its own before it (§104) — and dropping the tab then drops its command
		// sender, which ends its worker loop (§4, §26).
		if let Some(region) = self.regions.get_mut(pane) {
			region.tabs[index].end_session();
		}
		self.remove_tab(pane, index)
	}

	/// Drop the tab at `index` of the region `pane`, bringing forward the tab the user was on before
	/// this one (§26, §37) — or closing the region itself if that was its last tab (§48).
	fn remove_tab(&mut self, pane: pane_grid::Pane, index: usize) -> iced::Task<Message> {
		// A viewer closed mid-read: stop the read (§121). Done HERE and not in `take_tab`, which is
		// the shared bookkeeping for a close and a MOVE (§52) — a tab dragged to another region is
		// still waiting for its file, and cancelling that would turn a drag into a failed open.
		//
		// The cancel goes down the PARENT session's channel, because a viewer has none of its own.
		// Read the pair before the mutable borrow below, since sending needs `self` again.
		if let Some((parent, viewer_id)) = self
			.regions
			.get(pane)
			.and_then(|region| region.tabs.get(index))
			.and_then(Tab::loading_read)
			&& let Some(session) = self.tab_mut(parent)
		{
			session.send_command(SshCommand::CancelFileLoad { viewer_id });
		}
		let Some(region) = self.regions.get_mut(pane) else {
			return iced::Task::none();
		};
		if index >= region.tabs.len() {
			return iced::Task::none();
		}
		// Save a live tab's session before it goes (§22) — the same snapshot a disconnect writes.
		region.tabs[index].persist_session();
		// Everything else about taking a tab out of a strip is shared with a MOVE (§52), which does
		// the same bookkeeping and then puts the tab somewhere rather than dropping it. The `Tab`
		// falls out of scope here, which is what ends its session: its command channel goes with it.
		if self.take_tab(pane, index).is_none() {
			return iced::Task::none();
		}
		// The strip is empty, so the region has nothing to show. With a split open the region closes
		// and gives its room back to the one beside it (§48); with no split this is unreachable,
		// because the last tab of the only region raises a quit instead (§30).
		if self
			.regions
			.get(pane)
			.is_some_and(|region| region.tabs.is_empty())
		{
			return self.close_region(pane);
		}
		// Re-measure rather than trust the carried size: the tab coming forward may have been in the
		// background across a window resize or a divider drag (§48).
		self.relayout()
	}

	/// Lift the tab at `index` out of `pane` and hand it back, WITHOUT ending it (§52).
	///
	/// The strip bookkeeping a departure needs is the same whether the tab is being closed or moved
	/// to another region, so both go through here and the caller decides the tab's fate. What it does
	/// NOT do is deal with a strip left empty — a close turns that into a closed region, a move can
	/// only reach it when there is somewhere for the room to go — so the caller checks for that too.
	///
	/// `None` means there was nothing at that position: a stale index, or a region that has closed
	/// since the message naming it was raised.
	fn take_tab(&mut self, pane: pane_grid::Pane, index: usize) -> Option<Tab> {
		let region = self.regions.get_mut(pane)?;
		if index >= region.tabs.len() {
			return None;
		}
		// The tab currently on screen holds the freshest window geometry / focus, and the tab coming
		// forward is given them. Read them BEFORE the removal: when the tab leaving IS the one on
		// screen, this is the last moment they exist (§26).
		let carried = region.carried();
		let gone = region.tabs.remove(index);
		// Take the tab out of the activation order, which names the one that should come forward:
		// the most recently activated of those left (§37).
		let forward = region.recent.forget(gone.id);
		if region.tabs.is_empty() {
			return Some(gone);
		}
		match forward.and_then(|id| region.tabs.iter().position(|tab| tab.id == id)) {
			// One rule covers both cases. Losing the tab ON SCREEN pops the order's top, so this is
			// the tab the user was last on — not whichever chip happens to sit next door in the strip.
			// Losing a BACKGROUND tab leaves the top alone, so this resolves to the on-screen tab
			// itself, which is exactly right: a tab leaving from off screen must not change what is
			// shown.
			Some(position) => region.active = position,
			// Only reachable if a tab were opened without ever being activated, which no path does.
			// Fall back to the old strip arithmetic rather than leave `active` pointing anywhere.
			None => {
				if index < region.active {
					region.active -= 1;
				} else if region.active >= region.tabs.len() {
					region.active = region.tabs.len() - 1;
				}
			}
		}
		let (size, focused, modifiers) = carried;
		let tab = region.active_mut();
		tab.window_size = size;
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		Some(gone)
	}

	/// The region an area of the window currently is, or `None` for one that would have to be cut
	/// first (§52).
	fn pane_of(&self, area: ui::split::Area) -> Option<pane_grid::Pane> {
		ui::split::areas(&self.regions, self.window)
			.into_iter()
			.find(|(named, _)| *named == area)
			.map(|(_, pane)| pane)
	}

	/// What a duplicate of the tab at `index` of `pane` would be opened from (§52): the endpoint to
	/// dial again, and the directory its shell is standing in, if it has announced one.
	///
	/// `None` when there is nothing to copy — a home tab, a connect form, an editor. A copy is a
	/// second connection to the same machine, so it needs a session to have been made in the first
	/// place; everything else the menu greys out on the strength of this answer.
	fn copy_source(&self, pane: pane_grid::Pane, index: usize) -> Option<(String, Option<String>)> {
		let tab = self.regions.get(pane)?.tabs.get(index)?;
		if !tab.is_live() {
			return None;
		}
		let endpoint = tab.connection.clone()?;
		let cwd = tab
			.terminal
			.as_ref()
			.and_then(term::Terminal::cwd)
			.map(str::to_owned);
		Some((endpoint, cwd))
	}

	/// The menu's own rows for the tab it is open on (§52): which areas it offers, and which of the
	/// two actions can act on each.
	///
	/// An undivided window offers all three, because two of them are a cut away and the menu is what
	/// makes the cut. A split one offers only the two that exist: the third would need the window
	/// made whole and cut the other way, which is more than a menu item should be asked to mean.
	fn destinations(&self, menu: StripMenu) -> Vec<ui::tabs::Destination> {
		let areas = ui::split::areas(&self.regions, self.window);
		let offered: Vec<ui::split::Area> = if areas.len() == 1 {
			vec![
				ui::split::Area::Main,
				ui::split::Area::Right,
				ui::split::Area::Bottom,
			]
		} else {
			areas.iter().map(|(area, _)| *area).collect()
		};
		// Read once: both flags are about the tab and its strip, not about any one destination.
		let alone = self.region_at(menu.pane).tabs.len() < 2;
		let can_duplicate = self.copy_source(menu.pane, menu.index).is_some();
		offered
			.into_iter()
			.map(|area| {
				let pane = areas
					.iter()
					.find(|(named, _)| *named == area)
					.map(|(_, pane)| *pane);
				let can_move = match pane {
					// An area that is already there: a move means something unless the tab is
					// already in it.
					Some(pane) => pane != menu.pane,
					// An area that would have to be cut: the tab's own region cannot be the one
					// emptied to make it, or the cut and the collapse that follows would cancel out
					// and leave nothing behind but a window that grew and shrank again.
					None => !alone,
				};
				ui::tabs::Destination {
					area,
					can_move,
					can_duplicate,
				}
			})
			.collect()
	}

	/// Menu "Move to … area" (§52): take the tab the menu is open on out of its strip and put it in
	/// `area`, cutting the window first if that area does not exist yet.
	///
	/// The moved tab arrives ON SCREEN in its new region and takes the keyboard with it (§50): the
	/// user has just said where they want this tab, and a move that left it hidden behind whatever
	/// was showing there would have to be followed by a hunt through the strip to find it.
	///
	/// A move that empties its old region closes it, and the window is whole again (§48) — which
	/// makes this the way back from a split without closing anything: send the last tab across and
	/// the seam goes with it.
	fn move_tab_to(&mut self, area: ui::split::Area) -> iced::Task<Message> {
		let Some(menu) = self.strip_menu.take() else {
			return iced::Task::none();
		};
		let Some(dest) = self.pane_of(area) else {
			// The area is not on screen, so the move is a split whose fresh region opens holding
			// this tab. `Main` always exists, so `way` is never `None` here.
			let Some(way) = area.way() else {
				return iced::Task::none();
			};
			return self.request_split(menu.pane, way, SplitSeed::Move(menu.index));
		};
		if dest == menu.pane {
			return iced::Task::none();
		}
		let Some(mut tab) = self.take_tab(menu.pane, menu.index) else {
			return iced::Task::none();
		};
		let emptied = self
			.regions
			.get(menu.pane)
			.is_some_and(|region| region.tabs.is_empty());
		// The arriving tab is stamped with its new region's focus and modifier state — the relayout
		// below hands it the box, but neither of those travels with a resize (§48).
		let (_, focused, modifiers) = self.region_at(dest).carried();
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		let id = tab.id;
		let Some(region) = self.regions.get_mut(dest) else {
			return iced::Task::none();
		};
		region.tabs.push(tab);
		region.active = region.tabs.len() - 1;
		// Arriving on screen counts as a visit, so closing it later comes back to whatever this
		// region was showing before (§37).
		region.recent.touch(id);
		self.focus = dest;
		if emptied {
			// `close_region` re-measures everything itself, and leaves the focus alone because it
			// now names the region that is staying.
			return self.close_region(menu.pane);
		}
		self.relayout()
	}

	/// Menu "Duplicate to … area" (§52): open a second tab on the same endpoint in `area` and dial
	/// it, carrying the source shell's directory over so the copy opens where the original stands.
	///
	/// The copy is a fresh connection, not a clone: a session is a socket and a remote process, and
	/// neither can be forked from this end. It re-runs the connect the source tab made — which is why
	/// the menu offers it only on a tab that has a session (`copy_source`) — and dials straight away
	/// unless something still has to be typed, in which case the pre-filled form is what opens.
	fn duplicate_tab_to(&mut self, area: ui::split::Area) -> iced::Task<Message> {
		let Some(menu) = self.strip_menu.take() else {
			return iced::Task::none();
		};
		let Some((endpoint, cwd)) = self.copy_source(menu.pane, menu.index) else {
			return iced::Task::none();
		};
		let Some(dest) = self.pane_of(area) else {
			let Some(way) = area.way() else {
				return iced::Task::none();
			};
			return self.request_split(menu.pane, way, SplitSeed::Duplicate(menu.index));
		};
		let id = self.next_id;
		self.next_id += 1;
		let (size, focused, modifiers) = self.region_at(dest).carried();
		let mut tab = Tab::home(self.targets.clone(), self.vault.clone(), id, size);
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		let opening = tab.open_copy_of(&endpoint, cwd);
		let Some(region) = self.regions.get_mut(dest) else {
			return iced::Task::none();
		};
		// A copy made into its own strip lands NEXT TO the tab it came from, the way a duplicated
		// row lands beside its original; sent to the other region there is no "beside", so it goes
		// on the end. Either way it opens on screen, since dialing is something to watch.
		let at = if dest == menu.pane {
			menu.index + 1
		} else {
			region.tabs.len()
		};
		region.tabs.insert(at, tab);
		region.active = at;
		region.recent.touch(id);
		self.focus = dest;
		iced::Task::batch([self.relayout(), opening])
	}

	/// Open a remote file in a new VIEWER tab (§32, §53), parented to the session it was opened from,
	/// and send the read on THAT session's channel. The viewer tab has no worker of its own; its
	/// reply (`FileLoaded` / `FileLoadFailed`) rides the parent's stream and routes back here by
	/// viewer id.
	///
	/// WHICH KIND OF VIEWER IS DECIDED HERE, ONCE — the text editor, or the picture preview if the
	/// file is an image (§53). Both entry points (a double-click and the pane's open item) arrive at
	/// this one function precisely so the answer cannot differ between them. The decision is by
	/// EXTENSION rather than by content, because it chooses which tab to open and that has to happen
	/// before a byte has been read; the DECODER, in contrast, is chosen by the bytes themselves
	/// (`preview::decode_image`), so a mislabelled file still opens correctly once it arrives.
	///
	/// The tab opens in `pane`, the region the file was clicked in (§48) — beside its own session's
	/// chip, in the same strip. It could be argued the other way, that a file wants a region of its
	/// own, but the tab it is grouped with is the one it reads through: keeping the two in one strip
	/// keeps that relationship visible instead of scattering a session's files across the window.
	fn open_viewer(
		&mut self,
		pane: pane_grid::Pane,
		session: u64,
		path: String,
	) -> iced::Task<Message> {
		let id = self.next_id;
		self.next_id += 1;
		// Inherit the window geometry / focus from the region's on-screen tab so the first paint is
		// sized right, exactly as `open_tab` does for a home tab (§26).
		let (size, focused, modifiers) = self.region_at(pane).carried();
		// The account the file is being opened as (§46) — the one the parent session is SHOWING right
		// now. Fixed into the viewer here rather than read again at save time: the file belongs to
		// that account, and the panes may well have switched to another by the time it is saved.
		let identity = self
			.tabs()
			.find(|tab| tab.id == session)
			.map_or(bridge::LOGIN_IDENTITY, |tab| tab.identity);
		let picture = crate::preview::opens_preview(&path);
		let mut tab = if picture {
			Tab::new_preview(id, session, path.clone(), size)
		} else {
			// Open in the scheme this file type was last edited in (§32); an unseen extension starts
			// on the default. The choice is recorded back in `settings` when the toolbar's select
			// changes it, and rides `settings.json`, so the type keeps its scheme across a restart
			// (§31).
			let theme = self
				.settings
				.editor_theme(&crate::editor::extension_key(&path));
			Tab::new_editor(id, session, identity, path.clone(), size, theme)
		};
		tab.window_focused = focused;
		tab.modifiers = modifiers;
		let Some(region) = self.regions.get_mut(pane) else {
			return iced::Task::none();
		};
		// Not at the end of the strip: right beside the session the file came from (§38), so a
		// session and the files opened out of it stay one group instead of drifting apart as other
		// sessions are opened.
		let slot = region.editor_slot(session);
		region.tabs.insert(slot, tab);
		region.active = slot;
		// An editor tab opens active like any other (§37) — so closing it (its "×", or Save & close)
		// returns to the session tab the file was opened from, which is where the user was.
		region.recent.touch(id);
		// The region it opened in takes the keyboard, so Ctrl+S reaches the buffer that just opened
		// rather than whatever another region had on screen (§48).
		self.focus = pane;

		// Ask the parent session to read the file. If the parent is gone the viewer opens straight
		// into its "session closed" state rather than hanging on a load that can never arrive. The
		// match resolves to a plain `bool` so the parent borrow is released before the fallback,
		// which borrows the tabs again to reach the just-opened viewer.
		//
		// The ceiling rides the command because the two viewers disagree about it (§53): 8 MiB is
		// generous for a config file and mean for a photograph.
		let limit = if picture {
			crate::preview::MAX_SIZE
		} else {
			crate::ssh::edit::MAX_SIZE
		};
		let sent = match self.tab_mut(session) {
			Some(parent) if parent.command_tx.is_some() => {
				parent.send_command(SshCommand::FileLoad {
					identity,
					viewer_id: id,
					path,
					limit,
				})
			}
			_ => false,
		};
		if !sent && let Some(viewer) = self.viewer_mut(id) {
			viewer.parent_gone(
				"The session this file was opened from is no longer available.".to_owned(),
			);
		}
		// The strip gained a chip, which changes nothing about the region's box — but the tab that
		// just came on screen has never been measured, so it is given one (§48).
		self.relayout()
	}

	/// Send an editor tab's buffer to its parent session for saving (§32). Raised by the tab after a
	/// Save / Save As; only the App can reach across to the parent's channel — across regions too
	/// since §48, since an editor and its session can be dragged apart. A parent that has gone away
	/// leaves the editor's save marked failed rather than hanging on a reply that never comes.
	fn flush_editor_save(&mut self, viewer_id: u64) -> iced::Task<Message> {
		// The identity comes from the EDITOR, not from what the session is showing now (§46): the file
		// was read as that account and has to be written back as the same one.
		let Some((session, identity, path, bytes)) = self
			.tabs()
			.find(|tab| tab.id == viewer_id)
			.and_then(Tab::editor)
			.map(|editor| {
				(
					editor.session,
					editor.identity,
					editor.path.clone(),
					editor.save_bytes(),
				)
			})
		else {
			return iced::Task::none();
		};
		let sent = match self.tab_mut(session) {
			Some(parent) => parent.send_command(SshCommand::EditSave {
				identity,
				viewer_id,
				path,
				bytes,
			}),
			None => false,
		};
		if !sent && let Some(editor) = self.editor_mut(viewer_id) {
			editor.mark_parent_gone();
			editor.save_failed("The session this file came from is closed.".to_owned());
		}
		iced::Task::none()
	}

	/// Tell every viewer opened from session `id` that its parent is gone (§32, §53). The two kinds
	/// need different things said, because they lost different things with it.
	///
	/// An EDITOR loses its way to save: the buffer stays open to read and copy, and the toolbar
	/// disables Save with a note. An editor that never finished loading keeps that state too — its
	/// buffer is empty and there is nothing to lose.
	///
	/// A PREVIEW that is still LOADING loses everything, because a picture half-read is no picture:
	/// the read it is waiting on can never arrive now, so it is failed here rather than left showing
	/// "Loading…" for the rest of the tab's life. One that already has its picture is untouched —
	/// the image is decoded and in memory, and it stays as good as it was a moment ago.
	fn orphan_viewers(&mut self, id: u64) {
		for tab in self.tabs_mut() {
			if let Some(viewer) = tab.viewer.as_mut()
				&& viewer.session() == id
			{
				viewer.orphan();
			}
		}
	}

	/// The unsaved-editor close prompt's "Save & close" (§32): begin the save with a close-after flag
	/// so the tab drops itself once the write lands (or stays, showing the error, if it fails).
	fn editor_close_save(&mut self) -> iced::Task<Message> {
		let Some(id) = self.pending_editor_close.take() else {
			return iced::Task::none();
		};
		let Some(editor) = self.editor_mut(id) else {
			return iced::Task::none();
		};
		if editor.begin_save_and_close() {
			return self.flush_editor_save(id);
		}
		// Nothing to save (or no channel): just close it.
		self.force_close(id)
	}

	/// The unsaved-editor close prompt's "Discard" (§32): drop the tab and lose the edits.
	fn editor_close_discard(&mut self) -> iced::Task<Message> {
		let Some(id) = self.pending_editor_close.take() else {
			return iced::Task::none();
		};
		self.force_close(id)
	}

	/// Drop the tab with this id if it is still there (§32) — the shared tail of the discard, the
	/// after-save auto-close and the editor's own Ctrl+W.
	fn force_close(&mut self, id: u64) -> iced::Task<Message> {
		match self.locate(id) {
			Some((pane, index)) => self.remove_tab(pane, index),
			None => iced::Task::none(),
		}
	}

	/// Apply a theme pick from an editor's toolbar (§32): paint that editor in the new scheme and
	/// remember it against the file's extension, so the next editor opened on that type inherits it.
	/// The toolbar that raised this belongs to the tab on screen in `pane` — the region the pick was
	/// made in (§48) — so that is the editor it sets.
	fn set_editor_theme(
		&mut self,
		pane: pane_grid::Pane,
		theme: crate::editor::EditorTheme,
	) -> iced::Task<Message> {
		let Some(region) = self.regions.get_mut(pane) else {
			return iced::Task::none();
		};
		if let Some(editor) = region.tabs.get_mut(region.active).and_then(Tab::editor_mut) {
			editor.set_theme(theme);
			let ext = crate::editor::extension_key(&editor.path);
			// Remembered app-wide and written on the way out (§31), so this file type keeps the
			// scheme next run; the returned "changed?" flag is not needed here.
			self.settings.set_editor_theme(ext, theme);
		}
		iced::Task::none()
	}

	/// The viewer on the tab with this id, mutably (§32, §53), wherever in the window that tab sits
	/// — whichever of the two kinds it is. Callers that need one kind in particular go on to ask
	/// for it; callers that only need what the two share (the parent session, the path, "your
	/// parent is gone") stop here, and that is most of them.
	fn viewer_mut(&mut self, id: u64) -> Option<&mut Viewer> {
		self.tab_mut(id).and_then(|tab| tab.viewer.as_mut())
	}

	/// The editor on the tab with this id, mutably (§32) — `None` when that tab is showing a
	/// picture, which has no buffer to give.
	fn editor_mut(&mut self, id: u64) -> Option<&mut crate::editor::Editor> {
		self.viewer_mut(id).and_then(Viewer::editor_mut)
	}

	/// True while an App-level overlay card is on screen (§26, §30): a live tab's close
	/// confirmation or the quit dialog. Both float over the whole window and share one card, so
	/// the header-drag messages are steered here (rather than to the active tab) while it holds.
	fn overlay_open(&self) -> bool {
		self.quit.is_some() || self.pending_close.is_some() || self.pending_editor_close.is_some()
	}

	/// The window title, from the tab on screen in the region holding the keyboard (its endpoint and
	/// shell directory, §17). One window has one title bar however many regions are in it, so the
	/// focused region is the one that gets to name it (§48).
	fn title(&self) -> String {
		self.active().title()
	}

	/// Draw the window, and let the hand cursor watch it being drawn (§52).
	///
	/// Every grab handle says it is still on screen as it builds itself, so a hand held by one that
	/// has GONE — a dialog closed by the ✕ under the pointer, a chip closed or sent to another
	/// region — is let go the moment the frame that no longer contains it is finished. iced offers
	/// nothing better to hang this on: a widget publishes its own `on_exit`, so a widget that has
	/// left the tree publishes nothing at all, and the frame is the only place that knows what is
	/// still there.
	///
	/// Bracketing here rather than inside `screen` is what makes it one pair of calls instead of one
	/// per early return.
	fn view(&self) -> Element<'_, Message> {
		crate::cursor::frame_begin();
		let frame = self.screen();
		crate::cursor::frame_end();
		frame
	}

	/// Draw the split frame — every region's strip and the tab beneath it — then, if a quit or a
	/// live tab's close is pending, the confirmation over everything (§26, §30, §48).
	fn screen(&self) -> Element<'_, Message> {
		// Copied out rather than read through `self` inside the closure, so nothing borrows the App
		// for longer than the regions themselves do.
		let focus = self.focused_pane();
		// Whether any strip shows the split controls (§48). Read once for the whole frame: it is a fact
		// about the window, not about a region, and every region has to agree on it.
		let splittable = self.splittable();
		let body = ui::split::frame(&self.regions, move |pane, region| {
			Self::region_view(pane, region, pane == focus, splittable)
		});

		// A chip's menu (§52), over the whole window rather than inside its region: it offers to send
		// the tab across a seam, and a menu the seam could clip would be a poor advertisement for
		// that. It hangs from the strip it was raised on — the region's own top-left corner, just
		// below the bar — so it needs no stored pointer position and follows a divider dragged while
		// it is open. Ranked below every modal: the dialogs return before this point.
		let body = match self.strip_menu {
			Some(menu) => {
				let regions = ui::split::regions(&self.regions, self.window);
				match regions.get(&menu.pane) {
					Some(region) => {
						let at = iced::Point::new(region.x, region.y + ui::tabs::STRIP_HEIGHT);
						iced::widget::stack![
							body,
							ui::menu::dismiss_layer(Message::TabMenuDismissed),
							ui::tabs::context_menu(at, self.window, &self.destinations(menu)),
						]
						.width(iced::Length::Fill)
						.height(iced::Length::Fill)
						.into()
					}
					// The region has closed since the menu opened — nothing to hang it from, and
					// nothing it could still act on.
					None => body,
				}
			}
			None => body,
		};

		// The app-wide quit dialog outranks a single tab's close: it floats over the whole window,
		// every split and strip included (§30, §48). While confirming it offers Cancel / Quit; while
		// draining it just reports progress with no buttons, since there is nothing left to cancel.
		if let Some(quit) = &self.quit {
			return self.quit_overlay(body, quit);
		}

		// A dirty editor's close waits on a three-way prompt (§32): Save & close / Discard / Cancel.
		// Ranked below the app-wide quit but, like the live-session close, over the whole window.
		if let Some(id) = self.pending_editor_close {
			let name = self
				.tabs()
				.find(|tab| tab.id == id)
				.and_then(Tab::editor)
				.map_or_else(
					|| "This file".to_owned(),
					|editor| crate::explorer::name(&editor.path).to_owned(),
				);
			let message = text(format!("“{name}” has unsaved changes.")).size(14);
			let footer = vec![
				iced::widget::button(text("Cancel"))
					.on_press(Message::EditorCloseCancelled)
					.into(),
				iced::widget::button(text("Discard"))
					.on_press(Message::EditorCloseDiscard)
					.into(),
				iced::widget::button(text("Save & close"))
					.on_press(Message::EditorCloseSave)
					.into(),
			];
			let card = ui::dialog::dialog(
				"Save changes?".to_owned(),
				Message::EditorCloseCancelled,
				message.into(),
				footer,
				self.overlay,
			);
			return iced::widget::stack![
				body,
				ui::dialog::backdrop(Message::EditorCloseCancelled),
				card
			]
			.width(iced::Length::Fill)
			.height(iced::Length::Fill)
			.into();
		}

		if self.pending_close.is_none() {
			return body;
		}

		// A live tab's close waits on this confirmation, floated over the whole window (§26). Its
		// card is draggable by the header, so it is placed from the App-level `overlay` card
		// (centred when it opened) rather than being pinned centred every frame.
		let message =
			text("This tab has a live session. Closing it will disconnect the shell.").size(14);
		let footer = vec![
			iced::widget::button(text("Cancel"))
				.on_press(Message::TabCloseCancelled)
				.into(),
			iced::widget::button(text("Close tab"))
				.on_press(Message::TabCloseConfirmed)
				.into(),
		];
		let card = ui::dialog::dialog(
			"Close this tab?".to_owned(),
			Message::TabCloseCancelled,
			message.into(),
			footer,
			self.overlay,
		);
		iced::widget::stack![body, ui::dialog::backdrop(Message::TabCloseCancelled), card]
			.width(iced::Length::Fill)
			.height(iced::Length::Fill)
			.into()
	}

	/// Everything one region shows (§48): its own tab strip, and the tab it has on screen beneath it.
	///
	/// The `map` at the end is the load-bearing line. Every message the region's widgets raise is
	/// stamped with the region it came from, so `update` can apply it THERE rather than wherever the
	/// keyboard happens to be. It also means nothing inside a region — not the strip, not the
	/// terminal, not a dialog — has to know a region exists, which is why §48 could be built without
	/// touching a single one of them: the wrapper carries the one fact they would have needed.
	///
	/// An associated function rather than a method so the closure `view` hands to `ui::split::frame`
	/// captures nothing but a `Pane` and two `bool`s, and so cannot borrow the App a second time.
	fn region_view(
		pane: pane_grid::Pane,
		region: &Region,
		focused: bool,
		splittable: bool,
	) -> Element<'_, Message> {
		// The slot a drag would drop into, if one is in flight (§38): that chip wears the drop mark,
		// and every chip switches to the "grabbing" cursor so the whole strip reads as in motion.
		let drop_target = region.tab_drag.as_ref().and_then(|drag| drag.over);
		let chips: Vec<ui::tabs::Chip> = region
			.tabs
			.iter()
			.enumerate()
			.map(|(index, tab)| ui::tabs::Chip {
				id: tab.id,
				label: tab.strip_label(),
				active: index == region.active,
				status: tab.prompt_status(),
				progress: tab.command_progress(),
				branch: tab.branch(),
				drop_target: drop_target == Some(tab.id),
			})
			.collect();
		let strip = ui::tabs::strip(&chips, region.tab_drag.is_some(), focused, splittable);
		let stacked = iced::widget::column![strip, region.active().view()]
			.width(iced::Length::Fill)
			.height(iced::Length::Fill);
		Element::from(stacked).map(move |inner| Message::In(pane, Box::new(inner)))
	}

	/// Float the quit confirmation / drain card over the whole window (§30). Confirming: how many
	/// sessions the quit will disconnect, with Cancel / Quit. Draining: a bare "closing sessions"
	/// note with no buttons — the backdrop's dismiss message is `QuitCancelled`, which is inert
	/// once draining, so a stray click cannot abort a teardown already under way.
	fn quit_overlay<'a>(
		&self,
		body: Element<'a, Message>,
		quit: &QuitPhase,
	) -> Element<'a, Message> {
		// The quit card is draggable too (§30): it is the same App-level `overlay` card, centred
		// when the quit flow opened.
		let (heading, detail, footer): (&str, String, Vec<Element<'a, Message>>) = match quit {
			QuitPhase::Confirming => {
				// Every region's tabs, not just the focused one's (§48): a quit ends the process, so
				// the count has to be honest about what it is about to disconnect.
				let live = self.tabs().filter(|tab| tab.is_live()).count();
				let detail = if live == 0 {
					"Close cmote and all its tabs?".to_owned()
				} else {
					format!(
						"{live} live session{} will be disconnected.",
						if live == 1 { "" } else { "s" }
					)
				};
				let footer = vec![
					iced::widget::button(text("Cancel"))
						.on_press(Message::QuitCancelled)
						.into(),
					iced::widget::button(text("Quit"))
						.on_press(Message::QuitConfirmed)
						.into(),
				];
				("Quit cmote?", detail, footer)
			}
			QuitPhase::Draining { .. } => (
				"Quitting cmote…",
				"Closing sessions cleanly…".to_owned(),
				Vec::new(),
			),
		};
		let card = ui::dialog::dialog(
			heading.to_owned(),
			Message::QuitCancelled,
			text(detail).size(14).into(),
			footer,
			self.overlay,
		);
		iced::widget::stack![body, ui::dialog::backdrop(Message::QuitCancelled), card]
			.width(iced::Length::Fill)
			.height(iced::Length::Fill)
			.into()
	}

	/// The streams the app listens to (§4, §26, §48). One SSH worker PER tab wherever it sits, tagged
	/// with the tab id so its events route back to the right session; the window geometry / focus
	/// streams, global to the OS window; the frame clock while ANY region has work for it; and — keyed
	/// on the FOCUSED region's on-screen tab — the keyboard listener.
	fn subscription(&self) -> iced::Subscription<Message> {
		let mut subs: Vec<iced::Subscription<Message>> = self
			.tabs()
			// A viewer tab — an editor or a picture preview — has no session of its own (§32, §53):
			// it reads and saves through the tab it was opened from, so it starts NO worker. Opening
			// ten files costs no network threads.
			.filter(|tab| !tab.is_viewer())
			.map(|tab| {
				// `map` demands a NON-capturing closure, so the tab id cannot be closed over. `with`
				// threads it into each event as `(id, event)`, which the plain closure then unpacks
				// — routing every session's output back to the tab that owns it (§4, §26).
				bridge::session_subscription(tab.id)
					.with(tab.id)
					.map(|(id, event)| Message::Ssh(id, event))
			})
			.collect();

		// Window size and focus are the OS window's, so they are handled at the App and shared out to
		// every region rather than reacted to by one tab (§10, §23, §48).
		subs.push(iced::window::resize_events().map(|(_id, size)| Message::WindowResized(size)));
		subs.push(focus_events());
		subs.push(file_drop_events());
		// The raw pointer, asked for ONLY while there is a divider to double-click (§48). It is the
		// one stream in cmote that carries a message per pointer move, so it is switched off the
		// moment the window is whole again — an undivided window pays nothing for a gesture it
		// cannot perform, and the split one pays a field store per move.
		if self.regions.len() > 1 {
			subs.push(divider_events());
		}
		// The OS window's title-bar × arrives here rather than closing the window, because
		// `exit_on_close_request(false)` held it back — so cmote can quit on its own terms (§30).
		subs.push(iced::window::close_requests().map(|_id| Message::QuitRequested));
		// While a quit is draining, tick each frame to re-check the timeout — the same frame clock
		// the toast uses, added only for the moment the drain is in flight (§30).
		if matches!(self.quit, Some(QuitPhase::Draining { .. })) {
			subs.push(iced::window::frames().map(|_instant| Message::QuitTick));
		}

		// The two frame clocks below are asked for if ANY region needs one, and the tick then goes to
		// every region (§48). Before splits there was one visible tab and only its flags could matter;
		// now a toast in the region beside this one is just as much on screen, and a clock that
		// skipped it would leave that toast up for good.
		let on_screen = || self.regions.iter().map(|(_, region)| region.active());
		if on_screen().any(|tab| tab.snackbar.is_some()) {
			subs.push(iced::window::frames().map(|_instant| Message::SnackbarTick));
		}
		// Output landed under an open find bar (§44): tick so the re-scan runs once on the next frame
		// instead of once per output chunk. Only an ON-SCREEN tab's flag is consulted — a background
		// tab's bar is not visible, and its flag is still set when the user comes back to it, which is
		// when this subscription appears and the scan happens. Same shape as the toast above: the
		// clock exists only while there is work for it.
		if on_screen().any(|tab| tab.search_stale) {
			subs.push(iced::window::frames().map(|_instant| Message::TermFindRescan));
		}
		// A drop has landed and its paths are still arriving, one event each (§29): tick once so the
		// whole set is read together. Same shape as the two clocks above — the clock exists only
		// while there is work for it, and the tick goes to every region since the tab that caught
		// the drop is the one with paths waiting.
		if on_screen().any(|tab| tab.transfers.settling()) {
			subs.push(iced::window::frames().map(|_instant| Message::FileDropSettled));
		}
		// A terminal is holding a synchronized update (§122): tick so the 150 ms it is allowed is
		// actually measured against a clock, since `vte` sets that deadline and never reads it. Asked
		// over EVERY tab rather than the on-screen ones — the clock's whole job here is to unstick a
		// screen nobody is looking at yet — which is also why this condition is a tab method instead
		// of the `on_screen()` helper the three clocks above share.
		if self.tabs().any(Tab::holds_update) {
			subs.push(iced::window::frames().map(|_instant| Message::HeldUpdateExpired));
		}
		// The keyboard, by contrast, has exactly one destination: the region that holds it (§48).
		let active = self.active();
		match active.screen {
			AppScreen::Terminal => subs.push(iced::keyboard::listen().map(Message::Key)),
			// The form's own focus ring (Tab / Shift+Tab / Enter / Space, §10) — but ONLY while
			// nothing is being asked over it (§7, §8, §16). A prompt's fields type through the
			// widget tree, so leaving the ring live would move the highlight around behind the
			// dialog and let Enter press the Connect button under it. This is the one place that
			// rule is stated; it used to be implicit in the six `AppScreen` variants that each had no
			// keyboard subscription of their own.
			AppScreen::Connect if active.prompt.is_none() => {
				subs.push(iced::keyboard::listen().map(Message::FormKey));
			}
			AppScreen::Home => subs.push(iced::keyboard::listen().map(Message::HomeKey)),
			// What a viewer listens for depends on what it is holding, so it is asked (§32, §53).
			// The editor wants its shortcut keys (Ctrl+S / Ctrl+Shift+S / Ctrl+W) while typing goes
			// to the widget. A picture has nothing to type into, so it listens for one thing: the
			// key that closes it — without which it would be the only tab in the app a keyboard
			// cannot dismiss, which reads as a bug rather than as a design.
			AppScreen::Viewer => match &active.viewer {
				Some(Viewer::Editor(_)) => {
					subs.push(iced::keyboard::listen().map(Message::EditorKey));
				}
				Some(Viewer::Picture(_)) => {
					subs.push(iced::keyboard::listen().map(Message::PreviewKey));
				}
				None => {}
			},
			AppScreen::Connect | AppScreen::Connecting { .. } => {}
		}

		iced::Subscription::batch(subs)
	}
}

/// Which screen the single window is currently showing. This is the small state
/// machine from PLAN §10 — every transition happens in `update`.
#[derive(Debug, Default)]
pub enum AppScreen {
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
	/// A live shell: the vt100 grid fills the window.
	Terminal,
	/// A remote file open for viewing — a text editor (§32) or a picture (§53). This tab is NOT a
	/// session: it has no connection of its own, and its load (and its saves, if it has any) ride
	/// the parent session's channel. WHICH of the two it is lives in `Tab::viewer`, which is `Some`
	/// exactly while this screen shows.
	///
	/// One variant and not two, because the kind is not a property of the screen: every place that
	/// branched on `AppScreen::Editor` vs `AppScreen::Preview` immediately went on to unwrap the matching
	/// field, so the screen's job was only ever to say WHETHER a viewer is open. Saying it twice
	/// meant a `Tab` could be built claiming one kind while holding the other, and nothing rejected
	/// it. The picture screen exists at all because a `.png` opened in a text editor can only ever
	/// be refused, and "here is the picture" is the answer the double-click wanted.
	Viewer,
}

/// The question the connect flow is holding, over the (dimmed) form (§7, §8, §12, §16).
///
/// These were six `AppScreen` variants of their own — `ConfirmHostKey`, `HostKeyChanged`,
/// `NeedPassphrase`, `Interactive`, `VaultUnlock`, `Error` — but they were never separate screens:
/// every one of them renders `form_with_dialog(…)`, the connect form with a dialog over it. Calling
/// them screens cost a real thing, which is that `AppScreen::Connect`'s keyboard subscription was the
/// only place the form's own Tab / Enter ring was live. Six variants that each had to remember to
/// switch it off; one `Option` that says it once.
///
/// Each variant carries what answering it needs, so the answer is read off the thing that asked.
/// The two host-key variants carry nothing: their message is already in the selectable dialog body,
/// and the CHOICE goes back down the wire, so there is nothing to hold on this side (§8).
#[derive(Debug)]
enum Prompt {
	/// First contact with an unknown host: the server's fingerprint is shown and the user must
	/// accept or reject before the handshake continues (§8).
	HostKey,
	/// The server's host key does NOT match the one pinned for it (§8) — key rotation, or a
	/// man-in-the-middle. The loud override dialog shows both fingerprints and offers reject /
	/// trust once / replace. Dismissing REJECTS, whichever way it is dismissed.
	HostKeyChanged,
	/// The chosen private key is encrypted (§7): the passphrase being typed. Moved into a `Secret`
	/// on submit and this buffer dropped with the prompt, so no plain copy is kept (§12).
	Passphrase(String),
	/// The server posed a keyboard-interactive challenge (§7): 2FA / OTP or any
	/// challenge-response scheme. `fields` is the request, one per prompt with its echo hint, and
	/// `answers` the user's in-progress replies in the same order — moved into `Secret`s on submit.
	Interactive {
		fields: Vec<bridge::InteractivePrompt>,
		answers: Vec<String>,
	},
	/// The master-passphrase prompt for the portable secret vault (§16): CREATE it (first time,
	/// typed twice) or UNLOCK it. `pending` is what a successful unlock resumes, held here rather
	/// than beside the prompt so a dismissed prompt cannot leave a deferred connect behind.
	Vault {
		input: String,
		confirm: String,
		/// Whether this is a create (no vault file yet, two fields) rather than an unlock (one).
		/// Fixed when the prompt opens, so the view need not re-check the disk.
		creating: bool,
		/// Whether to show the "wrong / do not match" hint — set on a failed unlock or a
		/// mismatched create, and false again each time the prompt is opened afresh.
		failed: bool,
		pending: VaultPending,
	},
	/// A failure. The generic, non-leaking message (§12) is in the selectable dialog body so it can
	/// be copied; this variant just says the failure dialog is what is showing.
	Failed,
}

/// Which part of the terminal screen the keyboard is talking to (§20).
///
/// The shell is not the only thing on this screen any more: two panes sit beside it, and
/// both want the arrow keys. Rather than guess from the pointer, the window has one focus
/// at a time — the terminal to begin with, a click moves it to whatever was clicked, and
/// Ctrl+Tab cycles. While a pane holds it, no key reaches the shell: a pane that
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

/// One account this session has a shell for (§45).
///
/// An SSH connection authenticates once, as one user; becoming another account is a program run on
/// that same connection (`sudo -u root -i`), which gets a channel and a shell of its own. So this is
/// not a second connection and not a second tab — it is one more shell on the one session, with its
/// own view of the machine parked beside it.
///
/// The account NAME came back in §47, which is what this doc used to predict: it was removed with
/// §45's UX because the status bar's read-only label duplicated the centred endpoint, and it returns
/// because the accounts dialog lists these by name and the bar's button names the one on screen —
/// and neither of those is the endpoint. The endpoint says who the session AUTHENTICATED as; after
/// an elevation that is no longer who is typing.
#[derive(Debug, Default)]
struct Identity {
	/// The number the SSH task knows this shell by. `bridge::LOGIN_IDENTITY` for the account the
	/// session authenticated as; counted up from 1 for each elevation.
	id: u64,
	/// The account this shell runs as, for the dialog's list and the status bar's button (§47).
	/// `None` for the login identity, whose name is the `user@` half of the session's endpoint and
	/// is already on screen — so it is read from there rather than stored twice.
	account: Option<String>,
	/// Whether its shell is through its credential conversation and live. A shell still elevating
	/// is in the list (so a failure has something to report against) but cannot be switched to.
	ready: bool,
	/// This identity's view of the machine while it is NOT on screen. The one on screen keeps its
	/// state in `Tab`'s own fields instead, so it is `Workspace::default()` here.
	work: Workspace,
}

/// One identity's own view of the machine (§45): everything on the terminal side of a tab that
/// belongs to the account rather than to the connection.
///
/// This exists so switching accounts can be a SWAP. The fields live on `Tab` for whichever identity
/// is on screen — untouched, so every path that reads `self.terminal` or `self.selection` carries on
/// exactly as it did — and the ones off screen are held here. `Tab::exchange` moves a whole view in
/// and the live one out in a single step, which is also the one place that has to be complete: a
/// field left out of it would leak one account's state into another's pane.
///
/// What is deliberately NOT here: the folder tree, the files pane and the transfers. Those all run
/// over sftp, which the SSH server starts as the account the session LOGGED IN as — `sudo` in a
/// shell cannot reach them (§45). They are one view, shared by every identity, until §46 gives the
/// file layer its own elevation; splitting them now would only pretend they differ.
#[derive(Debug, Default)]
struct Workspace {
	terminal: Option<term::Terminal>,
	selection: Option<ui::selection::Selection>,
	selecting: bool,
	hover_cell: ui::selection::ScreenSpot,
	clicks: ui::selection::Clicks<ui::selection::ScreenSpot>,
	search: Option<term::search::Search>,
	search_stale: bool,
}

/// What a VIEWER tab is showing (§32, §53): a text buffer, or a picture.
///
/// The two are siblings, not a base and a special case — an editor has an encoding to preserve, a
/// dirty flag, changed-line marks, a theme and a save path, and a preview has none of those,
/// because it cannot write. Folding the read-only one INTO the read-write one would have meant a
/// dozen fields that are always empty on one of the two, which is the shape §16's queue was pulled
/// out of `Tab` to escape. An enum keeps both whole and adds no empty field to either.
///
/// It replaces a PAIR of `Option` fields that modelled one thing. The pair could represent states
/// that cannot exist — both `Some`, or neither `Some` on a viewer tab — so "exactly one of these is
/// open" was a convention maintained by hand at every site that touched them, and the fork between
/// the two kinds was written out five times. Here the invariant is the type's, and the fork is a
/// `match` the compiler completes.
///
/// What the two DO share is stated once, below: a viewer is parented to a session, and a viewer is
/// open on a path. Those two facts drive most of the call sites, and neither needs to know which
/// kind it is holding to ask for them.
#[derive(Debug)]
enum Viewer {
	Editor(crate::editor::Editor),
	Picture(crate::preview::Preview),
}

impl Viewer {
	/// The session this viewer was opened from (§32, §53) — the tab whose channel carries its
	/// loads, and its saves if it has any. Both kinds have one, so asking costs no fork.
	fn session(&self) -> u64 {
		match self {
			Self::Editor(editor) => editor.session,
			Self::Picture(picture) => picture.session,
		}
	}

	/// The remote path it is open on. Both kinds have one; only the editor's can change, when a
	/// Save As lands (§32).
	fn path(&self) -> &str {
		match self {
			Self::Editor(editor) => &editor.path,
			Self::Picture(picture) => &picture.path,
		}
	}

	/// The buffer, when this is an editor. `None` for a picture, which is the honest answer to
	/// "give me the text of this" rather than something to guard against at the call site.
	fn editor(&self) -> Option<&crate::editor::Editor> {
		match self {
			Self::Editor(editor) => Some(editor),
			Self::Picture(_) => None,
		}
	}

	/// The buffer, mutably.
	fn editor_mut(&mut self) -> Option<&mut crate::editor::Editor> {
		match self {
			Self::Editor(editor) => Some(editor),
			Self::Picture(_) => None,
		}
	}

	/// The picture, mutably — the twin of `editor_mut`.
	fn picture_mut(&mut self) -> Option<&mut crate::preview::Preview> {
		match self {
			Self::Picture(picture) => Some(picture),
			Self::Editor(_) => None,
		}
	}

	/// What the tab strip's chip says (§32, §53): the file's name, with a dot in front of it when
	/// there are unsaved edits. A picture never wears the dot — it has nothing to save.
	fn label(&self) -> String {
		let name = crate::explorer::name(self.path());
		match self {
			Self::Editor(editor) if editor.is_dirty() => format!("• {name}"),
			_ => name.to_owned(),
		}
	}

	/// Tell it the session it was opened from has gone (§32, §53). The two kinds lose different
	/// things with it, so they are told different things — which is exactly the sort of per-kind
	/// difference that belongs in here rather than at the caller.
	///
	/// An EDITOR loses its way to save: the buffer stays open to read and copy, and the toolbar
	/// disables Save with a note. A PICTURE still LOADING loses the read it is waiting on, so it is
	/// failed rather than left showing "Loading…" for the rest of the tab's life; one that already
	/// has its picture is untouched, because the image is decoded and in memory and is as good as it
	/// was a moment ago.
	fn orphan(&mut self) {
		match self {
			Self::Editor(editor) => editor.mark_parent_gone(),
			Self::Picture(picture) => {
				if matches!(picture.status, crate::preview::PreviewStatus::Loading(_)) {
					picture.load_failed(
						"The session this file was opened from closed before it finished loading."
							.to_owned(),
					);
				}
			}
		}
	}

	/// Say that the load could not happen at all, because the parent went away before it was asked
	/// (§32, §53). The editor also loses Save — there is no channel to save through — while the
	/// picture has no Save to lose, so the sentence in place of the image is its whole story.
	fn parent_gone(&mut self, reason: String) {
		match self {
			Self::Editor(editor) => {
				editor.mark_parent_gone();
				editor.load_failed(reason);
			}
			Self::Picture(picture) => picture.load_failed(reason),
		}
	}
}

/// Everything a target remembers, read out of the shared list in ONE borrow (§14, §22, §27).
///
/// A connection arriving is the most consequential moment in the app: it is where a saved target,
/// a remembered layout, a saved set of forwards and a possibly-stored secret all meet a live shell.
/// All four used to be read inline in the `Connected` arm, in three separate borrow scopes of the
/// shared target cell, arranged in a fixed order — with two comments whose entire subject was that
/// ordering. The ordering existed only because the reads were interleaved with `&mut self` calls
/// that want the same cell.
///
/// Reading everything first and acting afterwards makes the borrow discipline a property of one
/// function rather than a rule the caller has to keep in its head.
#[derive(Debug, Default)]
struct Arrival {
	/// The target's key in the saved list — what pre-selects its row for a return to the home list.
	key: String,
	/// The session this endpoint was last left in (§22), if it has been connected to before.
	session: Option<crate::targets::SessionState>,
	/// The port forwards saved against it (§27), to be re-established once the shell is up.
	forwards: Vec<crate::forward::ForwardSpec>,
}

/// Who has the keyboard right now (§10, §14, §17, §18, §27, §35).
///
/// Something on screen is often holding the keyboard: a dialog, a field being typed into, a
/// question waiting for a button. While one of them holds it, a key press must reach IT and nothing
/// else — most of all not the remote, because a rename field and the shell prompt would otherwise
/// receive the same keystroke, and renaming a folder would be typing at the remote at the same time.
///
/// That rule was already obeyed, by a run of seven `if` blocks — all the same shape, "if this thing
/// is up, handle Escape and return" — spread across two key handlers. It worked, but the PRIORITY
/// was nowhere written down: it was the source order of the blocks, so swapping two of them silently
/// changed which one wins, and no test could name a pair because there was nothing to name. This
/// enum is that priority made into a value, so [`Tab::keyboard_claim`] can be asked and answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyboardClaim {
	/// Home: the "remove this target?" confirmation (§14). Outranks the rename below it so a stray
	/// Enter cannot open a connection behind the modal.
	DeleteTarget,
	/// Home: the inline rename of a saved target (§14).
	TargetRename,
	/// Terminal: a dialog over the screen (§10, §17, §18, §27). The ones with a FIELD take the
	/// keyboard so their typing does not also reach the remote; the ones without take it so Ctrl+C
	/// copies the message rather than sending ETX down the channel.
	Modal,
	/// Terminal: one of the transfer flow's questions (§17, §21) — asked of the queue, which is the
	/// only thing that knows what it is holding.
	Transfers,
	/// Terminal: the folder tree's inline rename (§18).
	TreeRename,
	/// Terminal: the files pane's inline rename (§19).
	PaneRename,
	/// Terminal: the scrollback find bar (§35). Ranked LAST on purpose, and it is the only claimant
	/// with an exception above it — see `on_key`, where Ctrl+Shift+F is allowed through so pressing
	/// it again refocuses the field rather than being swallowed by the bar it opened.
	Find,
}

/// One session's whole state — its screen, its connection, its terminal and panes, its
/// dialogs (§6). This used to BE the app; with tabs (§26) the app owns a `Vec<Tab>` and each
/// tab is one of these, fully independent: a tab can sit at the home list while another runs a
/// shell. Everything here is per-tab EXCEPT the two `Rc<RefCell<…>>` fields, which are shared
/// clones of the single app-wide target list and secret vault (see `App`).
#[expect(
	clippy::struct_excessive_bools,
	reason = "eight unrelated facts about six subsystems, not one state in disguise — see §111"
)]
#[derive(Debug, Default)]
pub struct Tab {
	/// This tab's stable identity, handed out by `App` and never reused (§26). It keys the
	/// tab's own SSH worker subscription and routes that session's events back to this tab.
	id: u64,
	/// Which screen is visible.
	pub screen: AppScreen,
	/// The saved connection targets shown on the home screen (§14, §26). A shared clone of the
	/// ONE app-wide list (loaded from disk at startup, kept sorted, re-saved on any change): a
	/// rename or delete in one tab's home screen is seen by every other, and there is a single
	/// file on disk. Targets only — never any secret material (§12).
	targets: Rc<RefCell<crate::targets::Targets>>,
	/// What is typed in the home screen's filter box (§49); empty means the whole list is on
	/// show. Per tab, like the selection below it: two regions of a split window are two places
	/// to be looking for two different machines, and one filter shared between them would move
	/// under a user who never touched it.
	home_filter: String,
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
	/// The target (no secret) captured when a connect is dialed, saved to `targets`
	/// once the session actually opens (§14). `None` between attempts so a failed or
	/// abandoned connect never persists a target.
	pending_target: Option<crate::targets::Target>,
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
	/// Dial as soon as this tab HAS a worker (§52) — set by a duplicate that has everything it needs
	/// and nothing to ask the user, cleared by the `Ready` that fires it.
	///
	/// A tab is born without a worker: one is started by the subscription list, which iced rebuilds
	/// only after the update that created the tab has returned, and it announces itself a moment
	/// later with `SshEvent::Ready`. So a copy cannot dial in the same breath as it is made — it
	/// tried, and got "SSH worker is not ready yet" for its trouble. The pre-filled form is what
	/// shows in the meantime, which is also the honest fallback if a worker never arrives.
	pending_connect: bool,
	/// The terminal emulator, alive only while a shell is open. `Some` from
	/// `Connected` until `Disconnected`; output bytes are fed into it and the
	/// Terminal screen renders its grid.
	terminal: Option<term::Terminal>,
	/// What this tab is VIEWING, when it is showing a remote file rather than running a session
	/// (§32, §53) — a text buffer or a picture, see [`Viewer`]. `Some` exactly while `screen` is
	/// `AppScreen::Viewer`, and that is now one invariant rather than the two it used to be: this was a
	/// pair of `Option` fields, `editor` and `preview`, each paired with a `AppScreen` variant of its
	/// own, so four values had to agree about a thing that is one thing.
	viewer: Option<Viewer>,
	/// The question the connect flow is holding over the form, `None` when it is holding none
	/// (§7, §8, §16). Each variant carries what answering it needs — the passphrase being typed,
	/// the interactive challenge and its answers, the vault's two fields and what its unlock
	/// resumes — so nothing is read from a field that might be left over from an earlier prompt.
	///
	/// While it is `Some` the form's own keyboard ring is off (see `subscription`): the prompt's
	/// fields type through the widget tree, and Tab / Enter belong to them. As six `AppScreen`
	/// variants that was six places that each had to remember.
	prompt: Option<Prompt>,
	/// Whether a passphrase has already been submitted this connection. The SSH task
	/// re-emits `NeedPassphrase` for both the first ask and a wrong-passphrase re-ask,
	/// so this flag is how the passphrase prompt knows to show its "incorrect" hint:
	/// if it is set when the prompt appears, the previous attempt was rejected (§7).
	/// Reset at the start of each connection attempt.
	///
	/// It is HERE rather than in `Prompt::Passphrase` because it outlives the prompt: the flag is
	/// set as the answer goes down the wire, and read when the re-ask builds the next prompt.
	passphrase_failed: bool,
	/// The `user@host:port` of the current session, shown in the terminal's status
	/// bar (§10). Set when a connection is dialed and cleared when it ends. Holds no
	/// secret, so it is safe in `Debug`.
	connection: Option<String>,
	/// Which local shell this tab is running, or `None` for an SSH session (§103).
	///
	/// Set beside `connection` when a Local bar button is pressed and cleared with it. It exists for
	/// the two features that are meaningless without a remote — the tunnels manager (§27) and shell
	/// integration (§17) — whose buttons are not offered while it is `Some`. The session task refuses
	/// both with their reason anyway, so this is about not asking rather than about safety: a button
	/// that can only ever answer "not here" is a button that should not be there.
	///
	/// It holds the KIND and not a `bool` because the kind is the thing worth knowing: it is what
	/// `connection` was built from, and the next thing to want it (an icon on the tab chip, a per-shell
	/// default directory) will want the kind rather than the fact.
	local: Option<crate::local::shells::ShellKind>,
	/// A Ctrl+D sent to a local shell that may not act on it, and what has come back since (§104).
	///
	/// `Some` from the moment the byte goes out until the shell's answer has been weighed. The answer is
	/// accumulated rather than judged chunk by chunk, because a nineteen-byte reply is free to arrive as
	/// two reads and half an echo is not an echo.
	eof_probe: Option<Vec<u8>>,
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
	hover_cell: ui::selection::ScreenSpot,
	/// The multi-click tally over the grid (§42): how many presses in a row landed on one cell, so a
	/// press knows whether it is a plain click, a word (double) or a line (triple). `mouse_area`
	/// reports presses one at a time and counts nothing itself.
	clicks: ui::selection::Clicks<ui::selection::ScreenSpot>,
	/// The scrollback find bar's state while it is open, `None` when closed (§35). Holds the query
	/// and the match list; the current match is shown as an ordinary `selection`, so the highlight
	/// and Copy paths need no notion of searching at all. While it is `Some` the bar owns the
	/// keyboard, so nothing typed into it also reaches the remote.
	search: Option<term::search::Search>,
	/// Output has landed since the find bar's match list was built, so the list describes a document
	/// that no longer exists (§44). Two things go wrong at once: the fresh output can hold hits the
	/// bar has never seen, and — once the scrollback is at its cap (§23) — every line that scrolls
	/// off renumbers the ones above it, so a stored match's absolute line (§40) points one line
	/// further from its text with each one.
	///
	/// It is a FLAG rather than a re-scan on the spot because a scan walks every retained line
	/// (`Terminal::find`), and a flood of output arrives as dozens of chunks per frame: scanning per
	/// chunk would spend the frame searching instead of drawing. Set by the output path, read one
	/// window frame later, and cleared by whatever rebuilds the list — which is also what stops the
	/// frame clock, exactly as the toast's dwell does (§10).
	search_stale: bool,
	/// The accounts this session is currently a shell for (§45), in the order they were opened —
	/// the one it authenticated as first, then each one elevated into. Empty until the shell opens.
	///
	/// Every entry but the one on screen carries its own parked `Workspace`: its grid, its
	/// scrollback, its selection and its find bar. So switching accounts is not re-purposing one
	/// terminal, it is putting a different one in front of the user — a build left running as `cme`
	/// keeps filling `cme`'s scrollback while root's shell is on screen, and switching back finds it
	/// where it was.
	identities: Vec<Identity>,
	/// Which of `identities` is on screen. Its workspace is the LIVE one — the `terminal`,
	/// `selection`, `search` fields above — which is why those fields stay where they are and only
	/// the ones off screen are parked; nothing in the thousands of lines that touch `self.terminal`
	/// has to learn about identities.
	identity: u64,
	/// The number the next elevated identity gets (§45). Never reused within a session, so a late
	/// event for a shell that has gone can never be mistaken for one about its replacement.
	next_identity: u64,
	/// The last pointer position, local to the grid, used to place the right-click
	/// context menu — a right-press carries no coordinates of its own (§10).
	pointer: iced::Point,
	/// The context menu's anchor when it is open, `None` when closed (§10).
	menu: Option<iced::Point>,
	/// The dialog open over the terminal screen, `None` when none is (§10).
	///
	/// One field, because one dialog: they share the body buffer below, and every one of them
	/// takes the keyboard while it is up. As four independent fields nothing but care kept two
	/// from being set at once, nothing closed one when the next opened, and three of them were
	/// missing from both the keyboard guard and the session teardown.
	modal: Option<Modal>,
	/// The body message of whatever dialog is currently open, held as `text_editor`
	/// content so the user can *select* it and copy the selection (§10). It is
	/// read-only in practice — `update` performs every action except an edit — and is
	/// reseeded each time a dialog opens. Only one dialog is ever visible — `modal` above is
	/// how that is now stated — so a single buffer serves them all.
	dialog_body: text_editor::Content,
	/// Where this tab's open dialog floats and whether it is being dragged (§10). Centred each
	/// time a dialog opens, then it follows the header. The same `ui::dialog::Card` the App-level
	/// overlay holds — the two differ only in the box they are measured against, and that box is
	/// an argument, not a second copy of the arithmetic.
	card: ui::dialog::Card,
	/// The last known size of the box this tab fills (§10, §48) — the OS window when the window is
	/// whole, this tab's REGION once it is split. Tracked from resize events so a dialog can be
	/// centred and clamped within the space the tab actually occupies.
	window_size: iced::Size,
	/// Moving bytes between here and the remote (§16, §17, §19, §21, §29): the batch being set
	/// up, the files, folders and downloads waiting their turn, the ONE transfer slot they all
	/// share, whichever question the flow is holding, and the last outcome the status bar shows.
	///
	/// Eighteen fields and twenty methods on this struct before it was a module of its own —
	/// which meant the one-transfer-at-a-time rule was spelled out at each of its six entrances,
	/// slightly differently every time, and a session teardown had to remember to clear twelve of
	/// them by hand (it missed six). `Tab` now says what the user did and asks `busy()`; the queue
	/// answers with `transfer::Effects`, which `apply` carries out.
	transfers: transfer::Queue,
	/// What the LAST session on this tab was still transferring when it ended (§16), or `None` —
	/// which is the ordinary case, since most sessions end with nothing moving.
	///
	/// It sits outside `transfers` on purpose: everything in the queue belongs to one session and
	/// is cleared with it, and this is the single thing that must outlive one. It names the
	/// endpoint it was made on, so the session that adopts it can refuse a resume point belonging
	/// to another machine. Spent by the first session to open afterwards, used or not.
	///
	/// It is deliberately NOT persisted with the target (§22): a partial and its source are facts
	/// about this machine's disk and that server's, right now, and an offer to append to one after
	/// a restart hours later trusts far more than the size comparison behind it can carry.
	unfinished: Option<transfer::Unfinished>,
	/// The two file panes — the folder tree beside the grid (§18) and the file pane under it
	/// (§19) — as one thing, because a good deal about them is true of the PAIR (§22).
	///
	/// Each pane still owns what only concerns it: its visibility, its size, its expansion state,
	/// its selection, its menu. `panes` owns what neither can answer alone — where the session is,
	/// what a deletion means, the remembered layout, and the `.*` toggle that filters both — and
	/// hands back the listings to ask for rather than reaching for a channel it does not have.
	/// Those rules used to sit in eighteen `app` methods that sequenced both models by hand.
	panes: crate::panes::Panes,
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
	/// The copy-confirmation toast currently showing, if any (§10). Set on every clipboard
	/// write and cleared once its dwell elapses; `None` the rest of the time. The timestamp
	/// inside it is the dwell clock — see `Snackbar`.
	snackbar: Option<Snackbar>,
	/// Where a duplicate opens (§52), or `None` for a tab that is nobody's copy.
	///
	/// Set once, when the chip menu makes the tab, and spent when the shell opens — where it is
	/// replayed as a `cd`, exactly as a remembered session's directory is (§22), and outranks it:
	/// the user asked for a copy of THIS shell, standing where it is standing now, not for another
	/// visit to wherever this target was left last time.
	carry_cwd: Option<Carry>,
	/// The shell cwd a reconnect is waiting to settle at (§22), or `None` when not resuming.
	/// Set on connect when a remembered terminal path is replayed as a `cd`: until the shell
	/// announces this exact directory, the files pane is pinned to its own remembered path so
	/// the login-then-`cd` announcements do not drag it off. Cleared the moment the shell
	/// reaches it, or when the user moves the shell themselves.
	resume_cwd: Option<String>,
	/// The unlocked secret vault (§16, §26), or `None` until the user unlocks it. A shared clone
	/// of the ONE app-wide vault: unlocking it in any tab unlocks it for all, so a return visit in
	/// another tab needs no re-prompt. Held so repeated stores/reads need no re-prompt; dropped
	/// when the app exits, wiping the decrypted secrets it carries. Lazy: a user who never opts in
	/// never has one.
	vault: Rc<RefCell<Option<crate::vault::Vault>>>,
	/// The secret captured at dial time to store once the connect succeeds (§16), with its
	/// endpoint. Set only when "Remember" is on and the secret is non-empty; taken and written
	/// on `Connected`, cleared if the connect never leaves. Persisting only on success means a
	/// wrong password is never saved.
	pending_remember: Option<(String, Secret)>,
	/// The elevation in flight (§47), or `None` when none is. See [`PendingElevation`] for why the
	/// answer to a credential question lives here rather than in the dialog or in the vault.
	pending_elevation: Option<PendingElevation>,
	/// This session's port forwards (§27), each an entry with its runtime id, spec and status.
	/// Populated on connect from the target's saved set and by the tunnels dialog; the ids key a
	/// forward to its `ForwardReady` / `ForwardFailed` event and to the `RemoveForward` command.
	forwards: Vec<crate::forward::ForwardEntry>,
	/// The next forward id to hand out (§27). Monotonic per tab, never reused, so a removed
	/// forward's late event can never land on a new one.
	next_forward_id: u64,
}

/// The dialog open over the terminal screen (§10), and whatever answering it needs.
///
/// This screen can put five questions to the user, and it can put only ONE at a time: they share
/// the tab's single body buffer and its single card, and each of them owns the keyboard while it
/// is up. As four separate fields — two bools and two `Option`s — that was a convention rather
/// than a fact, and the convention had holes: opening one left the others alone, three of the four
/// were absent from the keyboard guard (so naming a new folder also typed at the remote prompt),
/// and two were absent from the session teardown (so a delete confirmation could outlive the
/// server whose paths it was holding). One `Option` is all four rules at once.
#[derive(Debug)]
pub enum Modal {
	/// Ending this shell (§10). The Disconnect button only ever raises this; the teardown happens
	/// on the explicit confirm, so an accidental click cannot end a live session.
	Disconnect,
	/// A folder to be made inside `parent` — a tree folder that was right-clicked, or the files
	/// pane's own directory — with the name typed so far (§18).
	NewFolder { parent: String, name: String },
	/// The remote entries a delete confirmation is holding (§18): the paths that will be removed
	/// once the user confirms. Deleting is not undoable, so nothing is sent until then.
	Delete(Vec<String>),
	/// The port-forwards manager (§27) and its add form. The session's forwards themselves are
	/// NOT here — they outlive any number of opens and closes of this dialog.
	Forwards(ui::forward::ForwardForm),
	/// The accounts dialog (§47) and the elevation being asked for or answered. The session's
	/// accounts themselves are not here either — they are on the tab, and outlive the dialog.
	Elevate(ui::elevate::ElevateForm),
	/// Setting the remote's shell up to announce its working directory (§17), and how far that has
	/// got. Everything the dialog SAYS is in the shared body buffer, as every other dialog's text
	/// is; this carries only what the buttons need to act on.
	Integration(Integration),
}

/// How far the shell-integration errand has got (§17) — the state of the one dialog that is not a
/// question but a small conversation with the server: ask, look, write, report.
///
/// Each state decides which buttons the dialog offers, and only `Found` can act: an install needs
/// the file to write and the shell whose block goes in it, and neither is known until the server
/// has answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Integration {
	/// The probe is out; the dialog is waiting for the server to answer.
	Asking,
	/// The server answered. `shell` is `None` when it could not be established, or names fish,
	/// which announces its own directory — in both cases there is nothing to offer, and the body
	/// says why. `installed` decides whether the one action is Install or Remove.
	Found {
		shell: Option<crate::integration::IntegrationShell>,
		path: String,
		installed: bool,
	},
	/// A write is out; the dialog is waiting for it to land.
	Writing,
	/// The errand finished, one way or the other. The body holds what happened — the file that was
	/// written, or the server's own reason for refusing — so the only thing left to do is close.
	Done,
}

/// An elevation that has been asked for and has not yet succeeded or failed (§47).
///
/// It exists for one reason: to decide, once the answer is known good, whether it may be kept. The
/// dialog cannot hold this — it is dismissible, and an elevation started from a saved preference has
/// no dialog behind it at all — and the vault must not hold it yet, because a password that turns out
/// to be wrong must never be stored.
///
/// SECURITY. `answer` is the one place cmote holds a credential between sending it and learning
/// whether it worked. It is a `Secret`, so it is redacted in `Debug` and zeroized when this is
/// dropped, and it is dropped the moment the elevation resolves either way (§12).
#[derive(Debug)]
struct PendingElevation {
	/// The identity the elevation is opening.
	identity: u64,
	/// The account being become, which is half of the vault key (`vault::elevation_key`).
	account: String,
	/// Whether the user asked for the password to be remembered. `false` means the answer below is
	/// dropped on success rather than stored, and any password the vault already held for this
	/// account is forgotten — unticking the box is how a stored one is removed.
	remember: bool,
	/// Whether this elevation was started from the target's stored preference rather than from the
	/// dialog. It decides what a FAILURE does: a hands-free attempt that was refused must put the
	/// dialog up, or the session sits at the login account with nothing said.
	automatic: bool,
	/// The last answer given, held only until the elevation resolves. `None` before the first
	/// question and after the answer has been stored or dropped.
	answer: Option<Secret>,
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

/// What the fresh region of a split opens with (§48, §52).
///
/// A split is two steps with an OS question between them — how big is the monitor — so what the new
/// region is FOR has to survive the round trip. It rides in the message rather than in a field on
/// `App` for the same reason `pane` does: two of them could be in flight at once, and a field would
/// let the second overwrite the first's intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitSeed {
	/// The target list, which is what the strip's own split buttons ask for (§48): the point of
	/// asking for a split is almost always to connect somewhere else, and the list is where that
	/// starts.
	Home,
	/// The tab at this position in the strip being cut, moved across (§52).
	Move(usize),
	/// A second copy of the tab at this position in the strip being cut (§52).
	Duplicate(usize),
}

/// Where a duplicate opens, and the connection that answer belongs to (§52).
///
/// The endpoint travels with the directory because a path only means anything on one machine. A copy
/// that is dialed straight away spends this on its first `Connected` and the two always agree — but a
/// copy that stopped at the form is a form, and a form can be edited: change the host, press Connect,
/// and a directory carried from somewhere else would `cd` a shell into a stranger's filesystem. So the
/// carry names its own endpoint and is used only when the session that opened is that one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Carry {
	endpoint: String,
	cwd: String,
}

/// The chip menu that is open, if any (§52): which strip it was raised on, and the tab it acts on.
///
/// It lives on `App` rather than on a `Region` because it is drawn over the WHOLE window — a menu
/// clipped to its region would be cut off by the very seam it is offering to send the tab across.
///
/// It carries no anchor. Every other menu in cmote hangs from the pointer, but a right press
/// publishes no position (iced's `mouse_area` reports the button, not where it was) and the raw
/// stream that would supply one is asked for only while the window is split (§48) — it is the one
/// subscription that costs a message per pointer move, and switching it on for every window would
/// make an undivided one pay for a menu it opens once in a session. So this menu hangs from the
/// STRIP instead: the region's own top-left, just under the bar the chip sits in. The tree's menu
/// is anchored to its pane for the same reason (§18), it is always on screen, and it follows a
/// divider dragged while the menu is open, which a stored point would not.
#[derive(Debug, Clone, Copy)]
struct StripMenu {
	pane: pane_grid::Pane,
	index: usize,
}

/// Every event the app can react to. UI events come from widgets; `Ssh` events
/// are surfaced from the background tokio task via a subscription (§4).
#[derive(Debug, Clone)]
pub enum Message {
	// --- home screen: saved targets (§14) ---
	/// Open a blank connect form for a brand-new connection.
	HomeNewPressed,
	/// A button on the home screen's Local bar was pressed (§103): open a session on THIS machine
	/// running that shell. It carries the whole `IntegrationShell` — the program cmote resolved and the arguments
	/// it wants — rather than an index into the catalogue, so nothing re-searches the disk on the press
	/// and a stale index can never open the wrong program.
	///
	/// There is no form and no `Connecting` question to answer: the next event is `Connected`.
	HomeLocalPressed(crate::local::shells::LocalShell),
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
	/// The home screen's filter box changed (§49) — narrow the list to the rows it matches.
	HomeFilterEdited(String),
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
	/// The user clicked the certificate Browse button — open the native certificate picker (§7).
	BrowseCertPressed,
	/// The certificate picker closed: `Some(path)` if chosen, `None` if cancelled.
	CertFilePicked(Option<PathBuf>),
	/// The user clicked Clear beside the certificate — drop back to plain key auth (§7).
	ClearCertPressed,
	// --- form actions ---
	ConnectPressed,
	BackPressed,
	/// A key press on the connect form, used to move focus between inputs with
	/// Tab / Shift+Tab (§10). Wired only on the Connect screen; non-Tab keys are
	/// ignored here and still reach the focused input through the widget tree.
	FormKey(iced::keyboard::Event),
	// --- host-key confirmation (§8) ---
	/// Accept a first-contact (unknown) key: pin it and continue.
	AcceptHostKey,
	/// Reject a host-key prompt (unknown or changed): refuse the connection — the safe default,
	/// also emitted by the dialog's ✕ and a backdrop click.
	RejectHostKey,
	/// Override a CHANGED key for this session only, without touching `known_hosts` (§8).
	TrustHostKeyOnce,
	/// Override a CHANGED key by replacing the stale `known_hosts` entry with the new one (§8).
	ReplaceHostKey,
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
	// --- the connect form's elevation fields (§47) ---
	/// The "Become" field changed — the account this target's sessions should become. Named apart
	/// from the accounts dialog's `ElevateAccountEdited` because they edit different things: this one
	/// a preference on the form, that one an elevation being asked for right now.
	FormElevateAccountChanged(String),
	/// The form's program radios (`sudo` / `su`).
	FormElevateKindChanged(crate::elevate::ElevateKind),
	/// The form's "Become it on connect" toggle. Carries no state, like `RememberToggled` beside it.
	FormElevateOnConnectToggled,
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
	/// A file is being dragged over the window from the OS (§29): light the files pane as the
	/// drop target. iced reports the hover with no pointer position, so this carries none — every
	/// drop lands in the pane's own directory, so the fact of a hover is all that matters.
	FileHovered,
	/// The OS drag left the window without dropping (§29): put the pane's drop highlight out.
	FileDropLeft,
	/// A path was dropped onto the window from the OS (§29). One event PER PATH: a drop of five
	/// files arrives as five of these, and nothing says which is the last — so each is only
	/// gathered, and `FileDropSettled` below is what reads them.
	FileDropped(PathBuf),
	/// The frame after a drop landed (§29): the whole set of paths is known now, so decide what it
	/// is — a batch of files, one folder, or a mixture to decline — and start it.
	FileDropSettled,
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
	/// The scrollbar was dragged (§116); the payload is the viewport offset to move TO — 0 the live
	/// bottom, `history_size()` the oldest retained line. Absolute rather than a delta because the
	/// thumb follows the pointer: what a drag knows is where the view should be, and a delta would
	/// mean the widget keeping its own copy of the offset and drifting from the engine's clamping.
	/// Raised for the press, every move under it and nothing else, so a drag that runs off an end
	/// simply repeats the offset it clamped to.
	TerminalScrollTo(u16),
	/// The scrollbar was pressed / let go (§119). Carries nothing: the offset travels in
	/// `TerminalScrollTo` above and these two say only that a drag began or ended, which is what
	/// closes and opens the hand. The pair a tab chip spells `TabSelected`/`TabDropped` and a dialog
	/// header spells `DialogGrabbed`/`DialogReleased`; this is the same pair for the third handle.
	ScrollbarGrabbed,
	ScrollbarReleased,
	/// Copy the current selection to the system clipboard.
	CopyPressed,
	// --- scrollback find bar (§35) ---
	/// Open the find bar over the grid and focus its field — Ctrl+Shift+F. Raised again while it
	/// is already open, which simply refocuses the field.
	TermFindOpen,
	/// Close the find bar — Esc or its ✕. The current match stays SELECTED, so what was found can
	/// still be copied once the bar is out of the way.
	TermFindClose,
	/// The find field's text changed: re-scan the scrollback and reveal the newest match.
	TermFindQuery(String),
	/// Step to the next match: `true` toward the live prompt (↓, newer), `false` back into history
	/// (↑, older). Both wrap.
	TermFindStep(bool),
	/// A window-frame tick raised while output has landed under an open find bar (§44): rebuild the
	/// match list so the count and the washes describe the document as it is now. Carries no payload
	/// — the query is the bar's own — and, like `SnackbarTick`, is only subscribed to while there is
	/// something to do, so a bar over an idle shell costs no frames at all.
	TermFindRescan,
	/// A window-frame tick raised while SOME terminal is holding a synchronized update (§122):
	/// re-check the 150 ms a held frame is allowed and let it go once that has passed. Carries no
	/// payload — every terminal in the window is asked, because a background tab's held frame is
	/// exactly the one whose staleness would go unnoticed until it was switched to. Like the two
	/// ticks above it is subscribed to only while an update is actually being held, so an ordinary
	/// shell pays nothing for it.
	HeldUpdateExpired,
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
	/// The status bar's "Reveal" button (§19): the other direction — bring the files pane and the
	/// folder tree to the directory the SHELL is in. Carries no path for the same reason
	/// `SyncPressed` does not: the announced cwd is read when the press arrives, so it can never
	/// be a directory the shell has since left.
	RevealPressed,
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
	UploadClashResolved(transfer::ClashChoice),
	/// The user backed out of an upload confirmation or its collision question (Cancel / ✕ /
	/// backdrop / Esc) — nothing is sent.
	UploadCancelled,
	/// The status bar's ✕ on a running transfer (§16): stop the in-flight file and drop the rest
	/// of the batch. The partial it was writing is deleted, so a cancel is final.
	TransferCancelPressed,
	/// The status bar's Resume after an interrupted transfer (§16): relaunch it, appending only
	/// the bytes still missing, then carry on with any batch still queued behind it.
	TransferResumePressed,
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
	DownloadClash(transfer::ClashChoice),
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
	/// — the pane has a dozen interactions of its own, and burying them in this enum
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
	/// An event from one tab's SSH worker (§4, §26). The `u64` is the tab id it belongs to, so
	/// `App` feeds it to the right session even when that tab is in the background — a shell
	/// there keeps drawing while another is on screen.
	Ssh(u64, SshEvent),
	// --- tab strip (§26, §38). Mouse-only: click a tab to switch, drag one to move it along the
	// strip, "+" to open, "×" to close. ---
	/// The "+" button — open a new tab on the home screen and make it active.
	TabNew,
	/// A tab was pressed — make it active AND arm a strip drag (§38). One press does both, so
	/// rearranging tabs needs no separate handle: a press that never travels to another chip is
	/// simply a click. Payload: its position in the strip.
	TabSelected(usize),
	/// The pointer entered the chip at this strip position (§38). While a drag is armed, that chip
	/// is the slot the grabbed tab will drop into; with no drag in flight it is ignored.
	TabDraggedOver(usize),
	/// The pointer entered something grabbable while nothing is being dragged (§51): a tab chip, a
	/// dialog header, or whatever wears the hand next. The window shows the open hand.
	///
	/// It carries the handle's NAME (§52) — a tab's id, or `cursor::HEADER` for a dialog header —
	/// because the hand has to be able to notice a handle that vanished under the pointer, and a
	/// handle that is gone raises no exit of its own. Naming the claimant is what lets the frame
	/// itself say who is still there.
	GrabEntered(u64),
	/// The pointer left something grabbable (§51), naming which so it can only let go of its own
	/// claim — the chip being left must not cancel the chip being entered on the same move.
	GrabExited(u64),
	/// The pointer entered a CONTROL sitting on a grab handle (§52): a chip's "×", a dialog header's
	/// ✕. It carries the handle it sits on, and it wins — the hand gives way to whatever cursor that
	/// control asks for, because the pointer is over something to click rather than something to
	/// pick up.
	GrabControlEntered(u64),
	/// The pointer left that control — back onto the handle around it, or off both (§52).
	GrabControlExited(u64),
	/// The button was released over the strip (§38) — move the grabbed tab into the hovered slot, or
	/// do nothing when the press never travelled to another chip.
	TabDropped,
	/// The pointer left the strip mid-drag (§38) — abandon the move; the strip keeps its order.
	TabDragCancelled,
	/// A tab's "×" (or a middle-click) — close it (payload: its id). A live session first
	/// raises the Disconnect confirmation; an idle tab closes at once.
	TabCloseRequested(u64),
	/// The close confirmation for a live tab was accepted — disconnect it and drop it.
	TabCloseConfirmed,
	/// The close confirmation was dismissed — keep the tab.
	TabCloseCancelled,
	// --- a chip's own menu: send this tab to another area of the window (§52) ---
	/// A chip was right-clicked — open its menu on the tab at this strip position. Arrives wrapped
	/// in `In`, which is what names the strip it was raised on; the pointer's window position is
	/// read from `App`, since a right press carries none of its own.
	TabMenuOpened(usize),
	/// Dismiss the chip menu without choosing an item.
	TabMenuDismissed,
	/// Menu "Move to … area" — take the tab the menu is open on out of its strip and put it in the
	/// named area, cutting the window first if that area does not exist yet.
	TabMoveTo(ui::split::Area),
	/// Menu "Duplicate to … area" — open a second tab on the same endpoint there, dialing it and
	/// carrying the source shell's directory over so the copy opens where the original is standing.
	TabDuplicateTo(ui::split::Area),
	// --- the window's split regions (§48). Mouse-only, like the strip the buttons live on. ---
	/// A message raised by ONE region's widgets, carrying the region it came from.
	///
	/// `view` wraps everything a region draws in this, so an event is applied WHERE IT HAPPENED
	/// rather than wherever the keyboard is. That is not a tidiness point: a left press inside an
	/// unfocused region produces two messages — the press itself and the focus change — and the press
	/// arrives FIRST, because `pane_grid` lets a region's own widgets see an event before it looks at
	/// it. Routed by focus, that press would land in the previously focused region's terminal,
	/// clobbering a selection there and starting a drag nobody asked for.
	///
	/// Boxed because a `Message` cannot contain itself by value.
	In(pane_grid::Pane, Box<Message>),
	/// A strip's split button — cut the region that strip belongs to in two, and open a fresh region
	/// beside it or below it. Arrives wrapped in `In`, which is what names the region to cut.
	Split(ui::split::Way),
	/// The second half of a split, once the monitor has been measured: `size` is the grown window
	/// clamped to the screen, `window` is the window to ask for it, and `pane` is the region to cut —
	/// carried rather than re-read from the focus, which could have moved while the OS was being
	/// asked. `seed` says what the fresh region opens with (§52).
	SplitSized {
		pane: pane_grid::Pane,
		way: ui::split::Way,
		window: iced::window::Id,
		size: iced::Size,
		seed: SplitSeed,
	},
	/// A left press landed in this region — it takes the keyboard. Raised by `pane_grid` itself, so
	/// it arrives unwrapped and already naming its region.
	SplitFocused(pane_grid::Pane),
	/// A divider was dragged — the two regions either side of it re-share their room, as a ratio
	/// rather than a pixel count so the share survives a window resize.
	SplitResized {
		split: pane_grid::Split,
		ratio: f32,
	},
	/// The pointer moved, in window coordinates. Raised from the raw event stream, and ONLY while
	/// the window is split (§48) — it exists to give the press below somewhere to have landed.
	PointerMoved(iced::Point),
	/// A left button went down somewhere in the window. Carries no position, because iced's raw
	/// press event has none: `PointerMoved` above is the position, and this is the moment (§48).
	PointerPressed,
	// --- quitting cmote (§30): the last-tab close or the OS window's × ---
	/// Quit was requested — from the window's title-bar × or from closing the last tab. Raises
	/// the "Quit cmote?" confirmation; nothing is torn down until it is accepted.
	QuitRequested,
	/// The "Quit cmote?" confirmation was accepted — disconnect every session cleanly, then exit.
	QuitConfirmed,
	/// The "Quit cmote?" confirmation was dismissed — stay open (inert once draining has begun).
	QuitCancelled,
	/// A frame tick while draining (§30): re-checks the drain timeout so a wedged session cannot
	/// hold the process open. Carries no payload — `update` reads the drain's own age.
	QuitTick,
	// --- shell integration (§17): teaching a silent remote shell to announce its directory ---
	/// The terminal's context-menu "Shell integration…" — open the dialog and ask the server what
	/// its login shell's config looks like.
	IntegrationPressed,
	/// Write the block into the file the probe found. The path and shell are read from the open
	/// dialog when the press arrives rather than carried here, so the button can never write to a
	/// file the probe has since been re-run against.
	IntegrationInstall,
	/// Cut the block back out of that same file.
	IntegrationRemove,
	/// The dialog was dismissed (Close / ✕ / backdrop / Esc). Nothing in flight is cancelled — a
	/// write already sent lands whether or not the dialog is watching — but nothing is written
	/// either, since only the two buttons above send anything.
	IntegrationClosed,
	// --- port forwarding (§27): the tunnels dialog opened from the status bar ---
	/// The status bar's "Tunnels" button — open the port-forwards manager.
	ForwardsPressed,
	/// The tunnels dialog was dismissed (Close / ✕ / backdrop) — nothing is torn down.
	ForwardsClosed,
	/// The add form's kind selector changed (Local / Remote / Dynamic).
	ForwardKindSelected(crate::forward::ForwardKind),
	/// The add form's listen field changed.
	ForwardListenChanged(String),
	/// The add form's target field changed.
	ForwardToChanged(String),
	/// The add form's Add button (or Enter in a field) — parse, validate, and start the forward.
	ForwardAddPressed,
	/// A forward's row ✕ — tear that forward down (payload: its runtime id).
	ForwardRemove(u64),
	// --- accounts (§45, §47): the "Log in as…" dialog opened from the status bar ---
	/// The status bar's Account button — open the accounts dialog. This is the ONLY way in, which is
	/// the point: §45 spread the job over four controls and the rethink kept one (§47).
	AccountPressed,
	/// The dialog was dismissed (Close / ✕ / backdrop). Nothing in flight is cancelled — an
	/// elevation already sent goes on with its conversation — but a question that was outstanding
	/// goes unanswered, which the SSH side reads as an elevation that was abandoned (§45).
	ElevateClosed,
	/// The form's program selector changed (`sudo` / `su`).
	ElevateKindPicked(crate::elevate::ElevateKind),
	/// The account field changed.
	ElevateAccountEdited(String),
	/// "Do this on every connection to this target" was toggled — the stored preference (§14).
	ElevateOnConnectToggled(bool),
	/// "Remember the password" was toggled — the vault opt-in, and a deliberate relaxation of the
	/// rule that a sudo password lives in RAM only (§12, §16).
	ElevateRememberToggled(bool),
	/// The form's "Log in as…" button (or Enter in the account field) — vet the account and send the
	/// elevation.
	ElevateSubmitted,
	/// The answer field changed. Carried as a plain `String` because that is what a `text_input`
	/// gives; it becomes a `Secret` the moment it is sent, and the field is cleared with it.
	ElevateAnswerEdited(String),
	/// The answer's Send button (or Enter in the field) — write it to the elevating shell.
	ElevateAnswerSubmitted,
	/// An account's name in the dialog was clicked — put that identity's terminal on screen (§45).
	IdentitySelected(u64),
	/// An account's ✕ — end that elevated shell (§45). The login identity has no ✕: ending it is
	/// what Disconnect does.
	IdentityClosed(u64),
	// --- the in-tab viewers (§32, §53): a tab can show a remote file, not only run a session ---
	/// Open a remote file in a new VIEWER tab (payload: the parent session's id and the path).
	/// Raised by the files pane's open item or a file double-click; `App` creates the tab — a text
	/// editor, or a picture preview if the file is an image (§53) — then sends the read on the
	/// parent session's channel and routes the reply back by viewer id.
	ViewerOpen {
		session: u64,
		path: String,
	},
	/// Something happened in an editor buffer or its toolbar (§32). Nested like `Files` — an
	/// editor has enough interactions of its own to keep out of this enum's top level.
	Editor(crate::editor::EditorMessage),
	/// A keystroke while an editor tab is active (§32): the shortcuts (Ctrl+S save, Ctrl+Shift+S
	/// save as, Ctrl+W close). Typing itself reaches the text widget directly, not here.
	EditorKey(iced::keyboard::Event),
	/// A keystroke while a PREVIEW tab is active (§53). It claims two: Ctrl+W and Escape, both of
	/// which close the tab. There is nothing else to press on a picture.
	PreviewKey(iced::keyboard::Event),
	/// The editor tab `id` asked to flush its buffer to the network (§32). Raised by the tab
	/// after a Save / Save As; handled by `App`, which alone can reach the parent's channel.
	EditorFlush(u64),
	/// The unsaved-editor close prompt's "Save & close" (§32): save, then close once it lands.
	EditorCloseSave,
	/// The unsaved-editor close prompt's "Discard" — close the tab and lose the edits (§32).
	EditorCloseDiscard,
	/// The unsaved-editor close prompt's "Cancel" — keep the tab (§32).
	EditorCloseCancelled,
	/// Close editor tab `id` now, unconditionally (§32): the auto-close once its "Save & close"
	/// finished writing.
	EditorCloseNow(u64),
	/// A picture finished decoding, off the GUI thread (§121).
	///
	/// The decode used to run inline where `FileLoaded` arrived, which held the window for up to
	/// ~280 ms on a 32-megapixel PNG — the largest `preview::MAX_ALLOC` lets through. Bounded, but
	/// seventeen dropped frames is a stutter, and it is the preview's half of the same complaint that
	/// `Content::with_text` was the editor's.
	///
	/// `bytes` is the FILE's size, carried across the decode rather than re-derived from it: the
	/// toolbar shows the number the files pane showed, not the decoded pixels', which is a bigger
	/// number the user has no way to recognise.
	PictureDecoded {
		viewer_id: u64,
		bytes: u64,
		decoded: Result<crate::preview::Decoded, String>,
	},
	/// The active editor's toolbar picked a colour scheme (§32). Handled by `App` — it sets the
	/// active editor's theme AND records the choice against the file's extension, so the memory is
	/// App-wide, not trapped in one tab.
	EditorThemeSelected(crate::editor::EditorTheme),
}

impl Tab {
	/// Build a fresh tab sitting on the home screen (§26), sharing the app-wide target list and
	/// vault handed in, and stamped with the current window size so a dialog it opens before the
	/// first resize is still centred (§10). `App::new` builds the first one; the "+" strip button
	/// builds each later one.
	fn home(
		targets: Rc<RefCell<crate::targets::Targets>>,
		vault: Rc<RefCell<Option<crate::vault::Vault>>>,
		id: u64,
		window_size: iced::Size,
	) -> Self {
		Self {
			id,
			targets,
			vault,
			window_size,
			// A window opens focused, and a program that enables focus reporting assumes the
			// same (§23); both are corrected by the first real change if the platform disagrees.
			window_focused: true,
			shell_focus_reported: true,
			..Self::default()
		}
	}

	/// Build a fresh EDITOR tab (§32): no session of its own, its buffer `Loading` until the parent
	/// session's channel delivers the file. `session` is the tab it was opened from, whose channel
	/// its loads and saves ride. The shared target list and vault are left at their defaults — an
	/// editor tab never shows the home screen, so it never reads them.
	fn new_editor(
		id: u64,
		session: u64,
		identity: u64,
		path: String,
		window_size: iced::Size,
		theme: crate::editor::EditorTheme,
	) -> Self {
		Self {
			id,
			screen: AppScreen::Viewer,
			viewer: Some(Viewer::Editor(crate::editor::Editor::loading(
				session, identity, path, theme,
			))),
			window_size,
			window_focused: true,
			shell_focus_reported: true,
			..Self::default()
		}
	}

	/// Build a fresh PREVIEW tab (§53): no session of its own, `Loading` until the parent session's
	/// channel delivers the picture. `session` is the tab it was opened from, whose channel its one
	/// read rides. It takes no identity and no theme — it never writes, and a photograph has no
	/// syntax to colour.
	fn new_preview(id: u64, session: u64, path: String, window_size: iced::Size) -> Self {
		Self {
			id,
			screen: AppScreen::Viewer,
			viewer: Some(Viewer::Picture(crate::preview::Preview::loading(
				session, path,
			))),
			window_size,
			window_focused: true,
			shell_focus_reported: true,
			..Self::default()
		}
	}

	/// Whether this tab is a VIEWER — an editor or a picture preview (§32, §53) — rather than a
	/// session. The property that matters is the one they share: no connection of its own, so no SSH
	/// worker is started for it.
	fn is_viewer(&self) -> bool {
		self.viewer.is_some()
	}

	/// The buffer this tab is editing, if it is editing one (§32).
	fn editor(&self) -> Option<&crate::editor::Editor> {
		self.viewer.as_ref().and_then(Viewer::editor)
	}

	/// The buffer this tab is editing, mutably.
	fn editor_mut(&mut self) -> Option<&mut crate::editor::Editor> {
		self.viewer.as_mut().and_then(Viewer::editor_mut)
	}

	/// Ask `App` to open `path` in a new viewer tab parented to THIS session (§32, §53). Raised by
	/// the files pane's open item and a file double-click; `App` creates the tab — an editor or a
	/// picture preview, by what the file is — and drives the load.
	///
	/// The kind is decided in ONE place, `App::open_viewer`, rather than here: both entry points
	/// send the same message, so a rule about which files are pictures cannot end up half-applied.
	fn request_open(&self, path: String) -> iced::Task<Message> {
		iced::Task::done(Message::ViewerOpen {
			session: self.id,
			path,
		})
	}

	/// The label this tab shows on its strip chip (§26): the connected endpoint once a shell is
	/// open (or dialing), otherwise a word for the screen it is sitting on. Names the session so a
	/// user with several open can tell them apart.
	fn strip_label(&self) -> String {
		match &self.screen {
			AppScreen::Terminal | AppScreen::Connecting { .. } => {
				let endpoint = self
					.connection
					.clone()
					.unwrap_or_else(|| "session".to_owned());
				// The icon name the remote set for this tab, if it set one (OSC 1, §69) — `vim`, a
				// build, a tmux window. It is what tells two shells on the SAME host apart, which
				// the endpoint alone cannot do.
				//
				// AFTER the endpoint, never in place of it. Same rule, and the same reason, as the
				// branch pill's (§55): the endpoint is what says which machine this is, so
				// remote-chosen text must never be readable as the start of it — a remote that
				// could rename its own chip could dress a staging box as production. Already
				// stripped of control characters and capped by `term::icon`, so this line draws it
				// and does not police it.
				match self.terminal.as_ref().and_then(term::Terminal::icon_name) {
					Some(icon) => format!("{endpoint} — {icon}"),
					None => endpoint,
				}
			}
			AppScreen::Home => "Home".to_owned(),
			// A viewer tab is named by its file, with a dot when there are unsaved edits — which
			// only an editor can have (§32, §53). Both halves of that are the viewer's own.
			AppScreen::Viewer => match &self.viewer {
				Some(viewer) => viewer.label(),
				None => "file".to_owned(),
			},
			// The connect form and every prompt over it are one "new connection" in progress —
			// except a failure, which is worth naming on the chip so a tab that fell over says so
			// without being opened.
			AppScreen::Connect => match self.prompt {
				Some(Prompt::Failed) => "Error".to_owned(),
				_ => "New connection".to_owned(),
			},
		}
	}

	/// The command-status dot for this tab's chip (§34), from its OSC 133 shell-integration marks.
	/// A running command wins over any past result; otherwise the last exit code decides ok vs
	/// failed. `None` when the tab runs no shell, or when the shell has announced no integration
	/// (no command has finished and none is running) — so the chip shows no dot at all.
	fn prompt_status(&self) -> Option<ui::tabs::TabStatus> {
		let terminal = self.terminal.as_ref()?;
		match terminal.command_state() {
			term::osc133::CommandState::Running => Some(ui::tabs::TabStatus::Running),
			// At a prompt or idle: the last command's exit code is what the dot reports, if one has
			// finished. A shell that never emits the `D` mark leaves this `None`.
			_ => match terminal.last_exit()? {
				0 => Some(ui::tabs::TabStatus::Ok),
				_ => Some(ui::tabs::TabStatus::Failed),
			},
		}
	}

	/// What this tab reports about work in flight (§54, §121), for the bar along the bottom of its
	/// chip. A live shell reports what its commands sent via OSC 9;4, which for most shells is
	/// nothing; a VIEWER tab reports how much of its file has arrived.
	///
	/// The two cannot collide — a tab is a session or a viewer, never both — so they share the bar
	/// rather than each getting its own strip of a 30-pixel chip. What the bar means is the same in
	/// both cases ("this tab is busy, this far through"), which is the test for whether sharing a
	/// channel is reuse or a pun.
	fn command_progress(&self) -> term::progress::Progress {
		if let Some(progress) = self.load_progress() {
			return progress.as_progress();
		}
		match self.terminal.as_ref() {
			Some(terminal) => terminal.progress(),
			None => term::progress::Progress::None,
		}
	}

	/// How far this tab's file read has got, if it is a viewer still loading one (§121).
	fn load_progress(&self) -> Option<crate::viewer::LoadProgress> {
		match self.viewer.as_ref()? {
			Viewer::Editor(editor) => editor.load_progress(),
			Viewer::Picture(picture) => picture.load_progress(),
		}
	}

	/// The read this tab is waiting on, as `(parent session id, this tab's id)` (§121) — what a close
	/// needs in order to cancel it. `None` for anything that is not a viewer mid-load, and for a
	/// viewer whose parent session has already gone, since there is nothing left to send down.
	fn loading_read(&self) -> Option<(u64, u64)> {
		self.load_progress()?;
		let parent = match self.viewer.as_ref()? {
			Viewer::Editor(editor) => {
				if editor.parent_gone {
					return None;
				}
				editor.session
			}
			Viewer::Picture(picture) => picture.session,
		};
		Some((parent, self.id))
	}

	/// The branch this tab's remote shell announced (§55), for the pill on its chip. `None` on a tab
	/// with no terminal, and on every shell that does not set iTerm2's `gitBranch` user variable —
	/// which is most of them, so most chips carry no pill.
	fn branch(&self) -> Option<String> {
		self.terminal.as_ref()?.branch().map(str::to_owned)
	}

	/// Whether this tab holds a live shell (§26). Closing one is confirmed like a Disconnect;
	/// closing a tab still at the home list or the connect form just drops it.
	fn is_live(&self) -> bool {
		matches!(self.screen, AppScreen::Terminal)
	}

	/// Whether this tab is an editor with unsaved edits (§32). Its "×" is confirmed like a live
	/// session's, so a stray click cannot lose the work.
	fn is_dirty_editor(&self) -> bool {
		self.editor().is_some_and(crate::editor::Editor::is_dirty)
	}

	/// Apply an editor-buffer message (§32): typing and the Save As prompt's own field are handled
	/// here; a Save / Save As confirm updates local state then asks `App` to flush the bytes, which
	/// alone can reach the parent session's channel.
	fn on_editor(&mut self, message: crate::editor::EditorMessage) -> iced::Task<Message> {
		use crate::editor::EditorMessage;
		let Some(editor) = self.editor_mut() else {
			return iced::Task::none();
		};
		match message {
			EditorMessage::Action(action) => {
				editor.perform(action);
				// Keep the cursor on screen after the move (§32). The buffer no longer scrolls itself on
				// EITHER axis (the gutter/horizontal trick), so both follows are driven here — the same
				// keep-it-visible math the panes use for a selected cell (§20), now applied on both axes.
				if let Some(task) = follow_editor_cursor_task(editor) {
					return task;
				}
			}
			EditorMessage::Scrolled {
				offset_x,
				offset_y,
				view_width,
				view_height,
			} => editor.set_viewport(offset_x, offset_y, view_width, view_height),
			EditorMessage::FindOpen => {
				// Open (or keep) the bar and focus its field so the user types straight away; if a query
				// was already there, jump to its current match too.
				let followed = editor.find_open();
				let focus = iced::widget::operation::focus(ui::editor::FIND_INPUT_ID);
				if followed && let Some(task) = follow_editor_cursor_task(editor) {
					return iced::Task::batch([focus, task]);
				}
				return focus;
			}
			EditorMessage::FindClose => editor.find_close(),
			EditorMessage::FindQueryChanged(query) => {
				if editor.find_query_changed(query)
					&& let Some(task) = follow_editor_cursor_task(editor)
				{
					return task;
				}
			}
			EditorMessage::FindStep(forward) => {
				if editor.find_step(forward)
					&& let Some(task) = follow_editor_cursor_task(editor)
				{
					return task;
				}
			}
			EditorMessage::ReplaceToggle => editor.replace_toggle(),
			EditorMessage::ReplaceChanged(text) => editor.replace_changed(text),
			EditorMessage::ReplaceOne => {
				if editor.replace_one()
					&& let Some(task) = follow_editor_cursor_task(editor)
				{
					return task;
				}
			}
			EditorMessage::ReplaceAll => {
				// Follow the cursor to the current match afterwards, like ReplaceOne / FindStep — the
				// rebuild re-selects it, so keep it on screen instead of leaving the view where it was.
				if editor.replace_all()
					&& let Some(task) = follow_editor_cursor_task(editor)
				{
					return task;
				}
			}
			EditorMessage::Save => {
				if editor.begin_save() {
					return iced::Task::done(Message::EditorFlush(self.id));
				}
			}
			EditorMessage::SaveAsStart => {
				editor.begin_save_as();
				// Focus the path field so the user types straight away, as the rename field does (§18).
				return iced::widget::operation::focus(ui::editor::SAVE_AS_INPUT_ID);
			}
			EditorMessage::SaveAsChanged(path) => editor.save_as_changed(path),
			EditorMessage::SaveAsConfirm => {
				if editor.save_as_confirm() {
					return iced::Task::done(Message::EditorFlush(self.id));
				}
			}
			EditorMessage::SaveAsCancel => editor.save_as_cancel(),
		}
		iced::Task::none()
	}

	/// The editor's keyboard shortcuts (§32): Ctrl/Cmd+S saves, Ctrl/Cmd+Shift+S saves as, Ctrl/Cmd+W
	/// closes. Typing itself reaches the text widget through the normal event path, so this listener
	/// acts only on the modified combinations and lets every other key fall through untouched.
	fn on_editor_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use crate::editor::EditorMessage;
		use iced::keyboard::key::Named;
		use iced::keyboard::{Event, Key};
		let Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};
		// Escape closes the find bar if it is open (§32), whatever holds focus — the field has no close
		// of its own. When the bar is closed, Escape does nothing here and falls through.
		if matches!(key, Key::Named(Named::Escape)) {
			let find_open = self.editor().is_some_and(|e| e.find.is_some());
			return if find_open {
				iced::Task::done(Message::Editor(EditorMessage::FindClose))
			} else {
				iced::Task::none()
			};
		}
		if !modifiers.command() {
			return iced::Task::none();
		}
		match key {
			Key::Character(c) if c.as_str().eq_ignore_ascii_case("s") => {
				let inner = if modifiers.shift() {
					EditorMessage::SaveAsStart
				} else {
					EditorMessage::Save
				};
				iced::Task::done(Message::Editor(inner))
			}
			Key::Character(c) if c.as_str().eq_ignore_ascii_case("w") => {
				iced::Task::done(Message::TabCloseRequested(self.id))
			}
			// Ctrl/Cmd+F opens the find bar and focuses it (§32).
			Key::Character(c) if c.as_str().eq_ignore_ascii_case("f") => {
				iced::Task::done(Message::Editor(EditorMessage::FindOpen))
			}
			// Ctrl/Cmd+H or Ctrl/Cmd+R opens the bar (if closed) and reveals its replace row (§32).
			Key::Character(c)
				if c.as_str().eq_ignore_ascii_case("h") || c.as_str().eq_ignore_ascii_case("r") =>
			{
				iced::Task::batch([
					iced::Task::done(Message::Editor(EditorMessage::FindOpen)),
					iced::Task::done(Message::Editor(EditorMessage::ReplaceToggle)),
				])
			}
			_ => iced::Task::none(),
		}
	}

	/// The preview's keyboard (§53): Ctrl/Cmd+W and Escape both close the tab, and nothing else is
	/// claimed. A picture has no text to type into and no state to change, so its whole keyboard is
	/// "I am done with this" — Escape as well as the editor's Ctrl+W, because a read-only thing
	/// opened with a double-click is one a user expects Escape to dismiss.
	fn on_preview_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;
		use iced::keyboard::{Event, Key};
		let Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};
		let close = match key {
			Key::Named(Named::Escape) => true,
			Key::Character(ref c) => modifiers.command() && c.as_str().eq_ignore_ascii_case("w"),
			_ => false,
		};
		if close {
			// Through the ordinary close path, not `force_close`: a preview is never dirty, so it
			// takes the plain branch, but routing it here keeps one way for a tab to leave (§26).
			return iced::Task::done(Message::TabCloseRequested(self.id));
		}
		iced::Task::none()
	}

	/// Apply a picture load reply routed here by id (§53). The bytes are decoded on ARRIVAL rather
	/// than at draw time — once, into a renderer handle — so a repaint never re-runs a decoder, and
	/// a file that turns out not to be a picture it can draw says so in place of the image.
	///
	/// The decode itself goes to a task rather than running here (§121). It used to run inline, with
	/// a `ponytail:` saying that was bounded and only worth moving if a real picture ever stuttered —
	/// so it was measured: ~280 ms for the largest picture `MAX_ALLOC` admits (about 32 megapixels),
	/// ~180 ms for a 30-megapixel JPEG, ~75 ms for an ordinary phone photograph. Seventeen dropped
	/// frames is a stutter, so it moved. iced's default executor is a thread pool, which is what makes
	/// this a `Task::perform` and not a new thread.
	///
	/// The tab stays `Loading` across the decode, which is honest — the wait is not over — and the
	/// view says "Decoding…" once the read is complete, so the two halves of the wait are told apart
	/// without a fourth status to keep in step.
	fn on_preview_event(&mut self, event: SshEvent) -> iced::Task<Message> {
		let viewer_id = self.id;
		let Some(picture) = self.viewer.as_mut().and_then(Viewer::picture_mut) else {
			return iced::Task::none();
		};
		match event {
			SshEvent::FileLoaded { bytes, .. } => {
				// The FILE's size, kept before the bytes are consumed: it is the number the files
				// pane showed, so it is the one the toolbar repeats back — not the decoded pixels',
				// which would be a bigger number the user has no way to recognise.
				let size = bytes.len() as u64;
				// The bar reaches its end here: everything has been read, and what is left is the
				// decode. Without this the chip would sit at whatever the last chunk reported.
				picture.set_progress(crate::viewer::LoadProgress {
					read: size,
					total: Some(size),
				});
				return iced::Task::perform(
					async move { crate::preview::decode_image(&bytes) },
					move |decoded| Message::PictureDecoded {
						viewer_id,
						bytes: size,
						decoded,
					},
				);
			}
			SshEvent::FileLoadProgress { read, total, .. } => {
				picture.set_progress(crate::viewer::LoadProgress { read, total });
			}
			SshEvent::FileLoadFailed { reason, .. } => picture.load_failed(reason),
			// A preview never asked for a save, so a save reply cannot be for it.
			_ => {}
		}
		iced::Task::none()
	}

	/// Apply a viewer load/save reply routed here by id (§32, §53). A successful load fills the
	/// buffer (or, if the bytes are not text in a supported encoding, shows the reason in its
	/// place); a successful save clears the marks and — after a "Save & close" — drops the tab.
	///
	/// A preview tab takes the same two load replies and none of the save ones, so it is answered
	/// first and separately rather than by threading an `Option` through the editor's arms.
	fn on_viewer_event(&mut self, event: SshEvent) -> iced::Task<Message> {
		if matches!(self.viewer, Some(Viewer::Picture(_))) {
			return self.on_preview_event(event);
		}
		let id = self.id;
		let Some(editor) = self.editor_mut() else {
			return iced::Task::none();
		};
		match event {
			SshEvent::FileLoaded { bytes, .. } => match crate::editor::decode_text(&bytes) {
				Some((text, encoding)) => editor.set_loaded(&text, encoding),
				None => editor.load_failed(
					"This file is not text in a supported encoding (UTF-8 or UTF-16).".to_owned(),
				),
			},
			SshEvent::FileLoadProgress { read, total, .. } => {
				editor.set_progress(crate::viewer::LoadProgress { read, total });
			}
			SshEvent::FileLoadFailed { reason, .. } => editor.load_failed(reason),
			SshEvent::EditSaved { path, .. } => {
				editor.path = path;
				// `mark_saved` answers whether this was a "Save & close": the write landed, so drop
				// the tab now (§32). A failed save answers nothing and keeps the tab, showing the
				// error — which is why the intent rides out of the model rather than being collected
				// from it afterwards.
				if editor.mark_saved() {
					return iced::Task::done(Message::EditorCloseNow(id));
				}
			}
			SshEvent::EditSaveFailed { reason, .. } => editor.save_failed(reason),
			// Not an editor event; nothing to do.
			_ => {}
		}
		iced::Task::none()
	}

	/// The heart of the Elm loop: apply one `Message` to the state. Returns a
	/// `Task` for any async follow-up work (none yet in the skeleton).
	#[expect(
		clippy::too_many_lines,
		reason = "the Elm loop's dispatch table, one arm per Message: length is 101 variants"
	)]
	fn update(&mut self, message: Message) -> iced::Task<Message> {
		match message {
			// --- home screen (§14) ---
			Message::HomeNewPressed => return self.open_form_new(),
			// A Local bar button (§103). It goes straight to the session — no form, because there is
			// nothing to fill in, and no host key or credential, because there is no other machine.
			Message::HomeLocalPressed(shell) => return self.dial_local(shell),
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
			Message::HomeFilterEdited(pattern) => self.on_home_filter(pattern),
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
				if let Some(path) = path {
					// Auto-fill the certificate from the OpenSSH `<key>-cert.pub` sibling when one
					// sits beside the key and no certificate is already chosen — the same
					// convenience the command-line client offers (§7). Non-destructive: a
					// certificate the user picked or a key with no sibling is left untouched.
					if self.form.cert_path.is_none()
						&& let Some(sibling) = crate::ssh::keyfile::cert_sibling(&path)
						&& sibling.is_file()
					{
						self.form.cert_path = Some(sibling);
					}
					self.form.key_path = Some(path);
				}
			}
			// Opening the certificate picker is async, like the key picker above.
			Message::BrowseCertPressed => return browse_cert(),
			// A cancelled picker keeps whatever was already chosen; a pick sets the certificate.
			Message::CertFilePicked(path) => {
				if path.is_some() {
					self.form.cert_path = path;
				}
			}
			// Clear drops the certificate back to plain key auth, undoing an auto-filled or
			// mistaken choice (§7).
			Message::ClearCertPressed => self.form.cert_path = None,
			Message::ConnectPressed => return self.on_connect_pressed(),
			Message::BackPressed => return self.go_to_form(),
			Message::FormKey(event) => return self.on_form_key(event),
			// Two buttons, one command, and that is the design rather than a coincidence:
			// `HostKeyChoice::Pin` means "write this key to `known_hosts`", and whether that LEARNS a
			// first-contact key or REPLACES a stale line is decided by the verdict the SSH side is
			// already holding (§8). The GUI does not get to choose which — it only says "pin it".
			Message::AcceptHostKey | Message::ReplaceHostKey => {
				self.on_host_key_decision(HostKeyChoice::Pin);
			}
			Message::RejectHostKey => self.on_host_key_decision(HostKeyChoice::Reject),
			Message::TrustHostKeyOnce => self.on_host_key_decision(HostKeyChoice::TrustOnce),
			// The prompts' own field edits (§7, §16). Each one goes to the open prompt or nowhere:
			// with the prompt closed there is no buffer to type into, which is the point of the
			// buffers living inside it.
			Message::PassphraseChanged(value) => {
				if let Some(Prompt::Passphrase(input)) = &mut self.prompt {
					*input = value;
				}
			}
			Message::PassphraseSubmitted => self.on_passphrase_submitted(),
			// Two dialogs, one meaning: the credential asked for cannot be given, so the handshake
			// waiting on it is over (§7).
			Message::PassphraseCancelled | Message::InteractiveCancelled => {
				return self.on_credential_cancelled();
			}
			Message::InteractiveAnswerChanged(index, value) => {
				if let Some(Prompt::Interactive { answers, .. }) = &mut self.prompt
					&& let Some(slot) = answers.get_mut(index)
				{
					*slot = value;
				}
			}
			Message::InteractiveSubmitted => return self.on_interactive_submitted(),
			Message::RememberToggled => self.form.remember = !self.form.remember,
			// The form's elevation fields (§47). Blanking the account is what withdraws the whole
			// preference, so nothing else has to be cleared with it — `ConnectForm::elevation` reads
			// the field as the gate.
			Message::FormElevateAccountChanged(value) => self.form.elevate_account = value,
			Message::FormElevateKindChanged(kind) => self.form.elevate_kind = kind,
			Message::FormElevateOnConnectToggled => {
				self.form.elevate_on_connect = !self.form.elevate_on_connect;
			}
			Message::VaultInputChanged(value) => {
				if let Some(Prompt::Vault { input, .. }) = &mut self.prompt {
					*input = value;
				}
			}
			Message::VaultConfirmChanged(value) => {
				if let Some(Prompt::Vault { confirm, .. }) = &mut self.prompt {
					*confirm = value;
				}
			}
			Message::VaultSubmitted => return self.on_vault_submitted(),
			Message::VaultCancelled => return self.on_vault_cancelled(),
			Message::Key(event) => return self.on_key(event),
			Message::WindowResized(size) => self.on_window_resized(size),
			Message::WindowFocus(focused) => self.on_window_focus(focused),
			// OS file drops (§29): a drag over the window lights the pane as the drop target, and a
			// drop uploads the file into it. Only a live session can be a target, so a hover with no
			// shell open lights nothing.
			Message::FileHovered => self.transfers.hover(self.terminal.is_some()),
			Message::FileDropLeft => self.transfers.hover(false),
			// One event per PATH, so nothing is decided here: the paths gather and the next frame
			// reads the whole drop at once (§29).
			Message::FileDropped(path) => self.transfers.caught(path),
			Message::FileDropSettled => {
				// The pane's directory is where a drop lands (§29); taken as an owned string so the
				// queue can be borrowed mutably to settle into it.
				let dir = self.panes.pane.path().map(str::to_owned);
				let effects = self.transfers.settle(self.terminal.is_some(), dir.as_deref());
				return self.apply(effects);
			}
			Message::DisconnectPressed => self.on_disconnect_pressed(),
			Message::DisconnectConfirmed => return self.on_disconnect_confirmed(),
			Message::GridMoved(point) => self.on_grid_moved(point),
			Message::GridPressed => self.on_grid_pressed(),
			Message::GridReleased => self.on_grid_released(),
			Message::GridRightPressed => self.menu = Some(self.pointer),
			Message::MouseReport(bytes) => self.on_mouse_report(bytes),
			Message::TerminalScroll(lines) => self.on_terminal_scroll(lines),
			Message::TerminalScrollTo(offset) => self.on_terminal_scroll_to(offset),
			// The thumb was picked up or put down (§119). The hand closes for the whole drag and
			// opens again on release, wherever the pointer has got to meanwhile — the same two lines
			// a dialog header's grab and release run, for the same reason (§51).
			Message::ScrollbarGrabbed => crate::cursor::set_dragging(true),
			Message::ScrollbarReleased => crate::cursor::set_dragging(false),
			Message::CopyPressed => {
				self.on_terminal_command();
				return self.on_copy_rich();
			}
			Message::TermFindOpen => return self.open_term_find(),
			Message::TermFindClose => self.search = None,
			Message::TermFindQuery(query) => self.term_find_query(query),
			Message::TermFindStep(newer) => self.term_find_step(newer),
			// A frame tick with output waiting under the bar (§44). Re-scanning clears the flag, which
			// removes the `frames()` subscription next diff — so the ticking stops on its own once the
			// output does, like the toast's.
			Message::TermFindRescan => self.rescan_find(),
			Message::LinkOpen(uri) => {
				self.on_terminal_command();
				self.follow_link(&uri);
			}
			Message::LinkCopy(uri) => {
				self.on_terminal_command();
				return self.copy_to_clipboard(uri);
			}
			Message::PastePressed => {
				self.on_terminal_command();
				return self.on_paste();
			}
			Message::Pasted(text) => self.on_pasted(text),
			Message::SyncPressed => self.on_sync(),
			Message::RevealPressed => self.on_reveal(),
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
			Message::UploadFilesPicked(files) => self.transfers.pick(files),
			// Started from a right-click surface: the folder is already known, so pick the
			// files and go straight to the confirmation.
			Message::UploadFilesPickedInto { files, dir } => {
				let effects = self.transfers.pick_into(files, dir);
				return self.apply(effects);
			}
			Message::UploadPressed => {
				// Started from the status bar, which names no folder: the shell's own cwd is the
				// destination, and the confirmation lets it be corrected before anything is sent.
				let dir = self
					.terminal
					.as_ref()
					.and_then(term::Terminal::cwd)
					.unwrap_or_default()
					.to_owned();
				self.menu = None;
				let effects = self.transfers.open_confirm(dir);
				return self.apply(effects);
			}
			Message::TerminalUploadPressed => {
				// The grid's right-click "Upload…": pick files for the shell's own directory.
				self.on_terminal_command();
				let dir = self
					.terminal
					.as_ref()
					.and_then(term::Terminal::cwd)
					.unwrap_or_default()
					.to_owned();
				return browse_upload_into(dir);
			}
			Message::UploadDestChanged(value) => self.transfers.set_dest(value),
			Message::UploadConfirmed => {
				let effects = self.transfers.send_batch();
				return self.apply(effects);
			}
			// One answer, two questions (§17, §21): only one of the two collision dialogs can ever
			// be open, so the queue reads off which it is holding rather than the message saying.
			Message::UploadClashResolved(choice) | Message::DownloadClash(choice) => {
				let effects = self.transfers.answer_clash(choice);
				return self.apply(effects);
			}
			Message::UploadCancelled => self.transfers.cancel_batch(),
			Message::TransferCancelPressed => {
				let effects = self.transfers.cancel();
				return self.apply(effects);
			}
			Message::TransferResumePressed => {
				let effects = self.transfers.resume();
				return self.apply(effects);
			}
			Message::Explorer(message) => return self.on_explorer(message),
			Message::Files(message) => return self.on_files(message),
			// The editor buffer's own interactions (§32): typing, Save / Save As, the prompt field.
			// The App has already peeled off the ones needing cross-tab reach (open / flush / close).
			Message::Editor(message) => return self.on_editor(message),
			Message::EditorKey(event) => return self.on_editor_key(event),
			// The picture tab's two keys (§53) — both of them "close this".
			Message::PreviewKey(event) => return self.on_preview_key(event),
			Message::DownloadTargetPicked { remote, local } => {
				let effects = self.transfers.download(remote, local);
				return self.apply(effects);
			}
			Message::DownloadFolderPicked { remotes, dir } => {
				let effects = self.transfers.download_into(remotes, dir);
				return self.apply(effects);
			}
			// Create / delete / recursive transfer (§18, §17, §19).
			Message::NewFolderNameChanged(value) => {
				if let Some(Modal::NewFolder { name, .. }) = &mut self.modal {
					*name = value;
				}
			}
			Message::NewFolderConfirmed => self.confirm_new_folder(),
			// Every dialog over this screen dismisses the same way: the modal closes and nothing
			// it was holding is acted on. That is what makes the ✕, the backdrop and Esc all safe.
			Message::NewFolderCancelled
			| Message::DeleteCancelled
			| Message::DisconnectCancelled
			| Message::IntegrationClosed
			| Message::ForwardsClosed
			// Dismissing the accounts dialog cancels nothing already sent (§47).
			| Message::ElevateClosed => self.modal = None,
			Message::DeleteConfirmed => self.confirm_remote_delete(),
			Message::TransferConflictResolved(choice) => {
				let effects = self.transfers.answer_conflict(choice);
				return self.apply(effects);
			}
			Message::UploadFolderPicked { local, dir } => {
				let effects = self.transfers.upload_tree(local, dir);
				return self.apply(effects);
			}
			Message::DownloadFolderTargetPicked { remote, local } => {
				// The refusal for this one is shown in the files pane it was started from, so the
				// guard sits here rather than inside the queue (§19).
				if self.transfers.busy() {
					self.panes.pane.set_notice(transfer::BUSY_NOTICE.to_owned());
					return iced::Task::none();
				}
				let effects = self.transfers.download_tree(remote, local);
				return self.apply(effects);
			}
			// Apply a selection/cursor action to the dialog body, but never an edit:
			// that keeps the message read-only while still selectable and copyable (§10).
			Message::DialogAction(action) => {
				if !action.is_edit() {
					self.dialog_body.perform(action);
				}
			}
			Message::DialogGrabbed => {
				self.card.grab();
				// Held: the hand closes until the release (§51), exactly as the overlay cards'
				// copy of this arm does — the two paths differ in WHICH card moves, not in what
				// the pointer is doing.
				crate::cursor::set_dragging(true);
			}
			// Measured against this tab's own box, which in a split window is its REGION and not
			// the whole window (§48) — the one thing that differs from the App's identical arm.
			Message::DialogDragged(pointer) => self.card.drag_to(pointer, self.window_size),
			Message::DialogReleased => {
				self.card.release();
				crate::cursor::set_dragging(false);
			}
			// `App` routes an SSH event to the tab that owns the session before delegating, so it
			// already picked the right `self`; the id is not needed again here (§26).
			Message::Ssh(_id, event) => return self.on_ssh_event(event),
			// The hand cursor is the WINDOW's, so `App` answers these too (§51) — a tab's own dialog
			// header raises them just as a chip does, and both mean the same thing to the pointer.
			Message::GrabEntered(_)
			| Message::GrabExited(_)
			| Message::GrabControlEntered(_)
			| Message::GrabControlExited(_)
			// Tab-strip management is `App`'s job — it intercepts these before delegating, so a
			// tab never sees them. The arms exist only to keep the match total (§26).
			| Message::TabNew
			| Message::TabSelected(_)
			| Message::TabDraggedOver(_)
			| Message::TabDropped
			| Message::TabDragCancelled
			| Message::TabCloseRequested(_)
			| Message::TabCloseConfirmed
			| Message::TabCloseCancelled
			// A chip's menu moves tabs between REGIONS (§52), which is a fact about the window that
			// no single tab can see, let alone act on.
			| Message::TabMenuOpened(_)
			| Message::TabMenuDismissed
			| Message::TabMoveTo(_)
			| Message::TabDuplicateTo(_)
			// The held-frame clock reaches every tab rather than the on-screen one (§122), so `App`
			// walks the tabs itself and calls the method; the message stops there.
			| Message::HeldUpdateExpired
			// The quit flow is `App`'s job too (§30) — a tab never sees these.
			| Message::QuitRequested
			| Message::QuitConfirmed
			| Message::QuitCancelled
			| Message::QuitTick
			// The viewers' cross-tab work is `App`'s job (§32, §53): opening a tab, flushing a save
			// through the parent's channel, and the unsaved-close prompt all need reach a tab lacks.
			| Message::ViewerOpen { .. }
			| Message::EditorFlush(_)
			| Message::EditorCloseSave
			| Message::EditorCloseDiscard
			| Message::EditorCloseCancelled
			| Message::EditorCloseNow(_)
			| Message::EditorThemeSelected(_)
			// The decode reply is routed by viewer id, which only `App` can resolve (§121).
			| Message::PictureDecoded { .. }
			// Splitting the window is `App`'s job too (§48): a tab has no idea it sits in a region,
			// and `In` is the wrapper `App` puts round these on the way OUT of `view` — a tab is
			// handed the message already unwrapped, so it never sees one.
			| Message::In(_, _)
			| Message::Split(_)
			| Message::SplitSized { .. }
			| Message::SplitFocused(_)
			| Message::SplitResized { .. }
			// The raw pointer stream belongs to the window, not to anything inside a region: it
			// exists to catch the press on a divider, which sits BETWEEN two regions (§48).
			| Message::PointerMoved(_)
			| Message::PointerPressed
			// A click swallowed by a dialog card: nothing to do — capturing it is the whole point
			// (it stops the click reaching the backdrop, §10). It sits with this group because the
			// body is the same and there is only one way to write "nothing".
			| Message::Ignored => {}
			// Shell integration (§17).
			Message::IntegrationPressed => self.open_integration_dialog(),
			Message::IntegrationInstall => self.write_integration(true),
			Message::IntegrationRemove => self.write_integration(false),
			// Port forwarding (§27).
			Message::ForwardsPressed => return self.open_forwards_dialog(),
			// Any edit to the add form clears the last parse error under it, so a stale complaint
			// never sits under a field the user has since fixed.
			Message::ForwardKindSelected(kind) => {
				if let Some(form) = self.forward_form_mut() {
					form.kind = kind;
					form.error = None;
				}
			}
			Message::ForwardListenChanged(value) => {
				if let Some(form) = self.forward_form_mut() {
					form.listen = value;
					form.error = None;
				}
			}
			Message::ForwardToChanged(value) => {
				if let Some(form) = self.forward_form_mut() {
					form.to = value;
					form.error = None;
				}
			}
			Message::ForwardAddPressed => self.add_forward(),
			Message::ForwardRemove(id) => self.remove_forward(id),
			// Accounts (§45, §47). The dialog is the only way in and the only way between.
			Message::AccountPressed => return self.open_accounts_dialog(),

			// Any edit clears the last complaint under the form, the same rule the add form above
			// keeps: a stale error must not sit under a field the user has since fixed.
			Message::ElevateKindPicked(kind) => {
				if let Some(form) = self.elevate_form_mut() {
					form.kind = kind;
					form.error = None;
				}
			}
			Message::ElevateAccountEdited(value) => {
				if let Some(form) = self.elevate_form_mut() {
					form.account = value;
					form.error = None;
				}
			}
			Message::ElevateOnConnectToggled(on) => {
				if let Some(form) = self.elevate_form_mut() {
					form.on_connect = on;
				}
			}
			Message::ElevateRememberToggled(on) => {
				if let Some(form) = self.elevate_form_mut() {
					form.remember = on;
				}
			}
			Message::ElevateSubmitted => return self.submit_elevation(),
			Message::ElevateAnswerEdited(value) => {
				if let Some(form) = self.elevate_form_mut()
					&& let ui::elevate::Stage::Answering { answer, .. } = &mut form.stage
				{
					*answer = value;
				}
			}
			Message::ElevateAnswerSubmitted => return self.send_elevate_answer(),
			Message::IdentitySelected(id) => return self.switch_identity(id),
			Message::IdentityClosed(id) => return self.close_identity(id),
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
		//
		// Written UNCONDITIONALLY, `None` included. An earlier attempt may have captured a secret
		// and then failed, and skipping this line when Remember is off would leave that capture in
		// place: the next successful connect would store the OLD host's password under the OLD
		// endpoint, with nothing ticked and no connection to it (§12, §16).
		self.pending_remember = if self.form.remember {
			extract_secret(&params.auth).map(|secret| {
				let endpoint = crate::targets::endpoint_of(&params.user, &params.host, params.port);
				(endpoint, secret)
			})
		} else {
			None
		};
		// A secret is in play, so the vault must be unlocked to store it. If it is not yet, defer
		// the connect behind the master-passphrase prompt and resume it on unlock.
		if self.pending_remember.is_some() && self.vault.borrow().is_none() {
			return self.open_vault_modal(VaultPending::Connect(params));
		}

		self.dial(params)
	}

	/// Send a validated `Connect` to the SSH task and move to the connecting screen (§10). Split
	/// from `on_connect_pressed` so the deferred-vault path can resume straight here once the
	/// master passphrase is entered (§16). Records the target (no secret) to save if the
	/// session opens (§14).
	fn dial(&mut self, params: bridge::ConnectParams) -> iced::Task<Message> {
		// Fresh attempt: no passphrase has been tried yet, so any upcoming prompt is
		// a first ask (no "incorrect" hint) until the user submits one (§7).
		self.passphrase_failed = false;

		// Capture the target (no secret) to save if this connect succeeds (§14). The
		// key path and certificate are only meaningful for key auth; the name here is a
		// placeholder — `upsert_on_connect` keeps an existing target's custom name.
		let (key_path, cert_path) = if self.form.auth_kind == ui::connect::AuthKind::Key {
			(self.form.key_path.clone(), self.form.cert_path.clone())
		} else {
			(None, None)
		};
		self.pending_target = Some(crate::targets::Target {
			name: crate::targets::endpoint_of(&params.user, &params.host, params.port),
			host: params.host.clone(),
			port: params.port,
			user: params.user.clone(),
			auth_kind: self.form.auth_kind,
			key_path,
			cert_path,
			// Placeholder like `name`: the stored preference wins on connect, and a
			// brand-new target takes the default `upsert_on_connect` gives it (§14).
			show_hidden: self.panes.show_hidden(),
			// The pending target only carries auth into `upsert_on_connect`; the remembered
			// session (§22), the remember flag (§16) and the saved forwards (§27) live with the
			// *stored* target, which the upsert leaves untouched, so these placeholders are never read.
			terminal_path: None,
			files_path: None,
			explorer_width: None,
			files_height: None,
			// A pending target's sort is a placeholder too: the stored target's remembered sort
			// wins on connect, and a brand-new one starts unsorted (§19, §22).
			sort: None,
			sort_dir: None,
			remember_secret: false,
			forwards: Vec::new(),
			// And a placeholder elevation for the same reason: what the form asked for is applied to
			// the STORED target after the connect succeeds (§47).
			elevate: None,
		});

		let status = format!("connecting to {}:{}…", params.host, params.port);
		// The label the terminal status bar will show once the shell is open (§10);
		// capture it now, before `params` moves into the command.
		let endpoint = format!("{}@{}:{}", params.user, params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			self.connection = Some(endpoint);
			self.screen = AppScreen::Connecting { status };
		} else {
			// The command never left, so there is no attempt for either capture to belong to.
			self.abandon_attempt();
		}
		iced::Task::none()
	}

	/// Where the file panes open when this session remembers nowhere (§22, §103).
	///
	/// A REMOTE session opens at `/`, because that is the top of the server and there is nothing better
	/// to say about a machine cmote has just met. A LOCAL one can do better: the shell is standing in
	/// the user's own folder from its first prompt, and opening the panes at the drive list would put
	/// two clicks between the session and the first folder anyone wants. So the two panes start where
	/// the shell already is.
	fn default_files_root(&self) -> String {
		if self.local.is_some() {
			crate::local::path::home()
		} else {
			explorer::ROOT.to_owned()
		}
	}

	/// Open a session on THIS machine (§103) — the home screen's Local bar.
	///
	/// The twin of [`dial`], and shorter for everything it does not have to do. There is no target to
	/// capture (a local shell is not a target: no host, no account, nothing to remember), no secret to
	/// store, and no passphrase state to reset — so `abandon_attempt` runs to drop anything a previous,
	/// abandoned connect attempt left behind rather than to prepare for this one.
	///
	/// What it does share with `dial` is the shape: send the command, and move to `Connecting` only if
	/// it left. The status is a full sentence rather than "connecting to …:22" because nothing is being
	/// connected to; it is on screen for a frame or two, until `Connected` arrives.
	fn dial_local(&mut self, shell: crate::local::shells::LocalShell) -> iced::Task<Message> {
		self.abandon_attempt();
		let status = format!("starting {}…", shell.kind.label());
		let endpoint = shell.endpoint();
		let kind = shell.kind;
		if self.send_command(SshCommand::ConnectLocal(shell)) {
			self.connection = Some(endpoint);
			self.local = Some(kind);
			self.screen = AppScreen::Connecting { status };
		}
		iced::Task::none()
	}

	/// This connection attempt is over without opening a session (§14, §16): drop the two things
	/// it was carrying on the promise that it would.
	///
	/// The target is only a target, but the SECRET matters. It is captured when Connect is
	/// pressed with Remember ticked and stored only when the session opens, so anything that ends
	/// the attempt in between has to drop it — otherwise a later successful connect finds it still
	/// there and stores it, under the endpoint it was captured for rather than the one that just
	/// succeeded (§12). One method, so a new way for an attempt to die cannot forget half of it.
	fn abandon_attempt(&mut self) {
		self.pending_target = None;
		self.pending_remember = None;
	}

	/// Open the master-passphrase prompt for the secret vault (§16), recording what to resume
	/// once it unlocks. The prompt is in CREATE mode (two fields) when no vault file exists yet,
	/// UNLOCK mode (one field) when it does — fixed here so the view need not re-check the disk.
	/// It shows over the connect form, so the caller has already put the form on screen.
	fn open_vault_modal(&mut self, pending: VaultPending) -> iced::Task<Message> {
		let creating = !crate::vault::Vault::exists();
		let body = if creating {
			ui::VAULT_CREATE_BODY
		} else {
			ui::VAULT_UNLOCK_BODY
		};
		self.open_prompt(
			Prompt::Vault {
				input: String::new(),
				confirm: String::new(),
				creating,
				failed: false,
				pending,
			},
			body,
		);
		iced::widget::operation::focus(ui::VAULT_INPUT_ID)
	}

	/// Handle the vault prompt's submit (§16). Creating: the passphrase must be non-empty and
	/// match its confirmation, else re-ask with the mismatch hint. Unlocking: a wrong passphrase
	/// (or an unreadable file) re-asks with the "not correct" hint — no oracle beyond that
	/// (§12). On success the unlocked vault is kept for the session and the pending action
	/// resumes. The typed values are taken (not copied) out of the fields so nothing lingers.
	fn on_vault_submitted(&mut self) -> iced::Task<Message> {
		// Taking the prompt takes the typed values with it: whatever happens next, the passphrase
		// is not left sitting in app state (§12). A re-ask below builds a fresh prompt.
		let Some(Prompt::Vault {
			input,
			confirm,
			creating,
			pending,
			..
		}) = self.prompt.take()
		else {
			return iced::Task::none();
		};

		let opened = if creating {
			// A new master passphrase must be non-empty and typed identically twice, so the one
			// value that protects everything can never be a typo the user cannot reproduce.
			if input.is_empty() || input != confirm {
				return self.reask_vault(creating, pending);
			}
			crate::vault::Vault::create(input)
		} else {
			crate::vault::Vault::unlock(input)
		};

		match opened {
			Ok(vault) => {
				*self.vault.borrow_mut() = Some(vault);
				self.resume_vault_pending(pending)
			}
			Err(error) => {
				// Wrong passphrase, or a damaged / unresolvable file: re-ask. The detail is
				// logged, never shown (§12).
				eprintln!("could not open the vault: {error:#}");
				self.reask_vault(creating, pending)
			}
		}
	}

	/// Ask again, with the "wrong / do not match" hint and empty fields (§16). The prompt is
	/// rebuilt rather than edited in place, so the rejected passphrase is dropped rather than
	/// left in the buffer the next attempt types over.
	fn reask_vault(&mut self, creating: bool, pending: VaultPending) -> iced::Task<Message> {
		self.prompt = Some(Prompt::Vault {
			input: String::new(),
			confirm: String::new(),
			creating,
			failed: true,
			pending,
		});
		iced::widget::operation::focus(ui::VAULT_INPUT_ID)
	}

	/// Resume whatever the vault unlock was blocking (§16): continue the deferred connect, or
	/// pre-fill the form's masked field from the now-readable secret. A `Prefill` whose entry is
	/// missing (the flag out of step with the vault) simply leaves the field blank.
	fn resume_vault_pending(&mut self, pending: VaultPending) -> iced::Task<Message> {
		match pending {
			VaultPending::Connect(params) => self.dial(params),
			VaultPending::Prefill(endpoint) => {
				// Read the secret in a short borrow that ends before the `&mut self` call: a held
				// `Ref` on the shared vault cell would clash with `fill_secret_field` (§26).
				let secret = self
					.vault
					.borrow()
					.as_ref()
					.and_then(|vault| vault.get(&endpoint).cloned());
				if let Some(secret) = secret {
					self.fill_secret_field(&secret);
				}
				self.go_to_form()
			}
		}
	}

	/// Dismiss the vault prompt (§16): the prompt goes, and the typed values and the deferred
	/// action go with it, leaving the connect form (populated behind the prompt in both flows).
	/// Cancelling never stores anything — the deferred connect and the pre-fill are simply
	/// abandoned; the user can still type the secret by hand.
	fn on_vault_cancelled(&mut self) -> iced::Task<Message> {
		self.prompt = None;
		// The connect this prompt was blocking is abandoned with it, and the secret it captured
		// goes too (§12, §16).
		self.abandon_attempt();
		self.screen = AppScreen::Connect;
		iced::Task::none()
	}

	/// Put a decrypted secret into the masked form field its auth method uses (§16): the
	/// password under password auth, the key passphrase under key auth. One endpoint has one
	/// stored secret and one auth kind, so the destination is unambiguous.
	fn fill_secret_field(&mut self, secret: &Secret) {
		match self.form.auth_kind {
			AuthKind::Password => secret.expose().clone_into(&mut self.form.password),
			AuthKind::Key => secret.expose().clone_into(&mut self.form.passphrase),
			// The promptless methods have no stored secret to fill — interactive types every
			// factor live and agent auth signs with a key the agent already holds (§7). A
			// remembered target is never one of these, so these arms are not reached in practice.
			AuthKind::Interactive | AuthKind::Agent => {}
		}
	}

	/// An answer went back and the handshake carries on (§7, §8): the question is closed and the
	/// status line says what is happening now. Said in one place because it is one fact — three
	/// prompts reach it, and a copy that forgot to close the prompt would leave the dialog on screen
	/// over a connection that had already moved on.
	fn authenticating(&mut self) {
		self.prompt = None;
		self.screen = AppScreen::Connecting {
			status: "authenticating…".to_owned(),
		};
	}

	/// Relay the user's host-key choice to the SSH task (§8): reject, trust once, or pin. Any
	/// choice but reject means the handshake proceeds, so we go back to a connecting status; on
	/// reject the refused handshake surfaces its own error and moves the screen.
	fn on_host_key_decision(&mut self, choice: HostKeyChoice) {
		let proceeding = choice != HostKeyChoice::Reject;
		if self.send_command(SshCommand::HostKeyResponse(choice)) && proceeding {
			self.authenticating();
		}
	}

	/// Send the typed passphrase to the SSH task (§7) and return to a connecting
	/// status. The text is moved straight into a `Secret` and the input field
	/// cleared, so no plain copy of the passphrase lingers in app state (§12).
	fn on_passphrase_submitted(&mut self) {
		// Taking the prompt takes the typed text with it and moves it straight into a `Secret`, so
		// no plain copy is left behind whether the send succeeds or not (§12).
		let Some(Prompt::Passphrase(input)) = self.prompt.take() else {
			return;
		};
		if self.send_command(SshCommand::Passphrase(Secret::new(input))) {
			// An attempt is now in flight. If the key does not unlock, the SSH task
			// re-asks and this flag makes the next prompt show its "incorrect" hint (§7).
			self.passphrase_failed = true;
			self.authenticating();
		}
	}

	/// Dismiss a credential prompt mid-handshake — the key passphrase (§7) or the server's
	/// keyboard-interactive challenge (§7). Both mean the same thing and did the same three lines
	/// twice: the prompt goes first, so the discarded text does not linger (§12); the half-done
	/// handshake is torn down, because there is no way to answer it later; and what the attempt
	/// captured is abandoned rather than left for a future connect to store (§16).
	///
	/// The vault prompt is NOT one of these: it is asked BEFORE anything is dialed, so there is no
	/// handshake to tear down — see `on_vault_cancelled`.
	fn on_credential_cancelled(&mut self) -> iced::Task<Message> {
		self.prompt = None;
		self.send_command(SshCommand::Disconnect);
		self.abandon_attempt();
		self.go_to_form()
	}

	/// Send the typed keyboard-interactive answers to the SSH task (§7) and return to a
	/// connecting status. Each answer is moved straight into a `Secret` and the buffers cleared,
	/// so no plain copy of an OTP or password lingers in app state (§12). The server drives what
	/// happens next: another prompt (the dialog reappears), success, or a generic failure.
	fn on_interactive_submitted(&mut self) -> iced::Task<Message> {
		// Taking the prompt takes the answers with it and moves each straight into a `Secret`, so
		// no plain copy of an OTP or password is left behind (§12).
		let Some(Prompt::Interactive { answers, .. }) = self.prompt.take() else {
			return iced::Task::none();
		};
		let answers: Vec<Secret> = answers.into_iter().map(Secret::new).collect();
		if self.send_command(SshCommand::Interactive(answers)) {
			self.authenticating();
		}
		iced::Task::none()
	}

	/// Send one command to the SSH task. Returns whether it was sent; a
	/// missing/closed channel becomes a visible error rather than a silent drop.
	/// `try_send` is non-blocking, so it is safe on the synchronous GUI thread.
	fn send_command(&mut self, command: SshCommand) -> bool {
		if let Some(sender) = &self.command_tx {
			match sender.try_send(command) {
				Ok(()) => true,
				Err(error) => {
					self.show_error(&format!("Could not reach the SSH worker: {error}"));
					false
				}
			}
		} else {
			self.show_error("SSH worker is not ready yet.");
			false
		}
	}

	/// Whether any terminal in this tab is holding a synchronized update (§122) — the question the
	/// subscription list asks to decide whether a frame clock is needed at all.
	///
	/// The parked identities are counted as well as the visible one, for the reason
	/// `App::release_held_updates` gives: a background shell can be mid-frame too, and no clock
	/// means its frame is held until the next byte arrives to push it out.
	fn holds_update(&self) -> bool {
		let visible = self
			.terminal
			.as_ref()
			.and_then(term::Terminal::held_update_expiry);
		visible.is_some()
			|| self.identities.iter().any(|entry| {
				entry
					.work
					.terminal
					.as_ref()
					.is_some_and(|terminal| terminal.held_update_expiry().is_some())
			})
	}

	/// Let go of this tab's held frames whose 150 ms has run out (§122), sending back whatever
	/// replies the released bytes asked for.
	///
	/// The two halves send by different routes, and that is the whole reason this is written out
	/// rather than looped: the visible identity's replies go down the typing path, and a parked
	/// identity's are addressed to THAT shell by name — the same split `SshEvent::Output` makes for
	/// a live chunk (§45), and for the same reason. A program blocked reading its own stdin is not
	/// helped by an answer sent to the account that happens to be on screen.
	fn release_held_updates(&mut self) -> iced::Task<Message> {
		if let Some(replies) = self
			.terminal
			.as_mut()
			.and_then(term::Terminal::release_held_update)
			&& !replies.is_empty()
		{
			self.send_command(SshCommand::Input(replies));
		}
		// Collected before any is sent: `send_command` needs `self` whole, and the walk above it
		// holds `self.identities` mutably.
		let parked: Vec<(u64, Vec<u8>)> = self
			.identities
			.iter_mut()
			.filter_map(|entry| {
				let replies = entry.work.terminal.as_mut()?.release_held_update()?;
				(!replies.is_empty()).then_some((entry.id, replies))
			})
			.collect();
		for (identity, bytes) in parked {
			self.send_command(SshCommand::Reply { identity, bytes });
		}
		iced::Task::none()
	}

	/// Carry out what a nudge to the transfer queue asked for (§17): send its commands, seed the
	/// shared dialog buffer for a question it opened, focus the destination field, re-list a folder
	/// something just landed in.
	///
	/// ONE place turns transfer effects into the rest of the app, which is what lets the queue
	/// itself reach for nothing — no SSH channel, no dialog buffer, no panes — and therefore be
	/// tested with none of them. A dead channel is already an error screen (`send_command` says
	/// so), and with no session there is nowhere left to send what was queued, so the queue drops
	/// it rather than firing at whatever this tab connects to next.
	fn apply(&mut self, effects: transfer::Effects) -> iced::Task<Message> {
		if let Some(body) = &effects.body {
			self.set_dialog_body(body);
		}
		for command in effects.commands {
			if !self.send_command(command) {
				// A dead channel is a dead session, so this is a teardown like any other: what was
				// moving is kept for the next connection, and the rest of the queue goes (§16).
				self.abandon_transfers();
				return iced::Task::none();
			}
		}
		if let Some(dir) = &effects.refresh {
			self.refresh_remote_dir(dir);
		}
		if effects.focus_dest {
			return iced::widget::operation::focus(ui::terminal::UPLOAD_INPUT_ID);
		}
		iced::Task::none()
	}

	/// The session is ending: keep whatever a later one could still finish, and let the queue
	/// forget the rest (§16).
	///
	/// Called from every teardown — a remote hangup, a session failure, a confirmed Disconnect, a
	/// worker channel that has died — because none of them says anything about the bytes already
	/// on the far side, which survive all four. A deliberate Disconnect is included on purpose: the
	/// ✕ beside the progress bar is how a transfer is *cancelled*, and the partial it leaves is
	/// deleted; leaving the server instead leaves the partial there, so offering to finish it beats
	/// leaving a half file behind with nothing said about it.
	///
	/// It reads `connection` and so must run BEFORE the teardown clears it — the endpoint is what
	/// stops the offer being made to the next machine this tab visits.
	fn abandon_transfers(&mut self) {
		let Some(endpoint) = self.connection.clone() else {
			return;
		};
		// Only ever overwritten by a real one: a session that ended with nothing moving must not
		// wipe the offer an earlier one left (a tab that reconnects, sits idle and drops again).
		if let Some(unfinished) = self.transfers.abandon(&endpoint) {
			self.unfinished = Some(unfinished);
		}
	}

	/// A shell has opened: offer to finish what the last session on this tab did not (§16). The
	/// queue itself decides whether this is the same server, and says nothing if it is not.
	///
	/// Taken whether or not it is used, exactly as a duplicate's carried directory is (§52), so
	/// the offer is spent by the FIRST session that opens afterwards. An endpoint of `None` cannot
	/// match a real one, so a session with no connection key simply drops it.
	fn adopt_unfinished(&mut self) {
		let Some(unfinished) = self.unfinished.take() else {
			return;
		};
		let endpoint = self.connection.clone().unwrap_or_default();
		self.transfers.adopt(unfinished, &endpoint);
	}

	/// Open a dialog over the terminal screen (§10). ONE way in, so every one of them gets the
	/// same four things: whatever was open closes (they share a body buffer and a card, and only
	/// one can be on screen), any context menu goes with it, the body is seeded, and the card is
	/// centred fresh rather than inheriting the last dialog's position.
	fn open_modal(&mut self, modal: Modal, body: &str) {
		self.menu = None;
		self.set_dialog_body(body);
		self.modal = Some(modal);
	}

	/// Load `text` into the dialog body buffer so the dialog about to open shows it as
	/// selectable, copyable content (§10). Called at each dialog-open transition; a
	/// fresh `Content` also resets any selection left from a previous dialog.
	fn set_dialog_body(&mut self, text: &str) {
		self.dialog_body = text_editor::Content::with_text(text);
		// A freshly opened dialog starts centred and not being dragged, so a position
		// left over from a previous dialog never carries across (§10).
		self.card = ui::dialog::Card::opened(self.window_size);
	}

	/// Show the error screen with `message`, also seeding it as the dialog's selectable
	/// body so the user can copy the failure text (§10, §12). Central so every error
	/// path (validation, a dead worker channel, a session failure) stays consistent.
	fn show_error(&mut self, message: &str) {
		self.open_prompt(Prompt::Failed, message);
		// A failure is shown over the connect FORM, wherever it came from — a validation slip on
		// the form, a dead worker channel, a session that dropped — so Back leaves the user
		// somewhere they can retry from rather than on a dead terminal screen (§10).
		self.screen = AppScreen::Connect;
	}

	/// Open a prompt over the connect form (§7, §8, §16), the mirror of `open_modal` on the
	/// terminal screen: whatever was being asked is replaced, the selectable body is seeded, and
	/// the card is centred fresh rather than inheriting the last prompt's position.
	fn open_prompt(&mut self, prompt: Prompt, body: &str) {
		self.set_dialog_body(body);
		self.prompt = Some(prompt);
	}

	/// React to an event from the SSH task. Returns a `Task` for any follow-up
	/// work — most events have none, but a freshly opened shell fetches the window
	/// size to fit its grid right away (§9).
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over SshEvent: length is the number of events, not depth"
	)]
	fn on_ssh_event(&mut self, event: SshEvent) -> iced::Task<Message> {
		match event {
			SshEvent::Ready(sender) => {
				self.command_tx = Some(sender);
				// A duplicate that was waiting for exactly this (§52): it had everything it needed
				// the moment it was made, but nothing to send it down. Now there is.
				if self.pending_connect {
					self.pending_connect = false;
					return self.on_connect_pressed();
				}
			}
			SshEvent::Connecting => {
				self.screen = AppScreen::Connecting {
					status: "connecting…".to_string(),
				}
			}
			SshEvent::HostKey(fingerprint) => {
				// Seed the selectable body with the explanation plus the fingerprint on
				// its own line, so the whole message — the fingerprint included — can be
				// selected and copied for out-of-band comparison (§8, §10).
				let body = format!("{}\n\n{fingerprint}", ui::HOST_KEY_DIALOG_BODY);
				self.open_prompt(Prompt::HostKey, &body);
				self.screen = AppScreen::Connect;
			}
			SshEvent::HostKeyChanged { stored, presented } => {
				// Seed the selectable body with the warning plus BOTH fingerprints, each labelled
				// and on its own line, so the whole block — what was trusted vs what was sent — can
				// be selected and copied for out-of-band comparison (§8, §10).
				let body = format!(
					"{}\n\nStored (trusted before):\n{stored}\n\nPresented (sent now):\n{presented}",
					ui::HOST_KEY_CHANGED_DIALOG_BODY
				);
				self.open_prompt(Prompt::HostKeyChanged, &body);
				self.screen = AppScreen::Connect;
			}
			SshEvent::NeedPassphrase => {
				// A fresh, empty buffer each time we ask — including a re-ask after a wrong
				// passphrase — so a stale attempt is never resent (§7, §12).
				self.open_prompt(
					Prompt::Passphrase(String::new()),
					ui::PASSPHRASE_DIALOG_BODY,
				);
				self.screen = AppScreen::Connect;
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
				// Start every field blank, one per prompt, and show the dialog. The server only
				// sends a request with at least one prompt here (an empty, message-only request
				// is answered by the SSH task itself), so focusing the first field is always apt.
				self.open_prompt(
					Prompt::Interactive {
						answers: vec![String::new(); prompts.len()],
						fields: prompts,
					},
					&body,
				);
				self.screen = AppScreen::Connect;
				return iced::widget::operation::focus(ui::interactive_field_id(0));
			}
			SshEvent::Connected => {
				let mut resume_terminal = None;
				let mut resume_files = None;
				let mut saved_forwards = Vec::new();
				if let Some(target) = self.pending_target.take() {
					// Everything this target remembers, in one read (§14, §22, §27) — see `Arrival`.
					let arrival = self.adopt_target(target);
					saved_forwards = arrival.forwards;
					// Restore the remembered session before the panes list anything (§22): the
					// `.*` filter and both pane sizes go on now, and the resume paths come back to
					// drive the cd / pane / tree restore below.
					if let Some(session) = arrival.session {
						(resume_terminal, resume_files) = self.restore_session(session);
					}
					self.settle_remembered_secret(&arrival.key);
					self.home_selected = Some(arrival.key);
					if let Err(error) = self.targets.borrow().save() {
						eprintln!("could not save targets: {error:#}");
					}
				}
				// A shell is open: spin up an emulator at the pty size we asked for,
				// show the terminal, then immediately refit it to the real window
				// rather than waiting for the first resize event.
				self.terminal = Some(new_emulator());
				self.clear_grid_interaction();
				// After the clear, never before: `clear_grid_interaction` empties the queue this
				// puts the resume offer back into (§16).
				self.adopt_unfinished();
				self.screen = AppScreen::Terminal;
				// This shell is the session's first identity (§45): the account it authenticated as.
				// It is the one that can never be elevated away or closed, and the one every other
				// identity falls back to.
				self.identities = vec![Identity {
					id: bridge::LOGIN_IDENTITY,
					// The login account's name is the endpoint's own `user@`, so it is not stored
					// again here (§47).
					account: None,
					ready: true,
					work: Workspace::default(),
				}];
				self.identity = bridge::LOGIN_IDENTITY;
				self.next_identity = 1;
				// And, if this target remembers one, the elevation it remembers (§47). Here because
				// this is the first moment a program can be run on the connection at all.
				self.elevate_on_connect();

				// A duplicate opens where the tab it was copied from is standing (§52), which
				// outranks whatever this target remembers: the user pointed at a shell, not at a
				// machine. The files pane is left on its own remembered directory either way — the
				// pin below holds it there until the shell settles, exactly as on a reconnect.
				//
				// Taken whether or not it is used, so a carry is spent by the first session either
				// way; kept only when THIS is the connection it was made for, since the form it
				// rode in on could have been pointed at another machine in between.
				if let Some(carried) = self.carry_cwd.take()
					&& self.connection.as_deref() == Some(carried.endpoint.as_str())
				{
					resume_terminal = Some(carried.cwd);
				}

				// Resume where the last session left off (§22), falling back to the root for a
				// first connection or a shell that never announced a cwd — the previous
				// behaviour. The pane opens at its own remembered directory; the tree opens the
				// chain down to it and selects it, so both panes start on the resume point.
				let files_start = resume_files.unwrap_or_else(|| self.default_files_root());
				let needed = self.panes.tree.reveal_if_new(&files_start);
				self.list_dirs(needed);
				if let Some(request) = self.panes.pane.show(&files_start) {
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

				// Re-establish the forwards this target saved (§27), now the connection is up.
				// Each is queued as `Starting` and asked for down the same channel; the server /
				// listener reports readiness or failure back as a `ForwardReady`/`ForwardFailed`.
				self.establish_forwards(saved_forwards);
				return fit_terminal();
			}
			// Output for an identity that is NOT on screen (§45): it goes into that identity's own
			// parked emulator and stops there. Nothing else in this arm applies — the cwd only
			// follows the pane the user is looking at, the find bar belongs to the visible grid, and
			// focus reporting describes where the keyboard actually is. A query the background shell
			// sent is still answered, because the program that sent it is blocked reading its stdin
			// until it is (§23) — and answered to THAT shell by name, not down the typing path,
			// which goes to whichever identity is selected.
			SshEvent::Output { identity, bytes } if identity != self.identity => {
				// An emulator is made here if that account has none yet, rather than the bytes being
				// dropped: an elevation's last words — the greeting and the first prompt, flushed as the
				// program hands the channel to the shell — can arrive before `IdentityReady` has been
				// acted on, and dropping them left a freshly elevated terminal blank (§45).
				let replies = self
					.identities
					.iter_mut()
					.find(|entry| entry.id == identity)
					.map(|entry| {
						entry
							.work
							.terminal
							.get_or_insert_with(new_emulator)
							.process(&bytes)
					})
					.unwrap_or_default();
				if !replies.is_empty() {
					self.send_command(SshCommand::Reply {
						identity,
						bytes: replies,
					});
				}
			}
			SshEvent::Output { bytes, .. } => {
				// A Ctrl+D is in flight at a local shell (§104), and this chunk is the answer. If the shell
				// echoed the byte back, nothing consumed it — so it is the shell's own EOF being ignored,
				// and the session ends here instead of the key doing nothing. Weighed BEFORE the bytes
				// reach the emulator, since the teardown drops it a line later either way.
				if self.judge_eof(&bytes) {
					return self.exit_the_local_shell();
				}
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
				// That chunk changed the document the find bar searched, so its match list is now a
				// description of an older one (§44). Marked, not re-scanned here: the scan walks every
				// retained line and this arm runs once per chunk of output, so a frame tick collapses a
				// burst of them into one. An empty query has no list to invalidate, and a closed bar has
				// no list at all — neither starts the clock.
				if self
					.search
					.as_ref()
					.is_some_and(|search| !search.query.is_empty())
				{
					self.search_stale = true;
				}
				// That chunk may have turned focus reporting on or off (§23); reconcile the
				// remote to the shell's true focus, so a program enabling `?1004` while a side
				// pane holds the keyboard is not left believing the shell is focused.
				self.report_focus();
				if let Some(cwd) = cwd {
					// While a reconnect is resuming (§22) BOTH panes are pinned to the
					// directories the restore put them on: the shell's login-then-`cd`
					// announcements must not drag either off until it has settled at the cwd we
					// replayed. Once it has, seed both follow-guards — so neither pane jumps
					// now but both *do* follow the next real `cd` — and stop pinning. Off the
					// resume path they follow the shell as usual (§18, §19): only a real move
					// re-lists.
					//
					// The tree sits INSIDE the pin rather than in front of it, which is the half
					// this used to miss. It followed every announcement while the pane was held,
					// so a resume walked it to the login directory and then on to the replayed
					// one — opening each chain in turn and asking the server for a listing of
					// every folder along both — to end up somewhere the pane had deliberately not
					// gone. A session is meant to open with the two panes agreeing, and one of
					// them was leaving before the user ever saw it there.
					match self.resume_cwd.as_deref() {
						Some(target) if target == cwd.as_str() => {
							self.panes.tree.set_revealed(&cwd);
							self.panes.pane.set_followed(&cwd);
							self.resume_cwd = None;
						}
						Some(_) => {}
						None => {
							let fetches = self.panes.follow(&cwd);
							self.send_fetches(fetches);
						}
					}
				}
			}
			SshEvent::IdentityReady { identity, factors } => {
				return self.on_identity_ready(identity, factors);
			}
			SshEvent::IdentityEnded { identity, reason } => {
				return self.on_identity_ended(identity, reason);
			}
			SshEvent::FilesChunk {
				request,
				entries,
				done,
			} => self.panes.pane.chunk(request, entries, done),
			SshEvent::FilesFailed { request, reason } => self.panes.pane.failed(request, reason),
			// The server's own timezone and one resolved symlink, both for the details
			// popup beside the selection (§20).
			SshEvent::Zone(zone) => self.panes.pane.set_zone(zone),
			SshEvent::LinkTarget { path, target } => self.panes.pane.set_link_target(path, target),
			// The four ways the one transfer slot can empty (§16, §17, §21). Which DIRECTION the
			// thing in it was going is not said here: the queue remembers, which is why an upload's
			// ending and a download's are one arm apiece rather than one pair apiece.
			SshEvent::DownloadDone(path) | SshEvent::UploadDone(path) => {
				let effects = self.transfers.ended(transfer::Ended::Done(path));
				return self.apply(effects);
			}
			SshEvent::DownloadFailed(message) | SshEvent::UploadFailed(message) => {
				let effects = self.transfers.ended(transfer::Ended::Failed(message));
				return self.apply(effects);
			}
			SshEvent::TransferInterrupted { message } => {
				let effects = self.transfers.ended(transfer::Ended::Interrupted(message));
				return self.apply(effects);
			}
			SshEvent::UploadExists(path) => {
				let effects = self.transfers.ended(transfer::Ended::Skipped(path));
				return self.apply(effects);
			}
			SshEvent::DirListed { path, dirs } => self.panes.tree.listed(&path, dirs),
			SshEvent::DirFailed { path, reason } => self.panes.tree.failed(&path, reason),
			SshEvent::RenameDone { from, to } => {
				// The entry moved: re-list its parent so the row reappears under the new
				// name, in the right sort position. Both panes may be showing it (§19).
				if let Some(parent) = self.panes.tree.renamed(&from, &to) {
					self.send_command(SshCommand::ListDir(parent));
				}
				if let Some(request) = self.panes.pane.renamed(&from) {
					self.list_files(request);
				}
			}
			SshEvent::MakeDirDone(path) => {
				// The new folder appeared inside its parent: re-list the parent in both panes so
				// it shows in the right sort position (§18). Take an owned parent to end the borrow.
				if let Some(parent) = explorer::parent(&path).map(str::to_owned) {
					self.refresh_remote_dir(&parent);
				}
			}
			SshEvent::DeleteDone(paths) => self.on_deleted(&paths),
			// A rename, a mkdir or a delete that failed. All three answer the same way and for the
			// same reason: the server's own words go on the pane's notice line, and NOTHING is
			// re-listed, because a failure changed nothing to re-read (§18, §19).
			SshEvent::RenameFailed(reason)
			| SshEvent::MakeDirFailed(reason)
			| SshEvent::DeleteFailed(reason) => self.panes.set_notice(reason),
			SshEvent::TransferConflict { name } => {
				let effects = self.transfers.conflicted(&name);
				return self.apply(effects);
			}
			SshEvent::UploadPrescan { collisions } => {
				let effects = self.transfers.prescan(collisions);
				return self.apply(effects);
			}
			SshEvent::TransferProgress { sent, total } => self.transfers.progressed(sent, total),
			// A forward came up or failed (§27): mark its row. A failure never tears the shell
			// down — the tunnel simply shows as failed in the dialog. A late event for a forward
			// already removed finds no entry and is dropped.
			// The shell-integration errand (§17). None of the three touches the session: the dialog
			// is the only thing that changes, and a reply for a dialog the user has closed is
			// dropped where it is handled.
			SshEvent::IntegrationProbed {
				shell,
				path,
				installed,
			} => self.on_integration_probed(shell, path, installed),
			SshEvent::IntegrationWritten { path, installed } => {
				self.on_integration_written(&path, installed);
			}
			SshEvent::IntegrationFailed(reason) => self.on_integration_failed(&reason),
			SshEvent::ForwardReady { id, assigned_port } => {
				self.mark_forward_ready(id, assigned_port);
			}
			SshEvent::ForwardFailed { id, reason } => {
				self.set_forward_status(id, crate::forward::ForwardStatus::Failed(reason));
			}
			// A connection opened or closed on a forward (§27): move its live gauge. A late event
			// for a forward already removed finds no row and is dropped.
			SshEvent::ForwardConnectionOpened { id } => self.bump_forward(id, true),
			SshEvent::ForwardConnectionClosed { id } => self.bump_forward(id, false),
			SshEvent::Disconnected => {
				// A remote hangup ends a live session too: remember where it was (§22).
				self.persist_session();
				// And what it was still transferring, so the next connection to this same server
				// can offer to finish it (§16). Before `connection` is cleared — that endpoint is
				// what the offer is matched against.
				self.abandon_transfers();
				self.abandon_attempt();
				self.terminal = None;
				self.forget_connection();
				self.clear_grid_interaction();
				self.forget_identities();
				return self.go_home();
			}
			SshEvent::Error(message) => {
				// Only saves when a shell had actually opened — an auth/handshake failure
				// reaches here with no terminal, and `persist_session` then does nothing (§22).
				self.persist_session();
				// A session that failed under a running transfer is the very case Resume is for
				// (§16): the bytes that reached the far side are still there.
				self.abandon_transfers();
				// A refused handshake is the commonest way an attempt dies: whatever it captured on
				// the promise of succeeding goes now, secret first (§12, §16).
				self.abandon_attempt();
				self.terminal = None;
				self.forget_connection();
				self.clear_grid_interaction();
				self.forget_identities();
				self.show_error(&message);
			}
			// An editor's load/save replies are routed by `App` straight to the editor tab that asked
			// (`on_viewer_event`, §32), so a session's own event stream never delivers them here.
			SshEvent::FileLoaded { .. }
			| SshEvent::FileLoadProgress { .. }
			| SshEvent::FileLoadFailed { .. }
			| SshEvent::EditSaved { .. }
			| SshEvent::EditSaveFailed { .. } => {}
			// A credential question from an elevating shell (§45), answered again since §47: into the
			// dialog if it is open, and into a dialog opened for it if it is not — which is what a
			// hands-free elevation from a stored preference looks like.
			SshEvent::ElevatePrompt {
				identity,
				label,
				refusal,
			} => return self.on_elevate_prompt(identity, label, refusal),
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
		// The files pane takes its height out of the grid — the terminal is full width now, the
		// tree sits under it (§18) — so the same call serves a window resize and the pane's own
		// resize (§19).
		let (rows, cols) = ui::terminal::grid_size(size, self.panes.pane.reserved());
		let changed = match self.terminal.as_mut() {
			Some(terminal) if terminal.screen().size() != (rows, cols) => {
				terminal.resize(rows, cols);
				true
			}
			_ => false,
		};
		if changed {
			// The grid the user was pointing at, selecting in and searching through is not the grid
			// that exists now (§43).
			self.on_grid_reflowed();
			self.send_command(SshCommand::Resize { cols, rows });
		}
	}

	/// Let go of what was anchored to the grid the resize just reflowed (§43).
	///
	/// The selection, the find bar's match list, the prompt ticks (§34) and the inline images (§41)
	/// all name positions in *absolute document lines* (§40), and a reflow moves them: re-wrapping the
	/// scrollback at a new width changes how many lines it holds, so a line number recorded before the
	/// resize names other text after it. `Terminal::resize` already drops the marks and the pictures
	/// for that reason — this is the same clean-up for the two things that live up here, and the one
	/// place a reflow's fallout is handled, so a pane resize (§19) cannot be fixed while a window
	/// resize stays broken.
	///
	/// The selection is **dropped** rather than mapped through the reflow. A highlight that survived
	/// onto unrelated text would be worse than none: Copy would put text on the clipboard that the
	/// user never selected, and nothing on screen would say so.
	///
	/// The find bar is **re-scanned** rather than dropped, which is what a step already does (§35).
	/// Its washes are rebuilt from the match list on every frame, so a stale list paints hits over
	/// whatever text the reflow moved onto those lines; a fresh scan of the same query is honest, and
	/// `refresh` keeps the current match by identity wherever it survived. The revealed match's own
	/// highlight goes with the selection above — the next step puts it back.
	fn on_grid_reflowed(&mut self) {
		self.selection = None;
		self.selecting = false;
		// The tally counts presses that land on ONE cell (§42), and that cell now shows different
		// text — so the next press there starts a fresh count instead of expanding a word the user
		// never clicked on once.
		self.clicks = ui::selection::Clicks::default();
		if let Some(terminal) = self.terminal.as_ref() {
			// The pointer has not moved, but the cell under it has: resolve it again from the last known
			// position against the new grid, exactly as a move would (§10). Without this a press that
			// arrives before the next mouse-move — a keyboard resize, a window snap — anchors at a row
			// the shrunken grid no longer has.
			self.hover_cell = ui::terminal::cell_under(&terminal.screen(), self.pointer);
		}
		self.rescan_find();
	}

	/// The Disconnect button (§10): open the confirmation modal instead of dropping
	/// the session immediately, so an accidental click cannot end a live shell. Also
	/// closes any open context menu so only the modal is shown. The teardown happens
	/// in `on_disconnect_confirmed` once the user confirms.
	fn on_disconnect_pressed(&mut self) {
		self.open_modal(Modal::Disconnect, ui::terminal::DISCONNECT_DIALOG_BODY);
	}

	/// Confirmed Disconnect (§10): tell the SSH task to tear down, then drop the local
	/// emulator and return to the form right away — the `Disconnected` event that
	/// follows just confirms what we have already done. Mirrors the passphrase-cancel
	/// path, which also acts immediately rather than waiting.
	fn on_disconnect_confirmed(&mut self) -> iced::Task<Message> {
		// Save where the shell and pane were before any of it is torn down (§22), and keep what
		// was still moving for the next connection to this server (§16) — leaving mid-transfer
		// leaves the partial on the far side either way.
		self.persist_session();
		self.abandon_transfers();
		// Before the emulator is dropped on the next line: `end_session` reads the grid to decide whether
		// typing at this shell is safe (§104), so the order of these two is not cosmetic.
		self.end_session();
		self.terminal = None;
		self.forget_connection();
		self.clear_grid_interaction();
		self.go_home()
	}

	/// Ask this tab's session to end, giving a LOCAL shell the chance to end itself first (§104).
	///
	/// Every teardown goes through here — the Disconnect button, Ctrl+D, a tab closing, cmote quitting —
	/// because the difference is not in why the session is ending but in what ending it means.
	///
	/// A **remote** needs nothing extra: `Disconnect` closes the SSH channel and the far side's shell gets
	/// a hangup it can act on, which is the protocol's own clean path. A **local** session has no protocol
	/// at all — the teardown underneath is `TerminateProcess` on the shell — so the shell is asked, in its
	/// own language, to leave. `exit` at a prompt runs whatever that shell runs on the way out: PSReadLine
	/// flushing its history, a `~/.bash_logout`, an exit trap the user wrote. The session task then waits a
	/// fraction of a second for it to go before killing it, so the kill becomes the fallback rather than
	/// the mechanism.
	///
	/// **Not while the alternate screen is up**, and that is the load-bearing half. `exit` is not a
	/// message, it is keystrokes: at a `vim` in normal mode `x` deletes the character under the cursor and
	/// `i` starts inserting, so the tidier teardown would edit the user's file on its way out. A session
	/// showing a full-screen program is therefore torn down the abrupt way, exactly as it was before this
	/// existed. A line-based program (a `node` REPL) is not detectable this way and gets the interrupt and
	/// the word as input, which it answers with an error and cmote follows with the kill — noisy in the
	/// scrollback nobody reads, and no worse than before.
	fn end_session(&mut self) {
		if self.local.is_some() && !self.on_alternate_screen() {
			self.send_command(SshCommand::Input(crate::local::shells::quit_sequence()));
		}
		self.send_command(SshCommand::Disconnect);
	}

	/// Weigh the shell's answer to a Ctrl+D that is in flight, and say whether the session should end
	/// (§104).
	///
	/// `false` whenever no Ctrl+D is outstanding, which is almost every chunk of output cmote ever sees —
	/// so the cost of this on the hot path is one `Option` test.
	///
	/// The answer is accumulated across chunks until the echo shows up or [`EOF_ANSWER_CAP`] bytes have
	/// gone by, and NOT settled on the first chunk. That distinction is the whole of a bug this shipped
	/// with: both PowerShells answer in **two** reads —
	///
	/// | chunk | bytes |
	/// |---|---|
	/// | 1 | `ESC[?25l` — six bytes, hiding the cursor. No echo, and no partial one either |
	/// | 2 | `ESC[93m^D…` — the echo, in PSReadLine's colour |
	///
	/// — so a rule that decided on chunk one read "some program answered the byte", disarmed, and left the
	/// echo to be drawn on screen with nothing else happening. Which is exactly what a user saw, and exactly
	/// what three earlier probes had missed by concatenating every chunk into one string before printing it.
	/// (`cmd` answers in a single two-byte read, so `cmd` worked. One shell out of three passing is what a
	/// wrong boundary assumption looks like from the outside.)
	///
	/// The budget is what stops a probe outliving its keypress. Running out means giving up, and giving up
	/// means the session stays — the safe direction, since every wrong answer here should read as "Ctrl+D did
	/// nothing" and never as "the session ended by itself".
	fn judge_eof(&mut self, bytes: &[u8]) -> bool {
		let Some(heard) = self.eof_probe.as_mut() else {
			return false;
		};
		heard.extend_from_slice(bytes);
		if heard
			.windows(EOF_ECHO.len())
			.any(|window| window == EOF_ECHO)
		{
			self.eof_probe = None;
			return true;
		}
		if heard.len() >= EOF_ANSWER_CAP {
			self.eof_probe = None;
		}
		false
	}

	/// Answer a Ctrl+D the shell handed straight back: run the shell's OWN `exit` (§104).
	///
	/// cmote tears nothing down here. It cancels the input line — which is carrying the `^D` the shell just
	/// echoed into it — and types `exit`, and then the shell does what `exit` does: runs its exit path,
	/// leaves, and the session ends because its shell ended. `Disconnected` arrives through the ordinary
	/// route and the tab lands on the home screen exactly as it does when the user types the word by hand.
	///
	/// That indirection is the whole point of it, and it buys three things the earlier version could not:
	///
	///   * **What ends is what echoed.** A `pwsh` started inside the tab's `pwsh` also echoes `^D`, and this
	///     ends THAT one, back to the outer prompt with the session intact. A version that ran the session's
	///     teardown would have closed the tab's shell from under a nested one.
	///   * **The shell is never killed on this path.** No `Disconnect`, so no 800 ms window and no
	///     `TerminateProcess` fallback: the word landed or nothing happened.
	///   * **Nothing happened is a real outcome.** If the shell refuses the word the session is simply still
	///     there — the safe direction this whole rule is built to fail in.
	///
	/// No confirmation card, and now for a plainer reason than before: this is not a teardown to confirm, it
	/// is four characters typed at a prompt. The Disconnect BUTTON keeps its modal.
	fn exit_the_local_shell(&mut self) -> iced::Task<Message> {
		self.send_command(SshCommand::Input(crate::local::shells::quit_sequence()));
		iced::Task::none()
	}

	/// Whether a full-screen program currently owns the grid (§104). No terminal at all counts as "no",
	/// which is the same answer for the purpose here: nothing is holding the keyboard.
	fn on_alternate_screen(&self) -> bool {
		self.terminal
			.as_ref()
			.is_some_and(|terminal| terminal.screen().is_alternate())
	}

	/// Forget which session this tab was showing (§10, §103): the label the status bar reads and, with
	/// it, which KIND of session it was.
	///
	/// One method rather than two assignments at each of the three endings, because the two are one
	/// fact — a tab with no connection is not a local one either — and an ending that cleared only the
	/// label would leave a `local` flag behind, hiding the Tunnels button on the next session this tab
	/// opens to a real server.
	fn forget_connection(&mut self) {
		self.connection = None;
		self.local = None;
		// A Ctrl+D whose answer never came is answered by the session ending (§104). Left behind, it
		// would weigh the FIRST chunk of the next session's output on this tab and could end that one.
		self.eof_probe = None;
	}

	/// Open the shell-integration dialog and ask the server what it is looking at (§17).
	///
	/// The dialog opens on the WAIT rather than after it: the probe is two or three round trips and
	/// opening only once it lands would leave the menu item feeling dead on a slow link. Nothing is
	/// written by this — the whole point of the dialog is that the block and the file it goes in are
	/// shown before anything happens.
	fn open_integration_dialog(&mut self) {
		self.open_modal(
			Modal::Integration(Integration::Asking),
			ui::terminal::INTEGRATION_ASKING_BODY,
		);
		// The account to look up in the remote's `/etc/passwd`. `connection` is the endpoint key
		// `user@host:port`, which is the only place the GUI still holds the login name once the
		// form has been left — and a username cannot contain an `@`, so the first one splits it.
		let user = self
			.connection
			.as_deref()
			.and_then(|endpoint| endpoint.split_once('@'))
			.map(|(user, _)| user.to_owned())
			.unwrap_or_default();
		self.send_command(SshCommand::ProbeIntegration { user });
	}

	/// Install or remove the block, on the file the probe found (§17).
	///
	/// The path and the shell are read out of the OPEN dialog rather than carried on the message:
	/// the only thing that can put them there is a probe that answered, so a button press can never
	/// name a file the server did not offer. A press in any other state does nothing, which is what
	/// makes a stray Enter harmless — including a shell cmote has no block for, where the dialog
	/// offers no button at all and this refuses to invent one.
	fn write_integration(&mut self, install: bool) {
		let Some(Modal::Integration(Integration::Found {
			shell: Some(shell),
			path,
			..
		})) = &self.modal
		else {
			return;
		};
		if !shell.installable() {
			return;
		}
		let path = path.clone();
		let shell = *shell;
		self.open_modal(
			Modal::Integration(Integration::Writing),
			ui::terminal::INTEGRATION_WRITING_BODY,
		);
		self.send_command(SshCommand::WriteIntegration {
			path,
			shell,
			install,
		});
	}

	/// The server answered the probe (§17): show what it found, and what can be done about it. A
	/// reply that arrives after the dialog was closed is dropped — the user asked and then left, so
	/// re-opening the dialog on their behalf would be the app talking over them.
	fn on_integration_probed(
		&mut self,
		shell: Option<crate::integration::IntegrationShell>,
		path: String,
		installed: bool,
	) {
		if !matches!(self.modal, Some(Modal::Integration(_))) {
			return;
		}
		self.set_dialog_body(&ui::terminal::integration_found_body(
			shell, &path, installed,
		));
		self.modal = Some(Modal::Integration(Integration::Found {
			shell,
			path,
			installed,
		}));
	}

	/// The write landed (§17). The file now says what it says; the session in front of the user is
	/// unaffected, because a shell reads its config at login and this one has already started.
	fn on_integration_written(&mut self, path: &str, installed: bool) {
		if !matches!(self.modal, Some(Modal::Integration(_))) {
			return;
		}
		self.set_dialog_body(&ui::terminal::integration_done_body(path, installed));
		self.modal = Some(Modal::Integration(Integration::Done));
	}

	/// The probe or the write did not happen (§17). Shown in the dialog rather than as a session
	/// error: this is a side errand, and a remote that refuses it is still a perfectly good remote
	/// to be typing at.
	fn on_integration_failed(&mut self, reason: &str) {
		if !matches!(self.modal, Some(Modal::Integration(_))) {
			return;
		}
		self.set_dialog_body(&ui::terminal::integration_failed_body(reason));
		self.modal = Some(Modal::Integration(Integration::Done));
	}

	/// Open the port-forwards manager (§27): the dialog opens centred with a blank add form, and
	/// the listen field takes the keyboard so a forward can be typed straight away. The form goes
	/// with the dialog, so reopening it never shows what a previous visit left half typed.
	fn open_forwards_dialog(&mut self) -> iced::Task<Message> {
		// The manager draws its own list; the shared body buffer has nothing to say for it, and is
		// seeded empty so no previous dialog's message lingers behind it.
		self.open_modal(Modal::Forwards(ui::forward::ForwardForm::default()), "");
		iced::widget::operation::focus(ui::forward::LISTEN_INPUT_ID)
	}

	/// The add form of the open tunnels dialog, or `None` when that is not what is open (§27).
	fn forward_form_mut(&mut self) -> Option<&mut ui::forward::ForwardForm> {
		match &mut self.modal {
			Some(Modal::Forwards(form)) => Some(form),
			_ => None,
		}
	}

	/// Add the forward described by the add form (§27): parse the two fields, reject a duplicate
	/// bind, then hand it a fresh id, queue it as `Starting`, ask the worker to start it, and
	/// save the updated set to the target. A parse error is shown under the form and nothing is
	/// sent. The listen/target fields are cleared on success so the next forward starts blank;
	/// the kind is kept, since adding several of one kind is common.
	fn add_forward(&mut self) {
		let Some(Modal::Forwards(form)) = &self.modal else {
			return;
		};
		let parsed = crate::forward::ForwardSpec::parse(form.kind, &form.listen, &form.to);
		let spec = match parsed {
			Ok(spec) => spec,
			Err(reason) => return self.refuse_forward(reason),
		};
		// Two forwards cannot bind the same local (or server) endpoint; refuse the duplicate
		// before it is sent, so the second one's inevitable bind failure never happens.
		if self
			.forwards
			.iter()
			.any(|entry| entry.spec.same_endpoint(&spec))
		{
			return self.refuse_forward("A forward already binds that address.".to_owned());
		}

		let id = self.next_forward_id;
		self.next_forward_id += 1;
		if self.send_command(SshCommand::AddForward {
			id,
			spec: spec.clone(),
		}) {
			self.forwards.push(crate::forward::ForwardEntry {
				id,
				spec,
				status: crate::forward::ForwardStatus::Starting,
				// Set only if this is a `-R 0` and the server later reports the port it chose.
				bound_port: None,
				// A fresh forward has carried nothing yet; the gauge fills as connections flow.
				open_count: 0,
				total_count: 0,
			});
			if let Some(form) = self.forward_form_mut() {
				form.listen.clear();
				form.to.clear();
				form.error = None;
			}
			self.persist_forwards();
		}
	}

	/// Show why an add was refused, under the form that asked for it (§27). Nothing is sent, and
	/// what was typed stays, so the reason names a field the user can still see.
	fn refuse_forward(&mut self, reason: String) {
		if let Some(form) = self.forward_form_mut() {
			form.error = Some(reason);
		}
	}

	/// Tear down the forward with this id (§27): drop it from the list, ask the worker to stop
	/// it, and save the shrunk set. An unknown id is a no-op.
	fn remove_forward(&mut self, id: u64) {
		let Some(index) = self.forwards.iter().position(|entry| entry.id == id) else {
			return;
		};
		self.forwards.remove(index);
		self.send_command(SshCommand::RemoveForward(id));
		self.persist_forwards();
	}

	/// Open the accounts dialog (§47) — the one way in.
	///
	/// The form opens from the target's SAVED elevation when it has one, so a return visit sees what
	/// the next connection will do and turning it off is one click rather than a re-type. With
	/// nothing saved it opens blank on `sudo`, which is what a sudoers-managed machine expects.
	fn open_accounts_dialog(&mut self) -> iced::Task<Message> {
		// A question already outstanding is not to be thrown away by re-opening: pressing Account
		// while sudo is asking must show that question, not a blank form over an elevation that is
		// still waiting for an answer (§47).
		if self
			.elevate_form_mut()
			.is_some_and(|form| form.is_answering())
		{
			return iced::widget::operation::focus(ui::elevate::ANSWER_INPUT_ID);
		}
		let saved = self
			.connection
			.as_deref()
			.and_then(|endpoint| self.targets.borrow().find(endpoint).cloned())
			.and_then(|target| target.elevate);
		let form = saved.as_ref().map_or_else(
			ui::elevate::ElevateForm::default,
			ui::elevate::ElevateForm::from_saved,
		);
		// The dialog draws its own list and form; the shared body buffer has nothing to say for it,
		// and is seeded empty so no previous dialog's message lingers behind it.
		self.open_modal(Modal::Elevate(form), "");
		iced::widget::operation::focus(ui::elevate::ACCOUNT_INPUT_ID)
	}

	/// The elevation form of the open accounts dialog, or `None` when that is not what is open (§47).
	fn elevate_form_mut(&mut self) -> Option<&mut ui::elevate::ElevateForm> {
		match &mut self.modal {
			Some(Modal::Elevate(form)) => Some(form),
			_ => None,
		}
	}

	/// The rows the accounts dialog lists (§47): every identity this session has, named, with the
	/// one on screen marked and every elevated one closable.
	///
	/// The login identity's name comes from the session's endpoint rather than from the identity —
	/// see [`Identity`] for why it is not stored twice.
	fn account_rows(&self) -> Vec<ui::elevate::AccountRow> {
		let login = self.login_account();
		self.identities
			.iter()
			.map(|identity| ui::elevate::AccountRow {
				identity: identity.id,
				label: match &identity.account {
					Some(account) => account.clone(),
					None => login.clone(),
				},
				selected: identity.id == self.identity,
				closable: identity.id != bridge::LOGIN_IDENTITY,
			})
			.collect()
	}

	/// The account the session authenticated as, read off its endpoint (§47). `user@host:port` up to
	/// the `@`, falling back to a plain word when there is no session — which the dialog cannot be
	/// open without, so the fallback is for the type rather than for the screen.
	fn login_account(&self) -> String {
		self.connection
			.as_deref()
			.and_then(|endpoint| endpoint.split('@').next())
			.unwrap_or("login")
			.to_owned()
	}

	/// The account whose terminal is on screen, for the status bar's button (§47). `None` for the
	/// login identity, which the bar's centred endpoint already names.
	fn showing_account(&self) -> Option<&str> {
		self.identities
			.iter()
			.find(|identity| identity.id == self.identity)
			.and_then(|identity| identity.account.as_deref())
	}

	/// Send the elevation the dialog is asking for (§47).
	///
	/// The account is vetted here and nowhere later: `elevate::valid_user` is the rule that keeps
	/// anything but a plain login name out of the command line `ElevateKind::command` composes, and
	/// this is the boundary the user's own text crosses (§12). A refused name is reported under the
	/// form and nothing is sent.
	fn submit_elevation(&mut self) -> iced::Task<Message> {
		let Some(form) = self.elevate_form_mut() else {
			return iced::Task::none();
		};
		// A conversation already running must not be restarted by a second press.
		if !matches!(form.stage, ui::elevate::Stage::Asking) {
			return iced::Task::none();
		}
		let account = form.account.trim().to_owned();
		if account.is_empty() {
			form.error = Some("Which account?".to_owned());
			return iced::Task::none();
		}
		if !crate::elevate::valid_user(&account) {
			form.error = Some(
				"An account is a plain login name — letters, digits, and `_ - .` (§12).".to_owned(),
			);
			return iced::Task::none();
		}
		let (kind, on_connect, remember) = (form.kind, form.on_connect, form.remember);
		form.error = None;
		self.start_elevation(&account, kind, remember, false);
		// The preference is stored on the way OUT, not on success: it says what the next connection
		// should try, and a refused attempt is still what the user asked for. The password is the
		// other way round — see `settle_elevation_secret` (§47).
		self.persist_elevation(&account, kind, on_connect);
		iced::Task::none()
	}

	/// Ask the session to become `account`, and record what has to be known when it resolves (§47).
	///
	/// `automatic` says whether this came from the target's stored preference rather than from the
	/// dialog, which is what decides how a FAILURE is reported: a hands-free attempt has no dialog
	/// behind it, so a refusal has to put one up.
	fn start_elevation(
		&mut self,
		account: &str,
		kind: crate::elevate::ElevateKind,
		remember: bool,
		automatic: bool,
	) {
		let identity = self.next_identity;
		if !self.send_command(SshCommand::Elevate {
			identity,
			kind,
			user: account.to_owned(),
		}) {
			return;
		}
		self.next_identity += 1;
		// Listed straight away, and NOT ready: a shell still elevating cannot be switched to, but it
		// has to be in the list for a failure to be reported against (§45).
		self.identities.push(Identity {
			id: identity,
			account: Some(account.to_owned()),
			ready: false,
			work: Workspace::default(),
		});
		self.pending_elevation = Some(PendingElevation {
			identity,
			account: account.to_owned(),
			remember,
			automatic,
			answer: None,
		});
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Waiting { identity };
		}
	}

	/// Write the answer the dialog is holding to the elevating shell (§47).
	///
	/// The typed text becomes a `Secret` here — the last point it is an ordinary `String` — and a
	/// COPY of it is kept in `pending_elevation` until the elevation resolves, which is the only way
	/// a password can be stored after being proved good rather than before (§12, §16).
	fn send_elevate_answer(&mut self) -> iced::Task<Message> {
		let Some(form) = self.elevate_form_mut() else {
			return iced::Task::none();
		};
		let ui::elevate::Stage::Answering {
			identity, answer, ..
		} = &mut form.stage
		else {
			return iced::Task::none();
		};
		let identity = *identity;
		// Taken, not cloned: the field is cleared as the answer leaves it, so the plaintext is not
		// left sitting in a widget behind the dialog.
		let secret = Secret::new(std::mem::take(answer));
		form.stage = ui::elevate::Stage::Waiting { identity };
		if let Some(pending) = self.pending_elevation.as_mut()
			&& pending.identity == identity
		{
			pending.answer = Some(secret.clone());
		}
		self.send_command(SshCommand::ElevateAnswer { identity, secret });
		iced::Task::none()
	}

	/// A credential question arrived from an elevating shell (§45, §47).
	///
	/// Two things can be true when one lands. If the dialog is open, the question goes into it. If it
	/// is NOT — which is what an elevation started from the target's stored preference looks like —
	/// the dialog is opened to ask it, because a question nobody is shown is an elevation that hangs.
	///
	/// A password the vault holds is tried FIRST, and only for the first question: `refusal.is_some()`
	/// means the stored one was just rejected, and a question after the first may be a second factor,
	/// which a stored password must never be offered as (§45).
	fn on_elevate_prompt(
		&mut self,
		identity: u64,
		label: String,
		refusal: Option<String>,
	) -> iced::Task<Message> {
		// A question for an elevation that is not the one in flight is stale — its shell has since
		// ended — and answering it would put a password on a channel nobody is watching.
		if self
			.pending_elevation
			.as_ref()
			.is_none_or(|pending| pending.identity != identity)
		{
			return iced::Task::none();
		}
		if refusal.is_none()
			&& let Some(secret) = self.stored_elevation_secret(identity)
		{
			if let Some(pending) = self.pending_elevation.as_mut() {
				pending.answer = Some(secret.clone());
			}
			self.send_command(SshCommand::ElevateAnswer { identity, secret });
			return iced::Task::none();
		}
		let mut task = iced::Task::none();
		if self.elevate_form_mut().is_none() {
			task = self.open_accounts_dialog();
		}
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Answering {
				identity,
				label,
				refusal,
				answer: String::new(),
			};
		}
		// The answer field, not the account field: the dialog is a prompt now, and nothing else on
		// it is worth typing into.
		iced::Task::batch([
			task,
			iced::widget::operation::focus(ui::elevate::ANSWER_INPUT_ID),
		])
	}

	/// The password the vault holds for the elevation in flight, if the user asked for one to be
	/// kept and the vault is open (§47).
	///
	/// Offered ONCE per elevation, because a refusal comes back as a question with a `refusal`
	/// attached and `on_elevate_prompt` will not answer one of those from the vault. A locked vault
	/// yields nothing rather than prompting for the master passphrase — an elevation is not the
	/// moment to interrupt with a second question.
	fn stored_elevation_secret(&self, identity: u64) -> Option<Secret> {
		let pending = self.pending_elevation.as_ref()?;
		if pending.identity != identity || !pending.remember {
			return None;
		}
		let endpoint = self.connection.as_deref()?;
		let key = crate::vault::elevation_key(endpoint, &pending.account);
		self.vault.borrow().as_ref()?.get(&key).cloned()
	}

	/// Store or forget the password of an elevation that has just resolved (§47).
	///
	/// The rule is §45's, applied one layer up: `factors` is how many DISTINCT things were asked
	/// for, and only when it is 1 is the answer a PASSWORD. More than one means a second factor was
	/// involved, and a one-time code kept as a password would be replayed to a machine that has
	/// already spent it. A question re-put after a refusal is the same factor over again, so a
	/// corrected password still counts as one.
	///
	/// Unticking "Remember the password" is how a stored one is removed, which is why the `false`
	/// branch forgets rather than doing nothing.
	fn settle_elevation_secret(&mut self, factors: u32) {
		let Some(pending) = self.pending_elevation.take() else {
			return;
		};
		let Some(endpoint) = self.connection.clone() else {
			return;
		};
		let key = crate::vault::elevation_key(&endpoint, &pending.account);
		let mut stored = false;
		if let Some(vault) = self.vault.borrow_mut().as_mut() {
			if pending.remember && factors == 1 {
				if let Some(secret) = pending.answer {
					match vault.store(&key, secret) {
						Ok(()) => stored = true,
						Err(error) => eprintln!("could not save the vault: {error:#}"),
					}
				}
			} else if let Err(error) = vault.forget(&key) {
				eprintln!("could not update the vault: {error:#}");
			}
		}
		// The flag follows what the vault ACTUALLY holds, so the dialog never opens promising a
		// hands-free elevation that cannot happen — §16's own rule for the connect secret.
		// Non-overlapping borrows of the shared target cell (see `commit_rename`).
		let moved =
			self.targets
				.borrow_mut()
				.set_elevation_remembered(&endpoint, &pending.account, stored);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Remember (or update) what this target's sessions should become (§47).
	///
	/// The password flag is not touched here: it follows what the vault actually holds, which is
	/// `settle_elevation_secret`'s business.
	fn persist_elevation(
		&mut self,
		account: &str,
		kind: crate::elevate::ElevateKind,
		on_connect: bool,
	) {
		let Some(endpoint) = self.connection.clone() else {
			return;
		};
		let moved = self
			.targets
			.borrow_mut()
			.set_elevation(&endpoint, account, kind, on_connect);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// End one elevated shell (§45): EOF on its channel, which ends its login shell and with it the
	/// elevation. The list entry goes when the session says the shell has ended, not here — a shell
	/// that refuses to die must not vanish from the dialog while it is still running.
	fn close_identity(&mut self, identity: u64) -> iced::Task<Message> {
		if identity != bridge::LOGIN_IDENTITY {
			self.send_command(SshCommand::CloseIdentity(identity));
		}
		iced::Task::none()
	}

	/// Start the elevation this target remembers, if it remembers one (§47).
	///
	/// Called once the login shell is live, which is the earliest moment a program can be run on the
	/// connection. Three things stop it: no stored elevation, one whose account this build declines
	/// to act on (`Elevation::usable` — `targets.json` is a file the user is invited to edit), and
	/// one that says only "remember this account" rather than "do it every time".
	fn elevate_on_connect(&mut self) {
		let saved = self
			.connection
			.as_deref()
			.and_then(|endpoint| self.targets.borrow().find(endpoint).cloned())
			.and_then(|target| target.elevate);
		let Some(saved) = saved else { return };
		if !saved.on_connect || !saved.usable() {
			return;
		}
		self.start_elevation(&saved.account, saved.kind, saved.remember_password, true);
	}

	/// Start a set of forwards a reconnect restored (§27): each gets a fresh id, is queued as
	/// `Starting`, and is asked for down the channel. No persistence here — the set came FROM the
	/// stored target, so it is already saved.
	fn establish_forwards(&mut self, specs: Vec<crate::forward::ForwardSpec>) {
		for spec in specs {
			let id = self.next_forward_id;
			self.next_forward_id += 1;
			if self.send_command(SshCommand::AddForward {
				id,
				spec: spec.clone(),
			}) {
				self.forwards.push(crate::forward::ForwardEntry {
					id,
					spec,
					status: crate::forward::ForwardStatus::Starting,
					// Set only if this is a `-R 0` and the server later reports the port it chose.
					bound_port: None,
					// A fresh forward has carried nothing yet; the gauge fills as connections flow.
					open_count: 0,
					total_count: 0,
				});
			}
		}
	}

	/// Mark a forward's row from a worker event (§27). An id with no matching entry — a late
	/// event for one already removed — is ignored.
	fn set_forward_status(&mut self, id: u64, status: crate::forward::ForwardStatus) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			entry.status = status;
		}
	}

	/// A forward came up (§27): mark its row Active, and for a `-R 0` record the port the server
	/// assigned so the row shows where it is actually listening. The spec keeps its authored 0, so
	/// a reconnect asks for a fresh port rather than pinning this ephemeral one.
	fn mark_forward_ready(&mut self, id: u64, assigned_port: Option<u16>) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			entry.status = crate::forward::ForwardStatus::Active;
			if assigned_port.is_some() {
				entry.bound_port = assigned_port;
			}
		}
	}

	/// A connection opened or closed on forward `id` (§27): move its live gauge. `opened` raises the
	/// open and total counts; a close lowers the open count (the total only ever grows). An id with
	/// no matching row — a late event for one already removed — is ignored.
	fn bump_forward(&mut self, id: u64, opened: bool) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			if opened {
				entry.connection_opened();
			} else {
				entry.connection_closed();
			}
		}
	}

	/// Save the session's current forward set to its target (§27), so a reconnect re-establishes
	/// them. Only meaningful with a live connection (the forwards belong to that target); the
	/// specs are written whole, and `set_forwards` skips the disk write when nothing changed.
	fn persist_forwards(&mut self) {
		let Some(endpoint) = self.connection.clone() else {
			return;
		};
		let specs: Vec<crate::forward::ForwardSpec> = self
			.forwards
			.iter()
			.map(|entry| entry.spec.clone())
			.collect();
		// Non-overlapping borrows of the shared target cell (see `commit_rename`).
		let moved = self.targets.borrow_mut().set_forwards(&endpoint, specs);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Return to the connect form: reset the keyboard focus to the first field and
	/// focus it natively, so the form is ready for typing and its highlight ring is
	/// aligned (§10). Used by the paths that keep the user on the form to retry
	/// (error Back, passphrase cancel) — a full return to the list uses `go_home`.
	fn go_to_form(&mut self) -> iced::Task<Message> {
		self.screen = AppScreen::Connect;
		// Nothing is being asked any more, which is what puts the form's own keyboard ring back on
		// (§7): the ring and the prompt are never both live.
		self.prompt = None;
		self.form_focus = ui::connect::FormStop::Host;
		self.apply_form_focus()
	}

	/// Return to the home screen (§14). Closes any open menu / rename, drops a pending
	/// (unsaved) target, and clears the typed secrets out of the form so they do not
	/// linger once we leave it (§12). The saved-target selection is kept so the list
	/// re-opens on the last-used row.
	fn go_home(&mut self) -> iced::Task<Message> {
		self.screen = AppScreen::Home;
		// Whatever the connect flow was asking is abandoned with the connect itself, and the
		// buffers it was holding go with it (§12).
		self.prompt = None;
		self.home_menu_open = false;
		self.home_rename = None;
		self.confirm_delete = false;
		// Leaving for the list abandons any connect in flight, so what it was carrying goes with
		// it — the unsaved target and, above all, the secret it captured (§12, §14, §16).
		self.abandon_attempt();
		self.form.password.clear();
		self.form.passphrase.clear();
		// Going back to the list abandons the connect a copy was opened for, so its carried
		// directory goes too (§52) — whatever is dialed from here is not that copy. The armed dial
		// goes with it, or a worker arriving a moment later would dial from the home screen.
		self.carry_cwd = None;
		self.pending_connect = false;
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
		// A deferred task means the secret is behind the master passphrase and the form is not
		// finished being filled; otherwise it is ready as it stands and all that is left is to show
		// it (§16).
		self.seed_form(&key).unwrap_or_else(|| self.go_to_form())
	}

	/// Fill the connect form from the stored target `key`, secret and all (§14, §16). Shared by the
	/// home list's Open and by a chip menu's Duplicate (§52), which needs the same form filled the
	/// same way before it can dial.
	///
	/// Returns `Some(task)` when the fill could NOT be finished on the spot: the target remembers a
	/// secret and the vault holding it is locked, so the task is the master-passphrase prompt and
	/// the fill resumes on unlock. `None` means the form is ready as it stands — which includes a
	/// target that remembers nothing, and one whose secret was already to hand.
	///
	/// A key naming no stored target answers `None` with the form untouched, since there is nothing
	/// to fill it from.
	fn seed_form(&mut self, key: &str) -> Option<iced::Task<Message>> {
		// Copy out the fields before touching `self.form`, so the borrow of `self.targets` ends
		// first (assigning the form mutably borrows `self`).
		let (host, port, user, auth_kind, key_path, cert_path, remember, elevate) =
			self.targets.borrow().find(key).map(|target| {
				(
					target.host.clone(),
					target.port,
					target.user.clone(),
					target.auth_kind,
					target.key_path.clone(),
					target.cert_path.clone(),
					target.remember_secret,
					target.elevate.clone(),
				)
			})?;
		self.form = ui::connect::ConnectForm {
			host,
			port: port.to_string(),
			user,
			auth_kind,
			password: String::new(),
			key_path,
			cert_path,
			passphrase: String::new(),
			// A remembered target opens with the box already ticked (§16); untick to stop
			// remembering it, which forgets the stored secret on the next connect.
			remember,
			// And with whatever it remembers about becoming another account (§47), so a return
			// visit sees what the next session will do and can change it before connecting.
			elevate_account: elevate
				.as_ref()
				.map(|saved| saved.account.clone())
				.unwrap_or_default(),
			elevate_kind: elevate
				.as_ref()
				.map_or(crate::elevate::ElevateKind::default(), |saved| saved.kind),
			elevate_on_connect: elevate.is_some_and(|saved| saved.on_connect),
		};

		if remember {
			// Read the vault's state in short borrows and drop them before any `&mut self` call
			// (`fill_secret_field` / `open_vault_modal`), so the shared cell is never held across
			// a mutation of the tab (§26).
			if self.vault.borrow().is_some() {
				// Vault already open this session: pull the secret straight into the field.
				let secret = self
					.vault
					.borrow()
					.as_ref()
					.and_then(|vault| vault.get(key).cloned());
				if let Some(secret) = secret {
					self.fill_secret_field(&secret);
				}
			} else {
				// Vault locked: show the (now populated) form as the backdrop and prompt to
				// unlock; the pre-fill resumes on success.
				self.screen = AppScreen::Connect;
				return Some(self.open_vault_modal(VaultPending::Prefill(key.to_owned())));
			}
		}
		None
	}

	/// Open this fresh tab as a copy of a session on `endpoint`, standing in `cwd` (§52).
	///
	/// The tab is brand new and on the home screen; this fills its form from the same stored target
	/// the endpoint was connected through and dials it, so a duplicate is one menu click rather than
	/// a form to fill in again. The carried directory is set first and spent when the shell opens.
	///
	/// It does NOT reach into the source tab for the secret. The credential comes from the vault, by
	/// exactly the route the home list's Open takes (§16), so a duplicate can do no more than the
	/// user could do by hand — and a password that was typed once and never stored still has to be
	/// typed again, which is the promise "remember" is the opt-in to (§12).
	///
	/// Three ways this can end, and the form is filled in all of them:
	///   * the vault is locked — the master-passphrase prompt opens over the form, and the user
	///     presses Connect once it is filled;
	///   * something is still needed from the user (a password that was never remembered) — the
	///     form opens with the rest already in it;
	///   * nothing is — it dials at once, which is the common case and the point of the feature.
	fn open_copy_of(&mut self, endpoint: &str, cwd: Option<String>) -> iced::Task<Message> {
		self.carry_cwd = cwd.map(|cwd| Carry {
			endpoint: endpoint.to_owned(),
			cwd,
		});
		if let Some(deferred) = self.seed_form(endpoint) {
			return deferred;
		}
		if self.ready_to_dial() {
			// The worker is normally not there yet — this tab was made a moment ago — so the dial is
			// armed and fired by the `Ready` that follows. A tab that somehow already has one is
			// dialed on the spot rather than made to wait for an event that has been and gone.
			if self.command_tx.is_some() {
				return self.on_connect_pressed();
			}
			self.pending_connect = true;
		}
		self.go_to_form()
	}

	/// Whether a connect could be sent with the form exactly as it stands (§52) — nothing left for
	/// the user to type.
	///
	/// Validation on its own is not the test: it accepts an EMPTY password, deliberately, because
	/// some servers do (§7). Dialing on an empty password field would spend an authentication
	/// attempt to arrive back at the same form with a failure notice on it, so a password that is
	/// not there is treated as something still to type. Every other method needs no field — a key's
	/// passphrase, a keyboard-interactive challenge and an agent's confirmation are all asked for
	/// during the connect, exactly as they would be from the form's own button.
	fn ready_to_dial(&self) -> bool {
		if self.form.auth_kind == ui::connect::AuthKind::Password && self.form.password.is_empty() {
			return false;
		}
		self.form.validate().is_ok()
	}

	/// A new pattern in the home screen's filter box (§49): keep it, and let go of the selection
	/// if the row it names is no longer on screen.
	///
	/// Dropping it is the whole point. Every shortcut this screen has acts on the selection and
	/// not on what the pointer is over — F2 renames it, Enter opens it, Delete asks to remove it
	/// — so a selection hidden behind a filter is one keystroke away from renaming or deleting a
	/// row the user cannot see, and the confirmation naming a target that is not in the list
	/// reads as a bug rather than as the warning it is. Re-selecting is a click, the same click
	/// that selected it in the first place, so nothing is lost by letting go.
	fn on_home_filter(&mut self, pattern: String) {
		self.home_filter = pattern;
		let still_shown = self.home_selected.as_deref().is_some_and(|key| {
			self.targets
				.borrow()
				.find(key)
				.is_some_and(|target| target.matches(&self.home_filter))
		});
		if !still_shown {
			self.home_selected = None;
			// The context menu is anchored to the selected row, so it cannot outlive it.
			self.home_menu_open = false;
		}
	}

	/// Begin an inline rename of the selected target (§14): seed the edit with its
	/// current name and focus the field so the user types straight away. No selection
	/// (or a stale one) is a no-op.
	fn start_rename(&mut self) -> iced::Task<Message> {
		self.home_menu_open = false;
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		let Some(name) = self
			.targets
			.borrow()
			.find(&key)
			.map(|target| target.name.clone())
		else {
			return iced::Task::none();
		};
		self.home_rename = Some(ui::home::RenameState { key, text: name });
		iced::widget::operation::focus(ui::home::RENAME_INPUT_ID)
	}

	/// Commit the in-progress rename (§14): apply it (which re-sorts the list) and save.
	/// A blank name is rejected by the store, so committing one just discards the edit.
	fn commit_rename(&mut self) {
		if let Some(rename) = self.home_rename.take() {
			// Two borrows of the one shared cell must not overlap (a mut + a shared borrow is a
			// RefCell panic), so the rename's `borrow_mut` ends on its own line before `save`
			// takes a fresh shared borrow (§26).
			let renamed = self.targets.borrow_mut().rename(&rename.key, &rename.text);
			if renamed && let Err(error) = self.targets.borrow().save() {
				eprintln!("could not save targets: {error:#}");
			}
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
		let Some(name) = self
			.targets
			.borrow()
			.find(&key)
			.map(|target| target.name.clone())
		else {
			return;
		};
		let body = format!("{}\n\n{}  ({key})", ui::home::DELETE_DIALOG_BODY, name);
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
			if let Some(vault) = self.vault.borrow_mut().as_mut()
				&& let Err(error) = vault.forget(&key)
			{
				eprintln!("could not update the vault: {error:#}");
			}
			// Fresh, non-overlapping borrows of the shared target cell (see `commit_rename`).
			let removed = self.targets.borrow_mut().remove(&key);
			if removed && let Err(error) = self.targets.borrow().save() {
				eprintln!("could not save targets: {error:#}");
			}
		}
	}

	/// Who is holding the keyboard on this tab, if anyone (§10, §14, §17, §18, §27, §35) — see
	/// [`KeyboardClaim`], which is where the reasoning lives.
	///
	/// THE ORDER OF THESE TESTS IS THE PRIORITY, and that is the whole point of the function: it
	/// used to be the order of seven `if` blocks in two different handlers, which is a rule nothing
	/// could read back. Here it is one list, in one place, and the tests below can assert a pair.
	///
	/// Pure, and it reads only what is already on the tab, so "who has the keyboard" can be asked
	/// without a window and without pressing anything.
	fn keyboard_claim(&self) -> Option<KeyboardClaim> {
		match self.screen {
			AppScreen::Home => {
				if self.confirm_delete {
					return Some(KeyboardClaim::DeleteTarget);
				}
				if self.home_rename.is_some() {
					return Some(KeyboardClaim::TargetRename);
				}
				None
			}
			AppScreen::Terminal => {
				if self.modal.is_some() {
					return Some(KeyboardClaim::Modal);
				}
				if self.transfers.holds_keyboard() {
					return Some(KeyboardClaim::Transfers);
				}
				if self.panes.tree.editing().is_some() {
					return Some(KeyboardClaim::TreeRename);
				}
				if self.panes.pane.editing().is_some() {
					return Some(KeyboardClaim::PaneRename);
				}
				if self.search.is_some() {
					return Some(KeyboardClaim::Find);
				}
				None
			}
			// The connect form, the viewers and the connecting screen have keyboard handlers of
			// their own, and nothing on them holds the keyboard against the others.
			_ => None,
		}
	}

	/// Escape, given to whoever is holding the keyboard (§10, §14, §17, §18, §27, §35). Every
	/// claimant can be backed out of, and none of them acts on being dismissed — which is what makes
	/// one key safe for all seven.
	fn dismiss(&mut self, claim: KeyboardClaim) {
		match claim {
			KeyboardClaim::DeleteTarget => self.confirm_delete = false,
			KeyboardClaim::TargetRename => self.home_rename = None,
			KeyboardClaim::Modal => self.modal = None,
			KeyboardClaim::Transfers => self.transfers.escape(),
			KeyboardClaim::TreeRename => self.panes.tree.cancel_rename(),
			KeyboardClaim::PaneRename => self.panes.pane.cancel_rename(),
			// The current match stays selected when the bar closes, so it can still be copied.
			KeyboardClaim::Find => self.search = None,
		}
	}

	/// Handle a key on the home screen (§14). While something is holding the keyboard — the delete
	/// prompt, an inline rename — the list shortcuts are inert and only Esc is handled; a stray
	/// Enter must not open a connection behind the modal, and a rename's Enter belongs to the
	/// field's own `on_submit`. Otherwise F2 renames the selection, Enter opens it, Delete asks to
	/// remove it; all are no-ops without a selection. Other keys fall through.
	fn on_home_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		let iced::keyboard::Event::KeyPressed {
			key,
			modifiers,
			repeat,
			..
		} = event
		else {
			return iced::Task::none();
		};

		if let Some(claim) = self.keyboard_claim() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.dismiss(claim);
			}
			return iced::Task::none();
		}

		// Ctrl+D closes this tab — but only from the home screen, i.e. once logged off from any
		// remote (§30). On a live shell the same key is EOF to the remote (the way you log out),
		// so it is left to the terminal there; pressing it logs the shell out, which lands back
		// here, and a second Ctrl+D then closes the tab — mirroring a terminal's own Ctrl+D twice.
		// It routes through `TabCloseRequested`, so closing the last tab still asks to quit cmote.
		//
		// An AUTO-REPEAT is not the second press — see `is_close_tab`, which is where that fix lives (§104).
		if is_close_tab(&key, modifiers, repeat) {
			return iced::Task::done(Message::TabCloseRequested(self.id));
		}

		// Ctrl+F puts the cursor in the filter box (§49) — the browser's shortcut for the same
		// thing, and the one the terminal's find bar answers to a screen away (Ctrl+Shift+F,
		// §35; the shell has a claim on plain Ctrl+F, this screen does not). Pressing it while
		// already typing there simply focuses it again, which is a no-op rather than a surprise.
		if modifiers.control()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& matches!(&key, iced::keyboard::Key::Character(character) if character.as_str() == "f")
		{
			return iced::widget::operation::focus(ui::home::FILTER_INPUT_ID);
		}

		match key {
			iced::keyboard::Key::Named(Named::F2) => self.start_rename(),
			iced::keyboard::Key::Named(Named::Enter) => self.open_selected_target(),
			iced::keyboard::Key::Named(Named::Delete) => {
				self.ask_delete_selected_target();
				iced::Task::none()
			}
			// Esc empties the filter box and puts the whole list back (§49) — the way out of a
			// pattern that matches nothing, without going back to the box to erase it. From
			// INSIDE the box it takes two presses: iced's text input unfocuses on Esc and
			// captures the event, so the first press only hands the keyboard back and the second
			// one arrives here. That is the widget's behaviour, not a rule of this screen.
			iced::keyboard::Key::Named(Named::Escape) => {
				self.on_home_filter(String::new());
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
			.input_id(self.form.shape())
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

		// Not while something is being asked over the form (§7, §8, §16). `subscription` already
		// switches this listener off, but iced rebuilds the subscription list only AFTER the update
		// that opened the prompt has returned — so a key pressed in the same frame the dialog
		// appeared still arrives here. Without this, Enter could press the Connect button under a
		// host-key dialog.
		if self.prompt.is_some() {
			return iced::Task::none();
		}

		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};

		match key {
			iced::keyboard::Key::Named(Named::Tab) => {
				let shape = self.form.shape();
				self.form_focus = if modifiers.shift() {
					self.form_focus.previous(shape)
				} else {
					self.form_focus.next(shape)
				};
				self.apply_form_focus()
			}
			iced::keyboard::Key::Named(named @ (Named::Enter | Named::Space)) => {
				if self.form_focus.input_id(self.form.shape()).is_some() {
					// A text stop: Enter submits the form (the field has no submit of its
					// own), Space types a space and is left to the field.
					if named == Named::Enter {
						iced::Task::done(Message::ConnectPressed)
					} else {
						iced::Task::none()
					}
				} else if let Some(message) = self.form_focus.activation(self.form.shape()) {
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

	/// Route a key press on the terminal screen (§20): to the focused pane, or — when the
	/// shell has the focus, which is where every session starts — down the channel.
	/// Non-input keys (bare modifiers, unmapped keys) encode to nothing and are
	/// dropped. Keyboard events only reach here on the Terminal screen (the
	/// subscription is added only there), so no extra screen check is needed.
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over the keyboard: length is the number of keys claimed, not depth"
	)]
	fn on_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::{Code, Named, Physical};

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
			// Named rather than `_`, so that a keyboard event iced adds later is a COMPILE error
			// here instead of a silently dropped keystroke. This one arm is already handled by the
			// early return above; it is spelled out only to close the match.
			iced::keyboard::Event::ModifiersChanged(_) => return iced::Task::none(),
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

		// Whoever is holding the keyboard gets it, and gets it before anything on this screen may
		// see the key (§10, §17, §18, §27, §35) — see `KeyboardClaim`. Esc backs out of whichever it is;
		// everything else waits for the field or the button that is holding it.
		let claim = self.keyboard_claim();
		if let Some(claim) = claim
			&& claim != KeyboardClaim::Find
		{
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.dismiss(claim);
			}
			return iced::Task::none();
		}

		// Ctrl+Shift+F opens the scrollback find bar and focuses its field (§35). THE ONE EXCEPTION
		// to the rule above, and the reason `KeyboardClaim::Find` is ranked last and singled out here:
		// pressing it while the bar is already up refocuses the field rather than being swallowed by
		// the bar it opened. Every other claimant still outranks it — with a modal or a rename up,
		// this key does nothing, which is the behaviour the old block order gave by sitting exactly
		// here. Matched on the PHYSICAL key like the copy/paste bindings, so it holds on any layout;
		// plain Ctrl+F belongs to the shell (readline's forward-char), which is why only the Shift
		// form is cmote's.
		if modifiers.control()
			&& modifiers.shift()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& matches!(physical_key, Physical::Code(Code::KeyF))
		{
			return self.open_term_find();
		}

		// And now the find bar itself, having let its own shortcut past.
		if let Some(claim) = claim {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.dismiss(claim);
			}
			return iced::Task::none();
		}

		// Ctrl+Tab hands the keyboard on to the next pane, Ctrl+Shift+Tab to the previous
		// one (§20). Taken before anything else on this screen: it is the way *out* of a
		// pane that is swallowing keys, so nothing may shadow it.
		if modifiers.control() && matches!(key, iced::keyboard::Key::Named(Named::Tab)) {
			self.cycle_focus(modifiers.shift());
			return iced::Task::none();
		}

		// Typing takes the keyboard back to the shell (§50). A pane answers to the arrows, the
		// Page keys, Tab, Enter, F2, F5 and Esc — never to a plain character — so a letter
		// arriving while a pane holds the ring is someone starting a command at the prompt they
		// are looking at, with the focus left on a pane they navigated a while ago. The old
		// behaviour dropped that keystroke: the pane swallowed it, nothing happened, and the
		// first letter of the command was silently eaten (or, worse, the first several, until the
		// missing echo was noticed). Handing the focus over is what the user was asking for by
		// typing at all. Taken before the pane dispatch below, so the key itself goes on to the
		// shell rather than being spent on the switch.
		if !matches!(self.focus, Focus::Terminal) && is_typing(&key, modifiers) {
			self.set_focus(Focus::Terminal);
		}

		// Ctrl+V is typing by another route, so it is answered from wherever the keyboard is (§50)
		// — the same reading that makes the menu's own Paste take the focus back. It sits ABOVE the
		// pane dispatch rather than in the copy/paste block below for exactly that reason: down
		// there it is only reached with the shell already focused, and a paste aimed at the shell
		// while a pane held the ring would be dropped on the floor with no echo to say so. Neither
		// pane claims Ctrl+V, so nothing is being taken from them.
		//
		// Ctrl+C is NOT treated this way. It reads the terminal's own selection or, with none, is
		// the interrupt for the remote — neither is text going in, and the panes have the better
		// claim on a future "copy what is selected here".
		if is_paste(physical_key, modifiers) {
			self.on_terminal_command();
			return self.on_paste();
		}

		// A focused pane keeps the key; only the shell's own focus reaches the channel.
		match self.focus {
			Focus::Tree => return self.on_tree_key(&key),
			Focus::Files => return self.on_files_key(&key, modifiers),
			Focus::Terminal => {}
		}

		// The copy keyboard shortcuts, with the shell focused (§10) — paste is answered above,
		// before the focus dispatch. Taken before the key is encoded for the remote, so a terminal
		// binding wins over the program — the way xterm and kitty keep these for the terminal
		// itself. Matched on the PHYSICAL key, so the shortcut holds on any layout (AZERTY,
		// Dvorak, …), not only where C sits on QWERTY. Alt / Logo held means it is some other
		// combination, so leave those for the shell.
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
				// Ctrl+Shift+O selects the last finished command's output (§34), so the existing
				// Copy then grabs it. Plain Ctrl+O has a readline meaning (the shell's), so only
				// the Shift form is ours — the bare key falls through to the channel below.
				Physical::Code(Code::KeyO) if modifiers.shift() => {
					self.select_command_output();
					return iced::Task::none();
				}
				_ => {}
			}
		}

		// Ctrl+D at a local shell that may not act on it (§104). The key is NOT taken here — it goes to
		// the shell exactly as it would in any terminal, and what comes back decides whether the session
		// ends. All this does is start listening.
		//
		// The problem it solves: EOF at a POSIX prompt is how you log out, and the shell exiting is what
		// ends the session — but the three Windows interpreters do not act on `0x04` at all, so the key
		// did nothing whatsoever in a `pwsh` tab while the same key in the Git Bash tab beside it logged
		// out. The first fix took the key and ended the session, which was wrong in the case that
		// matters: with `node` running at that prompt, Ctrl+D belongs to node — and node quits on it,
		// measured, leaving the shell exactly where it was. Taking the key threw away a program's own
		// EOF handling to do something cruder.
		//
		// So the byte is sent and the shell is allowed to answer. There are only two answers, and they
		// are told apart by what a Windows interpreter does with a control byte it has no meaning for: it
		// ECHOES it, as the two characters `^D`, onto its input line. So an answer containing `^D` means
		// nothing consumed the byte and the session ends (`judge_eof`); anything else — a fresh prompt
		// after node exited, a pager scrolling — means the byte did its job and the session is left
		// alone. The cost of being right is one output round trip, measured at 10-17 ms.
		//
		// Matched on the LOGICAL character rather than the physical key, unlike the copy bindings above,
		// because the byte the encoder is about to derive comes from that same character. Plain Ctrl+D
		// only: Ctrl+Shift+D encodes to the same `0x04` and is deliberately left unwatched, so it is the
		// way to send a bare EOF to a shell that echoes it without ending the session.
		//
		// Not while the alternate screen is up, which is belt and braces on top of the echo test: a pager
		// showing a file that happens to contain the text `^D` would otherwise be answering for the
		// shell. A full-screen program asked for the whole screen, so the key is simply its own.
		if modifiers.control()
			&& !modifiers.shift()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& self.local.is_some_and(|kind| !kind.quits_on_eof())
			&& !self.on_alternate_screen()
			&& matches!(&key, iced::keyboard::Key::Character(character) if character.as_str() == "d")
		{
			// Armed, and then deliberately NOT returned from: the encoder below sends the byte.
			self.eof_probe = Some(Vec::new());
		}

		// Ctrl+Shift+Up / Ctrl+Shift+Down jump the scrollback to the previous / next shell prompt
		// (§34), from the OSC 133 marks. Like the Shift+Page scroll below, it is cmote's own view
		// motion — nothing is sent to the remote — and reached only with the shell focused. Guarded
		// on Ctrl+Shift together so a bare or singly-modified arrow still reaches the shell.
		if modifiers.control()
			&& modifiers.shift()
			&& let iced::keyboard::Key::Named(named) = &key
			&& let Some(direction) = prompt_jump(*named)
			&& let Some(terminal) = self.terminal.as_mut()
		{
			terminal.jump_prompt(direction);
			return iced::Task::none();
		}

		// Shift + PageUp / PageDown page through the shell's own scrollback, and Shift + Home /
		// End jump to its ends, rather than reaching the remote (§23). Shift-guarded so the bare
		// keys still send their CSI sequences to a full-screen program; reached only with the
		// shell focused, since a focused pane has already claimed the arrows and their neighbours.
		if modifiers.shift()
			&& let iced::keyboard::Key::Named(named) = &key
			&& let Some(motion) = scroll_motion(*named)
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
		// that mode, so cmote scans the stream for it (§9). DECCKM, DECKPAM and the kitty flags, by
		// contrast, the engine does track, so they come off the screen seam (§25, §36).
		let modes = self
			.terminal
			.as_ref()
			.map(|terminal| term::keymap::Modes {
				application_cursor: terminal.screen().application_cursor(),
				application_keypad: terminal.screen().application_keypad(),
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
	/// a side pane holds the focus — in every such case a keystroke belongs to cmote's own UI, not
	/// the session. Used to decide whether a key *release* should reach the shell; a press is routed
	/// by the fuller guard chain in `on_key`, which this mirrors.
	fn shell_owns_keyboard(&self) -> bool {
		self.modal.is_none()
			&& !self.transfers.holds_keyboard()
			&& self.panes.tree.editing().is_none()
			&& self.panes.pane.editing().is_none()
			&& matches!(self.focus, Focus::Terminal)
	}

	/// The focus ring (§20): shell, tree, files pane, and round again — skipping whichever
	/// panes are hidden, since a stop you cannot see is a dead press of Ctrl+Tab. The
	/// shell is always in the ring; it is the one thing always on this screen.
	fn cycle_focus(&mut self, backwards: bool) {
		let mut ring = vec![Focus::Terminal];
		if self.panes.tree.visible() {
			ring.push(Focus::Tree);
		}
		if self.panes.pane.visible() {
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

	/// A command from the terminal's own surface ran — an item of the grid's right-click menu, the
	/// status-bar button that duplicates it, or the Ctrl+V that is the same command off the
	/// keyboard (§50). Whatever held cmote's keyboard until now, the user just reached into the
	/// terminal and acted on it, so the ring goes back there.
	///
	/// It matters most for **Paste**, which is typing at the prompt by another route: pasting a
	/// command while the files pane held the focus used to leave the next keystroke — the Enter
	/// that runs it — going to the pane. But the same reading covers the rest of the menu: a copy,
	/// an upload into the shell's directory, a link followed out of its scrollback are all work on
	/// the terminal, and none of them is a reason to keep the keyboard parked on a pane.
	///
	/// Only an ITEM does this, not the right-press that opens the menu: opening it is a question
	/// about what is under the pointer, and dismissing it (Esc, or a click on the dismiss layer)
	/// leaves everything as it was — including where the keyboard is.
	fn on_terminal_command(&mut self) {
		self.focus_pane(Focus::Terminal);
	}

	/// Give the keyboard to a pane because it was clicked (§20). Also closes the OTHER
	/// pane's context menu — clicking into a pane is as much a click-away from the menu
	/// next door as clicking the grid is.
	fn focus_pane(&mut self, focus: Focus) {
		self.set_focus(focus);
		self.menu = None;
	}

	/// Move cmote's keyboard ring to `focus`, the single funnel for every internal focus move
	/// (§20, §23). Routing them all through here means focus reporting sees each one: a switch
	/// off the shell to a pane reads as the shell losing focus, and back as regaining it. Only
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
	/// terminal — so alt-tabbing away and switching to a side pane both read as a focus-out,
	/// per the reading that the remote, blind to cmote's panes, should hear about either.
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
				// Open the folder — the same call the row click makes: a folder never listed is
				// fetched here too, and re-opening a closed one re-lists it (`expand`), so the
				// keyboard catches a shell-side change just as the mouse does.
				if let Some(path) = self.panes.tree.selected().map(str::to_owned)
					&& let Some(fetch) = self.panes.tree.expand(&path, false)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				return iced::Task::none();
			}
			Named::ArrowLeft => {
				if let Some(path) = self.panes.tree.selected().map(str::to_owned) {
					self.panes.tree.collapse(&path);
				}
				return iced::Task::none();
			}
			Named::Enter => {
				let Some(path) = self.panes.tree.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::Cd(path));
			}
			Named::F2 => {
				let Some(path) = self.panes.tree.selected().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_explorer(ExplorerMessage::RenameStarted(path));
			}
			// F5 refreshes the whole visible tree, the same as the header ↻ button — the
			// familiar file-manager key for "bring what I am looking at up to date".
			Named::F5 => return self.on_explorer(ExplorerMessage::RefreshTree),
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		self.panes.tree.step(step);
		self.scroll_tree_into_view()
	}

	/// Keys while the files pane has the focus (§20). Left/Right step one cell and Up/Down
	/// a whole row — the grid wraps at the window's width, so how many cells that is comes
	/// from the same arithmetic the layout uses. Tab/Shift+Tab are next/previous, Enter
	/// opens a folder, F2 renames, and Esc hands the keyboard back to the shell.
	///
	/// The movement keys fold into a `FilesNav` first, because a step is relative to the current
	/// cell while an edge is an absolute end of the grid. Home and End MUST be absolute: a relative
	/// jump reads the empty-selection default and would land on the wrong end when nothing is
	/// selected yet (see `Files::jump_to_edge`).
	fn on_files_key(
		&mut self,
		key: &iced::keyboard::Key,
		modifiers: iced::keyboard::Modifiers,
	) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		// What one movement key asks for. Declared here rather than mid-body: an item is in scope
		// from the start of the block whatever line it is written on, so writing it where it is first
		// used only makes the reader think its scope begins there (`items_after_statements`, §111).
		enum FilesNav {
			/// Relative to the current cell, in model-space cells.
			Step(isize),
			/// An absolute end of the grid — `true` for the last cell.
			Edge(bool),
		}

		// Ctrl+A takes the whole listing (§21). Checked before the named-key gate below,
		// since it is the pane's only shortcut on a character key.
		if modifiers.control()
			&& matches!(key, iced::keyboard::Key::Character(character)
				if character.as_str().eq_ignore_ascii_case("a"))
		{
			self.panes.pane.select_all(self.panes.show_hidden());
			return iced::Task::none();
		}

		let iced::keyboard::Key::Named(named) = key else {
			return iced::Task::none();
		};

		// Signed, because these become the deltas the pane's `step` walks by and a delta goes both
		// ways. `cast_signed` says the reinterpretation out loud where `as isize` only implied it.
		let columns = ui::files::columns(self.files_width()).cast_signed();
		// A page is a screenful of rows (less one, for context), turned into a model-space delta
		// by the column count — the same units `step` moves the arrows in.
		let page = ui::files::page_rows(&self.panes.pane).cast_signed() * columns;
		// Shift held on a movement key extends the selection instead of moving it (§21). Not on
		// Tab: there, Shift already means "the other way".
		let extend = modifiers.shift();
		let (nav, extend) = match named {
			Named::ArrowRight => (FilesNav::Step(1), extend),
			Named::ArrowLeft => (FilesNav::Step(-1), extend),
			Named::ArrowDown => (FilesNav::Step(columns), extend),
			Named::ArrowUp => (FilesNav::Step(-columns), extend),
			// PageDown/PageUp are focus-gated to the pane, so they never fight the terminal's own
			// scrollback on the same keys (`scroll_motion`) — that fires only while the terminal
			// holds the keyboard.
			Named::PageDown => (FilesNav::Step(page), extend),
			Named::PageUp => (FilesNav::Step(-page), extend),
			// Home/End land on an absolute end, right even with nothing selected yet.
			Named::Home => (FilesNav::Edge(false), extend),
			Named::End => (FilesNav::Edge(true), extend),
			Named::Tab if modifiers.shift() => (FilesNav::Step(-1), false),
			Named::Tab => (FilesNav::Step(1), false),
			Named::Enter => {
				let Some(path) = self.panes.pane.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				// Straight through the double-click's own handler, which is where "only a
				// directory can be entered" is decided.
				return self.on_files(FilesMessage::EntryOpened(path));
			}
			Named::F2 => {
				let Some(path) = self.panes.pane.cursor().map(str::to_owned) else {
					return iced::Task::none();
				};
				return self.on_files(FilesMessage::RenameStarted(path));
			}
			// F5 re-lists the directory on show, the same as the header ↻ button — the pane's
			// twin of the tree's F5, each refreshing the pane that holds the keyboard.
			Named::F5 => return self.on_files(FilesMessage::Refresh),
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		let show_hidden = self.panes.show_hidden();
		match nav {
			FilesNav::Step(delta) => self.panes.pane.step(show_hidden, delta, extend),
			FilesNav::Edge(to_last) => self.panes.pane.jump_to_edge(show_hidden, to_last, extend),
		}
		self.resolve_selected_link();
		// Only the keyboard scrolls: a click is already on a cell the user can see, and
		// scrolling under their cursor would move the thing they just aimed at.
		self.scroll_files_into_view()
	}

	/// Select whatever the rubber band now covers (§21). The grid's geometry belongs to the
	/// view, so the band is turned into cell indices there and back into paths here — the
	/// same split the arrow keys already use.
	fn apply_band(&mut self) {
		let Some(rect) = self.panes.pane.band().map(files::Band::rect) else {
			return;
		};
		let Some(directory) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		let rows = self.panes.rows();
		let paths: Vec<String> = ui::files::band_hits(
			rect,
			ui::files::columns(self.files_width()),
			rows.len(),
			self.panes.pane.scroll(),
		)
		.into_iter()
		.filter_map(|index| Some(explorer::join(&directory, &rows.get(index)?.name)))
		.collect();
		self.panes.pane.set_band_selection(paths);
	}

	/// Which entries a context-menu item acts on (§21): the whole selection when the menu
	/// was opened on part of it, that one entry otherwise. In grid order, since that is the
	/// order a list of copied names should come out in.
	fn action_targets(&self, path: &str) -> Vec<String> {
		if self.panes.pane.selected_count() > 1 && self.panes.pane.is_selected(path) {
			self.panes
				.pane
				.selected_rows(self.panes.show_hidden())
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
		if let Some(path) = self.panes.pane.cursor().map(str::to_owned)
			&& self.panes.pane.kind_of(&path) == Some(files::FilesKind::Link)
			&& self.panes.pane.link_target().is_none()
		{
			self.send_command(SshCommand::ReadLink(path));
		}
	}

	/// How wide the files pane is: the window less the folder tree's column beside it (§18, §19).
	/// The tree took its width off the terminal before; it takes it off the pane now, so every
	/// piece of the pane's geometry that keys off its width — the column count, the popup, the
	/// rubber band, the menus — reads this rather than the raw window width. `Explorer::reserved`
	/// is zero when the tree is hidden, so the pane is the full window then.
	fn files_width(&self) -> f32 {
		self.window_size.width - self.panes.tree.reserved()
	}

	/// Scroll the files pane so the selected cell is on screen (§20). The grid's geometry
	/// is the view's (`ui::files`), so the same arithmetic that lays the cells out is what
	/// works out where the selected one sits. The model is told the new offset as well as
	/// the widget, because the details popup is placed against it on this very frame.
	fn scroll_files_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.panes.pane.selected_index(self.panes.show_hidden()) else {
			return iced::Task::none();
		};
		let row = index / ui::files::columns(self.files_width());
		let current = self.panes.pane.scroll();
		// Already visible falls back to where it already is: the model and the widget are told the
		// offset either way, because the details popup is placed against it on this very frame.
		let offset = keep_visible(
			current,
			ui::files::grid_height(&self.panes.pane),
			ui::files::row_top(row),
			ui::files::CELL_HEIGHT,
		)
		.unwrap_or(current);
		self.panes.pane.set_scroll(offset);
		iced::widget::operation::scroll_to(
			ui::files::GRID_ID,
			iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: offset },
		)
	}

	/// The same, for the folder tree — one fixed-height row rather than a wrapping grid.
	fn scroll_tree_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.panes.tree.selected_index() else {
			return iced::Task::none();
		};
		let current = self.panes.tree.scroll();
		let offset = keep_visible(
			current,
			ui::explorer::tree_height(
				self.panes.pane.height(),
				self.panes.pane.path(),
				self.panes.tree.width(),
			),
			ui::pixels(index, ui::explorer::ROW_HEIGHT),
			ui::explorer::ROW_HEIGHT,
		)
		.unwrap_or(current);
		self.panes.tree.set_scroll(offset);
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
		let screen = terminal.screen();
		let hovered = ui::terminal::cell_under(&screen, point);
		// The head is resolved to a DOCUMENT position here (§40), where the viewport's own numbers are
		// still to hand: the pointer is over a screen row, but what it selects is the line that row is
		// showing — so the selection keeps covering that text however the scrollback then moves.
		let head = hovered.to_doc(screen);
		self.hover_cell = hovered;
		if self.selecting
			&& let Some(selection) = self.selection
		{
			self.selection = Some(selection.with_head(head));
		}
	}

	/// Begin a selection at the hovered cell (§10). Also closes any open context
	/// menu — a fresh press on the grid dismisses it — and the find bar (§35).
	fn on_grid_pressed(&mut self) {
		self.menu = None;
		// The find bar closes on a click-away too, like the menus (§35). It holds the keyboard while
		// it is open, and a press on the grid takes the focus off its field — so leaving it up would
		// leave every keystroke swallowed by a field that no longer has the cursor.
		self.search = None;
		// A click on the grid is also how the keyboard comes back to the shell (§20).
		self.set_focus(Focus::Terminal);
		// Any press on the grid ends a walk back through the commands (§34): the next Ctrl+Shift+O
		// starts from the newest again. The gutter branch below re-parks it on the tick that was
		// clicked, so a click on a prompt still says "carry on back from HERE".
		if let Some(terminal) = self.terminal.as_mut() {
			terminal.restart_output_walk();
		}
		let Some(terminal) = self.terminal.as_ref() else {
			return;
		};
		// The cell pressed on, as the document position it is showing (§40) — resolved before any of
		// the branches below, so the one place a mouse selection is anchored reads the viewport once.
		let anchor = self.hover_cell.to_doc(terminal.screen());
		// A click on a prompt tick in the left padding gutter selects that command's output (§34)
		// instead of starting a text selection. The ticks live inside `GRID_PADDING`; a press there
		// on a row whose prompt has a finished command resolves to it, and anything else — the gutter
		// beside a plain row — falls through to the ordinary selection below.
		if self.pointer.x < ui::terminal::GRID_PADDING
			&& self.select_output_at_gutter(self.hover_cell.row)
		{
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
		// A double click selects the word under the pointer, a triple the whole logical line (§42).
		// The count is kept here because `mouse_area` reports each press on its own; the expansion
		// itself is `ui::selection`'s, and what it hands back is an ordinary selection — so the grid
		// highlights it and Copy copies it with no further wiring, the same route §34 took.
		let click = self
			.clicks
			.press(self.hover_cell, std::time::Instant::now());
		// The screen is borrowed again here rather than kept from above: the gutter branch needs `self`
		// mutably, so holding one borrow across both is what the borrow checker (rightly) refuses.
		let expanded = self.terminal.as_ref().and_then(|terminal| match click {
			ui::selection::Click::Single => None,
			ui::selection::Click::Double => {
				ui::selection::Selection::word(terminal.screen(), anchor)
			}
			ui::selection::Click::Triple => {
				ui::selection::Selection::line(terminal.screen(), anchor)
			}
		});
		if let Some(selection) = expanded {
			self.selection = Some(selection);
			// And NO drag from here. The pointer is already sitting on the word, so the next mouse-move
			// would extend a selection anchored at the press cell — collapsing the span the double
			// click just made on the first stray pixel of movement. `ponytail:` dragging on from a
			// double click therefore does nothing rather than extending word by word, as xterm does.
			self.selecting = false;
			return;
		}
		self.selection = Some(ui::selection::Selection::new(anchor));
		self.selecting = true;
	}

	/// The URI of the OSC 8 hyperlink on a grid cell, if any (§24). `None` with no session,
	/// an out-of-bounds cell, or a cell that is not part of a link. Returned owned so the
	/// short-lived screen borrow is dropped before the caller acts on it.
	fn link_at(&self, cell: ui::selection::ScreenSpot) -> Option<String> {
		self.terminal
			.as_ref()?
			.screen()
			.cell(cell.row, cell.col)?
			.hyperlink()
			.map(str::to_owned)
	}

	/// Open an OSC 8 hyperlink (§24), or note it when its scheme is refused. Web and mail
	/// links open in the OS's default browser; anything else is blocked with a toast, since
	/// the URI is the remote's to choose (`link::open_uri` is the policy). Shared by Ctrl+click
	/// and the context menu's "Open link".
	fn follow_link(&mut self, uri: &str) {
		if !link::open_uri(uri) {
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

	/// Park the scrollback at an absolute offset, because the scrollbar was dragged there (§116). The
	/// grid resolved the pointer into an offset — it owns the bar's geometry — so there is nothing to
	/// decide here beyond passing it on; `ScrollMotion::To` clamps against the engine's own history.
	///
	/// Like the wheel, this changes only what the user is looking at: nothing is sent to the remote,
	/// the focus stays where it is, and no selection is started or moved. That last one is the reason
	/// the grid CAPTURES the press — a press that also reached the selection path would drag a
	/// selection across the screen behind the bar.
	fn on_terminal_scroll_to(&mut self, offset: u16) {
		if let Some(terminal) = self.terminal.as_mut() {
			terminal.scroll(term::ScrollMotion::To(offset));
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

	/// Select a finished command's output as a text selection (§34) — the Ctrl+Shift+O keybind. The
	/// first press takes the latest command; each press after it steps one command further back, so
	/// the key reads BACK through the session rather than only grabbing the last thing run. The
	/// terminal reveals the output (scrolling up to it when it has left the live screen) and hands
	/// back the document lines it fills; those become a stream selection the existing Copy path then
	/// copies. A no-op when no command has finished, and at the oldest one held — the selection stays
	/// where it is rather than wrapping round to the newest.
	fn select_command_output(&mut self) {
		if let Some(terminal) = self.terminal.as_mut()
			&& let Some(span) = terminal.select_output_back()
		{
			self.set_output_selection(span);
		}
	}

	/// The same for the command whose prompt tick was clicked in the left gutter (§34), returning
	/// whether a command was found there — so a gutter press on a row with no finished command falls
	/// through to an ordinary text selection.
	fn select_output_at_gutter(&mut self, row: u16) -> bool {
		let Some(terminal) = self.terminal.as_mut() else {
			return false;
		};
		let Some(span) = terminal.select_output_at_row(row) else {
			return false;
		};
		self.set_output_selection(span);
		true
	}

	/// Turn a located output span (§34) into the active grid selection, replacing any mouse selection
	/// and ending any drag — the one place both the keybind and the gutter click land, so the two can
	/// never build the selection differently. Dismisses any open context menu too, as a fresh grid
	/// interaction does. The span is already in document lines (§40), so an output taller than the
	/// screen is selected — and copied — in full.
	fn set_output_selection(&mut self, span: term::OutputSpan) {
		let start = ui::selection::DocSpot {
			line: span.start_line,
			col: 0,
		};
		let head = ui::selection::DocSpot {
			line: span.end_line,
			col: span.last_col,
		};
		// A RANGE, not a drag (§42): an output exactly one cell long — a command that printed a single
		// character — would otherwise read as "nothing selected" and be neither highlighted nor copyable.
		self.selection = Some(ui::selection::Selection::spanning(start, head));
		self.selecting = false;
		self.menu = None;
	}

	/// Open the scrollback find bar and focus its field (§35) — Ctrl+Shift+F. Already open, it is
	/// only refocused (and its query kept), so pressing the shortcut a second time puts the cursor
	/// back in the field instead of doing nothing the user has to reach for the mouse to fix.
	fn open_term_find(&mut self) -> iced::Task<Message> {
		if self.search.is_none() {
			self.search = Some(term::search::Search::default());
		}
		iced::widget::operation::focus(ui::terminal::SEARCH_INPUT_ID)
	}

	/// A new query in the find bar (§35): scan the whole scrollback and reveal the NEWEST match.
	/// Find-as-you-type, like a browser's bar — there is no cursor at the remote prompt to yank
	/// here (the field holds the keyboard while the bar is open), so showing the hit as it is typed
	/// costs nothing and saves a keystroke. A session-less tab scans nothing and simply shows no
	/// results.
	fn term_find_query(&mut self, query: String) {
		// The list built below describes the document as it is right now, so a re-scan left pending by
		// output that landed a moment ago (§44) has nothing left to do.
		self.search_stale = false;
		let matches = match self.terminal.as_ref() {
			Some(terminal) => terminal.find(&query),
			None => Vec::new(),
		};
		let Some(search) = self.search.as_mut() else {
			return;
		};
		search.query = query;
		search.set_matches(matches);
		let found = search.current();
		self.reveal_match(found);
	}

	/// Step to the neighbouring match (§35). The scan is REDONE first, so output that arrived since
	/// the query was typed joins the list rather than being invisible until the query is retyped;
	/// `refresh` keeps the current match by identity across that re-scan, so the step moves exactly
	/// one hit even when the list grew underneath it.
	fn term_find_step(&mut self, newer: bool) {
		self.rescan_find();
		let Some(search) = self.search.as_mut() else {
			return;
		};
		let found = search.step(newer);
		self.reveal_match(found);
	}

	/// Rebuild the open find bar's match list from the document as it stands, keeping the current hit
	/// by identity wherever it survived (§35). Three callers, all of them the document changing under
	/// the bar rather than the user asking for anything: a reflow (§43), output arriving (§44), and a
	/// step, which scans first so that a hit printed since the query was typed can be stepped onto.
	///
	/// Nothing is revealed and nothing is selected. A step does both afterwards because the user asked
	/// to move; the other two must not, or a shell printing under an open bar would drag the viewport
	/// about while it is being read. What the user sees change is the count and the washes.
	fn rescan_find(&mut self) {
		// Cleared first and unconditionally, because this is what stops the frame clock (§44): a tick
		// that finds nothing to do — the bar closed in the same batch — still has to end the ticking.
		self.search_stale = false;
		let Some(query) = self.search.as_ref().map(|search| search.query.clone()) else {
			return;
		};
		let matches = match self.terminal.as_ref() {
			Some(terminal) => terminal.find(&query),
			None => Vec::new(),
		};
		if let Some(search) = self.search.as_mut() {
			search.refresh(matches);
		}
	}

	// --- more than one account on one connection (§45) ---

	/// Put another identity's terminal on screen (§45).
	///
	/// The swap is the whole mechanism: the live view moves into the identity being left, and the
	/// arriving one's parked view becomes live. The SSH task is told which shell typing belongs to
	/// now, ahead of any keystroke for it — both ride one ordered channel, so they cannot cross.
	///
	/// The returned task re-fits the grid, because the view arriving was laid out for the window as
	/// it was when it was parked: a resize while it was away reached its pty (every shell is
	/// reflowed, §45) but not its emulator, and this is what brings the two back into step.
	fn switch_identity(&mut self, to: u64) -> iced::Task<Message> {
		if to == self.identity {
			return iced::Task::none();
		}
		// An identity still elevating has no terminal to show; its shell does not exist yet.
		if !self
			.identities
			.iter()
			.any(|identity| identity.id == to && identity.ready)
		{
			return iced::Task::none();
		}
		// The identity being LEFT must have an entry to be parked into. It always does — the login
		// identity is listed the moment the shell opens — but checking before anything moves means a
		// list that somehow disagreed would leave the view alone rather than drop a whole terminal on
		// the floor.
		if !self
			.identities
			.iter()
			.any(|identity| identity.id == self.identity)
		{
			return iced::Task::none();
		}
		// Taken out of the list first so the swap borrows nothing that is still inside it.
		let mut incoming = match self
			.identities
			.iter_mut()
			.find(|identity| identity.id == to)
		{
			Some(identity) => std::mem::take(&mut identity.work),
			None => return iced::Task::none(),
		};
		self.exchange(&mut incoming);
		let leaving = self.identity;
		if let Some(identity) = self
			.identities
			.iter_mut()
			.find(|identity| identity.id == leaving)
		{
			identity.work = incoming;
		}
		self.identity = to;
		self.send_command(SshCommand::SelectIdentity(to));
		// The file panes follow the same switch (§46) — and are announced AFTER `SelectIdentity`, on
		// the same ordered channel, so the listings cannot be answered by the account being left.
		self.reread_panes();
		// Nothing about the account switch belongs to the grid the user was on: a half-made
		// selection, a drag in flight, a click tally. They are all parked with it and the arriving
		// view brings its own.
		fit_terminal()
	}

	/// Read the tree and the files pane again as the account now selected (§46).
	///
	/// The panes are NOT parked per account the way the terminal is, and that is the deliberate
	/// choice: a scrollback is a record of what that account did, but a folder is a place, and the
	/// reason to become root is usually a file in the folder you are already looking at. So the path
	/// stays and the contents are re-read through the new account's eyes.
	///
	/// Both panes are emptied first. Until the new listing lands there is nothing on screen that
	/// belongs to the account just left — and if the new one cannot list at all, they stay empty
	/// beside the remote's own reason rather than quietly showing another account's files.
	fn reread_panes(&mut self) {
		let fetches = self.panes.reread();
		self.send_fetches(fetches);
	}

	/// The session is real: persist the target it was made for and read back everything it
	/// remembers (§14, §22, §27). Targets only — no secret goes in here.
	///
	/// `upsert_on_connect` adds the endpoint, or refreshes a known one while keeping its custom
	/// name, and hands back its key. It leaves a known endpoint's saved state alone, so the session
	/// snapshot and the forwards are still there to read straight afterwards — which they are, in
	/// ONE borrow, as owned values. That is the whole point of the function: the caller then acts on
	/// what it was given with nothing borrowed, instead of interleaving three short borrows with the
	/// `&mut self` calls that want the same cell (§26).
	fn adopt_target(&mut self, target: crate::targets::Target) -> Arrival {
		let key = self.targets.borrow_mut().upsert_on_connect(
			&target.host,
			target.port,
			&target.user,
			target.auth_kind,
			target.key_path,
			target.cert_path,
		);
		// What the FORM asked this target to become (§47), applied before anything reads the target
		// back: `elevate_on_connect` runs when the shell opens, and it reads the stored preference.
		// A blank field says nothing rather than "stay put", so a target's remembered account is not
		// erased by connecting from a form that never mentioned it — the dialog's own field, and
		// clearing the form's, are the two ways to change it.
		if let Some((account, kind, on_connect)) = self.form.elevation() {
			let moved = self
				.targets
				.borrow_mut()
				.set_elevation(&key, &account, kind, on_connect);
			if moved && let Err(error) = self.targets.borrow().save() {
				eprintln!("could not save targets: {error:#}");
			}
		}
		let targets = self.targets.borrow();
		let saved = targets.find(&key);
		Arrival {
			session: saved.map(crate::targets::Target::session),
			forwards: saved
				.map(|target| target.forwards.clone())
				.unwrap_or_default(),
			key,
		}
	}

	/// Remembered-secret bookkeeping for a connect that has just succeeded (§16).
	///
	/// A successful connect is the ONLY place a secret is persisted, and that is the rule this
	/// function exists to keep in one piece: the credentials are now known good, so a wrong password
	/// was never stored. With "Remember" on, store what dial captured; with it off, forget whatever
	/// the vault held for this endpoint. The target's flag is then synced to what the vault ACTUALLY
	/// holds, so the home list never promises a pre-fill that is not there.
	///
	/// All of it needs the vault unlocked, which the dial / open flow already ensured whenever a
	/// secret was in play. If it is locked — the user never engaged it — the flag is left as stored
	/// rather than being cleared on the strength of a vault nobody has opened.
	fn settle_remembered_secret(&mut self, key: &str) {
		if let Some(vault) = self.vault.borrow_mut().as_mut() {
			if let Some((endpoint, secret)) = self.pending_remember.take() {
				if let Err(error) = vault.store(&endpoint, secret) {
					eprintln!("could not save the vault: {error:#}");
				}
			} else if !self.form.remember
				&& let Err(error) = vault.forget(key)
			{
				eprintln!("could not update the vault: {error:#}");
			}
			self.targets
				.borrow_mut()
				.set_remembered(key, vault.get(key).is_some());
		}
		self.pending_remember = None;
	}

	/// Turn what the panes asked for into commands (§18, §19).
	///
	/// The one place the pair's [`panes::Fetches`](crate::panes::Fetches) becomes network traffic.
	/// `panes` decides WHAT is needed and can be tested doing it; this decides how to ask, which
	/// needs the channel and cannot be. Before there was a `Fetches` this pattern —
	/// "ask the model, then relay the listing" — was open-coded at fifteen call sites.
	fn send_fetches(&mut self, fetches: crate::panes::Fetches) {
		self.list_dirs(fetches.dirs);
		if let Some(request) = fetches.files {
			self.list_files(request);
		}
	}

	/// Swap the live terminal-side view with `other` (§45).
	///
	/// The one place that has to be COMPLETE: every field of `Workspace` is exchanged here, and a
	/// field added there without a line here would leak one account's state into another's pane. It
	/// is a swap rather than two moves so the caller ends up holding what was on screen, which is
	/// exactly what has to be parked.
	fn exchange(&mut self, other: &mut Workspace) {
		std::mem::swap(&mut self.terminal, &mut other.terminal);
		std::mem::swap(&mut self.selection, &mut other.selection);
		std::mem::swap(&mut self.selecting, &mut other.selecting);
		std::mem::swap(&mut self.hover_cell, &mut other.hover_cell);
		std::mem::swap(&mut self.clicks, &mut other.clicks);
		std::mem::swap(&mut self.search, &mut other.search);
		std::mem::swap(&mut self.search_stale, &mut other.search_stale);
	}

	/// An elevated shell is through its conversation (§45): it now has a terminal of its own, so
	/// give it one, close the dialog and put it on screen.
	fn on_identity_ready(&mut self, identity: u64, factors: u32) -> iced::Task<Message> {
		// The elevation resolved, so the answer it was holding is either stored or dropped (§47).
		// Before the early return below: an identity the list has lost is exactly the case where a
		// held credential must not be left in memory.
		if self
			.pending_elevation
			.as_ref()
			.is_some_and(|pending| pending.identity == identity)
		{
			self.settle_elevation_secret(factors);
			// The dialog was the thing asking; with the account up and running there is nothing left
			// on it to answer, so it closes rather than sitting over the new terminal.
			if matches!(self.modal, Some(Modal::Elevate(_))) {
				self.modal = None;
			}
		}
		let Some(entry) = self
			.identities
			.iter_mut()
			.find(|entry| entry.id == identity)
		else {
			return iced::Task::none();
		};
		entry.ready = true;
		// Its own emulator, parked until the switch below brings it forward. Built exactly like the
		// login shell's, so an elevated terminal is in every way the same terminal (§9).
		//
		// `get_or_insert_with`, not an assignment: output for this identity may already have built one
		// and put bytes in it. The session sends this event before the flush that carries the account's
		// greeting and first prompt precisely so it does not have to (`ssh::shell`), but a plain
		// assignment here would silently discard anything that did arrive first — which is the bug that
		// left an elevated terminal blank. Two ways of not losing it are better than one.
		entry.work.terminal.get_or_insert_with(new_emulator);
		self.switch_identity(identity)
	}

	/// An elevated shell has gone (§45): it exited, or it never opened.
	///
	/// If it was on screen, the login identity comes forward — there is always one, and it is the
	/// one shell that cannot go while the session lives. A reason is the remote's own words about
	/// its own policy, so it is shown: a user who cannot tell "wrong password" from "not in the
	/// sudoers file" can fix neither.
	fn on_identity_ended(&mut self, identity: u64, reason: Option<String>) -> iced::Task<Message> {
		let mut task = iced::Task::none();
		if identity == self.identity {
			task = self.switch_identity(bridge::LOGIN_IDENTITY);
		}
		self.identities.retain(|entry| entry.id != identity);
		let Some(reason) = reason else {
			return task; // an ordinary `exit` at an elevated prompt
		};
		// The elevation that failed was holding an answer; it goes now, stored nowhere (§47). The
		// factor count is irrelevant — nothing is kept from an elevation that did not happen — and
		// `settle_elevation_secret` is called with a count that cannot store, so the "unticked means
		// forget" half still runs.
		let automatic = self
			.pending_elevation
			.as_ref()
			.filter(|pending| pending.identity == identity)
			.map(|pending| pending.automatic);
		if automatic.is_some() {
			self.settle_elevation_secret(u32::MAX);
		}
		// Where the reason goes depends on who asked. With the dialog open it goes under the form,
		// beside the account that was refused, and the form goes back to asking so the name can be
		// corrected. A hands-free attempt from the target's stored preference has no dialog behind
		// it, so one is opened to carry the news — otherwise the session simply stays at the login
		// account with nothing said (§47).
		if automatic == Some(true) && self.elevate_form_mut().is_none() {
			task = iced::Task::batch([task, self.open_accounts_dialog()]);
		}
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Asking;
			form.error = Some(reason);
			return task;
		}
		// Nothing open to say it in: a toast says why without stealing the keyboard (§10).
		self.toast(reason);
		task
	}

	/// The session has ended, so every account it was a shell for has ended with it (§45): the list
	/// and the parked views.
	fn forget_identities(&mut self) {
		self.identities.clear();
		self.identity = bridge::LOGIN_IDENTITY;
		self.next_identity = 1;
	}

	/// Show one short message in the copy toast (§10) — used where something failed but nothing was
	/// asked, so a modal would be an interruption rather than a question.
	fn toast(&mut self, message: String) {
		self.snackbar = Some(Snackbar {
			message,
			shown_at: std::time::Instant::now(),
		});
	}

	/// Reveal a found match and select it (§35): the terminal scrolls it into view (centred, and
	/// left alone when it is already on screen), and the match's own coordinates — an absolute line
	/// and its columns — become an ordinary one-line selection. That is the whole reason this feature
	/// needs no rendering or clipboard work: the grid highlights a selection and Copy copies one,
	/// whatever put it there (§34 took the same route). Since §40 the selection speaks the same
	/// absolute lines a match does, so nothing is converted here at all. A match whose line has
	/// scrolled past the retained history cannot be shown, and leaves the view and the selection as
	/// they were.
	fn reveal_match(&mut self, found: Option<term::search::SearchMatch>) {
		let (Some(found), Some(terminal)) = (found, self.terminal.as_mut()) else {
			return;
		};
		if !terminal.reveal_line(found.line) {
			return;
		}
		let start = ui::selection::DocSpot {
			line: found.line,
			col: found.start_col,
		};
		let head = ui::selection::DocSpot {
			line: found.line,
			col: found.end_col,
		};
		// A RANGE, not a drag (§42): a ONE-CHARACTER query matches a single cell, and as a drag that
		// would read as "nothing selected" — the hit would be revealed and then not highlighted.
		self.selection = Some(ui::selection::Selection::spanning(start, head));
		self.selecting = false;
		self.menu = None;
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
		// A find bar left open across a session change would be searching a scrollback that no
		// longer exists, and would go on swallowing the keyboard (§35) — so it closes with the rest.
		self.search = None;
		// Whichever dialog was open belongs to the session it was asked about (§10, §18, §27). One
		// line, and it covers the two the three hand-written clears here used to forget: a delete
		// confirmation left holding one server's paths, and a "new folder" dialog left naming one
		// server's parent — either of which, on the next connect, would have acted on the NEW
		// server with the old one's arguments.
		self.modal = None;
		// Everything about moving bytes belongs to the session that asked for it (§17, §21, §29):
		// the picked batch, the queues behind it, a drag mid-hover, and — the one the twelve
		// hand-written clears here used to forget — a Resume offer, which would otherwise relaunch
		// a transfer against whatever server this tab connected to next (§16).
		//
		// `unfinished` is deliberately NOT cleared here (§16), for the same reason the carried
		// directory below is not: this runs on the way INTO a session as well as out of one, and
		// an offer that a dropped session left is meant to be adopted by the connect that is
		// opening. The teardown paths set it through `abandon_transfers` before calling this, and
		// the connect spends it straight after — matched against its own endpoint, so an offer
		// made on another server can never be replayed here.
		self.transfers.reset();
		// Every session starts with the keyboard at the shell (§20), and none is mid-resume:
		// a torn-down session has nothing to settle, and a fresh one sets this itself once it
		// knows whether it has a shell directory to replay (§22). Set straight rather than
		// through `set_focus`: opening or closing a session is not a focus move to report, and
		// the new session's remote starts out believing the shell is focused (§23), so the
		// reported baseline is reset to match — the window's own focus is left as it is.
		self.focus = Focus::Terminal;
		self.shell_focus_reported = true;
		self.resume_cwd = None;
		// The carried directory is deliberately NOT cleared here (§52): this runs on the way INTO a
		// session as well as out of one, and a copy's whole point is to be spent by the connect that
		// is opening. It is taken by that connect, and what it is not spent on it is matched
		// against, so nothing stale can be replayed into a later session.
		// The panes' own size and visibility are user preferences, not session state,
		// so `reset` deliberately leaves those alone.
		self.panes.reset();
		// A session's forwards die with it (§27): the worker drops its listeners when the session
		// ends, so the list belongs to this session and is cleared — the manager dialog over it
		// went with `modal` above. A fresh session re-establishes the target's saved set itself,
		// after this runs.
		self.forwards.clear();
	}

	/// A snapshot of this session's per-target UI state (§22): where the shell and files pane
	/// are, the `.*` filter, and the two pane sizes. One place names everything worth
	/// remembering — `persist_session` writes it, `restore_session` reads it back — so adding
	/// another value is one field here (and one on `Target`). The shell cwd is `None` on a
	/// server that announces none (§17); `set_session` treats a `None` as "leave it", so a
	/// silent session never erases what an earlier one recorded.
	fn capture_session(&self) -> crate::targets::SessionState {
		crate::targets::SessionState {
			// The panes' whole half of the snapshot, from the pair that owns it.
			terminal_path: self
				.terminal
				.as_ref()
				.and_then(term::Terminal::cwd)
				.map(str::to_owned),
			..self.panes.capture()
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
		// Non-overlapping borrows of the shared target cell (see `commit_rename`).
		let moved = self.targets.borrow_mut().set_session(&endpoint, session);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Apply a target's remembered session state to the panes before the first listing (§22):
	/// the `.*` filter and the two pane sizes go straight onto the models, and the resume
	/// paths (shell, pane) are handed back for the caller to drive the `cd` / pane / tree
	/// restore — coordination that belongs in `update`, not here. Each size is clamped to the
	/// same window fraction a splitter drag is, and applied only once the window size is known,
	/// so a restore before the first resize event cannot shrink a pane to its minimum.
	fn restore_session(
		&mut self,
		session: crate::targets::SessionState,
	) -> (Option<String>, Option<String>) {
		let resume = self.panes.restore(session, self.window_size);
		(resume.terminal, resume.pane)
	}

	/// Handle one event from the remote folder tree (§18). The model decides what the
	/// action means; this only relays the network side of it — the listings it asks for,
	/// the `cd` it types into the shell, the clipboard writes — and refits the grid when
	/// the pane's footprint changes.
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over ExplorerMessage: length is the number of tree actions, not depth"
	)]
	fn on_explorer(&mut self, message: ExplorerMessage) -> iced::Task<Message> {
		match message {
			ExplorerMessage::Toggled => {
				self.panes.tree.toggle();
				// A hidden pane cannot hold the keyboard: hand it back to the shell (§20).
				if !self.panes.tree.visible() && self.focus == Focus::Tree {
					self.set_focus(Focus::Terminal);
				}
				// The pane's width just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new column count.
				self.refit_grid();
			}
			ExplorerMessage::HiddenToggled => {
				self.panes.tree.toggle_hidden();
				// Persist the flip now (§14, §22): the toggle folds into the same per-target
				// snapshot as the paths and pane sizes, so it survives even a later hard exit.
				self.persist_session();
			}
			ExplorerMessage::PanePressed => self.focus_pane(Focus::Tree),
			ExplorerMessage::Scrolled(offset) => self.panes.tree.set_scroll(offset),
			ExplorerMessage::RowClicked(path) => {
				self.focus_pane(Focus::Tree);
				if let Some(fetch) = self.panes.tree.toggle_node(&path) {
					self.send_command(SshCommand::ListDir(fetch));
				}
				// Clicking a folder in the tree also points the files pane at it, WITHOUT
				// moving the shell — that is what makes the pane usable to look inside a
				// folder you are not in (§19).
				if let Some(request) = self.panes.pane.show(&path) {
					self.list_files(request);
				}
			}
			ExplorerMessage::RowRightClicked(path) => {
				self.focus_pane(Focus::Tree);
				self.panes.tree.select(&path);
				self.panes.tree.open_menu(path);
			}
			ExplorerMessage::PointerMoved(point) => self.panes.tree.set_pointer(point),
			ExplorerMessage::MenuDismissed => self.panes.tree.close_menu(),
			ExplorerMessage::RefreshDir(path) => {
				self.panes.tree.close_menu();
				// The menu's "Refresh" answers "is this folder still here, under this name, holding
				// these children?" Its CONTENTS come from re-listing the folder itself (forced open,
				// so the result shows at once); its own NAME and EXISTENCE come from re-listing its
				// PARENT — a rename or deletion made from the shell surfaces in the parent's listing,
				// never the folder's. The root has no parent, so only its contents refresh.
				if let Some(parent) = explorer::parent(&path).map(str::to_owned)
					&& let Some(fetch) = self.panes.tree.refresh_dir(&parent)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				if let Some(fetch) = self.panes.tree.expand(&path, true) {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::RefreshTree => {
				// The header ↻ button and F5: re-list every open folder, so all the expanded
				// content is current in one action — the user never has to work out which folders
				// a move touched. Each becomes its own listing request.
				self.panes.tree.close_menu();
				for fetch in self.panes.tree.refresh_open() {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::CollapseAll => {
				// The header's collapse-all button: close every branch back to the root's own
				// children. Local state only — nothing is re-fetched — so this needs no command.
				self.panes.tree.close_menu();
				self.panes.tree.collapse_all();
			}
			ExplorerMessage::Cd(path) => {
				// The tree's "Open in terminal" and its Enter key: a deliberate console move,
				// quoted so a folder name carrying a quote stays one argument (§18). The pane
				// then follows the `cd` it can see, the same as any other console move.
				self.panes.tree.close_menu();
				self.move_shell_to(&path);
			}
			ExplorerMessage::UploadHere(path) => {
				// The tree's "Upload…": pick local files to send into this folder (§17),
				// whichever directory the shell itself is in.
				self.panes.tree.close_menu();
				return browse_upload_into(path);
			}
			ExplorerMessage::UploadFolderHere(path) => {
				// The tree's "Upload folder…": pick a local folder to send whole into this one (§17).
				self.panes.tree.close_menu();
				return browse_upload_folder_into(path);
			}
			ExplorerMessage::NewFolderHere(path) => {
				// The tree's "New folder…": create a subfolder inside the right-clicked one (§18).
				self.panes.tree.close_menu();
				return self.begin_new_folder(path);
			}
			ExplorerMessage::DeleteStarted(path) => {
				// The tree's "Delete…": remove this folder and its whole subtree, once confirmed (§18).
				self.panes.tree.close_menu();
				self.begin_delete(vec![path]);
			}
			ExplorerMessage::RenameStarted(path) => {
				self.panes.tree.start_rename(path);
				// The root has no parent, so it declines to be renamed; only focus the
				// field when an edit actually opened.
				if self.panes.tree.editing().is_some() {
					return iced::widget::operation::focus(ui::explorer::RENAME_INPUT_ID);
				}
			}
			ExplorerMessage::RenameEdited(text) => self.panes.tree.edit_rename(text),
			ExplorerMessage::RenameCommitted => {
				if let Some((from, to)) = self.panes.tree.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			ExplorerMessage::CopyName(path) => {
				self.panes.tree.close_menu();
				let text = explorer::name(&path).to_owned();
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyRelative(path) => {
				self.panes.tree.close_menu();
				// The menu disables this item without a cwd, so this is belt and braces.
				let Some(cwd) = self.terminal.as_ref().and_then(term::Terminal::cwd) else {
					return iced::Task::none();
				};
				let text = explorer::relative(cwd, &path);
				return self.copy_to_clipboard(text);
			}
			ExplorerMessage::CopyPath(path) => {
				self.panes.tree.close_menu();
				return self.copy_to_clipboard(path);
			}
			ExplorerMessage::CopyCurrentPath => {
				// The header path, not a tree selection: copy the one directory the header
				// names — the files view's — verbatim, the twin of the pane's own button.
				if let Some(path) = self.panes.pane.path() {
					let text = path.to_owned();
					return self.copy_to_clipboard(text);
				}
			}
			ExplorerMessage::SplitterGrabbed => self.panes.tree.set_dragging(true),
			ExplorerMessage::SplitterDragged(pointer) => {
				if self.panes.tree.dragging() {
					// The splitter sits at the pane's left edge and the pane runs to the
					// window's right edge, so the pointer's distance from that edge IS the
					// width — no drag anchor to track. The clamp and the arithmetic are the
					// pair's, so this arm and the pane's twin below no longer restate them
					// with `width`/`x` swapped for `height`/`y`.
					self.panes.drag_tree_splitter(pointer.x, self.window_size);
					self.refit_grid();
				}
			}
			ExplorerMessage::SplitterReleased => self.panes.tree.set_dragging(false),
			// Hover only lights the bar (§18); no relayout, so no grid refit.
			ExplorerMessage::SplitterEntered => self.panes.tree.set_splitter_hovered(true),
			ExplorerMessage::SplitterExited => self.panes.tree.set_splitter_hovered(false),
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
	/// On a LOCAL session (§103) neither half of that line holds: the pane path is not a path on this
	/// platform, and the four shells the Local bar offers disagree about both the spelling of a path and
	/// the name of the command. So the shell composes its own (`local::shells::ShellKind::cd`), and a path
	/// that will not translate types nothing at all rather than a `cd` to somewhere invented.
	fn move_shell_to(&mut self, path: &str) {
		self.resume_cwd = None;
		let command = match self.local {
			Some(kind) => kind.cd(path),
			None => Some(format!("cd {}", explorer::shell_quote(path))),
		};
		let Some(command) = command else {
			return;
		};
		self.send_command(SshCommand::Input(format!("{command}\r").into_bytes()));
	}

	/// The status bar's "Sync" button (§19): move the console into the directory the files
	/// pane is showing. Browsing the pane or the tree leaves the console where it is, so the
	/// two drift apart on purpose; this is the deliberate, manual way to bring the console
	/// (and with it the tree and the title, which follow it) to the folder on show. A no-op
	/// with no shell or no directory on show; the button dims in those cases and when the two
	/// already agree, so pressing it always has something to do.
	fn on_sync(&mut self) {
		let Some(path) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		self.move_shell_to(&path);
	}

	/// The status bar's "Reveal" button (§19): Sync read backwards — bring the PANES to the shell.
	///
	/// The two drift apart in both directions, and until now only one of them could be closed from
	/// the bar. Browsing moves the pane and leaves the console alone (§19), and the shell's own
	/// re-announcement cannot bring the pane back: `Files::follow` acts on a *move*, and a shell
	/// standing still at the same prompt announces the same directory every time. So a browse three
	/// folders away was undone only by `cd`-ing the shell somewhere — moving the thing that was
	/// already where you wanted it — or by walking the tree back by hand.
	///
	/// It moves nothing on the remote. No `cd` is typed, no bytes reach the shell: this is the
	/// local view catching up with a shell that stays exactly where it is, which is why it is safe
	/// while a full-screen program is running and `move_shell_to` is not.
	///
	/// Four things happen, and all four are the point:
	///
	/// * the tree opens the chain down to the cwd and selects it — through `Explorer::reveal`, the
	///   **unguarded** one, since the whole reason to press this is that the tree has been walked away
	///   from a cwd that never changed;
	/// * the pane shows that directory (`Files::show`, the deliberate move, not `follow`); and
	/// * both follow-guards are seeded with the same path, so the next prompt's announcement is
	///   correctly read as "still there, nothing to do" rather than as a move — and a real `cd`
	///   after it still carries both panes along; and
	/// * any reconnect resume still settling is ended, the same rule an explicit `move_shell_to`
	///   follows and for the same reason (§22). The pin exists to hold the panes against the
	///   shell's login-then-`cd` announcements until it settles; the user saying out loud where
	///   the panes go outranks that, and leaving it armed would let it swallow the settle as
	///   "already there" and strand them at the login directory — the exact drift this button is
	///   for, caused by pressing it.
	///
	/// A no-op when the shell has never announced a cwd (§17: it needs OSC 7, or a shell configured
	/// to send it) — the button dims then, and whenever the panes are already there. Nothing is
	/// spent in that case either, the pin included: there is no ask to outrank.
	fn on_reveal(&mut self) {
		let Some(cwd) = self
			.terminal
			.as_ref()
			.and_then(term::Terminal::cwd)
			.map(str::to_owned)
		else {
			return;
		};
		self.resume_cwd = None;
		let fetches = self.panes.reveal(&cwd);
		self.send_fetches(fetches);
	}

	/// Browse the files pane into a directory (§19): a double-clicked folder, the toolbar's
	/// "up" button, or Enter on the keyboard. This points the PANE only — the console stays
	/// put, so you can look inside a folder you are not in without disturbing the shell. The
	/// console is moved separately and on purpose, by Sync or "Open in terminal"
	/// (`move_shell_to`); a real `cd` there is what brings the pane back into step, via the
	/// shell-follow (§19 "last one wins").
	fn browse_to(&mut self, path: &str) {
		let fetches = self.panes.browse(path);
		self.send_fetches(fetches);
	}

	/// Handle one event from the files pane (§19). Same division of labour as the tree's
	/// handler: the model decides what an action means, this relays the network side of
	/// it — the listings, the `cd`, the clipboard writes, the download — and refits the
	/// grid when the pane's footprint changes.
	#[expect(
		clippy::too_many_lines,
		reason = "a dispatch over FilesMessage: length is the number of pane actions, not depth"
	)]
	fn on_files(&mut self, message: FilesMessage) -> iced::Task<Message> {
		match message {
			FilesMessage::Toggled => {
				self.panes.pane.toggle();
				// A hidden pane cannot hold the keyboard: hand it back to the shell (§20).
				if !self.panes.pane.visible() && self.focus == Focus::Files {
					self.set_focus(Focus::Terminal);
				}
				// The pane's height just moved between it and the grid: reflow both the
				// local emulator and the remote pty to the new row count.
				self.refit_grid();
			}
			FilesMessage::PanePressed => {
				self.focus_pane(Focus::Files);
				// A cell's own `mouse_area` swallows the press that lands on it, so one that
				// reaches the pane missed them all. On the grid that starts a rubber band
				// (§21) — which also clears the selection, as every file manager's empty
				// space does; on the header or the notice line it only clears it.
				let pointer = self.panes.pane.pointer();
				let grid = pointer.y >= ui::files::HEADER_HEIGHT
					&& pointer.y
						<= ui::files::HEADER_HEIGHT + ui::files::grid_height(&self.panes.pane);
				if grid {
					self.panes
						.pane
						.begin_band(pointer, self.modifiers.control());
				} else if !self.modifiers.control() {
					self.panes.pane.deselect();
				}
			}
			FilesMessage::PaneReleased => self.panes.pane.end_band(),
			FilesMessage::PaneRightPressed => {
				// A right-press that reached the pane missed every cell, so it landed on the
				// empty grid: open the pane's own menu there (§17). The keyboard follows too,
				// as a left-press would.
				self.focus_pane(Focus::Files);
				self.panes.pane.open_pane_menu();
			}
			FilesMessage::PaneUploadHere => {
				// "Upload… here": send local files into the directory the pane is showing.
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return browse_upload_into(dir);
			}
			FilesMessage::PaneUploadFolderHere => {
				// "Upload folder… here": send a whole local folder into the directory on show (§17).
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return browse_upload_folder_into(dir);
			}
			FilesMessage::NewFolderHere => {
				// "New folder…": create a folder in the directory the pane is showing (§18).
				self.panes.pane.close_menu();
				let dir = self.panes.pane.path().unwrap_or("").to_owned();
				return self.begin_new_folder(dir);
			}
			FilesMessage::DeleteStarted(path) => {
				// "Delete…": remove the whole selection once confirmed (§18). A right-click inside
				// the selection kept it; one outside has already collapsed onto the clicked entry.
				self.panes.pane.close_menu();
				let targets = self.action_targets(&path);
				self.begin_delete(targets);
			}
			FilesMessage::DownloadFolder(path) => {
				// "Download folder…": recreate this remote directory's tree locally (§19). One
				// transfer at a time, like every other, so a running one blocks it.
				self.panes.pane.close_menu();
				if self.transfers.busy() {
					self.panes.pane.set_notice(transfer::BUSY_NOTICE.to_owned());
					return iced::Task::none();
				}
				return pick_download_tree_target(path);
			}
			FilesMessage::BandMoved(point) => {
				// Window coordinates from the capture layer: the pane's left edge is the window's
				// and it runs to the bottom, so only the vertical origin — the strip's top — comes off.
				let local = iced::Point::new(
					point.x,
					point.y - (self.window_size.height - self.panes.pane.height()),
				);
				self.panes.pane.set_pointer(local);
				if self.panes.pane.drag_band(local) {
					self.apply_band();
				}
			}
			FilesMessage::Scrolled(offset) => self.panes.pane.set_scroll(offset),
			FilesMessage::EntryClicked(path) => {
				self.focus_pane(Focus::Files);
				self.panes.pane.close_menu();
				let show_hidden = self.panes.show_hidden();
				// Shift runs a range from the anchor, Ctrl adds or removes this one, a plain
				// click takes it alone (§21).
				if self.modifiers.shift() {
					self.panes.pane.extend_selection(show_hidden, &path);
				} else if self.modifiers.control() {
					self.panes.pane.toggle_selection(&path);
				} else {
					self.panes.pane.select(&path);
				}
				// A clicked link is resolved the same way a walked-to one is (§20).
				self.resolve_selected_link();
			}
			FilesMessage::EntryOpened(path) => {
				self.panes.pane.close_menu();
				// A directory is entered — browsing the PANE there, the console left where it is
				// (§19). A FILE opens in a new editor tab (§32). The console is moved on purpose, by
				// Sync or "Open in terminal", never as a side effect of either.
				match self.panes.pane.kind_of(&path) {
					Some(files::FilesKind::Dir) => self.browse_to(&path),
					Some(_) => return self.request_open(path),
					None => {}
				}
			}
			FilesMessage::OpenStarted(path) => {
				// The menu's "Edit…" — the deliberate twin of a file double-click (§32).
				self.panes.pane.close_menu();
				return self.request_open(path);
			}
			FilesMessage::OpenInTerminal(path) => {
				// The pane's own "Open in terminal": the deliberate console move that a
				// double-click no longer is (§19). Same landing as the tree's item.
				self.panes.pane.close_menu();
				self.move_shell_to(&path);
			}
			FilesMessage::ParentOpened => {
				self.panes.pane.close_menu();
				// The toolbar disables the button at the root and before the first listing,
				// so this is belt and braces — and the parent is read HERE, from the
				// directory actually on show, rather than carried in the message. Browses the
				// PANE up; the console is left where it is (§19).
				let Some(parent) = self.panes.pane.path().and_then(explorer::parent) else {
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
				if !self.panes.pane.is_selected(&path) {
					self.panes.pane.select(&path);
				}
				self.panes.pane.open_menu(path);
				self.resolve_selected_link();
			}
			FilesMessage::PointerMoved(point) => {
				self.panes.pane.set_pointer(point);
				// A move with the button down is a band being stretched (§21).
				if self.panes.pane.drag_band(point) {
					self.apply_band();
				}
			}
			FilesMessage::MenuDismissed => self.panes.pane.close_menu(),
			// The sort menu is a plain view preference: none of these re-list or re-fetch, they
			// only re-order what `rows` already holds, so each just mutates and falls through to
			// the shared `Task::none()` below (§19).
			FilesMessage::SortMenuOpened => self.panes.pane.toggle_sort_menu(),
			FilesMessage::SortMenuDismissed => self.panes.pane.close_sort_menu(),
			// Picking a key or a direction leaves the menu open, so both halves of a sort can be
			// set in one visit; a click-away (or the button) closes it. Each pick persists the sort
			// into the connected target (§22), the same way the `.*` toggle folds into the snapshot,
			// so the chosen order survives a disconnect and even a later hard exit.
			FilesMessage::SortKeyPicked(key) => {
				self.panes.pane.pick_sort_key(key);
				self.persist_session();
			}
			FilesMessage::SortDirPicked(dir) => {
				self.panes.pane.pick_sort_dir(dir);
				self.persist_session();
			}
			FilesMessage::Refresh => {
				self.panes.pane.close_menu();
				if let Some(request) = self.panes.pane.refresh() {
					self.list_files(request);
				}
			}
			FilesMessage::CopyName(path) => {
				self.panes.pane.close_menu();
				let names = self.action_targets(&path);
				let text = join_lines(names.iter().map(|path| explorer::name(path).to_owned()));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyRelative(path) => {
				self.panes.pane.close_menu();
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
				self.panes.pane.close_menu();
				let text = join_lines(self.action_targets(&path));
				return self.copy_to_clipboard(text);
			}
			FilesMessage::CopyCurrentPath => {
				// The header path, not a selection: copy the one directory verbatim, with no
				// `action_targets` detour and no line-joining — there is only ever the one.
				if let Some(path) = self.panes.pane.path() {
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
				self.panes.pane.start_rename(path);
				return iced::widget::operation::focus(ui::files::RENAME_INPUT_ID);
			}
			FilesMessage::RenameEdited(text) => self.panes.pane.edit_rename(text),
			FilesMessage::RenameCommitted => {
				if let Some((from, to)) = self.panes.pane.commit_rename() {
					self.send_command(SshCommand::RenameDir { from, to });
				}
			}
			FilesMessage::Download(path) => {
				self.panes.pane.close_menu();
				// One transfer at a time — the status bar has one progress bar, and two
				// concurrent transfers would fight over it (§17). A batch respects that by
				// queueing; a batch started while something else runs still has to wait.
				if self.transfers.busy() {
					self.panes.pane.set_notice(transfer::BUSY_NOTICE.to_owned());
					return iced::Task::none();
				}
				// Folders are dropped rather than refused: a band that swept up a directory
				// alongside nine files should still fetch the nine (§21).
				let mut targets = self.action_targets(&path);
				targets.retain(|path| self.panes.pane.kind_of(path) != Some(files::FilesKind::Dir));
				return match targets.len() {
					0 => iced::Task::none(),
					// One file keeps the save dialog, which asks its own overwrite question.
					1 => pick_download_target(targets.remove(0)),
					_ => pick_download_folder(targets),
				};
			}
			FilesMessage::SplitterGrabbed => self.panes.pane.set_dragging(true),
			FilesMessage::SplitterDragged(pointer) => {
				if self.panes.pane.dragging() {
					// The splitter sits at the pane's top edge and the pane runs to the
					// window's bottom edge, so the pointer's distance from that edge IS the
					// height — no drag anchor to track. The tree's twin, on the other axis.
					self.panes.drag_pane_splitter(pointer.y, self.window_size);
					self.refit_grid();
				}
			}
			FilesMessage::SplitterReleased => self.panes.pane.set_dragging(false),
			// Hover only lights the bar (§19); no relayout, so no grid refit.
			FilesMessage::SplitterEntered => self.panes.pane.set_splitter_hovered(true),
			FilesMessage::SplitterExited => self.panes.pane.set_splitter_hovered(false),
		}
		iced::Task::none()
	}

	/// Open the "new folder" dialog for a folder to be created inside `parent` (§18): the tree
	/// folder that was right-clicked, or the directory the files pane is showing. Seeds the body
	/// with what it does and where, then focuses the name field so the user types straight away.
	/// An empty parent (the pane has shown nothing yet) asks nothing.
	fn begin_new_folder(&mut self, parent: String) -> iced::Task<Message> {
		if parent.is_empty() {
			return iced::Task::none();
		}
		let body = format!("{}\n\n{parent}", ui::terminal::NEW_FOLDER_DIALOG_BODY);
		self.open_modal(
			Modal::NewFolder {
				parent,
				name: String::new(),
			},
			&body,
		);
		iced::widget::operation::focus(ui::terminal::NEW_FOLDER_INPUT_ID)
	}

	/// Ask the server to create the folder the dialog is holding (§18). A blank name, or one
	/// carrying a path separator (which would put the folder somewhere other than asked), is not
	/// submittable — the dialog stays open rather than closing on nothing, the same rule the
	/// inline rename follows. A good name closes the dialog and sends the request.
	fn confirm_new_folder(&mut self) {
		let Some(Modal::NewFolder { parent, name }) = &self.modal else {
			return;
		};
		if !explorer::is_plain_name(name) {
			return;
		}
		let path = explorer::join(parent, name.trim());
		self.modal = None;
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
		let body = format!("{}\n\n{names}", ui::terminal::DELETE_DIALOG_BODY);
		self.open_modal(Modal::Delete(paths), &body);
	}

	/// Delete the held entries (§18) — only reached from a confirmed prompt. The panes re-list
	/// when the server reports it done (`on_deleted`), so nothing is dropped from the view on a
	/// hopeful guess.
	fn confirm_remote_delete(&mut self) {
		let paths = match self.modal.take() {
			Some(Modal::Delete(paths)) => paths,
			// Some other dialog, or none: put back what was open and send nothing. Taking the
			// paths is what closes the confirmation, so nothing can be deleted twice.
			other => {
				self.modal = other;
				return;
			}
		};
		self.send_command(SshCommand::Delete(paths));
	}

	/// Re-list a remote directory in whichever pane is showing it (§18): the tree, if it knows
	/// the folder, and the files pane, if that is the directory on show. The refresh a create or a
	/// delete triggers, so a new row appears — or a gone one vanishes — in place.
	fn refresh_remote_dir(&mut self, dir: &str) {
		let fetches = self.panes.refresh_dir(dir);
		self.send_fetches(fetches);
	}

	/// Entries were deleted (§18): step the files pane out of any folder that is now gone, drop
	/// the deleted subtrees from the tree, and re-list each parent they vanished from so the rows
	/// update in place. Done here rather than in a model because it spans both panes and the
	/// pane's own idea of where it is.
	fn on_deleted(&mut self, paths: &[String]) {
		let fetches = self.panes.deleted(paths);
		self.send_fetches(fetches);
	}

	/// Reflow the terminal to the current window *and* pane footprint (§18). The pane
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
		let Some(path) = self.panes.pane.path().map(str::to_owned) else {
			return;
		};
		self.send_command(SshCommand::ListFiles { path, request });
	}

	/// The window title (§17). Off-session it is just the app name; with a shell open it
	/// carries the session and — as soon as the shell announces one — the remote working
	/// directory, so the directory is visible without stealing room from the grid.
	fn title(&self) -> String {
		let connected = matches!(self.screen, AppScreen::Terminal);
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
		// The one floating card every dialog on this screen is placed by (§10) — only one is ever
		// open at a time, so one card serves them all; the arms that draw no dialog ignore it.
		let card = self.card;
		match &self.screen {
			// The shared target list is read through a short-lived borrow; `home::view` clones
			// every name it needs, so nothing in the returned element outlives the borrow (§26).
			AppScreen::Home => ui::home::view(
				self.targets.borrow().items(),
				ui::home::View {
					filter: &self.home_filter,
					selected: self.home_selected.as_deref(),
					rename: self.home_rename.as_ref(),
					menu_open: self.home_menu_open,
					confirm_delete: self.confirm_delete,
					dialog_body: &self.dialog_body,
					card,
					// The shells this machine can open (§103), searched once per run and kept — see
					// `local::shells::catalogue`, which is why this is free to ask for on every frame.
					shells: crate::local::shells::catalogue(),
					// Why the store was not read, when it was not (§110) — shown where the
					// "no saved targets yet" hint would otherwise be, which is the one place a
					// user looking for their targets is already looking.
					refusal: self.targets.borrow().refusal().map(str::to_owned),
				},
			),
			// The connect form, and — when the flow is holding a question — the dialog that asks
			// it, floating over the (dimmed) form rather than replacing it, so the page stays in
			// view behind it (§10). The second argument to `form_with_dialog` is what a click on
			// the BACKDROP does, and every one of them is the safe answer: reject, cancel, back.
			AppScreen::Connect => match &self.prompt {
				None => ui::connect::view(&self.form, self.form_focus),
				Some(Prompt::HostKey) => self.form_with_dialog(
					ui::host_key_view(&self.dialog_body, card),
					Message::RejectHostKey,
				),
				// Dismissing the mismatch override REJECTS — the safe default — so a backdrop
				// click, the ✕ and Esc all refuse a changed key rather than trusting it (§8).
				Some(Prompt::HostKeyChanged) => self.form_with_dialog(
					ui::host_key_changed_view(&self.dialog_body, card),
					Message::RejectHostKey,
				),
				Some(Prompt::Passphrase(input)) => self.form_with_dialog(
					ui::passphrase_view(input, self.passphrase_failed, &self.dialog_body, card),
					Message::PassphraseCancelled,
				),
				Some(Prompt::Interactive { fields, answers }) => self.form_with_dialog(
					ui::interactive_view(fields, answers, &self.dialog_body, card),
					Message::InteractiveCancelled,
				),
				Some(Prompt::Vault {
					input,
					confirm,
					creating,
					failed,
					..
				}) => self.form_with_dialog(
					ui::vault_view(input, confirm, *creating, *failed, &self.dialog_body, card),
					Message::VaultCancelled,
				),
				Some(Prompt::Failed) => self.form_with_dialog(
					ui::error_view(&self.dialog_body, card),
					Message::BackPressed,
				),
			},
			AppScreen::Connecting { status } => text(status).into(),
			AppScreen::Terminal => match &self.terminal {
				Some(terminal) => {
					let base = ui::terminal::view(
						terminal,
						ui::terminal::UiTerminalSession {
							endpoint: self.connection.as_deref().unwrap_or(""),
							local: self.local.is_some(),
							account: self.showing_account(),
						},
						self.selection.as_ref(),
						self.menu,
						ui::terminal::Modals {
							open: self.modal.as_ref(),
							forwards: &self.forwards,
							// Built for the frame rather than kept on the tab: see `Modals`.
							accounts: self.account_rows(),
							search: self.search.as_ref(),
							body: &self.dialog_body,
							card,
						},
						// The transfer flow itself (§17), borrowed rather than copied into a view
						// struct: the status bar, the two collision dialogs, the confirmation and
						// the pane's drop highlight all read it, and it is the only thing that
						// knows what it is holding.
						&self.transfers,
						ui::terminal::PanesView {
							explorer: &self.panes.tree,
							files: &self.panes.pane,
							focus: self.focus,
							// The pane's width (the window less the tree's column beside it), which is
							// what its grid wraps at and its overlays are placed against (§18, §19).
							width: self.files_width(),
							height: self.window_size.height,
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
			// A viewer tab (§32, §53). The in-tab editor's whole screen — toolbar, gutter, buffer —
			// comes from `ui::editor`; the picture's toolbar and zoomable image come from
			// `ui::preview`. Both borrow what they draw in place, so neither the buffer nor the
			// decoded pixels are copied per frame.
			AppScreen::Viewer => match &self.viewer {
				Some(Viewer::Editor(editor)) => ui::editor::view(editor, self.id),
				Some(Viewer::Picture(picture)) => ui::preview::view(picture, self.id),
				None => text("opening…").into(),
			},
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
}

/// Fetch the current window size and turn it into a `WindowResized`, so a newly
/// opened terminal fits the window immediately instead of waiting for the first
/// resize event (§9). `latest()` yields the most-recently-opened window and
/// `and_then` unwraps it — if there is somehow no window, this is a no-op.
/// A fresh emulator for a shell that has just opened, at the pty size cmote asks for and knowing
/// the cell's pixel size (§9, §23). Shared by the session's first shell and by every account
/// elevated into afterwards (§45), so an elevated terminal is in every way the same terminal.
fn new_emulator() -> term::Terminal {
	let mut terminal = term::Terminal::new(term::DEFAULT_ROWS, term::DEFAULT_COLS);
	terminal.set_cell_pixels(
		ui::cell_index(ui::terminal::CELL_WIDTH.round(), 1.0),
		ui::cell_index(ui::terminal::CELL_HEIGHT.round(), 1.0),
	);
	terminal
}

fn fit_terminal() -> iced::Task<Message> {
	iced::window::latest().and_then(|id| iced::window::size(id).map(Message::WindowResized))
}

/// Hand the window itself to `cursor` and `taskbar`, once, at start-up (§51, §54).
///
/// The hands are painted through a Win32 window subclass, so the one thing that layer needs is the
/// window's own handle — and `iced::window::run` is the only way iced offers to reach it: the
/// closure is handed the live window on the UI thread, which is also the thread that pumps its
/// messages, so the subclass is installed from the right place.
///
/// §54's taskbar progress needs the same handle for a different reason (`ITaskbarList3` addresses the
/// button by window), so it is taken here too rather than through a second boot task that would reach
/// for the identical thing.
///
/// `discard` because the installation raises no message: everything after it is driven by the tab
/// strip's own pointer events. Off Windows both calls resolve to no-ops that cost one boot task.
fn install_hand_cursors() -> iced::Task<Message> {
	iced::window::latest()
		.and_then(|id| {
			iced::window::run(id, |window| {
				use iced::window::raw_window_handle::RawWindowHandle;

				// A handle iced could not give us, or one that is not a Win32 window, means there is
				// nothing to subclass — the strip then keeps whatever cursor the toolkit gives it.
				let Ok(handle) = window.window_handle() else {
					return;
				};
				if let RawWindowHandle::Win32(win32) = handle.as_raw() {
					crate::cursor::install(win32.hwnd.get());
					crate::taskbar::attach(win32.hwnd.get());
				}
			})
		})
		.discard()
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

/// OS file-drop events, as upload triggers (§29). iced surfaces a drag from the desktop as window
/// events with NO pointer position, so a drop cannot be aimed at a widget — but the feature aims
/// every drop at the files pane's own directory anyway, so the fact of the drop is all it needs.
/// `FileHovered` lights the pane as the drop target, `FilesHoveredLeft` puts it out again, and
/// `FileDropped` carries the local path to upload. Every other event is dropped here, so the shell
/// is not woken on the rest of the stream — the same discipline `focus_events` keeps.
fn file_drop_events() -> iced::Subscription<Message> {
	iced::event::listen_with(|event, _status, _window| match event {
		iced::Event::Window(iced::window::Event::FileHovered(_)) => Some(Message::FileHovered),
		iced::Event::Window(iced::window::Event::FilesHoveredLeft) => Some(Message::FileDropLeft),
		iced::Event::Window(iced::window::Event::FileDropped(path)) => {
			Some(Message::FileDropped(path))
		}
		_ => None,
	})
}

/// The raw pointer, for the divider double-click (§48): where it is, and when a left button goes
/// down on it.
///
/// Two events rather than one because iced's press event carries no position — the position comes
/// from the move stream, and the press is only the moment. Everything else is dropped here, the same
/// discipline `focus_events` and `file_drop_events` keep.
///
/// This is the one subscription in cmote that raises a message per pointer MOVE, which is why
/// `subscription` asks for it only while the window is split: with one region there is no seam to
/// hit, so there is nothing for the position to be tested against. The handler on the other end
/// stores a `Point` and returns no task, so a move costs a field write and the repaint iced was
/// already going to do.
///
/// It has to be the raw stream because a press on a divider is the one click in the window that
/// reaches no widget: `pane_grid` captures it to start its own resize gesture and publishes nothing,
/// so there is no widget event to listen for and `mouse_area`'s own `on_double_click` would never
/// fire — its child has already captured the press by the time it looks.
fn divider_events() -> iced::Subscription<Message> {
	iced::event::listen_with(|event, _status, _window| match event {
		iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
			Some(Message::PointerMoved(position))
		}
		iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
			Some(Message::PointerPressed)
		}
		_ => None,
	})
}

/// The scrollback motion a Shift+navigation key asks for, or `None` for a key that does not
/// scroll (§23). PageUp/PageDown page through history, Home/End jump to the oldest retained
/// line and back to the live bottom — the xterm shifted-navigation set the terminal owns.
fn scroll_motion(named: iced::keyboard::key::Named) -> Option<term::ScrollMotion> {
	use iced::keyboard::key::Named;
	match named {
		Named::PageUp => Some(term::ScrollMotion::PageUp),
		Named::PageDown => Some(term::ScrollMotion::PageDown),
		Named::Home => Some(term::ScrollMotion::Top),
		Named::End => Some(term::ScrollMotion::Bottom),
		_ => None,
	}
}

/// The prompt-jump direction a Ctrl+Shift+arrow asks for, or `None` for any other key (§34). Up
/// climbs to the previous prompt, Down returns toward the live one — the direction the arrow
/// itself points through the scrollback.
fn prompt_jump(named: iced::keyboard::key::Named) -> Option<term::osc133::Osc133Direction> {
	use iced::keyboard::key::Named;
	match named {
		Named::ArrowUp => Some(term::osc133::Osc133Direction::Previous),
		Named::ArrowDown => Some(term::osc133::Osc133Direction::Next),
		_ => None,
	}
}

/// Whether this key press is plain TYPING — a character meant to appear at a prompt — rather
/// than a shortcut or a navigation key (§50). This is what decides that a keystroke aimed at a
/// focused pane was really meant for the shell.
///
/// Two conditions, and both are needed:
///   * a `Character` key, never a `Named` one. Enter, Tab, the arrows, F2, Esc, Backspace and
///     Delete are all `Named`, and every one of them is a pane's own key (§20) — a rule written
///     on the produced `text` instead would catch Enter (which carries `"\r"`) and take the
///     folder tree's "send the shell there" away from it;
///   * no Ctrl, Alt or Logo. Those make a combination, not a character: the files pane's Ctrl+A
///     takes the whole listing (§21), and Ctrl+Tab is the way out of a pane at all.
///
/// Shift is allowed through, since a capital letter is as much typing as a small one.
///
/// `ponytail:` on Windows AltGr arrives as Ctrl+Alt, so an AltGr character — `@` on an AZERTY
/// layout — reads as a combination here and does not, on its own, hand the keyboard over. The
/// letters typed around it do, which is the case that matters: a command starts with a word.
fn is_typing(key: &iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> bool {
	matches!(key, iced::keyboard::Key::Character(_))
		&& !modifiers.control()
		&& !modifiers.alt()
		&& !modifiers.logo()
}

/// Whether this press is the home screen's close-tab gesture: Ctrl+D, deliberately pressed (§30, §104).
///
/// A predicate rather than an inline condition for one reason — the `repeat` term is a bug fix that wants
/// a test, and what a key MEANS is the testable half of a handler that otherwise only returns an opaque
/// `iced::Task`.
///
/// `repeat` is the term that was missing. The gesture is TWO deliberate presses, the first of which lands
/// on the terminal screen and ends the shell; a Ctrl+D held for half a second arrived here as an
/// auto-repeat and closed the tab as well, so the tab vanished and took the home screen the user was meant
/// to land on with it. Holding a key is one press. Shift is not excluded, since Ctrl+Shift+D has no meaning
/// on this screen — that exclusion belongs to the terminal screen, where it is the way to send a bare EOF.
fn is_close_tab(
	key: &iced::keyboard::Key,
	modifiers: iced::keyboard::Modifiers,
	repeat: bool,
) -> bool {
	modifiers.control()
		&& !modifiers.alt()
		&& !modifiers.logo()
		&& !repeat
		&& matches!(key, iced::keyboard::Key::Character(character) if character.as_str() == "d")
}

/// Whether this press is the paste shortcut — Ctrl+V, or Ctrl+Shift+V (§10). Both paste the same
/// plain text: a terminal takes bytes for the remote shell, so there is no styled paste to
/// distinguish (pasting escape codes would be a paste-injection hazard, the one the
/// bracketed-paste strip guards). Matched on the PHYSICAL key so it holds on any layout — AZERTY,
/// Dvorak — and not only where V sits on QWERTY. Alt or Logo held makes it some other
/// combination, which belongs to the shell.
fn is_paste(physical: iced::keyboard::key::Physical, modifiers: iced::keyboard::Modifiers) -> bool {
	use iced::keyboard::key::{Code, Physical};
	modifiers.control()
		&& !modifiers.alt()
		&& !modifiers.logo()
		&& matches!(physical, Physical::Code(Code::KeyV))
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

/// Open the native file picker for an OpenSSH certificate (§7). Same async-`Task` shape as
/// `browse_key` — the modal dialog would block the GUI thread — with the pick arriving back as
/// `Message::CertFilePicked`. The certificate is validated (parsed) later, at connect time.
fn browse_cert() -> iced::Task<Message> {
	iced::Task::perform(
		rfd::AsyncFileDialog::new()
			.set_title("Select a certificate")
			.pick_file(),
		|handle| Message::CertFilePicked(handle.map(|handle| handle.path().to_path_buf())),
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

/// The secret a "Remember" tick should persist for this auth method (§16): the password, or a
/// non-empty pre-seeded key passphrase. An empty secret is nothing worth storing, so it maps to
/// `None` — the target flag then stays off and the vault keeps no empty entry. A key relying on
/// the interactive passphrase prompt (§7) has no form secret to capture here, so it is `None`
/// too; remembering a key passphrase means typing it on the form.
fn extract_secret(auth: &bridge::AuthMethod) -> Option<Secret> {
	let secret = match auth {
		// The two methods that carry a secret the form typed.
		bridge::AuthMethod::Password(secret)
		| bridge::AuthMethod::Key {
			passphrase: Some(secret),
			..
		} => secret,
		// A key with no form passphrase relies on the interactive prompt (§7), so there is nothing
		// here to capture; and the promptless methods carry no secret at all — interactive answers
		// every factor live, agent auth signs with a key the agent holds.
		bridge::AuthMethod::Key {
			passphrase: None, ..
		}
		| bridge::AuthMethod::Interactive
		| bridge::AuthMethod::Agent => return None,
	};
	if secret.expose().is_empty() {
		None
	} else {
		Some(secret.clone())
	}
}

/// The scroll offset that brings the band `top..top + height` into a `view`-tall window
/// currently scrolled to `offset` (§20) — shared by both panes, since "keep the thing
/// the arrow keys just selected on screen" is the same question for a row and a cell.
///
/// `None` means *do not move*: a keyboard walk across a screenful of entries should scroll only when
/// it reaches an edge, not re-centre on every press.
///
/// Two different cases answer `None`, and the second is why this is not as simple as it looks (§111).
/// The band being already inside the window is the obvious one. The other is a band the window CANNOT
/// contain — a cell in a pane dragged shorter than one row — where the clamp below computes an offset
/// that turns out to be the one already in force. The comparison is on `to_bits`, because the question
/// is not "are these numbers close" but "will the widget be handed a different f32 than it holds": a
/// scroll offset is stored and rendered verbatim, so the honest test is a bit-level one, and spelling
/// it as integers says that rather than leaving a bare `==` on floats for a reader to second-guess.
fn keep_visible(offset: f32, view: f32, top: f32, height: f32) -> Option<f32> {
	let wanted = if top < offset {
		top
	} else if top + height > offset + view {
		// Park it against the bottom edge — but never past its own top, or an item taller
		// than the window (a cell in a pane dragged short) would be shown headless.
		(top + height - view).max(0.0).min(top)
	} else {
		return None;
	};
	(wanted.to_bits() != offset.to_bits()).then_some(wanted)
}

/// Scroll the editor buffer so the cursor line is on screen, updating the model to match (§32).
/// Returns the new offset when a scroll is needed, or `None` when the line already shows (or the
/// viewport is not measured yet). Shared by a plain edit and every Find jump — both move the cursor
/// and want it followed. A free function, not a `Tab` method, so it can be called while a `&mut
/// Editor` is already borrowed inside `on_editor` without re-borrowing the whole tab.
fn follow_editor_cursor(editor: &mut crate::editor::Editor) -> Option<f32> {
	let view_height = editor.view_height();
	if view_height <= 0.0 {
		return None;
	}
	let top = ui::editor::line_top(editor.cursor_line());
	let offset = keep_visible(editor.scroll(), view_height, top, ui::editor::LINE_HEIGHT)?;
	// Pre-seat the offset so a second keystroke arriving before the scrollable reports back still
	// measures against the value we just asked for.
	editor.set_scroll_y(offset);
	Some(offset)
}

/// The horizontal half of the cursor-follow (§32) — the mirror of `follow_editor_cursor` on the X
/// axis. A fixed-width `text_editor` no longer scrolls to keep the cursor's column in view, so this
/// does it: bring the cursor's column x into the visible width, pre-seating the horizontal offset.
/// Returns the new offset when a scroll is needed, else `None` (already visible, or width unmeasured).
fn follow_editor_cursor_x(editor: &mut crate::editor::Editor) -> Option<f32> {
	let view_width = editor.view_width();
	if view_width <= 0.0 {
		return None;
	}
	let left = ui::editor::col_x(editor.cursor_display_column());
	let offset = keep_visible(
		editor.scroll_x(),
		view_width,
		left,
		ui::editor::CHAR_ADVANCE,
	)?;
	editor.set_scroll_x(offset);
	Some(offset)
}

/// Follow the cursor on both axes after a move (§32) and return the one `scroll_to` task that brings
/// it on screen, or `None` when neither axis needs to move. Both follows pre-seat the model, and the
/// task always carries BOTH offsets so scrolling one axis never resets the other to zero.
fn follow_editor_cursor_task(editor: &mut crate::editor::Editor) -> Option<iced::Task<Message>> {
	let moved_y = follow_editor_cursor(editor).is_some();
	let moved_x = follow_editor_cursor_x(editor).is_some();
	(moved_x || moved_y).then(|| scroll_editor_to(editor.scroll_x(), editor.scroll()))
}

/// The `scroll_to` task that moves the editor buffer to `(x, y)` (§32) — the operation the panes use
/// to bring a selected cell on screen, here on both axes so a horizontal follow keeps the vertical
/// offset and vice versa.
fn scroll_editor_to(x: f32, y: f32) -> iced::Task<Message> {
	iced::widget::operation::scroll_to(
		ui::editor::BUFFER_SCROLL_ID,
		iced::widget::scrollable::AbsoluteOffset { x, y },
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	// An `App` with a live emulator and an open command channel, so `send_command` succeeds
	// and its bytes can be read back off `rx`. The window starts focused with the ring on the
	// shell — the baseline a program assumes — so a focus change is measured against it.
	fn app_with_terminal(rx_cap: usize) -> (Tab, mpsc::Receiver<SshCommand>) {
		let (tx, rx) = mpsc::channel(rx_cap);
		let app = Tab {
			command_tx: Some(tx),
			terminal: Some(term::Terminal::new(24, 80)),
			// A tab with a live terminal is ON the terminal screen — the fixture left this at its
			// `Home` default for a long time and nothing minded, because `on_key` only ever ran on
			// this screen in production and so never had to ask. `keyboard_claim` does ask, since
			// which claimants exist at all is a property of the screen.
			screen: AppScreen::Terminal,
			window_focused: true,
			shell_focus_reported: true,
			focus: Focus::Terminal,
			..Tab::default()
		};
		(app, rx)
	}

	// One chunk of output from the LOGIN shell (§45) — the identity every test's terminal is,
	// since none of them elevate unless they say so.
	fn shell_output(bytes: &[u8]) -> SshEvent {
		SshEvent::Output {
			identity: bridge::LOGIN_IDENTITY,
			bytes: bytes.to_vec(),
		}
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
	/// off the shell to a side pane (§23) — and hears each edge only once.
	#[test]
	fn focus_reporting_answers_window_and_pane_changes() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(b"\x1b[?1004h");

		// The window loses, then regains, OS focus.
		app.on_window_focus(false);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[O"[..]));
		app.on_window_focus(true);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[I"[..]));

		// The keyboard ring moving off the shell to a side pane is a focus-out to the remote,
		// which knows nothing of cmote's panes.
		app.set_focus(Focus::Files);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"\x1b[O"[..]));

		// Moving between two panes never restores the shell's focus, so nothing more is sent.
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

	// The next command queued for the SSH worker, or `None` if nothing was sent. Broader than
	// `next_input`, since the forward flow sends `AddForward` / `RemoveForward`, not `Input`.
	fn next_command(rx: &mut mpsc::Receiver<SshCommand>) -> Option<SshCommand> {
		rx.try_recv().ok()
	}

	// Open the tunnels dialog if it is not already up and type a forward into its add form (§27) —
	// the way the user does, since the form lives INSIDE the open modal and there is no field on
	// the tab to set. Driving it through `update` is also what pins the wiring: an edit reaching a
	// closed dialog would silently do nothing, and these tests would catch it.
	fn type_forward(app: &mut Tab, kind: crate::forward::ForwardKind, listen: &str, to: &str) {
		if !matches!(app.modal, Some(Modal::Forwards(_))) {
			let _ = app.open_forwards_dialog();
		}
		let _ = app.update(Message::ForwardKindSelected(kind));
		let _ = app.update(Message::ForwardListenChanged(listen.to_owned()));
		let _ = app.update(Message::ForwardToChanged(to.to_owned()));
	}

	// The open tunnels dialog's add form, for assertions about what it is left holding.
	fn forward_form(app: &Tab) -> &ui::forward::ForwardForm {
		match &app.modal {
			Some(Modal::Forwards(form)) => form,
			other => panic!("the tunnels dialog is not open: {other:?}"),
		}
	}

	/// Only ONE dialog can be over this screen (§10): they share the body buffer and the card, so
	/// opening one has to close whatever was up. As four separate fields it did not — both stayed
	/// set, and both cards drew, one on top of the other.
	#[test]
	fn opening_a_dialog_closes_the_one_before_it() {
		let (mut app, _rx) = app_with_terminal(16);
		app.begin_delete(vec!["/srv/a".to_owned()]);
		assert!(matches!(app.modal, Some(Modal::Delete(_))));
		app.on_disconnect_pressed();
		assert!(matches!(app.modal, Some(Modal::Disconnect)));
	}

	// The shell-integration dialog, driven to the point where it has an answer to act on (§17) —
	// the probe out, the server's reply in. Every test below starts here, because nothing about
	// the dialog can be exercised until the server has said what it found.
	fn probed(
		shell: Option<crate::integration::IntegrationShell>,
		path: &str,
		installed: bool,
	) -> (Tab, mpsc::Receiver<SshCommand>) {
		let (mut app, mut rx) = app_with_terminal(16);
		app.connection = Some("root@sybille-rec:22".to_owned());
		let _ = app.update(Message::IntegrationPressed);
		let _ = next_command(&mut rx); // the probe itself, asserted on in its own test
		let _ = app.on_ssh_event(SshEvent::IntegrationProbed {
			shell,
			path: path.to_owned(),
			installed,
		});
		(app, rx)
	}

	/// The dialog asks about the LOGIN account (§17) — the one whose shell a reconnect opens, not
	/// whichever account the panes have been elevated to. The name comes off the endpoint, the only
	/// place the tab still holds it once the connect form has been left.
	#[test]
	fn the_shell_integration_dialog_asks_about_the_login_account() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.connection = Some("rocky@gw-test:22".to_owned());
		let _ = app.update(Message::IntegrationPressed);
		assert!(matches!(
			app.modal,
			Some(Modal::Integration(Integration::Asking))
		));
		assert!(
			matches!(next_command(&mut rx), Some(SshCommand::ProbeIntegration { user }) if user == "rocky")
		);
	}

	/// Reading is not writing (§17). The probe answers, the dialog fills with the block, and NOTHING
	/// has been sent to the server — the whole promise of this dialog is that the user sees the text
	/// before their config file is touched.
	#[test]
	fn a_silent_bash_is_shown_the_block_before_anything_is_written() {
		use crate::integration::IntegrationShell;

		let (mut app, mut rx) = probed(Some(IntegrationShell::Bash), "/root/.bashrc", false);
		assert!(matches!(
			app.modal,
			Some(Modal::Integration(Integration::Found { .. }))
		));
		assert!(next_command(&mut rx).is_none(), "the probe wrote nothing");

		// Now the explicit act, and only now.
		let _ = app.update(Message::IntegrationInstall);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::WriteIntegration { path, shell, install })
				if path == "/root/.bashrc" && shell == IntegrationShell::Bash && install
		));
		assert!(matches!(
			app.modal,
			Some(Modal::Integration(Integration::Writing))
		));
	}

	/// A file that already carries the block is offered removal instead, and removal is the same
	/// command with the flag turned over (§17) — one round trip, one code path, so an install and a
	/// removal cannot drift apart.
	#[test]
	fn a_file_that_already_has_the_block_is_offered_its_removal() {
		use crate::integration::IntegrationShell;

		let (mut app, mut rx) = probed(Some(IntegrationShell::Zsh), "/home/cme/.zshrc", true);
		let _ = app.update(Message::IntegrationRemove);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::WriteIntegration { path, install, .. })
				if path == "/home/cme/.zshrc" && !install
		));
	}

	/// A shell cmote has no block for is never written to (§17). fish announces its own directory,
	/// and an unrecognised login shell is one cmote must not guess at — writing bash syntax into a
	/// ksh rc file is how an account loses its login. The dialog offers no button in either case;
	/// this pins that a message arriving anyway still sends nothing.
	#[test]
	fn a_shell_cmote_has_no_block_for_is_never_written_to() {
		use crate::integration::IntegrationShell;

		for shell in [None, Some(IntegrationShell::Fish)] {
			let (mut app, mut rx) = probed(shell, "/home/cme/.config/fish/config.fish", false);
			let _ = app.update(Message::IntegrationInstall);
			assert!(
				next_command(&mut rx).is_none(),
				"nothing is written for {shell:?}"
			);
		}
	}

	/// An answer for a dialog the user has closed is dropped (§17). Re-opening it on their behalf
	/// would be the app talking over them — and would put an Install button under a cursor that has
	/// moved on to the shell.
	#[test]
	fn an_answer_for_a_closed_dialog_is_dropped() {
		use crate::integration::IntegrationShell;

		let (mut app, mut rx) = app_with_terminal(16);
		app.connection = Some("root@sybille-rec:22".to_owned());
		let _ = app.update(Message::IntegrationPressed);
		let _ = next_command(&mut rx);
		let _ = app.update(Message::IntegrationClosed);

		let _ = app.on_ssh_event(SshEvent::IntegrationProbed {
			shell: Some(IntegrationShell::Bash),
			path: "/root/.bashrc".to_owned(),
			installed: false,
		});
		assert!(app.modal.is_none(), "the dialog stays closed");
	}

	/// A refused errand is a dialog message, not a session failure (§17). The remote said no to a
	/// side errand on its own channel; the shell in front of the user is untouched, and so is the
	/// screen it is drawn on.
	#[test]
	fn a_refused_errand_leaves_the_session_alone() {
		use crate::integration::IntegrationShell;

		let (mut app, _rx) = probed(Some(IntegrationShell::Bash), "/root/.bashrc", false);
		app.screen = AppScreen::Terminal;
		let _ = app.on_ssh_event(SshEvent::IntegrationFailed(
			"could not open /root/.bashrc: permission denied".to_owned(),
		));
		assert!(matches!(
			app.modal,
			Some(Modal::Integration(Integration::Done))
		));
		assert!(app.terminal.is_some(), "the shell is still there");
		assert!(
			matches!(app.screen, AppScreen::Terminal),
			"and still on screen"
		);
	}

	/// Who holds the keyboard when several things could (§10, §17, §18, §35). This is the assertion
	/// that could not be written before: the priority was the source order of seven `if` blocks, so
	/// there was no value to compare against and no way to name a pair.
	#[test]
	fn the_keyboard_goes_to_one_claimant_in_a_stated_order() {
		let (mut app, _rx) = app_with_terminal(16);
		assert_eq!(app.keyboard_claim(), None, "nothing is holding it");

		// The find bar is ranked last, so anything else opened over it takes precedence.
		let _ = app.open_term_find();
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::Find));

		app.panes.tree.start_rename("/srv".to_owned());
		assert_eq!(
			app.keyboard_claim(),
			Some(KeyboardClaim::TreeRename),
			"a rename field outranks the find bar"
		);

		// A dialog outranks the rename, which is what stops a key reaching two fields at once.
		let _ = app.begin_new_folder("/srv".to_owned());
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::Modal));

		// And dismissing them gives the keyboard back in the reverse order, one at a time.
		app.dismiss(KeyboardClaim::Modal);
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::TreeRename));
		app.dismiss(KeyboardClaim::TreeRename);
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::Find));
		app.dismiss(KeyboardClaim::Find);
		assert_eq!(app.keyboard_claim(), None);
	}

	/// The home screen's claimants are the home screen's (§14). A terminal-screen holder is not
	/// consulted there and vice versa, which is what the screen match in `keyboard_claim` says.
	#[test]
	fn the_home_screen_has_claimants_of_its_own() {
		let mut app = Tab::default();
		assert!(matches!(app.screen, AppScreen::Home));
		assert_eq!(app.keyboard_claim(), None);

		app.home_rename = Some(ui::home::RenameState {
			key: "one".to_owned(),
			text: "one".to_owned(),
		});
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::TargetRename));

		// The delete confirmation outranks the rename: a stray Enter must not open a connection
		// from behind the modal.
		app.confirm_delete = true;
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::DeleteTarget));
	}

	/// A dialog owns the keyboard while it is up (§10, §18). Its own field types through the widget
	/// tree, so a key reaching the shell as well would be typing at the remote prompt at the same
	/// time — exactly what the inline rename fields already guard against.
	#[test]
	fn a_dialog_takes_the_keyboard_from_the_shell() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = app_with_terminal(16);
		let _ = app.begin_new_folder("/srv".to_owned());
		let _ = app.on_key(character_press("x", Code::KeyX, Modifiers::empty()));
		assert_eq!(next_input(&mut rx), None, "the shell heard nothing");
		assert!(!app.shell_owns_keyboard());

		// Esc closes it, and creates nothing — the same as the ✕ and the backdrop.
		let _ = app.on_key(key_press(Named::Escape, Code::Escape, Modifiers::empty()));
		assert!(app.modal.is_none());
		assert!(next_command(&mut rx).is_none());
		assert!(app.shell_owns_keyboard());
	}

	/// SECURITY (§16, §12): a secret captured for ONE attempt must never be stored on the back of a
	/// later one. The capture happens when Connect is pressed with Remember ticked; the store happens
	/// only on a successful connect. A failed attempt in between has to drop it, or the two ends stop
	/// describing the same connection.
	#[test]
	fn a_failed_attempt_leaves_no_secret_for_a_later_connect_to_store() {
		let (mut app, _rx) = app_with_terminal(16);
		app.pending_remember = Some(("u@a:22".to_owned(), Secret::new("hunter2".to_owned())));
		// A dial's own capture: the target it would save if the session opened (§14).
		app.form.host = "a".to_owned();
		app.form.port = "22".to_owned();
		app.form.user = "u".to_owned();
		app.form.auth_kind = AuthKind::Password;
		app.form.password = "hunter2".to_owned();
		let _ = app.dial(app.form.validate().expect("a valid form"));
		assert!(app.pending_target.is_some(), "the dial captured a target");

		let _ = app.on_ssh_event(SshEvent::Error("authentication failed".to_owned()));

		assert!(
			app.pending_remember.is_none(),
			"the secret belonged to the attempt that just failed"
		);
		assert!(app.pending_target.is_none());
	}

	/// SECURITY (§16): pressing Connect with Remember OFF captures nothing — and clears anything a
	/// previous press captured. Otherwise a secret from an earlier attempt would still be sitting
	/// there when this connection succeeds, and would be stored under the EARLIER endpoint: a
	/// password persisted for a host the user is not even connecting to, without ticking anything.
	#[test]
	fn a_connect_with_remember_off_captures_nothing_and_clears_what_came_before() {
		let (mut app, _rx) = app_with_terminal(16);
		app.pending_remember = Some(("u@a:22".to_owned(), Secret::new("hunter2".to_owned())));

		app.form.host = "b".to_owned();
		app.form.port = "22".to_owned();
		app.form.user = "u".to_owned();
		app.form.auth_kind = AuthKind::Password;
		app.form.password = "different".to_owned();
		app.form.remember = false;
		let _ = app.on_connect_pressed();

		assert!(
			app.pending_remember.is_none(),
			"nothing was ticked, so there is nothing to store — for this host or any other"
		);
	}

	/// SECURITY (§8): an unknown host key is trusted ONLY by an explicit choice. The prompt itself
	/// sends nothing — the handshake is parked on the far side — and Reject sends a refusal without
	/// moving on to "authenticating", so a rejected server never looks like a connecting one.
	#[test]
	fn an_unknown_host_key_is_trusted_only_by_an_explicit_choice() {
		let (mut app, mut rx) = app_with_terminal(16);
		let _ = app.on_ssh_event(SshEvent::HostKey("SHA256:aaaa".to_owned()));
		assert!(matches!(app.prompt, Some(Prompt::HostKey)));
		assert!(matches!(app.screen, AppScreen::Connect));
		assert!(
			next_command(&mut rx).is_none(),
			"asking is not answering: nothing goes back until the user chooses"
		);

		let _ = app.update(Message::RejectHostKey);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::HostKeyResponse(HostKeyChoice::Reject))
		));
		assert!(
			!matches!(app.screen, AppScreen::Connecting { .. }),
			"a refusal does not read as a connection in progress"
		);
	}

	/// SECURITY (§8): a CHANGED host key is the man-in-the-middle case, so rejecting is the default
	/// and trusting is the deliberate act. Every dismissal route on that dialog carries
	/// `RejectHostKey` — the ✕, the backdrop, and now Esc — and only the two explicit buttons pin.
	#[test]
	fn a_changed_host_key_rejects_unless_the_user_says_otherwise() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = app_with_terminal(16);
		let _ = app.on_ssh_event(SshEvent::HostKeyChanged {
			stored: "SHA256:old".to_owned(),
			presented: "SHA256:new".to_owned(),
		});
		assert!(matches!(app.prompt, Some(Prompt::HostKeyChanged)));
		assert!(next_command(&mut rx).is_none());

		// Both fingerprints are in the copyable body, so the user can compare them out of band.
		let body = app.dialog_body.text();
		assert!(body.contains("SHA256:old") && body.contains("SHA256:new"));

		// A key pressed in the frame the dialog appeared must not reach the form's ring underneath —
		// Enter there would press Connect (§10).
		let _ = app.on_form_key(key_press(Named::Enter, Code::Enter, Modifiers::empty()));
		assert!(next_command(&mut rx).is_none());
		assert!(matches!(app.prompt, Some(Prompt::HostKeyChanged)));

		// Replacing the pinned key is the deliberate act, and only then does the handshake go on.
		let _ = app.update(Message::ReplaceHostKey);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::HostKeyResponse(HostKeyChoice::Pin))
		));
		assert!(matches!(app.screen, AppScreen::Connecting { .. }));
		assert!(app.prompt.is_none(), "the question is answered and gone");
	}

	/// A prompt holds the secret being typed, so dismissing it drops that secret (§7, §12) — there
	/// is no buffer left on the tab for a later prompt to inherit, or for a Debug dump to find.
	#[test]
	fn dismissing_a_prompt_drops_what_was_typed_into_it() {
		let (mut app, mut rx) = app_with_terminal(16);
		let _ = app.on_ssh_event(SshEvent::NeedPassphrase);
		let _ = app.update(Message::PassphraseChanged("hunter2".to_owned()));
		assert!(matches!(&app.prompt, Some(Prompt::Passphrase(input)) if input == "hunter2"));

		let _ = app.update(Message::PassphraseCancelled);
		assert!(app.prompt.is_none());
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::Disconnect)
		));
		// And the next prompt starts empty rather than showing the abandoned attempt.
		let _ = app.on_ssh_event(SshEvent::NeedPassphrase);
		assert!(matches!(&app.prompt, Some(Prompt::Passphrase(input)) if input.is_empty()));
	}

	/// A wrong master passphrase re-asks with the hint and EMPTY fields (§16, §12): the rejected
	/// value is dropped rather than left in the buffer the next attempt types over. The deferred
	/// action it was blocking survives the re-ask, or the retry would unlock into nothing.
	#[test]
	fn a_refused_vault_passphrase_re_asks_empty_and_keeps_what_it_was_blocking() {
		// Create mode with a mismatched confirmation — no vault file is touched, so this needs no disk.
		let mut app = Tab {
			prompt: Some(Prompt::Vault {
				input: "one".to_owned(),
				confirm: "two".to_owned(),
				creating: true,
				failed: false,
				pending: VaultPending::Prefill("u@h:22".to_owned()),
			}),
			..Tab::default()
		};
		let _ = app.on_vault_submitted();
		match &app.prompt {
			Some(Prompt::Vault {
				input,
				confirm,
				failed,
				pending,
				..
			}) => {
				assert!(input.is_empty() && confirm.is_empty());
				assert!(*failed, "the hint says the two did not match");
				assert!(matches!(pending, VaultPending::Prefill(_)));
			}
			other => panic!("expected the prompt to re-ask, got {other:?}"),
		}
	}

	/// A dialog belongs to the session it asked about (§10, §18). A delete confirmation holding one
	/// server's paths must not survive into the next connection — confirming it there would delete
	/// those paths on a DIFFERENT machine.
	#[test]
	fn a_dialog_does_not_outlive_the_session_it_asked_about() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.begin_delete(vec!["/srv/a".to_owned()]);
		app.clear_grid_interaction();
		assert!(app.modal.is_none());
		// Nothing is left to confirm, so a stray confirm sends nothing at all.
		app.confirm_remote_delete();
		assert!(next_command(&mut rx).is_none());
	}

	/// Adding a forward from the dialog parses the two fields, queues the entry as `Starting`,
	/// sends the worker an `AddForward`, and clears the fields for the next one (§27).
	#[test]
	fn adding_a_forward_parses_queues_and_sends_it() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);

		app.add_forward();

		// Queued once, marked starting, and the input fields reset (the kind is kept).
		assert_eq!(app.forwards.len(), 1);
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Starting
		);
		assert!(forward_form(&app).listen.is_empty());
		assert!(forward_form(&app).to.is_empty());
		assert!(forward_form(&app).error.is_none());

		// The worker was asked to start exactly that spec.
		match next_command(&mut rx) {
			Some(SshCommand::AddForward { id, spec }) => {
				assert_eq!(id, app.forwards[0].id);
				assert_eq!(spec.listen_port, 8080);
				assert_eq!(spec.target_host, "db");
				assert_eq!(spec.target_port, 5432);
			}
			other => panic!("expected AddForward, got {other:?}"),
		}
	}

	/// A forward that does not parse sets the inline error and sends nothing (§27).
	#[test]
	fn a_bad_forward_shows_an_error_and_sends_nothing() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"not-a-port",
			"db:5432",
		);

		app.add_forward();

		assert!(app.forwards.is_empty());
		assert!(forward_form(&app).error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Two forwards cannot bind the same endpoint: the duplicate is refused before it is sent,
	/// so the second one's inevitable bind failure never happens (§27).
	#[test]
	fn a_duplicate_bind_is_refused() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(&mut app, crate::forward::ForwardKind::Local, "8080", "a:1");
		app.add_forward();
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::AddForward { .. })
		));

		// Same bind, different target: rejected, nothing added, nothing sent.
		type_forward(&mut app, crate::forward::ForwardKind::Local, "8080", "b:2");
		app.add_forward();
		assert_eq!(app.forwards.len(), 1);
		assert!(forward_form(&app).error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Removing a forward drops its row and asks the worker to tear it down (§27).
	#[test]
	fn removing_a_forward_drops_it_and_sends_remove() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(&mut app, crate::forward::ForwardKind::Dynamic, "1080", "");
		app.add_forward();
		let id = app.forwards[0].id;
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::AddForward { .. })
		));

		app.remove_forward(id);
		assert!(app.forwards.is_empty());
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::RemoveForward(removed)) if removed == id
		));

		// Removing an unknown id is a no-op — no row change, no command.
		app.remove_forward(999);
		assert!(next_command(&mut rx).is_none());
	}

	/// A worker's readiness / failure event marks the matching row; an event for a forward
	/// already removed is ignored (§27).
	#[test]
	fn a_forward_event_marks_its_row() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);
		app.add_forward();
		let id = app.forwards[0].id;

		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id,
			assigned_port: None,
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Active
		);

		let _ = app.on_ssh_event(SshEvent::ForwardFailed {
			id,
			reason: "port in use".to_owned(),
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Failed("port in use".to_owned())
		);

		// A stale event for a removed forward touches nothing.
		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id: 999,
			assigned_port: None,
		});
		assert_eq!(app.forwards.len(), 1);
	}

	/// A `-R 0` forward's readiness carries the port the server chose; the row records it (so it
	/// shows where the server listens) while the spec keeps its authored 0 (§27).
	#[test]
	fn a_server_assigned_remote_port_is_recorded_on_the_row() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Remote,
			"0",
			"localhost:3000",
		);
		app.add_forward();
		let id = app.forwards[0].id;
		// Authored as 0, no assigned port yet.
		assert_eq!(app.forwards[0].spec.listen_port, 0);
		assert_eq!(app.forwards[0].bound_port, None);

		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id,
			assigned_port: Some(38217),
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Active
		);
		assert_eq!(app.forwards[0].bound_port, Some(38217));
		// The row shows the real port; the persisted spec still asks for a fresh one on reconnect.
		assert_eq!(
			app.forwards[0].label(),
			"R  127.0.0.1:38217 → localhost:3000"
		);
		assert_eq!(app.forwards[0].spec.listen_port, 0);
	}

	/// Connection open/close events move a forward's live gauge (§27): opens raise the live and
	/// total counts, a close lowers the live count while the total stands, and a stale event for a
	/// removed forward is ignored.
	#[test]
	fn a_forward_connection_event_moves_the_gauge() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);
		app.add_forward();
		let id = app.forwards[0].id;
		// A fresh forward has carried nothing.
		assert_eq!(app.forwards[0].activity_gauge(), "0 open · 0 total");

		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id });
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id });
		assert_eq!(app.forwards[0].activity_gauge(), "2 open · 2 total");

		// A close drops the live count; the total, a record of traffic seen, stays.
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionClosed { id });
		assert_eq!(app.forwards[0].activity_gauge(), "1 open · 2 total");

		// A stale event for a forward that no longer exists changes nothing.
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id: 999 });
		assert_eq!(app.forwards[0].activity_gauge(), "1 open · 2 total");
	}

	/// Two saved targets, `root@web-01:22` and `root@db-01:22`, on a tab sitting at the home
	/// list — enough to have one row the filter keeps and one it hides (§49).
	fn tab_with_targets() -> Tab {
		let tab = Tab::default();
		{
			let mut targets = tab.targets.borrow_mut();
			targets.upsert_on_connect("web-01", 22, "root", AuthKind::Password, None, None);
			targets.upsert_on_connect("db-01", 22, "root", AuthKind::Password, None, None);
		}
		tab
	}

	/// A pending target as `dial` builds one (§14): auth and endpoint only. Everything else is a
	/// placeholder there too — the STORED target's remembered session, forwards and flag are what
	/// `adopt_target` reads back, and `upsert_on_connect` leaves those alone.
	fn pending_target(host: &str, user: &str) -> crate::targets::Target {
		crate::targets::Target {
			name: crate::targets::endpoint_of(user, host, 22),
			host: host.to_owned(),
			port: 22,
			user: user.to_owned(),
			auth_kind: AuthKind::Password,
			key_path: None,
			cert_path: None,
			show_hidden: true,
			terminal_path: None,
			files_path: None,
			explorer_width: None,
			files_height: None,
			sort: None,
			sort_dir: None,
			remember_secret: false,
			forwards: Vec::new(),
			elevate: None,
		}
	}

	/// A connection arriving reads everything the target remembers in ONE go (§14, §22, §27).
	///
	/// The read used to be three separate borrows of the shared target list, interleaved with the
	/// `&mut self` calls that act on what they found — so the order was load-bearing and two
	/// comments existed to say so. Asked as one question, it can be asserted as one answer.
	#[test]
	fn a_connect_reads_the_targets_layout_and_forwards_in_one_go() {
		let mut tab = tab_with_targets();
		let endpoint = "root@web-01:22";

		// The endpoint has been connected to before, so it carries a layout and a forward.
		{
			let mut targets = tab.targets.borrow_mut();
			targets.set_session(
				endpoint,
				crate::targets::SessionState {
					files_path: Some("/srv/data".to_owned()),
					show_hidden: Some(true),
					..crate::targets::SessionState::default()
				},
			);
			targets.set_forwards(
				endpoint,
				vec![crate::forward::ForwardSpec {
					kind: crate::forward::ForwardKind::Local,
					listen_host: "127.0.0.1".to_owned(),
					listen_port: 8080,
					target_host: "localhost".to_owned(),
					target_port: 80,
				}],
			);
		}

		let arrival = tab.adopt_target(pending_target("web-01", "root"));

		assert_eq!(arrival.key, endpoint, "and it re-uses the saved row");
		let session = arrival.session.expect("the remembered layout came back");
		assert_eq!(session.files_path.as_deref(), Some("/srv/data"));
		assert_eq!(session.show_hidden, Some(true));
		assert_eq!(arrival.forwards.len(), 1, "and so did the saved forward");
		assert_eq!(arrival.forwards[0].listen_port, 8080);
	}

	/// A target never connected to before remembers nothing, and says so rather than being absent
	/// (§14) — the first-connection path, which then falls back to the root and a login directory.
	#[test]
	fn a_first_connection_brings_back_nothing_to_restore() {
		let mut tab = Tab::default();
		let arrival = tab.adopt_target(pending_target("new-host", "cme"));

		assert_eq!(arrival.key, "cme@new-host:22");
		// It IS saved now — a real connect persists the target (§14) — it just has no history.
		assert!(arrival.forwards.is_empty());
		assert!(
			arrival
				.session
				.is_none_or(|session| session.files_path.is_none()),
			"nothing to resume to"
		);
	}

	/// Typing a pattern the selected row still matches leaves the selection alone — a list that
	/// narrows under the pointer must not also move what the keyboard is aimed at (§49).
	#[test]
	fn a_filter_the_selection_survives_keeps_it_selected() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());

		tab.on_home_filter("web".to_owned());

		assert_eq!(tab.home_selected.as_deref(), Some("root@web-01:22"));
	}

	/// A pattern that hides the selected row lets go of it, and of the menu anchored to it (§49).
	/// Every shortcut on this screen acts on the selection — F2 renames it, Enter opens it,
	/// Delete asks to remove it — so a selection behind the filter is one keystroke away from
	/// acting on a row that is not on screen.
	#[test]
	fn a_filter_that_hides_the_selection_drops_it() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());
		tab.home_menu_open = true;

		tab.on_home_filter("db".to_owned());

		assert_eq!(
			tab.home_selected, None,
			"the hidden row is no longer selected"
		);
		assert!(!tab.home_menu_open, "and its context menu went with it");
	}

	/// The pattern is matched against the endpoint as well as the name, so a target still called
	/// after its endpoint — which is how every target starts out — is findable by its host, its
	/// login or its port (§49).
	#[test]
	fn a_filter_matches_the_endpoint_as_well_as_the_name() {
		let tab = tab_with_targets();
		tab.targets.borrow_mut().rename("root@db-01:22", "ledger");
		let targets = tab.targets.borrow();

		let ledger = targets.find("root@db-01:22").expect("the renamed target");
		assert!(ledger.matches("ledger"), "by the name the user gave it");
		assert!(ledger.matches("db-01"), "and by where it actually is");
		assert!(ledger.matches("root@*"), "globs read the endpoint too");
		assert!(!ledger.matches("web"), "the other row is not this one");
	}

	/// Esc empties the filter box and puts the whole list back (§49) — the way out of a pattern
	/// that matches nothing without going back to the box to erase it.
	#[test]
	fn escape_empties_the_home_filter() {
		let mut tab = tab_with_targets();
		tab.home_filter = "prod".to_owned();

		let _ = tab.on_home_key(key_press(
			iced::keyboard::key::Named::Escape,
			iced::keyboard::key::Code::Escape,
			iced::keyboard::Modifiers::empty(),
		));

		assert!(tab.home_filter.is_empty());
	}

	// A press of a printable character key, the way a real typed letter arrives: the logical key
	// AND the text the OS produced for it, since the encoder reads both.
	fn character_press(
		character: &str,
		code: iced::keyboard::key::Code,
		modifiers: iced::keyboard::Modifiers,
	) -> iced::keyboard::Event {
		let key = iced::keyboard::Key::Character(character.into());
		iced::keyboard::Event::KeyPressed {
			key: key.clone(),
			modified_key: key,
			physical_key: iced::keyboard::key::Physical::Code(code),
			location: iced::keyboard::Location::Standard,
			modifiers,
			text: Some(character.into()),
			repeat: false,
		}
	}

	/// Typing while a side pane holds the keyboard hands it back to the shell, and the letter
	/// that did it goes down the channel rather than being spent on the switch (§50). Without
	/// this the pane swallowed it and the first character of a command vanished.
	#[test]
	fn typing_while_a_pane_has_the_keyboard_gives_it_to_the_shell() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.focus = Focus::Files;

		let _ = app.on_key(character_press(
			"l",
			iced::keyboard::key::Code::KeyL,
			iced::keyboard::Modifiers::empty(),
		));

		assert_eq!(app.focus, Focus::Terminal, "typing means the shell");
		assert_eq!(
			next_input(&mut rx),
			Some(b"l".to_vec()),
			"and the letter goes with it"
		);
	}

	/// A navigation key is the pane's own, so it keeps both the key and the keyboard (§20, §50).
	/// This is the half of the rule that makes the other half safe: walking a tree with the arrows
	/// must not read as typing at the prompt.
	#[test]
	fn an_arrow_while_a_pane_has_the_keyboard_stays_with_the_pane() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.focus = Focus::Tree;

		let _ = app.on_key(key_press(
			iced::keyboard::key::Named::ArrowDown,
			iced::keyboard::key::Code::ArrowDown,
			iced::keyboard::Modifiers::empty(),
		));

		assert_eq!(app.focus, Focus::Tree, "the tree is still being walked");
		assert_eq!(next_input(&mut rx), None, "and nothing reached the shell");
	}

	/// A character under Ctrl is a combination, not typing (§50): the files pane's Ctrl+A still
	/// takes the whole listing (§21) instead of handing the keyboard over and sending an `a`.
	#[test]
	fn a_control_combination_is_not_typing() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.focus = Focus::Files;

		let _ = app.on_key(character_press(
			"a",
			iced::keyboard::key::Code::KeyA,
			iced::keyboard::Modifiers::CTRL,
		));

		assert_eq!(app.focus, Focus::Files, "the pane keeps its own shortcut");
		assert_eq!(next_input(&mut rx), None, "and nothing reached the shell");
	}

	/// Ctrl+V is the menu's Paste off the keyboard, so it is answered from wherever the ring is
	/// and takes it back with it (§50). It used to be dropped: the pane swallowed it and the
	/// paste never happened, with nothing on screen to say why.
	#[test]
	fn ctrl_v_pastes_from_a_pane_and_takes_the_keyboard_back() {
		let (mut app, _rx) = app_with_terminal(16);
		app.focus = Focus::Files;

		let _ = app.on_key(character_press(
			"v",
			iced::keyboard::key::Code::KeyV,
			iced::keyboard::Modifiers::CTRL,
		));

		assert_eq!(app.focus, Focus::Terminal, "the paste goes to the shell");
	}

	/// Ctrl+C is not treated that way (§50): it reads the terminal's own selection, or is the
	/// interrupt for the remote — neither is text going in — so a pane holding the ring keeps it.
	#[test]
	fn ctrl_c_does_not_take_the_keyboard_from_a_pane() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.focus = Focus::Files;

		let _ = app.on_key(character_press(
			"c",
			iced::keyboard::key::Code::KeyC,
			iced::keyboard::Modifiers::CTRL,
		));

		assert_eq!(app.focus, Focus::Files, "the pane keeps the keyboard");
		assert_eq!(
			next_input(&mut rx),
			None,
			"and the shell hears no interrupt"
		);
	}

	// A local session on a shell that may ignore EOF, ready for a Ctrl+D (§104).
	fn local_shell_tab(kind: crate::local::shells::ShellKind) -> (Tab, mpsc::Receiver<SshCommand>) {
		let (mut app, rx) = app_with_terminal(16);
		app.connection = Some(format!("local — {}", kind.slug()));
		app.local = Some(kind);
		(app, rx)
	}

	// Ctrl+D, the way the OS delivers it.
	fn ctrl_d() -> iced::keyboard::Event {
		character_press(
			"d",
			iced::keyboard::key::Code::KeyD,
			iced::keyboard::Modifiers::CTRL,
		)
	}

	/// The whole of §104 against a REAL shell: a live `pwsh` on a real pty, its bytes arriving in whatever
	/// reads the ConPTY chooses, one Ctrl+D pressed at its prompt, and the shell expected to leave.
	///
	/// This test exists because the unit tests above could not catch what shipped. Every one of them feeds
	/// bytes cmote's author typed, so they encode an assumption about where a read boundary falls — and the
	/// assumption was wrong: the echo comes in two reads, not one, and the rule decided on the first. Only a
	/// real shell has an opinion about that. It skips on a machine with no such shell, which is the price of
	/// the coverage being real.
	///
	/// It drives the same three parts the app wires together: `local::session` on one side, the tab's own
	/// `on_ssh_event` / `on_key` on the other, and the translation `ssh::client` normally does in between.
	#[cfg(windows)]
	#[tokio::test]
	async fn a_real_local_shell_answers_ctrl_d_by_leaving() {
		// The shell prints for a while as its profile runs; the press goes in once it has been quiet
		// for this long AND has printed something, which together are this test's definition of "at a
		// prompt".
		//
		// The "has printed something" half is not decoration. Quiet before the first byte is a shell
		// that has not started yet, and it looks exactly like quiet at a prompt — so on a loaded
		// machine this test used to press Ctrl+D at a shell with no prompt to echo it at, and then
		// fail on `typed` thirty seconds later. It reproduced 17 times out of 17 with eight busy
		// cores beside it, and the whole failure was one missing precondition.
		const SETTLED: std::time::Duration = std::time::Duration::from_millis(2500);

		let Some(shell) = crate::local::shells::catalogue()
			.iter()
			.find(|shell| !shell.kind.quits_on_eof())
			.cloned()
		else {
			eprintln!("skipped: this machine offers no shell that ignores EOF");
			return;
		};
		let (mut tab, mut commands) = app_with_terminal(256);
		tab.local = Some(shell.kind);
		tab.connection = Some(shell.endpoint());

		let (event_tx, mut events) = mpsc::channel::<SshEvent>(256);
		let (to_session, from_tab) = mpsc::channel::<crate::ssh::client::SessionMsg>(64);
		let session = tokio::spawn(crate::local::session::run(shell, event_tx, from_tab));

		let mut pressed = false;
		let mut typed = false;
		let mut ended = false;
		// Whether the shell has printed anything at all yet. See `SETTLED`.
		let mut heard = false;
		let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
			loop {
				tokio::select! {
					biased;
					event = events.recv() => {
						let Some(event) = event else { break };
						let done = matches!(event, SshEvent::Disconnected);
						heard |= matches!(event, SshEvent::Output { .. });
						let _ = tab.on_ssh_event(event);
						if done { ended = true; break }
					}
					command = commands.recv() => {
						use crate::ssh::client::SessionMsg;
						let Some(command) = command else { break };
						// What `ssh::client::run` does for a real session, in three arms.
						let forwarded = match command {
							SshCommand::Input(bytes) => {
								typed |= bytes == crate::local::shells::quit_sequence();
								Some(SessionMsg::Data(bytes))
							}
							SshCommand::Reply { identity, bytes } => {
								Some(SessionMsg::Reply { identity, bytes })
							}
							SshCommand::Disconnect => Some(SessionMsg::Disconnect),
							_ => None,
						};
						if let Some(message) = forwarded {
							let _ = to_session.send(message).await;
						}
					}
					() = tokio::time::sleep(SETTLED) => {
						// Silence with nothing printed yet is a slow start, not a prompt.
						if !heard || typed { continue }
						// Pressed on EVERY settle, not once. Quiet is the only prompt signal available
						// here, and it is not a reliable one: on a loaded machine a gap in the
						// profile's own output looks exactly like a prompt, so a lone press can land
						// before the shell reads input and simply be lost — which is what made this
						// test fail 17 times out of 17 beside eight busy cores. Re-pressing costs
						// nothing, because an unanswered Ctrl+D is precisely the case §104 is built to
						// survive, and the claim under test is that a press AT a prompt is echoed and
						// answered — not that the first guess at where the prompt is happens to be
						// right.
						pressed = true;
						let _ = tab.on_key(ctrl_d());
					}
				}
			}
		})
		.await;
		// Cleanup before the assertions, and bounded: if the shell did NOT leave, the session task is
		// still holding a live pty, and awaiting it would hang this test rather than fail it. Which it did,
		// the first time it was run against a deliberately broken `judge_eof` — a test that hangs instead of
		// failing is not a test.
		let _ = to_session
			.send(crate::ssh::client::SessionMsg::Disconnect)
			.await;
		if tokio::time::timeout(std::time::Duration::from_secs(5), session)
			.await
			.is_err()
		{
			eprintln!("the local session did not come down; the pty goes with the test process");
		}

		assert!(pressed, "the shell settled at a prompt and the key went in");
		assert!(
			typed,
			"the shell handed the byte back, so cmote typed its own exit at it"
		);
		assert!(
			ended,
			"and the shell left, which is what ends the session — no kill was involved"
		);
		assert!(
			matches!(tab.screen, AppScreen::Home) && tab.connection.is_none(),
			"the tab landed on the home screen, where a second Ctrl+D closes it (§30)"
		);
	}

	/// Ctrl+D at a local shell goes TO the shell, exactly as it would in any terminal (§104). cmote takes
	/// nothing and decides nothing yet — it only starts listening for the answer.
	///
	/// This is the half the first version got wrong: it claimed the key and ended the session, so a
	/// `node` running at that prompt never got the EOF that would have quit it.
	#[test]
	fn ctrl_d_at_a_local_shell_reaches_the_shell_first() {
		let (mut app, mut rx) = local_shell_tab(crate::local::shells::ShellKind::Pwsh);

		let _ = app.on_key(ctrl_d());

		assert_eq!(
			next_input(&mut rx).as_deref(),
			Some(&[0x04][..]),
			"the byte goes down the channel like any other keystroke"
		);
		assert!(
			app.eof_probe.is_some(),
			"and cmote is listening for an answer"
		);
		assert!(
			app.connection.is_some() && app.terminal.is_some(),
			"nothing has been torn down on a guess"
		);
	}

	/// The shell echoing the byte back is what says nothing consumed it (§104) — a Windows interpreter handed
	/// a control byte it has no meaning for prints `^D` onto its input line. cmote answers by running the
	/// shell's OWN `exit`: an interrupt to clear the line the echo landed on, then the word. Nothing is torn
	/// down from this side, and the session ends when the shell does.
	///
	/// Fed as the TWO CHUNKS a real `pwsh` sends, which is the boundary this shipped a bug on: the first read
	/// is six bytes of cursor-hiding with no echo in it at all, and a version that decided on the first chunk
	/// concluded "a program answered" and left the echo to be drawn with nothing else happening. The bytes
	/// below are copied from a probe that printed each read separately — earlier ones concatenated them and
	/// so could not have shown this.
	#[test]
	fn the_shell_echoing_the_byte_back_is_told_to_exit() {
		let (mut app, mut rx) = local_shell_tab(crate::local::shells::ShellKind::Pwsh);
		let _ = app.on_key(ctrl_d());
		assert_eq!(next_input(&mut rx).as_deref(), Some(&[0x04][..]));

		let _ = app.on_ssh_event(shell_output(b"\x1b[?25l"));
		assert!(
			app.eof_probe.is_some(),
			"the first read carries no echo and must not settle it"
		);
		assert!(
			next_command(&mut rx).is_none(),
			"and nothing is sent on the strength of it"
		);
		let _ = app.on_ssh_event(shell_output(
			b"\x1b[93m^D\x1b[97m\x1b[2m\x1b[3mexit\x1b[2;20H\x1b[?25h",
		));

		assert_eq!(
			next_input(&mut rx).as_deref(),
			Some(&b"\x03exit\r"[..]),
			"the line the echo landed on is cancelled, then the shell's own exit is typed"
		);
		assert!(
			next_command(&mut rx).is_none(),
			"and nothing else: no Disconnect, so no kill — the shell leaves because it was asked to"
		);
		assert!(
			app.connection.is_some() && app.terminal.is_some(),
			"the session is still up until the shell actually goes"
		);
		assert!(
			app.eof_probe.is_none(),
			"the question is settled either way"
		);

		// And when it goes, the ordinary hangup path lands the tab where §30 wants it.
		let _ = app.on_ssh_event(SshEvent::Disconnected);
		assert!(
			app.connection.is_none() && app.local.is_none() && app.terminal.is_none(),
			"the tab has forgotten it had a session"
		);
		assert!(
			matches!(app.screen, AppScreen::Home),
			"landing on the home screen, where a second Ctrl+D closes the tab (§30)"
		);
	}

	/// The case the whole rule exists for: a program at that prompt takes the EOF and quits, the shell
	/// prints a fresh prompt, and the SESSION IS LEFT ALONE (§104). Measured against real node — `0x04`
	/// makes it exit and `pwsh` answers with `\r\nPS C:\Users\cme> `, which carries no echo.
	///
	/// The second case is the budget: a program that answers with more than a round trip's worth of output and
	/// no echo in it has consumed the byte, and the probe gives up rather than listening indefinitely. Both
	/// failure directions are "Ctrl+D did nothing", never "the session ended by mistake".
	#[test]
	fn a_program_that_answers_the_byte_keeps_the_session() {
		let chatter = vec![b'.'; EOF_ANSWER_CAP + 1];
		// The second entry says whether the answer is long enough to exhaust the budget on its own.
		let answers: [(&[u8], bool); 2] = [(b"\r\nPS C:\\Users\\cme> ", false), (&chatter, true)];
		for (answer, spent) in answers {
			let (mut app, mut rx) = local_shell_tab(crate::local::shells::ShellKind::Pwsh);
			let _ = app.on_key(ctrl_d());
			assert_eq!(next_input(&mut rx).as_deref(), Some(&[0x04][..]));

			let _ = app.on_ssh_event(shell_output(answer));

			assert!(
				app.connection.is_some() && app.terminal.is_some(),
				"the byte did its job, so the session is none of cmote's business ({spent})"
			);
			assert!(
				next_command(&mut rx).is_none(),
				"and nothing was sent on the strength of it ({spent})"
			);
			assert_eq!(
				app.eof_probe.is_none(),
				spent,
				"the probe lives exactly as long as its budget, no further ({spent})"
			);
		}
	}

	/// A held Ctrl+D is ONE press (§104). The gesture is two deliberate ones — the first ends the shell
	/// and lands on the home screen, the second closes the tab — and an auto-repeat from the first used to
	/// do both, so the tab vanished instantly and took the screen the user was meant to land on with it.
	///
	/// Asserted on the predicate `on_home_key` consults, the way `is_typing` and `is_paste` are tested:
	/// what the key MEANS is the decision worth pinning, and an `iced::Task` cannot be looked inside from
	/// here.
	#[test]
	fn a_held_ctrl_d_does_not_close_the_tab_behind_the_session_it_just_ended() {
		let d = iced::keyboard::Key::Character("d".into());
		let ctrl = iced::keyboard::Modifiers::CTRL;

		assert!(
			is_close_tab(&d, ctrl, false),
			"a press of its own is the second half of the gesture"
		);
		assert!(
			!is_close_tab(&d, ctrl, true),
			"an auto-repeat of it never is"
		);
		// And the guards that were always there: it is Ctrl+D and nothing else.
		assert!(!is_close_tab(&d, iced::keyboard::Modifiers::empty(), false));
		assert!(!is_close_tab(
			&d,
			ctrl | iced::keyboard::Modifiers::ALT,
			false
		));
		assert!(!is_close_tab(
			&iced::keyboard::Key::Character("w".into()),
			ctrl,
			false
		));
	}

	/// What a teardown types, and at which shells (§104). A local session's kill is `TerminateProcess`,
	/// so the shell is asked to leave first and gets to run its own exit path; the two cases that are NOT
	/// asked are the point of the test.
	///
	/// A remote is not asked because closing the SSH channel already hangs its shell up properly, and a
	/// session showing a full-screen program is not asked because `exit` is four keystrokes rather than a
	/// message — at a `vim` in normal mode they would delete a character and start inserting.
	#[test]
	fn a_local_teardown_asks_the_shell_to_leave_and_a_remote_is_left_alone() {
		let cases = [
			(Some(crate::local::shells::ShellKind::Pwsh), false, true),
			(Some(crate::local::shells::ShellKind::Pwsh), true, false),
			(None, false, false),
		];
		for (local, alternate, asked) in cases {
			let (mut app, mut rx) = app_with_terminal(16);
			app.connection = Some("somewhere".to_owned());
			app.local = local;
			if alternate {
				app.terminal
					.as_mut()
					.expect("the fixture's emulator")
					.process(b"\x1b[?1049h");
			}

			app.end_session();

			let case = format!("local: {local:?}, full-screen program: {alternate}");
			if asked {
				assert_eq!(
					next_input(&mut rx).as_deref(),
					Some(&b"\x03exit\r"[..]),
					"the input line is cancelled and the shell asked to leave ({case})"
				);
			}
			assert!(
				matches!(next_command(&mut rx), Some(SshCommand::Disconnect)),
				"the teardown itself always follows ({case})"
			);
			assert!(
				next_command(&mut rx).is_none(),
				"and nothing else is sent ({case})"
			);
		}
	}

	/// Every shell that answers EOF itself is not even listened to (§30, §104): a local Git Bash, whose exit
	/// is what ends the session, and any remote — where Ctrl+D is the way you log out and cmote has never
	/// had a claim on it. Both get the plain `0x04`, no probe is armed, and neither session is touched from
	/// this side. The unarmed half matters: it is what keeps a `^D` printed by some program on a REMOTE from
	/// ever being read as an answer to a key.
	#[test]
	fn ctrl_d_is_left_to_every_shell_that_answers_it() {
		for local in [Some(crate::local::shells::ShellKind::GitBash), None] {
			let (mut app, mut rx) = app_with_terminal(16);
			app.connection = Some("root@web-01:22".to_owned());
			app.local = local;

			let _ = app.on_key(ctrl_d());

			assert_eq!(
				next_input(&mut rx).as_deref(),
				Some(&[0x04][..]),
				"EOF goes down the channel as it always has ({local:?})"
			);
			assert!(
				app.eof_probe.is_none(),
				"and nothing is listening for an echo ({local:?})"
			);
			// Even an answer that looks exactly like an echo decides nothing here.
			let _ = app.on_ssh_event(shell_output(b"\x1b[93m^D"));
			assert!(
				app.connection.is_some() && app.terminal.is_some(),
				"the session is still up: the shell decides, not cmote ({local:?})"
			);
		}
	}

	/// The two presses §104 does not listen to, on the very shell it otherwise listens to.
	///
	/// A full-screen program owns Ctrl+D — half a page down in `less` and in every pager built on it — and
	/// it asked for the whole screen to get it. The echo test alone would nearly cover that, since a pager
	/// scrolling answers with a screenful and not with `^D`; not listening at all is what covers the pager
	/// showing a FILE that happens to contain the characters `^D`. And Ctrl+SHIFT+D encodes to the same
	/// `0x04` while staying unwatched, which makes it the way to hand a bare EOF to a shell that would echo
	/// it — the escape hatch out of this whole rule.
	#[test]
	fn a_full_screen_program_and_a_shifted_press_keep_their_ctrl_d() {
		let cases = [
			(true, iced::keyboard::Modifiers::CTRL, "a pager is up"),
			(
				false,
				iced::keyboard::Modifiers::CTRL | iced::keyboard::Modifiers::SHIFT,
				"the press is shifted",
			),
		];
		for (alternate, modifiers, why) in cases {
			let (mut app, mut rx) = app_with_terminal(16);
			app.connection = Some("local — pwsh".to_owned());
			app.local = Some(crate::local::shells::ShellKind::Pwsh);
			if alternate {
				app.terminal
					.as_mut()
					.expect("the fixture's emulator")
					.process(b"\x1b[?1049h");
			}

			let _ = app.on_key(character_press(
				"d",
				iced::keyboard::key::Code::KeyD,
				modifiers,
			));

			assert_eq!(
				next_input(&mut rx).as_deref(),
				Some(&[0x04][..]),
				"the byte reaches the shell ({why})"
			);
			assert!(
				app.eof_probe.is_none(),
				"and nothing is listening for an echo ({why})"
			);
			// So even the shell's own echo, arriving here, is just output.
			let _ = app.on_ssh_event(shell_output(b"\x1b[93m^D"));
			assert!(
				app.connection.is_some(),
				"and the session is untouched ({why})"
			);
		}
	}

	/// A command from the terminal's own surface — here Paste, off the grid's right-click menu —
	/// puts the keyboard back on the shell (§50). Pasting a command while a pane held the focus
	/// used to leave the Enter that runs it going to the pane.
	#[test]
	fn a_terminal_menu_command_takes_the_keyboard_back() {
		let (mut app, _rx) = app_with_terminal(16);
		app.focus = Focus::Files;
		app.menu = Some(iced::Point::new(10.0, 10.0));

		let _ = app.update(Message::PastePressed);

		assert_eq!(
			app.focus,
			Focus::Terminal,
			"the paste lands where the keyboard now is"
		);
		assert!(app.menu.is_none(), "and the menu it was chosen from closed");
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
	fn with_history(app: &mut Tab) {
		let output: Vec<u8> = (0..40).flat_map(|_| b"x\r\n".to_vec()).collect();
		app.terminal.as_mut().unwrap().process(&output);
	}

	// The current scrollback offset off the live emulator.
	fn offset(app: &Tab) -> u16 {
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

	/// A scrollbar drag parks the view at the offset it names and does NOTHING else (§116). The
	/// geometry is the grid's and the clamping is the engine's, both tested where they live; what is
	/// asserted here is the handler's whole contract — that dragging the bar is purely a view change.
	/// Each of these would be a real bug: a drag that stole the keyboard, that cleared or moved a
	/// selection made before it, or that sent a byte to the remote because it went through the input
	/// path. The last matters most: scrolling is local (§23), and a bar that typed at the shell would
	/// be a remote-visible side effect of looking at history.
	#[test]
	fn dragging_the_scrollbar_parks_the_view_and_touches_nothing_else() {
		let (mut app, mut rx) = app_with_terminal(16);
		with_history(&mut app);
		// A selection and a focus that must both survive the drag.
		let anchor = ui::selection::ScreenSpot { row: 0, col: 0 };
		let selection =
			ui::selection::Selection::new(anchor.to_doc(app.terminal.as_ref().unwrap().screen()));
		app.selection = Some(selection);
		app.focus = Focus::Files;

		app.on_terminal_scroll_to(4);
		assert_eq!(offset(&app), 4, "parked where the drag asked");
		// A drag republishes on every pointer move, so the same offset arriving twice must be inert
		// rather than moving twice as far.
		app.on_terminal_scroll_to(4);
		assert_eq!(offset(&app), 4);
		// Down again, through zero, and off the far end — the engine clamps, nothing wraps.
		app.on_terminal_scroll_to(0);
		assert_eq!(offset(&app), 0, "back at the live bottom");
		app.on_terminal_scroll_to(u16::MAX);
		let history = app.terminal.as_ref().unwrap().screen().history_size();
		assert_eq!(offset(&app), history, "pinned at the oldest retained line");

		assert_eq!(app.selection, Some(selection), "the selection is untouched");
		assert_eq!(app.focus, Focus::Files, "and the keyboard stayed put");
		assert!(
			next_input(&mut rx).is_none(),
			"scrolling is local — nothing reaches the remote"
		);
	}

	/// Ctrl+Shift+Up / Ctrl+Shift+Down move cmote's view between shell prompts, from the OSC 133
	/// marks (§34), and send nothing to the remote. Two prompts with output between them: the first
	/// scrolls off, so jumping up climbs into history to it and jumping down returns toward the live
	/// prompt.
	#[test]
	fn ctrl_shift_arrows_jump_between_prompts() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = app_with_terminal(16);
		let terminal = app.terminal.as_mut().unwrap();
		// A first prompt, then enough output to push it up into history, then a second prompt.
		terminal.process(b"\x1b]133;A\x07first$ \r\n");
		let filler: Vec<u8> = (0..30).flat_map(|_| b"output\r\n".to_vec()).collect();
		terminal.process(&filler);
		terminal.process(b"\x1b]133;A\x07second$ ");
		assert_eq!(offset(&app), 0, "starts at the live bottom");

		let jump = Modifiers::CTRL | Modifiers::SHIFT;
		let _ = app.on_key(key_press(Named::ArrowUp, Code::ArrowUp, jump));
		let climbed = offset(&app);
		assert!(climbed > 0, "jumped up into history to the earlier prompt");
		// The jump is cmote's own view motion — nothing was sent to the shell.
		assert_eq!(next_input(&mut rx), None);

		let _ = app.on_key(key_press(Named::ArrowDown, Code::ArrowDown, jump));
		assert!(
			offset(&app) < climbed,
			"jumped back down toward the live prompt"
		);
	}

	/// Ctrl+Shift+O selects the last finished command's output as a text selection (§34), so the
	/// existing Copy grabs it — and sends nothing to the remote. One command bracketed by OSC 133
	/// marks with two lines of output; after the keybind the selection extracts exactly that output.
	#[test]
	fn ctrl_shift_o_selects_the_last_commands_output() {
		use iced::keyboard::key::{Code, Physical};
		use iced::keyboard::{Key, Location, Modifiers};

		let (mut app, mut rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07alpha\r\nbeta\r\n\x1b]133;D;0\x07",
		);

		// Ctrl+Shift+O, matched on the physical key so the logical value does not matter.
		let press = iced::keyboard::Event::KeyPressed {
			key: Key::Character("o".into()),
			modified_key: Key::Character("o".into()),
			physical_key: Physical::Code(Code::KeyO),
			location: Location::Standard,
			modifiers: Modifiers::CTRL | Modifiers::SHIFT,
			text: None,
			repeat: false,
		};
		let _ = app.on_key(press);

		let selection = app.selection.expect("the command's output is selected");
		assert!(!selection.is_empty());
		let text = selection.extract(app.terminal.as_ref().unwrap().screen());
		assert_eq!(text, "alpha\nbeta");
		// The keybind is cmote's own view action — nothing reached the shell.
		assert_eq!(next_input(&mut rx), None);
	}

	/// Ctrl+Shift+O grabs a command's WHOLE output, even one taller than the screen (§40). This is the
	/// viewport-bound capture §34 shipped with and wrote down as deferred: the span is now document
	/// lines and the copy reads the history directly, so the screenful that happens to show no longer
	/// bounds what is selected.
	#[test]
	fn ctrl_shift_o_selects_output_taller_than_the_screen() {
		let (mut app, _rx) = app_with_terminal(16);
		{
			let terminal = app.terminal.as_mut().unwrap();
			terminal.process(b"\x1b]133;A\x07$ \x1b]133;B\x07seq\r\n\x1b]133;C\x07");
			// Forty lines of output on a 24-row screen: most of it is up in the history by the time
			// the command finishes.
			let output: Vec<u8> = (0..40)
				.flat_map(|n| format!("out {n}\r\n").into_bytes())
				.collect();
			terminal.process(&output);
			terminal.process(b"\x1b]133;D;0\x07");
		}

		app.select_command_output();

		let selection = app.selection.expect("the command's output is selected");
		let text = selection.extract(app.terminal.as_ref().unwrap().screen());
		let lines: Vec<&str> = text.lines().collect();
		assert_eq!(
			lines.len(),
			40,
			"every printed line is copied, not just the screenful on show"
		);
		assert_eq!(lines.first(), Some(&"out 0"));
		assert_eq!(lines.last(), Some(&"out 39"));
	}

	/// A mouse drag anchors and extends in DOCUMENT coordinates (§40), so a selection made while
	/// scrolled back into history keeps covering the lines it was dragged over — and copies them —
	/// once the view has moved on. Under viewport coordinates the same selection would stay on its
	/// rows and copy whatever later slid into them.
	#[test]
	fn a_drag_selects_the_lines_it_covered_not_the_rows_it_covered() {
		let (mut app, _rx) = app_with_terminal(16);
		{
			let terminal = app.terminal.as_mut().unwrap();
			let output: Vec<u8> = (0..60)
				.flat_map(|n| format!("line {n}\r\n").into_bytes())
				.collect();
			terminal.process(&output);
		}

		// The pointer, in grid-local pixels, over a given cell.
		let point = |row: u16, col: u16| {
			iced::Point::new(
				ui::terminal::GRID_PADDING + f32::from(col) * ui::terminal::CELL_WIDTH + 0.5,
				ui::terminal::GRID_PADDING + f32::from(row) * ui::terminal::CELL_HEIGHT + 0.5,
			)
		};
		// Scroll back into the history, then drag across the seven cells of the top visible row.
		app.on_terminal_scroll(20);
		app.on_grid_moved(point(0, 0));
		app.on_grid_pressed();
		app.on_grid_moved(point(0, 6));
		app.on_grid_released();

		let selection = app.selection.expect("the drag selected a run of cells");
		let text = selection.extract(app.terminal.as_ref().unwrap().screen());
		assert!(
			text.starts_with("line "),
			"a numbered line was dragged over"
		);

		// Back at the live bottom, other lines are on that row — and the selection still extracts
		// exactly the text it was dragged over.
		app.on_terminal_scroll(-20);
		assert_eq!(offset(&app), 0, "returned to the live bottom");
		assert_eq!(
			selection.extract(app.terminal.as_ref().unwrap().screen()),
			text
		);
	}

	/// Clicking a prompt tick in the left gutter selects that command's output (§34) — the other
	/// trigger. A press with the pointer inside `GRID_PADDING` on the prompt's row resolves to its
	/// command and selects the output, as a discrete action (no drag begins).
	#[test]
	fn clicking_a_prompt_tick_selects_that_commands_output() {
		let (mut app, _rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07alpha\r\nbeta\r\n\x1b]133;D;0\x07",
		);

		// The prompt sits on viewport row 0; a gutter press there (x < GRID_PADDING) selects it.
		app.pointer = iced::Point::new(1.0, 1.0);
		app.hover_cell = ui::selection::ScreenSpot { row: 0, col: 0 };
		app.on_grid_pressed();

		let selection = app.selection.expect("the tick click selected the output");
		let text = selection.extract(app.terminal.as_ref().unwrap().screen());
		assert_eq!(text, "alpha\nbeta");
		assert!(
			!app.selecting,
			"a tick click is a discrete action, not a drag"
		);
	}

	/// A double click on the grid selects the word under the pointer and a triple the whole line (§42),
	/// counted from the presses themselves — and neither starts a drag, since the next pointer move
	/// would otherwise collapse the span back to the cell that was pressed.
	#[test]
	fn a_double_click_selects_a_word_and_a_triple_the_line() {
		let (mut app, _rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(b"cat /etc/hosts");

		// Clear of the left gutter, so this is an ordinary grid press and not a prompt tick (§34).
		app.pointer = iced::Point::new(50.0, 5.0);
		app.hover_cell = ui::selection::ScreenSpot { row: 0, col: 6 };

		// One press selects nothing on its own …
		app.on_grid_pressed();
		let selection = app.selection.expect("a press anchors a selection");
		assert!(selection.is_empty(), "a bare click selects nothing");
		assert!(app.selecting, "and it does begin a drag");

		// … a second on the same cell takes the word …
		app.on_grid_pressed();
		let screen = app.terminal.as_ref().unwrap().screen();
		let selection = app.selection.expect("the double click selected a word");
		assert_eq!(selection.extract(screen), "/etc/hosts");
		assert!(!app.selecting, "a word selection is not a drag");

		// … and a third the whole line.
		app.on_grid_pressed();
		let screen = app.terminal.as_ref().unwrap().screen();
		let selection = app.selection.expect("the triple click selected a line");
		assert_eq!(selection.extract(screen), "cat /etc/hosts");
	}

	/// A resize reflows the scrollback, so an absolute document line recorded before it names other
	/// text after it (§43). The selection is dropped rather than left pointing somewhere it no longer
	/// belongs — a highlight that lies is worse than none, since Copy would then hand the clipboard
	/// text the user never selected. The find bar keeps its query and is re-scanned instead, so its
	/// washes follow the text they belong to.
	#[test]
	fn a_resize_drops_the_selection_and_rescans_the_find_bar() {
		let (mut app, _rx) = app_with_terminal(16);
		// The files pane starts open (§19) and takes its height out of the grid, so the window sizes
		// here have to allow for it or the reflow lands on a one-row grid.
		let reserved = app.panes.pane.reserved();
		app.window_size = ui::terminal::window_size(80, 24, reserved);
		app.terminal.as_mut().unwrap().process(b"hello world");

		// A find bar with a hit, and a selection of the user's own over the same line.
		let _focus = app.open_term_find();
		app.term_find_query("hello".to_owned());
		assert!(
			app.search.as_ref().unwrap().current().is_some(),
			"the query matched before the resize"
		);
		app.selection = Some(ui::selection::Selection::spanning(
			ui::selection::DocSpot { line: 0, col: 0 },
			ui::selection::DocSpot { line: 0, col: 4 },
		));
		app.selecting = true;

		// The window narrows to 60 columns, which is what reflows the grid.
		app.on_window_resized(ui::terminal::window_size(60, 24, reserved));

		assert_eq!(
			app.terminal.as_ref().unwrap().screen().size(),
			(24, 60),
			"the grid did reflow, or this test proves nothing"
		);
		assert!(app.selection.is_none(), "the stale selection is dropped");
		assert!(!app.selecting, "and any drag with it");
		let search = app.search.as_ref().expect("the find bar stays open");
		assert_eq!(search.query, "hello", "with its query");
		assert!(
			search.current().is_some(),
			"re-scanned, so the hit is where the reflow left it"
		);
	}

	/// The multi-click tally starts over on a resize (§43): the cell it was counting presses on shows
	/// different text once the grid has reflowed, so the next press there is a plain click and not the
	/// second half of a double click the user never made (§42).
	#[test]
	fn a_resize_starts_the_multi_click_tally_over() {
		let (mut app, _rx) = app_with_terminal(16);
		let reserved = app.panes.pane.reserved();
		app.window_size = ui::terminal::window_size(80, 24, reserved);
		app.terminal.as_mut().unwrap().process(b"cat /etc/hosts");

		// Clear of the left gutter, so this is an ordinary grid press and not a prompt tick (§34).
		app.pointer = iced::Point::new(50.0, 5.0);
		app.hover_cell = ui::selection::ScreenSpot { row: 0, col: 6 };
		app.on_grid_pressed();

		app.on_window_resized(ui::terminal::window_size(60, 24, reserved));

		// The pointer never moved, so the tally's cell is the one this press lands on — the second
		// press would take the word if the resize had not reset the count.
		assert_eq!(
			app.hover_cell,
			ui::selection::ScreenSpot { row: 0, col: 6 },
			"the hovered cell is resolved again against the new grid"
		);
		app.on_grid_pressed();
		let selection = app.selection.expect("a press anchors a selection");
		assert!(selection.is_empty(), "a plain click, not a word");
		assert!(app.selecting, "and it begins a drag");
	}

	/// Ctrl+Shift+F opens the scrollback find bar, and while it is open the bar owns the keyboard
	/// (§35): a keystroke searches instead of reaching the remote. Esc closes it and the shell has
	/// the keyboard back — otherwise a search would leave the session mute.
	#[test]
	fn ctrl_shift_f_opens_the_find_bar_and_takes_the_keyboard() {
		use iced::keyboard::key::{Code, Named, Physical};
		use iced::keyboard::{Key, Location, Modifiers};

		// A press carrying produced text, the way a real typed character arrives.
		fn typed(
			key: Key,
			code: Code,
			modifiers: Modifiers,
			text: Option<&str>,
		) -> iced::keyboard::Event {
			iced::keyboard::Event::KeyPressed {
				key: key.clone(),
				modified_key: key,
				physical_key: Physical::Code(code),
				location: Location::Standard,
				modifiers,
				text: text.map(Into::into),
				repeat: false,
			}
		}

		let (mut app, mut rx) = app_with_terminal(16);
		let _ = app.on_key(typed(
			Key::Character("f".into()),
			Code::KeyF,
			Modifiers::CTRL | Modifiers::SHIFT,
			None,
		));
		assert!(app.search.is_some(), "the find bar opened");
		assert_eq!(next_input(&mut rx), None, "the shortcut is cmote's own");

		// A plain keystroke now belongs to the bar's field (which types through the widget tree),
		// so nothing goes to the remote.
		let x = typed(
			Key::Character("x".into()),
			Code::KeyX,
			Modifiers::empty(),
			Some("x"),
		);
		let _ = app.on_key(x.clone());
		assert_eq!(next_input(&mut rx), None, "typing searched, not the shell");

		// Esc closes the bar, and the very same keystroke reaches the shell again.
		let _ = app.on_key(key_press(Named::Escape, Code::Escape, Modifiers::empty()));
		assert!(app.search.is_none(), "Esc closed the find bar");
		let _ = app.on_key(x);
		assert_eq!(next_input(&mut rx).as_deref(), Some(&b"x"[..]));

		// A click on the grid closes it too (§35) — it takes the focus off the bar's field, so
		// leaving the bar up would leave the keyboard swallowed by a field without the cursor.
		let _ = app.on_key(typed(
			Key::Character("f".into()),
			Code::KeyF,
			Modifiers::CTRL | Modifiers::SHIFT,
			None,
		));
		assert!(app.search.is_some(), "reopened");
		app.on_grid_pressed();
		assert!(app.search.is_none(), "a grid press dismissed the find bar");
	}

	/// A query scans the WHOLE scrollback, lands on the newest match and selects it, and stepping ↑
	/// walks back into history — scrolling the older hit into view (§35). The selection is an
	/// ordinary one, which is what makes the existing highlight and Copy serve a search result.
	#[test]
	fn a_query_finds_the_newest_match_and_stepping_walks_back_into_history() {
		let (mut app, _rx) = app_with_terminal(16);
		{
			let terminal = app.terminal.as_mut().unwrap();
			// One hit far enough back to have scrolled off the 24-row screen, and one near the
			// live bottom.
			terminal.process(b"needle first\r\n");
			let filler: Vec<u8> = (0..40).flat_map(|_| b"filler\r\n".to_vec()).collect();
			terminal.process(&filler);
			terminal.process(b"needle last\r\n");
		}

		let _ = app.open_term_find();
		app.term_find_query("needle".to_owned());

		let search = app.search.as_ref().expect("the bar is open");
		assert_eq!(search.count(), 2, "both hits found, history included");
		assert_eq!(search.ordinal(), 2, "a new query lands on the newest match");
		let newest = search.current().expect("a current match").line;
		assert_eq!(offset(&app), 0, "the newest hit was already on screen");
		let text = app
			.selection
			.expect("the match is selected")
			.extract(app.terminal.as_ref().unwrap().screen());
		assert_eq!(text, "needle");

		// Step ↑ (older): the earlier hit left the screen long ago, so the view climbs to it.
		app.term_find_step(false);
		let search = app.search.as_ref().expect("the bar is still open");
		assert_eq!(search.ordinal(), 1);
		assert!(
			search.current().expect("a current match").line < newest,
			"stepped back into history"
		);
		assert!(offset(&app) > 0, "the view climbed to show the older hit");
		let text = app
			.selection
			.expect("the older match is selected")
			.extract(app.terminal.as_ref().unwrap().screen());
		assert_eq!(text, "needle");
	}

	/// Every hit on the visible screen is handed to the renderer, not only the current one (§39) —
	/// the wash the grid paints under the others. The list is resolved against the viewport as it is
	/// parked, so a hit up in the history counts in the bar's total without being painted, and the
	/// current hit is in the list too, with the selection drawn over it.
	#[test]
	fn the_renderer_is_given_every_hit_on_the_visible_screen() {
		let (mut app, _rx) = app_with_terminal(16);
		{
			let terminal = app.terminal.as_mut().unwrap();
			// One hit scrolled off the 24-row screen, then two within a couple of rows of the bottom.
			terminal.process(b"needle offscreen\r\n");
			let filler: Vec<u8> = (0..40).flat_map(|_| b"filler\r\n".to_vec()).collect();
			terminal.process(&filler);
			terminal.process(b"needle one\r\nneedle two\r\n");
		}

		let _ = app.open_term_find();
		app.term_find_query("needle".to_owned());

		let search = app.search.as_ref().expect("the bar is open");
		assert_eq!(search.count(), 3, "all three hits are in the bar's total");
		let screen = app.terminal.as_ref().unwrap().screen();
		let visible = search.visible(
			screen.history_size(),
			screen.display_offset(),
			screen.size().0,
		);
		assert_eq!(
			visible.len(),
			2,
			"the two hits on screen are washed; the one in history is not"
		);
		// Both cover the six cells of the word, on two different rows.
		assert!(
			visible
				.iter()
				.all(|found| (found.start_col, found.end_col) == (0, 5))
		);
		assert_ne!(visible[0].row, visible[1].row);
		// The current hit is selected, over its own cells — which is what makes it draw in the
		// selection's colour over the wash rather than beside it. Both speak absolute lines (§40), so
		// the check needs no mapping of its own.
		let current = search.current().expect("a current match");
		let selection = app.selection.expect("the current match is selected");
		assert!(selection.contains(current.line, current.start_col));
		assert!(selection.contains(current.line, current.end_col));
	}

	/// A hit printed while the bar is open joins the count and the washes on its own (§44) — the bar
	/// keeps up with a `tail -f` instead of describing the scrollback as it was when the query was
	/// typed. The scan is deferred to a frame tick rather than run per output chunk, so this comes in
	/// two halves: the chunk marks the list stale, and the tick is what rebuilds it.
	#[test]
	fn output_under_an_open_find_bar_is_picked_up_on_the_next_frame() {
		let (mut app, _rx) = app_with_terminal(16);
		app.terminal.as_mut().unwrap().process(b"needle first\r\n");

		let _focus = app.open_term_find();
		app.term_find_query("needle".to_owned());
		assert_eq!(
			app.search.as_ref().unwrap().count(),
			1,
			"one hit when the query was typed"
		);
		assert!(
			!app.search_stale,
			"and the list is as fresh as the document"
		);

		// The shell prints a second hit. The chunk itself must not scan — a flood of them arrives per
		// frame, and paying for a whole-document walk on each is what the flag exists to avoid.
		let _ = app.on_ssh_event(shell_output(b"needle second\r\n"));
		assert!(app.search_stale, "the chunk marked the list stale");
		assert_eq!(
			app.search.as_ref().unwrap().count(),
			1,
			"and did not scan on the spot"
		);

		// The frame tick the flag subscribed to.
		app.rescan_find();
		let search = app.search.as_ref().expect("the bar stays open");
		assert_eq!(search.count(), 2, "the hit that arrived is in the count");
		assert_eq!(
			search.ordinal(),
			1,
			"and the current hit stayed put rather than jumping to it"
		);
		assert!(!app.search_stale, "which stops the frame clock again");
	}

	/// A re-scan is not a step: it must not scroll, and it must not move the selection (§44). Output
	/// arriving under a bar parked up in the history would otherwise drag the viewport to the newest
	/// hit while the older one is being read.
	#[test]
	fn a_rescan_leaves_the_viewport_and_the_selection_where_they_are() {
		let (mut app, _rx) = app_with_terminal(16);
		{
			let terminal = app.terminal.as_mut().unwrap();
			// One hit far enough back that reaching it has to scroll the 24-row screen.
			terminal.process(b"needle first\r\n");
			let filler: Vec<u8> = (0..40).flat_map(|_| b"filler\r\n".to_vec()).collect();
			terminal.process(&filler);
			terminal.process(b"needle last\r\n");
		}

		let _focus = app.open_term_find();
		app.term_find_query("needle".to_owned());
		// Step back to the older hit, which climbs into the history and parks there.
		app.term_find_step(false);
		assert!(offset(&app) > 0, "parked up in the history");

		let _ = app.on_ssh_event(shell_output(b"needle third\r\n"));
		// The offset is read AFTER the output: the engine moves the viewport itself to keep the same
		// text on screen as lines scroll off, and that is not what this test is about.
		let parked = offset(&app);
		let selected = app.selection;

		app.rescan_find();

		assert_eq!(offset(&app), parked, "the re-scan did not scroll");
		assert_eq!(app.selection, selected, "nor moved the selection");
		assert_eq!(
			app.search.as_ref().unwrap().count(),
			3,
			"it did pick the new hit up, though"
		);
	}

	/// Nothing to re-scan starts no frame clock (§44): a closed bar has no match list, and an open one
	/// with nothing typed in it has no matches. Output on an ordinary session must not put the window
	/// into a per-frame scan it has no use for.
	#[test]
	fn output_with_no_query_starts_no_frame_clock() {
		let (mut app, _rx) = app_with_terminal(16);

		let _ = app.on_ssh_event(shell_output(b"hello\r\n"));
		assert!(!app.search_stale, "no bar, so no list to invalidate");

		let _focus = app.open_term_find();
		let _ = app.on_ssh_event(shell_output(b"hello again\r\n"));
		assert!(!app.search_stale, "an idle bar has nothing to re-scan");
	}

	// --- becoming another account on the same connection (§45) ---

	// A tab whose login shell is up and listed as its first identity, exactly as `Connected`
	// leaves it. Every elevation test starts from here, since an identity to park INTO is what
	// makes a switch possible at all.
	fn app_with_login_identity() -> (Tab, mpsc::Receiver<SshCommand>) {
		let (mut app, rx) = app_with_terminal(32);
		app.screen = AppScreen::Terminal;
		app.identities = vec![Identity {
			id: bridge::LOGIN_IDENTITY,
			account: None,
			ready: true,
			work: Workspace::default(),
		}];
		app.identity = bridge::LOGIN_IDENTITY;
		app.next_identity = 1;
		(app, rx)
	}

	// Put a second account's shell on screen the way the SSH side reports one. It no longer goes
	// through a dialog — that UX was withdrawn — so the identity is listed here as `elevate_submit`
	// used to list it, and then announced live. Returns the new identity's number, which is also on
	// screen when this returns.
	fn elevate_to(app: &mut Tab) -> u64 {
		let id = app.next_identity;
		app.next_identity += 1;
		app.identities.push(Identity {
			id,
			account: Some("root".to_owned()),
			ready: false,
			work: Workspace::default(),
		});
		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity: u64::MAX, // a stray event for nothing, to prove it disturbs nothing
			reason: None,
		});
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity: id,
			factors: 1,
		});
		id
	}

	// --- §47: the accounts dialog, the stored preference, and the remembered password ---

	// A tab with a login shell up and a saved target behind it, which is what every §47 test needs:
	// the preference and the password flag live on the TARGET, so a session with no target to write
	// to would exercise half of each path.
	fn app_with_saved_target() -> (Tab, mpsc::Receiver<SshCommand>) {
		let (mut app, rx) = app_with_login_identity();
		app.connection = Some("cme@rec:22".to_owned());
		app.targets.borrow_mut().upsert_on_connect(
			"rec",
			22,
			"cme",
			AuthKind::Password,
			None,
			None,
		);
		(app, rx)
	}

	// What the target remembers about becoming another account, or `None`.
	fn saved_elevation(app: &Tab) -> Option<crate::targets::Elevation> {
		app.targets
			.borrow()
			.find("cme@rec:22")
			.and_then(|target| target.elevate.clone())
	}

	// Ask for an elevation the way the user does: open the dialog, type the account, tick what is
	// wanted, submit. Driving it through `update` is what pins the wiring — an edit reaching a closed
	// dialog would silently do nothing and these tests would catch it.
	fn ask_to_become(app: &mut Tab, account: &str, on_connect: bool, remember: bool) {
		if app.elevate_form_mut().is_none() {
			let _focus = app.update(Message::AccountPressed);
		}
		let _ = app.update(Message::ElevateAccountEdited(account.to_owned()));
		if on_connect {
			let _ = app.update(Message::ElevateOnConnectToggled(true));
		}
		if remember {
			let _ = app.update(Message::ElevateRememberToggled(true));
		}
		let _ = app.update(Message::ElevateSubmitted);
	}

	/// The whole of the ordinary path (§47): ask to become root, answer the question sudo asks, and
	/// end up with root's terminal on screen — with what was asked for remembered on the target.
	#[test]
	fn becoming_another_account_asks_answers_and_lands() {
		let (mut app, mut rx) = app_with_saved_target();

		ask_to_become(&mut app, "root", true, false);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate {
				identity,
				kind,
				user,
			}) => {
				assert_eq!(kind, crate::elevate::ElevateKind::Sudo);
				assert_eq!(user, "root");
				identity
			}
			other => panic!("expected an elevation, got {other:?}"),
		};
		// Listed at once and not ready: a shell still elevating cannot be switched to, but a failure
		// has to have something to be reported against (§45).
		assert!(
			app.identities
				.iter()
				.any(|entry| entry.id == identity && !entry.ready),
			"the elevating identity is listed"
		);
		// The preference is stored on the way out, not on success: it says what the NEXT connection
		// should try, and a refused attempt is still what the user asked for.
		let saved = saved_elevation(&app).expect("the target remembers the account");
		assert_eq!(saved.account, "root");
		assert!(saved.on_connect);
		assert!(!saved.remember_password, "nothing was asked to be kept");

		// sudo asks, in its own words, and the dialog puts exactly that question.
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		match &app.elevate_form_mut().expect("the dialog is open").stage {
			ui::elevate::Stage::Answering { label, refusal, .. } => {
				assert_eq!(label, crate::elevate::MARKER);
				assert!(refusal.is_none());
			}
			other => panic!("expected a question, got {other:?}"),
		}

		// The answer goes down the wire as a `Secret`, and the field it was typed into is cleared.
		let _ = app.update(Message::ElevateAnswerEdited("hunter2".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		match next_command(&mut rx) {
			Some(SshCommand::ElevateAnswer {
				identity: to,
				secret,
			}) => {
				assert_eq!(to, identity);
				assert_eq!(secret.expose(), "hunter2");
			}
			other => panic!("expected an answer, got {other:?}"),
		}

		// The shell comes up, root's terminal is put on screen, and the dialog closes — there is
		// nothing left on it to answer.
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 1,
		});
		assert_eq!(app.identity, identity, "root's terminal is on screen");
		assert!(app.modal.is_none(), "the dialog is done asking");
		assert_eq!(app.showing_account(), Some("root"), "and the bar names it");
	}

	/// An account name is vetted at the field, not quoted and hoped for (§12, §47): the one place
	/// cmote composes a remote command line from something the user typed.
	#[test]
	fn an_account_that_is_not_a_login_name_is_refused_at_the_field() {
		let (mut app, mut rx) = app_with_saved_target();

		for attempt in ["root; rm -rf /", "-froot", "", "ro ot", "root$(id)"] {
			ask_to_become(&mut app, attempt, false, false);
			assert!(
				next_command(&mut rx).is_none(),
				"{attempt:?} must not reach the wire"
			);
			let form = app.elevate_form_mut().expect("the dialog stays open");
			assert!(
				form.error.is_some(),
				"{attempt:?} is reported under the form"
			);
			assert!(
				matches!(form.stage, ui::elevate::Stage::Asking),
				"{attempt:?} leaves the form asking"
			);
		}
		assert!(saved_elevation(&app).is_none(), "and nothing is remembered");
	}

	/// A password is stored only when the elevation SUCCEEDED and only when one factor was asked for
	/// (§45, §47). This is the ordinary case: one question, one answer, kept.
	#[test]
	fn a_password_that_worked_is_kept_when_it_was_asked_for() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		let _ = app.update(Message::ElevateAnswerEdited("hunter2".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		let _drain = next_command(&mut rx);
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 1,
		});

		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert_eq!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.map(|secret| secret.expose().to_owned()),
			Some("hunter2".to_owned()),
			"the password that worked is in the vault"
		);
		assert!(
			saved_elevation(&app).expect("remembered").remember_password,
			"and the target says so, so the dialog can promise it"
		);
	}

	/// SECURITY (§45, §47): an account that took TWO factors has nothing kept. The second question
	/// may have been a one-time code, and a code stored as a password would be replayed to a machine
	/// that has already spent it — which is the same rule that stops the FILE side following such an
	/// account (§46), read off the same number.
	#[test]
	fn an_account_that_took_two_factors_has_nothing_kept() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		// The password, then a second factor — both under cmote's own marker, which is exactly why
		// the wording cannot be what tells them apart.
		for _ in 0..2 {
			let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
				identity,
				label: crate::elevate::MARKER.to_owned(),
				refusal: None,
			});
			let _ = app.update(Message::ElevateAnswerEdited("123456".to_owned()));
			let _ = app.update(Message::ElevateAnswerSubmitted);
			let _drain = next_command(&mut rx);
		}
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 2,
		});

		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.is_none(),
			"two factors, so nothing is kept"
		);
		assert!(
			!saved_elevation(&app).expect("remembered").remember_password,
			"and the flag says nothing is stored, so nothing promises otherwise"
		);
	}

	/// A refused elevation keeps nothing either (§47), and the reason goes under the form so the
	/// account can be corrected where it was typed.
	#[test]
	fn a_refused_elevation_reports_where_it_was_asked_and_keeps_nothing() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		let _ = app.update(Message::ElevateAnswerEdited("wrong".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		let _drain = next_command(&mut rx);
		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity,
			reason: Some("3 incorrect password attempts".to_owned()),
		});

		let form = app.elevate_form_mut().expect("the dialog is still open");
		assert_eq!(
			form.error.as_deref(),
			Some("3 incorrect password attempts"),
			"the remote's own words, under the form"
		);
		assert!(
			matches!(form.stage, ui::elevate::Stage::Asking),
			"and the form is asking again, so the account can be corrected"
		);
		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.is_none(),
			"a password that was refused is never stored"
		);
	}

	/// A target that remembers an elevation acts on it as soon as the shell is live (§47), and the
	/// stored password answers the first question without a dialog.
	#[test]
	fn a_remembered_elevation_runs_itself_on_connect() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		let mut vault = crate::vault::Vault::for_tests(dir.path());
		vault
			.store(
				&crate::vault::elevation_key("cme@rec:22", "root"),
				Secret::new("hunter2".to_owned()),
			)
			.expect("the test vault stores");
		*app.vault.borrow_mut() = Some(vault);
		app.targets.borrow_mut().set_elevation(
			"cme@rec:22",
			"root",
			crate::elevate::ElevateKind::Sudo,
			true,
		);
		app.targets
			.borrow_mut()
			.set_elevation_remembered("cme@rec:22", "root", true);

		app.elevate_on_connect();
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, user, .. }) => {
				assert_eq!(user, "root");
				identity
			}
			other => panic!("expected an elevation, got {other:?}"),
		};
		// No dialog: nobody asked for one, and the stored password answers the question by itself.
		assert!(app.modal.is_none(), "nothing was put in the user's way");
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		match next_command(&mut rx) {
			Some(SshCommand::ElevateAnswer { secret, .. }) => {
				assert_eq!(secret.expose(), "hunter2", "answered from the vault");
			}
			other => panic!("expected an answer, got {other:?}"),
		}
		assert!(app.modal.is_none(), "and still nothing in the way");
	}

	/// A stored password that the remote REFUSES puts the question to the user rather than trying it
	/// again (§47): a refusal arrives as the same question with the program's words attached, and a
	/// stored password is offered once.
	#[test]
	fn a_refused_stored_password_puts_the_question_to_the_user() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		let mut vault = crate::vault::Vault::for_tests(dir.path());
		vault
			.store(
				&crate::vault::elevation_key("cme@rec:22", "root"),
				Secret::new("stale".to_owned()),
			)
			.expect("the test vault stores");
		*app.vault.borrow_mut() = Some(vault);

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		assert!(
			matches!(
				next_command(&mut rx),
				Some(SshCommand::ElevateAnswer { .. })
			),
			"the stored password is tried first"
		);
		// Refused: the same question comes back with the program's words about the last answer.
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: Some("Sorry, try again.".to_owned()),
		});
		assert!(
			next_command(&mut rx).is_none(),
			"the stored password is not tried twice"
		);
		match &app.elevate_form_mut().expect("the dialog is open").stage {
			ui::elevate::Stage::Answering { refusal, .. } => {
				assert_eq!(refusal.as_deref(), Some("Sorry, try again."));
			}
			other => panic!("expected the question to be put, got {other:?}"),
		}
	}

	/// A hand-edited `targets.json` is remote input as far as the account check is concerned (§12,
	/// §47): an elevation whose account is not a plain login name is a stored preference cmote
	/// declines to act on, not an error to report.
	#[test]
	fn a_stored_elevation_with_an_impossible_account_is_not_acted_on() {
		let (mut app, mut rx) = app_with_saved_target();
		// Written past the dialog's own check, which is what editing the file by hand does: the
		// setter stores what it is given, and the READ is where the account is vetted.
		app.targets.borrow_mut().set_elevation(
			"cme@rec:22",
			"root; id",
			crate::elevate::ElevateKind::Sudo,
			true,
		);

		app.elevate_on_connect();
		assert!(
			next_command(&mut rx).is_none(),
			"nothing composed from it reaches the wire"
		);
		assert!(app.modal.is_none(), "and nothing is put in the way");
	}

	/// Switching between the accounts a session has, and closing one, are where the dialog puts them
	/// — beside the account they act on (§45, §47). The login account has no ✕: ending it is what
	/// Disconnect does.
	#[test]
	fn the_dialog_lists_every_account_and_only_elevated_ones_close() {
		let (mut app, mut rx) = app_with_saved_target();
		let root = elevate_to(&mut app);

		let rows = app.account_rows();
		assert_eq!(rows.len(), 2, "the login account and root");
		let login = rows
			.iter()
			.find(|row| row.identity == bridge::LOGIN_IDENTITY)
			.expect("the login account is listed");
		assert_eq!(login.label, "cme", "named from the session's endpoint");
		assert!(!login.closable, "ending it is what Disconnect does");
		let elevated = rows
			.iter()
			.find(|row| row.identity == root)
			.expect("root is listed");
		assert_eq!(elevated.label, "root");
		assert!(elevated.closable);
		assert!(elevated.selected, "and it is the one on screen");

		// Switching back to the login account, by clicking its name.
		let _task = app.update(Message::IdentitySelected(bridge::LOGIN_IDENTITY));
		assert_eq!(app.identity, bridge::LOGIN_IDENTITY);
		assert_eq!(app.showing_account(), None, "so the bar stops naming one");

		// And closing root: EOF on its channel. Drained rather than taken one at a time, because a
		// switch sends the file panes' re-listing ahead of it (§46).
		let _task = app.update(Message::IdentityClosed(root));
		let mut closed = Vec::new();
		while let Some(command) = next_command(&mut rx) {
			if let SshCommand::CloseIdentity(id) = command {
				closed.push(id);
			}
		}
		assert_eq!(closed, vec![root], "the close goes down the wire");
		// The list entry stays until the session says the shell has ended — a shell that refuses to
		// die must not vanish from the dialog.
		assert!(app.identities.iter().any(|entry| entry.id == root));
		// The login identity is not closable this way, whatever asks.
		let _task = app.update(Message::IdentityClosed(bridge::LOGIN_IDENTITY));
		let mut after = Vec::new();
		while let Some(command) = next_command(&mut rx) {
			if let SshCommand::CloseIdentity(id) = command {
				after.push(id);
			}
		}
		assert!(after.is_empty(), "the login shell is Disconnect's to end");
	}

	/// Pressing Account while sudo is asking shows the question, not a blank form over an elevation
	/// that is still waiting for an answer (§47).
	#[test]
	fn re_opening_the_dialog_does_not_throw_away_an_outstanding_question() {
		let (mut app, mut rx) = app_with_saved_target();
		ask_to_become(&mut app, "root", false, false);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevatePrompt {
			identity,
			label: "Verification code:".to_owned(),
			refusal: None,
		});

		let _focus = app.update(Message::AccountPressed);
		match &app.elevate_form_mut().expect("still open").stage {
			ui::elevate::Stage::Answering { label, .. } => {
				assert_eq!(
					label, "Verification code:",
					"the question survives the press"
				);
			}
			other => panic!("expected the question, got {other:?}"),
		}
	}

	/// Switching accounts moves the FILE panes too (§46), and reads them again as the account now
	/// selected: the path stays — elevating because a folder would not open is the ordinary reason to
	/// do it — but nothing another account listed is left on screen while the new listing is awaited.
	#[test]
	fn switching_accounts_reads_the_file_panes_again_as_the_new_account() {
		let (mut app, mut rx) = app_with_login_identity();
		// A tree with a listed, open folder and a pane showing it — `cme`'s view of /etc.
		let _fetch = app.panes.tree.expand("/etc", false);
		app.panes.tree.listed("/etc", vec!["ssl".to_owned()]);
		if let Some(request) = app.panes.pane.show("/etc") {
			app.list_files(request);
		}
		// Becoming root puts root's shell on screen, and that same switch moves the panes.
		let root = elevate_to(&mut app);
		assert_eq!(app.identity, root);

		let sent = drain(&mut rx);
		// The account is announced BEFORE the listings, on the one ordered channel, so a listing can
		// never be answered by the account being left.
		let select = sent
			.iter()
			.position(|command| matches!(command, SshCommand::SelectIdentity(id) if *id == root))
			.expect("the switch is announced");
		let listed = sent
			.iter()
			.position(|command| matches!(command, SshCommand::ListDir(path) if path == "/etc"))
			.expect("the open folder is read again");
		assert!(select < listed, "the account is named first");
		assert!(
			sent.iter().any(
				|command| matches!(command, SshCommand::ListFiles { path, .. } if path == "/etc")
			),
			"and so is the pane's own folder"
		);
		// Nothing `cme` listed is on screen in the meantime: the rows stand empty under the spinner
		// until root's own listing lands.
		assert!(
			app.panes
				.tree
				.rows()
				.iter()
				.all(|row| row.path != "/etc/ssl"),
			"another account's children must not survive the switch"
		);
		assert_eq!(app.panes.pane.count(), 0, "nor its files");

		// And it happens in both directions: going back to `cme` re-reads what root had listed.
		app.panes.tree.listed("/etc", vec!["shadow.d".to_owned()]);
		let _task = app.switch_identity(bridge::LOGIN_IDENTITY);
		let back = drain(&mut rx);
		assert!(
			back.iter()
				.any(|command| matches!(command, SshCommand::ListDir(path) if path == "/etc")),
			"the folder is read again as the login account too"
		);
		assert!(
			app.panes
				.tree
				.rows()
				.iter()
				.all(|row| row.path != "/etc/shadow.d"),
			"and root's children go with the switch"
		);
	}

	/// A file opened as root belongs to root for as long as the editor lives (§46): its save names
	/// that account, not whichever one the session happens to be showing when Save is pressed.
	#[test]
	fn a_file_opened_as_root_is_still_saved_as_root_after_switching_back() {
		let (mut session, rx) = app_with_login_identity();
		let root = elevate_to(&mut session);
		let mut app = tab_app();
		let id = session.id;
		let region = strip_mut(&mut app);
		region.tabs.clear();
		region.tabs.push(session);
		region.active = 0;
		app.next_id = id + 1;

		let _task = app.open_viewer(app.focus, id, "/root/.ssh/authorized_keys".to_owned());
		let editor = app
			.tabs()
			.find_map(Tab::editor)
			.expect("the editor tab is open");
		assert_eq!(editor.identity, root, "opened as the account on screen");

		// The session goes back to `cme` while the file is still open, and the save still names root.
		let viewer_id = app
			.tabs()
			.find(|tab| tab.editor().is_some())
			.map(|tab| tab.id)
			.expect("the editor tab has an id");
		if let Some(tab) = app.tab_mut(id) {
			let _task = tab.switch_identity(bridge::LOGIN_IDENTITY);
		}
		let mut rx = rx;
		let _drained = drain(&mut rx);
		let _task = app.flush_editor_save(viewer_id);

		let saved = drain(&mut rx)
			.into_iter()
			.find_map(|command| match command {
				SshCommand::EditSave { identity, .. } => Some(identity),
				_ => None,
			})
			.expect("the save was sent");
		assert_eq!(saved, root, "written back as the account that read it");
	}

	// The commands queued for the SSH task, drained in order.
	fn drain(rx: &mut mpsc::Receiver<SshCommand>) -> Vec<SshCommand> {
		let mut out = Vec::new();
		while let Ok(command) = rx.try_recv() {
			out.push(command);
		}
		out
	}

	/// Switching accounts swaps a whole view, not just the grid (§45): the scrollback, the
	/// selection and the find bar all belong to the account, and all of them come back.
	#[test]
	fn switching_accounts_parks_one_whole_view_and_restores_the_other() {
		let (mut app, _rx) = app_with_login_identity();
		let _ = app.on_ssh_event(shell_output(b"i am cme\r\n"));
		let _focus = app.open_term_find();
		app.term_find_query("cme".to_owned());
		assert_eq!(app.search.as_ref().unwrap().count(), 1);

		let root = elevate_to(&mut app);
		assert_eq!(app.identity, root, "the new account comes forward");
		assert!(
			app.search.is_none(),
			"root's view has its own find bar, which is shut"
		);
		assert!(
			app.terminal.as_ref().unwrap().find("i am cme").is_empty(),
			"and its own scrollback, which is empty"
		);

		// Back to the login account: everything that was parked is on screen again.
		let _task = app.switch_identity(bridge::LOGIN_IDENTITY);
		assert_eq!(app.identity, bridge::LOGIN_IDENTITY);
		assert!(
			!app.terminal.as_ref().unwrap().find("i am cme").is_empty(),
			"cme's scrollback survived the round trip"
		);
		assert_eq!(
			app.search
				.as_ref()
				.map(super::super::term::search::Search::count),
			Some(1),
			"and so did its find bar, query and all"
		);
	}

	/// Output for an account that is NOT on screen fills that account's own scrollback (§45) — a
	/// build left running as cme must not print into root's grid, and must not be lost either.
	#[test]
	fn output_for_a_parked_account_goes_to_its_own_scrollback() {
		let (mut app, mut rx) = app_with_login_identity();
		let root = elevate_to(&mut app);
		let _ = drain(&mut rx);

		// cme's shell keeps talking while root's is on screen.
		let _ = app.on_ssh_event(shell_output(b"still building\r\n"));
		assert!(
			app.terminal
				.as_ref()
				.unwrap()
				.find("still building")
				.is_empty(),
			"root's grid is not where cme's output belongs"
		);
		let parked = app
			.identities
			.iter()
			.find(|identity| identity.id == bridge::LOGIN_IDENTITY)
			.and_then(|identity| identity.work.terminal.as_ref())
			.expect("cme's terminal is parked, not dropped");
		assert!(
			!parked.find("still building").is_empty(),
			"it went into cme's own scrollback"
		);
		assert_eq!(app.identity, root, "and the view never moved");
	}

	/// A query from a parked account is still answered — its program is blocked until it is
	/// (§23) — and answered on that account's OWN channel, not down the typing path, which goes
	/// wherever the user is looking (§45).
	#[test]
	fn a_parked_accounts_query_is_answered_on_its_own_channel() {
		let (mut app, mut rx) = app_with_login_identity();
		let _root = elevate_to(&mut app);
		let _ = drain(&mut rx);

		// A cursor-position report request from the shell the user is NOT looking at.
		let _ = app.on_ssh_event(shell_output(b"\x1b[6n"));
		let sent = drain(&mut rx);
		let reply = sent
			.iter()
			.find_map(|command| match command {
				SshCommand::Reply { identity, bytes } => Some((*identity, bytes.clone())),
				_ => None,
			})
			.expect("the query was answered");
		assert_eq!(
			reply.0,
			bridge::LOGIN_IDENTITY,
			"to the shell that asked, not the one on screen"
		);
		assert!(!reply.1.is_empty());
		assert!(
			!sent
				.iter()
				.any(|command| matches!(command, SshCommand::Input(_))),
			"never as ordinary input, which would go to the wrong shell"
		);
	}

	/// The words an elevation ends with are the account's own greeting and its first prompt, flushed as
	/// the program hands the channel over (§45). They must survive arriving BEFORE the identity has an
	/// emulator — dropping them is what left a freshly elevated terminal blank but for its caret.
	#[test]
	fn the_greeting_an_elevation_ends_with_is_not_lost_to_the_order_it_arrives_in() {
		let (mut app, _rx) = app_with_login_identity();
		let root = app.next_identity;
		app.next_identity += 1;
		app.identities.push(Identity {
			id: root,
			account: Some("root".to_owned()),
			ready: false,
			work: Workspace::default(),
		});

		// The flush, arriving while root is still off screen and has no terminal of its own.
		let _task = app.on_ssh_event(SshEvent::Output {
			identity: root,
			bytes: b"root@rec:~# ".to_vec(),
		});
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity: root,
			factors: 1,
		});
		assert_eq!(app.identity, root, "and it is brought forward");
		assert!(
			!app.terminal
				.as_ref()
				.expect("root has a terminal")
				.find("root@rec")
				.is_empty(),
			"the prompt it printed is on screen, not swallowed"
		);
	}

	/// An elevated shell exiting brings the login account forward with its view intact (§45), and
	/// the session carries on — only the login shell going down ends that.
	#[test]
	fn an_elevated_shell_exiting_falls_back_to_the_login_account() {
		let (mut app, _rx) = app_with_login_identity();
		let _ = app.on_ssh_event(shell_output(b"i am cme\r\n"));
		let root = elevate_to(&mut app);

		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity: root,
			reason: None,
		});
		assert_eq!(app.identity, bridge::LOGIN_IDENTITY);
		assert!(
			!app.terminal.as_ref().unwrap().find("i am cme").is_empty(),
			"back to cme's own scrollback, where it was left"
		);
		assert_eq!(app.identities.len(), 1);
		assert!(
			matches!(app.screen, AppScreen::Terminal),
			"the session is still up"
		);
	}

	/// The session ending takes every identity with it (§45): they were shells on that connection.
	#[test]
	fn disconnecting_forgets_every_account() {
		let (mut app, _rx) = app_with_login_identity();
		let _root = elevate_to(&mut app);

		let _task = app.on_ssh_event(SshEvent::Disconnected);
		assert!(app.identities.is_empty());
		assert_eq!(app.identity, bridge::LOGIN_IDENTITY);
	}

	#[test]
	fn prompt_jump_maps_only_the_vertical_arrows() {
		use iced::keyboard::key::Named;

		assert_eq!(
			prompt_jump(Named::ArrowUp),
			Some(term::osc133::Osc133Direction::Previous)
		);
		assert_eq!(
			prompt_jump(Named::ArrowDown),
			Some(term::osc133::Osc133Direction::Next)
		);
		assert_eq!(prompt_jump(Named::ArrowLeft), None);
		assert_eq!(prompt_jump(Named::PageUp), None);
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
		// A 100-tall window over 20-tall rows, scrolled to the top. `None` IS the assertion here:
		// an already-visible row must produce no scroll at all, not a scroll to where it already is.
		assert_eq!(
			keep_visible(0.0, 100.0, 40.0, 20.0),
			None,
			"already visible"
		);
		// Off the bottom: scroll just far enough that its bottom edge lands on the
		// window's, not far enough to re-centre it.
		assert_eq!(keep_visible(0.0, 100.0, 120.0, 20.0), Some(40.0));
		// Off the top: its own top becomes the offset.
		assert_eq!(keep_visible(200.0, 100.0, 60.0, 20.0), Some(60.0));
		// A row taller than the window is shown from its top rather than its bottom.
		assert_eq!(keep_visible(0.0, 30.0, 10.0, 50.0), Some(10.0));
		assert_eq!(keep_visible(0.0, 30.0, 0.0, 50.0), None);
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
		let mut app = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};

		// A target connected to before, remembered at a shell directory and a *different*
		// pane directory — the divergent case a tree-click peek leaves behind.
		app.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		app.targets.borrow_mut().set_session(
			"u@h:22",
			crate::targets::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::targets::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.borrow().find("u@h:22").unwrap().clone());

		// One OSC 7 cwd announcement, as the shell emits on each prompt (§17).
		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		// Connect: the pane opens at its remembered directory, and the shell is set to resume
		// at its own — so the pane is pinned to `/etc` until the shell reaches `/var/log`.
		let _ = app.on_ssh_event(SshEvent::Connected);
		assert!(matches!(app.screen, AppScreen::Terminal));
		assert_eq!(app.panes.pane.path(), Some("/etc"));
		assert_eq!(app.resume_cwd.as_deref(), Some("/var/log"));

		// The login prompt announces the login directory first. The pane must NOT follow it
		// off `/etc` while the resume is still pending.
		let _ = app.on_ssh_event(announce("/home/u"));
		assert_eq!(
			app.panes.pane.path(),
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
		assert_eq!(app.panes.pane.path(), Some("/etc"), "kept, not clobbered");
		assert_eq!(app.resume_cwd, None, "no longer pinned");

		// A real move afterwards follows normally: the pane tracks the shell again.
		let _ = app.on_ssh_event(announce("/var/log/nginx"));
		assert_eq!(
			app.panes.pane.path(),
			Some("/var/log/nginx"),
			"following resumed"
		);
	}

	/// The pin is for BOTH panes (§18, §22). The tree follows the shell on every announcement
	/// exactly as the pane does, so the same login-then-`cd` sequence that would drag the pane off
	/// the restored view drags the tree off it too — and more expensively, since revealing a
	/// directory opens its whole chain and asks the server for a listing of every folder along it.
	/// A resume must leave both panes on the resume point, and both free to follow the next real
	/// move.
	#[test]
	fn a_reconnect_pins_the_tree_as_well_as_the_pane() {
		use crate::ui::connect::AuthKind;

		let (tx, _rx) = mpsc::channel(64);
		let mut app = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};

		app.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		app.targets.borrow_mut().set_session(
			"u@h:22",
			crate::targets::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::targets::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.borrow().find("u@h:22").unwrap().clone());

		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		// Both panes open on the resume point: the pane at its remembered directory, the tree
		// with the chain down to it open and that folder selected.
		let _ = app.on_ssh_event(SshEvent::Connected);
		assert_eq!(app.panes.tree.selected(), Some("/etc"));

		// The login prompt announces a directory the shell is about to leave. Neither pane may
		// be dragged onto it — the pane was always safe here, the tree was not.
		let _ = app.on_ssh_event(announce("/home/u"));
		assert_eq!(
			app.panes.tree.selected(),
			Some("/etc"),
			"the tree is pinned too"
		);

		// The replayed `cd` lands: the shell has settled and the pin lifts, but the restored
		// view stands in both panes rather than being clobbered by the arrival.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(
			app.panes.tree.selected(),
			Some("/etc"),
			"kept, not clobbered"
		);

		// A real move afterwards carries both panes, exactly as it always did.
		let _ = app.on_ssh_event(announce("/var/log/nginx"));
		assert_eq!(
			app.panes.tree.selected(),
			Some("/var/log/nginx"),
			"following resumed"
		);
	}

	/// A transfer that a DROPPED CONNECTION stopped is offered again by the next session to that
	/// same server (§16) — the whole lifecycle, through the two paths that wire it: the teardown
	/// that keeps the resume point and the connect that puts it back. Cancel and resume used to
	/// live inside one session, which left the commonest way a big transfer stops as the one way
	/// it could not be picked up from.
	#[test]
	fn a_transfer_the_lost_connection_stopped_is_offered_by_the_next_session() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.connection = Some("u@h:22".to_owned());

		// A folder coming down when the link dies. Started through the queue's own entrance, so
		// the slot and the in-flight memory are set exactly as a real download sets them.
		let effects = app
			.transfers
			.download_tree("/srv/logs".to_owned(), Some(PathBuf::from("/local")));
		let _ = app.apply(effects);
		assert!(app.transfers.progress().is_some(), "bytes are moving");

		// The remote hangs up. The queue is emptied with the rest of the session, and what it was
		// moving is the one thing kept — the partial on disk did not go anywhere.
		let _task = app.on_ssh_event(SshEvent::Disconnected);
		assert!(!app.transfers.can_resume(), "the queue kept nothing itself");
		assert!(app.unfinished.is_some(), "the tab did");

		// Reconnecting to the same endpoint offers to finish it, and says why it is asking.
		app.connection = Some("u@h:22".to_owned());
		let _task = app.on_ssh_event(SshEvent::Connected);
		assert!(app.transfers.can_resume());
		assert_eq!(
			app.transfers.notice(),
			Some("logs stopped when the connection dropped")
		);
		assert!(app.unfinished.is_none(), "the offer is spent either way");

		// And Resume re-issues the very same transfer, this time in resume mode: the task sizes
		// the destination and sends only what is missing.
		while next_command(&mut rx).is_some() {}
		let _task = app.update(Message::TransferResumePressed);
		match next_command(&mut rx) {
			Some(SshCommand::DownloadTree {
				remote,
				local,
				resume,
			}) => {
				assert_eq!(remote, "/srv/logs");
				assert_eq!(local, PathBuf::from("/local"));
				assert!(resume);
			}
			other => panic!("expected the same folder, resumed: {other:?}"),
		}
	}

	/// The offer belongs to the machine it was made on (§16): both its paths are that server's,
	/// and the partial it would append to is over there. A tab that goes somewhere else next is
	/// offered nothing — and is not left holding the offer either, since a resume point that
	/// waited through a session on another machine is one nobody remembers making.
	#[test]
	fn an_unfinished_transfer_is_not_offered_to_a_different_server() {
		let (mut app, _rx) = app_with_terminal(16);
		app.connection = Some("u@h:22".to_owned());
		let effects = app
			.transfers
			.download_tree("/srv/logs".to_owned(), Some(PathBuf::from("/local")));
		let _ = app.apply(effects);
		let _task = app.on_ssh_event(SshEvent::Disconnected);

		app.connection = Some("u@elsewhere:22".to_owned());
		let _task = app.on_ssh_event(SshEvent::Connected);
		assert!(!app.transfers.can_resume());
		assert_eq!(app.transfers.notice(), None);
		assert!(app.unfinished.is_none(), "spent, not left waiting");
	}

	/// Reveal is an explicit ask, so it ends the resume pin (§19, §22) — the same rule
	/// `move_shell_to` already follows, for the same reason: once the user has said where the
	/// panes go, the pin protecting the restored view has nothing left to protect.
	///
	/// Without that, pressing Reveal in the window between the login prompt and the replayed `cd`
	/// landing left the panes stranded. They went to the login directory, the still-armed pin
	/// swallowed the settle as "already there", and the shell then sat at a directory the panes
	/// had been explicitly asked to come to and had not — with no further announcement coming to
	/// put it right, since a shell standing still announces no move.
	#[test]
	fn reveal_during_a_resume_ends_the_pin_rather_than_stranding_the_panes() {
		use crate::ui::connect::AuthKind;

		let (tx, _rx) = mpsc::channel(64);
		let mut app = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};

		app.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		app.targets.borrow_mut().set_session(
			"u@h:22",
			crate::targets::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::targets::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.borrow().find("u@h:22").unwrap().clone());

		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		let _ = app.on_ssh_event(SshEvent::Connected);
		let _ = app.on_ssh_event(announce("/home/u"));
		assert_eq!(
			app.resume_cwd.as_deref(),
			Some("/var/log"),
			"still settling"
		);

		// The user asks for the panes to come to the shell, mid-resume.
		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.panes.pane.path(), Some("/home/u"), "the panes came");
		assert_eq!(app.resume_cwd, None, "and the pin is spent");

		// The replayed `cd` lands. It is a real move now, so both panes follow it — where
		// before, the leftover pin read it as "already there" and left them behind.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.panes.pane.path(), Some("/var/log"), "the pane kept up");
		assert_eq!(
			app.panes.tree.selected(),
			Some("/var/log"),
			"and the tree with it"
		);
	}

	/// The status bar's Reveal button (§19): the panes come to the shell, and nothing is typed at
	/// it. The case that matters is the one the shell cannot fix by itself — a browse away from a
	/// shell that has not moved since. Its next prompt announces the same directory, which is not a
	/// move, so the pane rightly stays put and only an explicit ask brings it back.
	#[test]
	fn reveal_brings_the_panes_to_the_shell_without_typing_anything() {
		let (mut app, mut rx) = app_with_terminal(32);
		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		// The shell says where it is, and both panes follow it there as usual.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.panes.pane.path(), Some("/var/log"));
		assert_eq!(app.panes.tree.selected(), Some("/var/log"));

		// A look somewhere else, with the tree walked off the shell's folder too.
		app.browse_to("/etc");
		app.panes.tree.select("/etc");
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(
			app.panes.pane.path(),
			Some("/etc"),
			"a re-announcement is not a move, so the browse stands (§19)"
		);

		let _ = drain(&mut rx);
		let _task = app.update(Message::RevealPressed);
		assert_eq!(
			app.panes.pane.path(),
			Some("/var/log"),
			"the pane came back"
		);
		assert_eq!(
			app.panes.tree.selected(),
			Some("/var/log"),
			"and the tree with it"
		);
		assert!(
			!drain(&mut rx)
				.iter()
				.any(|command| matches!(command, SshCommand::Input(_))),
			"the shell was never typed at — this moves the local view alone"
		);
	}

	/// With no cwd announcement (§17: it takes OSC 7, which not every shell sends) Reveal has
	/// nowhere to go, so it leaves both panes where they are rather than guessing at the root.
	/// The button dims in that case; this is what sits behind the dimming.
	#[test]
	fn reveal_does_nothing_when_the_shell_never_said_where_it_is() {
		let (mut app, mut rx) = app_with_terminal(32);
		app.browse_to("/etc");
		let _ = drain(&mut rx);

		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.panes.pane.path(), Some("/etc"), "left where it was");
		assert!(drain(&mut rx).is_empty(), "and nothing asked of the server");
	}

	/// Shift+click and Shift+arrow through the app's own handlers (§21) — the model's rules
	/// are tested next door in `files`, but only this path proves the wiring: the modifier
	/// state comes off the keyboard subscription, and a mouse press carries none of its own.
	#[test]
	fn shift_click_and_shift_arrow_reach_the_selection() {
		use iced::keyboard::{Event, Modifiers};

		let mut app = Tab::default();
		let request = app
			.panes
			.pane
			.show("/home")
			.expect("a new directory needs listing");
		app.panes.pane.chunk(
			request,
			["a", "b", "c", "d"]
				.into_iter()
				.map(|name| files::Entry {
					name: name.to_owned(),
					kind: files::FilesKind::File,
					meta: files::Meta::default(),
				})
				.collect(),
			true,
		);
		let chosen = |app: &Tab| {
			app.panes
				.pane
				.selected_rows(app.panes.show_hidden())
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

	// A bare app with one undivided region holding one home tab, and empty shared state, so the
	// tab-strip bookkeeping (§26) is exercised without an iced runtime or the disk. The `Task`s these
	// calls return are dropped — only the tab list and active index are under test.
	fn tab_app() -> App {
		let targets = Rc::new(RefCell::new(crate::targets::Targets::default()));
		let vault = Rc::new(RefCell::new(None));
		let first = Tab::home(targets.clone(), vault.clone(), 0, iced::Size::default());
		// The order starts on the tab already on screen — id 0, the home tab built above (§37).
		let (regions, focus) = pane_grid::State::new(Region::new(first));
		App {
			regions,
			focus,
			// A plausible window, so the split tests have room to divide and the overlay tests have
			// bounds to be clamped into (§48).
			window: iced::Size::new(1200.0, 800.0),
			next_id: 1,
			targets,
			vault,
			pending_close: None,
			pending_editor_close: None,
			quit: None,
			overlay: ui::dialog::Card::default(),
			// Default (nothing remembered): `save` is a no-op on default, so a quit test never
			// touches the disk (§31).
			settings: crate::settings::Settings::default(),
			// The pointer has not moved and no seam has been pressed — the state a divider
			// double-click starts from (§48).
			pointer: iced::Point::ORIGIN,
			seam_clicks: ui::selection::Clicks::default(),
			strip_menu: None,
		}
	}

	/// The tabs of the region holding the keyboard (§48). The strip assertions below read through
	/// this so they still say "the strip" and mean the strip the test set up; a test that splits
	/// deliberately names its regions instead.
	fn strip(app: &App) -> &[Tab] {
		&app.region().tabs
	}

	/// Which of that region's tabs is on screen (§48).
	fn on_screen(app: &App) -> usize {
		app.region().active
	}

	/// That region, mutably, for the tests that arrange a strip rather than assert on one (§48).
	fn strip_mut(app: &mut App) -> &mut Region {
		let pane = app.focused_pane();
		app.regions.get_mut(pane).expect("the focused region")
	}

	/// How many regions the window is divided into (§48).
	fn region_count(app: &App) -> usize {
		app.regions.len()
	}

	#[test]
	fn opening_a_tab_adds_a_fresh_home_tab_and_activates_it() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		assert_eq!(strip(&app).len(), 2);
		assert_eq!(on_screen(&app), 1, "the new tab is the active one");
		assert_ne!(strip(&app)[0].id, strip(&app)[1].id, "ids are never reused");
		assert!(matches!(strip(&app)[1].screen, AppScreen::Home));
	}

	#[test]
	fn closing_an_idle_tab_keeps_the_active_tab_the_same() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus); // 2 tabs, active = 1
		let _ = app.open_tab(app.focus); // 3 tabs, active = 2
		let first_id = strip(&app)[0].id;
		let active_id = strip(&app)[on_screen(&app)].id;
		// Closing an idle tab BEFORE the active one shifts indices but must leave the same tab active.
		let _ = app.request_close(first_id);
		assert_eq!(strip(&app).len(), 2);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			active_id,
			"same tab still on screen"
		);
	}

	#[test]
	fn closing_the_last_tab_asks_to_quit_instead_of_replacing_it() {
		let mut app = tab_app();
		let only_id = strip(&app)[0].id;
		let _ = app.request_close(only_id);
		// Closing the last tab would empty the window, so it raises the quit confirmation and keeps
		// the tab exactly as it was — the app leaves only once that is accepted (§30).
		assert!(matches!(app.quit, Some(QuitPhase::Confirming)));
		assert_eq!(strip(&app).len(), 1);
		assert_eq!(
			strip(&app)[0].id,
			only_id,
			"the tab is untouched, not replaced"
		);
	}

	#[test]
	fn a_window_close_request_raises_the_quit_confirmation() {
		let mut app = tab_app();
		let _ = app.request_quit();
		assert!(matches!(app.quit, Some(QuitPhase::Confirming)));
	}

	#[test]
	fn cancelling_backs_out_of_the_quit_while_still_confirming() {
		let mut app = tab_app();
		let _ = app.request_quit();
		let _ = app.quit_cancelled();
		assert!(
			app.quit.is_none(),
			"the confirmation is dismissed, app stays open"
		);
	}

	#[test]
	fn cancelling_is_inert_once_the_drain_has_begun() {
		let mut app = tab_app();
		// A stray backdrop click mid-teardown must not abort a disconnect already under way (§30).
		app.quit = Some(QuitPhase::Draining {
			pending: vec![1],
			since: std::time::Instant::now(),
		});
		let _ = app.quit_cancelled();
		assert!(
			matches!(app.quit, Some(QuitPhase::Draining { .. })),
			"draining cannot be cancelled"
		);
	}

	#[test]
	fn requesting_quit_supersedes_a_pending_single_tab_close() {
		let mut app = tab_app();
		// Quitting closes every tab, so a lone tab's close confirmation is dropped in its favour.
		app.pending_close = Some(strip(&app)[0].id);
		let _ = app.request_quit();
		assert!(app.pending_close.is_none());
		assert!(matches!(app.quit, Some(QuitPhase::Confirming)));
	}

	#[test]
	fn requesting_quit_again_does_not_restart_an_in_flight_quit() {
		let mut app = tab_app();
		app.quit = Some(QuitPhase::Draining {
			pending: vec![1],
			since: std::time::Instant::now(),
		});
		let _ = app.request_quit();
		assert!(
			matches!(app.quit, Some(QuitPhase::Draining { .. })),
			"a second request does not knock the drain back to confirming"
		);
	}

	#[test]
	fn draining_exits_only_once_the_last_session_reports_down() {
		let mut app = tab_app();
		app.quit = Some(QuitPhase::Draining {
			pending: vec![7, 9],
			since: std::time::Instant::now(),
		});
		// The first of two down: still waiting, so no exit yet.
		assert!(app.note_drained(7).is_none());
		match &app.quit {
			Some(QuitPhase::Draining { pending, .. }) => assert_eq!(pending, &[9]),
			_ => panic!("still draining"),
		}
		// The last down: now the process may exit.
		assert!(app.note_drained(9).is_some(), "all sessions down → exit");
	}

	#[test]
	fn draining_ignores_a_session_it_is_not_waiting_on() {
		let mut app = tab_app();
		app.quit = Some(QuitPhase::Draining {
			pending: vec![7],
			since: std::time::Instant::now(),
		});
		// An unrelated tab's id must not empty the wait list.
		assert!(app.note_drained(3).is_none());
		match &app.quit {
			Some(QuitPhase::Draining { pending, .. }) => assert_eq!(pending, &[7]),
			_ => panic!("still draining"),
		}
	}

	#[test]
	fn noting_a_drain_outside_the_quit_flow_does_nothing() {
		let mut app = tab_app();
		// Not draining (nor even quitting): a stray Disconnected is just ignored (§30).
		assert!(app.note_drained(1).is_none());
		assert!(app.quit.is_none());
	}

	#[test]
	fn confirming_quit_with_no_live_session_exits_without_draining() {
		let mut app = tab_app();
		let _ = app.request_quit();
		// The lone tab is a home tab, not a live shell, so there is nothing to disconnect: the
		// confirm returns the exit task straight away rather than entering the drain (§30).
		let _ = app.quit_confirmed();
		assert!(
			!matches!(app.quit, Some(QuitPhase::Draining { .. })),
			"no live session means no drain phase"
		);
	}

	/// Opening an overlay seeds a FRESH card, so a spot dragged into during a previous one never
	/// carries across (§26, §30). Where "fresh" puts it, and that it is at rest, is `Card`'s own
	/// business and is tested at its interface — what this asserts is that the quit flow asks for
	/// one, measured against the OS WINDOW rather than any region (§48).
	#[test]
	fn the_overlay_card_opens_fresh_and_measured_against_the_window() {
		let mut app = tab_app();
		// A tab's own box is a different size, and must not be what an overlay is centred in.
		strip_mut(&mut app).tabs[0].window_size = iced::Size::new(400.0, 300.0);
		let _ = app.request_quit();
		assert_eq!(app.overlay, ui::dialog::Card::opened(app.window));
	}

	/// The header-drag messages a dialog emits are steered to the App while an overlay is up (§26),
	/// and reach the App's own card. The anchor-then-delta arithmetic itself belongs to `Card` and
	/// is tested there; this is about the wiring reaching it.
	#[test]
	fn dragging_the_overlay_card_follows_the_pointer() {
		let mut app = tab_app();
		let _ = app.request_quit();
		let start = app.overlay.pos();
		let _ = app.update(Message::DialogGrabbed);
		assert!(app.overlay.is_dragging());
		let _ = app.update(Message::DialogDragged(iced::Point::new(500.0, 500.0)));
		let _ = app.update(Message::DialogDragged(iced::Point::new(520.0, 540.0)));
		assert_eq!(
			app.overlay.pos(),
			iced::Point::new(start.x + 20.0, start.y + 40.0),
			"the moves land on the App's card"
		);
		let _ = app.update(Message::DialogReleased);
		assert!(!app.overlay.is_dragging(), "releasing ends the drag");
	}

	/// Every grabbable surface drives the ONE hand (§51). A dialog header is not a tab chip and
	/// knows nothing about one, but the pointer does the same thing on both — so both report the
	/// same two events and the cursor state is told what the POINTER is doing, never which widget
	/// did it. The chip half of this is the same sequence with `TabSelected` / `TabDropped`.
	#[test]
	fn a_dialog_header_and_a_chip_drive_the_same_hand() {
		let _held = crate::cursor::TEST_LOCK
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		crate::cursor::forget();
		let mut app = tab_app();

		// The header, with no overlay up, so the tab's own dialog drives it.
		let _ = app.update(Message::GrabEntered(crate::cursor::HEADER));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);
		let _ = app.update(Message::DialogGrabbed);
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Closed, "held");
		let _ = app.update(Message::DialogReleased);
		assert_eq!(
			crate::cursor::hand(),
			crate::cursor::Hand::Open,
			"let go, and the pointer is still on the header"
		);
		let _ = app.update(Message::GrabExited(crate::cursor::HEADER));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::None);

		// A chip, through its own messages, arriving at the same three states. It names itself by
		// its tab's id (§52), which is what lets a vanished chip be told from a live one.
		let chip = strip(&app)[0].id;
		let _ = app.update(Message::GrabEntered(chip));
		let _ = app.update(Message::TabSelected(0));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Closed);
		let _ = app.update(Message::TabDropped);
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);
		// Off the strip: the claim is dropped whoever holds it, so a chip that closed under the
		// pointer cannot leave a hand behind.
		let _ = app.update(Message::TabDragCancelled);
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::None);
		crate::cursor::forget();
	}

	/// The scrollbar is the third grabbable surface and drives the same one hand (§119, §51): open
	/// over the bar, closed for the whole drag, open again on release, gone when the pointer leaves.
	///
	/// The frame assertion in the middle is the one that matters, and it is why the `drawn` call lives
	/// in `ui::terminal`'s view rather than in the grid's paint. `frame_begin` / `frame_end` bracket
	/// `App::view` — the tree being BUILT — while `Widget::draw` runs later, during rendering. A
	/// re-assertion from the paint lands after the frame it belongs to has been judged, so `frame_end`
	/// finds nothing seen and revokes the claim: the hand would appear on the enter and flicker off on
	/// the very next frame. This test fails on exactly that.
	#[test]
	fn the_scroll_handle_wears_the_same_hand_as_a_chip_and_a_header() {
		let _held = crate::cursor::TEST_LOCK
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		crate::cursor::forget();

		// A tab with a live terminal that has history, so a bar is drawn at all.
		let (mut session, _rx) = app_with_terminal(16);
		with_history(&mut session);
		let mut app = tab_app();
		let id = session.id;
		let region = strip_mut(&mut app);
		region.tabs.clear();
		region.tabs.push(session);
		region.active = 0;
		app.next_id = id + 1;

		let _ = app.update(Message::GrabEntered(crate::cursor::SCROLLBAR));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);

		// A frame with the bar still in it: it re-asserts, so it keeps the hand.
		let _ = app.view();
		assert_eq!(
			crate::cursor::hand(),
			crate::cursor::Hand::Open,
			"the bar said it is still on screen, so the hand survives the frame"
		);

		let _ = app.update(Message::ScrollbarGrabbed);
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Closed, "held");
		// And it stays closed across a frame — a drag outlives the frame it started on.
		let _ = app.view();
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Closed);

		let _ = app.update(Message::ScrollbarReleased);
		assert_eq!(
			crate::cursor::hand(),
			crate::cursor::Hand::Open,
			"let go, and the pointer is still on the bar"
		);
		let _ = app.update(Message::GrabExited(crate::cursor::SCROLLBAR));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::None);
		crate::cursor::forget();
	}

	/// A bar that stops being drawn lets go of the hand, the same rule a vanishing chip obeys (§52,
	/// §119) — and the bar really can vanish under the pointer: a program taking the alternate screen
	/// keeps no history, so `vim` starting is a bar disappearing. Nothing raises an exit for chrome
	/// that merely stopped being painted, so the frame is the only thing that knows.
	#[test]
	fn a_scroll_handle_that_vanishes_under_the_pointer_lets_go_of_the_hand() {
		let _held = crate::cursor::TEST_LOCK
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		crate::cursor::forget();

		let (mut session, _rx) = app_with_terminal(16);
		with_history(&mut session);
		let mut app = tab_app();
		let id = session.id;
		let region = strip_mut(&mut app);
		region.tabs.clear();
		region.tabs.push(session);
		region.active = 0;
		app.next_id = id + 1;

		let _ = app.update(Message::GrabEntered(crate::cursor::SCROLLBAR));
		let _ = app.view();
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);

		// The alternate screen: no history over there, so no bar (§23).
		strip_mut(&mut app)
			.active_mut()
			.terminal
			.as_mut()
			.expect("the session's terminal")
			.process(b"\x1b[?1049h");
		let _ = app.view();
		assert_eq!(
			crate::cursor::hand(),
			crate::cursor::Hand::None,
			"no history means no bar, so it cannot still be under the pointer"
		);
		crate::cursor::forget();
	}

	/// A handle that goes away under the pointer lets go of the hand (§52). iced publishes a
	/// widget's `on_exit` from the widget itself, so a chip that closed — or was sent to another
	/// region — never says it lost the pointer, and before this the window went on wearing an open
	/// hand over everything.
	#[test]
	fn a_chip_that_vanishes_under_the_pointer_lets_go_of_the_hand() {
		let _held = crate::cursor::TEST_LOCK
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		crate::cursor::forget();
		let mut app = tab_app();
		let pane = app.focus;
		let _ = app.update(Message::In(pane, Box::new(Message::TabNew)));
		let chip = strip(&app)[0].id;

		let _ = app.update(Message::GrabEntered(chip));
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);
		// A frame with the chip still in it: it redraws, so it keeps the hand.
		let _ = app.view();
		assert_eq!(crate::cursor::hand(), crate::cursor::Hand::Open);

		// Closed under the pointer — an idle home tab goes at once, with no confirmation (§26).
		let _ = app.update(Message::In(
			pane,
			Box::new(Message::TabCloseRequested(chip)),
		));
		let _ = app.view();
		assert_eq!(
			crate::cursor::hand(),
			crate::cursor::Hand::None,
			"the chip is not on screen any more, so it cannot still be under the pointer"
		);
		crate::cursor::forget();
	}

	#[test]
	fn a_header_drag_goes_to_the_tab_when_no_overlay_is_up() {
		let mut app = tab_app();
		// With no overlay floating, the same DialogGrabbed drives the ACTIVE TAB's own dialog (§10),
		// not the App-level card — the guard keeps the two drag states from crossing wires.
		let _ = app.update(Message::DialogGrabbed);
		assert!(
			!app.overlay.is_dragging(),
			"the App-level card is untouched"
		);
		assert!(
			strip(&app)[0].card.is_dragging(),
			"the tab drives its own dialog"
		);
	}

	/// A resize reflows the open overlay against the NEW window (§26), so a card flung to the far
	/// corner is not left stranded off-screen. The clamp is `Card`'s; that the resize asks for it,
	/// and asks with the window's new size, is the App's.
	#[test]
	fn shrinking_the_window_pulls_a_dragged_overlay_back_into_reach() {
		let mut app = tab_app();
		let _ = app.request_quit();
		app.overlay.grab();
		let _ = app.update(Message::DialogDragged(iced::Point::new(900.0, 900.0)));
		let _ = app.update(Message::DialogDragged(iced::Point::new(4000.0, 4000.0)));
		let shrunk = iced::Size::new(500.0, 400.0);
		let _ = app.update(Message::WindowResized(shrunk));

		// Where the card actually is, not where a bound says it may be (§107). This used to build its
		// own `expected` by calling `reflow` — the function under test — on the value already
		// produced, and then compare the two: an assertion that `reflow` agrees with itself. With
		// the two bounds below it, the test could not tell a correct pull-back from any other
		// position inside the window.
		//
		// 500 - 460 = 40 and 400 - 44 = 356, the same two numbers `dialog`'s own shrink test pins,
		// which is the point: the App is expected to hand the card the SAME clamp, not its own.
		assert_px!(app.overlay.pos().x, 40.0);
		assert_px!(app.overlay.pos().y, 356.0);
	}

	#[test]
	fn closing_the_active_tab_falls_back_to_the_previously_visited_one() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus); // 2 tabs, active = 1
		let _ = app.open_tab(app.focus); // 3 tabs, active = 2
		let last_visited = strip(&app)[2].id;
		// Go back to the leftmost tab, so the trail is now 1, 2, 0 — the visit order and the strip
		// order disagree, which is the case strip arithmetic gets wrong (§37).
		let _ = app.select_tab(app.focus, 0);
		let closing = strip(&app)[0].id;
		let _ = app.request_close(closing);
		assert_eq!(strip(&app).len(), 2);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			last_visited,
			"the tab the user was on before, not the strip neighbour"
		);
	}

	#[test]
	fn a_closed_tab_is_dropped_from_the_visit_order() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus); // 2 tabs, active = 1
		let _ = app.open_tab(app.focus); // 3 tabs, active = 2 — trail 0, 1, 2
		let first_id = strip(&app)[0].id;
		let middle_id = strip(&app)[1].id;
		let active_id = strip(&app)[2].id;
		// Close the middle tab from its own "×" while it is in the background: the window must not
		// move, and that tab must be gone from the trail for good.
		let _ = app.request_close(middle_id);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			active_id,
			"the screen is unmoved"
		);
		// Now close the active tab. The fallback skips the closed middle tab to the one before it.
		let _ = app.request_close(active_id);
		assert_eq!(strip(&app).len(), 1);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			first_id,
			"a closed tab never comes forward"
		);
	}

	#[test]
	fn revisiting_a_tab_re_dates_it_rather_than_queueing_it_twice() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus); // trail 0, 1
		let _ = app.open_tab(app.focus); // trail 0, 1, 2
		let first_id = strip(&app)[0].id;
		let second_id = strip(&app)[1].id;
		let third_id = strip(&app)[2].id;
		// Bounce back to the first tab, then away again: trail 1, 2, 0 then 1, 0, 2. The stale entry
		// for tab 0 must not still be waiting further down, or the fallbacks come out in the wrong
		// order (§37).
		let _ = app.select_tab(app.focus, 0);
		let _ = app.select_tab(app.focus, 2);
		let _ = app.request_close(third_id);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			first_id,
			"the most recent visit"
		);
		let _ = app.request_close(first_id);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			second_id,
			"then the one before it"
		);
	}

	#[test]
	fn the_tab_brought_forward_by_a_close_is_measured_and_inherits_the_window_focus() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		// The tab a close brings forward must not paint against a stale geometry (§26, §37). Since
		// §48 the SIZE comes from re-measuring the region rather than from the outgoing tab, which is
		// strictly better: the outgoing tab's copy could itself be stale, because a background tab
		// misses a window resize and a divider drag alike. The window FOCUS is still carried, since
		// nothing measures that.
		let stale = iced::Size::new(1234.0, 567.0);
		let region = strip_mut(&mut app);
		let showing = region.active;
		region.tabs[showing].window_size = stale;
		region.tabs[showing].window_focused = false;
		let closing = strip(&app)[on_screen(&app)].id;
		let _ = app.request_close(closing);
		// The one region fills the whole window, less the strip above the tab.
		let expected = iced::Size::new(1200.0, 800.0 - ui::tabs::STRIP_HEIGHT);
		assert_eq!(strip(&app)[on_screen(&app)].window_size, expected);
		assert!(!strip(&app)[on_screen(&app)].window_focused);
	}

	// The path of an editor tab's file, for asserting where in the strip it landed (§38).
	fn editor_path(app: &App, index: usize) -> String {
		strip(app)[index]
			.editor()
			.expect("an editor tab")
			.path
			.clone()
	}

	#[test]
	fn an_editor_tab_opens_beside_the_session_it_came_from() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let _ = app.open_tab(app.focus); // 3 tabs; the file is opened from the LEFTMOST one
		let session = strip(&app)[0].id;
		let _ = app.open_viewer(app.focus, session, "/home/user/notes.txt".to_owned());
		assert_eq!(strip(&app).len(), 4);
		assert_eq!(
			on_screen(&app),
			1,
			"right after its session, not at the far end"
		);
		assert_eq!(editor_path(&app, 1), "/home/user/notes.txt");
	}

	#[test]
	fn files_opened_from_one_session_stay_grouped_in_the_order_they_were_opened() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let session = strip(&app)[0].id;
		let _ = app.open_viewer(app.focus, session, "first.txt".to_owned());
		let _ = app.open_viewer(app.focus, session, "second.txt".to_owned());
		// The second file goes after the first, not between it and its session — so the group reads
		// left to right in the order the files were opened (§38).
		assert_eq!(editor_path(&app, 1), "first.txt");
		assert_eq!(editor_path(&app, 2), "second.txt");
		assert_eq!(on_screen(&app), 2);
	}

	#[test]
	fn each_session_keeps_its_own_group_of_files() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let first_session = strip(&app)[0].id;
		let second_session = strip(&app)[1].id;
		let _ = app.open_viewer(app.focus, first_session, "one.txt".to_owned());
		// The second session's file goes beside IT, so the run of editors after the first session
		// ends at the first chip that is not one of its own (§38).
		let _ = app.open_viewer(app.focus, second_session, "two.txt".to_owned());
		assert_eq!(editor_path(&app, 1), "one.txt");
		assert_eq!(strip(&app)[2].id, second_session);
		assert_eq!(editor_path(&app, 3), "two.txt");
	}

	#[test]
	fn a_file_whose_session_has_gone_opens_at_the_end() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		// The session tab closed while the load was in flight: there is nothing to sit beside, so the
		// editor takes the end of the strip rather than a guessed slot (§38).
		let _ = app.open_viewer(app.focus, 9_999, "orphan.txt".to_owned());
		assert_eq!(on_screen(&app), strip(&app).len() - 1);
		assert_eq!(editor_path(&app, on_screen(&app)), "orphan.txt");
	}

	/// A real PNG of the given size, so the preview tests run over bytes a decoder actually produced
	/// rather than a hand-forged header (§53).
	fn png(width: u32, height: u32) -> Vec<u8> {
		let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([9, 9, 9, 255]));
		let mut bytes = Vec::new();
		image::DynamicImage::ImageRgba8(picture)
			.write_to(
				&mut std::io::Cursor::new(&mut bytes),
				image::ImageFormat::Png,
			)
			.expect("the test's own encoder writes");
		bytes
	}

	/// Open `path` from `session` and hand back the id of the viewer tab it made (§53). The id is
	/// taken BEFORE the call — it is the one `open_viewer` is about to hand out — because the new
	/// tab is slotted beside its session (§38) rather than appended, so neither end of the strip is
	/// reliably the one just opened.
	fn open_file(app: &mut App, session: u64, path: &str) -> u64 {
		let id = app.next_id;
		let _task = app.open_viewer(app.focus, session, path.to_owned());
		id
	}

	/// An app whose one tab is a LIVE session with a command channel, and that channel's receiving
	/// end (§53). The preview tests need it: a tab with no channel fails every load the instant it
	/// is opened, so a preview would never be seen in the `Loading` state that half of these are
	/// about.
	fn app_with_session() -> (App, u64, mpsc::Receiver<SshCommand>) {
		let (session, mut rx) = app_with_login_identity();
		let mut app = tab_app();
		let id = session.id;
		let region = strip_mut(&mut app);
		region.tabs.clear();
		region.tabs.push(session);
		region.active = 0;
		app.next_id = id + 1;
		// Whatever the session sent on its way up is not this test's business.
		let _drained = drain(&mut rx);
		(app, id, rx)
	}

	/// Deliver a picture's bytes to its tab the way the runtime does since §121: the read's reply,
	/// then the decode's.
	///
	/// The decode is a `Task` now, so `FileLoaded` alone leaves the tab mid-wait — these two steps
	/// are what a test has to do to stand in for the thread pool. Doing the decode HERE, with the
	/// real `decode_image`, is what keeps the tests about the model's response rather than about a
	/// stubbed decoder.
	fn deliver_picture(app: &mut App, session: u64, id: u64, path: &str, bytes: Vec<u8>) {
		let size = bytes.len() as u64;
		let decoded = crate::preview::decode_image(&bytes);
		let _read = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: path.to_owned(),
				bytes,
			},
		);
		let _decode = app.update_in(
			app.focus,
			Message::PictureDecoded {
				viewer_id: id,
				bytes: size,
				decoded,
			},
		);
	}

	/// The picture tab with this id (§53).
	fn preview_of(app: &App, id: u64) -> &crate::preview::Preview {
		match app.tabs().find(|tab| tab.id == id).map(|tab| &tab.viewer) {
			Some(Some(Viewer::Picture(picture))) => picture,
			_ => panic!("a preview tab"),
		}
	}

	/// A progress event for a viewer reaches that viewer's status AND its chip's bar (§121) — the
	/// whole point of the event, since the tab strip is where a background load is visible at all.
	#[test]
	fn a_reads_progress_reaches_the_tab_strips_bar() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/notes.txt");

		// Nothing reported yet: the size is unknown, so the bar pulses rather than sitting at 0%.
		let tab = app.tabs().find(|tab| tab.id == id).expect("the viewer");
		assert_eq!(
			tab.command_progress(),
			term::progress::Progress::Indeterminate,
			"a read that has not reported has no share to show"
		);

		let _task = app.route_ssh(
			session,
			SshEvent::FileLoadProgress {
				viewer_id: id,
				read: 256,
				total: Some(1024),
			},
		);

		let tab = app.tabs().find(|tab| tab.id == id).expect("the viewer");
		assert_eq!(
			tab.command_progress(),
			term::progress::Progress::Working(25),
			"a quarter read is a quarter of a bar"
		);
	}

	/// A viewer's bar goes away when its file is open (§121). Without this the chip would keep a
	/// full bar for the whole life of the tab, saying "busy" about a file that is merely being read.
	#[test]
	fn the_bar_leaves_when_the_file_has_arrived() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/notes.txt");

		let _task = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: "/srv/notes.txt".to_owned(),
				bytes: b"alpha\nbeta\n".to_vec(),
			},
		);

		let tab = app.tabs().find(|tab| tab.id == id).expect("the viewer");
		assert_eq!(
			tab.command_progress(),
			term::progress::Progress::None,
			"an open file is not work in flight"
		);
	}

	/// A progress event can arrive BEHIND the `FileLoaded` it belongs to — the reader sends one per
	/// chunk and they queue — so it must not be able to drag a loaded editor back into `Loading` and
	/// blank the buffer that just arrived (§121).
	#[test]
	fn a_late_progress_event_cannot_unload_a_file_that_arrived() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/notes.txt");

		let _loaded = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: "/srv/notes.txt".to_owned(),
				bytes: b"alpha\nbeta\n".to_vec(),
			},
		);
		// The straggler, naming a read that is over.
		let _late = app.route_ssh(
			session,
			SshEvent::FileLoadProgress {
				viewer_id: id,
				read: 4,
				total: Some(11),
			},
		);

		let tab = app.tabs().find(|tab| tab.id == id).expect("the viewer");
		let editor = tab.editor().expect("an editor");
		assert!(
			matches!(editor.status, crate::editor::EditorStatus::Ready),
			"the file is open; a stale chunk count does not reopen the wait"
		);
		assert_eq!(editor.content.text(), "alpha\nbeta\n");
	}

	/// Closing a viewer that is still reading tells the session to stop reading (§121). Otherwise the
	/// server keeps sending a file to a tab that no longer exists.
	#[test]
	fn closing_a_viewer_mid_read_cancels_the_read() {
		let (mut app, session, mut rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/big.log");
		let _sent = drain(&mut rx);

		let _task = app.force_close(id);

		let cancels: Vec<u64> = drain(&mut rx)
			.into_iter()
			.filter_map(|command| match command {
				SshCommand::CancelFileLoad { viewer_id } => Some(viewer_id),
				_ => None,
			})
			.collect();
		assert_eq!(
			cancels,
			vec![id],
			"the closed tab's own read, named by its id"
		);
	}

	/// Closing a viewer whose file is already open cancels nothing (§121) — there is no read to stop,
	/// and a cancel naming a finished read would be noise on the channel.
	#[test]
	fn closing_a_loaded_viewer_cancels_nothing() {
		let (mut app, session, mut rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/notes.txt");
		let _loaded = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: "/srv/notes.txt".to_owned(),
				bytes: b"alpha\n".to_vec(),
			},
		);
		let _sent = drain(&mut rx);

		let _task = app.force_close(id);

		assert!(
			!drain(&mut rx)
				.iter()
				.any(|command| matches!(command, SshCommand::CancelFileLoad { .. })),
			"nothing is in flight, so nothing is cancelled"
		);
	}

	/// The double-click that used to hand a `.png` to a text editor now opens a picture instead
	/// (§53) — and the file that always belonged in the editor still gets there.
	#[test]
	fn a_picture_opens_a_preview_and_everything_else_opens_the_editor() {
		let (mut app, session, _rx) = app_with_session();

		let picture = open_file(&mut app, session, "/srv/shot.png");
		let tab = app.tabs().find(|tab| tab.id == picture).expect("the tab");
		// "And never a buffer as well" used to need its own assertion, because the two kinds were
		// two `Option` fields and nothing stopped both being `Some`. One enum makes that
		// unrepresentable, so matching the variant IS the whole claim.
		assert!(
			matches!(tab.viewer, Some(Viewer::Picture(_))),
			"a picture opens a picture"
		);
		assert!(matches!(tab.screen, AppScreen::Viewer));

		let notes = open_file(&mut app, session, "/srv/notes.txt");
		let tab = app.tabs().find(|tab| tab.id == notes).expect("the tab");
		assert!(
			matches!(tab.viewer, Some(Viewer::Editor(_))),
			"text still opens the editor"
		);
	}

	/// What the two viewer kinds SHARE, asked of the enum rather than forked on at the call site
	/// (§32, §53). No window and no session: these are the accessors most of the old fork sites
	/// actually wanted, and they are the reason the fork disappeared rather than moved.
	#[test]
	fn both_viewer_kinds_answer_for_their_parent_and_their_path() {
		let editor = Viewer::Editor(crate::editor::Editor::loading(
			7,
			bridge::LOGIN_IDENTITY,
			"/srv/notes.txt".to_owned(),
			crate::editor::EditorTheme::default(),
		));
		let picture = Viewer::Picture(crate::preview::Preview::loading(
			7,
			"/srv/shot.png".to_owned(),
		));

		assert_eq!(editor.session(), 7);
		assert_eq!(picture.session(), 7, "a picture is parented the same way");
		assert_eq!(editor.path(), "/srv/notes.txt");
		assert_eq!(picture.path(), "/srv/shot.png");
	}

	/// The chip's label, which used to be two `AppScreen` arms each unwrapping its own field (§32,
	/// §53).
	#[test]
	fn only_an_editor_can_wear_the_unsaved_dot() {
		let clean = Viewer::Editor(crate::editor::Editor::loading(
			7,
			bridge::LOGIN_IDENTITY,
			"/srv/notes.txt".to_owned(),
			crate::editor::EditorTheme::default(),
		));
		let picture = Viewer::Picture(crate::preview::Preview::loading(
			7,
			"/srv/shot.png".to_owned(),
		));

		// Both are named by the file's own name, not by its path.
		assert_eq!(clean.label(), "notes.txt");
		// A picture has nothing to save, so it can never be dirty and never wears the dot. That
		// used to be enforced by writing the picture's arm without one; now it is enforced by
		// there being no `is_dirty` to reach on this variant.
		assert_eq!(picture.label(), "shot.png");
	}

	/// An SVG is a picture by icon and text by nature, and the editor can genuinely edit it (§53).
	#[test]
	fn an_svg_still_opens_in_the_editor() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/logo.svg");
		assert!(
			app.tabs()
				.find(|tab| tab.id == id)
				.is_some_and(|tab| tab.editor().is_some())
		);
	}

	/// A preview costs no network thread, exactly as an editor does not (§32, §53).
	#[test]
	fn a_preview_tab_starts_no_worker_of_its_own() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		let tab = app.tabs().find(|tab| tab.id == id).expect("the tab");
		assert!(tab.is_viewer(), "so the subscription list skips it");
		assert!(tab.command_tx.is_none(), "and it holds no channel");
	}

	/// The bytes arrive and become a picture, described by what they turned out to BE (§53).
	#[test]
	fn a_picture_that_arrives_is_shown_with_what_it_turned_out_to_be() {
		let (mut app, session, _rx) = app_with_session();
		// Named `.jpg` and carrying PNG bytes: the tab is chosen by the name, the decoder by the
		// bytes, so it opens anyway and reports the truth (§53).
		let id = open_file(&mut app, session, "/srv/mislabelled.jpg");
		let bytes = png(5, 3);
		let size = bytes.len() as u64;
		deliver_picture(&mut app, session, id, "/srv/mislabelled.jpg", bytes);

		let preview = preview_of(&app, id);
		assert_eq!(preview.status, crate::preview::PreviewStatus::Ready);
		let picture = preview.picture.as_ref().expect("a decoded picture");
		assert_eq!((picture.width, picture.height), (5, 3));
		assert_eq!(picture.format, "PNG", "the bytes, not the extension");
		assert_eq!(picture.bytes, size, "the FILE's size, not the pixels'");
	}

	/// The bytes arriving is no longer the end of the wait (§121): the decode runs on the thread pool,
	/// so the tab stays `Loading` — with its bar at the end, since everything HAS been read — until
	/// the decode reply comes back.
	#[test]
	fn a_pictures_bytes_do_not_become_a_picture_until_the_decode_returns() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		let bytes = png(4, 4);
		let size = bytes.len() as u64;

		// Only the READ's reply. The decode's is what the pool would send afterwards.
		let _read = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: "/srv/shot.png".to_owned(),
				bytes,
			},
		);

		let preview = preview_of(&app, id);
		assert_eq!(
			preview.status,
			crate::preview::PreviewStatus::Loading(crate::viewer::LoadProgress {
				read: size,
				total: Some(size),
			}),
			"read in full, still decoding"
		);
		assert!(
			preview.picture.is_none(),
			"nothing to draw until the decode lands"
		);
		// And the chip says so: a full bar, not an absent one.
		let tab = app.tabs().find(|tab| tab.id == id).expect("the viewer");
		assert_eq!(
			tab.command_progress(),
			term::progress::Progress::Working(100)
		);
	}

	/// A decode can come back after its tab has gone — it ran on a pool thread while the user closed
	/// the tab (§121). The reply is dropped, not treated as an error and not able to resurrect
	/// anything.
	#[test]
	fn a_decode_that_returns_to_a_closed_tab_is_dropped() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		let bytes = png(4, 4);
		let size = bytes.len() as u64;
		let decoded = crate::preview::decode_image(&bytes);
		let before = app.tabs().count();

		let _closed = app.force_close(id);
		let _late = app.update_in(
			app.focus,
			Message::PictureDecoded {
				viewer_id: id,
				bytes: size,
				decoded,
			},
		);

		assert_eq!(
			app.tabs().count(),
			before - 1,
			"the tab stayed closed and nothing was rebuilt for it"
		);
		assert!(
			!app.tabs().any(|tab| tab.id == id),
			"a decode cannot bring a closed viewer back"
		);
	}

	/// A decode returning to a tab that is still THERE but no longer waiting must not overwrite what
	/// happened while it ran (§121).
	///
	/// The case that makes this real: the parent session dies mid-decode, so the tab is already
	/// showing "the session closed before it finished loading" — and then the pool comes back with a
	/// perfectly good picture. Drawing it would put an image on screen for a session that is gone, and
	/// silently replace a message the user needs. This is the guard the closed-tab test does NOT
	/// cover, because a closed tab is caught earlier by having no viewer at all.
	#[test]
	fn a_decode_cannot_overwrite_the_failure_that_landed_while_it_ran() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		let bytes = png(4, 4);
		let size = bytes.len() as u64;
		let decoded = crate::preview::decode_image(&bytes);
		assert!(decoded.is_ok(), "the decode itself succeeds");

		// Read done, decode in flight on the pool.
		let _read = app.route_ssh(
			session,
			SshEvent::FileLoaded {
				viewer_id: id,
				path: "/srv/shot.png".to_owned(),
				bytes,
			},
		);
		// The session dies while the pool is busy.
		app.orphan_viewers(session);
		// And only now does the decode come back.
		let _late = app.update_in(
			app.focus,
			Message::PictureDecoded {
				viewer_id: id,
				bytes: size,
				decoded,
			},
		);

		let preview = preview_of(&app, id);
		assert!(
			matches!(preview.status, crate::preview::PreviewStatus::Failed(_)),
			"the session's death is what the tab still has to report, not a picture"
		);
		assert!(preview.picture.is_none(), "and there is nothing to draw");
	}

	/// A file that is not a picture cmote can draw says so where the picture would have been (§53).
	#[test]
	fn a_file_that_is_not_a_picture_shows_the_reason_in_place_of_the_image() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/broken.png");
		deliver_picture(
			&mut app,
			session,
			id,
			"/srv/broken.png",
			b"this is not a picture".to_vec(),
		);
		let preview = preview_of(&app, id);
		assert!(matches!(
			preview.status,
			crate::preview::PreviewStatus::Failed(_)
		));
		assert!(preview.picture.is_none());
	}

	/// A read that failed on the far side reports the SERVER's reason, not a generic one (§53).
	#[test]
	fn a_read_that_failed_shows_the_servers_own_reason() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/secret.png");
		if let Some(tab) = app.tab_mut(id) {
			let _task = tab.on_viewer_event(SshEvent::FileLoadFailed {
				viewer_id: id,
				reason: "permission denied".to_owned(),
			});
		}
		assert_eq!(
			preview_of(&app, id).status,
			crate::preview::PreviewStatus::Failed("permission denied".to_owned())
		);
	}

	/// A picture half-read is no picture: when the session carrying the read ends, a preview still
	/// waiting on it is failed rather than left on "Loading…" for the life of the tab (§53).
	#[test]
	fn a_preview_still_loading_when_its_session_ends_is_told_so() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		assert!(matches!(
			preview_of(&app, id).status,
			crate::preview::PreviewStatus::Loading(_)
		));

		app.orphan_viewers(session);
		let crate::preview::PreviewStatus::Failed(reason) = &preview_of(&app, id).status else {
			panic!("the load can never arrive now, so it is failed");
		};
		assert!(reason.contains("closed"), "and it says why: {reason}");
	}

	/// One that already HAS its picture keeps it: the image is decoded and in memory, and the
	/// session it came from has nothing more to give it (§53).
	#[test]
	fn a_preview_that_already_has_its_picture_outlives_its_session() {
		let (mut app, session, _rx) = app_with_session();
		let id = open_file(&mut app, session, "/srv/shot.png");
		deliver_picture(&mut app, session, id, "/srv/shot.png", png(2, 2));

		app.orphan_viewers(session);
		assert_eq!(
			preview_of(&app, id).status,
			crate::preview::PreviewStatus::Ready
		);
		assert!(preview_of(&app, id).picture.is_some(), "still on screen");
	}

	/// The size ceiling rides the read, and it is the one belonging to the viewer that asked (§53) —
	/// so a photograph is not refused by a limit chosen for config files.
	#[test]
	fn the_read_carries_the_ceiling_of_the_viewer_that_asked() {
		let (mut app, id, mut rx) = app_with_session();

		let _picture = open_file(&mut app, id, "/srv/holiday.jpg");
		let _text = open_file(&mut app, id, "/srv/notes.txt");
		let limits: Vec<u64> = drain(&mut rx)
			.into_iter()
			.filter_map(|command| match command {
				SshCommand::FileLoad { limit, .. } => Some(limit),
				_ => None,
			})
			.collect();
		assert_eq!(
			limits,
			vec![crate::preview::MAX_SIZE, crate::ssh::edit::MAX_SIZE],
			"the picture's ceiling, then the editor's"
		);
	}

	#[test]
	fn dragging_a_chip_onto_another_moves_it_into_that_slot() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let _ = app.open_tab(app.focus);
		let grabbed = strip(&app)[0].id;
		let passed = strip(&app)[1].id;
		// Press the leftmost chip, travel to the rightmost, release: the grabbed tab lands where that
		// chip was and the ones it passed shuffle left (§38).
		let _ = app.update(Message::TabSelected(0));
		let _ = app.update(Message::TabDraggedOver(2));
		let _ = app.update(Message::TabDropped);
		assert_eq!(strip(&app)[2].id, grabbed);
		assert_eq!(strip(&app)[0].id, passed, "the tabs it passed moved left");
		assert_eq!(
			on_screen(&app),
			2,
			"the tab on screen followed its own move"
		);
		assert!(app.region().tab_drag.is_none(), "the gesture is over");
	}

	#[test]
	fn a_press_that_never_leaves_its_chip_is_only_a_click() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let first = strip(&app)[0].id;
		// Press and release on the same chip: it selects, and the strip keeps its order — a drag with
		// no target drops nothing (§38).
		let _ = app.update(Message::TabSelected(0));
		let _ = app.update(Message::TabDropped);
		assert_eq!(strip(&app)[0].id, first);
		assert_eq!(on_screen(&app), 0, "it did still select");
	}

	#[test]
	fn leaving_the_strip_abandons_the_move() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let first = strip(&app)[0].id;
		let _ = app.update(Message::TabSelected(0));
		let _ = app.update(Message::TabDraggedOver(1));
		// The pointer wanders off the strip: the gesture is called off, so a release afterwards (over
		// the terminal, say) must not still reorder anything (§38).
		let _ = app.update(Message::TabDragCancelled);
		let _ = app.update(Message::TabDropped);
		assert_eq!(strip(&app)[0].id, first);
	}

	#[test]
	fn dragging_back_onto_the_grabbed_chip_clears_the_target() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let first = strip(&app)[0].id;
		// Out to the neighbour and back again: changing your mind mid-drag leaves the order alone.
		let _ = app.update(Message::TabSelected(0));
		let _ = app.update(Message::TabDraggedOver(1));
		let _ = app.update(Message::TabDraggedOver(0));
		let _ = app.update(Message::TabDropped);
		assert_eq!(strip(&app)[0].id, first);
	}

	#[test]
	fn hovering_the_strip_at_rest_moves_nothing() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let first = strip(&app)[0].id;
		// No press, so no drag: the pointer crossing the chips and a stray release are both inert.
		let _ = app.update(Message::TabDraggedOver(0));
		let _ = app.update(Message::TabDropped);
		assert_eq!(strip(&app)[0].id, first);
		assert!(app.region().tab_drag.is_none());
	}

	#[test]
	fn a_reorder_keeps_whatever_tab_is_on_screen_on_screen() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let _ = app.open_tab(app.focus);
		let showing = strip(&app)[on_screen(&app)].id;
		// A drag armed on a tab that is NOT the one on screen — a close confirmation can leave another
		// tab active. `active` is a strip position, so it has to follow its own tab's id (§38).
		let (grabbed, over) = (strip(&app)[0].id, strip(&app)[1].id);
		strip_mut(&mut app).tab_drag = Some(TabDrag {
			grabbed,
			over: Some(over),
		});
		strip_mut(&mut app).drop_tab();
		assert_eq!(strip(&app)[on_screen(&app)].id, showing);
	}

	#[test]
	fn a_reorder_leaves_the_visit_order_alone() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus);
		let _ = app.open_tab(app.focus); // trail 0, 1, 2 — the third tab is on screen
		let third = strip(&app)[2].id;
		// Grab the leftmost tab (which selects it, so the trail becomes 1, 2, 0) and drop it at the
		// end. The activation order is keyed by id, so shuffling positions must not disturb it (§37).
		let _ = app.update(Message::TabSelected(0));
		let _ = app.update(Message::TabDraggedOver(2));
		let _ = app.update(Message::TabDropped);
		let moved = strip(&app)[on_screen(&app)].id;
		let _ = app.request_close(moved);
		assert_eq!(
			strip(&app)[on_screen(&app)].id,
			third,
			"the fallback is still the tab visited before it, wherever the strip has put it"
		);
	}

	#[test]
	fn selecting_switches_the_active_tab_and_ignores_bad_indices() {
		let mut app = tab_app();
		let _ = app.open_tab(app.focus); // active = 1
		let _ = app.select_tab(app.focus, 0);
		assert_eq!(on_screen(&app), 0);
		// Out of range or the current tab is a no-op.
		let _ = app.select_tab(app.focus, 9);
		assert_eq!(on_screen(&app), 0);
	}

	// --- splitting the window (§48) ---

	/// Apply a split the way the UI does, but with the monitor step already answered — the real flow
	/// asks the OS how big the screen is and comes back a turn later, which no test can await.
	fn split(app: &mut App, way: ui::split::Way) -> iced::Size {
		let pane = app.focus;
		let grown = way.grown(app.window);
		let _ = app.apply_split(
			pane,
			way,
			iced::window::Id::unique(),
			grown,
			SplitSeed::Home,
		);
		grown
	}

	#[test]
	fn only_the_whole_window_offers_a_split() {
		let mut app = tab_app();
		assert!(
			app.splittable(),
			"undivided, and its one region is the original — the controls are on its strip"
		);
		let fresh = {
			let _ = split(&mut app, ui::split::Way::Horizontal);
			app.focus
		};
		assert!(
			!app.splittable(),
			"one cut is all there is: neither strip offers another"
		);
		// Give the room back and the offer returns — this is the only way to get a different split,
		// so it has to come back rather than be spent for the life of the window.
		let _ = app.close_region(fresh);
		assert_eq!(region_count(&app), 1);
		assert!(app.splittable());
	}

	#[test]
	fn the_split_region_may_split_again_once_the_original_region_goes() {
		let mut app = tab_app();
		let original = app.focus;
		let _ = split(&mut app, ui::split::Way::Vertical);
		// Close the region the split was made FROM. The one that was below inherits the whole window,
		// which makes it the top-left region — and the rule follows the shape, not the history.
		let _ = app.close_region(original);
		assert_eq!(region_count(&app), 1);
		assert_ne!(app.focus, original, "the keyboard moved to the survivor");
		assert!(app.splittable());
	}

	#[test]
	fn a_second_split_is_refused_even_when_it_was_already_in_flight() {
		let mut app = tab_app();
		// Two quick presses: both leave while the window is whole, because the monitor is measured
		// asynchronously. The second arrives after the first has landed and must do nothing at all —
		// not add a third region, and not double the window a second time.
		let grown = split(&mut app, ui::split::Way::Horizontal);
		let _ = split(&mut app, ui::split::Way::Vertical);
		assert_eq!(region_count(&app), 2, "still one cut");
		assert_eq!(app.window, grown, "and the window grew only once");
	}

	#[test]
	fn a_split_opens_a_second_region_on_the_target_list_and_gives_it_the_keyboard() {
		let mut app = tab_app();
		let first = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		assert_eq!(region_count(&app), 2, "the window is divided in two");
		assert_ne!(app.focus, first, "the fresh region takes the keyboard");
		// A fresh application layout: one tab, and it is sitting on the saved-target list.
		assert_eq!(strip(&app).len(), 1);
		assert!(matches!(strip(&app)[0].screen, AppScreen::Home));
		// Tab ids stay app-wide, so the new region's tab cannot collide with the old region's (§26).
		let ids: Vec<u64> = app.tabs().map(|tab| tab.id).collect();
		assert_eq!(ids.len(), 2);
		assert_ne!(ids[0], ids[1]);
	}

	#[test]
	fn a_split_doubles_the_window_the_way_it_cuts_and_the_old_region_keeps_its_size() {
		let mut app = tab_app();
		let before = app.window;
		let old = app.focus;
		let grown = split(&mut app, ui::split::Way::Horizontal);
		assert_px!(grown.width, before.width * 2.0, "twice as wide");
		assert_px!(grown.height, before.height, "no taller");
		assert_eq!(
			app.window, grown,
			"the app measures against the grown window"
		);
		// The point of growing rather than halving: the region that was already there is left the
		// size it was, so the shell in it does not reflow. Half a seam is lost to the divider.
		let boxes = ui::split::regions(&app.regions, app.window);
		let kept = boxes[&old];
		assert!(
			(kept.width - (before.width - ui::split::SPACING / 2.0)).abs() < 1.0,
			"the split region kept its width, less its half of the seam: {}",
			kept.width
		);
	}

	#[test]
	fn a_vertical_split_grows_the_window_downwards_instead() {
		let mut app = tab_app();
		let before = app.window;
		let grown = split(&mut app, ui::split::Way::Vertical);
		assert_px!(grown.width, before.width);
		assert_px!(grown.height, before.height * 2.0);
	}

	#[test]
	fn every_region_is_measured_against_its_own_box_not_the_whole_window() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		// Each region's on-screen tab is handed its own box less the strip above it, so the terminal
		// in there picks a column count for the half it occupies rather than for the whole window.
		let boxes = ui::split::regions(&app.regions, app.window);
		for (pane, rect) in &boxes {
			let region = app.regions.get(*pane).expect("a live region");
			assert_px!(region.active().window_size.width, rect.width);
			assert_px!(
				region.active().window_size.height,
				rect.height - ui::tabs::STRIP_HEIGHT
			);
		}
		// And they really are halves, not two copies of the window.
		let widths: Vec<f32> = boxes.values().map(|rect| rect.width).collect();
		assert!(widths.iter().all(|width| *width < app.window.width));
	}

	#[test]
	fn a_region_event_is_applied_where_it_happened_not_where_the_keyboard_is() {
		let mut app = tab_app();
		let old = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		assert_ne!(old, fresh);
		// A "+" pressed on the OTHER region's strip arrives wrapped in that region's name. Routed by
		// focus it would open a tab in the region holding the keyboard — which is the bug the wrapper
		// exists to prevent (§48).
		let _ = app.update(Message::In(old, Box::new(Message::TabNew)));
		assert_eq!(
			app.regions.get(old).expect("the split region").tabs.len(),
			2,
			"the tab opened in the region the press came from"
		);
	}

	#[test]
	fn a_divider_drag_re_shares_the_room_and_re_measures_both_regions() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let (split_id, _) = *app
			.regions
			.layout()
			.split_regions(ui::split::SPACING, ui::split::MIN_SIZE, app.window)
			.iter()
			.next()
			.map(|(id, value)| (*id, *value))
			.as_ref()
			.expect("one divider");
		let _ = app.update(Message::SplitResized {
			split: split_id,
			ratio: 0.25,
		});
		let boxes = ui::split::regions(&app.regions, app.window);
		let mut widths: Vec<f32> = boxes.values().map(|rect| rect.width).collect();
		widths.sort_by(f32::total_cmp);
		assert!(
			widths[0] < widths[1],
			"the drag left one region narrower than the other: {widths:?}"
		);
		// Every region's tab agrees with its new box — the drag is not finished until the grids know.
		for (pane, rect) in &boxes {
			let region = app.regions.get(*pane).expect("a live region");
			assert_px!(region.active().window_size.width, rect.width);
		}
	}

	#[test]
	fn closing_a_regions_last_tab_closes_the_region_and_hands_back_its_room() {
		let mut app = tab_app();
		let kept = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		// What the survivor is showing right now, so the close can be held to leaving it alone.
		let before = app
			.regions
			.get(kept)
			.expect("the region that was split")
			.active()
			.window_size;
		let only_tab = strip(&app)[0].id;
		let _ = app.request_close(only_tab);
		assert_eq!(region_count(&app), 1, "the region went with its last tab");
		assert!(
			app.regions.get(fresh).is_none(),
			"and it is the region that was closed"
		);
		assert_eq!(app.focused_pane(), kept, "the keyboard followed");
		// The window gave the space back, so the survivor spans it exactly as it did before — the
		// mirror of the grow, and the reason nothing in it reflows either way (§48).
		assert_px!(app.window.height, 800.0, "the other axis is untouched");
		assert!(
			(app.window.width - (1200.0 - ui::split::SPACING / 2.0)).abs() < 1.0,
			"back to its pre-split width, less the half seam the split cost it: {}",
			app.window.width
		);
		let boxes = ui::split::regions(&app.regions, app.window);
		assert_px!(boxes[&kept].width, app.window.width, "and it has all of it");
		assert_eq!(
			app.regions
				.get(kept)
				.expect("the survivor")
				.active()
				.window_size,
			before,
			"measured to the same box it already had, so its grid never reflowed"
		);
		// And the size the next run opens at is the shrunk one, not the split's.
		assert_eq!(
			app.settings.window_size().map(|size| size.width),
			Some(app.window.width)
		);
	}

	#[test]
	fn closing_a_stacked_region_gives_the_height_back_not_the_width() {
		let mut app = tab_app();
		let kept = app.focus;
		let _ = split(&mut app, ui::split::Way::Vertical);
		let only_tab = strip(&app)[0].id;
		let _ = app.request_close(only_tab);
		assert_px!(app.window.width, 1200.0, "the other axis is untouched");
		assert!(
			(app.window.height - (800.0 - ui::split::SPACING / 2.0)).abs() < 1.0,
			"back to its pre-split height: {}",
			app.window.height
		);
		assert_eq!(app.focused_pane(), kept);
	}

	#[test]
	fn the_shrink_stops_at_the_smallest_window_that_can_be_remembered() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		// The user narrows the split window by hand afterwards, leaving each region a sliver. The
		// close must not hand back so much room that the window ends up smaller than the smallest one
		// cmote will reopen — a size the settings file refuses to remember is one that jumps back on
		// the next run (§31).
		let _ = app.update(Message::WindowResized(iced::Size::new(700.0, 600.0)));
		let only_tab = strip(&app)[0].id;
		let _ = app.request_close(only_tab);
		assert_eq!(region_count(&app), 1);
		assert_px!(app.window.width, crate::settings::MIN_WINDOW);
		assert_px!(app.window.height, 600.0, "the untouched axis is not raised");
	}

	#[test]
	fn closing_the_last_tab_of_the_last_region_is_a_quit_not_an_empty_window() {
		let mut app = tab_app();
		let only_tab = strip(&app)[0].id;
		let _ = app.request_close(only_tab);
		assert!(
			matches!(app.quit, Some(QuitPhase::Confirming)),
			"an undivided window with one tab left raises the quit confirmation (§30)"
		);
		assert_eq!(region_count(&app), 1);
		assert_eq!(strip(&app).len(), 1, "and nothing was closed yet");
	}

	#[test]
	fn a_split_of_a_region_that_has_already_gone_changes_nothing() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		let only_tab = strip(&app)[0].id;
		let _ = app.request_close(only_tab); // the region closes with it
		let before = (region_count(&app), app.window, app.next_id);
		// The monitor answer comes back naming a region that no longer exists. Nothing must happen —
		// above all the window must not grow for a split that cannot be made (§48).
		let _ = app.apply_split(
			fresh,
			ui::split::Way::Horizontal,
			iced::window::Id::unique(),
			iced::Size::new(9999.0, 9999.0),
			SplitSeed::Home,
		);
		assert_eq!((region_count(&app), app.window, app.next_id), before);
	}

	/// The widths of the two regions, in `Pane` order — what a divider gesture is judged by.
	fn shares(app: &App) -> Vec<f32> {
		ui::split::regions(&app.regions, app.window)
			.values()
			.map(|rect| rect.width)
			.collect()
	}

	/// Where the seam is right now, in window coordinates: the gap between the two regions.
	fn seam_middle(app: &App) -> iced::Point {
		let boxes = ui::split::regions(&app.regions, app.window);
		let left = boxes.values().next().expect("a first region");
		iced::Point::new(
			left.x + left.width + ui::split::SPACING / 2.0,
			left.y + 10.0,
		)
	}

	/// Press the left button at `pointer`, in the two events the raw stream reports it as (§48).
	fn press_at(app: &mut App, pointer: iced::Point) {
		let _ = app.update(Message::PointerMoved(pointer));
		let _ = app.update(Message::PointerPressed);
	}

	/// Press the left button on the seam, wherever it is right now — a divider that has just been
	/// dragged is no longer in the middle, and the pointer that dragged it is on it.
	fn press_seam(app: &mut App) {
		let seam = seam_middle(app);
		press_at(app, seam);
	}

	#[test]
	fn a_double_click_on_the_divider_evens_the_shares() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let split_id = ui::split::seam_at(&app.regions, app.window, seam_middle(&app))
			.expect("a split window has a seam");
		// Drag it well off centre, as a user placing the divider by hand would.
		let _ = app.on_divider_dragged(split_id, 0.75);
		let lopsided = shares(&app);
		assert!(lopsided[0] > lopsided[1], "the drag took effect");
		// Two presses on the seam, close enough together to be one gesture — the drag before them
		// forgot its own press, so these two are a whole double click of their own.
		let seam = seam_middle(&app);
		press_at(&mut app, seam);
		press_at(&mut app, seam);
		let evened = shares(&app);
		assert!(
			(evened[0] - evened[1]).abs() < 0.5,
			"a double-clicked divider goes back to the middle: {evened:?}"
		);
	}

	#[test]
	fn one_press_on_the_divider_leaves_the_shares_alone() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let split_id = ui::split::seam_at(&app.regions, app.window, seam_middle(&app))
			.expect("a split window has a seam");
		let _ = app.on_divider_dragged(split_id, 0.75);
		let placed = shares(&app);
		// A single click is how a drag STARTS. If one press evened the shares, no divider could ever
		// be dragged anywhere (§48).
		press_seam(&mut app);
		assert_eq!(shares(&app), placed);
	}

	#[test]
	fn a_press_that_follows_a_drag_cannot_complete_a_double_click() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let split_id = ui::split::seam_at(&app.regions, app.window, seam_middle(&app))
			.expect("a split window has a seam");
		// Nudge, nudge: press, drag, press again straight away — the ordinary way a divider is
		// placed. The second press is on the same seam inside the double-click window, so only the
		// drag in between stops it throwing away the share just set (§48).
		press_seam(&mut app);
		let _ = app.on_divider_dragged(split_id, 0.7);
		let placed = shares(&app);
		press_seam(&mut app);
		assert_eq!(
			shares(&app),
			placed,
			"a nudge after a nudge must not read as a double click"
		);
	}

	#[test]
	fn two_presses_inside_a_region_do_not_touch_the_divider() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let split_id = ui::split::seam_at(&app.regions, app.window, seam_middle(&app))
			.expect("a split window has a seam");
		let _ = app.on_divider_dragged(split_id, 0.75);
		let placed = shares(&app);
		// A double click in a terminal takes a word (§42) and must do nothing else. The raw stream
		// reports every press in the window, so this is the case the geometry has to rule out.
		let boxes = ui::split::regions(&app.regions, app.window);
		let first = *boxes.values().next().expect("a first region");
		let inside = iced::Point::new(first.x + first.width / 2.0, first.y + first.height / 2.0);
		press_at(&mut app, inside);
		press_at(&mut app, inside);
		assert_eq!(shares(&app), placed);
	}

	#[test]
	fn a_press_elsewhere_between_two_seam_presses_breaks_the_double_click() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let split_id = ui::split::seam_at(&app.regions, app.window, seam_middle(&app))
			.expect("a split window has a seam");
		let _ = app.on_divider_dragged(split_id, 0.75);
		let placed = shares(&app);
		let boxes = ui::split::regions(&app.regions, app.window);
		let first = *boxes.values().next().expect("a first region");
		press_seam(&mut app);
		// A click into the shell in between — the run is over, whatever the clock says (§42).
		press_at(
			&mut app,
			iced::Point::new(first.x + first.width / 2.0, first.y + first.height / 2.0),
		);
		press_seam(&mut app);
		assert_eq!(shares(&app), placed);
	}

	#[test]
	fn a_window_resize_reaches_every_region_not_just_the_focused_one() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Vertical);
		let _ = app.update(Message::WindowResized(iced::Size::new(1000.0, 900.0)));
		assert_eq!(app.window, iced::Size::new(1000.0, 900.0));
		let boxes = ui::split::regions(&app.regions, app.window);
		for (pane, rect) in &boxes {
			let region = app.regions.get(*pane).expect("a live region");
			assert_eq!(
				region.active().window_size,
				iced::Size::new(rect.width, rect.height - ui::tabs::STRIP_HEIGHT),
				"a background region's grid would otherwise keep painting at the old size"
			);
		}
	}

	#[test]
	fn losing_the_window_focus_is_told_to_every_visible_shell() {
		let mut app = tab_app();
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let _ = app.update(Message::WindowFocus(false));
		// Focus reporting is a promise to the program in each shell (§23), and since §48 there is one
		// visible shell per region — a region left un-told would keep reporting the window as focused.
		for (_, region) in app.regions.iter() {
			assert!(!region.active().window_focused);
		}
	}

	#[test]
	fn a_press_in_a_region_gives_it_the_keyboard() {
		let mut app = tab_app();
		let old = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		assert_ne!(app.focus, old);
		let _ = app.update(Message::SplitFocused(old));
		assert_eq!(app.focus, old, "a click moves the keyboard back");
	}

	#[test]
	fn a_strip_drag_never_reaches_another_regions_tabs() {
		let mut app = tab_app();
		let old = app.focus;
		let _ = app.open_tab(old); // the left region has two tabs
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		let left_order: Vec<u64> = app
			.regions
			.get(old)
			.expect("the split region")
			.tabs
			.iter()
			.map(|tab| tab.id)
			.collect();
		// Grab a chip in the left region, then hover and drop over the RIGHT region's only slot. The
		// gesture belongs to the strip it started on, so the drop finds no target there and moves
		// nothing — in either region.
		let _ = app.update(Message::In(old, Box::new(Message::TabSelected(0))));
		let _ = app.update(Message::In(fresh, Box::new(Message::TabDraggedOver(0))));
		let _ = app.update(Message::In(fresh, Box::new(Message::TabDropped)));
		let after: Vec<u64> = app
			.regions
			.get(old)
			.expect("the split region")
			.tabs
			.iter()
			.map(|tab| tab.id)
			.collect();
		assert_eq!(after, left_order, "the left strip kept its order");
		assert_eq!(
			app.regions.get(fresh).expect("the fresh region").tabs.len(),
			1,
			"and nothing landed in the right one"
		);
	}

	#[test]
	fn a_quit_counts_and_drains_the_sessions_in_every_region() {
		let mut app = tab_app();
		// Two live sessions, one in each region, so the count the confirmation quotes and the drain
		// list it builds both have to see past the focused region (§30, §48).
		let (mut left, _left_rx) = app_with_terminal(4);
		let (mut right, _right_rx) = app_with_terminal(4);
		// `is_live` is "a shell is on screen", which is the terminal screen — the helper above builds
		// the emulator and the channel but leaves the tab on its default screen.
		left.screen = AppScreen::Terminal;
		right.screen = AppScreen::Terminal;
		let old = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		app.regions.get_mut(old).expect("the left region").tabs[0] = left;
		app.regions.get_mut(fresh).expect("the right region").tabs[0] = right;
		assert_eq!(app.tabs().filter(|tab| tab.is_live()).count(), 2);
		let _ = app.quit_confirmed();
		match &app.quit {
			Some(QuitPhase::Draining { pending, .. }) => assert_eq!(pending.len(), 2),
			_ => panic!("the quit must wait for both sessions to report down"),
		}
	}

	// --- a chip's menu: sending a tab to another area of the window (§52) ---

	/// Right-click the chip at `index` on the focused region's strip, and hand back what its menu
	/// offers. Driven through `update` so the wrapping that names the strip is exercised too.
	fn chip_menu(app: &mut App, index: usize) -> Vec<ui::tabs::Destination> {
		let pane = app.focus;
		let _ = app.update(Message::In(pane, Box::new(Message::TabMenuOpened(index))));
		let menu = app.strip_menu.expect("a right press opens the menu");
		app.destinations(menu)
	}

	/// The areas a menu lists, in the order it lists them.
	fn offered(destinations: &[ui::tabs::Destination]) -> Vec<ui::split::Area> {
		destinations
			.iter()
			.map(|destination| destination.area)
			.collect()
	}

	/// A tab with a live session on `endpoint`, its shell standing in `cwd` — the only kind of tab
	/// there is anything to duplicate. The receiver is handed back to keep the channel open.
	fn live_tab(id: u64, endpoint: &str, cwd: &str) -> (Tab, mpsc::Receiver<SshCommand>) {
		let (mut tab, rx) = app_with_terminal(32);
		tab.id = id;
		tab.screen = AppScreen::Terminal;
		tab.connection = Some(endpoint.to_owned());
		// One OSC 7 announcement, which is how a shell says where it is (§17).
		let _ = tab.on_ssh_event(shell_output(
			format!("\x1b]7;file://host{cwd}\x07").as_bytes(),
		));
		(tab, rx)
	}

	#[test]
	fn a_whole_window_offers_every_area_and_a_split_one_only_the_two_it_has() {
		let mut app = tab_app();
		// Nothing is cut yet, so all three are on the menu: two of them are a cut away, and taking
		// one is what makes the cut (§52).
		assert_eq!(
			offered(&chip_menu(&mut app, 0)),
			vec![
				ui::split::Area::Main,
				ui::split::Area::Right,
				ui::split::Area::Bottom
			]
		);
		let _ = app.update(Message::TabMenuDismissed);

		let _ = split(&mut app, ui::split::Way::Horizontal);
		// One cut is all there is (§48), so the area the window does NOT have is not offered: it
		// would mean closing a region and cutting the other way, which is more than a menu row can
		// honestly say.
		assert_eq!(
			offered(&chip_menu(&mut app, 0)),
			vec![ui::split::Area::Main, ui::split::Area::Right]
		);
	}

	#[test]
	fn the_menu_greys_a_move_that_would_do_nothing_or_undo_itself() {
		let mut app = tab_app();
		let menu = chip_menu(&mut app, 0);
		assert!(!menu[0].can_move, "it is already in the main area");
		assert!(
			!menu[1].can_move && !menu[2].can_move,
			"the only tab of a region cannot be the one cut away into a new one — the cut and the \
			 collapse behind it would cancel out"
		);
		let _ = app.update(Message::TabMenuDismissed);

		let pane = app.focus;
		let _ = app.update(Message::In(pane, Box::new(Message::TabNew)));
		let menu = chip_menu(&mut app, 0);
		assert!(!menu[0].can_move, "still its own area");
		assert!(
			menu[1].can_move && menu[2].can_move,
			"now the strip has a tab to spare"
		);
	}

	#[test]
	fn a_copy_is_offered_only_where_there_is_a_session_to_copy() {
		let mut app = tab_app();
		assert!(
			chip_menu(&mut app, 0)
				.iter()
				.all(|destination| !destination.can_duplicate),
			"a home tab is nobody's original — there is no connection to make a second time"
		);
		let _ = app.update(Message::TabMenuDismissed);

		let pane = app.focus;
		let (source, _rx) = live_tab(0, "u@h:22", "/srv");
		app.regions.get_mut(pane).expect("the one region").tabs[0] = source;
		assert!(
			chip_menu(&mut app, 0)
				.iter()
				.all(|destination| destination.can_duplicate)
		);
	}

	#[test]
	fn a_move_takes_the_tab_out_of_one_strip_and_puts_it_on_screen_in_the_other() {
		let mut app = tab_app();
		let main = app.focus;
		let _ = app.update(Message::In(main, Box::new(Message::TabNew)));
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let moved = app.regions.get(main).expect("the main region").tabs[0].id;

		app.strip_menu = Some(StripMenu {
			pane: main,
			index: 0,
		});
		let _ = app.move_tab_to(ui::split::Area::Right);

		let fresh = app.pane_of(ui::split::Area::Right).expect("still cut");
		assert_eq!(
			app.regions.get(main).expect("the main region").tabs.len(),
			1
		);
		let right = app.regions.get(fresh).expect("the right region");
		assert_eq!(right.tabs.len(), 2);
		assert_eq!(
			right.tabs[right.active].id, moved,
			"a tab sent somewhere arrives on screen, not hidden behind what was already there"
		);
		assert_eq!(app.focus, fresh, "and the keyboard goes with it (§50)");
		assert!(app.strip_menu.is_none(), "the menu closed behind the item");
	}

	#[test]
	fn moving_the_last_tab_out_of_a_region_makes_the_window_whole_again() {
		let mut app = tab_app();
		let main = app.focus;
		let _ = split(&mut app, ui::split::Way::Horizontal);
		let fresh = app.focus;
		let moved = app.regions.get(fresh).expect("the fresh region").tabs[0].id;

		// The merge gesture: the region a tab leaves empty closes and gives its room back (§48), so
		// this is the way back from a split without closing anything.
		app.strip_menu = Some(StripMenu {
			pane: fresh,
			index: 0,
		});
		let _ = app.move_tab_to(ui::split::Area::Main);

		assert_eq!(region_count(&app), 1);
		let region = app.regions.get(main).expect("main took the room back");
		assert_eq!(region.tabs.len(), 2);
		assert_eq!(region.tabs[region.active].id, moved);
		assert_eq!(app.focus, main);
	}

	#[test]
	fn a_move_to_an_area_that_is_not_there_yet_cuts_the_window_and_lands_in_the_new_half() {
		let mut app = tab_app();
		let main = app.focus;
		let _ = app.update(Message::In(main, Box::new(Message::TabNew)));
		let moved = app.regions.get(main).expect("the one region").tabs[1].id;
		let ids = app.next_id;

		// The monitor step already answered, exactly as the `split` helper does (§48).
		let grown = ui::split::Way::Vertical.grown(app.window);
		let _ = app.apply_split(
			main,
			ui::split::Way::Vertical,
			iced::window::Id::unique(),
			grown,
			SplitSeed::Move(1),
		);

		assert_eq!(region_count(&app), 2);
		assert_eq!(app.regions.get(main).expect("the region cut").tabs.len(), 1);
		let fresh = app.pane_of(ui::split::Area::Bottom).expect("cut downwards");
		assert_eq!(
			app.regions.get(fresh).expect("the fresh region").tabs[0].id,
			moved,
			"the tab that was sent is the tab that is there"
		);
		assert_eq!(
			app.next_id, ids,
			"no id was handed out: the tab was carried across, not built again"
		);
	}

	#[test]
	fn a_region_is_never_emptied_into_a_split_of_itself() {
		let mut app = tab_app();
		let main = app.focus;
		let before = (region_count(&app), app.window);
		// The menu greys this, but the monitor is measured in between and a tab can close in that
		// time — so the rule is enforced again where the cut is actually made (§52).
		let grown = ui::split::Way::Horizontal.grown(app.window);
		let _ = app.apply_split(
			main,
			ui::split::Way::Horizontal,
			iced::window::Id::unique(),
			grown,
			SplitSeed::Move(0),
		);
		assert_eq!(
			(region_count(&app), app.window),
			before,
			"nothing cut, and above all the window did not grow for a split that cannot happen"
		);
		assert_eq!(app.regions.get(main).expect("the one region").tabs.len(), 1);
	}

	#[test]
	fn a_copy_opens_beside_its_original_and_carries_the_shell_directory() {
		use crate::ui::connect::AuthKind;

		let mut app = tab_app();
		let main = app.focus;
		let (source, _rx) = live_tab(0, "u@h:22", "/srv/www");
		app.regions.get_mut(main).expect("the one region").tabs[0] = source;
		app.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);

		app.strip_menu = Some(StripMenu {
			pane: main,
			index: 0,
		});
		let _ = app.duplicate_tab_to(ui::split::Area::Main);

		let region = app.regions.get(main).expect("the one region");
		assert_eq!(region.tabs.len(), 2);
		assert_eq!(
			region.active, 1,
			"a copy made into its own strip lands next to the tab it came from, and on screen"
		);
		let copy = &region.tabs[1];
		assert_eq!(
			copy.carry_cwd.as_ref().map(|carry| carry.cwd.as_str()),
			Some("/srv/www")
		);
		assert_eq!(
			(copy.form.host.as_str(), copy.form.user.as_str()),
			("h", "u")
		);
		assert!(
			matches!(copy.screen, AppScreen::Connect),
			"the password was never remembered, so the copy stops at the form with everything else \
			 already filled in rather than spending an attempt on an empty field"
		);
	}

	#[test]
	fn a_copy_dials_the_same_target_and_lands_the_shell_where_the_original_stood() {
		use crate::ui::connect::AuthKind;

		let (tx, mut rx) = mpsc::channel(64);
		let mut copy = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};
		// Agent auth reads no field (§7), so the copy has everything it needs the moment the form
		// is filled — the common case the menu item exists for.
		copy.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Agent, None, None);

		let _ = copy.open_copy_of("u@h:22", Some("/srv/www".to_owned()));
		assert!(
			matches!(copy.screen, AppScreen::Connecting { .. }),
			"dialed without asking anything"
		);
		assert!(matches!(rx.try_recv(), Ok(SshCommand::Connect(_))));

		// The shell opens: the carried directory is replayed as a `cd` and pins the pane against the
		// announcements that follow, exactly as a remembered one is (§22).
		let _ = copy.on_ssh_event(SshEvent::Connected);
		assert_eq!(copy.resume_cwd.as_deref(), Some("/srv/www"));
		assert_eq!(
			copy.carry_cwd, None,
			"spent, so the next session inherits nothing"
		);
		let mut sent = Vec::new();
		while let Ok(command) = rx.try_recv() {
			sent.push(command);
		}
		assert!(
			sent.iter().any(|command| matches!(
				command,
				SshCommand::Input(bytes) if bytes.as_slice() == b"cd '/srv/www'\r"
			)),
			"the copy walks itself to where the original was standing"
		);
	}

	#[test]
	fn a_copy_of_a_password_session_stops_at_the_form_rather_than_dialing_blind() {
		use crate::ui::connect::AuthKind;

		let (tx, mut rx) = mpsc::channel(8);
		let mut copy = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};
		copy.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);

		let _ = copy.open_copy_of("u@h:22", Some("/srv".to_owned()));
		assert!(matches!(copy.screen, AppScreen::Connect));
		assert_eq!(copy.form.host, "h");
		assert!(rx.try_recv().is_err(), "nothing was dialed");
		assert_eq!(
			copy.carry_cwd.as_ref().map(|carry| carry.cwd.as_str()),
			Some("/srv"),
			"still carried: the user types the password and presses Connect, and the copy still \
			 opens where the original stands"
		);
	}

	/// A copy is made and dialed in the same breath, but a tab is born with **no worker**: iced
	/// rebuilds the subscription list only after the update that created the tab returns, and the
	/// worker announces itself with `Ready` a moment later. So the dial is armed and fired then —
	/// dialing on the spot is what produced "SSH worker is not ready yet" (§52).
	#[test]
	fn a_copy_waits_for_its_own_worker_before_dialing() {
		use crate::ui::connect::AuthKind;

		let mut copy = Tab::default();
		copy.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Agent, None, None);

		let _ = copy.open_copy_of("u@h:22", Some("/srv/www".to_owned()));
		assert!(copy.pending_connect, "armed, waiting for a channel to use");
		assert!(
			matches!(copy.screen, AppScreen::Connect),
			"the pre-filled form is what shows in the meantime, and what is left behind if no \
			 worker ever arrives"
		);
		assert!(
			copy.prompt.is_none(),
			"and above all not an error dialog about a worker that was never late"
		);

		// The worker checks in. The dial goes now, down the channel it just handed over.
		let (tx, mut rx) = mpsc::channel(64);
		let _ = copy.on_ssh_event(SshEvent::Ready(tx));
		assert!(!copy.pending_connect, "spent");
		assert!(matches!(copy.screen, AppScreen::Connecting { .. }));
		assert!(matches!(rx.try_recv(), Ok(SshCommand::Connect(_))));
		assert_eq!(
			copy.carry_cwd.as_ref().map(|carry| carry.cwd.as_str()),
			Some("/srv/www"),
			"still carried across the wait"
		);
	}

	#[test]
	fn a_carried_directory_is_dropped_when_the_form_was_pointed_somewhere_else() {
		use crate::ui::connect::AuthKind;

		let (tx, _rx) = mpsc::channel(64);
		let mut copy = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};
		copy.targets
			.borrow_mut()
			.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		let _ = copy.open_copy_of("u@h:22", Some("/srv/www".to_owned()));

		// The copy stopped at the form, and the user typed a different machine into it. A path only
		// means something on the host it came from, so this one goes unspent (§52).
		copy.form.host = "elsewhere".to_owned();
		copy.form.password = "pw".to_owned();
		let _ = copy.on_connect_pressed();
		let _ = copy.on_ssh_event(SshEvent::Connected);
		assert_eq!(
			copy.resume_cwd, None,
			"no `cd` into a directory from another filesystem"
		);
		assert_eq!(
			copy.carry_cwd, None,
			"and spent either way, so it cannot resurface"
		);
	}

	// --- §122: the frame clock over a held synchronized update ---

	// Long enough that the 150 ms `vte` gives a held frame has certainly passed. A monotonic
	// deadline, so a busy machine only ever makes this MORE true (§122). The same fact under the
	// same name in `term`'s tests — the grace is `vte`'s and it exports no constant for it, so the
	// two modules that have to outwait it each say how long by.
	const HELD_FRAME_SLEEP: std::time::Duration = std::time::Duration::from_millis(200);

	/// The condition the subscription list reads. A shell mid-frame is what asks for a clock; an
	/// ordinary one must not, or every session in the window would tick for ever.
	#[test]
	fn only_a_tab_holding_a_frame_asks_for_a_clock() {
		let (mut app, _rx) = app_with_terminal(8);
		let _task = app.on_ssh_event(shell_output(b"hello"));
		assert!(!app.holds_update(), "an ordinary chunk holds nothing");

		let _task = app.on_ssh_event(shell_output(b"\x1b[?2026hheld"));
		assert!(app.holds_update(), "an open bracket does");

		let _task = app.on_ssh_event(shell_output(b"\x1b[?2026l"));
		assert!(!app.holds_update(), "and the closing byte ends it");
	}

	/// The half that is easy to get wrong: a PARKED identity's shell can be mid-frame too (§45), and
	/// it is the one whose held screen nothing on screen would hint at. `broadcast` would not reach
	/// it, which is why `release_held_updates` walks the tabs itself.
	#[test]
	fn a_parked_identity_holding_a_frame_asks_for_a_clock_too() {
		let (mut app, _rx) = app_with_login_identity();
		let id = elevate_to(&mut app);
		// Output for an account that is NOT on screen goes to its parked emulator and stops there.
		let _task = app.on_ssh_event(SshEvent::Output {
			identity: if app.identity == id {
				bridge::LOGIN_IDENTITY
			} else {
				id
			},
			bytes: b"\x1b[?2026hheld".to_vec(),
		});
		assert!(
			app.holds_update(),
			"the tab answers for every terminal it owns, not just the visible one"
		);
	}

	/// The parked half of the release, which is the one with a route of its own: a background
	/// shell's answer is addressed to THAT shell by name, not sent down the typing path, because the
	/// typing path goes to whichever account is on screen (§45).
	#[test]
	fn a_parked_identity_s_reply_is_addressed_to_its_own_shell() {
		let (mut app, mut rx) = app_with_login_identity();
		let id = elevate_to(&mut app);
		let parked = if app.identity == id {
			bridge::LOGIN_IDENTITY
		} else {
			id
		};
		let _task = app.on_ssh_event(SshEvent::Output {
			identity: parked,
			bytes: b"\x1b[?2026h\x1b[c".to_vec(),
		});
		while rx.try_recv().is_ok() {
			// Drain whatever the elevation itself queued, so what is read below is this frame's.
		}

		std::thread::sleep(HELD_FRAME_SLEEP);
		let _task = app.release_held_updates();
		match rx.try_recv() {
			Ok(SshCommand::Reply { identity, bytes }) => {
				assert_eq!(identity, parked, "answered to the shell that asked");
				assert!(
					bytes.ends_with(b";4c"),
					"and it is the DA1 reply: {bytes:?}"
				);
			}
			other => panic!("expected a reply addressed to the parked shell, got {other:?}"),
		}
	}

	/// A query the shell sent inside a held frame is answered when the frame is released, and the
	/// answer goes back down the typing path — the same route `process`'s replies take, because the
	/// program that asked is blocked reading its stdin until they arrive (§23).
	#[test]
	fn a_released_frame_sends_its_replies_back_to_the_shell() {
		let (mut app, mut rx) = app_with_terminal(8);
		let _task = app.on_ssh_event(shell_output(b"\x1b[?2026h\x1b[c"));
		assert_eq!(
			next_input(&mut rx),
			None,
			"nothing goes back while the frame is held — the engine has not read the query"
		);

		std::thread::sleep(HELD_FRAME_SLEEP);
		let _task = app.release_held_updates();
		let reply = next_input(&mut rx).expect("the held device-attributes query is answered");
		assert!(
			reply.starts_with(b"\x1b[?") && reply.ends_with(b";4c"),
			"a DA1 reply, sixel-amended like any other (§41): {reply:?}"
		);
		assert!(
			!app.holds_update(),
			"and the clock is no longer asked for, so the ticking stops on its own"
		);
	}

	/// A tick that arrives INSIDE the 150 ms must do nothing at all: a frame torn in half is the
	/// very thing the bracket exists to prevent. `frames()` fires every frame, so this is the common
	/// case rather than an edge one.
	#[test]
	fn a_tick_inside_the_grace_releases_nothing() {
		let (mut app, mut rx) = app_with_terminal(8);
		let _task = app.on_ssh_event(shell_output(b"\x1b[?2026h\x1b[c"));
		let _task = app.release_held_updates();
		assert_eq!(next_input(&mut rx), None, "still held");
		assert!(app.holds_update(), "so the clock is still wanted");
	}
}

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
use std::path::{Path, PathBuf};
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

/// The safety net on a clean quit (§30): once the user confirms, cmote waits for every live
/// session to report it has disconnected before the process exits, so no remote connection is
/// cut mid-flight. A session that never acknowledges (a wedged transport) must not wedge quit
/// with it, so after this long the app leaves anyway. In practice the drain finishes in
/// milliseconds — a local channel EOF, not a network round-trip — so this bound is never hit.
const QUIT_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
		// terminal and tall enough to also show the files strip under it (§18, §19). The tree
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
/// a strip of tabs; each `Tab` in one is a whole session (its own screen, terminal, panels and
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
	targets: Rc<RefCell<crate::profiles::Targets>>,
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
			.and_then(|tab| tab.editor.as_ref())
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
		let targets = Rc::new(RefCell::new(crate::profiles::Targets::load()));
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
			Message::EditorOpen { session, path } => self.open_editor(pane, session, path),
			Message::EditorFlush(id) => self.flush_editor_save(id),
			Message::EditorCloseSave => self.editor_close_save(),
			Message::EditorCloseDiscard => self.editor_close_discard(),
			Message::EditorCloseCancelled => {
				self.pending_editor_close = None;
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
		// An editor's load/save reply rides the SESSION's stream (it has no channel of its own) but
		// belongs to the EDITOR tab that asked — route it there by editor id (§32).
		if let Some(editor_id) = event.editor_target() {
			return match self.tab_mut(editor_id) {
				Some(tab) => tab.on_edit_event(event),
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
			// Any editors opened from this session can no longer save — mark them so (§32).
			self.orphan_editors(id);
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
				opening = Some(tab.open_copy_of(key, cwd));
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
			tab.send_command(SshCommand::Disconnect);
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
		let event = match message {
			Message::Key(event) | Message::HomeKey(event) | Message::FormKey(event) => event,
			_ => return None,
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
		// Tear the session down cleanly first: the Disconnect closes the remote side; dropping the
		// tab then drops its command sender, which ends its worker loop (§4, §26).
		if let Some(region) = self.regions.get_mut(pane) {
			region.tabs[index].send_command(SshCommand::Disconnect);
		}
		self.remove_tab(pane, index)
	}

	/// Drop the tab at `index` of the region `pane`, bringing forward the tab the user was on before
	/// this one (§26, §37) — or closing the region itself if that was its last tab (§48).
	fn remove_tab(&mut self, pane: pane_grid::Pane, index: usize) -> iced::Task<Message> {
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
		let opening = tab.open_copy_of(endpoint, cwd);
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

	/// Open a remote file in a new editor tab (§32), parented to the session it was opened from, and
	/// send the load on THAT session's channel. The editor tab has no worker of its own; its reply
	/// (`EditLoaded` / `EditLoadFailed`) rides the parent's stream and routes back here by editor id.
	///
	/// The editor opens in `pane`, the region the file was clicked in (§48) — beside its own session's
	/// chip, in the same strip. It could be argued the other way, that a file wants a region of its
	/// own, but the tab it is grouped with is the one it saves through: keeping the two in one strip
	/// keeps that relationship visible instead of scattering a session's files across the window.
	fn open_editor(
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
		// Open in the scheme this file type was last edited in (§32); an unseen extension starts on
		// the default. The choice is recorded back in `settings` when the toolbar's select changes
		// it, and now rides `settings.json`, so the type keeps its scheme across a restart (§31).
		let theme = self
			.settings
			.editor_theme(&crate::editor::extension_key(&path));
		// The account the file is being opened as (§46) — the one the parent session is SHOWING right
		// now. Fixed into the editor here rather than read again at save time: the file belongs to
		// that account, and the panes may well have switched to another by the time it is saved.
		let identity = self
			.tabs()
			.find(|tab| tab.id == session)
			.map_or(bridge::LOGIN_IDENTITY, |tab| tab.identity);
		let mut tab = Tab::new_editor(id, session, identity, path.clone(), size, theme);
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

		// Ask the parent session to read the file. If the parent is gone the editor opens straight
		// into its "session closed" state rather than hanging on a load that can never arrive. The
		// match resolves to a plain `bool` so the parent borrow is released before the fallback,
		// which borrows the tabs again to reach the just-opened editor.
		let sent = match self.tab_mut(session) {
			Some(parent) if parent.command_tx.is_some() => {
				parent.send_command(SshCommand::EditLoad {
					identity,
					editor_id: id,
					path,
				})
			}
			_ => false,
		};
		if !sent && let Some(editor) = self.editor_mut(id) {
			editor.mark_parent_gone();
			editor.load_failed(
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
	fn flush_editor_save(&mut self, editor_id: u64) -> iced::Task<Message> {
		// The identity comes from the EDITOR, not from what the session is showing now (§46): the file
		// was read as that account and has to be written back as the same one.
		let Some((session, identity, path, bytes)) = self
			.tabs()
			.find(|tab| tab.id == editor_id)
			.and_then(|tab| tab.editor.as_ref())
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
				editor_id,
				path,
				bytes,
			}),
			None => false,
		};
		if !sent && let Some(editor) = self.editor_mut(editor_id) {
			editor.mark_parent_gone();
			editor.save_failed("The session this file came from is closed.".to_owned());
		}
		iced::Task::none()
	}

	/// Mark every editor opened from session `id` as orphaned (§32): its parent is gone, so it can no
	/// longer save. The buffer stays open to read and copy; the toolbar disables Save with a note.
	fn orphan_editors(&mut self, id: u64) {
		for tab in self.tabs_mut() {
			if let Some(editor) = tab.editor.as_mut()
				&& editor.session == id
			{
				editor.mark_parent_gone();
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
		if let Some(editor) = region
			.tabs
			.get_mut(region.active)
			.and_then(|tab| tab.editor.as_mut())
		{
			editor.set_theme(theme);
			let ext = crate::editor::extension_key(&editor.path);
			// Remembered app-wide and written on the way out (§31), so this file type keeps the
			// scheme next run; the returned "changed?" flag is not needed here.
			self.settings.set_editor_theme(ext, theme);
		}
		iced::Task::none()
	}

	/// The editor on the tab with this id, mutably (§32), wherever in the window that tab sits.
	fn editor_mut(&mut self, id: u64) -> Option<&mut crate::editor::Editor> {
		self.tab_mut(id).and_then(|tab| tab.editor.as_mut())
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
				.and_then(|tab| tab.editor.as_ref())
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
			// An editor tab has no session of its own (§32): it saves through the tab it was opened
			// from, so it starts NO worker — opening editors costs no network threads.
			.filter(|tab| tab.editor.is_none())
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
		if on_screen().any(|tab| !tab.dropped.is_empty()) {
			subs.push(iced::window::frames().map(|_instant| Message::FileDropSettled));
		}
		// The keyboard, by contrast, has exactly one destination: the region that holds it (§48).
		match self.active().screen {
			Screen::Terminal => subs.push(iced::keyboard::listen().map(Message::Key)),
			Screen::Connect => subs.push(iced::keyboard::listen().map(Message::FormKey)),
			Screen::Home => subs.push(iced::keyboard::listen().map(Message::HomeKey)),
			// The editor's shortcut keys (Ctrl+S / Ctrl+Shift+S / Ctrl+W); typing goes to the widget.
			Screen::Editor => subs.push(iced::keyboard::listen().map(Message::EditorKey)),
			_ => {}
		}

		iced::Subscription::batch(subs)
	}
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
	/// The server's host key does NOT match the one pinned for it (§8) — key rotation, or a
	/// man-in-the-middle. The loud override dialog shows both fingerprints (stored vs presented,
	/// carried in `App::dialog_body`) and offers reject / trust once / replace. Like
	/// `ConfirmHostKey` this variant is just the marker; the message lives in the dialog body.
	HostKeyChanged,
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
	/// A text editor open on a remote file (§32). This tab is NOT a session — it has no connection
	/// of its own; its loads and saves ride the parent session's channel. The buffer and its state
	/// live in `Tab::editor`, which is `Some` exactly while this screen shows.
	Editor,
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

/// One account this session has a shell for (§45).
///
/// An SSH connection authenticates once, as one user; becoming another account is a program run on
/// that same connection (`sudo -u root -i`), which gets a channel and a shell of its own. So this is
/// not a second connection and not a second tab — it is one more shell on the one session, with its
/// own view of the machine parked beside it.
///
/// It holds no account NAME. It used to, for the status bar's label and the switcher's entries, and
/// both are gone (§45's UX was withdrawn, and the label with it — the bar's centred endpoint already
/// says who the session is). A name nothing reads is a name nothing keeps true, so the elevation
/// this list is waiting for will add it back beside whatever displays it.
#[derive(Debug, Default)]
struct Identity {
	/// The number the SSH task knows this shell by. `bridge::LOGIN_IDENTITY` for the account the
	/// session authenticated as; counted up from 1 for each elevation.
	id: u64,
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
	hover_cell: ui::selection::Cell,
	clicks: ui::selection::Clicks<ui::selection::Cell>,
	search: Option<term::search::Search>,
	search_stale: bool,
}

/// One session's whole state — its screen, its connection, its terminal and panels, its
/// dialogs (§6). This used to BE the app; with tabs (§26) the app owns a `Vec<Tab>` and each
/// tab is one of these, fully independent: a tab can sit at the home list while another runs a
/// shell. Everything here is per-tab EXCEPT the two `Rc<RefCell<…>>` fields, which are shared
/// clones of the single app-wide target list and secret vault (see `App`).
#[derive(Debug, Default)]
pub struct Tab {
	/// This tab's stable identity, handed out by `App` and never reused (§26). It keys the
	/// tab's own SSH worker subscription and routes that session's events back to this tab.
	id: u64,
	/// Which screen is visible.
	pub screen: Screen,
	/// The saved connection targets shown on the home screen (§14, §26). A shared clone of the
	/// ONE app-wide list (loaded from disk at startup, kept sorted, re-saved on any change): a
	/// rename or delete in one tab's home screen is seen by every other, and there is a single
	/// file on disk. Profiles only — never any secret material (§12).
	targets: Rc<RefCell<crate::profiles::Targets>>,
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
	/// The open text editor, when this tab is editing a remote file rather than running a session
	/// (§32). `Some` exactly while `screen` is `Screen::Editor`; it holds the buffer, the encoding,
	/// the changed-line marks and the id of the parent session its saves ride through.
	editor: Option<crate::editor::Editor>,
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
	/// The multi-click tally over the grid (§42): how many presses in a row landed on one cell, so a
	/// press knows whether it is a plain click, a word (double) or a line (triple). `mouse_area`
	/// reports presses one at a time and counts nothing itself.
	clicks: ui::selection::Clicks<ui::selection::Cell>,
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
	/// Where this tab's open dialog floats and whether it is being dragged (§10). Centred each
	/// time a dialog opens, then it follows the header. The same `ui::dialog::Card` the App-level
	/// overlay holds — the two differ only in the box they are measured against, and that box is
	/// an argument, not a second copy of the arithmetic.
	card: ui::dialog::Card,
	/// The last known size of the box this tab fills (§10, §48) — the OS window when the window is
	/// whole, this tab's REGION once it is split. Tracked from resize events so a dialog can be
	/// centred and clamped within the space the tab actually occupies.
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
	/// The FOLDERS waiting to go, tree-and-all, into `upload_dir` (§29). A drop can carry both kinds
	/// at once, and the two travel by different routes — a file joins the `uploads` queue above, a
	/// folder is a whole recursive transfer of its own — so they queue separately and run one after
	/// another through the single transfer slot. Empty except while a drop's folders are draining.
	upload_trees: std::collections::VecDeque<PathBuf>,
	/// How many of the batch have landed, for its closing notice (§17).
	uploaded: usize,
	/// How many whole FOLDERS have landed, counted apart from the files so the closing notice can
	/// say what actually went (§29) — "3 files and 2 folders" rather than five of something.
	uploaded_trees: usize,
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
	/// Whether a file from the OS is being dragged over the window right now (§29). Lights the
	/// files pane as the drop target while true; set by `FileHovered`, cleared when the drag leaves
	/// or drops. Purely a visual cue — the drop itself reads the pane's directory, not this flag.
	drop_hover: bool,
	/// The paths of a drop that has just landed, gathering (§29). The OS reports a multi-file drop
	/// as one event PER PATH, with nothing to say the last has arrived — so they are collected here
	/// and read once on the next frame, when the whole drop is known. Empty at rest, which is also
	/// what tells `subscription` there is no settling to wait for.
	dropped: Vec<PathBuf>,
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
	/// The transfer running right now, remembered so a mid-flight failure can be resumed (§16). Set
	/// at every start — a queued file, a folder tree, or a resume itself — and cleared when it
	/// lands; it carries the direction and endpoints `resume_transfer` needs to relaunch it. `None`
	/// when nothing is transferring.
	in_flight: Option<Resumable>,
	/// A transfer that stopped on a failure and can be picked up where it left off (§16). Set from
	/// `in_flight` when a `TransferInterrupted` arrives — the partial was kept, unlike a cancel —
	/// and cleared by a resume, a cancel, a fresh transfer, or a clean finish. `Some` is what draws
	/// the status bar's Resume button.
	resumable: Option<Resumable>,
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
	/// This session's port forwards (§27), each an entry with its runtime id, spec and status.
	/// Populated on connect from the target's saved set and by the tunnels dialog; the ids key a
	/// forward to its `ForwardReady` / `ForwardFailed` event and to the `RemoveForward` command.
	forwards: Vec<crate::forward::ForwardEntry>,
	/// The next forward id to hand out (§27). Monotonic per tab, never reused, so a removed
	/// forward's late event can never land on a new one.
	next_forward_id: u64,
	/// Whether the port-forwards management dialog is open (§27).
	forward_dialog: bool,
	/// The add form's selected kind (§27).
	forward_kind: crate::forward::ForwardKind,
	/// The add form's listen and target fields, and the last parse error to show under them
	/// (§27). Cleared as forwards are added; the error is cleared on the next edit or a clean add.
	forward_listen: String,
	forward_to: String,
	forward_error: Option<String>,
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

/// A transfer kept so it can be relaunched (§16) — as `in_flight` while it runs, or as `resumable`
/// after a failure parked it. It carries just enough to re-issue the exact command: the direction
/// (which decides upload vs download, single file vs whole tree) and its two endpoints. A resume
/// re-sends with `resume` set, so the task appends only the bytes still missing rather than
/// starting the file over.
#[derive(Debug, Clone)]
enum Resumable {
	/// A single file going up: local source, remote destination.
	Upload { local: PathBuf, remote: String },
	/// A single file coming down: remote source, local destination.
	Download { remote: String, local: PathBuf },
	/// A whole folder going up (§17): local root, remote parent directory.
	UploadTree { local: PathBuf, remote: String },
	/// A whole folder coming down (§19): remote root, local parent directory.
	DownloadTree { remote: String, local: PathBuf },
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
/// is anchored to its panel for the same reason (§18), it is always on screen, and it follows a
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
	/// Override a CHANGED key for this session only, without touching known_hosts (§8).
	TrustHostKeyOnce,
	/// Override a CHANGED key by replacing the stale known_hosts entry with the new one (§8).
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
	UploadClashResolved(ClashChoice),
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
	// --- the in-tab text editor (§32): a tab can edit a remote file, not only run a session ---
	/// Open a remote file in a new editor tab (payload: the parent session's id and the path).
	/// Raised by the files pane's "Edit…" or a file double-click; `App` creates the tab, then
	/// sends the load on the parent session's channel and routes the reply back by editor id.
	EditorOpen {
		session: u64,
		path: String,
	},
	/// Something happened in an editor buffer or its toolbar (§32). Nested like `Files` — an
	/// editor has enough interactions of its own to keep out of this enum's top level.
	Editor(crate::editor::EditorMessage),
	/// A keystroke while an editor tab is active (§32): the shortcuts (Ctrl+S save, Ctrl+Shift+S
	/// save as, Ctrl+W close). Typing itself reaches the text widget directly, not here.
	EditorKey(iced::keyboard::Event),
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
		targets: Rc<RefCell<crate::profiles::Targets>>,
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
			screen: Screen::Editor,
			editor: Some(crate::editor::Editor::loading(
				session, identity, path, theme,
			)),
			window_size,
			window_focused: true,
			shell_focus_reported: true,
			..Self::default()
		}
	}

	/// Ask `App` to open `path` in a new editor tab parented to THIS session (§32). Raised by the
	/// files pane's "Edit…" and a file double-click; `App` creates the tab and drives the load.
	fn request_edit(&self, path: String) -> iced::Task<Message> {
		iced::Task::done(Message::EditorOpen {
			session: self.id,
			path,
		})
	}

	/// The label this tab shows on its strip chip (§26): the connected endpoint once a shell is
	/// open (or dialing), otherwise a word for the screen it is sitting on. Names the session so a
	/// user with several open can tell them apart.
	fn strip_label(&self) -> String {
		match &self.screen {
			Screen::Terminal | Screen::Connecting { .. } => self
				.connection
				.clone()
				.unwrap_or_else(|| "session".to_owned()),
			Screen::Home => "Home".to_owned(),
			Screen::Error => "Error".to_owned(),
			// An editor tab is named by its file, with a dot when it has unsaved edits (§32).
			Screen::Editor => match &self.editor {
				Some(editor) => {
					let name = crate::explorer::name(&editor.path);
					let dot = if editor.is_dirty() { "• " } else { "" };
					format!("{dot}{name}")
				}
				None => "editor".to_owned(),
			},
			// The connect form and every dialog over it are all one "new connection" in progress.
			_ => "New connection".to_owned(),
		}
	}

	/// The command-status dot for this tab's chip (§34), from its OSC 133 shell-integration marks.
	/// A running command wins over any past result; otherwise the last exit code decides ok vs
	/// failed. `None` when the tab runs no shell, or when the shell has announced no integration
	/// (no command has finished and none is running) — so the chip shows no dot at all.
	fn prompt_status(&self) -> Option<ui::tabs::Status> {
		let terminal = self.terminal.as_ref()?;
		match terminal.command_state() {
			term::osc133::CommandState::Running => Some(ui::tabs::Status::Running),
			// At a prompt or idle: the last command's exit code is what the dot reports, if one has
			// finished. A shell that never emits the `D` mark leaves this `None`.
			_ => match terminal.last_exit()? {
				0 => Some(ui::tabs::Status::Ok),
				_ => Some(ui::tabs::Status::Failed),
			},
		}
	}

	/// Whether this tab holds a live shell (§26). Closing one is confirmed like a Disconnect;
	/// closing a tab still at the home list or the connect form just drops it.
	fn is_live(&self) -> bool {
		matches!(self.screen, Screen::Terminal)
	}

	/// Whether this tab is an editor with unsaved edits (§32). Its "×" is confirmed like a live
	/// session's, so a stray click cannot lose the work.
	fn is_dirty_editor(&self) -> bool {
		self.editor
			.as_ref()
			.is_some_and(crate::editor::Editor::is_dirty)
	}

	/// Apply an editor-buffer message (§32): typing and the Save As prompt's own field are handled
	/// here; a Save / Save As confirm updates local state then asks `App` to flush the bytes, which
	/// alone can reach the parent session's channel.
	fn on_editor(&mut self, message: crate::editor::EditorMessage) -> iced::Task<Message> {
		use crate::editor::EditorMessage;
		let Some(editor) = self.editor.as_mut() else {
			return iced::Task::none();
		};
		match message {
			EditorMessage::Action(action) => {
				editor.perform(action);
				// Keep the cursor on screen after the move (§32). The buffer no longer scrolls itself on
				// EITHER axis (the gutter/horizontal trick), so both follows are driven here — the same
				// keep-it-visible math the panels use for a selected cell (§20), now applied on both axes.
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
			let find_open = self.editor.as_ref().is_some_and(|e| e.find.is_some());
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

	/// Apply an editor load/save reply routed here by id (§32). A successful load fills the buffer
	/// (or, if the bytes are not text in a supported encoding, shows the reason in its place); a
	/// successful save clears the marks and — after a "Save & close" — drops the tab.
	fn on_edit_event(&mut self, event: SshEvent) -> iced::Task<Message> {
		let id = self.id;
		let Some(editor) = self.editor.as_mut() else {
			return iced::Task::none();
		};
		match event {
			SshEvent::EditLoaded { bytes, .. } => match crate::editor::decode(&bytes) {
				Some((text, encoding)) => editor.set_loaded(text, encoding),
				None => editor.load_failed(
					"This file is not text in a supported encoding (UTF-8 or UTF-16).".to_owned(),
				),
			},
			SshEvent::EditLoadFailed { reason, .. } => editor.load_failed(reason),
			SshEvent::EditSaved { path, .. } => {
				editor.path = path;
				editor.mark_saved();
				// A "Save & close" waits on exactly this: the write landed, so drop the tab now (§32).
				if editor.take_close_after_save() {
					return iced::Task::done(Message::EditorCloseNow(id));
				}
			}
			SshEvent::EditSaveFailed { reason, .. } => {
				// Clear any pending close so a FAILED "Save & close" keeps the tab, showing the error.
				editor.take_close_after_save();
				editor.save_failed(reason);
			}
			// Not an editor event; nothing to do.
			_ => {}
		}
		iced::Task::none()
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
			Message::AcceptHostKey => self.on_host_key_decision(HostKeyChoice::Pin),
			Message::RejectHostKey => self.on_host_key_decision(HostKeyChoice::Reject),
			Message::TrustHostKeyOnce => self.on_host_key_decision(HostKeyChoice::TrustOnce),
			Message::ReplaceHostKey => self.on_host_key_decision(HostKeyChoice::Pin),
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
			// OS file drops (§29): a drag over the window lights the pane as the drop target, and a
			// drop uploads the file into it. Only a live session can be a target, so a hover with no
			// shell open lights nothing.
			Message::FileHovered => self.drop_hover = self.terminal.is_some(),
			Message::FileDropLeft => self.drop_hover = false,
			// One event per PATH, so nothing is decided here: the paths gather and the next frame
			// reads the whole drop at once (§29).
			Message::FileDropped(path) => {
				self.drop_hover = false;
				self.dropped.push(path);
			}
			Message::FileDropSettled => return self.on_drop_settled(),
			Message::DisconnectPressed => self.on_disconnect_pressed(),
			Message::DisconnectConfirmed => return self.on_disconnect_confirmed(),
			Message::DisconnectCancelled => self.confirm_disconnect = false,
			Message::GridMoved(point) => self.on_grid_moved(point),
			Message::GridPressed => self.on_grid_pressed(),
			Message::GridReleased => self.on_grid_released(),
			Message::GridRightPressed => self.menu = Some(self.pointer),
			Message::MouseReport(bytes) => self.on_mouse_report(bytes),
			Message::TerminalScroll(lines) => self.on_terminal_scroll(lines),
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
				self.on_terminal_command();
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
			Message::TransferCancelPressed => self.cancel_transfer(),
			Message::TransferResumePressed => return self.resume_transfer(),
			Message::Explorer(message) => return self.on_explorer(message),
			Message::Files(message) => return self.on_files(message),
			// The editor buffer's own interactions (§32): typing, Save / Save As, the prompt field.
			// The App has already peeled off the ones needing cross-tab reach (open / flush / close).
			Message::Editor(message) => return self.on_editor(message),
			Message::EditorKey(event) => return self.on_editor_key(event),
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
			// The quit flow is `App`'s job too (§30) — a tab never sees these.
			| Message::QuitRequested
			| Message::QuitConfirmed
			| Message::QuitCancelled
			| Message::QuitTick
			// The editor's cross-tab work is `App`'s job (§32): opening a tab, flushing a save
			// through the parent's channel, and the unsaved-close prompt all need reach a tab lacks.
			| Message::EditorOpen { .. }
			| Message::EditorFlush(_)
			| Message::EditorCloseSave
			| Message::EditorCloseDiscard
			| Message::EditorCloseCancelled
			| Message::EditorCloseNow(_)
			| Message::EditorThemeSelected(_)
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
			| Message::PointerPressed => {}
			// Port forwarding (§27).
			Message::ForwardsPressed => return self.open_forwards_dialog(),
			Message::ForwardsClosed => self.forward_dialog = false,
			Message::ForwardKindSelected(kind) => {
				self.forward_kind = kind;
				self.forward_error = None;
			}
			Message::ForwardListenChanged(value) => {
				self.forward_listen = value;
				self.forward_error = None;
			}
			Message::ForwardToChanged(value) => {
				self.forward_to = value;
				self.forward_error = None;
			}
			Message::ForwardAddPressed => self.add_forward(),
			Message::ForwardRemove(id) => self.remove_forward(id),
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
			if self.vault.borrow().is_none() {
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
		// key path and certificate are only meaningful for key auth; the name here is a
		// placeholder — `upsert_on_connect` keeps an existing target's custom name.
		let (key_path, cert_path) = if self.form.auth_kind == ui::connect::AuthKind::Key {
			(self.form.key_path.clone(), self.form.cert_path.clone())
		} else {
			(None, None)
		};
		self.pending_target = Some(crate::profiles::Target {
			name: crate::profiles::endpoint_of(&params.user, &params.host, params.port),
			host: params.host.clone(),
			port: params.port,
			user: params.user.clone(),
			auth_kind: self.form.auth_kind,
			key_path,
			cert_path,
			// Placeholder like `name`: the stored preference wins on connect, and a
			// brand-new target takes the default `upsert_on_connect` gives it (§14).
			show_hidden: self.explorer.show_hidden(),
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
				*self.vault.borrow_mut() = Some(vault);
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
			// The promptless methods have no stored secret to fill — interactive types every
			// factor live and agent auth signs with a key the agent already holds (§7). A
			// remembered target is never one of these, so these arms are not reached in practice.
			AuthKind::Interactive | AuthKind::Agent => {}
		}
	}

	/// Relay the user's host-key choice to the SSH task (§8): reject, trust once, or pin. Any
	/// choice but reject means the handshake proceeds, so we go back to a connecting status; on
	/// reject the refused handshake surfaces its own error and moves the screen.
	fn on_host_key_decision(&mut self, choice: HostKeyChoice) {
		let proceeding = choice != HostKeyChoice::Reject;
		if self.send_command(SshCommand::HostKeyResponse(choice)) && proceeding {
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
		self.card = ui::dialog::Card::opened(self.window_size);
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
			SshEvent::HostKeyChanged { stored, presented } => {
				// Seed the selectable body with the warning plus BOTH fingerprints, each labelled
				// and on its own line, so the whole block — what was trusted vs what was sent — can
				// be selected and copied for out-of-band comparison (§8, §10).
				self.set_dialog_body(&format!(
					"{}\n\nStored (trusted before):\n{stored}\n\nPresented (sent now):\n{presented}",
					ui::HOST_KEY_CHANGED_DIALOG_BODY
				));
				self.screen = Screen::HostKeyChanged;
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
				// The forwards this target saved (§27), read here and re-established once the shell
				// is up. Captured in the same short borrow discipline as the session snapshot.
				let mut saved_forwards = Vec::new();
				if let Some(target) = self.pending_target.take() {
					let key = self.targets.borrow_mut().upsert_on_connect(
						&target.host,
						target.port,
						&target.user,
						target.auth_kind,
						target.key_path,
						target.cert_path,
					);
					// Restore this target's remembered session before the panels list anything
					// (§22): the `.*` filter and panel sizes go on now, and the resume paths
					// come back to drive the cd / pane / tree restore below. `upsert_on_connect`
					// leaves a known endpoint's saved state untouched, so it is still here to
					// read; taking an owned snapshot ends the borrow before the panels change.
					// Snapshot the saved session in a short borrow that ends before the `&mut self`
					// call: a held `Ref` on the shared target cell would clash with
					// `restore_session` (§26).
					let session = self
						.targets
						.borrow()
						.find(&key)
						.map(crate::profiles::Target::session);
					if let Some(session) = session {
						(resume_terminal, resume_files) = self.restore_session(session);
					}
					// The saved forwards, taken by a short borrow that ends before any `&mut self`
					// call below (§27), to be started once the terminal is shown.
					saved_forwards = self
						.targets
						.borrow()
						.find(&key)
						.map(|target| target.forwards.clone())
						.unwrap_or_default();
					// Remembered-secret bookkeeping (§16). A successful connect is the ONLY place
					// a secret is persisted — the credentials are now known good, so a wrong
					// password was never stored. With "Remember" on, store what dial captured;
					// with it off, forget any secret the vault held for this endpoint. The
					// target's flag is then synced to what the vault actually holds, so the home
					// list never promises a pre-fill that is not there. All of this needs the
					// vault unlocked, which the dial / open flow already ensured whenever a secret
					// was in play; if it is locked (the user never engaged it) the flag is left
					// as stored.
					if let Some(vault) = self.vault.borrow_mut().as_mut() {
						if let Some((endpoint, secret)) = self.pending_remember.take() {
							if let Err(error) = vault.store(&endpoint, secret) {
								eprintln!("could not save the vault: {error:#}");
							}
						} else if !self.form.remember
							&& let Err(error) = vault.forget(&key)
						{
							eprintln!("could not update the vault: {error:#}");
						}
						self.targets
							.borrow_mut()
							.set_remembered(&key, vault.get(&key).is_some());
					}
					self.pending_remember = None;
					self.home_selected = Some(key);
					if let Err(error) = self.targets.borrow().save() {
						eprintln!("could not save targets: {error:#}");
					}
				}
				// A shell is open: spin up an emulator at the pty size we asked for,
				// show the terminal, then immediately refit it to the real window
				// rather than waiting for the first resize event.
				self.terminal = Some(new_emulator());
				self.clear_grid_interaction();
				self.screen = Screen::Terminal;
				// This shell is the session's first identity (§45): the account it authenticated as.
				// It is the one that can never be elevated away or closed, and the one every other
				// identity falls back to.
				self.identities = vec![Identity {
					id: bridge::LOGIN_IDENTITY,
					ready: true,
					work: Workspace::default(),
				}];
				self.identity = bridge::LOGIN_IDENTITY;
				self.next_identity = 1;

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
			// A credential question from an elevating shell (§45). Nothing can ask one while there is
			// no way to START an elevation — the dialog that answered these was withdrawn with the
			// rest of that UX — so this is ignored rather than answered. The arm stays because the
			// SSH side that raises it stays: whatever replaces the dialog will want it back.
			SshEvent::ElevatePrompt { .. } => {}
			SshEvent::IdentityReady { identity } => return self.on_identity_ready(identity),
			SshEvent::IdentityEnded { identity, reason } => {
				return self.on_identity_ended(identity, reason);
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
				// This file landed, so there is nothing to resume; the next one, if any, is
				// remembered afresh by `pump_downloads`.
				self.in_flight = None;
				self.resumable = None;
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
			// A transfer stopped mid-flight but kept its partial (§16), so it can be resumed rather
			// than lost: park it, show the reason, and offer Resume. The queue behind it is left in
			// place, so resuming the failed file drains the rest afterwards. Direction-agnostic —
			// `in_flight` already knows which command to relaunch.
			SshEvent::TransferInterrupted { message } => {
				self.transfer = None;
				self.transfer_notice = Some(message);
				self.resumable = self.in_flight.take();
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
				// One file — or one whole folder — landed; count it and start the next. The closing
				// notice, and clearing the picked files, wait until everything has drained (§17).
				// Which of the two it was is read off `in_flight` before that is cleared: a tree
				// reports the same `UploadDone` its files do, and the closing notice says what went.
				let was_tree = matches!(self.in_flight, Some(Resumable::UploadTree { .. }));
				self.transfer = None;
				// Landed, so nothing to resume; `pump_uploads` remembers the next file itself.
				self.in_flight = None;
				self.resumable = None;
				if was_tree {
					self.uploaded_trees += 1;
				} else {
					self.uploaded += 1;
				}
				self.pump_uploads();
				if self.transfer.is_none() && self.uploads.is_empty() {
					self.transfer_notice =
						Some(upload_summary(self.uploaded, self.uploaded_trees, &path));
					// Show what just landed: if the pane (or the tree) is on the folder we uploaded
					// into, re-list it so the new file — or folder — appears without a manual Refresh
					// (§29). Captured before `finish_batch`, which clears `upload_dir`.
					let dir = self.upload_dir.clone();
					self.finish_batch();
					self.refresh_remote_dir(&dir);
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
			// A forward came up or failed (§27): mark its row. A failure never tears the shell
			// down — the tunnel simply shows as failed in the dialog. A late event for a forward
			// already removed finds no entry and is dropped.
			SshEvent::ForwardReady { id, assigned_port } => {
				self.mark_forward_ready(id, assigned_port)
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
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				self.forget_identities();
				return self.go_home();
			}
			SshEvent::Error(message) => {
				// Only saves when a shell had actually opened — an auth/handshake failure
				// reaches here with no terminal, and `persist_session` then does nothing (§22).
				self.persist_session();
				self.terminal = None;
				self.connection = None;
				self.clear_grid_interaction();
				self.forget_identities();
				self.show_error(&message);
			}
			// An editor's load/save replies are routed by `App` straight to the editor tab that asked
			// (`on_edit_event`, §32), so a session's own event stream never delivers them here.
			SshEvent::EditLoaded { .. }
			| SshEvent::EditLoadFailed { .. }
			| SshEvent::EditSaved { .. }
			| SshEvent::EditSaveFailed { .. } => {}
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
		let (rows, cols) = ui::terminal::grid_size(size, self.files.reserved());
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
			let (rows, cols) = terminal.screen().size();
			self.hover_cell = ui::terminal::cell_at(self.pointer, rows, cols);
		}
		self.rescan_find();
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

	/// Open the port-forwards manager (§27): close any context menu, show the dialog centred, and
	/// focus the listen field so a forward can be typed straight away.
	fn open_forwards_dialog(&mut self) -> iced::Task<Message> {
		self.menu = None;
		self.forward_error = None;
		self.card = ui::dialog::Card::opened(self.window_size);
		self.forward_dialog = true;
		iced::widget::operation::focus(ui::forward::LISTEN_INPUT_ID)
	}

	/// Add the forward described by the add form (§27): parse the two fields, reject a duplicate
	/// bind, then hand it a fresh id, queue it as `Starting`, ask the worker to start it, and
	/// save the updated set to the target. A parse error is shown under the form and nothing is
	/// sent. The listen/target fields are cleared on success so the next forward starts blank;
	/// the kind is kept, since adding several of one kind is common.
	fn add_forward(&mut self) {
		let spec = match crate::forward::ForwardSpec::parse(
			self.forward_kind,
			&self.forward_listen,
			&self.forward_to,
		) {
			Ok(spec) => spec,
			Err(reason) => {
				self.forward_error = Some(reason);
				return;
			}
		};
		// Two forwards cannot bind the same local (or server) endpoint; refuse the duplicate
		// before it is sent, so the second one's inevitable bind failure never happens.
		if self
			.forwards
			.iter()
			.any(|entry| entry.spec.same_endpoint(&spec))
		{
			self.forward_error = Some("A forward already binds that address.".to_owned());
			return;
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
			self.forward_listen.clear();
			self.forward_to.clear();
			self.forward_error = None;
			self.persist_forwards();
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
		let (host, port, user, auth_kind, key_path, cert_path, remember) =
			self.targets.borrow().find(key).map(|target| {
				(
					target.host.clone(),
					target.port,
					target.user.clone(),
					target.auth_kind,
					target.key_path.clone(),
					target.cert_path.clone(),
					target.remember_secret,
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
				self.screen = Screen::Connect;
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
	fn open_copy_of(&mut self, endpoint: String, cwd: Option<String>) -> iced::Task<Message> {
		self.carry_cwd = cwd.map(|cwd| Carry {
			endpoint: endpoint.clone(),
			cwd,
		});
		if let Some(deferred) = self.seed_form(&endpoint) {
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

	/// Handle a key on the home screen (§14). While the delete prompt is up the list
	/// shortcuts are inert and only Esc is handled (it cancels, keeping the target) — a
	/// stray Enter must not open a connection behind the modal. While renaming, only Esc
	/// (cancel) is handled here — the field's own `on_submit` commits on Enter. Otherwise
	/// F2 renames the selection, Enter opens it, Delete asks to remove it; all are no-ops
	/// without a selection. Other keys fall through.
	fn on_home_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
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

		// Ctrl+D closes this tab — but only from the home screen, i.e. once logged off from any
		// remote (§30). On a live shell the same key is EOF to the remote (the way you log out),
		// so it is left to the terminal there; pressing it logs the shell out, which lands back
		// here, and a second Ctrl+D then closes the tab — mirroring a terminal's own Ctrl+D twice.
		// It routes through `TabCloseRequested`, so closing the last tab still asks to quit cmote.
		if modifiers.control()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& matches!(&key, iced::keyboard::Key::Character(character) if character.as_str() == "d")
		{
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

		// Ctrl+Shift+F opens the scrollback find bar and focuses its field (§35). Taken BEFORE the
		// bar's own keyboard guard below, so pressing it again while the bar is up refocuses the
		// field rather than being swallowed. Matched on the PHYSICAL key like the copy/paste
		// bindings, so it holds on any layout; plain Ctrl+F belongs to the shell (readline's
		// forward-char), which is why only the Shift form is cmote's.
		if modifiers.control()
			&& modifiers.shift()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& matches!(physical_key, Physical::Code(Code::KeyF))
		{
			return self.open_term_find();
		}

		// While the find bar is open it owns the keyboard (§35): its field types through the widget
		// tree, so nothing here may ALSO reach the remote — otherwise searching the scrollback would
		// be typing at the shell's prompt. Exactly the rule the inline rename fields above follow.
		// Esc closes the bar; the current match stays selected, so it can still be copied.
		if self.search.is_some() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.search = None;
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

		// Typing takes the keyboard back to the shell (§50). A panel answers to the arrows, the
		// Page keys, Tab, Enter, F2, F5 and Esc — never to a plain character — so a letter
		// arriving while a panel holds the ring is someone starting a command at the prompt they
		// are looking at, with the focus left on a pane they navigated a while ago. The old
		// behaviour dropped that keystroke: the panel swallowed it, nothing happened, and the
		// first letter of the command was silently eaten (or, worse, the first several, until the
		// missing echo was noticed). Handing the focus over is what the user was asking for by
		// typing at all. Taken before the panel dispatch below, so the key itself goes on to the
		// shell rather than being spent on the switch.
		if !matches!(self.focus, Focus::Terminal) && is_typing(&key, modifiers) {
			self.set_focus(Focus::Terminal);
		}

		// Ctrl+V is typing by another route, so it is answered from wherever the keyboard is (§50)
		// — the same reading that makes the menu's own Paste take the focus back. It sits ABOVE the
		// panel dispatch rather than in the copy/paste block below for exactly that reason: down
		// there it is only reached with the shell already focused, and a paste aimed at the shell
		// while a panel held the ring would be dropped on the floor with no echo to say so. Neither
		// panel claims Ctrl+V, so nothing is being taken from them.
		//
		// Ctrl+C is NOT treated this way. It reads the terminal's own selection or, with none, is
		// the interrupt for the remote — neither is text going in, and the panels have the better
		// claim on a future "copy what is selected here".
		if is_paste(physical_key, modifiers) {
			self.on_terminal_command();
			return self.on_paste();
		}

		// A focused panel keeps the key; only the shell's own focus reaches the channel.
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

		// Ctrl+Shift+Up / Ctrl+Shift+Down jump the scrollback to the previous / next shell prompt
		// (§34), from the OSC 133 marks. Like the Shift+Page scroll below, it is cmote's own view
		// motion — nothing is sent to the remote — and reached only with the shell focused. Guarded
		// on Ctrl+Shift together so a bare or singly-modified arrow still reaches the shell.
		if modifiers.control()
			&& modifiers.shift()
			&& let iced::keyboard::Key::Named(named) = &key
			&& let Some(direction) = prompt_jump(named)
			&& let Some(terminal) = self.terminal.as_mut()
		{
			terminal.jump_prompt(direction);
			return iced::Task::none();
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

	/// A command from the terminal's own surface ran — an item of the grid's right-click menu, the
	/// status-bar button that duplicates it, or the Ctrl+V that is the same command off the
	/// keyboard (§50). Whatever held cmote's keyboard until now, the user just reached into the
	/// terminal and acted on it, so the ring goes back there.
	///
	/// It matters most for **Paste**, which is typing at the prompt by another route: pasting a
	/// command while the files pane held the focus used to leave the next keystroke — the Enter
	/// that runs it — going to the pane. But the same reading covers the rest of the menu: a copy,
	/// an upload into the shell's directory, a link followed out of its scrollback are all work on
	/// the terminal, and none of them is a reason to keep the keyboard parked on a panel.
	///
	/// Only an ITEM does this, not the right-press that opens the menu: opening it is a question
	/// about what is under the pointer, and dismissing it (Esc, or a click on the dismiss layer)
	/// leaves everything as it was — including where the keyboard is.
	fn on_terminal_command(&mut self) {
		self.focus_pane(Focus::Terminal);
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
				// Open the folder — the same call the row click makes: a folder never listed is
				// fetched here too, and re-opening a closed one re-lists it (`expand`), so the
				// keyboard catches a shell-side change just as the mouse does.
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
			// F5 refreshes the whole visible tree, the same as the header ↻ button — the
			// familiar file-manager key for "bring what I am looking at up to date".
			Named::F5 => return self.on_explorer(ExplorerMessage::RefreshTree),
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

		let columns = ui::files::columns(self.files_width()) as isize;
		// A page is a screenful of rows (less one, for context), turned into a model-space delta
		// by the column count — the same units `step` moves the arrows in.
		let page = ui::files::page_rows(&self.files) as isize * columns;
		// Shift held on a movement key extends the selection instead of moving it (§21). Not on
		// Tab: there, Shift already means "the other way".
		let extend = modifiers.shift();
		// A step is relative to the current cell; an edge is an absolute end of the grid. Home
		// and End must be absolute — a relative jump reads the empty-selection default and would
		// land on the wrong end when nothing is selected yet (see `Files::jump_to_edge`).
		enum Nav {
			Step(isize),
			Edge(bool),
		}
		let (nav, extend) = match named {
			Named::ArrowRight => (Nav::Step(1), extend),
			Named::ArrowLeft => (Nav::Step(-1), extend),
			Named::ArrowDown => (Nav::Step(columns), extend),
			Named::ArrowUp => (Nav::Step(-columns), extend),
			// PageDown/PageUp are focus-gated to the pane, so they never fight the terminal's own
			// scrollback on the same keys (`scroll_motion`) — that fires only while the terminal
			// holds the keyboard.
			Named::PageDown => (Nav::Step(page), extend),
			Named::PageUp => (Nav::Step(-page), extend),
			// Home/End land on an absolute end, right even with nothing selected yet.
			Named::Home => (Nav::Edge(false), extend),
			Named::End => (Nav::Edge(true), extend),
			Named::Tab if modifiers.shift() => (Nav::Step(-1), false),
			Named::Tab => (Nav::Step(1), false),
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
			// F5 re-lists the directory on show, the same as the header ↻ button — the pane's
			// twin of the tree's F5, each refreshing the panel that holds the keyboard.
			Named::F5 => return self.on_files(FilesMessage::Refresh),
			Named::Escape => {
				self.set_focus(Focus::Terminal);
				return iced::Task::none();
			}
			_ => return iced::Task::none(),
		};

		let show_hidden = self.explorer.show_hidden();
		match nav {
			Nav::Step(delta) => self.files.step(show_hidden, delta, extend),
			Nav::Edge(to_last) => self.files.jump_to_edge(show_hidden, to_last, extend),
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
		let Some(rect) = self.files.band().map(files::Band::rect) else {
			return;
		};
		let Some(directory) = self.files.path().map(str::to_owned) else {
			return;
		};
		let rows = self.files.rows(self.explorer.show_hidden());
		let paths: Vec<String> = ui::files::band_hits(
			rect,
			ui::files::columns(self.files_width()),
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

	/// How wide the files pane is: the window less the folder tree's column beside it (§18, §19).
	/// The tree took its width off the terminal before; it takes it off the pane now, so every
	/// piece of the pane's geometry that keys off its width — the column count, the popup, the
	/// rubber band, the menus — reads this rather than the raw window width. `Explorer::reserved`
	/// is zero when the tree is hidden, so the pane is the full window then.
	fn files_width(&self) -> f32 {
		self.window_size.width - self.explorer.reserved()
	}

	/// Scroll the files pane so the selected cell is on screen (§20). The grid's geometry
	/// is the view's (`ui::files`), so the same arithmetic that lays the cells out is what
	/// works out where the selected one sits. The model is told the new offset as well as
	/// the widget, because the details popup is placed against it on this very frame.
	fn scroll_files_into_view(&mut self) -> iced::Task<Message> {
		let Some(index) = self.files.selected_index(self.explorer.show_hidden()) else {
			return iced::Task::none();
		};
		let row = index / ui::files::columns(self.files_width());
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
				self.files.height(),
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
		let screen = terminal.screen();
		let (rows, cols) = screen.size();
		let hovered = ui::terminal::cell_at(point, rows, cols);
		// The head is resolved to a DOCUMENT position here (§40), where the viewport's own numbers are
		// still to hand: the pointer is over a screen row, but what it selects is the line that row is
		// showing — so the selection keeps covering that text however the scrollback then moves.
		let head = hovered.spot(screen);
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
		let anchor = self.hover_cell.spot(terminal.screen());
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
		let start = ui::selection::Spot {
			line: span.start_line,
			col: 0,
		};
		let head = ui::selection::Spot {
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
		let needed = self.explorer.reread();
		self.list_dirs(needed);
		if let Some(request) = self.files.refresh() {
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
	fn on_identity_ready(&mut self, identity: u64) -> iced::Task<Message> {
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
		// A toast says why, without stealing the keyboard (§10). It used to go into the elevate
		// dialog when that attempt's dialog was still up; there is no dialog now.
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
	fn reveal_match(&mut self, found: Option<term::search::Match>) {
		let (Some(found), Some(terminal)) = (found, self.terminal.as_mut()) else {
			return;
		};
		if !terminal.reveal_line(found.line) {
			return;
		}
		let start = ui::selection::Spot {
			line: found.line,
			col: found.start_col,
		};
		let head = ui::selection::Spot {
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
		self.confirm_disconnect = false;
		self.transfer = None;
		self.upload_files.clear();
		self.upload_dir.clear();
		self.uploads.clear();
		self.uploaded = 0;
		self.upload_overwrite = false;
		self.upload_clash = None;
		self.transfer_notice = None;
		// A drag that was mid-hover when the session went is over with it (§29).
		self.drop_hover = false;
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
		// The carried directory is deliberately NOT cleared here (§52): this runs on the way INTO a
		// session as well as out of one, and a copy's whole point is to be spent by the connect that
		// is opening. It is taken by that connect, and what it is not spent on it is matched
		// against, so nothing stale can be replayed into a later session.
		// The panels' own size and visibility are user preferences, not session state,
		// so `reset` deliberately leaves those alone.
		self.explorer.reset();
		self.files.reset();
		// A session's forwards die with it (§27): the worker drops its listeners when the session
		// ends, so the list — and the open dialog — belong to this session and are cleared. A fresh
		// session re-establishes the target's saved set itself, after this runs.
		self.forwards.clear();
		self.forward_dialog = false;
		self.forward_error = None;
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
		// Files first, then the folders behind them (§29). Both queues share the one transfer slot,
		// so this is the only place that decides what runs next — and it drains the batch before
		// starting a tree, because the batch's collision question was answered up front (§17) while
		// a tree asks its own as it walks.
		if self.uploads.is_empty()
			&& let Some(local) = self.upload_trees.pop_front()
		{
			let dir = self.upload_dir.clone();
			self.start_upload_tree(Some(local), dir);
			return;
		}
		if let Some((local, remote)) = self.uploads.pop_front() {
			let total = std::fs::metadata(&local)
				.map(|meta| meta.len())
				.unwrap_or(0);
			// A fresh file starting means the previous transfer's resume offer, if any, is stale
			// (§16); remember this file so its own failure can be resumed.
			self.resumable = None;
			self.in_flight = Some(Resumable::Upload {
				local: local.clone(),
				remote: remote.clone(),
			});
			if self.send_command(SshCommand::Upload {
				local,
				remote,
				overwrite: self.upload_overwrite,
				resume: false,
			}) {
				self.transfer = Some(TransferState::Running { sent: 0, total });
			} else {
				self.transfer = None;
				self.in_flight = None;
			}
		}
	}

	/// Close a batch once it has fully drained (§17): no transfer running and nothing left in
	/// the queue. Clears the picked files (which disables the Upload button) and the folder,
	/// so a stray click cannot re-send what just landed. The closing notice is set by the
	/// caller that noticed the last file land.
	fn finish_batch_if_drained(&mut self) {
		if self.transfer.is_none() && self.uploads.is_empty() && self.upload_trees.is_empty() {
			self.finish_batch();
		}
	}

	/// Drop the finished batch's leftovers (§17), keeping whatever notice is showing.
	fn finish_batch(&mut self) {
		self.upload_files.clear();
		self.upload_dir.clear();
		self.uploads.clear();
		self.upload_trees.clear();
		self.uploaded = 0;
		self.uploaded_trees = 0;
		self.upload_overwrite = false;
	}

	/// Back out of the upload flow before or during a batch (§17): a cancelled confirmation
	/// or collision question, or Esc. Drops everything pending so nothing is sent; a transfer
	/// already in flight is left to finish, since its bytes are already on the wire.
	fn cancel_upload(&mut self) {
		self.upload_clash = None;
		self.uploads.clear();
		self.upload_trees.clear();
		self.uploaded = 0;
		self.uploaded_trees = 0;
		self.upload_overwrite = false;
		self.upload_files.clear();
		self.upload_dir.clear();
		if matches!(self.transfer, Some(TransferState::ConfirmPath)) {
			self.transfer = None;
		}
	}

	/// Stop the transfer running right now (§16) — the status bar's ✕. Empties both queues and
	/// forgets any resume point, since a deliberate cancel is final and takes the whole batch with
	/// it, then tells the worker to stop: its copy loop deletes the partial it was writing and
	/// reports the neutral "cancelled" outcome, which clears the bar and, the queues now empty,
	/// closes the batch out. `transfer` is left running until that outcome lands, so the bar does
	/// not flicker between the click and the worker winding down.
	fn cancel_transfer(&mut self) {
		self.uploads.clear();
		// The folders queued behind this transfer go with it: a deliberate cancel takes the whole
		// drop, not just the item on the wire (§16, §29).
		self.upload_trees.clear();
		self.downloads.clear();
		self.uploaded = 0;
		self.uploaded_trees = 0;
		self.downloaded = 0;
		self.resumable = None;
		self.in_flight = None;
		self.send_command(SshCommand::CancelTransfer);
	}

	/// Pick up a transfer that a failure interrupted (§16) — the status bar's Resume. Relaunches
	/// the exact command `resumable` remembers with `resume` set, so the task sizes the destination
	/// and sends only the bytes still missing; a single file left in a batch drains the rest once
	/// it lands. Does nothing if there is nothing to resume, or if the session has since gone.
	fn resume_transfer(&mut self) -> iced::Task<Message> {
		let Some(resumable) = self.resumable.take() else {
			return iced::Task::none();
		};
		// Mirror what a fresh start records, so this resumed transfer is itself resumable if it too
		// is interrupted (a flaky link may need more than one nudge).
		self.in_flight = Some(resumable.clone());
		let command = match resumable {
			Resumable::Upload { local, remote } => SshCommand::Upload {
				local,
				remote,
				// The partial is our own earlier work, not a clash, so skip the exists check and
				// go straight to the appending copy.
				overwrite: true,
				resume: true,
			},
			Resumable::Download { remote, local } => SshCommand::Download {
				remote,
				local,
				resume: true,
			},
			Resumable::UploadTree { local, remote } => SshCommand::UploadTree {
				local,
				remote,
				resume: true,
			},
			Resumable::DownloadTree { remote, local } => SshCommand::DownloadTree {
				remote,
				local,
				resume: true,
			},
		};
		if self.send_command(command) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		} else {
			self.in_flight = None;
		}
		iced::Task::none()
	}

	/// A drop landed and the frame after it has come round, so the whole set of paths is in hand
	/// (§29). Send it into the files pane's current directory, reusing the upload pipeline whole —
	/// the destination pre-scan and, on a name already taken, the same Overwrite / Keep both / Skip
	/// / Cancel dialog a menu upload opens (§17). The drop already said where the bytes go, so there
	/// is no destination confirmation; it goes straight to the pre-scan.
	///
	/// Waiting a frame is what makes a multi-file drop one batch: the OS reports each path as its
	/// own event and never says which is the last, so deciding on the first would send one file and
	/// then decline its own siblings as "a transfer is already running".
	fn on_drop_settled(&mut self) -> iced::Task<Message> {
		let dropped = std::mem::take(&mut self.dropped);
		if dropped.is_empty() {
			return iced::Task::none();
		}
		// A folder needs the tree flow and a file the batch flow (§17), so the drop is sorted into
		// its two kinds here — both are sent, one after the other, through the single transfer slot.
		let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
			dropped.into_iter().partition(|path| path.is_dir());
		// A batch already set up or running would fight over the one progress bar. `upload_files`
		// being non-empty catches a menu upload waiting on its confirmation, too: one flow at a time.
		let busy =
			self.transfer.is_some() || !self.uploads.is_empty() || !self.upload_files.is_empty();
		match drop_outcome(
			self.terminal.is_some(),
			busy,
			folders.len() + files.len(),
			self.files.path(),
		) {
			// No session (or not the terminal screen): nowhere to send, so say nothing.
			DropOutcome::Ignore => iced::Task::none(),
			DropOutcome::Busy => {
				self.transfer_notice = Some("A transfer is already running.".to_owned());
				iced::Task::none()
			}
			DropOutcome::NoDir => {
				self.transfer_notice = Some("Open a folder in the files pane first.".to_owned());
				iced::Task::none()
			}
			DropOutcome::Upload(dir) => {
				self.upload_dir = dir;
				self.transfer_notice = None;
				// Every folder queues, each to go tree-and-all exactly as the menu's "Upload
				// folder…" does (§17) — the same command, the same per-file collision questions,
				// the same resume. `pump_uploads` starts them once the files are through.
				self.upload_trees = folders.into();
				if files.is_empty() {
					// Folders only: there is no batch to pre-scan, so the first tree starts here.
					self.pump_uploads();
					return iced::Task::none();
				}
				// Seed the batch and run the ordinary confirmed-upload path: it pre-scans the
				// destination, then either sends or opens the collision dialog (§17). One file or
				// twenty, this is the same flow the picker's own selection takes.
				self.upload_files = files;
				self.on_upload_confirmed()
			}
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
			// The pane always knows its sort (both halves may be unset), so it is always `Some`
			// here — `set_session` then writes the tri-state through as-is (§19, §22).
			sort: Some(self.files.sort_key()),
			sort_dir: Some(self.files.sort_dir()),
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
		// The remembered sort goes straight onto the pane model, so the grid reopens in the order
		// this target was left in (§22). Both halves travel together from `capture_session`, so
		// they are applied together; `set_sort` writes the tri-state outright rather than toggling.
		if let (Some(sort), Some(sort_dir)) = (session.sort, session.sort_dir) {
			self.files.set_sort(sort, sort_dir);
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
			ExplorerMessage::RefreshDir(path) => {
				self.explorer.close_menu();
				// The menu's "Refresh" answers "is this folder still here, under this name, holding
				// these children?" Its CONTENTS come from re-listing the folder itself (forced open,
				// so the result shows at once); its own NAME and EXISTENCE come from re-listing its
				// PARENT — a rename or deletion made from the shell surfaces in the parent's listing,
				// never the folder's. The root has no parent, so only its contents refresh.
				if let Some(parent) = explorer::parent(&path).map(str::to_owned)
					&& let Some(fetch) = self.explorer.refresh_dir(&parent)
				{
					self.send_command(SshCommand::ListDir(fetch));
				}
				if let Some(fetch) = self.explorer.expand(&path, true) {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::RefreshTree => {
				// The header ↻ button and F5: re-list every open folder, so all the expanded
				// content is current in one action — the user never has to work out which folders
				// a move touched. Each becomes its own listing request.
				self.explorer.close_menu();
				for fetch in self.explorer.refresh_open() {
					self.send_command(SshCommand::ListDir(fetch));
				}
			}
			ExplorerMessage::CollapseAll => {
				// The header's collapse-all button: close every branch back to the root's own
				// children. Local state only — nothing is re-fetched — so this needs no command.
				self.explorer.close_menu();
				self.explorer.collapse_all();
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
			// Hover only lights the bar (§18); no relayout, so no grid refit.
			ExplorerMessage::SplitterEntered => self.explorer.set_splitter_hovered(true),
			ExplorerMessage::SplitterExited => self.explorer.set_splitter_hovered(false),
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
	/// Three things happen, and all three are the point:
	///
	/// * the tree opens the chain down to the cwd and selects it — through `Explorer::reveal`, the
	///   UNguarded one, since the whole reason to press this is that the tree has been walked away
	///   from a cwd that never changed;
	/// * the pane shows that directory (`Files::show`, the deliberate move, not `follow`); and
	/// * the follow-guard is seeded with the same path, so the next prompt's announcement is
	///   correctly read as "still there, nothing to do" rather than as a move — and a real `cd`
	///   after it still carries the pane along.
	///
	/// A no-op when the shell has never announced a cwd (§17: it needs OSC 7, or a shell configured
	/// to send it) — the button dims then, and whenever the panes are already there.
	fn on_reveal(&mut self) {
		let Some(cwd) = self
			.terminal
			.as_ref()
			.and_then(term::Terminal::cwd)
			.map(str::to_owned)
		else {
			return;
		};
		let needed = self.explorer.reveal(&cwd);
		self.list_dirs(needed);
		self.files.set_followed(&cwd);
		if let Some(request) = self.files.show(&cwd) {
			self.list_files(request);
		}
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
				// Window coordinates from the capture layer: the pane's left edge is the window's
				// and it runs to the bottom, so only the vertical origin — the strip's top — comes off.
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
				// A directory is entered — browsing the PANE there, the console left where it is
				// (§19). A FILE opens in a new editor tab (§32). The console is moved on purpose, by
				// Sync or "Open in terminal", never as a side effect of either.
				match self.files.kind_of(&path) {
					Some(files::Kind::Dir) => self.browse_to(&path),
					Some(_) => return self.request_edit(path),
					None => {}
				}
			}
			FilesMessage::EditStarted(path) => {
				// The menu's "Edit…" — the deliberate twin of a file double-click (§32).
				self.files.close_menu();
				return self.request_edit(path);
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
			// The sort menu is a plain view preference: none of these re-list or re-fetch, they
			// only re-order what `rows` already holds, so each just mutates and falls through to
			// the shared `Task::none()` below (§19).
			FilesMessage::SortMenuOpened => self.files.toggle_sort_menu(),
			FilesMessage::SortMenuDismissed => self.files.close_sort_menu(),
			// Picking a key or a direction leaves the menu open, so both halves of a sort can be
			// set in one visit; a click-away (or the button) closes it. Each pick persists the sort
			// into the connected target (§22), the same way the `.*` toggle folds into the snapshot,
			// so the chosen order survives a disconnect and even a later hard exit.
			FilesMessage::SortKeyPicked(key) => {
				self.files.pick_sort_key(key);
				self.persist_session();
			}
			FilesMessage::SortDirPicked(dir) => {
				self.files.pick_sort_dir(dir);
				self.persist_session();
			}
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
			// Hover only lights the bar (§19); no relayout, so no grid refit.
			FilesMessage::SplitterEntered => self.files.set_splitter_hovered(true),
			FilesMessage::SplitterExited => self.files.set_splitter_hovered(false),
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
		self.resumable = None;
		self.in_flight = Some(Resumable::Download {
			remote: remote.clone(),
			local: local.clone(),
		});
		if self.send_command(SshCommand::Download {
			remote,
			local,
			resume: false,
		}) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		} else {
			self.in_flight = None;
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
		self.resumable = None;
		self.in_flight = Some(Resumable::UploadTree {
			local: local.clone(),
			remote: dir.clone(),
		});
		if self.send_command(SshCommand::UploadTree {
			local,
			remote: dir.clone(),
			resume: false,
		}) {
			self.transfer_notice = None;
			// Remembered so completion re-lists this folder if the pane is on it (§29) — the same
			// refresh a single-file upload gets. The tree flow keeps no queue, so this is the only
			// reader of `upload_dir` for it, and `finish_batch` clears it at the end.
			self.upload_dir = dir;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		} else {
			self.in_flight = None;
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
		self.resumable = None;
		self.in_flight = Some(Resumable::DownloadTree {
			remote: remote.clone(),
			local: local.clone(),
		});
		if self.send_command(SshCommand::DownloadTree {
			remote,
			local,
			resume: false,
		}) {
			self.transfer_notice = None;
			self.transfer = Some(TransferState::Running { sent: 0, total: 0 });
		} else {
			self.in_flight = None;
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
		// The one floating card every dialog on this screen is placed by (§10) — only one is ever
		// open at a time, so one card serves them all; the arms that draw no dialog ignore it.
		let card = self.card;
		match &self.screen {
			// The shared target list is read through a short-lived borrow; `home::view` clones
			// every name it needs, so nothing in the returned element outlives the borrow (§26).
			Screen::Home => ui::home::view(
				self.targets.borrow().items(),
				ui::home::View {
					filter: &self.home_filter,
					selected: self.home_selected.as_deref(),
					rename: self.home_rename.as_ref(),
					menu_open: self.home_menu_open,
					confirm_delete: self.confirm_delete,
					dialog_body: &self.dialog_body,
					card,
				},
			),
			Screen::Connect => ui::connect::view(&self.form, self.form_focus),
			Screen::Connecting { status } => text(status).into(),
			// The connect-flow dialogs float over the (dimmed) form rather than replacing
			// it, so the page stays in view behind them (§10). A click on the backdrop
			// dismisses with the dialog's own safe action (reject / cancel / back).
			Screen::ConfirmHostKey => self.form_with_dialog(
				ui::host_key_view(&self.dialog_body, card),
				Message::RejectHostKey,
			),
			// The mismatch override dialog, over the same dimmed form. Dismissing rejects — the
			// safe default — so a backdrop click never trusts a changed key (§8).
			Screen::HostKeyChanged => self.form_with_dialog(
				ui::host_key_changed_view(&self.dialog_body, card),
				Message::RejectHostKey,
			),
			Screen::NeedPassphrase => self.form_with_dialog(
				ui::passphrase_view(
					&self.passphrase_input,
					self.passphrase_failed,
					&self.dialog_body,
					card,
				),
				Message::PassphraseCancelled,
			),
			Screen::Interactive => self.form_with_dialog(
				ui::interactive_view(
					&self.interactive_prompts,
					&self.interactive_answers,
					&self.dialog_body,
					card,
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
					card,
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
							forwards: ui::forward::ForwardsView {
								open: self.forward_dialog,
								entries: &self.forwards,
								kind: self.forward_kind,
								listen: &self.forward_listen,
								to: &self.forward_to,
								error: self.forward_error.as_deref(),
							},
							search: self.search.as_ref(),
							body: &self.dialog_body,
							card,
						},
						ui::terminal::UploadView {
							file_count: self.upload_files.len(),
							first_file: self.upload_files.first().map(|local| file_name_of(local)),
							dest: &self.upload_dir,
							state: self.transfer,
							notice: self.transfer_notice.as_deref(),
							resumable: self.resumable.is_some(),
						},
						ui::terminal::Panels {
							explorer: &self.explorer,
							files: &self.files,
							focus: self.focus,
							// The pane's width (the window less the tree's column beside it), which is
							// what its grid wraps at and its overlays are placed against (§18, §19).
							width: self.files_width(),
							height: self.window_size.height,
							drop_hover: self.drop_hover,
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
			// The in-tab editor (§32): its whole screen — toolbar, gutter, buffer — comes from
			// `ui::editor`, which borrows the buffer in place, so nothing outlives this frame.
			Screen::Editor => match &self.editor {
				Some(editor) => ui::editor::view(editor, self.id),
				None => text("editor starting…").into(),
			},
			Screen::Error => self.form_with_dialog(
				ui::error_view(&self.dialog_body, card),
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
		ui::terminal::CELL_WIDTH.round() as u16,
		ui::terminal::CELL_HEIGHT.round() as u16,
	);
	terminal
}

fn fit_terminal() -> iced::Task<Message> {
	iced::window::latest().and_then(|id| iced::window::size(id).map(Message::WindowResized))
}

/// Hand the window itself to `cursor`, once, at start-up (§51).
///
/// The hands are painted through a Win32 window subclass, so the one thing that layer needs is the
/// window's own handle — and `iced::window::run` is the only way iced offers to reach it: the
/// closure is handed the live window on the UI thread, which is also the thread that pumps its
/// messages, so the subclass is installed from the right place.
///
/// `discard` because the installation raises no message: everything after it is driven by the tab
/// strip's own pointer events. Off Windows this resolves to a no-op that costs one boot task.
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

/// The prompt-jump direction a Ctrl+Shift+arrow asks for, or `None` for any other key (§34). Up
/// climbs to the previous prompt, Down returns toward the live one — the direction the arrow
/// itself points through the scrollback.
fn prompt_jump(named: &iced::keyboard::key::Named) -> Option<term::osc133::Direction> {
	use iced::keyboard::key::Named;
	match named {
		Named::ArrowUp => Some(term::osc133::Direction::Previous),
		Named::ArrowDown => Some(term::osc133::Direction::Next),
		_ => None,
	}
}

/// Whether this key press is plain TYPING — a character meant to appear at a prompt — rather
/// than a shortcut or a navigation key (§50). This is what decides that a keystroke aimed at a
/// focused panel was really meant for the shell.
///
/// Two conditions, and both are needed:
///   * a `Character` key, never a `Named` one. Enter, Tab, the arrows, F2, Esc, Backspace and
///     Delete are all `Named`, and every one of them is a panel's own key (§20) — a rule written
///     on the produced `text` instead would catch Enter (which carries `"\r"`) and take the
///     folder tree's "send the shell there" away from it;
///   * no Ctrl, Alt or Logo. Those make a combination, not a character: the files pane's Ctrl+A
///     takes the whole listing (§21), and Ctrl+Tab is the way out of a panel at all.
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
		// The promptless methods carry no secret to remember — interactive answers every factor
		// live, and agent auth signs with a key the agent holds; neither has anything to store (§7).
		bridge::AuthMethod::Interactive | bridge::AuthMethod::Agent => return None,
	};
	if secret.expose().is_empty() {
		None
	} else {
		Some(secret.clone())
	}
}

/// What a drop onto the window should do (§29). Split out of `on_drop_settled` so the decision —
/// is there a session, is a transfer busy, is there anything to send, is there a directory to land
/// in — is pure and testable, the way `plan_uploads` and `band_hits` are.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DropOutcome {
	/// No live session (or not on the terminal), or nothing droppable at all: ignore the drop
	/// silently — with nowhere to send, there is nothing to tell the user.
	Ignore,
	/// A transfer is already running or a batch is being set up: decline, one flow at a time.
	Busy,
	/// The files pane has no directory yet, so there is nowhere to drop into.
	NoDir,
	/// Upload everything dropped into this remote directory — the pane's own. What is a file and
	/// what is a folder no longer changes the answer: files go as one batch and each folder goes
	/// tree-and-all, all of them queued behind the single transfer slot (§29).
	Upload(String),
}

/// Decide a drop's fate from the state it depends on (§29), free of `self` so it is tested on its
/// own. `items` is a count rather than the paths themselves, because whether there is anything at
/// all is the whole of what this decides — sorting files from folders is the caller's business.
///
/// The order is deliberate. No session outranks everything, so a drop onto a home tab is silent
/// whatever it held. Then a busy transfer, since nothing could start whatever the drop was. Then an
/// empty drop, which is silent for the same reason a drop with no session is. Only then does the
/// destination decide between `NoDir` and a real upload.
fn drop_outcome(connected: bool, busy: bool, items: usize, pane_dir: Option<&str>) -> DropOutcome {
	if !connected {
		return DropOutcome::Ignore;
	}
	if busy {
		return DropOutcome::Busy;
	}
	// Nothing to do, and nothing worth saying: a drop of nothing at all is not a mistake the user
	// made. (Reachable only if every dropped path vanished between the drop and this frame.)
	if items == 0 {
		return DropOutcome::Ignore;
	}
	match pane_dir {
		Some(dir) => DropOutcome::Upload(dir.to_owned()),
		None => DropOutcome::NoDir,
	}
}

/// The closing notice for an upload that has fully drained (§17, §29), from what actually landed.
/// Pure so the wording is testable — it is the one line a user reads to know a drop of several
/// things did all of them.
///
/// `last` is the path of the item that finished last, named only when it is the ONLY thing that
/// went: with one file, "Uploaded to /srv/notes.txt" says more than "Uploaded 1 file". Past that
/// the counts carry it, and a mixed drop names both kinds rather than adding them up into a total
/// of nothing in particular.
fn upload_summary(files: usize, folders: usize, last: &str) -> String {
	let files_part = |count: usize| {
		if count == 1 {
			"1 file".to_owned()
		} else {
			format!("{count} files")
		}
	};
	let folders_part = |count: usize| {
		if count == 1 {
			"1 folder".to_owned()
		} else {
			format!("{count} folders")
		}
	};
	match (files, folders) {
		// One thing on its own — the path is the most useful thing to show.
		(1, 0) | (0, 1) => format!("Uploaded to {last}"),
		(0, folders) => format!("Uploaded {}", folders_part(folders)),
		(files, 0) => format!("Uploaded {}", files_part(files)),
		(files, folders) => format!(
			"Uploaded {} and {}",
			files_part(files),
			folders_part(folders)
		),
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
	let offset = keep_visible(editor.scroll(), view_height, top, ui::editor::LINE_HEIGHT);
	if offset == editor.scroll() {
		return None;
	}
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
	);
	if offset == editor.scroll_x() {
		return None;
	}
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

/// The `scroll_to` task that moves the editor buffer to `(x, y)` (§32) — the operation the panels use
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

	// The next command queued for the SSH worker, or `None` if nothing was sent. Broader than
	// `next_input`, since the forward flow sends `AddForward` / `RemoveForward`, not `Input`.
	fn next_command(rx: &mut mpsc::Receiver<SshCommand>) -> Option<SshCommand> {
		rx.try_recv().ok()
	}

	/// Adding a forward from the dialog parses the two fields, queues the entry as `Starting`,
	/// sends the worker an `AddForward`, and clears the fields for the next one (§27).
	#[test]
	fn adding_a_forward_parses_queues_and_sends_it() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.forward_kind = crate::forward::ForwardKind::Local;
		app.forward_listen = "8080".to_owned();
		app.forward_to = "db:5432".to_owned();

		app.add_forward();

		// Queued once, marked starting, and the input fields reset (the kind is kept).
		assert_eq!(app.forwards.len(), 1);
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Starting
		);
		assert!(app.forward_listen.is_empty());
		assert!(app.forward_to.is_empty());
		assert!(app.forward_error.is_none());

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
		app.forward_listen = "not-a-port".to_owned();
		app.forward_to = "db:5432".to_owned();

		app.add_forward();

		assert!(app.forwards.is_empty());
		assert!(app.forward_error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Two forwards cannot bind the same endpoint: the duplicate is refused before it is sent,
	/// so the second one's inevitable bind failure never happens (§27).
	#[test]
	fn a_duplicate_bind_is_refused() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.forward_kind = crate::forward::ForwardKind::Local;
		app.forward_listen = "8080".to_owned();
		app.forward_to = "a:1".to_owned();
		app.add_forward();
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::AddForward { .. })
		));

		// Same bind, different target: rejected, nothing added, nothing sent.
		app.forward_listen = "8080".to_owned();
		app.forward_to = "b:2".to_owned();
		app.add_forward();
		assert_eq!(app.forwards.len(), 1);
		assert!(app.forward_error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Removing a forward drops its row and asks the worker to tear it down (§27).
	#[test]
	fn removing_a_forward_drops_it_and_sends_remove() {
		let (mut app, mut rx) = app_with_terminal(16);
		app.forward_kind = crate::forward::ForwardKind::Dynamic;
		app.forward_listen = "1080".to_owned();
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
		app.forward_kind = crate::forward::ForwardKind::Local;
		app.forward_listen = "8080".to_owned();
		app.forward_to = "db:5432".to_owned();
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
		app.forward_kind = crate::forward::ForwardKind::Remote;
		app.forward_listen = "0".to_owned();
		app.forward_to = "localhost:3000".to_owned();
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
		app.forward_kind = crate::forward::ForwardKind::Local;
		app.forward_listen = "8080".to_owned();
		app.forward_to = "db:5432".to_owned();
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

	/// Typing while a side panel holds the keyboard hands it back to the shell, and the letter
	/// that did it goes down the channel rather than being spent on the switch (§50). Without
	/// this the panel swallowed it and the first character of a command vanished.
	#[test]
	fn typing_while_a_panel_has_the_keyboard_gives_it_to_the_shell() {
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

	/// A navigation key is the panel's own, so it keeps both the key and the keyboard (§20, §50).
	/// This is the half of the rule that makes the other half safe: walking a tree with the arrows
	/// must not read as typing at the prompt.
	#[test]
	fn an_arrow_while_a_panel_has_the_keyboard_stays_with_the_panel() {
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
	/// and takes it back with it (§50). It used to be dropped: the panel swallowed it and the
	/// paste never happened, with nothing on screen to say why.
	#[test]
	fn ctrl_v_pastes_from_a_panel_and_takes_the_keyboard_back() {
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
	/// interrupt for the remote — neither is text going in — so a panel holding the ring keeps it.
	#[test]
	fn ctrl_c_does_not_take_the_keyboard_from_a_panel() {
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

	/// A command from the terminal's own surface — here Paste, off the grid's right-click menu —
	/// puts the keyboard back on the shell (§50). Pasting a command while a panel held the focus
	/// used to leave the Enter that runs it going to the panel.
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
		app.hover_cell = ui::selection::Cell { row: 0, col: 0 };
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
		app.hover_cell = ui::selection::Cell { row: 0, col: 6 };

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
		let reserved = app.files.reserved();
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
			ui::selection::Spot { line: 0, col: 0 },
			ui::selection::Spot { line: 0, col: 4 },
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
		let reserved = app.files.reserved();
		app.window_size = ui::terminal::window_size(80, 24, reserved);
		app.terminal.as_mut().unwrap().process(b"cat /etc/hosts");

		// Clear of the left gutter, so this is an ordinary grid press and not a prompt tick (§34).
		app.pointer = iced::Point::new(50.0, 5.0);
		app.hover_cell = ui::selection::Cell { row: 0, col: 6 };
		app.on_grid_pressed();

		app.on_window_resized(ui::terminal::window_size(60, 24, reserved));

		// The pointer never moved, so the tally's cell is the one this press lands on — the second
		// press would take the word if the resize had not reset the count.
		assert_eq!(
			app.hover_cell,
			ui::selection::Cell { row: 0, col: 6 },
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
		app.screen = Screen::Terminal;
		app.identities = vec![Identity {
			id: bridge::LOGIN_IDENTITY,
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
			ready: false,
			work: Workspace::default(),
		});
		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity: u64::MAX, // a stray event for nothing, to prove it disturbs nothing
			reason: None,
		});
		let _task = app.on_ssh_event(SshEvent::IdentityReady { identity: id });
		id
	}

	/// Switching accounts moves the FILE panes too (§46), and reads them again as the account now
	/// selected: the path stays — elevating because a folder would not open is the ordinary reason to
	/// do it — but nothing another account listed is left on screen while the new listing is awaited.
	#[test]
	fn switching_accounts_reads_the_file_panes_again_as_the_new_account() {
		let (mut app, mut rx) = app_with_login_identity();
		// A tree with a listed, open folder and a pane showing it — `cme`'s view of /etc.
		let _fetch = app.explorer.expand("/etc", false);
		app.explorer.listed("/etc", vec!["ssl".to_owned()]);
		if let Some(request) = app.files.show("/etc") {
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
			app.explorer.rows().iter().all(|row| row.path != "/etc/ssl"),
			"another account's children must not survive the switch"
		);
		assert_eq!(app.files.count(), 0, "nor its files");

		// And it happens in both directions: going back to `cme` re-reads what root had listed.
		app.explorer.listed("/etc", vec!["shadow.d".to_owned()]);
		let _task = app.switch_identity(bridge::LOGIN_IDENTITY);
		let back = drain(&mut rx);
		assert!(
			back.iter()
				.any(|command| matches!(command, SshCommand::ListDir(path) if path == "/etc")),
			"the folder is read again as the login account too"
		);
		assert!(
			app.explorer
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

		let _task = app.open_editor(app.focus, id, "/root/.ssh/authorized_keys".to_owned());
		let editor = app
			.tabs()
			.find_map(|tab| tab.editor.as_ref())
			.expect("the editor tab is open");
		assert_eq!(editor.identity, root, "opened as the account on screen");

		// The session goes back to `cme` while the file is still open, and the save still names root.
		let editor_id = app
			.tabs()
			.find(|tab| tab.editor.is_some())
			.map(|tab| tab.id)
			.expect("the editor tab has an id");
		if let Some(tab) = app.tab_mut(id) {
			let _task = tab.switch_identity(bridge::LOGIN_IDENTITY);
		}
		let mut rx = rx;
		let _drained = drain(&mut rx);
		let _task = app.flush_editor_save(editor_id);

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
			app.search.as_ref().map(|search| search.count()),
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
			ready: false,
			work: Workspace::default(),
		});

		// The flush, arriving while root is still off screen and has no terminal of its own.
		let _task = app.on_ssh_event(SshEvent::Output {
			identity: root,
			bytes: b"root@rec:~# ".to_vec(),
		});
		let _task = app.on_ssh_event(SshEvent::IdentityReady { identity: root });
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
			matches!(app.screen, Screen::Terminal),
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
			prompt_jump(&Named::ArrowUp),
			Some(term::osc133::Direction::Previous)
		);
		assert_eq!(
			prompt_jump(&Named::ArrowDown),
			Some(term::osc133::Direction::Next)
		);
		assert_eq!(prompt_jump(&Named::ArrowLeft), None);
		assert_eq!(prompt_jump(&Named::PageUp), None);
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
			crate::profiles::SessionState {
				terminal_path: Some("/var/log".to_owned()),
				files_path: Some("/etc".to_owned()),
				..crate::profiles::SessionState::default()
			},
		);
		app.connection = Some("u@h:22".to_owned());
		app.pending_target = Some(app.targets.borrow().find("u@h:22").unwrap().clone());

		// One OSC 7 cwd announcement, as the shell emits on each prompt (§17).
		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

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

	/// The status bar's Reveal button (§19): the panes come to the shell, and nothing is typed at
	/// it. The case that matters is the one the shell cannot fix by itself — a browse away from a
	/// shell that has not moved since. Its next prompt announces the same directory, which is not a
	/// move, so the pane rightly stays put and only an explicit ask brings it back.
	#[test]
	fn reveal_brings_the_panes_to_the_shell_without_typing_anything() {
		let (mut app, mut rx) = app_with_terminal(32);
		let announce = |dir: &str| shell_output(format!("\x1b]7;file://host{dir}\x07").as_bytes());

		// The shell says where it is, and both panels follow it there as usual.
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(app.files.path(), Some("/var/log"));
		assert_eq!(app.explorer.selected(), Some("/var/log"));

		// A look somewhere else, with the tree walked off the shell's folder too.
		app.browse_to("/etc");
		app.explorer.select("/etc");
		let _ = app.on_ssh_event(announce("/var/log"));
		assert_eq!(
			app.files.path(),
			Some("/etc"),
			"a re-announcement is not a move, so the browse stands (§19)"
		);

		let _ = drain(&mut rx);
		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.files.path(), Some("/var/log"), "the pane came back");
		assert_eq!(
			app.explorer.selected(),
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
	/// nowhere to go, so it leaves both panels where they are rather than guessing at the root.
	/// The button dims in that case; this is what sits behind the dimming.
	#[test]
	fn reveal_does_nothing_when_the_shell_never_said_where_it_is() {
		let (mut app, mut rx) = app_with_terminal(32);
		app.browse_to("/etc");
		let _ = drain(&mut rx);

		let _task = app.update(Message::RevealPressed);
		assert_eq!(app.files.path(), Some("/etc"), "left where it was");
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
		let chosen = |app: &Tab| {
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

	#[test]
	fn the_paths_of_one_drop_gather_until_the_frame_reads_them() {
		// The OS reports a multi-file drop as one event per path and never says which is the last
		// (§29), so each event only gathers. The frame that follows takes the lot and leaves nothing
		// behind — otherwise the next drop would inherit these paths and upload them again.
		let mut tab = Tab::default();
		let _ = tab.update(Message::FileDropped(PathBuf::from("/local/a.txt")));
		let _ = tab.update(Message::FileDropped(PathBuf::from("/local/b.txt")));
		assert_eq!(tab.dropped.len(), 2, "both paths waited for the frame");
		// No session here, so the decision itself is `Ignore` — what this pins is that the settle
		// consumes the set either way.
		let _ = tab.update(Message::FileDropSettled);
		assert!(tab.dropped.is_empty());
	}

	#[test]
	fn a_drop_puts_the_target_highlight_out() {
		// The drag is over the moment a path lands, whatever is then decided about it (§29).
		let mut tab = Tab::default();
		let _ = tab.update(Message::FileHovered);
		let _ = tab.update(Message::FileDropped(PathBuf::from("/local/a.txt")));
		assert!(!tab.drop_hover);
	}

	#[test]
	fn a_dropped_file_uploads_into_the_pane_directory() {
		// A live session, nothing transferring, one plain file, and the pane showing a folder: the
		// drop uploads into that folder.
		let outcome = drop_outcome(true, false, 1, Some("/home/user"));
		assert_eq!(outcome, DropOutcome::Upload("/home/user".to_owned()));
	}

	#[test]
	fn a_drop_of_many_things_of_either_kind_is_accepted_whole() {
		// Files, folders, or both together: the pane's directory takes the lot (§29). What each
		// path IS decides which queue it joins, not whether the drop is allowed at all.
		for items in [1, 7, 40] {
			assert_eq!(
				drop_outcome(true, false, items, Some("/home/user")),
				DropOutcome::Upload("/home/user".to_owned())
			);
		}
	}

	#[test]
	fn a_drop_with_no_session_is_ignored() {
		// No session outranks every other rule: with nowhere to send, the drop is silent rather
		// than a notice about something that could never have uploaded.
		assert_eq!(
			drop_outcome(false, false, 1, Some("/home/user")),
			DropOutcome::Ignore
		);
	}

	#[test]
	fn a_drop_while_busy_is_declined() {
		// A transfer in flight (or a batch being set up) declines the drop whatever it held — the
		// one progress bar cannot serve two flows at once (§17).
		assert_eq!(
			drop_outcome(true, true, 1, Some("/home/user")),
			DropOutcome::Busy
		);
		assert_eq!(
			drop_outcome(true, true, 6, Some("/home/user")),
			DropOutcome::Busy
		);
	}

	#[test]
	fn a_drop_with_no_pane_directory_has_nowhere_to_land() {
		// Connected and idle, a plain file, but the pane has listed nothing yet: there is no folder
		// to drop into, so the user is told to open one rather than the file landing on a guess.
		assert_eq!(drop_outcome(true, false, 1, None), DropOutcome::NoDir);
	}

	#[test]
	fn a_drop_that_held_nothing_says_nothing() {
		// Not a mistake the user made — every dropped path would have had to vanish between the drop
		// and the frame that reads it — so it is silent rather than a notice about an empty drop.
		assert_eq!(
			drop_outcome(true, false, 0, Some("/home/user")),
			DropOutcome::Ignore
		);
	}

	#[test]
	fn the_files_of_a_drop_go_before_its_folders() {
		// One queue each, one transfer slot between them (§29). The batch drains first, because its
		// collision question was answered up front (§17) while a tree asks its own as it walks —
		// so the folder is still waiting when the file starts.
		let mut tab = Tab {
			upload_dir: "/srv".to_owned(),
			..Tab::default()
		};
		tab.uploads
			.push_back((PathBuf::from("/local/a.txt"), "/srv/a.txt".to_owned()));
		tab.upload_trees.push_back(PathBuf::from("/local/photos"));
		tab.pump_uploads();
		assert!(tab.uploads.is_empty(), "the file was taken first");
		assert_eq!(tab.upload_trees.len(), 1, "the folder is still queued");
	}

	#[test]
	fn the_folders_of_a_drop_are_started_one_after_another() {
		// With the files through, the pump reaches the folder queue — and takes one at a time, so
		// the second waits for the first to report back rather than racing it (§29). There is no
		// session here, so each send fails and frees the slot immediately; what is pinned is that
		// the queue is walked at all, and one item per pump.
		let mut tab = Tab {
			upload_dir: "/srv".to_owned(),
			..Tab::default()
		};
		tab.upload_trees.push_back(PathBuf::from("/local/one"));
		tab.upload_trees.push_back(PathBuf::from("/local/two"));
		tab.pump_uploads();
		assert_eq!(tab.upload_trees.len(), 1);
		tab.pump_uploads();
		assert!(tab.upload_trees.is_empty());
	}

	#[test]
	fn a_batch_does_not_close_while_folders_are_still_queued() {
		// `finish_batch` clears the destination every queued folder is going to, so closing early
		// would strand them (§29).
		let mut tab = Tab {
			upload_dir: "/srv".to_owned(),
			..Tab::default()
		};
		tab.upload_trees.push_back(PathBuf::from("/local/photos"));
		tab.finish_batch_if_drained();
		assert_eq!(tab.upload_dir, "/srv");
		assert_eq!(tab.upload_trees.len(), 1);
	}

	#[test]
	fn one_thing_uploaded_is_named_by_its_path() {
		// With a single item the path says more than a count does — "Uploaded 1 file" tells the user
		// nothing they did not just watch happen (§29).
		assert_eq!(
			upload_summary(1, 0, "/srv/notes.txt"),
			"Uploaded to /srv/notes.txt"
		);
		assert_eq!(
			upload_summary(0, 1, "/srv/photos"),
			"Uploaded to /srv/photos"
		);
	}

	#[test]
	fn a_mixed_upload_names_both_kinds_rather_than_adding_them_up() {
		// Three files and two folders is not "five files": a drop that carried both kinds has to
		// read back as both, or the notice quietly misreports what landed (§29).
		assert_eq!(
			upload_summary(3, 2, "/srv/last"),
			"Uploaded 3 files and 2 folders"
		);
		assert_eq!(
			upload_summary(1, 1, "/srv/last"),
			"Uploaded 1 file and 1 folder"
		);
	}

	#[test]
	fn several_of_one_kind_are_counted() {
		assert_eq!(upload_summary(4, 0, "/srv/last"), "Uploaded 4 files");
		assert_eq!(upload_summary(0, 3, "/srv/last"), "Uploaded 3 folders");
	}

	// A bare app with one undivided region holding one home tab, and empty shared state, so the
	// tab-strip bookkeeping (§26) is exercised without an iced runtime or the disk. The `Task`s these
	// calls return are dropped — only the tab list and active index are under test.
	fn tab_app() -> App {
		let targets = Rc::new(RefCell::new(crate::profiles::Targets::default()));
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
		assert!(matches!(strip(&app)[1].screen, Screen::Home));
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
			.unwrap_or_else(|poisoned| poisoned.into_inner());
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

	/// A handle that goes away under the pointer lets go of the hand (§52). iced publishes a
	/// widget's `on_exit` from the widget itself, so a chip that closed — or was sent to another
	/// region — never says it lost the pointer, and before this the window went on wearing an open
	/// hand over everything.
	#[test]
	fn a_chip_that_vanishes_under_the_pointer_lets_go_of_the_hand() {
		let _held = crate::cursor::TEST_LOCK
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner());
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

		let mut expected = app.overlay;
		expected.reflow(shrunk);
		assert_eq!(app.overlay, expected, "already pulled back into the window");
		assert!(app.overlay.pos().x <= (500.0 - ui::dialog::DIALOG_WIDTH).max(0.0) + f32::EPSILON);
		assert!(app.overlay.pos().y <= 400.0 - ui::dialog::DIALOG_DRAG_MIN_VISIBLE + f32::EPSILON);
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
			.editor
			.as_ref()
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
		let _ = app.open_editor(app.focus, session, "/home/user/notes.txt".to_owned());
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
		let _ = app.open_editor(app.focus, session, "first.txt".to_owned());
		let _ = app.open_editor(app.focus, session, "second.txt".to_owned());
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
		let _ = app.open_editor(app.focus, first_session, "one.txt".to_owned());
		// The second session's file goes beside IT, so the run of editors after the first session
		// ends at the first chip that is not one of its own (§38).
		let _ = app.open_editor(app.focus, second_session, "two.txt".to_owned());
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
		let _ = app.open_editor(app.focus, 9_999, "orphan.txt".to_owned());
		assert_eq!(on_screen(&app), strip(&app).len() - 1);
		assert_eq!(editor_path(&app, on_screen(&app)), "orphan.txt");
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
		assert!(matches!(strip(&app)[0].screen, Screen::Home));
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
		assert_eq!(grown.width, before.width * 2.0, "twice as wide");
		assert_eq!(grown.height, before.height, "no taller");
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
		assert_eq!(grown.width, before.width);
		assert_eq!(grown.height, before.height * 2.0);
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
			assert_eq!(region.active().window_size.width, rect.width);
			assert_eq!(
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
			assert_eq!(region.active().window_size.width, rect.width);
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
		assert_eq!(app.window.height, 800.0, "the other axis is untouched");
		assert!(
			(app.window.width - (1200.0 - ui::split::SPACING / 2.0)).abs() < 1.0,
			"back to its pre-split width, less the half seam the split cost it: {}",
			app.window.width
		);
		let boxes = ui::split::regions(&app.regions, app.window);
		assert_eq!(boxes[&kept].width, app.window.width, "and it has all of it");
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
		assert_eq!(app.window.width, 1200.0, "the other axis is untouched");
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
		assert_eq!(app.window.width, crate::settings::MIN_WINDOW);
		assert_eq!(app.window.height, 600.0, "the untouched axis is not raised");
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
		left.screen = Screen::Terminal;
		right.screen = Screen::Terminal;
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
		tab.screen = Screen::Terminal;
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
			matches!(copy.screen, Screen::Connect),
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

		let _ = copy.open_copy_of("u@h:22".to_owned(), Some("/srv/www".to_owned()));
		assert!(
			matches!(copy.screen, Screen::Connecting { .. }),
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

		let _ = copy.open_copy_of("u@h:22".to_owned(), Some("/srv".to_owned()));
		assert!(matches!(copy.screen, Screen::Connect));
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

		let _ = copy.open_copy_of("u@h:22".to_owned(), Some("/srv/www".to_owned()));
		assert!(copy.pending_connect, "armed, waiting for a channel to use");
		assert!(
			matches!(copy.screen, Screen::Connect),
			"the pre-filled form is what shows in the meantime, and what is left behind if no \
			 worker ever arrives"
		);
		assert!(
			!matches!(copy.screen, Screen::Error),
			"and above all not an error about a worker that was never late"
		);

		// The worker checks in. The dial goes now, down the channel it just handed over.
		let (tx, mut rx) = mpsc::channel(64);
		let _ = copy.on_ssh_event(SshEvent::Ready(tx));
		assert!(!copy.pending_connect, "spent");
		assert!(matches!(copy.screen, Screen::Connecting { .. }));
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
		let _ = copy.open_copy_of("u@h:22".to_owned(), Some("/srv/www".to_owned()));

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
}

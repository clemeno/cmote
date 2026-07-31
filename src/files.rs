// files.rs — the remote file browser's model (PLAN §19).
//
// The pane under the terminal shows ONE directory at a time, every entry in it, as
// an icon grid. This module is the model only: which directory is shown, what came
// back for it, what is selected, and the little rules the context menu needs. It
// touches neither the network nor the widget tree — `ssh::browse` fetches the
// listing and `ui::files` draws it.
//
// Two things make this different from the folder tree next door (§18):
//
//   * it is FLAT — one directory, no recursion, so a crowded folder costs exactly
//     one listing and never fans out;
//   * the listing arrives in BATCHES. A directory with 50 000 entries would be one
//     enormous message and one enormous relayout; instead the server task sends
//     `BATCH` entries at a time and the grid fills as they land.
//
// Because batches arrive over time, every one carries the `request` number it
// belongs to. Leaving a directory bumps that number, so chunks still in flight for
// the folder we just left are dropped instead of being mixed into the new one.

use std::collections::HashSet;

use iced::{Point, Rectangle};
use serde::{Deserialize, Serialize};

/// The pane's starting height, the shortest the splitter may drag it to, and the
/// grab bar's own height (§19). `ui::terminal` subtracts the pane plus the bar from
/// the grid, so these three are the single source of truth for that arithmetic.
pub const DEFAULT_HEIGHT: f32 = 330.0;
pub const MIN_HEIGHT: f32 = 90.0;
pub const SPLITTER_HEIGHT: f32 = 6.0;

/// How many entries travel in one `FilesChunk` (§19). Big enough that an ordinary
/// directory arrives in a single message, small enough that a pathological one still
/// paints its first screenful immediately instead of after the whole listing.
pub const BATCH: usize = 1000;

/// What an entry is, as the server described it. A symlink keeps its own kind rather
/// than being resolved: following one costs a round trip *per link*, which is exactly
/// the cost this pane is built to avoid in a crowded directory (the tree, which sees
/// far fewer entries, does resolve them — §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
	Dir,
	File,
	Link,
}

/// One directory entry: its name, its kind, and what else the server volunteered about
/// it (§20). The extras cost nothing to collect — SFTP sends a name's attributes along
/// with the name — so they ride along with the listing rather than being asked for
/// per entry, which is what would make a big directory slow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
	pub name: String,
	pub kind: Kind,
	pub meta: Meta,
}

/// What the server said about an entry beyond its name and kind (§20), shown in the
/// details popup beside the selection. Every field is optional because every source is
/// partial: SFTP v3 gives size, time and numeric ids; the owner and group *names* come
/// from the listing's `longname` line, which not every server fills in; and the `ls`
/// fallback (§19) reports none of it, leaving this at its default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Meta {
	pub size: Option<u64>,
	/// Last modification, in seconds since the epoch — the same instant everywhere, so
	/// it is the *display* that needs the server's timezone, not this (`Zone`).
	pub mtime: Option<u32>,
	/// The owner and group as the server names them, falling back to the numeric id when
	/// it gave only that.
	pub owner: Option<String>,
	pub group: Option<String>,
}

/// The remote machine's timezone (§20), asked for once per session (`date +'%z %Z'`).
///
/// An mtime is an instant, not a wall clock: rendering it needs a zone, and the honest
/// one here is the SERVER's — the files being listed are its own, and `ls` on that
/// machine would say the same thing. Until the answer comes back (or when the server has
/// no `date`), this default renders as UTC, which is at least never wrong about the
/// instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Zone {
	/// Minutes east of UTC.
	pub offset: i32,
	/// What the server calls it ("CEST", "UTC"), empty when it would not say.
	pub label: String,
}

impl Default for Zone {
	fn default() -> Self {
		Self {
			offset: 0,
			label: "UTC".to_owned(),
		}
	}
}

/// The icon an entry gets (§19). Derived from its kind and its extension, so the grid
/// reads at a glance without a glyph per file type to maintain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
	Folder,
	Link,
	Image,
	Code,
	Archive,
	Document,
	Audio,
	Video,
	Plain,
}

/// The extension tables behind `category`. Lower-case, no dot. Anything unlisted is a
/// `Plain` file — an unknown type gets the neutral icon rather than a wrong one.
const IMAGE: &[&str] = &[
	"png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tif", "tiff", "heic",
];
const CODE: &[&str] = &[
	"rs", "c", "h", "cpp", "hpp", "cc", "java", "py", "js", "ts", "tsx", "jsx", "go", "rb", "php",
	"sh", "bash", "zsh", "fish", "pl", "lua", "sql", "html", "css", "scss", "json", "yaml", "yml",
	"toml", "xml", "ini", "conf", "cfg", "kt", "swift", "cs", "vim", "mk", "cmake",
];
const ARCHIVE: &[&str] = &[
	"zip", "gz", "tgz", "bz2", "xz", "zst", "tar", "7z", "rar", "jar", "deb", "rpm", "iso",
];
const DOCUMENT: &[&str] = &[
	"pdf", "doc", "docx", "odt", "rtf", "md", "txt", "log", "csv", "xls", "xlsx", "ods", "ppt",
	"pptx",
];
const AUDIO: &[&str] = &["mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "wma"];
const VIDEO: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "wmv", "flv", "m4v"];

/// Which column a user-chosen sort orders the grid by (§19). There is deliberately no `None`
/// variant: the ABSENCE of a sort is `Option::None` on the pane's `sort` field, and it means the
/// default order the server task already laid down — directories first, then everything else by
/// name (the free `sort`). Picking a key overrides that; picking the lit one again drops back to
/// it. `Extension` orders by the text after a name's last dot (all `.rs` together), which is why
/// it is not called "Type": it is the file's extension, not the SFTP entry kind.
///
/// The serde names are the lowercase words, so a target's remembered sort reads naturally in the
/// hand-editable `targets.json` (§22) — the same style `AuthKind` and `ForwardKind` use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortKey {
	Name,
	Modified,
	Extension,
	Size,
}

/// The direction a `SortKey` runs in (§19). Like the key it is OPTIONAL on the model
/// (`Option<SortDir>`): the order can be unset just as the criteria can, and an unset order sorts
/// ASCENDING — so the menu opens with neither direction ticked, and clicking the lit one unsets it
/// again. It flips the WITHIN-group order only: directories stay grouped ahead of files whichever
/// way it points — "folders first" is the one rule the direction never reverses (`compare_entries`).
///
/// Serialized lowercase for the same reason as `SortKey`, so a remembered order reads plainly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
	Ascending,
	Descending,
}

/// Everything the files pane can ask the app to do (§19). Nested under
/// `Message::Files` for the same reason the tree's messages are (§18): a dozen more
/// top-level variants would bury the rest of the enum.
#[derive(Debug, Clone)]
pub enum FilesMessage {
	/// Show or hide the whole pane (the status-bar button).
	Toggled,
	/// An entry was left-clicked: select it.
	EntryClicked(String),
	/// An entry was double-clicked (or Enter pressed): browse the PANE into it, if it is a
	/// directory. This no longer moves the console — that is Sync or "Open in terminal" (§19).
	EntryOpened(String),
	/// Menu "Open in terminal": move the console into this directory (§19). Split off from
	/// `EntryOpened` so a double-click browses the pane while this deliberately moves the
	/// console — the two used to be one action.
	OpenInTerminal(String),
	/// The toolbar's "up" button: browse the pane to the directory above the one on show.
	/// Carries no path — the pane's own is the only thing it can mean, and reading it when the
	/// press arrives is what keeps it from being a path the pane has since left.
	ParentOpened,
	/// An entry was right-clicked: select it and open the context menu on it.
	EntryRightClicked(String),
	/// A press landed anywhere in the pane — give it the keyboard (§20). Sent by the
	/// pane's own `mouse_area`, so an empty patch of grid focuses it just as a cell does.
	/// A press on the grid's empty space also starts a rubber band (§21).
	PanelPressed,
	/// The button came up: the rubber band, if one was being dragged, is finished (§21).
	PanelReleased,
	/// A right-press landed on the grid's empty space — not on any cell (§17). Opens the
	/// empty-space menu, whose one item uploads local files into the directory on show.
	PanelRightPressed,
	/// The empty-space menu's "Upload… here" (§17): pick local files to send into the pane's
	/// current directory. Carries no path — the pane's own is read when the press arrives.
	PaneUploadHere,
	/// The empty-space menu's "Upload folder… here" (§17): pick a local folder to send, tree and
	/// all, into the pane's current directory. Like `PaneUploadHere`, the pane's own path is read
	/// when the press arrives.
	PaneUploadFolderHere,
	/// The empty-space menu's "New folder…" (§18): open the dialog to create a folder in the
	/// pane's current directory.
	NewFolderHere,
	/// Menu "Delete…": open the confirmation to remove the selection — one entry or many (§18).
	DeleteStarted(String),
	/// Menu "Download folder…": recreate this remote directory's whole tree on this machine (§19).
	/// Offered only for a lone directory; a file uses `Download`, a mixed selection its files.
	DownloadFolder(String),
	/// The pointer moved while a band is being dragged, reported by the full-window capture
	/// layer, so the payload is in WINDOW coordinates rather than the pane's (§21).
	BandMoved(Point),
	/// The grid was scrolled; the payload is its absolute vertical offset. Tracked because
	/// the details popup is placed from the selected cell's position, which moves with the
	/// scroll, and because arrow-key navigation has to know what is already on screen (§20).
	Scrolled(f32),
	/// The pointer moved over the pane; the payload is its pane-local position. Tracked
	/// because a right-press carries no coordinates of its own (§18).
	PointerMoved(Point),
	/// Dismiss the context menu without choosing an item.
	MenuDismissed,
	/// The header's sort button: open, or close, the sort menu (§19). The menu itself carries the
	/// four keys and the two directions; this only toggles whether it is dropped down.
	SortMenuOpened,
	/// A click-away closed the sort menu, the way `MenuDismissed` closes the context one (§19).
	SortMenuDismissed,
	/// A key was picked in the sort menu (§19). Picking the ALREADY-LIT key clears the sort, back
	/// to the default order — the menu has no explicit "None", so the lit key is the way off.
	SortKeyPicked(SortKey),
	/// A direction was picked in the sort menu (§19). Remembered even with no key set, so it is
	/// ready the moment one is.
	SortDirPicked(SortDir),
	/// Re-list the directory on show — the refresh for a folder changed from the shell.
	Refresh,
	/// Menu "Copy name" / "Copy relative path" / "Copy full path".
	CopyName(String),
	CopyRelative(String),
	CopyPath(String),
	/// The header's copy button: put the directory on show onto the clipboard. Carries no
	/// path — like `ParentOpened`, the pane's own is the only thing it can mean, and reading
	/// it when the press arrives keeps it from being a directory the pane has since left.
	CopyCurrentPath,
	/// The details popup's copy button (§20): put its whole description — every line the
	/// card shows — onto the clipboard. Carries the already-joined text, built in the view
	/// from the same lines it draws, so the model side does not recompute it.
	CopyDetails(String),
	/// Menu "Rename": turn the cell's label into an edit field.
	RenameStarted(String),
	/// The inline rename field changed.
	RenameEdited(String),
	/// The inline rename was submitted (Enter) — ask the server to do it.
	RenameCommitted,
	/// Menu "Download": pick a local destination, then pull the file.
	Download(String),
	/// The splitter was pressed — begin resizing the pane.
	SplitterGrabbed,
	/// The pointer moved while resizing; the payload is its window position.
	SplitterDragged(Point),
	/// The resize ended (pointer released).
	SplitterReleased,
}

/// A rubber band being dragged over the grid (§21): where the press landed and where the
/// pointer is now, both in pane coordinates. Either corner can be the top-left, which is
/// what `rect` sorts out — a band dragged up and to the left is as ordinary as one dragged
/// down and to the right.
#[derive(Debug, Clone, Copy)]
pub struct Band {
	pub from: Point,
	pub to: Point,
}

impl Band {
	/// The band as a rectangle, whichever way round it was dragged.
	pub fn rect(&self) -> Rectangle {
		Rectangle {
			x: self.from.x.min(self.to.x),
			y: self.from.y.min(self.to.y),
			width: (self.to.x - self.from.x).abs(),
			height: (self.to.y - self.from.y).abs(),
		}
	}
}

/// An open context menu: the entry it acts on, that entry's kind (Download makes no
/// sense on a directory, Open makes none on a file), and where it is drawn. The anchor
/// is frozen when the menu opens — a menu that tracked the live pointer would walk out
/// from under the cursor before an item could be clicked (§18).
#[derive(Debug, Clone)]
pub struct Menu {
	pub path: String,
	pub kind: Kind,
	pub at: Point,
}

/// The in-progress inline rename: which entry, and the text typed so far.
#[derive(Debug, Clone)]
pub struct Rename {
	pub path: String,
	pub text: String,
}

/// The remote file browser (§19).
#[derive(Debug)]
pub struct Files {
	visible: bool,
	height: f32,
	dragging: bool,
	/// The directory being shown, or `None` before anything has been asked for.
	path: Option<String>,
	/// The last working directory the shell was followed into (§19). Kept apart from
	/// `path` because the pane can be pointed elsewhere from the tree: comparing against
	/// this is how a repeated announcement is told from a real move.
	followed: Option<String>,
	entries: Vec<Entry>,
	/// A listing is in flight: batches are still arriving.
	loading: bool,
	/// Which listing the pane is currently interested in. Bumped on every new request so
	/// a batch from the directory we just left is recognised and dropped.
	request: u64,
	/// Every selected entry, by full path (§21). A click selects one; a rubber band,
	/// Ctrl+click, Shift+click and Ctrl+A build a set out of many.
	selected: HashSet<String>,
	/// The entry the keyboard is on: what the arrows step from, what the details popup sits
	/// beside, and what Enter, F2 and the single-target menu items act on. One of
	/// `selected` whenever anything is selected.
	cursor: Option<String>,
	/// The fixed end of a Shift-extended range, set by the last plain or Ctrl+click. Range
	/// selection needs two ends, and the moving one is the cursor.
	anchor: Option<String>,
	/// The rubber band being dragged right now (§21), and the selection it started from —
	/// empty unless the drag is additive, in which case the band adds to it rather than
	/// replacing it.
	band: Option<Band>,
	band_base: HashSet<String>,
	menu: Option<Menu>,
	/// The empty-space menu's anchor when it is open (§17): a right-click on the grid's
	/// blank area, not on a cell. Kept apart from `menu` because it acts on the pane's own
	/// directory ("Upload… here"), not on any one entry — only one of the two is ever up.
	pane_menu: Option<Point>,
	pointer: Point,
	rename: Option<Rename>,
	notice: Option<String>,
	/// How far the grid is scrolled, in pixels (§20).
	scroll: f32,
	/// The remote machine's timezone, as `date` reported it (§20).
	zone: Zone,
	/// Where the selected symlink points, once the server has been asked (§20). Keyed by
	/// the link's own path, so an answer that arrives after the selection moved on is
	/// recognisable as stale. Resolving a link costs a round trip, which is why it is
	/// asked for one selection at a time rather than for every link in the listing (§19).
	link_target: Option<(String, String)>,
	/// The user's chosen sort, or `None` for the default dirs-first-by-name order (§19). `sort`
	/// is the key and `sort_dir` its direction — BOTH optional and independently unset-able: an
	/// unset direction sorts ascending (so a key alone already reorders), and either is cleared by
	/// clicking its own lit row in the menu. `sort_menu_open` is whether the header's sort menu is
	/// dropped down. Kept on the model, not the view, so a chosen order survives every relayout and
	/// outlives a change of directory — a sort is a view preference, not a property of one folder —
	/// and it is what `app` persists per target so it reopens as it was left (§22).
	sort: Option<SortKey>,
	sort_dir: Option<SortDir>,
	sort_menu_open: bool,
}

impl Default for Files {
	fn default() -> Self {
		Self {
			// The pane is the headline of this version, so it starts open; the initial
			// window is sized to fit it *and* the intended grid (`ui::terminal`).
			visible: true,
			height: DEFAULT_HEIGHT,
			dragging: false,
			path: None,
			followed: None,
			entries: Vec::new(),
			loading: false,
			request: 0,
			selected: HashSet::new(),
			cursor: None,
			anchor: None,
			band: None,
			band_base: HashSet::new(),
			menu: None,
			pane_menu: None,
			pointer: Point::ORIGIN,
			rename: None,
			notice: None,
			scroll: 0.0,
			zone: Zone::default(),
			link_target: None,
			sort: None,
			sort_dir: None,
			sort_menu_open: false,
		}
	}
}

impl Files {
	/// Whether the pane is showing.
	pub fn visible(&self) -> bool {
		self.visible
	}

	/// The pane's current height in logical pixels (without the splitter).
	pub fn height(&self) -> f32 {
		self.height
	}

	/// Whether the splitter is being dragged right now (§19).
	pub fn dragging(&self) -> bool {
		self.dragging
	}

	/// How much vertical room the pane takes from the terminal grid: itself plus its
	/// splitter, or nothing at all when hidden. `ui::terminal::grid_size` subtracts
	/// exactly this, so the reflow math and the layout can never drift.
	pub fn reserved(&self) -> f32 {
		if self.visible {
			self.height + SPLITTER_HEIGHT
		} else {
			0.0
		}
	}

	/// The directory on show, if any.
	pub fn path(&self) -> Option<&str> {
		self.path.as_deref()
	}

	/// Whether batches are still arriving.
	pub fn loading(&self) -> bool {
		self.loading
	}

	/// How many entries have landed so far (before the hidden-file filter).
	pub fn count(&self) -> usize {
		self.entries.len()
	}

	/// The entry the keyboard is on (§20): the one the popup describes and the one a
	/// single-target action acts on. `None` when nothing is selected.
	pub fn cursor(&self) -> Option<&str> {
		self.cursor.as_deref()
	}

	/// Whether an entry is part of the selection (§21) — what the grid asks of every cell
	/// it draws.
	pub fn is_selected(&self, path: &str) -> bool {
		self.selected.contains(path)
	}

	/// How many entries are selected.
	pub fn selected_count(&self) -> usize {
		self.selected.len()
	}

	/// The selected entries in GRID order, with their paths (§21). Order matters to every
	/// batch action — a list of copied names coming out in hash order would be nonsense —
	/// and the rows are the only place that order exists.
	pub fn selected_rows(&self, show_hidden: bool) -> Vec<(String, &Entry)> {
		let Some(directory) = self.path.as_deref() else {
			return Vec::new();
		};
		self.rows(show_hidden)
			.into_iter()
			.map(|entry| (crate::explorer::join(directory, &entry.name), entry))
			.filter(|(path, _)| self.selected.contains(path))
			.collect()
	}

	/// Where the cursor sits among the rows on show (§20) — the one number both the details
	/// popup (to place itself beside the cell) and the arrow keys (to step from it) are
	/// asking for. `None` when nothing is selected, or when the cursor is currently
	/// filtered out by the `.*` toggle.
	pub fn selected_index(&self, show_hidden: bool) -> Option<usize> {
		self.index_of(show_hidden, self.cursor.as_deref()?)
	}

	/// Where a path sits among the rows on show, if it is among them at all.
	fn index_of(&self, show_hidden: bool, path: &str) -> Option<usize> {
		let directory = self.path.as_deref()?;
		self.rows(show_hidden)
			.iter()
			.position(|entry| crate::explorer::join(directory, &entry.name) == path)
	}

	/// Move the cursor `delta` rows through the grid (§20): ±1 for Tab and the left/right
	/// arrows, ±(a row's worth of columns) for up and down. Clamped at both ends rather
	/// than wrapping — a grid has a first and a last item, and jumping from one to the
	/// other is never what the key meant. With nothing selected yet, a forward step starts
	/// at the top and a backward one at the bottom.
	///
	/// `extend` is Shift held down (§21): the selection then runs from the anchor to wherever
	/// the cursor lands, instead of being just the cell walked onto.
	pub fn step(&mut self, show_hidden: bool, delta: isize, extend: bool) {
		let rows = self.rows(show_hidden);
		// An empty grid has nowhere to step to, and `last` below would underflow.
		let Some(last) = rows.len().checked_sub(1) else {
			return;
		};
		let last = last as isize;
		let next = match self.selected_index(show_hidden) {
			Some(index) => (index as isize).saturating_add(delta),
			None if delta >= 0 => 0,
			None => last,
		};
		let next = next.clamp(0, last) as usize;
		if extend {
			self.select_range(show_hidden, next);
		} else {
			self.select_only(show_hidden, next);
		}
	}

	/// Select every row from the anchor to `index`, inclusive, and put the cursor on
	/// `index`. With no anchor — nothing has been clicked yet — the range is that one row.
	fn select_range(&mut self, show_hidden: bool, index: usize) {
		let Some(directory) = self.path.clone() else {
			return;
		};
		let anchor = self
			.anchor
			.clone()
			.and_then(|path| self.index_of(show_hidden, &path))
			.unwrap_or(index);
		let rows = self.rows(show_hidden);
		let (from, to) = (anchor.min(index), anchor.max(index).min(rows.len() - 1));
		let selected: HashSet<String> = rows[from..=to]
			.iter()
			.map(|entry| crate::explorer::join(&directory, &entry.name))
			.collect();
		let cursor = crate::explorer::join(&directory, &rows[index].name);
		self.selected = selected;
		self.cursor = Some(cursor);
		if self.anchor.is_none() {
			self.anchor = self.cursor.clone();
		}
	}

	/// Select the row at `index` and nothing else, anchor included — the plain click, and
	/// the plain arrow key.
	fn select_only(&mut self, show_hidden: bool, index: usize) {
		let Some(directory) = self.path.clone() else {
			return;
		};
		let rows = self.rows(show_hidden);
		let Some(entry) = rows.get(index) else {
			return;
		};
		let path = crate::explorer::join(&directory, &entry.name);
		self.select(&path);
	}

	/// How far the grid is scrolled (§20).
	pub fn scroll(&self) -> f32 {
		self.scroll
	}

	/// Remember the grid's scroll offset — from the scrollable's own report, and from the
	/// app when it scrolls a keyboard-moved selection back into view.
	pub fn set_scroll(&mut self, scroll: f32) {
		self.scroll = scroll.max(0.0);
	}

	/// The remote machine's timezone, used to render every mtime in the pane (§20).
	pub fn zone(&self) -> &Zone {
		&self.zone
	}

	/// The server answered the timezone probe.
	pub fn set_zone(&mut self, zone: Zone) {
		self.zone = zone;
	}

	/// Where the selected symlink points, if that is what is selected and the answer has
	/// arrived. Anything else — a file, a folder, a link still being resolved, an answer
	/// for a link the user has since moved off — reads as `None`.
	pub fn link_target(&self) -> Option<&str> {
		let (path, target) = self.link_target.as_ref()?;
		(Some(path.as_str()) == self.cursor.as_deref()).then_some(target.as_str())
	}

	/// A `readlink` came back. Kept whatever the selection is now — `link_target` is what
	/// decides whether it is still the interesting one.
	pub fn set_link_target(&mut self, path: String, target: String) {
		self.link_target = Some((path, target));
	}

	/// The open context menu, if any.
	pub fn menu(&self) -> Option<&Menu> {
		self.menu.as_ref()
	}

	/// The open empty-space menu's anchor, if one is showing (§17). Its only item uploads
	/// into the pane's current directory; the entry menu (`menu`) acts on one entry instead.
	pub fn pane_menu(&self) -> Option<Point> {
		self.pane_menu
	}

	/// Open the empty-space menu where the pointer is (§17), for a right-click on the grid's
	/// blank area. Closes any entry menu first, so only one menu is ever up.
	pub fn open_pane_menu(&mut self) {
		self.menu = None;
		self.pane_menu = Some(self.pointer);
		// Only one surface is ever up, and the sort menu is one of them (§19).
		self.sort_menu_open = false;
	}

	/// The chosen sort key, or `None` for the default order (§19). The header tints its sort
	/// button while this is set, and the sort menu ticks the row that matches.
	pub fn sort_key(&self) -> Option<SortKey> {
		self.sort
	}

	/// The chosen sort direction, or `None` when the order is unset (§19). An unset order sorts
	/// ascending (`rows`), and the direction is only ever felt once a key is set. Returned as it is
	/// stored — `None`, `Ascending` or `Descending` — so the menu ticks the row that matches, or
	/// none, and `app` can persist the exact tri-state per target (§22).
	pub fn sort_dir(&self) -> Option<SortDir> {
		self.sort_dir
	}

	/// Whether the header's sort menu is dropped down (§19).
	pub fn sort_menu_open(&self) -> bool {
		self.sort_menu_open
	}

	/// Toggle the sort menu open or shut — the header's sort button (§19). Opening it closes any
	/// context menu, so only one surface is ever up.
	pub fn toggle_sort_menu(&mut self) {
		self.sort_menu_open = !self.sort_menu_open;
		if self.sort_menu_open {
			self.menu = None;
			self.pane_menu = None;
		}
	}

	/// Close the sort menu — a click-away (§19).
	pub fn close_sort_menu(&mut self) {
		self.sort_menu_open = false;
	}

	/// Pick a sort key from the menu (§19). Picking the one already lit clears the sort: the menu
	/// carries no "None" row, so the active key doubles as the way back to the default order.
	pub fn pick_sort_key(&mut self, key: SortKey) {
		self.sort = if self.sort == Some(key) {
			None
		} else {
			Some(key)
		};
	}

	/// Pick a sort direction from the menu (§19), the exact twin of `pick_sort_key`: picking the one
	/// already lit unsets the order (back to the ascending default), so the menu needs no "None" row
	/// for the direction any more than it does for the key. Stored even with no key set, so it is
	/// ready the moment one is.
	pub fn pick_sort_dir(&mut self, dir: SortDir) {
		self.sort_dir = if self.sort_dir == Some(dir) {
			None
		} else {
			Some(dir)
		};
	}

	/// Set the sort outright — key and direction together — to restore a target's remembered choice
	/// (§22). Unlike the two `pick_*` menu actions (which toggle) this writes the values straight in,
	/// so a stored `None` reopens in the default order and a stored key/direction reopens on it. The
	/// next `rows` reorders the grid to match.
	pub fn set_sort(&mut self, sort: Option<SortKey>, dir: Option<SortDir>) {
		self.sort = sort;
		self.sort_dir = dir;
	}

	/// The in-progress inline rename, if any.
	pub fn editing(&self) -> Option<&Rename> {
		self.rename.as_ref()
	}

	/// The last thing that went wrong, shown under the grid until the next one replaces it.
	pub fn notice(&self) -> Option<&str> {
		self.notice.as_deref()
	}

	/// Show or hide the pane. Hiding gives its height back to the grid, so the caller
	/// refits the terminal afterwards.
	pub fn toggle(&mut self) {
		self.visible = !self.visible;
		self.menu = None;
	}

	/// Resize the pane from a splitter drag, clamped between `MIN_HEIGHT` and `max`
	/// (the app passes a fraction of the window, so the grid can never be squeezed out).
	pub fn set_height(&mut self, height: f32, max: f32) {
		self.height = height.clamp(MIN_HEIGHT, max.max(MIN_HEIGHT));
	}

	/// Start / stop a splitter drag.
	pub fn set_dragging(&mut self, dragging: bool) {
		self.dragging = dragging;
	}

	/// Remember where the pointer is, so a right-press — which carries no coordinates —
	/// can open the menu under it.
	pub fn set_pointer(&mut self, pointer: Point) {
		self.pointer = pointer;
	}

	/// Where the pointer last was, in pane coordinates (§21) — the rubber band starts from
	/// it, since a press carries no position of its own.
	pub fn pointer(&self) -> Point {
		self.pointer
	}

	/// Select one entry and nothing else — the plain click.
	pub fn select(&mut self, path: &str) {
		self.selected.clear();
		self.selected.insert(path.to_owned());
		self.cursor = Some(path.to_owned());
		self.anchor = Some(path.to_owned());
	}

	/// Add an entry to the selection, or take it out again — Ctrl+click (§21). The rest of
	/// the selection is left alone, and the entry becomes the new anchor either way: the
	/// next Shift+click ranges from where the user last pointed.
	pub fn toggle_selection(&mut self, path: &str) {
		let added = !self.selected.remove(path);
		if added {
			self.selected.insert(path.to_owned());
		}
		self.cursor = added.then(|| path.to_owned());
		self.anchor = Some(path.to_owned());
	}

	/// Select everything from the anchor to this entry — Shift+click (§21).
	pub fn extend_selection(&mut self, show_hidden: bool, path: &str) {
		if let Some(index) = self.index_of(show_hidden, path) {
			self.select_range(show_hidden, index);
		}
	}

	/// Select every entry on show — Ctrl+A (§21). The cursor goes to the first, so the
	/// popup has a cell to sit beside and an arrow key has somewhere to step from.
	pub fn select_all(&mut self, show_hidden: bool) {
		let Some(directory) = self.path.clone() else {
			return;
		};
		let rows = self.rows(show_hidden);
		let paths: Vec<String> = rows
			.iter()
			.map(|entry| crate::explorer::join(&directory, &entry.name))
			.collect();
		self.cursor = paths.first().cloned();
		self.anchor = paths.first().cloned();
		self.selected = paths.into_iter().collect();
	}

	/// Drop the selection, and with it the details popup — a press that reached the pane
	/// itself landed on empty space, beside every cell (§20).
	pub fn deselect(&mut self) {
		self.selected.clear();
		self.cursor = None;
		self.anchor = None;
	}

	/// Start a rubber band where the pointer is (§21). An additive drag (Ctrl held) keeps
	/// the current selection as the band's floor — the band can only add to it — while a
	/// plain one starts from nothing, which is also what makes a press on empty space
	/// clear the selection.
	pub fn begin_band(&mut self, at: Point, additive: bool) {
		self.band = Some(Band { from: at, to: at });
		if !additive {
			self.deselect();
		}
		self.band_base = self.selected.clone();
		self.menu = None;
	}

	/// The band being dragged, if one is.
	pub fn band(&self) -> Option<&Band> {
		self.band.as_ref()
	}

	/// The pointer moved: stretch the band to it. `false` when no band is being dragged,
	/// which is how the caller knows an ordinary pointer move from a banding one.
	pub fn drag_band(&mut self, to: Point) -> bool {
		let Some(band) = self.band.as_mut() else {
			return false;
		};
		band.to = to;
		true
	}

	/// The band now covers these entries, in grid order (§21). The cursor follows the last
	/// one it reached and the anchor stays on the first, so the details popup has a cell to
	/// sit beside and a following Shift+click extends from where the band began.
	pub fn set_band_selection(&mut self, paths: Vec<String>) {
		self.cursor = paths.last().cloned();
		self.anchor = paths.first().cloned().or_else(|| self.anchor.clone());
		let banded: HashSet<String> = paths.into_iter().collect();
		self.selected = self.band_base.union(&banded).cloned().collect();
	}

	/// The drag ended (the button came up).
	pub fn end_band(&mut self) {
		self.band = None;
		self.band_base.clear();
	}

	/// Open the context menu on an entry, anchored where the pointer is right now. Closes any
	/// empty-space menu first, so only one is ever up and the view's priority check is moot.
	pub fn open_menu(&mut self, path: String) {
		let kind = self.kind_of(&path).unwrap_or(Kind::File);
		self.pane_menu = None;
		// Only one surface is ever up, and the sort menu is one of them (§19).
		self.sort_menu_open = false;
		self.menu = Some(Menu {
			path,
			kind,
			at: self.pointer,
		});
	}

	/// Close whichever context menu is open — the entry menu or the empty-space one (§17).
	pub fn close_menu(&mut self) {
		self.menu = None;
		self.pane_menu = None;
	}

	/// An entry's kind, by full path. `None` when it is not in the directory on show.
	pub fn kind_of(&self, path: &str) -> Option<Kind> {
		let name = crate::explorer::name(path);
		self.entries
			.iter()
			.find(|entry| entry.name == name)
			.map(|entry| entry.kind)
	}

	/// Show a directory, returning the request number to list it under — or `None` when
	/// that directory is already on show. This is the deliberate, user-driven move: a
	/// click in the folder tree, or entering a directory here.
	pub fn show(&mut self, path: &str) -> Option<u64> {
		if self.path.as_deref() == Some(path) {
			return None;
		}
		self.path = Some(path.to_owned());
		Some(self.begin())
	}

	/// Follow the shell into its working directory (§19) — the passive move, called on
	/// every announcement the shell makes.
	///
	/// It acts only when the shell has actually MOVED, which is what lets the two sources
	/// coexist: the shell re-announces the same directory at every prompt, so without this
	/// check a tree click that pointed the pane somewhere else would be undone by the very
	/// next prompt. Whoever moved last wins, and a repeated announcement is not a move.
	pub fn follow(&mut self, cwd: &str) -> Option<u64> {
		if self.followed.as_deref() == Some(cwd) {
			return None;
		}
		self.followed = Some(cwd.to_owned());
		self.show(cwd)
	}

	/// Seed the shell-follow guard without moving the pane (§22). Used on reconnect: the
	/// pane is pointed at its own remembered directory, and once the shell has settled at the
	/// cwd we replayed with a `cd`, this marks that cwd as "already followed" — so it does not
	/// drag the pane off a *different* remembered files directory now, yet the next real `cd`
	/// (a move to somewhere else) still counts as a move and is followed.
	pub fn set_followed(&mut self, cwd: &str) {
		self.followed = Some(cwd.to_owned());
	}

	/// Re-list the directory on show (the Refresh item, and what a rename triggers).
	/// `None` when no directory has been shown yet.
	pub fn refresh(&mut self) -> Option<u64> {
		self.path.as_ref()?;
		Some(self.begin())
	}

	/// Start a new listing: drop what the previous one left and bump the request number,
	/// which is what makes its still-in-flight batches identifiable as stale.
	fn begin(&mut self) -> u64 {
		self.entries.clear();
		self.loading = true;
		self.deselect();
		self.end_band();
		self.menu = None;
		self.rename = None;
		self.notice = None;
		// A new directory starts at the top, and whatever link was resolved belonged to
		// the old one's selection.
		self.scroll = 0.0;
		self.link_target = None;
		self.request += 1;
		self.request
	}

	/// A batch of entries came back. Already sorted by the server task (which has the
	/// whole listing in hand), so batches simply append and the order holds across them.
	/// A batch for a directory we have left is dropped, and so are `.` and `..` — every
	/// other name lands, whatever it starts with, because the toggle is what hides things.
	pub fn chunk(&mut self, request: u64, entries: Vec<Entry>, done: bool) {
		if request != self.request {
			return;
		}
		self.entries.extend(
			entries
				.into_iter()
				.filter(|entry| !crate::explorer::is_dot_link(&entry.name)),
		);
		if done {
			self.loading = false;
		}
	}

	/// The listing failed (no permission, gone, the server refused). Stale failures are
	/// dropped the same way stale batches are.
	pub fn failed(&mut self, request: u64, reason: String) {
		if request != self.request {
			return;
		}
		self.loading = false;
		self.notice = Some(reason);
	}

	/// Put a message on the pane's notice line.
	pub fn set_notice(&mut self, notice: String) {
		self.notice = Some(notice);
	}

	/// Drop everything the pane knows (§19). Called when a session opens or closes, so
	/// one server's directory never shows up under the next one's. The height and
	/// visibility are user preferences, not session state, so they stay.
	pub fn reset(&mut self) {
		self.path = None;
		self.followed = None;
		self.entries.clear();
		self.loading = false;
		self.deselect();
		self.end_band();
		self.menu = None;
		self.rename = None;
		self.notice = None;
		self.dragging = false;
		self.scroll = 0.0;
		self.link_target = None;
		// The next session is a different machine: its clock is not this one's (§20).
		self.zone = Zone::default();
		// Not `begin()`: bumping the request here too is what stops a batch from the
		// previous session landing in the next one.
		self.request += 1;
	}

	/// Begin renaming an entry.
	pub fn start_rename(&mut self, path: String) {
		self.menu = None;
		let text = crate::explorer::name(&path).to_owned();
		self.rename = Some(Rename { path, text });
	}

	/// The inline rename field changed.
	pub fn edit_rename(&mut self, text: String) {
		if let Some(rename) = self.rename.as_mut() {
			rename.text = text;
		}
	}

	/// Abandon the inline rename (Esc).
	pub fn cancel_rename(&mut self) {
		self.rename = None;
	}

	/// Finish the inline rename and hand back `(from, to)` for the server to perform.
	/// Same rules as the tree's (§18): a blank name, a name carrying a path separator
	/// (which would *move* the entry, not rename it), or no change at all asks nothing.
	pub fn commit_rename(&mut self) -> Option<(String, String)> {
		let rename = self.rename.take()?;
		let new_name = rename.text.trim();
		let parent = crate::explorer::parent(&rename.path)?;
		if new_name.is_empty()
			|| new_name.contains('/')
			|| new_name == crate::explorer::name(&rename.path)
		{
			return None;
		}
		Some((rename.path.clone(), crate::explorer::join(parent, new_name)))
	}

	/// A rename succeeded somewhere. Returns the request number to re-list under when it
	/// happened in the directory on show — the entry has to move to its new sort
	/// position, and only the server knows where that is.
	pub fn renamed(&mut self, from: &str) -> Option<u64> {
		if crate::explorer::parent(from) != self.path.as_deref() {
			return None;
		}
		self.refresh()
	}

	/// The entries to draw, in order (§19). Dot-prefixed names are filtered here rather
	/// than at fetch time, so flipping the shared `.*` toggle (the tree's, §18) costs
	/// nothing — and the filter is the only thing that toggle does.
	pub fn rows(&self, show_hidden: bool) -> Vec<&Entry> {
		let mut rows: Vec<&Entry> = self
			.entries
			.iter()
			.filter(|entry| show_hidden || !entry.name.starts_with('.'))
			.collect();
		// Only a user-chosen sort re-orders here. With none, the entries are already in the
		// default dirs-first-by-name order the server task laid down (the free `sort` below),
		// so the common case pays nothing beyond the filter above.
		if let Some(key) = self.sort {
			// An unset direction sorts ascending, so a key on its own already reorders the grid.
			let dir = self.sort_dir.unwrap_or(SortDir::Ascending);
			rows.sort_by(|left, right| compare_entries(left, right, key, dir));
		}
		rows
	}
}

/// Put a listing in display order: directories first, then everything else, each group
/// case-insensitively by name. Done once by the server task, before the entries are cut
/// into batches, so the grid can simply append each batch as it lands.
pub fn sort(entries: &mut [Entry]) {
	entries.sort_by(|left, right| {
		let folder_first = (left.kind != Kind::Dir).cmp(&(right.kind != Kind::Dir));
		folder_first
			.then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
			// Two names differing only in case would otherwise compare equal, and an
			// unstable sort could then swap them between two listings of the same folder.
			.then_with(|| left.name.cmp(&right.name))
	});
}

/// Order two entries under a user-chosen sort (§19). Directories always come first, whatever the
/// key or direction — "folders first" is the one rule the direction never flips; it reorders only
/// WITHIN each group. Every key falls back to the name, so the order is total and stable across
/// re-listings, exactly as the default `sort` is.
fn compare_entries(left: &Entry, right: &Entry, key: SortKey, dir: SortDir) -> std::cmp::Ordering {
	use std::cmp::Ordering;
	// Folders ahead of the rest, settled BEFORE the direction so a descending sort cannot sink
	// them below the files.
	let folder_first = (left.kind != Kind::Dir).cmp(&(right.kind != Kind::Dir));
	if folder_first != Ordering::Equal {
		return folder_first;
	}
	let within = match key {
		SortKey::Name => name_cmp(left, right),
		// `Option` orders `None` before `Some`, so an entry the `ls` fallback left without a
		// size or time (§19) sorts ahead of the rest ascending — a stable, predictable spot.
		SortKey::Modified => left.meta.mtime.cmp(&right.meta.mtime),
		SortKey::Size => left.meta.size.cmp(&right.meta.size),
		// The shared `extension` returns `None` for a name with no extension (and for a bare
		// dot-file); `unwrap_or_default` maps that to "", which sorts ahead of any real
		// extension ascending — a stable, predictable spot for the extensionless.
		SortKey::Extension => extension(&left.name)
			.unwrap_or_default()
			.cmp(&extension(&right.name).unwrap_or_default()),
	}
	// The name settles every tie, so two files of one size (or one extension) keep a stable,
	// readable order rather than an arbitrary one.
	.then_with(|| name_cmp(left, right));
	match dir {
		SortDir::Ascending => within,
		SortDir::Descending => within.reverse(),
	}
}

/// Two names compared case-insensitively, with the exact bytes as the tie-break — the very order
/// the default `sort` uses, so "folders first, then by name" reads identically whether it came
/// from the server task or from a user picking `Name`.
fn name_cmp(left: &Entry, right: &Entry) -> std::cmp::Ordering {
	left.name
		.to_lowercase()
		.cmp(&right.name.to_lowercase())
		.then_with(|| left.name.cmp(&right.name))
}

/// Render an mtime in the server's own timezone (§20), as `YYYY-MM-DD HH:MM:SS ZONE`.
///
/// A fixed, ISO-ordered format rather than the machine's locale: the alternative is a
/// dependency (or an OS call) to learn what "24/07" means to this user, and an ordering
/// that is unambiguous everywhere costs neither. The zone tag is the server's own, so the
/// reading matches what `ls -l` on that machine would show.
pub fn format_mtime(epoch: u32, zone: &Zone) -> String {
	let (year, month, day, seconds) = local_parts(epoch, zone);
	let stamp = format!(
		"{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
		seconds / 3600,
		seconds % 3600 / 60,
		seconds % 60,
	);
	with_zone(stamp, zone)
}

/// A compact mtime for the file grid's cells (§19): `YYYY-MM-DD HH:MM ZONE`, without the
/// seconds `format_mtime` carries but still tagged with the server's zone. A cell has room to
/// say the day, the minute and the zone at a glance, not the exact second — that stays the
/// details popup's job (§20), which still calls `format_mtime` for the full reading.
pub fn format_mtime_short(epoch: u32, zone: &Zone) -> String {
	let (year, month, day, seconds) = local_parts(epoch, zone);
	let stamp = format!(
		"{year:04}-{month:02}-{day:02} {:02}:{:02}",
		seconds / 3600,
		seconds % 3600 / 60,
	);
	with_zone(stamp, zone)
}

/// Tag a formatted `stamp` with the server's zone (§20): its label, its `+HH:MM` offset, or
/// both — and nothing at all for plain UTC with no label. Shared by the full and the short
/// mtime formats so a time is tagged the same way wherever it is shown.
fn with_zone(stamp: String, zone: &Zone) -> String {
	match (zone.label.is_empty(), zone.offset) {
		(true, 0) => stamp,
		(true, offset) => format!("{stamp} {}", format_offset(offset)),
		(false, 0) => format!("{stamp} {}", zone.label),
		(false, offset) => format!("{stamp} {} ({})", zone.label, format_offset(offset)),
	}
}

/// The server-local calendar parts of a UTC `epoch`, shifted by the zone offset: the
/// `(year, month, day, seconds-into-day)` both mtime formats above build their string from.
///
/// The epoch is UTC; shifting by the offset gives the server's wall clock, and
/// `div_euclid`/`rem_euclid` keep that correct for the negative offsets west of Greenwich,
/// where a plain division would round towards zero and land a day out.
fn local_parts(epoch: u32, zone: &Zone) -> (i64, i64, i64, i64) {
	let local = i64::from(epoch) + i64::from(zone.offset) * 60;
	let (year, month, day) = civil_from_days(local.div_euclid(86_400));
	(year, month, day, local.rem_euclid(86_400))
}

/// A UTC offset in minutes as `+HH:MM`.
fn format_offset(offset: i32) -> String {
	let sign = if offset < 0 { '-' } else { '+' };
	let offset = offset.abs();
	format!("{sign}{:02}:{:02}", offset / 60, offset % 60)
}

/// The civil date `days` days after 1970-01-01, as `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days`, the standard closed-form conversion: it shifts the
/// epoch to 0000-03-01 so leap days land at the END of the 400-year era, which is what
/// lets the whole calendar — including the 100/400-year exceptions — fall out of integer
/// arithmetic with no table and no loop.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
	let shifted = days + 719_468;
	let era = shifted.div_euclid(146_097);
	let day_of_era = shifted.rem_euclid(146_097); // [0, 146096]
	let year_of_era =
		(day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
	let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
	let march_month = (5 * day_of_year + 2) / 153; // [0, 11], 0 = March
	let day = day_of_year - (153 * march_month + 2) / 5 + 1;
	let month = if march_month < 10 {
		march_month + 3
	} else {
		march_month - 9
	};
	(year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// Read `date +'%z %Z'` back off the server (§20): `+0200 CEST`. The label is optional —
/// some shells answer with the offset alone — but an unparseable offset means we do not
/// know the zone at all, and rendering times in a guessed one would be worse than UTC.
pub fn parse_zone(output: &str) -> Option<Zone> {
	let mut fields = output.split_whitespace();
	let offset = fields.next()?;
	let label = fields.next().unwrap_or_default().to_owned();
	let sign = match offset.as_bytes().first()? {
		b'+' => 1,
		b'-' => -1,
		_ => return None,
	};
	if offset.len() != 5 {
		return None;
	}
	let digits = offset.get(1..5)?;
	if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
		return None;
	}
	let hours: i32 = digits[..2].parse().ok()?;
	let minutes: i32 = digits[2..].parse().ok()?;
	Some(Zone {
		offset: sign * (hours * 60 + minutes),
		label,
	})
}

/// Pull the owner and group *names* out of an SFTP listing's `longname` (§20) — the
/// `ls -l` line the server builds for each entry:
///
/// ```text
/// -rw-r--r--    1 cme      staff        4311 Jul 24 18:03 notes.txt
/// ```
///
/// SFTP v3 carries only numeric ids in the attributes, so this line is the one place a
/// server that resolved `cme` and `staff` for itself actually says so. `None` when the
/// line is missing or is not that shape, and the caller then falls back to the numbers.
///
/// `ponytail:` a column split, and columns are a guess — a *group* name containing a
/// space would shift the fields. Requiring the mode column to be mode-shaped and the
/// fifth column to be a number is what rejects the shapes we would otherwise misread;
/// nothing here is trusted enough to act on, it is text shown beside a file.
pub fn parse_longname(longname: &str) -> Option<(String, String)> {
	let mut fields = longname.split_whitespace();
	let mode = fields.next()?;
	if mode.len() < 10 || !mode.starts_with(['-', 'd', 'l', 'b', 'c', 'p', 's']) {
		return None;
	}
	let _links = fields.next()?;
	let owner = fields.next()?;
	let group = fields.next()?;
	// The column after the group is the size on every ls-style line; if it is not a
	// number, the fields are not where we think they are.
	fields.next()?.parse::<u64>().ok()?;
	Some((owner.to_owned(), group.to_owned()))
}

/// The owner and group of an entry as one `owner:group` field (§20), each falling back to
/// `?` on its own — a server that names the user but not the group still says something
/// useful. `None` when it reported neither, which is the `ls` fallback's answer (§19).
pub fn owner_group(meta: &Meta) -> Option<String> {
	let owner = meta.owner.as_deref();
	let group = meta.group.as_deref();
	if owner.is_none() && group.is_none() {
		return None;
	}
	Some(format!("{}:{}", owner.unwrap_or("?"), group.unwrap_or("?")))
}

/// A name's lower-cased extension, if it has one. A dot-file with no other dot
/// (`.bashrc`) has none — that dot opens the name, it does not close a stem.
fn extension(name: &str) -> Option<String> {
	let (stem, extension) = name.rsplit_once('.')?;
	(!stem.is_empty()).then(|| extension.to_lowercase())
}

/// Which icon an entry gets (§19). Directories and symlinks go by kind; everything else
/// by its lower-cased extension, falling back to the neutral file icon.
pub fn category(entry: &Entry) -> Category {
	match entry.kind {
		Kind::Dir => return Category::Folder,
		Kind::Link => return Category::Link,
		Kind::File => {}
	}

	let Some(extension) = extension(&entry.name) else {
		return Category::Plain;
	};
	let extension = extension.as_str();
	for (table, category) in [
		(IMAGE, Category::Image),
		(CODE, Category::Code),
		(ARCHIVE, Category::Archive),
		(DOCUMENT, Category::Document),
		(AUDIO, Category::Audio),
		(VIDEO, Category::Video),
	] {
		if table.contains(&extension) {
			return category;
		}
	}
	Category::Plain
}

/// The type line of the details popup (§20): the file's MIME type, read off its extension.
/// Anything unlisted is `application/octet-stream`, which is what "some bytes" is called —
/// the same answer `file --mime-type` gives when it recognises nothing.
///
/// A table rather than a probe: asking the server would be a round trip per selection, and
/// the extension is already in hand. It is the guess every browser and web server makes.
pub fn mime(name: &str) -> &'static str {
	let Some(extension) = extension(name) else {
		return OCTET_STREAM;
	};
	MIME.iter()
		.find(|(known, _)| *known == extension)
		.map_or(OCTET_STREAM, |(_, mime)| mime)
}

/// The fallback type: bytes of unknown meaning.
const OCTET_STREAM: &str = "application/octet-stream";

/// Extension to MIME type, lower-case and without the dot — the extensions `category`
/// knows, plus the handful whose type is worth naming even though they share its neutral
/// icon. Registered names where IANA has one, the conventional `text/x-*` otherwise.
const MIME: &[(&str, &str)] = &[
	// Images.
	("png", "image/png"),
	("jpg", "image/jpeg"),
	("jpeg", "image/jpeg"),
	("gif", "image/gif"),
	("bmp", "image/bmp"),
	("webp", "image/webp"),
	("svg", "image/svg+xml"),
	("ico", "image/vnd.microsoft.icon"),
	("tif", "image/tiff"),
	("tiff", "image/tiff"),
	("heic", "image/heic"),
	// Code and configuration.
	("rs", "text/rust"),
	("c", "text/x-c"),
	("h", "text/x-c"),
	("cpp", "text/x-c++"),
	("hpp", "text/x-c++"),
	("cc", "text/x-c++"),
	("java", "text/x-java"),
	("py", "text/x-python"),
	("js", "text/javascript"),
	("jsx", "text/javascript"),
	("ts", "text/x-typescript"),
	("tsx", "text/x-typescript"),
	("go", "text/x-go"),
	("rb", "text/x-ruby"),
	("php", "application/x-httpd-php"),
	("sh", "application/x-shellscript"),
	("bash", "application/x-shellscript"),
	("zsh", "application/x-shellscript"),
	("fish", "application/x-shellscript"),
	("pl", "text/x-perl"),
	("lua", "text/x-lua"),
	("sql", "application/sql"),
	("html", "text/html"),
	("css", "text/css"),
	("scss", "text/x-scss"),
	("json", "application/json"),
	("yaml", "application/yaml"),
	("yml", "application/yaml"),
	("toml", "application/toml"),
	("xml", "application/xml"),
	("ini", "text/plain"),
	("conf", "text/plain"),
	("cfg", "text/plain"),
	("kt", "text/x-kotlin"),
	("swift", "text/x-swift"),
	("cs", "text/x-csharp"),
	("vim", "text/x-vim"),
	("mk", "text/x-makefile"),
	("cmake", "text/x-cmake"),
	// Archives and packages.
	("zip", "application/zip"),
	("gz", "application/gzip"),
	("tgz", "application/gzip"),
	("bz2", "application/x-bzip2"),
	("xz", "application/x-xz"),
	("zst", "application/zstd"),
	("tar", "application/x-tar"),
	("7z", "application/x-7z-compressed"),
	("rar", "application/vnd.rar"),
	("jar", "application/java-archive"),
	("deb", "application/vnd.debian.binary-package"),
	("rpm", "application/x-rpm"),
	("iso", "application/x-iso9660-image"),
	// Documents.
	("pdf", "application/pdf"),
	("doc", "application/msword"),
	(
		"docx",
		"application/vnd.openxmlformats-officedocument.wordprocessingml.document",
	),
	("odt", "application/vnd.oasis.opendocument.text"),
	("rtf", "application/rtf"),
	("md", "text/markdown"),
	("txt", "text/plain"),
	("log", "text/plain"),
	("csv", "text/csv"),
	("xls", "application/vnd.ms-excel"),
	(
		"xlsx",
		"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
	),
	("ods", "application/vnd.oasis.opendocument.spreadsheet"),
	("ppt", "application/vnd.ms-powerpoint"),
	(
		"pptx",
		"application/vnd.openxmlformats-officedocument.presentationml.presentation",
	),
	// Audio.
	("mp3", "audio/mpeg"),
	("wav", "audio/wav"),
	("flac", "audio/flac"),
	("ogg", "audio/ogg"),
	("m4a", "audio/mp4"),
	("aac", "audio/aac"),
	("opus", "audio/opus"),
	("wma", "audio/x-ms-wma"),
	// Video.
	("mp4", "video/mp4"),
	("mkv", "video/x-matroska"),
	("avi", "video/x-msvideo"),
	("mov", "video/quicktime"),
	("webm", "video/webm"),
	("wmv", "video/x-ms-wmv"),
	("flv", "video/x-flv"),
	("m4v", "video/x-m4v"),
	// Executables and libraries, which the grid draws as plain files.
	("exe", "application/vnd.microsoft.portable-executable"),
	("dll", "application/vnd.microsoft.portable-executable"),
	("so", "application/x-sharedlib"),
	("o", "application/x-object"),
	("a", "application/x-archive"),
	("wasm", "application/wasm"),
];

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(name: &str, kind: Kind) -> Entry {
		Entry {
			name: name.to_owned(),
			kind,
			meta: Meta::default(),
		}
	}

	/// A file entry carrying a size and an mtime, for the sort tests — the only ones that read
	/// past a name and a kind.
	fn sized(name: &str, size: u64, mtime: u32) -> Entry {
		Entry {
			name: name.to_owned(),
			kind: Kind::File,
			meta: Meta {
				size: Some(size),
				mtime: Some(mtime),
				owner: None,
				group: None,
			},
		}
	}

	/// The names `rows` returns, in order — what every sort test asserts against.
	fn names(files: &Files) -> Vec<String> {
		files
			.rows(false)
			.into_iter()
			.map(|entry| entry.name.clone())
			.collect()
	}

	/// A pane showing `/home` with one batch of entries already landed.
	fn pane(entries: &[Entry]) -> (Files, u64) {
		let mut files = Files::default();
		let request = files.show("/home").expect("a new directory needs listing");
		files.chunk(request, entries.to_vec(), true);
		(files, request)
	}

	#[test]
	fn the_same_directory_is_not_listed_twice() {
		let mut files = Files::default();
		assert_eq!(files.show("/home"), Some(1));
		// The shell re-announces its directory at every prompt; re-listing a crowded
		// folder on each one would never stop.
		assert_eq!(files.show("/home"), None);
		assert_eq!(files.show("/etc"), Some(2));
		// Refresh is the explicit re-ask, and it always goes.
		assert_eq!(files.refresh(), Some(3));
	}

	#[test]
	fn a_repeated_cwd_announcement_does_not_undo_a_tree_click() {
		let mut files = Files::default();
		assert_eq!(files.follow("/home/user"), Some(1));
		// Every prompt re-announces the same directory; that is not a move.
		assert_eq!(files.follow("/home/user"), None);

		// The tree points the pane somewhere the shell is not…
		assert_eq!(files.show("/etc"), Some(2));
		// …and the next announcement must leave it there, or looking inside a folder you
		// are not in would be impossible.
		assert_eq!(files.follow("/home/user"), None);
		assert_eq!(files.path(), Some("/etc"));

		// A real `cd` moves the pane again.
		assert_eq!(files.follow("/var/log"), Some(3));
	}

	#[test]
	fn batches_accumulate_until_the_last_one() {
		let mut files = Files::default();
		let request = files.show("/home").expect("a new directory needs listing");

		files.chunk(request, vec![entry("a", Kind::File)], false);
		assert!(files.loading(), "more batches are still coming");
		assert_eq!(files.count(), 1);

		files.chunk(request, vec![entry("b", Kind::File)], true);
		assert!(!files.loading());
		assert_eq!(files.count(), 2);
	}

	#[test]
	fn a_batch_for_a_directory_we_have_left_is_dropped() {
		let mut files = Files::default();
		let stale = files.show("/home").expect("a new directory needs listing");
		files
			.show("/etc")
			.expect("a second directory needs listing");

		// The old listing is still running on the server; its batches must not be mixed
		// into the folder now on show.
		files.chunk(stale, vec![entry("user", Kind::Dir)], true);
		assert_eq!(files.count(), 0);
		assert!(files.loading(), "the /etc listing is still in flight");

		files.failed(stale, "gone".to_owned());
		assert_eq!(files.notice(), None);
	}

	#[test]
	fn hidden_entries_are_filtered_by_the_shared_toggle() {
		let (files, _) = pane(&[entry(".bashrc", Kind::File), entry("notes", Kind::File)]);
		assert_eq!(files.rows(true).len(), 2);
		let shown = files.rows(false);
		assert_eq!(shown.len(), 1);
		assert_eq!(shown[0].name, "notes");
	}

	#[test]
	fn the_toggle_hides_nothing_but_dot_names_and_never_shows_the_dot_links() {
		let (files, _) = pane(&[
			entry(".", Kind::Dir),
			entry("..", Kind::Dir),
			entry(".hidden", Kind::File),
			entry("...odd", Kind::File),
			entry("normal", Kind::File),
			entry("link", Kind::Link),
		]);
		// Everything the server listed is here except the self and parent links.
		let names: Vec<&str> = files
			.rows(true)
			.iter()
			.map(|entry| entry.name.as_str())
			.collect();
		assert_eq!(names, vec![".hidden", "...odd", "normal", "link"]);
		assert_eq!(files.count(), 4, "the links are dropped, not just unshown");
	}

	#[test]
	fn the_up_button_has_somewhere_to_go_unless_the_pane_is_at_the_root() {
		// The one question behind both halves of the button: the toolbar asks it to decide
		// whether to enable, and the handler asks it again when the press arrives.
		let mut files = Files::default();
		let parent = |files: &Files| {
			files
				.path()
				.and_then(crate::explorer::parent)
				.map(str::to_owned)
		};
		assert_eq!(parent(&files), None, "nothing listed yet");

		let _ = files.show("/home/user");
		assert_eq!(parent(&files).as_deref(), Some("/home"));

		let _ = files.show(crate::explorer::ROOT);
		assert_eq!(parent(&files), None, "the root has nowhere above it");
	}

	#[test]
	fn the_arrow_keys_walk_the_grid_and_stop_at_both_ends() {
		let (mut files, _) = pane(&[
			entry("a", Kind::File),
			entry("b", Kind::File),
			entry("c", Kind::File),
			entry("d", Kind::File),
			entry("e", Kind::File),
		]);
		let selected = |files: &Files| files.cursor().unwrap_or("none").to_owned();

		// Nothing selected: forward starts at the top, backward at the bottom.
		files.step(true, 1, false);
		assert_eq!(selected(&files), "/home/a");
		files.step(true, 2, false); // a row down, on a two-column grid
		assert_eq!(selected(&files), "/home/c");
		files.step(true, -1, false);
		assert_eq!(selected(&files), "/home/b");

		// Both ends clamp rather than wrap: past the last row is the last row.
		files.step(true, 99, false);
		assert_eq!(selected(&files), "/home/e");
		files.step(true, -99, false);
		assert_eq!(selected(&files), "/home/a");
		assert_eq!(files.selected_index(true), Some(0));

		// A hidden name is not a step the keyboard can land on with the toggle off.
		let (mut hidden, _) = pane(&[entry(".ssh", Kind::Dir), entry("notes", Kind::File)]);
		hidden.step(false, 1, false);
		assert_eq!(selected(&hidden), "/home/notes");
		assert_eq!(hidden.selected_index(false), Some(0));

		// An empty directory has nowhere to go, and must not panic trying.
		let mut empty = Files::default();
		let request = empty.show("/home").expect("a new directory needs listing");
		empty.chunk(request, Vec::new(), true);
		empty.step(true, 1, false);
		assert_eq!(empty.cursor(), None);
	}

	#[test]
	fn a_set_of_entries_can_be_built_by_range_by_toggle_and_by_band() {
		let (mut files, _) = pane(&[
			entry("a", Kind::File),
			entry("b", Kind::File),
			entry("c", Kind::File),
			entry("d", Kind::File),
		]);
		let chosen = |files: &Files| {
			files
				.selected_rows(true)
				.into_iter()
				.map(|(path, _)| path)
				.collect::<Vec<_>>()
		};

		// A plain click is exclusive; Shift+click runs from it to the new end, either way.
		files.select("/home/b");
		files.extend_selection(true, "/home/d");
		assert_eq!(chosen(&files), ["/home/b", "/home/c", "/home/d"]);
		files.extend_selection(true, "/home/a");
		assert_eq!(
			chosen(&files),
			["/home/a", "/home/b"],
			"back past the anchor"
		);

		// Shift+arrow keeps extending from that same anchor — the cursor is on `a` after
		// that last Shift+click, so two forward lands on `c` and the range is `b`..`c`.
		files.step(true, 2, true);
		assert_eq!(chosen(&files), ["/home/b", "/home/c"]);
		assert_eq!(files.cursor(), Some("/home/c"));

		// Ctrl+click adds one, then takes the same one back out.
		files.toggle_selection("/home/a");
		assert_eq!(chosen(&files), ["/home/a", "/home/b", "/home/c"]);
		files.toggle_selection("/home/a");
		assert_eq!(chosen(&files), ["/home/b", "/home/c"]);

		// A plain band replaces the selection; an additive one adds to what was there.
		files.begin_band(Point::new(0.0, 40.0), false);
		files.set_band_selection(vec!["/home/a".to_owned()]);
		assert_eq!(chosen(&files), ["/home/a"]);
		files.end_band();
		files.begin_band(Point::new(0.0, 40.0), true);
		files.set_band_selection(vec!["/home/c".to_owned(), "/home/d".to_owned()]);
		assert_eq!(chosen(&files), ["/home/a", "/home/c", "/home/d"]);
		assert_eq!(
			files.cursor(),
			Some("/home/d"),
			"the last cell the band reached"
		);
		files.end_band();

		files.deselect();
		assert_eq!(files.selected_count(), 0);
	}

	#[test]
	fn a_link_target_belongs_to_the_selection_that_asked_for_it() {
		let (mut files, _) = pane(&[entry("latest", Kind::Link), entry("notes", Kind::File)]);
		files.select("/home/latest");
		assert_eq!(files.link_target(), None, "not resolved yet");

		files.set_link_target("/home/latest".to_owned(), "/srv/build-42".to_owned());
		assert_eq!(files.link_target(), Some("/srv/build-42"));

		// The answer is for a link the user has moved off: it is not this entry's target.
		files.select("/home/notes");
		assert_eq!(files.link_target(), None);
	}

	#[test]
	fn an_mtime_reads_as_the_servers_own_wall_clock() {
		let utc = Zone::default();
		assert_eq!(format_mtime(1_774_000_000, &utc), "2026-03-20 09:46:40 UTC");
		// East of Greenwich the same instant is later in the day…
		let paris = Zone {
			offset: 120,
			label: "CEST".to_owned(),
		};
		assert_eq!(
			format_mtime(1_774_000_000, &paris),
			"2026-03-20 11:46:40 CEST (+02:00)"
		);
		// …and west of it, earlier — here far enough to be the previous day, which is
		// what the euclidean division is for.
		let honolulu = Zone {
			offset: -600,
			label: "HST".to_owned(),
		};
		assert_eq!(
			format_mtime(1_774_000_000, &honolulu),
			"2026-03-19 23:46:40 HST (-10:00)"
		);
		// The epoch itself, and a leap day, pin the calendar arithmetic.
		assert_eq!(format_mtime(0, &utc), "1970-01-01 00:00:00 UTC");
		assert_eq!(format_mtime(1_709_164_800, &utc), "2024-02-29 00:00:00 UTC");
	}

	#[test]
	fn the_short_mtime_drops_the_seconds_but_keeps_the_zone() {
		// The grid cell's compact form: same instant, same zone shift and same zone tag as the
		// full format, but trimmed to the day and the minute — the seconds go, the zone stays.
		let utc = Zone::default();
		assert_eq!(
			format_mtime_short(1_774_000_000, &utc),
			"2026-03-20 09:46 UTC"
		);
		let paris = Zone {
			offset: 120,
			label: "CEST".to_owned(),
		};
		assert_eq!(
			format_mtime_short(1_774_000_000, &paris),
			"2026-03-20 11:46 CEST (+02:00)"
		);
	}

	#[test]
	fn the_zone_probe_reads_dates_own_answer() {
		assert_eq!(
			parse_zone("+0200 CEST\n"),
			Some(Zone {
				offset: 120,
				label: "CEST".to_owned()
			})
		);
		assert_eq!(
			parse_zone("-0930 MART"),
			Some(Zone {
				offset: -570,
				label: "MART".to_owned()
			})
		);
		// No label is still a usable answer; anything that is not an offset is not.
		assert_eq!(parse_zone("+0000").map(|zone| zone.offset), Some(0));
		for junk in ["", "CEST", "0200", "+02", "+02:00", "+abcd"] {
			assert_eq!(parse_zone(junk), None, "{junk} is not an offset");
		}
	}

	#[test]
	fn the_owner_and_group_names_come_out_of_the_listings_own_ls_line() {
		assert_eq!(
			parse_longname("-rw-r--r--    1 cme      staff        4311 Jul 24 18:03 notes.txt")
				.as_ref()
				.map(|(owner, group)| (owner.as_str(), group.as_str())),
			Some(("cme", "staff"))
		);
		// A directory line, and one whose file name carries spaces — the name is past
		// every column we read, so it cannot disturb them.
		assert_eq!(
			parse_longname("drwxr-xr-x 2 root root 4096 Jan  1  2026 my old notes")
				.as_ref()
				.map(|(owner, group)| (owner.as_str(), group.as_str())),
			Some(("root", "root"))
		);
		// Not an ls line: the server said nothing usable, so the caller keeps the numbers.
		for junk in [
			"",
			"notes.txt",
			"rw-r--r-- 1 cme staff 4311 Jul 24 18:03 x",
			"d 1 a b c",
		] {
			assert_eq!(parse_longname(junk), None, "{junk:?} is not an ls line");
		}
	}

	#[test]
	fn owner_and_group_survive_a_server_that_only_half_answers() {
		let named = Meta {
			owner: Some("cme".to_owned()),
			group: Some("staff".to_owned()),
			..Meta::default()
		};
		assert_eq!(owner_group(&named).as_deref(), Some("cme:staff"));
		let half = Meta {
			owner: Some("1000".to_owned()),
			..Meta::default()
		};
		assert_eq!(owner_group(&half).as_deref(), Some("1000:?"));
		// The `ls` fallback reports neither, and the popup then shows no owner line.
		assert_eq!(owner_group(&Meta::default()), None);
	}

	#[test]
	fn folders_sort_before_files_case_insensitively() {
		let mut entries = vec![
			entry("zebra", Kind::File),
			entry("Apple", Kind::File),
			entry("src", Kind::Dir),
			entry("banana", Kind::File),
			entry("Docs", Kind::Dir),
		];
		sort(&mut entries);
		let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
		assert_eq!(names, vec!["Docs", "src", "Apple", "banana", "zebra"]);
	}

	#[test]
	fn a_rename_needs_a_real_new_name() {
		let (mut files, _) = pane(&[entry("notes.txt", Kind::File)]);

		for text in ["notes.txt", "  ", "sub/notes.txt"] {
			files.start_rename("/home/notes.txt".to_owned());
			files.edit_rename(text.to_owned());
			assert_eq!(files.commit_rename(), None, "{text} should not rename");
		}

		files.start_rename("/home/notes.txt".to_owned());
		files.edit_rename("  todo.txt  ".to_owned());
		assert_eq!(
			files.commit_rename(),
			Some(("/home/notes.txt".to_owned(), "/home/todo.txt".to_owned()))
		);
	}

	#[test]
	fn only_a_rename_in_the_open_directory_re_lists_it() {
		let (mut files, request) = pane(&[entry("notes.txt", Kind::File)]);
		// Renamed elsewhere (the tree can rename any folder): nothing to redraw here.
		assert_eq!(files.renamed("/etc/hosts"), None);
		// Renamed in view: the entry has to move to its new sort position, so re-list.
		assert_eq!(files.renamed("/home/notes.txt"), Some(request + 1));
	}

	#[test]
	fn icons_come_from_the_kind_then_the_extension() {
		let cases = [
			("src", Kind::Dir, Category::Folder),
			("latest", Kind::Link, Category::Link),
			("logo.PNG", Kind::File, Category::Image), // case-insensitive
			("main.rs", Kind::File, Category::Code),
			("backup.tar.gz", Kind::File, Category::Archive),
			("report.pdf", Kind::File, Category::Document),
			("song.flac", Kind::File, Category::Audio),
			("clip.mkv", Kind::File, Category::Video),
			("mystery.qqq", Kind::File, Category::Plain),
			("README", Kind::File, Category::Plain),
			// A leading dot is not an extension: `.bashrc` is a name, not a "bashrc" file.
			(".bashrc", Kind::File, Category::Plain),
		];
		for (name, kind, expected) in cases {
			assert_eq!(category(&entry(name, kind)), expected, "{name}");
		}
	}

	#[test]
	fn a_type_comes_out_of_the_extension_and_lands_on_octet_stream_when_it_cannot() {
		let cases = [
			("logo.PNG", "image/png"), // case-insensitive, like the icons
			("backup.tar.gz", "application/gzip"),
			("notes.md", "text/markdown"),
			("mystery.qqq", OCTET_STREAM),
			("README", OCTET_STREAM),
			(".bashrc", OCTET_STREAM), // a name, not a "bashrc" file
		];
		for (name, expected) in cases {
			assert_eq!(mime(name), expected, "{name}");
		}
	}

	#[test]
	fn the_menu_anchor_is_frozen_when_it_opens() {
		let (mut files, _) = pane(&[entry("src", Kind::Dir)]);
		files.set_pointer(Point::new(40.0, 20.0));
		files.open_menu("/home/src".to_owned());

		files.set_pointer(Point::new(300.0, 90.0));
		let menu = files.menu().expect("the menu is open");
		assert_eq!(menu.at, Point::new(40.0, 20.0));
		// The kind travels with it: Download is meaningless on a directory.
		assert_eq!(menu.kind, Kind::Dir);
	}

	#[test]
	fn a_hidden_pane_takes_no_room_from_the_grid() {
		let mut files = Files::default();
		assert_eq!(files.reserved(), DEFAULT_HEIGHT + SPLITTER_HEIGHT);
		files.toggle();
		assert_eq!(files.reserved(), 0.0);

		files.set_height(10.0, 400.0);
		assert_eq!(files.height(), MIN_HEIGHT);
		files.set_height(5_000.0, 400.0);
		assert_eq!(files.height(), 400.0);
	}

	#[test]
	fn no_sort_leaves_the_rows_in_arrival_order() {
		// The server task pre-sorts before batching, so with no sort chosen the pane must hand
		// the entries back exactly as they landed — it adds no order of its own.
		let (files, _) = pane(&[
			entry("src", Kind::Dir),
			sized("apple.txt", 10, 100),
			sized("banana.txt", 20, 200),
		]);
		assert_eq!(files.sort_key(), None);
		assert_eq!(names(&files), ["src", "apple.txt", "banana.txt"]);
	}

	#[test]
	fn picking_the_lit_key_again_clears_the_sort() {
		let (mut files, _) = pane(&[entry("a", Kind::File)]);
		files.pick_sort_key(SortKey::Size);
		assert_eq!(files.sort_key(), Some(SortKey::Size));
		// The menu has no "None" row: the lit key is the way back to the default order.
		files.pick_sort_key(SortKey::Size);
		assert_eq!(files.sort_key(), None);
		// A different key just switches, it does not clear.
		files.pick_sort_key(SortKey::Name);
		files.pick_sort_key(SortKey::Modified);
		assert_eq!(files.sort_key(), Some(SortKey::Modified));
	}

	#[test]
	fn sorting_by_size_descending_keeps_folders_first() {
		let (mut files, _) = pane(&[
			sized("small.bin", 10, 100),
			entry("zzz_dir", Kind::Dir),
			sized("big.bin", 900, 200),
			entry("aaa_dir", Kind::Dir),
			sized("mid.bin", 400, 300),
		]);
		files.pick_sort_key(SortKey::Size);
		files.pick_sort_dir(SortDir::Descending);
		// Folders stay grouped at the top whatever the direction — that grouping is what the
		// direction never flips. WITHIN the group it does flip: the folders have no size, so they
		// tie on it and fall back to name, which descending then runs Z→A (zzz before aaa). The
		// files follow, biggest first.
		assert_eq!(
			names(&files),
			["zzz_dir", "aaa_dir", "big.bin", "mid.bin", "small.bin"]
		);
	}

	#[test]
	fn sorting_by_name_descending_reverses_within_each_group() {
		let (mut files, _) = pane(&[
			entry("alpha", Kind::Dir),
			entry("beta", Kind::Dir),
			entry("x.txt", Kind::File),
			entry("y.txt", Kind::File),
		]);
		files.pick_sort_key(SortKey::Name);
		files.pick_sort_dir(SortDir::Descending);
		// Folders still lead, but each group runs Z→A.
		assert_eq!(names(&files), ["beta", "alpha", "y.txt", "x.txt"]);
	}

	#[test]
	fn sorting_by_extension_groups_like_kinds_then_falls_back_to_name() {
		let (mut files, _) = pane(&[
			sized("c.txt", 1, 1),
			sized("b.rs", 1, 1),
			sized("a.txt", 1, 1),
			sized("d", 1, 1), // no extension sorts as the empty string, ahead of the rest
		]);
		files.pick_sort_key(SortKey::Extension);
		files.pick_sort_dir(SortDir::Ascending);
		// "" (d) < "rs" (b) < "txt" (a, c), and the name settles the two .txt files.
		assert_eq!(names(&files), ["d", "b.rs", "a.txt", "c.txt"]);
	}

	#[test]
	fn a_key_alone_sorts_ascending_and_the_lit_order_toggles_off() {
		// The order is a tri-state now: unset, ascending or descending. A key with the order left
		// unset must already sort — ascending — so picking just a criteria is enough.
		let (mut files, _) = pane(&[
			sized("b.txt", 20, 200),
			sized("a.txt", 10, 100),
			sized("c.txt", 30, 300),
		]);
		files.pick_sort_key(SortKey::Name);
		assert_eq!(files.sort_dir(), None, "order starts unset");
		assert_eq!(
			names(&files),
			["a.txt", "b.txt", "c.txt"],
			"unset sorts ascending"
		);

		// Picking Descending flips it; picking the now-lit Descending again unsets the order, which
		// falls back to ascending — the exact twin of clearing the lit key.
		files.pick_sort_dir(SortDir::Descending);
		assert_eq!(files.sort_dir(), Some(SortDir::Descending));
		assert_eq!(names(&files), ["c.txt", "b.txt", "a.txt"]);
		files.pick_sort_dir(SortDir::Descending);
		assert_eq!(files.sort_dir(), None, "clicking the lit order unsets it");
		assert_eq!(
			names(&files),
			["a.txt", "b.txt", "c.txt"],
			"back to ascending"
		);
	}

	#[test]
	fn opening_the_sort_menu_shuts_a_context_menu_and_vice_versa() {
		let (mut files, _) = pane(&[entry("src", Kind::Dir)]);
		files.open_pane_menu();
		assert!(files.pane_menu().is_some());

		// The sort menu takes the one surface: the context menu closes.
		files.toggle_sort_menu();
		assert!(files.sort_menu_open());
		assert!(files.pane_menu().is_none());

		// And opening a context menu closes the sort menu right back.
		files.open_menu("/home/src".to_owned());
		assert!(!files.sort_menu_open());
		assert!(files.menu().is_some());
	}
}

// explorer.rs — the remote folder tree's model (PLAN §18).
//
// The panel beside the terminal shows the remote filesystem as a tree of folders.
// This module is the *model* only: which folders are known, which are open, what is
// selected, and the path arithmetic the context menu needs. It touches neither the
// network nor the widget tree — `ssh::browse` fetches listings and `ui::explorer`
// draws them — so every rule here (what a `..` path looks like, what collapsing a
// branch does, which folders a `cd` reveals) is plain data and unit-testable.
//
// Paths are POSIX. SFTP puts `/`-separated paths on the wire regardless of the
// server's platform, so the tree speaks one dialect and never has to guess.
//
// The tree is *lazy*: a folder's children are unknown until something asks for them.
// `expand` / `reveal_if_new` therefore return the paths that still need fetching, and
// the app turns each into one `SshCommand::ListDir`. A remote filesystem is far too
// big to walk eagerly, and a bounded fetch is also what keeps a hostile or enormous
// directory from stalling the panel.

use std::collections::BTreeMap;

/// The tree's root. Every path in the model hangs off this one.
pub const ROOT: &str = "/";

/// The panel's starting width, the narrowest the splitter may drag it to, and the
/// grab bar's own width (§18). `ui::terminal` subtracts the panel plus the bar from
/// the grid, so these three are the single source of truth for that arithmetic.
pub const DEFAULT_WIDTH: f32 = 280.0;
pub const MIN_WIDTH: f32 = 160.0;
pub const SPLITTER_WIDTH: f32 = 6.0;

/// Everything the explorer panel can ask the app to do (§18). Nested under
/// `Message::Explorer` rather than flattened into `Message`, the way `SshEvent`
/// already is — a dozen more top-level variants would bury the rest of the enum.
#[derive(Debug, Clone)]
pub enum ExplorerMessage {
	/// Show or hide the whole panel (the status-bar button).
	Toggled,
	/// Show or hide dot-prefixed folders (the panel header's toggle).
	HiddenToggled,
	/// A row was left-clicked: select it and open/close it.
	RowClicked(String),
	/// A row was right-clicked: select it and open the context menu on it.
	RowRightClicked(String),
	/// The pointer moved over the panel; the payload is its panel-local position. Tracked
	/// because a right-press carries no coordinates of its own, and the menu is placed
	/// under the cursor.
	PointerMoved(iced::Point),
	/// Dismiss the context menu without choosing an item.
	MenuDismissed,
	/// A press landed anywhere in the panel — give it the keyboard (§20).
	PanelPressed,
	/// The tree was scrolled; the payload is its absolute vertical offset. Tracked so
	/// arrow-key navigation can tell whether the row it moved to is already on screen (§20).
	Scrolled(f32),
	/// Menu "Refresh" (one folder): re-list this folder's *contents*, and re-list its *parent* so
	/// its own name and existence are checked too — a rename or deletion made from the shell shows
	/// up in the parent's listing, not the folder's. So a right-click Refresh answers "is it still
	/// there, under this name, holding these children?" in one go. Named "Refresh", not "Expand",
	/// because that is the word a user hunts for when the tree has gone stale under a shell command.
	RefreshDir(String),
	/// The header ↻ button and F5 (§18): re-list every open folder in one action, so all the
	/// expanded content is current at once — the fix for a tree left stale by a `mv` or a
	/// `mkdir` typed in the console, where nothing in the GUI knew to re-fetch.
	RefreshTree,
	/// The header's collapse-all button (§18): close every branch back to the root's own children,
	/// the clean top-level view after exploring deep. A single folder still collapses by clicking
	/// its open row or pressing ←; only the menu item is gone.
	CollapseAll,
	/// Menu "Open in terminal": send a `cd` for this folder to the shell.
	Cd(String),
	/// Menu "Upload…": pick local files to send into this folder (§17). Carries the folder's
	/// path, so the files land in the one that was right-clicked, not wherever the shell sits.
	UploadHere(String),
	/// Menu "Upload folder…": pick a local folder to send, tree and all, into this one (§17).
	UploadFolderHere(String),
	/// Menu "New folder…": open the dialog to create a subfolder inside this one (§18).
	NewFolderHere(String),
	/// Menu "Delete…": open the confirmation to remove this folder and everything inside it (§18).
	DeleteStarted(String),
	/// Menu "Rename": turn the row into an edit field.
	RenameStarted(String),
	/// The inline rename field changed.
	RenameEdited(String),
	/// The inline rename was submitted (Enter) — ask the server to do it.
	RenameCommitted,
	/// Menu "Copy name" / "Copy relative path" / "Copy full path".
	CopyName(String),
	CopyRelative(String),
	CopyPath(String),
	/// The header's "copy path" button (§22): copy the directory on show — the files
	/// view's path, the one this header names — verbatim. Carries no path of its own;
	/// `app` reads it live, so the button and the header can never name different dirs.
	CopyCurrentPath,
	/// The splitter was pressed — begin resizing the panel.
	SplitterGrabbed,
	/// The pointer moved while resizing; the payload is its window position.
	SplitterDragged(iced::Point),
	/// The resize ended (pointer released).
	SplitterReleased,
	/// The pointer entered, or left, the splitter bar (§18). Only drives the bar's highlight —
	/// the visual cue that it is grabbable — so it carries nothing but which way it went.
	SplitterEntered,
	SplitterExited,
}

/// One folder as the model knows it. A node exists only once something has referred
/// to the folder — expanded it, revealed it, or listed its parent.
#[derive(Debug, Default)]
struct Node {
	/// The child folder *names* (not paths), sorted. `None` means "never listed";
	/// an empty vector means "listed, and it holds no folders" — the difference is
	/// what stops a failed or empty directory being re-fetched forever.
	children: Option<Vec<String>>,
	/// Whether the row is expanded in the view.
	open: bool,
	/// A listing is in flight for this folder.
	loading: bool,
}

/// An open context menu: the folder it acts on, and where it is drawn (§18). The anchor
/// is captured when the menu opens, NOT read live — otherwise the panel keeps reporting
/// pointer moves and the menu slides away from under the cursor before it can be clicked.
#[derive(Debug, Clone)]
pub struct Menu {
	pub path: String,
	pub at: iced::Point,
}

/// The in-progress inline rename: which folder, and the text typed so far (§18).
/// Same shape as the home screen's rename (§14), because it is the same interaction.
#[derive(Debug, Clone)]
pub struct Rename {
	pub path: String,
	pub text: String,
}

/// One visible line of the tree, as `ui::explorer` draws it. Produced by `rows`,
/// which flattens the open branches in display order.
#[derive(Debug, Clone)]
pub struct Row {
	pub path: String,
	pub name: String,
	pub depth: u16,
	pub open: bool,
	pub loading: bool,
}

/// The remote folder tree (§18).
#[derive(Debug)]
pub struct Explorer {
	visible: bool,
	show_hidden: bool,
	width: f32,
	dragging: bool,
	/// Whether the pointer is over the splitter right now (§18). Together with `dragging` it
	/// lights the bar so the user sees it is grabbable before pressing — the hover half of the
	/// same feedback the resize cursor gives.
	splitter_hovered: bool,
	nodes: BTreeMap<String, Node>,
	selected: Option<String>,
	menu: Option<Menu>,
	/// The last pointer position over the panel, in panel-local coordinates. Only read
	/// when a menu opens, which freezes it as that menu's anchor (§18).
	pointer: iced::Point,
	rename: Option<Rename>,
	notice: Option<String>,
	/// How far the tree is scrolled, in pixels (§20).
	scroll: f32,
	/// The last working directory that was revealed, so the shell announcing the same
	/// directory on every prompt does not re-open the chain (or re-fetch it) each time.
	revealed: Option<String>,
}

impl Default for Explorer {
	fn default() -> Self {
		Self {
			// The panel is the headline of this version, so it starts open; the initial
			// window is sized to fit it *and* the intended grid (`ui::terminal`).
			visible: true,
			// Dot-prefixed folders are shown by default: on a server, `.ssh` / `.config`
			// are usually the reason you opened the tree. The header toggles them off.
			show_hidden: true,
			width: DEFAULT_WIDTH,
			dragging: false,
			splitter_hovered: false,
			nodes: BTreeMap::new(),
			selected: None,
			menu: None,
			pointer: iced::Point::ORIGIN,
			rename: None,
			notice: None,
			scroll: 0.0,
			revealed: None,
		}
	}
}

impl Explorer {
	/// Whether the panel is showing.
	pub fn visible(&self) -> bool {
		self.visible
	}

	/// Whether dot-prefixed folders are listed.
	pub fn show_hidden(&self) -> bool {
		self.show_hidden
	}

	/// The panel's current width in logical pixels (the tree area, without the splitter).
	pub fn width(&self) -> f32 {
		self.width
	}

	/// Whether the splitter is being dragged right now (§18) — the app adds a
	/// pointer-capture layer while it is, the same way a dragged dialog does (§10).
	pub fn dragging(&self) -> bool {
		self.dragging
	}

	/// Whether the splitter should be drawn lit (§18): while the pointer is over it, or while
	/// it is being dragged — either way it is the active handle, so the bar brightens.
	pub fn splitter_active(&self) -> bool {
		self.dragging || self.splitter_hovered
	}

	/// The pointer entered or left the splitter bar (§18) — drives only its highlight.
	pub fn set_splitter_hovered(&mut self, hovered: bool) {
		self.splitter_hovered = hovered;
	}

	/// How much horizontal room the tree takes from the files pane beside it (§18, §19): the
	/// tree plus its splitter, or nothing at all when hidden. The pane's width is the window
	/// less exactly this, so the pane's grid math and the strip layout can never drift. The
	/// terminal reserves no width any more — the tree sits under it now, not beside it.
	pub fn reserved(&self) -> f32 {
		if self.visible {
			self.width + SPLITTER_WIDTH
		} else {
			0.0
		}
	}

	/// The selected folder's path, if any.
	pub fn selected(&self) -> Option<&str> {
		self.selected.as_deref()
	}

	/// Which visible row the selection is on (§20), for the arrow keys to step from and
	/// for the app to scroll back into view. `None` when nothing is selected, or when the
	/// selected folder is inside a branch that has since been collapsed.
	pub fn selected_index(&self) -> Option<usize> {
		let selected = self.selected.as_deref()?;
		self.rows().iter().position(|row| row.path == selected)
	}

	/// Move the selection `delta` visible rows (§20). Clamped at both ends, and with
	/// nothing selected a forward step starts at the root — the same rule the files pane
	/// follows, because it is the same key doing it.
	pub fn step(&mut self, delta: isize) {
		let rows = self.rows();
		let Some(last) = rows.len().checked_sub(1) else {
			return;
		};
		let last = last as isize;
		let next = match self.selected_index() {
			Some(index) => (index as isize).saturating_add(delta),
			None if delta >= 0 => 0,
			None => last,
		};
		self.selected = Some(rows[next.clamp(0, last) as usize].path.clone());
	}

	/// How far the tree is scrolled (§20).
	pub fn scroll(&self) -> f32 {
		self.scroll
	}

	/// Remember the tree's scroll offset — reported by the scrollable, and set by the app
	/// when it scrolls a keyboard-moved row back into view.
	pub fn set_scroll(&mut self, scroll: f32) {
		self.scroll = scroll.max(0.0);
	}

	/// The open context menu — the folder it acts on and where it is drawn — if any.
	pub fn menu(&self) -> Option<&Menu> {
		self.menu.as_ref()
	}

	/// Remember where the pointer is, so a right-press — which carries no coordinates —
	/// can open the menu under it.
	pub fn set_pointer(&mut self, pointer: iced::Point) {
		self.pointer = pointer;
	}

	/// The in-progress inline rename, if any.
	pub fn editing(&self) -> Option<&Rename> {
		self.rename.as_ref()
	}

	/// The last thing that went wrong (a refused listing, a failed rename), shown as a
	/// line under the tree until the next one replaces it.
	pub fn notice(&self) -> Option<&str> {
		self.notice.as_deref()
	}

	/// Show or hide the panel. Hiding gives its width back to the grid, so the caller
	/// refits the terminal afterwards.
	pub fn toggle(&mut self) {
		self.visible = !self.visible;
		self.menu = None;
	}

	/// Show or hide dot-prefixed folders. Purely a display filter — nothing is
	/// re-fetched, because listings always include them.
	pub fn toggle_hidden(&mut self) {
		self.show_hidden = !self.show_hidden;
		self.menu = None;
	}

	/// Apply the dotfile preference remembered with the target we just connected to
	/// (§14). Same display filter as `toggle_hidden`, set rather than flipped.
	pub fn set_hidden(&mut self, show_hidden: bool) {
		self.show_hidden = show_hidden;
	}

	/// Resize the panel from a splitter drag, clamped between `MIN_WIDTH` and `max`
	/// (the app passes a fraction of the window, so the grid can never be squeezed out).
	pub fn set_width(&mut self, width: f32, max: f32) {
		self.width = width.clamp(MIN_WIDTH, max.max(MIN_WIDTH));
	}

	/// Start / stop a splitter drag.
	pub fn set_dragging(&mut self, dragging: bool) {
		self.dragging = dragging;
	}

	/// Select a folder (a click on its row, or the folder a `cd` revealed).
	pub fn select(&mut self, path: &str) {
		self.selected = Some(path.to_owned());
	}

	/// Open the context menu on a folder, anchored where the pointer is right now. The
	/// anchor is a snapshot: the panel goes on reporting moves while the menu is up, and
	/// a menu that tracked them would walk out from under the cursor.
	pub fn open_menu(&mut self, path: String) {
		self.menu = Some(Menu {
			path,
			at: self.pointer,
		});
	}

	/// Close the context menu.
	pub fn close_menu(&mut self) {
		self.menu = None;
	}

	/// Drop everything the tree knows (§18). Called when a session opens or closes, so
	/// one server's directories never show up under the next one's.
	pub fn reset(&mut self) {
		self.nodes.clear();
		self.selected = None;
		self.menu = None;
		self.rename = None;
		self.notice = None;
		self.revealed = None;
		self.dragging = false;
		self.scroll = 0.0;
	}

	/// Open a folder, returning the path to list when its children need fetching.
	///
	/// A folder is re-listed whenever this call is what *opens* it — a genuine closed→open
	/// transition — not only the first time. That is the rule that keeps the tree honest: a
	/// user who collapses a folder, changes it from the shell (`mv` a child out of it), then
	/// clicks it open again must see the new contents, not the stale cache. Opening is a
	/// deliberate act, so it always asks the server. The old children stay on screen under a
	/// spinner until the fresh listing lands (`listed`), so the row never flashes empty.
	///
	/// `force` re-lists even without an open transition — an already-open folder — which is
	/// what the menu's Refresh and a completed rename need to pull fresh contents into a
	/// branch that is already showing.
	///
	/// A folder already loading, or one already open and cached when neither opening nor
	/// forced, needs nothing: `None` then. This last case is what keeps `reveal_if_new` from
	/// re-listing ancestors that are already open on every `cd`.
	pub fn expand(&mut self, path: &str, force: bool) -> Option<String> {
		let node = self.nodes.entry(path.to_owned()).or_default();
		let opening = !node.open; // this call is the closed→open transition
		node.open = true;
		if node.loading {
			return None;
		}
		if node.children.is_some() && !force && !opening {
			return None;
		}
		node.loading = true;
		Some(path.to_owned())
	}

	/// Close a folder *and every folder under it*, so re-opening it shows one clean
	/// level again (§18). Collapsing discards nothing — the cached children stay, so the
	/// row draws them instantly on re-open (no empty flash) while `expand` re-lists in the
	/// background to catch any shell-side change.
	pub fn collapse(&mut self, path: &str) {
		let prefix = format!("{}/", path.trim_end_matches('/'));
		for (key, node) in self.nodes.iter_mut() {
			if key == path || key.starts_with(&prefix) {
				node.open = false;
			}
		}
	}

	/// Collapse every branch back to the top level (§18): the header's collapse-all button. Closes
	/// each folder but the root, so the tree returns to showing just the root's own children — the
	/// clean starting view after a deep dive. Like `collapse`, this discards nothing: the cached
	/// listings stay, so a re-opened branch draws instantly while `expand` re-lists it in the
	/// background. The root is left open because it is the tree's anchor — closing it would
	/// collapse the panel to a single "/" row.
	pub fn collapse_all(&mut self) {
		for (key, node) in self.nodes.iter_mut() {
			if key.as_str() != ROOT {
				node.open = false;
			}
		}
	}

	/// A row click: select the folder and flip it open or shut. Returns a path to list when
	/// opening it needs one — and opening always re-lists (`expand`'s open transition), so a
	/// folder reopened after a shell-side change shows its current contents, not the cache.
	pub fn toggle_node(&mut self, path: &str) -> Option<String> {
		self.select(path);
		self.menu = None;
		if self.nodes.get(path).is_some_and(|node| node.open) {
			self.collapse(path);
			return None;
		}
		self.expand(path, false)
	}

	/// A listing came back: record the child folders and stop the spinner.
	pub fn listed(&mut self, path: &str, mut children: Vec<String>) {
		children.retain(|child| !is_dot_link(child));
		children.sort_unstable();
		let node = self.nodes.entry(path.to_owned()).or_default();
		node.children = Some(children);
		node.loading = false;
	}

	/// A listing failed (no permission, gone, the server refused). The folder is marked
	/// as holding nothing — otherwise every redraw would ask again — and the reason goes
	/// to the notice line. The path is the user's own, so showing it is what makes the
	/// message actionable (the same call as an upload failure, §17).
	pub fn failed(&mut self, path: &str, reason: String) {
		let node = self.nodes.entry(path.to_owned()).or_default();
		node.children = Some(Vec::new());
		node.loading = false;
		self.notice = Some(reason);
	}

	/// Put a message on the panel's notice line.
	pub fn set_notice(&mut self, notice: String) {
		self.notice = Some(notice);
	}

	/// Reveal the shell's working directory (§18): open every folder from the root down
	/// to it, select it, and return the paths that still need listing. Doing nothing when
	/// the directory has not changed is what makes this safe to call on every chunk of
	/// output — the shell re-announces the same path at every prompt.
	///
	/// `ponytail:` POSIX paths only. A remote that reports a native Windows directory
	/// (`C:\Users\…`, OSC 9;9 — §17) does not sit anywhere on this `/`-rooted tree, so it
	/// is left alone rather than revealed at a made-up place. Upgrade path: root the tree
	/// at the drive when the announced path carries one.
	pub fn reveal_if_new(&mut self, cwd: &str) -> Vec<String> {
		if self.revealed.as_deref() == Some(cwd) {
			return Vec::new();
		}
		self.reveal(cwd)
	}

	/// The same, asked for out loud (§19): open the chain down to `cwd` and select it whether or
	/// not the shell has announced that directory before.
	///
	/// Without the "has it changed" guard, which is why it is a separate entry point rather than an
	/// argument. The guard exists to keep the automatic call cheap and to stop a re-announcement
	/// undoing a browse — neither applies to a press of the **Reveal** button, whose whole purpose
	/// is to put the tree back where the shell is *after* it has been walked away from: collapse
	/// the branch, click elsewhere, and the cwd has not changed, so the guarded call would decline
	/// exactly when the user is asking for it.
	pub fn reveal(&mut self, cwd: &str) -> Vec<String> {
		if !cwd.starts_with('/') {
			return Vec::new();
		}
		self.revealed = Some(cwd.to_owned());

		let mut needed = Vec::new();
		for path in ancestors(cwd) {
			// Opening the directory itself as well as its parents means the tree shows
			// what is *inside* where the shell is, not just where it is.
			if let Some(fetch) = self.expand(&path, false) {
				needed.push(fetch);
			}
		}
		self.selected = Some(cwd.to_owned());
		needed
	}

	/// Seed the "has the shell moved" guard without opening or selecting anything (§22). The
	/// tree's half of the reconnect pin, and the exact mirror of `Files::set_followed`: while a
	/// resume is settling, the shell's login-then-`cd` announcements must not drag either panel
	/// off the directory the restore put it on — and the tree is the more expensive of the two to
	/// drag, since revealing a directory opens its whole chain and asks the server for a listing
	/// of every folder along it. Once the shell has settled, this marks its cwd as already seen,
	/// so the arrival moves nothing while a later, real `cd` still counts as a move.
	pub fn set_revealed(&mut self, cwd: &str) {
		self.revealed = Some(cwd.to_owned());
	}

	/// Begin renaming a folder. The root has no parent to rename it within, so it is
	/// left alone.
	pub fn start_rename(&mut self, path: String) {
		self.menu = None;
		if parent(&path).is_none() {
			return;
		}
		let text = name(&path).to_owned();
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
	/// A blank name, a name with a path separator in it (which would *move* the folder,
	/// not rename it), or no change at all just closes the edit and asks for nothing.
	pub fn commit_rename(&mut self) -> Option<(String, String)> {
		let rename = self.rename.take()?;
		let new_name = rename.text.trim();
		let parent = parent(&rename.path)?;
		if new_name.is_empty() || new_name.contains('/') || new_name == name(&rename.path) {
			return None;
		}
		Some((rename.path.clone(), join(parent, new_name)))
	}

	/// Re-list an already-known folder without opening or closing it (§18) — the refresh a
	/// create or a delete triggers, so a new child appears in place or a removed one vanishes.
	/// A folder never listed (nothing to refresh) or one already loading needs no fetch: `None`
	/// then. Unlike `expand`, this never forces a collapsed folder open — a change inside a
	/// closed branch is recorded for when it is next opened, not sprung into view.
	pub fn refresh_dir(&mut self, path: &str) -> Option<String> {
		let node = self.nodes.get_mut(path)?;
		if node.loading || node.children.is_none() {
			return None;
		}
		node.loading = true;
		Some(path.to_owned())
	}

	/// Re-list *every open folder* the tree is showing (§18): the whole-tree refresh behind the
	/// header ↻ button and F5. Each open, already-listed folder is marked loading and returned
	/// for the app to re-fetch, so a single action brings all the expanded content up to date —
	/// the user never has to work out which folders a `mv` touched. A folder that is closed
	/// (its rows are not on screen) or already loading is skipped: nothing changes under a
	/// branch you cannot see, and a fetch in flight will bring the fresh listing itself.
	pub fn refresh_open(&mut self) -> Vec<String> {
		let mut needed = Vec::new();
		for (path, node) in self.nodes.iter_mut() {
			if node.open && node.children.is_some() && !node.loading {
				node.loading = true;
				needed.push(path.clone());
			}
		}
		needed
	}

	/// Drop every folder's cached children but keep the SHAPE of the tree — which folders are open
	/// and which one is selected — and return the open ones to re-list (§46).
	///
	/// This is what an account switch needs, and it is deliberately harsher than `refresh_open`: a
	/// refresh leaves the old names on screen under a spinner because they are still that folder's
	/// names, only possibly stale. Here they are ANOTHER ACCOUNT's names. `cme` cannot see inside
	/// `/root`, root sees files in `/etc/ssl/private` that `cme` does not, and showing either set
	/// under the other account's name would be a lie about who is looking. So the contents go at
	/// once and the rows stand empty until the new account's listing lands — or, if it cannot list
	/// at all, stay empty beside the reason (§46).
	///
	/// The path a user was working in is kept on purpose: elevating BECAUSE a folder would not open
	/// is the ordinary reason to do it, so the same folder is exactly where they want to land.
	pub fn reread(&mut self) -> Vec<String> {
		let mut needed = Vec::new();
		for (path, node) in self.nodes.iter_mut() {
			node.children = None;
			node.loading = node.open;
			if node.open {
				needed.push(path.clone());
			}
		}
		// The notice belonged to the account that has just been left — its "permission denied" is
		// answered by the switch itself.
		self.notice = None;
		needed
	}

	/// Forget a folder and everything beneath it (§18): its subtree was just deleted, so those
	/// rows must go. A selection anywhere inside the gone subtree is dropped too, so the menu and
	/// the keyboard never point at a row that is no longer there. The parent's own cached child
	/// list still names it until the caller re-lists the parent (`refresh_dir`).
	pub fn forget(&mut self, path: &str) {
		let prefix = format!("{}/", path.trim_end_matches('/'));
		self.nodes
			.retain(|key, _| key != path && !key.starts_with(&prefix));
		if self
			.selected
			.as_deref()
			.is_some_and(|selected| selected == path || selected.starts_with(&prefix))
		{
			self.selected = None;
		}
	}

	/// A rename succeeded: forget everything the tree knew about the old path (its whole
	/// subtree moved with it), select the new one, and return the parent to re-list so the
	/// row reappears under its new name in the right sort position.
	pub fn renamed(&mut self, from: &str, to: &str) -> Option<String> {
		let prefix = format!("{}/", from.trim_end_matches('/'));
		self.nodes
			.retain(|key, _| key != from && !key.starts_with(&prefix));
		self.selected = Some(to.to_owned());
		self.notice = None;
		let parent = parent(to)?.to_owned();
		// Force the re-listing: the parent is already listed, and its old contents are
		// exactly what is now stale.
		self.expand(&parent, true)
	}

	/// The visible rows, top to bottom, as the panel draws them (§18): the root, then a
	/// depth-first walk of every open branch. Hidden folders are filtered here rather
	/// than at fetch time, so flipping the toggle costs nothing.
	pub fn rows(&self) -> Vec<Row> {
		let mut rows = Vec::new();
		self.push_rows(ROOT, ROOT, 0, &mut rows);
		rows
	}

	/// One row plus, when it is open, its children — the recursive half of `rows`.
	fn push_rows(&self, path: &str, name: &str, depth: u16, rows: &mut Vec<Row>) {
		let node = self.nodes.get(path);
		let open = node.is_some_and(|node| node.open);
		rows.push(Row {
			path: path.to_owned(),
			name: name.to_owned(),
			depth,
			open,
			loading: node.is_some_and(|node| node.loading),
		});
		if !open {
			return;
		}
		let Some(children) = node.and_then(|node| node.children.as_ref()) else {
			return;
		};
		for child in children {
			if !self.show_hidden && child.starts_with('.') {
				continue;
			}
			self.push_rows(&join(path, child), child, depth + 1, rows);
		}
	}
}

/// Join a folder path and a child name, POSIX-style, without doubling the root's slash.
pub fn join(directory: &str, child: &str) -> String {
	if directory.ends_with('/') {
		format!("{directory}{child}")
	} else {
		format!("{directory}/{child}")
	}
}

/// Whether `name` is usable as a single new folder or file name (§18): something after
/// trimming, and no path separator in it — a `/` would make it a *path*, not a name, and drop
/// the new entry somewhere other than where the user asked. The same rule the inline rename
/// enforces on the name it commits.
pub fn is_plain_name(name: &str) -> bool {
	let trimmed = name.trim();
	!trimmed.is_empty() && !trimmed.contains('/')
}

/// A path's own final component — the folder's name. The root is its own name.
pub fn name(path: &str) -> &str {
	let trimmed = path.trim_end_matches('/');
	if trimmed.is_empty() {
		return ROOT;
	}
	match trimmed.rfind('/') {
		Some(index) => &trimmed[index + 1..],
		None => trimmed,
	}
}

/// A path's containing folder, or `None` for the root (which has none).
pub fn parent(path: &str) -> Option<&str> {
	let trimmed = path.trim_end_matches('/');
	let index = trimmed.rfind('/')?;
	if index == 0 {
		Some(ROOT)
	} else {
		Some(&trimmed[..index])
	}
}

/// Whether a listed name is the `.` self-link or the `..` parent link. Both panels drop
/// these two at ingest and keep everything else — dot-prefixed, "hidden", whatever the
/// server considers a system file — because the `.*` toggle (§18, §19) is what decides
/// visibility, and these are not hidden entries: they are this folder and the one above
/// it, neither of which is a place to go from here. SFTP omits them today and `ls -A`
/// leaves them out, so this is the guard that makes it true of any listing source.
pub fn is_dot_link(name: &str) -> bool {
	matches!(name, "." | "..")
}

/// Every folder from the root down to `path`, inclusive — the chain `reveal_if_new`
/// opens. `/home/user` yields `/`, `/home`, `/home/user`.
pub fn ancestors(path: &str) -> Vec<String> {
	let mut chain = vec![ROOT.to_owned()];
	let mut current = String::new();
	for segment in path.split('/').filter(|segment| !segment.is_empty()) {
		current.push('/');
		current.push_str(segment);
		chain.push(current.clone());
	}
	chain
}

/// `to` expressed relative to `from` (§18) — what the context menu's "Copy relative
/// path" produces. Walks up with `..` for every level the two do not share, so the
/// result is always usable from the shell's current directory, even when the folder
/// sits on a different branch. Identical paths give `.`.
pub fn relative(from: &str, to: &str) -> String {
	let split = |path: &str| {
		path.split('/')
			.filter(|segment| !segment.is_empty())
			.map(str::to_owned)
			.collect::<Vec<_>>()
	};
	let from = split(from);
	let to = split(to);

	let shared = from
		.iter()
		.zip(to.iter())
		.take_while(|(left, right)| left == right)
		.count();

	let mut segments: Vec<&str> = vec![".."; from.len() - shared];
	segments.extend(to[shared..].iter().map(String::as_str));
	if segments.is_empty() {
		".".to_owned()
	} else {
		segments.join("/")
	}
}

/// Wrap a remote path for a POSIX shell (§18). Single quotes make everything inside
/// literal, and an embedded quote is closed, escaped and reopened — the standard
/// construction. A folder called `'; rm -rf ~` therefore reaches `cd` as a name, not
/// as commands, which matters because this string is typed straight into a live shell.
pub fn shell_quote(path: &str) -> String {
	format!("'{}'", path.replace('\'', r"'\''"))
}

/// How many `name-1`, `name-2`… candidates a "keep both" answer tries before it gives up (§17,
/// §19, §21). A hundred, because past that the folder is telling us something — and because on the
/// remote side every probe is a ROUND TRIP, so an unbounded search would hang a slow link on one
/// colliding file. The shell backend probes with `[ -e ]`, which is dearer than SFTP's own check
/// rather than cheaper, so it gets the same ceiling and not a looser one.
pub const FREE_NAME_TRIES: u32 = 100;

/// The `attempt`-th "keep both" candidate for a name already taken (§17, §19, §21): `notes.txt` at
/// attempt 1 is `notes-1.txt`.
///
/// Only the NAME — joining it onto a folder stays the caller's, because the callers join it four
/// ways (POSIX `/` for the remote, `PathBuf::push` locally) and only the SHAPE is shared. It was
/// written out five times before this, in three spellings, under two different caps, with three
/// different answers when the tries ran out. Sitting here it is one rule, next to the other
/// name-shaping ones, reachable from both the queue and the ssh layer without either depending on
/// the other.
///
/// The number goes BEFORE the extension so the copy still opens in the same program as the
/// original — `notes-1.txt` and not `notes.txt-1`. The split is `rsplit_once('.')` guarded on a
/// non-empty stem, and the guard is what makes a DOT-FILE keep its whole name: `.bashrc` has no
/// extension to preserve, it is all name, so it becomes `.bashrc-1` rather than `-1.bashrc`. A
/// name with several dots keeps every dot but the last in its stem — `archive.tar.gz` becomes
/// `archive.tar-1.gz` — because the last dot is the only one anything treats as the extension.
pub fn free_candidate(name: &str, attempt: u32) -> String {
	let (stem, extension) = match name.rsplit_once('.') {
		Some((stem, extension)) if !stem.is_empty() => (stem, format!(".{extension}")),
		// A dot-file (`.bashrc`) or a name with no dot at all: the whole thing is the stem.
		_ => (name, String::new()),
	};
	format!("{stem}-{attempt}{extension}")
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Re-reading for another account (§46) keeps WHERE the user is — the open folders, the selection
	/// — and drops WHAT was in them, because that was the other account's view of it.
	#[test]
	fn rereading_keeps_the_open_shape_and_drops_every_listed_child() {
		let mut explorer = tree(&["etc", "home"]);
		let _fetch = explorer.expand("/etc", false);
		explorer.listed("/etc", vec!["ssl".to_owned()]);
		explorer.select("/etc");
		// A folder that was listed but is CLOSED: nothing of it is on screen, so it is not re-fetched.
		let _fetch = explorer.expand("/home", false);
		explorer.listed("/home", vec!["cme".to_owned()]);
		explorer.collapse("/home");

		let needed = explorer.reread();

		assert!(needed.contains(&ROOT.to_owned()), "the root is open");
		assert!(needed.contains(&"/etc".to_owned()), "and so is /etc");
		assert!(
			!needed.contains(&"/home".to_owned()),
			"a closed folder shows nothing, so it waits until it is opened"
		);
		// Not one child name survives — the rows stand empty until the new account's listing lands.
		assert!(explorer.rows().iter().all(|row| row.path == ROOT));
		assert_eq!(
			explorer.selected(),
			Some("/etc"),
			"where the user was working is kept: it is usually why they elevated"
		);
	}

	/// A tree with `/` listed and open, holding the given child folders.
	fn tree(children: &[&str]) -> Explorer {
		let mut explorer = Explorer::default();
		explorer.expand(ROOT, false);
		explorer.listed(
			ROOT,
			children.iter().map(|name| (*name).to_owned()).collect(),
		);
		explorer
	}

	#[test]
	fn rows_walk_only_the_open_branches() {
		let mut explorer = tree(&["home", "etc"]);
		// Closed: only the root's own row plus its two children (the root is open).
		let names: Vec<String> = explorer.rows().into_iter().map(|row| row.name).collect();
		assert_eq!(names, vec!["/", "etc", "home"]);

		// Opening /home shows what it holds, indented one level deeper.
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		let rows = explorer.rows();
		let deepest = rows.last().expect("the tree has rows");
		assert_eq!((deepest.name.as_str(), deepest.depth), ("user", 2));
		assert_eq!(deepest.path, "/home/user");
	}

	#[test]
	fn hidden_folders_are_filtered_by_the_toggle_not_the_listing() {
		let mut explorer = tree(&["etc", ".ssh"]);
		// Shown by default — on a server the dot-folders are usually the point.
		assert!(explorer.rows().iter().any(|row| row.name == ".ssh"));

		explorer.toggle_hidden();
		assert!(!explorer.rows().iter().any(|row| row.name == ".ssh"));
		// The listing itself is untouched, so flipping back needs no re-fetch.
		explorer.toggle_hidden();
		assert!(explorer.rows().iter().any(|row| row.name == ".ssh"));
	}

	#[test]
	fn the_self_and_parent_links_never_become_rows() {
		// A server that reports `.` and `..` (SFTP omits them, `ls -A` leaves them out —
		// neither is guaranteed) must not grow two rows that walk back up the tree.
		let explorer = tree(&[".", "..", ".ssh", "etc"]);
		let names: Vec<String> = explorer.rows().into_iter().map(|row| row.name).collect();
		assert_eq!(names, vec!["/", ".ssh", "etc"]);
	}

	#[test]
	fn the_arrow_keys_walk_the_visible_rows_only() {
		let mut explorer = tree(&["etc", "home"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		// Rows: / , etc , home , home/user
		let selected = |explorer: &Explorer| explorer.selected().unwrap_or("none").to_owned();

		explorer.step(1);
		assert_eq!(
			selected(&explorer),
			ROOT,
			"nothing selected starts at the root"
		);
		explorer.step(2);
		assert_eq!(selected(&explorer), "/home");
		explorer.step(1);
		assert_eq!(selected(&explorer), "/home/user");
		// The last row is the end of the road, however hard the key is held.
		explorer.step(9);
		assert_eq!(selected(&explorer), "/home/user");
		assert_eq!(explorer.selected_index(), Some(3));

		// A collapsed branch is not somewhere the keyboard can be: the row is gone, so
		// the selection has no index and a step restarts from the top.
		explorer.collapse("/home");
		assert_eq!(explorer.selected_index(), None);
		explorer.step(1);
		assert_eq!(selected(&explorer), ROOT);
	}

	#[test]
	fn collapsing_closes_the_whole_subtree() {
		let mut explorer = tree(&["home"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		explorer.expand("/home/user", false);
		explorer.listed("/home/user", vec!["src".to_owned()]);
		assert_eq!(explorer.rows().len(), 4); // / + home + user + src

		explorer.collapse("/home");
		assert_eq!(explorer.rows().len(), 2); // / + home

		// Re-opening shows exactly one level again — the descendants stayed closed.
		explorer.expand("/home", false);
		assert_eq!(explorer.rows().len(), 3); // / + home + user
	}

	#[test]
	fn reopening_a_folder_re_lists_it_so_a_shell_side_move_shows() {
		// The reported bug: open a folder, move a child out of it from the shell, collapse the
		// folder by clicking, click it open again — and the moved-away child is still there. A
		// re-open is a deliberate act, so it must re-list, not trust the cache.
		let mut explorer = tree(&["cbl1m"]);
		assert_eq!(
			explorer.toggle_node("/cbl1m"),
			Some("/cbl1m".to_owned()),
			"opening a never-listed folder fetches it"
		);
		explorer.listed("/cbl1m", vec!["custom_deployment".to_owned()]);
		assert!(
			explorer
				.rows()
				.iter()
				.any(|row| row.name == "custom_deployment")
		);

		// Click to collapse, then click to re-open: the re-open re-lists even though /cbl1m is cached.
		assert_eq!(
			explorer.toggle_node("/cbl1m"),
			None,
			"the open row collapses, no fetch"
		);
		assert_eq!(
			explorer.toggle_node("/cbl1m"),
			Some("/cbl1m".to_owned()),
			"re-opening re-lists, so a folder moved out from the shell is caught"
		);
		// The fresh listing (custom_deployment was moved into _archives) replaces the stale child.
		explorer.listed("/cbl1m", vec!["_archives".to_owned()]);
		let names: Vec<String> = explorer.rows().into_iter().map(|row| row.name).collect();
		assert_eq!(names, vec!["/", "cbl1m", "_archives"]);
	}

	#[test]
	fn collapse_all_returns_to_the_top_level_but_keeps_the_root() {
		let mut explorer = tree(&["home", "etc"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		explorer.expand("/home/user", false);
		explorer.listed("/home/user", vec!["src".to_owned()]);
		assert_eq!(explorer.rows().len(), 5); // / + home + user + src + etc

		explorer.collapse_all();
		// The root stays open, so its own children still show; everything below them is closed.
		let names: Vec<String> = explorer.rows().into_iter().map(|row| row.name).collect();
		assert_eq!(names, vec!["/", "etc", "home"]);

		// Re-opening /home re-lists it (opening is deliberate, so it catches shell-side changes)…
		assert_eq!(explorer.expand("/home", false), Some("/home".to_owned()));
		// …but nothing was discarded: the cached level draws at once while that fetch is in flight.
		assert!(explorer.rows().iter().any(|row| row.path == "/home/user"));
	}

	#[test]
	fn revealing_a_directory_opens_its_whole_chain_once() {
		let mut explorer = Explorer::default();
		let needed = explorer.reveal_if_new("/home/user/src");
		assert_eq!(needed, vec!["/", "/home", "/home/user", "/home/user/src"]);
		assert_eq!(explorer.selected(), Some("/home/user/src"));

		// The shell re-announces the same directory at every prompt: no second fetch.
		assert!(explorer.reveal_if_new("/home/user/src").is_empty());

		// A native Windows directory does not belong on this POSIX tree.
		assert!(explorer.reveal_if_new("C:\\Users\\CLEm").is_empty());
	}

	#[test]
	fn a_rename_needs_a_real_new_name() {
		let mut explorer = tree(&["home"]);

		// Unchanged, blank, and separator-bearing names all cancel instead of renaming
		// (a `/` would move the folder, which is not what this edit offers).
		for text in ["home", "  ", "other/name"] {
			explorer.start_rename("/home".to_owned());
			explorer.edit_rename(text.to_owned());
			assert_eq!(explorer.commit_rename(), None, "{text} should not rename");
		}

		explorer.start_rename("/home".to_owned());
		explorer.edit_rename("  people  ".to_owned());
		assert_eq!(
			explorer.commit_rename(),
			Some(("/home".to_owned(), "/people".to_owned()))
		);

		// The root cannot be renamed — there is no parent to rename it within.
		explorer.start_rename(ROOT.to_owned());
		assert!(explorer.editing().is_none());
	}

	#[test]
	fn a_completed_rename_forgets_the_old_subtree_and_refreshes_the_parent() {
		let mut explorer = tree(&["home"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);

		assert_eq!(
			explorer.renamed("/home", "/people"),
			Some(ROOT.to_owned()),
			"the parent must be re-listed so the row reappears sorted"
		);
		assert_eq!(explorer.selected(), Some("/people"));
		// The stale branch is gone: nothing under the old name survives.
		assert!(!explorer.rows().iter().any(|row| row.path == "/home/user"));
	}

	#[test]
	fn refresh_re_lists_a_known_folder_without_opening_a_closed_one() {
		let mut explorer = tree(&["home"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		explorer.collapse("/home");

		// A known folder is re-fetched (a new child may have appeared)…
		assert_eq!(explorer.refresh_dir("/home"), Some("/home".to_owned()));
		// …but stays closed: the refresh records the change, it does not spring the branch open.
		assert!(!explorer.rows().iter().any(|row| row.path == "/home/user"));

		// A folder the tree has never listed has nothing to refresh, and neither has one already
		// loading (the fetch above is still in flight).
		assert_eq!(explorer.refresh_dir("/etc"), None);
		assert_eq!(explorer.refresh_dir("/home"), None);
	}

	#[test]
	fn refresh_open_re_lists_every_shown_branch_and_skips_the_rest() {
		let mut explorer = tree(&["home", "etc"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		explorer.expand("/etc", false);
		explorer.listed("/etc", Vec::new());
		// /home is now LISTED but CLOSED — its rows have left the screen.
		explorer.collapse("/home");

		// The whole-tree refresh re-lists exactly the open, listed branches — the root and /etc —
		// and skips /home (closed, so nothing under it is on screen to go stale). Sorted because
		// `BTreeMap` iterates in key order.
		let mut needed = explorer.refresh_open();
		needed.sort();
		assert_eq!(needed, vec!["/".to_owned(), "/etc".to_owned()]);

		// Every returned folder is now loading, so a second refresh before those fetches land asks
		// for nothing — no piling up duplicate listings.
		assert!(explorer.refresh_open().is_empty());
	}

	#[test]
	fn forgetting_drops_the_subtree_and_any_selection_in_it() {
		let mut explorer = tree(&["home", "etc"]);
		explorer.expand("/home", false);
		explorer.listed("/home", vec!["user".to_owned()]);
		explorer.select("/home/user");

		explorer.forget("/home");
		// The subtree BELOW the forgotten folder is gone, and the selection with it — nothing
		// points at a deleted row. The `/home` row itself lingers as a childless leaf until the
		// caller re-lists its parent (the parent's cached child list still names it), which is
		// exactly what `on_deleted` does next in the real flow.
		assert!(!explorer.rows().iter().any(|row| row.path == "/home/user"));
		assert_eq!(explorer.selected(), None);
		// A sibling outside the deleted subtree is untouched.
		assert!(explorer.rows().iter().any(|row| row.path == "/etc"));
	}

	#[test]
	fn a_plain_name_has_content_and_no_separator() {
		assert!(is_plain_name("notes"));
		assert!(is_plain_name("  spaced out  "));
		assert!(!is_plain_name(""));
		assert!(!is_plain_name("   "));
		assert!(!is_plain_name("a/b"));
	}

	#[test]
	fn relative_paths_walk_up_and_back_down() {
		assert_eq!(relative("/home/user", "/home/user/src"), "src");
		assert_eq!(relative("/home/user", "/home/user"), ".");
		assert_eq!(relative("/home/user", "/home"), "..");
		assert_eq!(relative("/home/user", "/var/log"), "../../var/log");
		assert_eq!(relative("/", "/etc"), "etc");
		assert_eq!(relative("/home/user/src", "/"), "../../..");
	}

	#[test]
	fn path_pieces_agree_on_the_root() {
		assert_eq!(name("/home/user"), "user");
		assert_eq!(name(ROOT), ROOT);
		assert_eq!(parent("/home/user"), Some("/home"));
		assert_eq!(parent("/home"), Some(ROOT));
		assert_eq!(parent(ROOT), None);
		assert_eq!(join(ROOT, "etc"), "/etc");
		assert_eq!(join("/etc", "ssh"), "/etc/ssh");
		assert_eq!(ancestors("/etc/ssh"), vec!["/", "/etc", "/etc/ssh"]);
	}

	#[test]
	fn a_quoted_path_cannot_break_out_of_its_quotes() {
		// The string below is typed into a live shell, so a folder name carrying a quote
		// must stay one argument — `'\''` closes, escapes, and reopens.
		assert_eq!(shell_quote("/tmp/plain"), "'/tmp/plain'");
		assert_eq!(shell_quote("/tmp/'; rm -rf ~"), r"'/tmp/'\''; rm -rf ~'");
	}

	#[test]
	fn the_menu_anchor_is_frozen_when_it_opens() {
		let mut explorer = tree(&["home"]);
		explorer.set_pointer(iced::Point::new(30.0, 60.0));
		explorer.open_menu("/home".to_owned());

		// The panel goes on reporting pointer moves while the menu is up. If the menu
		// followed them it would walk out from under the cursor on the way to an item.
		explorer.set_pointer(iced::Point::new(200.0, 400.0));
		let menu = explorer.menu().expect("the menu is open");
		assert_eq!(menu.path, "/home");
		assert_eq!(menu.at, iced::Point::new(30.0, 60.0));
	}

	#[test]
	fn a_keep_both_candidate_numbers_the_stem_and_keeps_the_extension() {
		// The number goes before the extension, so the copy still opens in the same program as
		// the original. This is the ordinary case, and the only one of the five old copies that
		// every one of them agreed on.
		assert_eq!(free_candidate("notes.txt", 1), "notes-1.txt");
		assert_eq!(free_candidate("notes.txt", 42), "notes-42.txt");
	}

	#[test]
	fn a_dot_file_is_all_name_and_has_no_extension_to_keep() {
		// `.bashrc` is not a file called nothing with an extension of `bashrc`. The guard on a
		// non-empty stem is what tells those two apart, and dropping it would produce `-1.bashrc`
		// — a different dot-file, in the same folder, that no longer sorts beside its original.
		assert_eq!(free_candidate(".bashrc", 1), ".bashrc-1");
		assert_eq!(free_candidate(".gitignore", 3), ".gitignore-3");
	}

	#[test]
	fn a_name_with_no_dot_is_numbered_on_the_end() {
		assert_eq!(free_candidate("README", 1), "README-1");
		assert_eq!(free_candidate("Makefile", 2), "Makefile-2");
	}

	#[test]
	fn only_the_last_dot_counts_as_the_extension() {
		// `archive.tar.gz` keeps `.tar` in its stem: the last dot is the only one anything reads
		// as an extension, so numbering before the `.tar` would rename the archive's type.
		assert_eq!(free_candidate("archive.tar.gz", 1), "archive.tar-1.gz");
		assert_eq!(free_candidate("v1.2.3.json", 1), "v1.2.3-1.json");
	}

	#[test]
	fn a_hidden_panel_takes_no_room_from_the_grid() {
		let mut explorer = Explorer::default();
		assert_eq!(explorer.reserved(), DEFAULT_WIDTH + SPLITTER_WIDTH);
		explorer.toggle();
		assert_eq!(explorer.reserved(), 0.0);

		// The splitter can never squeeze the panel below its minimum, nor past the cap
		// the caller derives from the window.
		explorer.set_width(10.0, 600.0);
		assert_eq!(explorer.width(), MIN_WIDTH);
		explorer.set_width(5_000.0, 600.0);
		assert_eq!(explorer.width(), 600.0);
	}
}

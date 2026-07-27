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
	/// Menu "Expand": open the folder and re-fetch its children — which doubles as
	/// the refresh for a directory changed from the shell.
	Expand(String),
	/// Menu "Collapse": close the folder and everything under it.
	Collapse(String),
	/// Menu "Open in terminal": send a `cd` for this folder to the shell.
	Cd(String),
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

	/// How much horizontal room the panel takes from the terminal grid: the tree plus
	/// its splitter, or nothing at all when hidden. `ui::terminal::grid_size` subtracts
	/// exactly this, so the reflow math and the layout can never drift.
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

	/// Open a folder, returning the path to list when its children are still needed.
	/// `force` re-fetches an already-listed folder — that is what the menu's Expand
	/// does, so it also serves as the refresh for a directory changed from the shell.
	pub fn expand(&mut self, path: &str, force: bool) -> Option<String> {
		let node = self.nodes.entry(path.to_owned()).or_default();
		node.open = true;
		if node.loading || (node.children.is_some() && !force) {
			return None;
		}
		node.loading = true;
		Some(path.to_owned())
	}

	/// Close a folder *and every folder under it*, so re-opening it shows one clean
	/// level again (§18). Collapsing is local state only — nothing is discarded, so
	/// re-expanding costs no round trip.
	pub fn collapse(&mut self, path: &str) {
		let prefix = format!("{}/", path.trim_end_matches('/'));
		for (key, node) in self.nodes.iter_mut() {
			if key == path || key.starts_with(&prefix) {
				node.open = false;
			}
		}
	}

	/// A row click: select the folder and flip it open or shut. Returns a path to list
	/// when opening it needs one.
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
		if self.revealed.as_deref() == Some(cwd) || !cwd.starts_with('/') {
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

#[cfg(test)]
mod tests {
	use super::*;

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

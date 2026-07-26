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

use iced::Point;

/// The pane's starting height, the shortest the splitter may drag it to, and the
/// grab bar's own height (§19). `ui::terminal` subtracts the pane plus the bar from
/// the grid, so these three are the single source of truth for that arithmetic.
pub const DEFAULT_HEIGHT: f32 = 220.0;
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

/// One directory entry. Names only — no size, no timestamps: the grid shows neither,
/// and asking for them is what makes a big directory slow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
	pub name: String,
	pub kind: Kind,
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

/// Everything the files pane can ask the app to do (§19). Nested under
/// `Message::Files` for the same reason the tree's messages are (§18): a dozen more
/// top-level variants would bury the rest of the enum.
#[derive(Debug, Clone)]
pub enum FilesMessage {
	/// Show or hide the whole pane (the status-bar button).
	Toggled,
	/// An entry was left-clicked: select it.
	EntryClicked(String),
	/// An entry was double-clicked: enter it, if it is a directory.
	EntryOpened(String),
	/// The toolbar's "up" button: enter the directory above the one on show. Carries no
	/// path — the pane's own is the only thing it can mean, and reading it when the press
	/// arrives is what keeps it from being a path the pane has since left.
	ParentOpened,
	/// An entry was right-clicked: select it and open the context menu on it.
	EntryRightClicked(String),
	/// The pointer moved over the pane; the payload is its pane-local position. Tracked
	/// because a right-press carries no coordinates of its own (§18).
	PointerMoved(Point),
	/// Dismiss the context menu without choosing an item.
	MenuDismissed,
	/// Re-list the directory on show — the refresh for a folder changed from the shell.
	Refresh,
	/// Menu "Copy name" / "Copy relative path" / "Copy full path".
	CopyName(String),
	CopyRelative(String),
	CopyPath(String),
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
	selected: Option<String>,
	menu: Option<Menu>,
	pointer: Point,
	rename: Option<Rename>,
	notice: Option<String>,
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
			selected: None,
			menu: None,
			pointer: Point::ORIGIN,
			rename: None,
			notice: None,
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

	/// The selected entry's full path, if any.
	pub fn selected(&self) -> Option<&str> {
		self.selected.as_deref()
	}

	/// The open context menu, if any.
	pub fn menu(&self) -> Option<&Menu> {
		self.menu.as_ref()
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

	/// Select an entry.
	pub fn select(&mut self, path: &str) {
		self.selected = Some(path.to_owned());
	}

	/// Open the context menu on an entry, anchored where the pointer is right now.
	pub fn open_menu(&mut self, path: String) {
		let kind = self.kind_of(&path).unwrap_or(Kind::File);
		self.menu = Some(Menu {
			path,
			kind,
			at: self.pointer,
		});
	}

	/// Close the context menu.
	pub fn close_menu(&mut self) {
		self.menu = None;
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
		self.selected = None;
		self.menu = None;
		self.rename = None;
		self.notice = None;
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
		self.selected = None;
		self.menu = None;
		self.rename = None;
		self.notice = None;
		self.dragging = false;
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
		self.entries
			.iter()
			.filter(|entry| show_hidden || !entry.name.starts_with('.'))
			.collect()
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

/// Which icon an entry gets (§19). Directories and symlinks go by kind; everything else
/// by its lower-cased extension, falling back to the neutral file icon.
pub fn category(entry: &Entry) -> Category {
	match entry.kind {
		Kind::Dir => return Category::Folder,
		Kind::Link => return Category::Link,
		Kind::File => {}
	}

	// A dot-file with no other dot (`.bashrc`) has no extension — it IS the name — so
	// `rsplit_once` on a name whose only dot is the leading one must not match.
	let Some((stem, extension)) = entry.name.rsplit_once('.') else {
		return Category::Plain;
	};
	if stem.is_empty() {
		return Category::Plain;
	}

	let extension = extension.to_lowercase();
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

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(name: &str, kind: Kind) -> Entry {
		Entry {
			name: name.to_owned(),
			kind,
		}
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
}

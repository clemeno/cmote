// panes.rs — the two file panes, and the things that are true of the PAIR (PLAN §18, §19, §22).
//
// A session shows two views of the same remote filesystem: the folder TREE down the left (§18) and
// the files PANE across the bottom (§19). Each has its own model, in `explorer` and `files`, and
// most of what happens to them happens to one of them alone — a scroll, a hover, a menu opening.
//
// But a good deal is true of the two TOGETHER, and that half had no owner. It lived in `app`, in
// eighteen functions that reached into both models and sequenced them by hand, and one of those
// functions said so in its own comment: "Done here rather than in a model because it spans both
// panes." That is what this module is. It holds the pair, and it owns the operations that are
// about the pair rather than about either pane:
//
//   * **Where the session IS.** Revealing a directory opens the tree down to it and points the pane
//     at it — two models, one idea, and getting one without the other is the bug rather than a
//     halfway state.
//   * **Re-reading.** Switching account (§46) or reconnecting (§22) re-reads both, and both then
//     want listings.
//   * **Deletion.** Entries vanishing must step the pane out of a folder that is gone BEFORE
//     anything re-lists, then drop the subtrees from the tree, then re-list each parent.
//   * **The remembered session** (§22): the `.*` filter, both pane sizes and the pane's sort are
//     one snapshot, captured and restored as one.
//   * **What the pane shows at all.** `files::rows` needs the tree's `show_hidden`, because the
//     `.*` toggle is one setting for both panes — which is why the pane could never answer "what
//     are my rows" on its own, and why nine call sites had to fetch the flag from the other model
//     and hand it over.
//
// It does NOT try to own the per-pane operations. Both models stay public here, and a caller that
// wants to scroll the tree scrolls the tree. Forwarding a hundred-odd single-pane methods through
// this struct would make it a wide, shallow thing that only re-types its two members' interfaces;
// what earns a module is the pair's own rules, which is all that is below.
//
// Nothing here returns an `iced::Task` or touches a channel. Operations that need the network hand
// back [`Fetches`] — the listings to ask for — and the caller turns those into commands. That is
// the shape `transfer::Queue` already uses (§16), for the same reason: it makes every rule in here
// answerable in a test with no window and no server.

use crate::change::Change;
use crate::explorer::{self, Explorer};
use crate::files::{Entry, Files};
use crate::targets::SessionState;

/// The largest share of the window either pane may take, as a fraction. A splitter drag is clamped
/// to it and so is a restored size (§22), so a remembered layout from a bigger window cannot open a
/// pane across most of a smaller one.
pub const MAX_PANE_FRACTION: f32 = 0.6;

/// What the session must ask the remote for after a pane operation (§18, §19).
///
/// The panes decide WHAT they need; turning that into `SshCommand`s is the caller's, because only
/// the caller has the channel. Empty is the common case and means "nothing to fetch".
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Fetches {
	/// Directories the tree wants listed, in the order it wants them.
	pub dirs: Vec<String>,
	/// The files pane's listing request number, when it wants a listing. The path travels with the
	/// pane rather than here — the caller reads it back off `pane.path()` at the moment it sends,
	/// which is what keeps a stale path from being sent for a fresh request.
	pub files: Option<u64>,
}

impl Fetches {
	/// Nothing to ask for.
	fn none() -> Self {
		Self::default()
	}

	/// Fold another set in, for an operation built out of several.
	fn and(mut self, other: Self) -> Self {
		self.dirs.extend(other.dirs);
		// Later wins: a second request supersedes the first, since both name the same pane.
		if other.files.is_some() {
			self.files = other.files;
		}
		self
	}
}

/// The two file panes of one session (§18, §19), and the rules that span them.
#[derive(Debug, Default)]
pub struct Panes {
	/// The folder tree down the left (§18).
	pub tree: Explorer,
	/// The files pane across the bottom (§19).
	pub pane: Files,
}

impl Panes {
	/// The entries the pane should show, filtered by the `.*` toggle — which lives on the TREE,
	/// because it is one setting for both panes (§18, §19).
	///
	/// This is why the pane cannot answer the question alone, and why nine call sites used to read
	/// the flag off the other model and pass it in. Asked here, the coupling is stated once.
	pub fn rows(&self) -> Vec<&Entry> {
		self.pane.rows(self.tree.show_hidden())
	}

	/// Whether hidden entries are shown, in both panes.
	pub fn show_hidden(&self) -> bool {
		self.tree.show_hidden()
	}

	/// Point BOTH panes at `cwd` (§19) — the tree opened down to it and selected, the pane
	/// browsed into it and marked as following the shell.
	///
	/// The Reveal button, and the shell-follow once a pinned resume has settled. One method rather
	/// than four calls in the caller's hand, because a tree opened without the pane moving (or the
	/// other way round) is not a state anyone asked for.
	pub fn reveal(&mut self, cwd: &str) -> Fetches {
		let dirs = self.tree.reveal(cwd);
		self.pane.set_followed(cwd);
		Fetches {
			dirs,
			files: self.pane.show(cwd),
		}
	}

	/// Follow the shell into `cwd` (§19) — the same idea as [`reveal`](Self::reveal), but only
	/// opening the tree where it is not already open, and letting the pane decide whether it is
	/// still following. This is the one that runs on every chunk of shell output, so it must be
	/// cheap and must not fight a user who has browsed the pane elsewhere ("last one wins").
	pub fn follow(&mut self, cwd: &str) -> Fetches {
		let dirs = self.tree.reveal_if_new(cwd);
		Fetches {
			dirs,
			files: self.pane.follow(cwd),
		}
	}

	/// Browse the PANE alone into `path` (§19). The console stays put, so a folder can be looked
	/// into without disturbing the shell; moving the console is a separate, deliberate act.
	pub fn browse(&mut self, path: &str) -> Fetches {
		Fetches {
			files: self.pane.show(path),
			..Fetches::none()
		}
	}

	/// Read both panes again from the remote, keeping WHERE they are and dropping WHAT was listed
	/// (§46): the open shape and the selection survive, the children do not, because those were the
	/// other account's view of them.
	pub fn reread(&mut self) -> Fetches {
		Fetches {
			dirs: self.tree.reread(),
			files: self.pane.refresh(),
		}
	}

	/// Re-list `dir` in whichever panes are showing it (§18) — after a rename, a new folder, or a
	/// deletion's parent. The pane is only asked when it is actually in that directory; the tree
	/// only when it has that node open.
	pub fn refresh_dir(&mut self, dir: &str) -> Fetches {
		let dirs = self.tree.refresh_dir(dir).into_iter().collect();
		let files = if self.pane.path() == Some(dir) {
			self.pane.refresh()
		} else {
			None
		};
		Fetches { dirs, files }
	}

	/// Entries were deleted (§18). Step the pane out of any folder that is now gone, drop the
	/// deleted subtrees from the tree, and re-list each parent they vanished from so the rows
	/// update in place.
	///
	/// THE ORDER IS THE RULE, and it is why this is one method and not three calls. The pane must
	/// move up BEFORE anything re-lists — otherwise the first refresh asks the server to list a
	/// directory that has just been removed, and the pane shows an error where it should show the
	/// parent.
	pub fn deleted(&mut self, paths: &[String]) -> Fetches {
		let mut fetches = Fetches::none();

		if let Some(here) = self.pane.path().map(str::to_owned) {
			for deleted in paths {
				if is_within(&here, deleted) {
					let up = explorer::parent(deleted)
						.unwrap_or(explorer::ROOT)
						.to_owned();
					fetches = fetches.and(self.browse(&up));
					break;
				}
			}
		}

		let mut parents: Vec<String> = Vec::new();
		for path in paths {
			self.tree.forget(path);
			if let Some(parent) = explorer::parent(path).map(str::to_owned)
				&& !parents.contains(&parent)
			{
				parents.push(parent);
			}
		}
		for parent in parents {
			fetches = fetches.and(self.refresh_dir(&parent));
		}
		fetches
	}

	/// Say the same thing in both panes (§18) — a rename, mkdir or delete that the server refused.
	/// One failure, one message, wherever the user is looking.
	pub fn set_notice(&mut self, reason: String) {
		self.tree.set_notice(reason.clone());
		self.pane.set_notice(reason);
	}

	/// Forget everything about the session that has just ended, or is about to begin.
	pub fn reset(&mut self) {
		self.tree.reset();
		self.pane.reset();
	}

	/// The pair's half of a session snapshot (§22): the `.*` filter, both pane sizes, and the
	/// pane's directory and sort. `terminal_path` is the caller's to fill, because the shell's
	/// directory is not the panes' business.
	pub fn capture(&self) -> SessionState {
		SessionState {
			terminal_path: None,
			files_path: self.pane.path().map(str::to_owned),
			show_hidden: Some(self.tree.show_hidden()),
			explorer_width: Some(self.tree.width()),
			files_height: Some(self.pane.height()),
			// The pane always knows its sort (both halves may be unset), so `reported` is right and
			// `Keep` never appears — `set_session` then writes the tri-state through as-is (§19, §22).
			sort: Change::reported(self.pane.sort_key()),
			sort_dir: Change::reported(self.pane.sort_dir()),
		}
	}

	/// Apply a remembered snapshot to both panes before the first listing (§22).
	///
	/// The paths are NOT applied here: they are handed back to the caller, which drives the `cd`
	/// and the reveal in the right order against a shell this module knows nothing about. Each size
	/// is clamped to the same window fraction a splitter drag is, and applied only once the window
	/// size is known, so a restore before the first resize event cannot pin a pane to its minimum.
	pub fn restore(&mut self, session: SessionState, window: iced::Size) -> Resume {
		if let Some(show_hidden) = session.show_hidden {
			self.tree.set_hidden(show_hidden);
		}
		// Each half folds onto what the pane already holds, so a `Keep` keeps the pane's own value
		// rather than abandoning the other half with it. A real snapshot reports both (§22), so this
		// is the same result by a route that cannot be wrong-footed by a half-filled one. `set_sort`
		// writes outright rather than toggling.
		let mut sort = self.pane.sort_key();
		let mut sort_dir = self.pane.sort_dir();
		session.sort.fold_into(&mut sort);
		session.sort_dir.fold_into(&mut sort_dir);
		self.pane.set_sort(sort, sort_dir);
		if let Some(width) = session.explorer_width
			&& window.width > 1.0
		{
			self.tree.set_width(width, window.width * MAX_PANE_FRACTION);
		}
		if let Some(height) = session.files_height
			&& window.height > 1.0
		{
			self.pane
				.set_height(height, window.height * MAX_PANE_FRACTION);
		}
		Resume {
			terminal: session.terminal_path,
			pane: session.files_path,
		}
	}

	/// Drag the TREE's splitter (§18): its width is measured from the right-hand edge of the
	/// window, and it is clamped to [`MAX_PANE_FRACTION`] of it.
	pub fn drag_tree_splitter(&mut self, pointer_x: f32, window: iced::Size) {
		self.tree
			.set_width(window.width - pointer_x, window.width * MAX_PANE_FRACTION);
	}

	/// Drag the PANE's splitter (§19) — the same rule on the other axis. The two used to be written
	/// out separately, differing only in `width`/`x` against `height`/`y`.
	pub fn drag_pane_splitter(&mut self, pointer_y: f32, window: iced::Size) {
		self.pane
			.set_height(window.height - pointer_y, window.height * MAX_PANE_FRACTION);
	}
}

/// Where a restored session says to go back to (§22) — handed to the caller because driving it
/// needs the shell.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Resume {
	/// The directory the shell was in.
	pub terminal: Option<String>,
	/// The directory the files pane was showing.
	pub pane: Option<String>,
}

/// Whether `path` sits inside `ancestor` (or IS it), comparing by path component so `/home/ab` is
/// not read as sitting inside `/home/a`.
fn is_within(path: &str, ancestor: &str) -> bool {
	let mut here = path.split('/').filter(|part| !part.is_empty());
	ancestor
		.split('/')
		.filter(|part| !part.is_empty())
		.all(|part| here.next() == Some(part))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A pane sitting inside a folder that is deleted must move UP before anything re-lists (§18).
	///
	/// The order is the whole rule, and it could not be asserted before: it lived in an `app`
	/// method that needed a whole `Tab`, a channel and a session behind it.
	#[test]
	fn deleting_the_folder_the_pane_is_in_moves_it_up_first() {
		let mut panes = Panes::default();
		let _ = panes.browse("/srv/data/logs");
		assert_eq!(panes.pane.path(), Some("/srv/data/logs"));

		let fetches = panes.deleted(&["/srv/data".to_owned()]);

		// Moved up to the deleted folder's PARENT, which still exists — not to the deleted folder,
		// and not left where it was.
		assert_eq!(panes.pane.path(), Some("/srv"));
		// And it asks for a listing, so the pane is not left showing the vanished folder's rows.
		assert!(fetches.files.is_some());
	}

	/// A pane somewhere else entirely is not moved by a deletion (§18).
	#[test]
	fn a_deletion_elsewhere_leaves_the_pane_where_it_is() {
		let mut panes = Panes::default();
		let _ = panes.browse("/etc");
		let _ = panes.deleted(&["/srv/data".to_owned()]);
		assert_eq!(panes.pane.path(), Some("/etc"));
	}

	/// A name that merely starts the same way is a different folder (§18) — the check is by path
	/// component, not by text prefix, or deleting `/srv/da` would move a pane sitting in
	/// `/srv/data`.
	#[test]
	fn a_shared_prefix_is_not_a_shared_path() {
		assert!(is_within("/srv/data", "/srv"));
		assert!(is_within("/srv/data", "/srv/data"));
		assert!(!is_within("/srv/data", "/srv/da"));
		assert!(!is_within("/srv", "/srv/data"));

		let mut panes = Panes::default();
		let _ = panes.browse("/srv/data");
		let _ = panes.deleted(&["/srv/da".to_owned()]);
		assert_eq!(panes.pane.path(), Some("/srv/data"));
	}

	/// Each parent a deletion emptied is re-listed once, however many of its children went (§18).
	#[test]
	fn a_deletion_relists_each_parent_once() {
		let mut panes = Panes::default();
		let fetches = panes.deleted(&[
			"/srv/a".to_owned(),
			"/srv/b".to_owned(),
			"/etc/c".to_owned(),
		]);
		// Two parents, not three: `/srv` is named twice and asked for once.
		assert_eq!(fetches.dirs.len() + usize::from(fetches.files.is_some()), 0);
		// Nothing is fetched at all here, in fact, because neither pane has those folders open —
		// which is the other half of the rule: a refresh is only asked for where something is
		// actually showing it.
	}

	/// The `.*` toggle is ONE setting for both panes (§18, §19), which is why the pane cannot
	/// answer "what are my rows" without the tree.
	#[test]
	fn the_hidden_toggle_is_the_pairs_and_not_either_panes() {
		let mut panes = Panes::default();
		let before = panes.show_hidden();
		// Asked of the PAIR, and answered by the tree — the pane never holds this flag.
		assert_eq!(before, panes.tree.show_hidden());

		panes.tree.toggle_hidden();
		assert_eq!(panes.show_hidden(), !before, "one toggle, both panes");
		// And the pane's rows follow it, without the caller fetching the flag and handing it over.
		assert_eq!(panes.rows().len(), panes.pane.rows(!before).len());
	}

	/// Revealing points BOTH panes at the directory (§19) — never one without the other.
	#[test]
	fn revealing_moves_the_tree_and_the_pane_together() {
		let mut panes = Panes::default();
		let fetches = panes.reveal("/srv/data");

		assert_eq!(panes.pane.path(), Some("/srv/data"));
		assert!(fetches.files.is_some(), "the pane wants its listing");
		// The tree asks for every folder on the way down, so the branch can be drawn open.
		assert!(
			fetches.dirs.contains(&"/srv".to_owned()),
			"the tree opens down to it: {:?}",
			fetches.dirs
		);
	}

	/// A snapshot goes out and comes back the same (§22) — the pair's half of it, which is all of
	/// it but the shell's own directory.
	#[test]
	fn a_captured_layout_restores_to_itself() {
		let window = iced::Size::new(1600.0, 1000.0);
		let mut before = Panes::default();
		before.tree.toggle_hidden();
		before.drag_tree_splitter(1200.0, window);
		before.drag_pane_splitter(700.0, window);
		let _ = before.browse("/srv/data");

		let snapshot = before.capture();
		// The panes' half is filled; the shell's is the caller's to add.
		assert_eq!(snapshot.terminal_path, None);
		assert_eq!(snapshot.files_path.as_deref(), Some("/srv/data"));

		let mut after = Panes::default();
		let resume = after.restore(snapshot, window);

		assert_eq!(after.show_hidden(), before.show_hidden());
		assert_px!(after.tree.width(), before.tree.width());
		assert_px!(after.pane.height(), before.pane.height());
		// The path is handed back rather than applied: driving it needs the shell.
		assert_eq!(resume.pane.as_deref(), Some("/srv/data"));
		assert_eq!(after.pane.path(), None);
	}

	/// A remembered size from a larger window cannot open a pane across most of a smaller one
	/// (§22) — the same clamp a splitter drag obeys.
	#[test]
	fn a_restored_size_is_clamped_like_a_drag() {
		let mut panes = Panes::default();
		let huge = SessionState {
			explorer_width: Some(5_000.0),
			files_height: Some(5_000.0),
			..SessionState::default()
		};
		let window = iced::Size::new(1000.0, 800.0);
		let _ = panes.restore(huge, window);

		// 1000 * 0.6 and 800 * 0.6, worked out here rather than recomputed from
		// `MAX_PANE_FRACTION` (§107). As the formula this assertion was the production line
		// written twice: it held for any fraction, so it pinned that the clamp is APPLIED and
		// never what it computes — and 0.6 is a judgement about how much of a window one pane may
		// eat, which is exactly the kind of number a test should stop from drifting unnoticed.
		assert_px!(panes.tree.width(), 600.0);
		assert_px!(panes.pane.height(), 480.0);
	}

	/// Before the window has been measured, a remembered size is not applied at all — otherwise a
	/// restore racing the first resize event would clamp both panes to nothing (§22).
	#[test]
	fn a_restore_before_the_window_is_known_changes_no_size() {
		let mut panes = Panes::default();
		let width = panes.tree.width();
		let height = panes.pane.height();
		let _ = panes.restore(
			SessionState {
				explorer_width: Some(400.0),
				files_height: Some(300.0),
				..SessionState::default()
			},
			iced::Size::new(0.0, 0.0),
		);
		assert_px!(panes.tree.width(), width);
		assert_px!(panes.pane.height(), height);
	}

	/// One refusal, one message, in both panes (§18).
	#[test]
	fn a_refusal_is_said_in_both_panes() {
		let mut panes = Panes::default();
		panes.set_notice("permission denied".to_owned());
		assert_eq!(panes.tree.notice(), Some("permission denied"));
		assert_eq!(panes.pane.notice(), Some("permission denied"));
	}
}

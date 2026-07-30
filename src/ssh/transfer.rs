// ssh/transfer.rs — the shared spine of a recursive folder transfer (PLAN §17, §19).
//
// A tree upload (`upload::transfer_tree`) and a tree download (`download::fetch_tree`) are
// mirror images: walk one side's directory tree, recreate it on the other, and copy each
// file across. Two things are the SAME whichever way the bytes flow, so they live here
// rather than being written twice:
//
//   * the SHAPE of a walked tree (`TreePlan`) — the directories to create (parents first)
//     and the files to copy (with their sizes, so the progress bar has a real total); and
//   * the CONFLICT protocol — when a file's destination is already taken, the transfer
//     parks, asks the GUI (`SshEvent::TransferConflict`), and waits for the answer
//     (`SshCommand::ResolveConflict`, delivered here as a `ConflictChoice`). A sticky
//     "…all" answer is remembered so the rest of the tree is settled without asking again.
//
// The directory-walking itself is NOT here: one side reads the local filesystem
// (`tokio::fs`) and the other the remote one (SFTP), so each owns its own walk. This
// module only defines the plan they both fill in and the decision they both make per file.

use std::path::{Path, PathBuf};

use tokio::sync::mpsc;

use crate::bridge::{ConflictChoice, SshEvent};
use crate::explorer;

/// A walked directory tree, ready to recreate on the other side (§17, §19). Paths are kept as
/// their component lists relative to the tree's root — not joined strings — so each side can
/// rebuild them with its own separator (`/` on the remote, the OS's locally) without re-parsing.
#[derive(Debug, Default)]
pub(crate) struct TreePlan {
	/// Every directory in the tree, PARENTS BEFORE CHILDREN, so creating them in order never
	/// asks for a folder whose parent does not exist yet. The root itself is the empty path.
	pub dirs: Vec<Vec<String>>,
	/// Every regular file, with the size the progress bar totals over.
	pub files: Vec<(Vec<String>, u64)>,
	/// Symbolic links found and left out (§17): following one risks a cycle and copying the link
	/// itself is not what SFTP's byte copy does, so a recursive transfer skips them and says how
	/// many in its closing notice rather than failing the whole tree over one.
	pub skipped_links: usize,
}

impl TreePlan {
	/// The total bytes to copy — what `SshEvent::TransferProgress` reports progress against.
	pub fn total(&self) -> u64 {
		self.files.iter().map(|(_, size)| size).sum()
	}
}

/// A collision answer that outlives the one file it was given for (§17): "overwrite everything"
/// or "skip everything" from here on. Remembered by the transfer so a `*All` answer settles the
/// rest of the tree with no further prompts.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Sticky {
	Overwrite,
	Skip,
}

/// What to do with ONE colliding file, once every "…all" sticky policy has been resolved down to
/// a plain per-file action. This is what the transfer acts on: write over the original, write a
/// free copy beside it, leave it alone, or stop the whole transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileAction {
	Overwrite,
	KeepBoth,
	Skip,
	Cancel,
}

/// Decide what to do about a file whose destination `name` is already taken (§17, §19).
///
/// A sticky policy already in force answers without troubling the user; otherwise the transfer
/// parks: it sends one `TransferConflict` and awaits the reply on `answers`. A `*All` answer is
/// recorded in `sticky` before being applied to this file, so every later collision is settled
/// silently. A dropped channel — the GUI went away, or the session is tearing down — reads as
/// Cancel, which stops the walk cleanly rather than blocking forever.
pub(crate) async fn resolve(
	events: &mpsc::Sender<SshEvent>,
	answers: &mut mpsc::Receiver<ConflictChoice>,
	sticky: &mut Option<Sticky>,
	name: &str,
) -> FileAction {
	if let Some(policy) = sticky {
		return match policy {
			Sticky::Overwrite => FileAction::Overwrite,
			Sticky::Skip => FileAction::Skip,
		};
	}

	// Ask once, then wait. If the event cannot even be sent the GUI is gone, so cancel.
	if events
		.send(SshEvent::TransferConflict {
			name: name.to_owned(),
		})
		.await
		.is_err()
	{
		return FileAction::Cancel;
	}

	match answers.recv().await {
		Some(ConflictChoice::Overwrite) => FileAction::Overwrite,
		Some(ConflictChoice::KeepBoth) => FileAction::KeepBoth,
		Some(ConflictChoice::Skip) => FileAction::Skip,
		Some(ConflictChoice::OverwriteAll) => {
			*sticky = Some(Sticky::Overwrite);
			FileAction::Overwrite
		}
		Some(ConflictChoice::SkipAll) => {
			*sticky = Some(Sticky::Skip);
			FileAction::Skip
		}
		// An explicit cancel, or the channel closing under us, both end the transfer.
		Some(ConflictChoice::Cancel) | None => FileAction::Cancel,
	}
}

/// Build a remote path from the tree's destination root and a relative component list (§17,
/// §19), joining POSIX-style the way every remote path does. Shared so the two directions build
/// the same string from the same parts.
pub(crate) fn remote_join(root: &str, rel: &[String]) -> String {
	let mut path = root.to_owned();
	for component in rel {
		path = explorer::join(&path, component);
	}
	path
}

/// The same for a local path, using THIS OS's own separator — a component list joined onto a
/// local root.
pub(crate) fn local_join(root: &Path, rel: &[String]) -> PathBuf {
	let mut path = root.to_path_buf();
	for component in rel {
		path.push(component);
	}
	path
}

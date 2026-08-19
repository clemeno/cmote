//! The one transfer slot, and everything queued behind it (§16, §17, §19, §21, §29).
//!
//! Moving bytes between this machine and the remote is ONE feature with many entrances: the
//! status bar's Files… / Upload pair, a right-click on the tree or the files pane, a save
//! dialog, a folder picker, a drag from Explorer dropped on the window. They all end in the
//! same place — a single transfer runs at a time, because the status bar has one progress bar
//! and two transfers would fight over it — and that rule used to be spelled out at each
//! entrance, slightly differently every time.
//!
//! `Queue` is where it is spelled out once. It owns the batch being set up, the three things
//! that can be waiting (files, whole folders, downloads), the slot itself, the question the
//! flow is currently holding, and the last outcome the bar shows. Every field is private: a
//! caller says what the user did and asks `busy()` before offering more work, and never has to
//! know that a folder queue drains after a file queue or that a resume point goes stale the
//! moment a fresh transfer starts.
//!
//! It reaches for nothing. No SSH channel, no dialog buffer, no panes — it returns [`Effects`]
//! saying what it needs done, and `Tab::apply` does it. That is what makes every rule in here
//! testable with no session, no window and no server.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use crate::bridge::{ConflictChoice, SshCommand};
use crate::explorer;

/// What every entrance says when the one transfer slot is already taken (§17). One string, so
/// the refusal reads the same whichever control the user reached for — only WHERE it is shown
/// differs, and that is the caller's business: an action started from the files pane says it in
/// the pane, one started from the status bar says it in the bar.
pub const BUSY_NOTICE: &str = "A transfer is already running.";

/// The body of the upload confirmation (§17); the picked file names are listed under it. The
/// destination below is a FOLDER — each file keeps its own name inside it — so one file or many
/// read the same, and the batch is confirmed once for the whole lot.
const UPLOAD_BODY: &str = "These files are sent over SFTP into the remote folder below. Edit the folder to send them somewhere else; leave it empty to use your login directory. Each file keeps its own name.";

/// The body of the upload batch's collision question (§17), followed by the names already in the
/// folder. Asked once for the whole batch after the server pre-scan, never per file — the mirror
/// of the download side (§21). Nothing has been sent when it appears, so every answer,
/// cancelling included, is safe to give.
const UPLOAD_CLASH_BODY: &str = "Some of these files are already in the destination folder. Skipping leaves those on the server as they are, keeping both adds a -1 to the name, and replacing overwrites them — replaced files are not recoverable. Nothing has been sent yet.";

/// The body of the multi-file download's collision question (§21), followed by the names that
/// clash. Nothing has been downloaded when it is asked, so every answer is safe to give —
/// including cancelling the batch outright.
const DOWNLOAD_CLASH_BODY: &str = "These files are already in the folder you picked. Skipping leaves the local copies alone, saving alongside adds a -1 to the name, and replacing overwrites them — replaced files are not recoverable. Nothing has been downloaded yet.";

/// The body of a recursive transfer's file-collision prompt (§17, §19), followed by the name of
/// the file already there. Asked one file at a time as the tree is walked: overwrite or skip just
/// this one, keep both (a -1 copy beside it), settle every later collision the same way at once,
/// or cancel the whole transfer — files already copied stay.
const CONFLICT_BODY: &str = "A file with this name is already at the destination. Choose what to do — replaced files are not recoverable. This applies as you go; \"all\" settles every remaining collision the same way.";

/// How far the transfer in the slot has got (§17, §19): the bytes written so far, out of the
/// file's size. A total of zero is a download that has not yet heard the size — or a zero-byte
/// file — so the bar shows only what has actually moved.
///
/// This says nothing about dialogs. The confirmation that used to be a state of the transfer is
/// a [`Question`] now: one is a thing on the wire, the other is a thing on the screen, and a
/// single type carrying both meant every reader had to say which it meant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferProgress {
	pub sent: u64,
	pub total: u64,
}

/// What to do about files a transfer would land on top of (§17, §21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClashChoice {
	/// Leave the copies already there alone and transfer only the rest.
	Skip,
	/// Overwrite them.
	Replace,
	/// Save alongside, as `name-1.ext`.
	KeepBoth,
	/// Transfer nothing at all.
	Cancel,
}

/// Which question the transfer flow is holding, for the view that draws it (§10). At most one is
/// ever open — each is raised by the single transfer slot, and nothing new starts while one waits
/// — so the view asks once and gets the whole answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Question {
	/// Where a picked batch is going, before anything is sent (§17). The only one with a field.
	Dest,
	/// Some of the batch's names are already in the destination folder (§17).
	UploadClash,
	/// Some of the download's names are already in the folder picked for it (§21).
	DownloadClash,
	/// A recursive transfer walked onto a file already at the destination (§17, §19).
	Conflict,
}

/// The question, with whatever answering it needs. The public shape is [`Question`]; this is the
/// private one, because the collisions a clash is holding are nobody else's business.
#[derive(Debug)]
enum Ask {
	/// The destination folder is being confirmed. The folder itself is `dest`, which the field
	/// edits in place, so this variant carries nothing.
	Dest,
	/// Each clashing name, paired with the free `name-1` path the server's pre-scan proposed for
	/// the "keep both" answer.
	UploadClash(Vec<(String, String)>),
	/// The batch that is waiting. The clashing names are NOT kept: the answer is applied by
	/// looking again, so a folder that changed while the question was open is still handled
	/// correctly.
	DownloadClash { remotes: Vec<String>, dir: PathBuf },
	/// The transfer is parked on the wire until the six-way answer goes back. The file's name is
	/// already in the dialog body, so nothing is kept here either.
	Conflict,
}

/// A transfer kept so it can be relaunched (§16) — as `in_flight` while it runs, or as
/// `resumable` after a failure parked it. It carries just enough to re-issue the exact command:
/// the direction (which decides upload vs download, single file vs whole tree) and its two
/// endpoints. A resume re-sends with `resume` set, so the task appends only the bytes still
/// missing rather than starting the file over.
///
/// It doubles as the memory of WHICH WAY the thing in the slot was going, which is how one
/// `ended` call serves an upload, a download and either kind of tree.
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

impl Resumable {
	/// What is being moved, by name, for the notice a LATER session shows about it (§16). The
	/// SOURCE's name in both directions: it is the name the user picked, while a destination may
	/// have been renamed to a `-1` copy on the way (§17, §21) and would read as something they
	/// never asked for.
	fn name(&self) -> &str {
		match self {
			Self::Upload { local, .. } | Self::UploadTree { local, .. } => file_name_of(local),
			Self::Download { remote, .. } | Self::DownloadTree { remote, .. } => {
				explorer::name(remote)
			}
		}
	}
}

/// A transfer a lost session took down with it (§16), kept for the NEXT session to offer to
/// finish.
///
/// Cancel and resume used to live entirely inside one connection: a session that dropped tore the
/// tab down to the error screen, and the resume point went with it — so the commonest reason a big
/// transfer stops, the link itself, was the one reason cmote could not offer to pick it up from.
/// The partial is still there, though: neither end deletes anything when a connection dies, so the
/// bytes that arrived are exactly as good as the ones a mid-flight failure leaves.
///
/// The endpoint rides along because a resume point means nothing away from the machine — and the
/// account — it was made on. Both its paths are that server's, and the partial a resume appends to
/// is on it. So this names where it belongs and the queue adopting it says where it now is; a tab
/// holding one can never hand it to another server by accident.
#[derive(Debug, Clone)]
pub struct Unfinished {
	/// The endpoint key (`user@host:port`) of the session it was running on.
	endpoint: String,
	/// What was moving, and between which two paths.
	what: Resumable,
}

/// How the transfer in the slot stopped (§16, §17, §21). Four ways, one call: which DIRECTION it
/// was going is not asked, because the queue already remembers it — so the six SSH events that
/// used to end a transfer are six one-line arms into here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
	/// It landed. The path is where it went — remote for an upload, local for a download.
	Done(String),
	/// It failed outright, leaving nothing to pick up. The rest of the batch still goes.
	Failed(String),
	/// It stopped but kept its partial (§16), so Resume can finish it.
	Interrupted(String),
	/// The destination appeared on the server AFTER the batch pre-scan settled the collisions
	/// (§17), so this file was skipped rather than reopening the question mid-batch.
	Skipped(String),
}

/// What a nudge to the queue asks the rest of the app to do (§17).
///
/// Returned rather than done, so the queue needs no reach into the SSH channel, the shared dialog
/// buffer or the panes — and so a test can read what a rule decided without wiring any of them
/// up. `Tab::apply` is the one place that carries these out.
#[derive(Debug, Default)]
pub struct Effects {
	/// Commands for the SSH worker, in the order they must go.
	pub commands: Vec<SshCommand>,
	/// Text for the shared dialog buffer (§10), when this nudge opened a question. The question
	/// itself is `asking`, which stays until it is answered.
	pub body: Option<String>,
	/// Put the keyboard in the destination field: the upload confirmation just opened (§17). The
	/// widget's id is the view's business, so it is not named here.
	pub focus_dest: bool,
	/// A remote directory to re-list: something just landed in it (§29).
	pub refresh: Option<String>,
}

impl Effects {
	/// One command and nothing else — what most starts amount to.
	fn send(command: SshCommand) -> Self {
		Self {
			commands: vec![command],
			..Self::default()
		}
	}

	/// A question opened, with the body to show under it.
	fn ask(body: String) -> Self {
		Self {
			body: Some(body),
			..Self::default()
		}
	}
}

/// The one transfer slot and everything queued behind it (§16, §17, §19, §21, §29).
///
/// Empty is idle: a fresh `Queue` has nothing picked, nothing queued, nothing running and no
/// question open, which is exactly what a tab starts with and what a session change returns it
/// to.
#[derive(Debug, Default)]
pub struct Queue {
	/// The local files picked for the current upload batch (§17), empty when none is pending —
	/// which is also what disables the status bar's Upload button. One file or many: the flow is
	/// the same, and the confirmation lists them.
	picked: Vec<PathBuf>,
	/// The destination FOLDER the batch goes into (§17), editable in the confirmation. Seeded
	/// from wherever the upload was started — the shell's cwd, the files pane's directory, or a
	/// folder right-clicked in the tree — and normalised to `.` (the login directory) when left
	/// empty. Each file keeps its own name inside it.
	dest: String,
	/// The batch waiting to send, a (local file, remote path) pair each (§17).
	files: VecDeque<(PathBuf, String)>,
	/// The FOLDERS waiting to go, tree-and-all, into `dest` (§29). A drop can carry both kinds at
	/// once and the two travel by different routes — a file joins `files` above, a folder is a
	/// whole recursive transfer of its own — so they queue separately and run one after another
	/// through the single slot.
	trees: VecDeque<PathBuf>,
	/// Downloads waiting their turn (§21) — remote path and where it is being saved.
	downloads: VecDeque<(String, PathBuf)>,
	/// How many of the batch have landed, for its closing notice (§17).
	files_done: usize,
	/// How many whole FOLDERS have landed, counted apart from the files so the closing notice can
	/// say what actually went (§29) — "3 files and 2 folders" rather than five of something.
	trees_done: usize,
	/// How many of the download batch have landed, for its closing notice (§21).
	downloads_done: usize,
	/// Whether the batch sends with overwrite set — true only when the user answered the collision
	/// question with "replace" (§17). Decided once, applied to every file; a free or "keep both"
	/// destination is written with it off and its own name.
	overwrite: bool,
	/// How far the transfer in the slot has got, `None` when nothing is on the wire. One state for
	/// both directions — only one transfer runs at a time, and an upload's progress bar and a
	/// download's read the same.
	slot: Option<TransferProgress>,
	/// The transfer running right now, remembered so a mid-flight failure can be resumed (§16),
	/// and so `ended` knows which way it was going. Set at every start — a queued file, a folder
	/// tree, or a resume itself — and cleared when it stops.
	in_flight: Option<Resumable>,
	/// A transfer that stopped on a failure and can be picked up where it left off (§16). Set from
	/// `in_flight` when the stop kept its partial — unlike a cancel — and cleared by a resume, a
	/// cancel, a fresh transfer, or a clean finish. `Some` is what draws the Resume button.
	resumable: Option<Resumable>,
	/// The question the flow is holding, `None` when it is holding none.
	ask: Option<Ask>,
	/// The last transfer outcome, shown in the status bar until the next one starts (§17, §19).
	/// `ponytail:` no timed fade — that would need a timer subscription for a line of text.
	notice: Option<String>,
	/// Whether a file from the OS is being dragged over the window right now (§29). Lights the
	/// files pane as the drop target while true. Purely a visual cue — the drop itself reads the
	/// pane's directory, not this flag.
	hovering: bool,
	/// The paths of a drop that has just landed, gathering (§29). The OS reports a multi-file drop
	/// as one event PER PATH, with nothing to say the last has arrived — so they are collected
	/// here and read once on the next frame, when the whole drop is known. Empty at rest, which is
	/// also what tells the subscription there is no settling to wait for.
	dropped: Vec<PathBuf>,
}

impl Queue {
	// ---- what the rest of the app asks -------------------------------------------------

	/// Whether anything at all is going on (§17): a batch picked, a queue loaded, a question
	/// open, or bytes on the wire. THE one-transfer-at-a-time rule, asked by every entrance
	/// before it offers the user more work — so a drop, a download and a folder upload all
	/// refuse on exactly the same condition rather than on three approximations of it.
	pub fn busy(&self) -> bool {
		self.slot_taken() || self.ask.is_some() || !self.picked.is_empty()
	}

	/// The same rule as `busy`, asked from INSIDE the upload flow, where the batch being set up
	/// is the thing asking and so cannot count against itself: is the slot taken, or is anything
	/// already queued for it?
	fn slot_taken(&self) -> bool {
		self.slot.is_some()
			|| !self.files.is_empty()
			|| !self.trees.is_empty()
			|| !self.downloads.is_empty()
	}

	/// Which question is open, `None` when none is (§10).
	pub fn asking(&self) -> Option<Question> {
		match self.ask {
			Some(Ask::Dest) => Some(Question::Dest),
			Some(Ask::UploadClash(_)) => Some(Question::UploadClash),
			Some(Ask::DownloadClash { .. }) => Some(Question::DownloadClash),
			Some(Ask::Conflict) => Some(Question::Conflict),
			None => None,
		}
	}

	/// How far the transfer on the wire has got, `None` when nothing is transferring (§17).
	pub fn progress(&self) -> Option<TransferProgress> {
		self.slot
	}

	/// The last outcome, for the status bar's centre zone (§17).
	pub fn notice(&self) -> Option<&str> {
		self.notice.as_deref()
	}

	/// Whether a failure left something to pick up (§16) — what draws the Resume button.
	pub fn can_resume(&self) -> bool {
		self.resumable.is_some()
	}

	/// How many local files are picked (§17): zero, and there is nothing to upload.
	pub fn picked_count(&self) -> usize {
		self.picked.len()
	}

	/// The first picked file's name, so the bar can label a lone pick by name rather than by a
	/// count of one (§17).
	pub fn first_picked(&self) -> Option<&str> {
		self.picked.first().map(|local| file_name_of(local))
	}

	/// The destination folder the confirmation is editing (§17).
	pub fn dest(&self) -> &str {
		&self.dest
	}

	/// Whether Upload has something to send and nothing in its way (§17) — the button's enable.
	pub fn can_send(&self) -> bool {
		!self.picked.is_empty() && self.slot.is_none() && self.ask.is_none()
	}

	/// Whether a file from the OS is being dragged over the window (§29).
	pub fn hovering(&self) -> bool {
		self.hovering
	}

	/// Whether a drop's paths are still gathering (§29) — what asks for the frame tick that
	/// reads them.
	pub fn settling(&self) -> bool {
		!self.dropped.is_empty()
	}

	/// Whether the keyboard belongs to this flow rather than to the shell (§17, §21). True while
	/// one of the modal questions is up, and while a transfer runs. The six-way conflict prompt is
	/// NOT among them: it is answered by its buttons alone, and a key pressed while it is up still
	/// reaches the shell, exactly as it did before this module existed.
	pub fn holds_keyboard(&self) -> bool {
		self.slot.is_some()
			|| matches!(
				self.ask,
				Some(Ask::Dest | Ask::UploadClash(_) | Ask::DownloadClash { .. })
			)
	}

	// ---- setting a batch up (§17) ------------------------------------------------------

	/// The file picker came back (§17). An empty pick is a cancelled picker, which keeps whatever
	/// was already chosen — the same rule the key-file picker on the connect form uses.
	pub fn pick(&mut self, files: Vec<PathBuf>) {
		if files.is_empty() {
			return;
		}
		self.picked = files;
		self.notice = None;
	}

	/// The picker came back from a right-click surface, where the folder is already known (§17):
	/// pick, and go straight to the confirmation.
	pub fn pick_into(&mut self, files: Vec<PathBuf>, dir: String) -> Effects {
		if files.is_empty() {
			return Effects::default();
		}
		self.pick(files);
		self.open_confirm(dir)
	}

	/// What the user is typing in the confirmation's destination field (§17).
	pub fn set_dest(&mut self, dir: String) {
		self.dest = dir;
	}

	/// Open the confirmation for the picked batch (§17): the files are listed in the body, `dir`
	/// goes in the editable field, and the field takes the keyboard so the folder can be corrected
	/// — or the batch confirmed with Enter — without reaching for the mouse. Nothing picked asks
	/// nothing; a slot already taken declines instead, since the bar has one progress bar.
	pub fn open_confirm(&mut self, dir: String) -> Effects {
		if self.picked.is_empty() {
			return Effects::default();
		}
		if self.slot_taken() {
			self.notice = Some(BUSY_NOTICE.to_owned());
			return Effects::default();
		}
		self.dest = dir;
		self.ask = Some(Ask::Dest);
		let names: Vec<&str> = self
			.picked
			.iter()
			.map(|local| file_name_of(local))
			.collect();
		Effects {
			body: Some(format!("{UPLOAD_BODY}\n\n{}", names.join("\n"))),
			focus_dest: true,
			..Effects::default()
		}
	}

	/// The destination was confirmed (§17): pre-scan the server for names already in it, so the
	/// "some are already there" question is asked once for the whole batch before a single byte is
	/// sent. An empty folder normalises to `.` — the login directory — so a shell that never
	/// announced its cwd still has somewhere to send to. The confirmation closes while the scan
	/// runs; the answer reopens as either the collision question or the transfer itself.
	pub fn send_batch(&mut self) -> Effects {
		if self.picked.is_empty() {
			self.cancel_batch();
			return Effects::default();
		}
		let dir = self.dest.trim();
		// A relative `.` resolves against the login directory server-side, and `join` keeps it in
		// front rather than turning a bare name into an absolute `/name`.
		self.dest = if dir.is_empty() {
			".".to_owned()
		} else {
			dir.to_owned()
		};
		let names: Vec<String> = self
			.picked
			.iter()
			.map(|local| file_name_of(local).to_owned())
			.collect();
		self.ask = None;
		self.notice = Some("Checking the destination…".to_owned());
		Effects::send(SshCommand::CheckUploads {
			dir: self.dest.clone(),
			names,
		})
	}

	/// Back out of the upload flow before or during a batch (§17): a cancelled confirmation or
	/// collision question, or Esc. Drops everything pending so nothing is sent; a transfer already
	/// in flight is left to finish, since its bytes are already on the wire.
	pub fn cancel_batch(&mut self) {
		if matches!(self.ask, Some(Ask::Dest | Ask::UploadClash(_))) {
			self.ask = None;
		}
		self.close_batch();
	}

	// ---- answers -----------------------------------------------------------------------

	/// A collision question was answered (§17, §21). WHICH question it was is not asked: only one
	/// can be open, so the queue reads it off what it is holding — an upload's answer builds the
	/// batch under that choice and starts it, a download's rebuilds the local names and starts
	/// those. `Cancel` drops the batch either way; nothing has been written when either question
	/// is up, so it costs nothing.
	pub fn answer_clash(&mut self, choice: ClashChoice) -> Effects {
		match self.ask.take() {
			Some(Ask::UploadClash(collisions)) => {
				if choice == ClashChoice::Cancel {
					self.cancel_batch();
					return Effects::default();
				}
				self.queue_batch(&collisions, choice)
			}
			Some(Ask::DownloadClash { remotes, dir }) => {
				if choice == ClashChoice::Cancel {
					return Effects::default();
				}
				self.queue_downloads(&remotes, &dir, choice)
			}
			// Some other question, or none: put back what was there and do nothing.
			other => {
				self.ask = other;
				Effects::default()
			}
		}
	}

	/// A recursive transfer's collision prompt was answered (§17, §19): close it and send the
	/// choice to the transfer parked on it, which resumes — or, on Cancel, winds down and reports
	/// back through the usual ending.
	pub fn answer_conflict(&mut self, choice: ConflictChoice) -> Effects {
		if matches!(self.ask, Some(Ask::Conflict)) {
			self.ask = None;
		}
		Effects::send(SshCommand::ResolveConflict(choice))
	}

	/// Back out of whatever is holding the keyboard (§17, §21) — Esc. The confirmation and the
	/// upload's collision question drop the whole batch; the download's just closes, which leaves
	/// nothing behind because nothing was queued yet; a running transfer has nothing to back out
	/// of, so the key is simply swallowed (the ✕ in the bar is how one is stopped).
	pub fn escape(&mut self) {
		match self.ask {
			Some(Ask::Dest | Ask::UploadClash(_)) => self.cancel_batch(),
			Some(Ask::DownloadClash { .. }) => self.ask = None,
			Some(Ask::Conflict) | None => {}
		}
	}

	// ---- downloads (§21) ---------------------------------------------------------------

	/// Fetch one remote file to the local path the save dialog picked (§19). A cancelled dialog
	/// (`None`) sends nothing.
	pub fn download(&mut self, remote: String, local: Option<PathBuf>) -> Effects {
		match local {
			Some(local) => self.start_download(remote, local),
			None => Effects::default(),
		}
	}

	/// The folder picker for a multi-file download closed (§21). Nothing is written yet: the local
	/// names already taken are looked up first, and if there are any the batch waits on the
	/// question about them.
	pub fn download_into(&mut self, remotes: Vec<String>, dir: Option<PathBuf>) -> Effects {
		let Some(dir) = dir else {
			return Effects::default();
		};
		let taken: Vec<String> = remotes
			.iter()
			.map(|remote| explorer::name(remote).to_owned())
			.filter(|name| dir.join(name).exists())
			.collect();
		if taken.is_empty() {
			// Nothing to lose: the choice cannot apply to anything, so any of them will do.
			return self.queue_downloads(&remotes, &dir, ClashChoice::Skip);
		}
		let body = format!("{DOWNLOAD_CLASH_BODY}\n\n{}", taken.join("\n"));
		self.ask = Some(Ask::DownloadClash { remotes, dir });
		Effects::ask(body)
	}

	/// Turn a picked folder and a batch of remote files into the download queue (§21), applying
	/// the answer to the "already there" question, then start it.
	fn queue_downloads(&mut self, remotes: &[String], dir: &Path, choice: ClashChoice) -> Effects {
		self.downloads.clear();
		self.downloads_done = 0;
		for remote in remotes {
			let name = explorer::name(remote);
			let local = dir.join(name);
			let local = match choice {
				_ if !local.exists() => local,
				ClashChoice::Replace => local,
				ClashChoice::KeepBoth => free_name(dir, name),
				// Cancel never gets this far — the answer drops the batch instead.
				ClashChoice::Skip | ClashChoice::Cancel => continue,
			};
			self.downloads.push_back((remote.clone(), local));
		}
		self.pump()
	}

	// ---- whole folders (§17, §19) ------------------------------------------------------

	/// Start a recursive folder upload the picker chose a source for (§17). A cancelled picker
	/// (`None`) sends nothing; a slot already taken declines. The bar starts at an unknown total
	/// the first progress event fills in.
	pub fn upload_tree(&mut self, local: Option<PathBuf>, dir: String) -> Effects {
		let Some(local) = local else {
			return Effects::default();
		};
		if self.slot_taken() {
			self.notice = Some(BUSY_NOTICE.to_owned());
			return Effects::default();
		}
		self.start_upload_tree(local, dir)
	}

	/// Start a recursive folder download the picker chose a destination for (§19). The mirror of
	/// `upload_tree` — the caller has already refused this while `busy`, because the refusal is
	/// shown in the files pane it was started from rather than in the status bar.
	pub fn download_tree(&mut self, remote: String, local: Option<PathBuf>) -> Effects {
		let Some(local) = local else {
			return Effects::default();
		};
		self.resumable = None;
		self.in_flight = Some(Resumable::DownloadTree {
			remote: remote.clone(),
			local: local.clone(),
		});
		self.notice = None;
		self.slot = Some(TransferProgress::default());
		Effects::send(SshCommand::DownloadTree {
			remote,
			local,
			resume: false,
		})
	}

	// ---- the slot ----------------------------------------------------------------------

	/// Stop the transfer running right now (§16) — the status bar's ✕. Empties every queue and
	/// forgets any resume point, since a deliberate cancel is final and takes the whole batch with
	/// it, then tells the worker to stop: its copy loop deletes the partial it was writing and
	/// reports the neutral "cancelled" outcome, which clears the bar and, the queues now empty,
	/// closes the batch out. The slot is left occupied until that outcome lands, so the bar does
	/// not flicker between the click and the worker winding down.
	pub fn cancel(&mut self) -> Effects {
		self.files.clear();
		// The folders queued behind this transfer go with it: a deliberate cancel takes the whole
		// drop, not just the item on the wire (§16, §29).
		self.trees.clear();
		self.downloads.clear();
		self.files_done = 0;
		self.trees_done = 0;
		self.downloads_done = 0;
		self.resumable = None;
		self.in_flight = None;
		Effects::send(SshCommand::CancelTransfer)
	}

	/// Pick up a transfer that a failure interrupted (§16) — the status bar's Resume. Relaunches
	/// the exact command `resumable` remembers with `resume` set, so the task sizes the
	/// destination and sends only the bytes still missing; a single file left in a batch drains
	/// the rest once it lands. Does nothing if there is nothing to resume.
	pub fn resume(&mut self) -> Effects {
		let Some(resumable) = self.resumable.take() else {
			return Effects::default();
		};
		// Mirror what a fresh start records, so this resumed transfer is itself resumable if it
		// too is interrupted (a flaky link may need more than one nudge).
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
		self.notice = None;
		self.slot = Some(TransferProgress::default());
		Effects::send(command)
	}

	// ---- OS drops (§29) ----------------------------------------------------------------

	/// A file from the OS is over the window, or has left it (§29). Only a live session can be a
	/// target, which is what the caller passes.
	pub fn hover(&mut self, over: bool) {
		self.hovering = over;
	}

	/// One dropped path arrived (§29). Nothing is decided here — the OS reports each path as its
	/// own event and never says which is the last, so they gather and the next frame reads the
	/// whole drop at once. The drag is over the moment a path lands, whatever is then decided
	/// about it, so the target highlight goes out here.
	pub fn caught(&mut self, path: PathBuf) {
		self.hovering = false;
		self.dropped.push(path);
	}

	/// The frame after a drop has come round, so the whole set of paths is in hand (§29). Send it
	/// into the files pane's current directory, reusing the upload pipeline whole — the
	/// destination pre-scan and, on a name already taken, the same question a menu upload opens
	/// (§17). The drop already said where the bytes go, so there is no destination confirmation.
	pub fn settle(&mut self, connected: bool, pane_dir: Option<&str>) -> Effects {
		let dropped = std::mem::take(&mut self.dropped);
		if dropped.is_empty() {
			return Effects::default();
		}
		// A folder needs the tree flow and a file the batch flow (§17), so the drop is sorted into
		// its two kinds here — both go, one after the other, through the single slot.
		let (folders, files): (Vec<PathBuf>, Vec<PathBuf>) =
			dropped.into_iter().partition(|path| path.is_dir());
		match drop_outcome(
			connected,
			self.busy(),
			folders.len() + files.len(),
			pane_dir,
		) {
			// No session (or not the terminal screen): nowhere to send, so say nothing.
			DropOutcome::Ignore => Effects::default(),
			DropOutcome::Busy => {
				self.notice = Some(BUSY_NOTICE.to_owned());
				Effects::default()
			}
			DropOutcome::NoDir => {
				self.notice = Some("Open a folder in the files pane first.".to_owned());
				Effects::default()
			}
			DropOutcome::Upload(dir) => {
				self.dest = dir;
				self.notice = None;
				// Every folder queues, each to go tree-and-all exactly as the menu's "Upload
				// folder…" does (§17) — the same command, the same per-file collision questions,
				// the same resume. The pump starts them once the files are through.
				self.trees = folders.into();
				if files.is_empty() {
					// Folders only: there is no batch to pre-scan, so the first tree starts here.
					return self.pump();
				}
				// Seed the batch and run the ordinary confirmed-upload path: it pre-scans the
				// destination, then either sends or opens the collision question (§17). One file
				// or twenty, this is the same flow the picker's own selection takes.
				self.picked = files;
				self.send_batch()
			}
		}
	}

	// ---- what the worker says ----------------------------------------------------------

	/// The batch pre-scan came back (§17). Nothing clashing → queue every file and start sending.
	/// Some clashing → hold the batch on the collision question, the names it found listed under
	/// it. A batch cancelled while the scan was in flight leaves nothing to do.
	pub fn prescan(&mut self, collisions: Vec<(String, String)>) -> Effects {
		self.notice = None;
		if self.picked.is_empty() {
			return Effects::default();
		}
		if collisions.is_empty() {
			// The choice is irrelevant when nothing collides — every file writes to its own free
			// name — so `Skip` (which touches only clashing names) does for all of them.
			return self.queue_batch(&[], ClashChoice::Skip);
		}
		let names: Vec<&str> = collisions.iter().map(|(name, _)| name.as_str()).collect();
		let body = format!("{UPLOAD_CLASH_BODY}\n\n{}", names.join("\n"));
		self.ask = Some(Ask::UploadClash(collisions));
		Effects::ask(body)
	}

	/// Bytes moved (§17). Only meaningful while a transfer is running; a late event after a
	/// failure must not revive the bar.
	pub fn progressed(&mut self, sent: u64, total: u64) {
		if self.slot.is_some() {
			self.slot = Some(TransferProgress { sent, total });
		}
	}

	/// A recursive transfer walked onto a file already at the destination (§17, §19): park it
	/// behind the six-way question, naming the file it is about.
	pub fn conflicted(&mut self, name: &str) -> Effects {
		self.ask = Some(Ask::Conflict);
		Effects::ask(format!("{CONFLICT_BODY}\n\n{name}"))
	}

	/// The transfer in the slot stopped (§16, §17, §21). ONE ending for all four ways it can, and
	/// for both directions: which way it was going is read off what was in flight, not asked for.
	/// Whatever is queued behind it starts here, which is what walks a batch to its end.
	pub fn ended(&mut self, how: Ended) -> Effects {
		self.slot = None;
		let was = self.in_flight.take();
		match how {
			Ended::Interrupted(message) => {
				// The partial was kept, unlike a cancel, so it can be resumed rather than lost.
				// The queue behind it is left in place: resuming the failed item drains the rest
				// afterwards.
				self.notice = Some(message);
				self.resumable = was;
				Effects::default()
			}
			Ended::Done(path) => {
				// Landed, so there is nothing to resume; the next item, if any, is remembered
				// afresh by the pump.
				self.resumable = None;
				self.landed(was.as_ref(), &path)
			}
			// One item failing does not abandon the rest of the batch — the notice says what went
			// wrong, and the queue moves on (§17, §21). A failure shows in the status bar rather
			// than on the error screen, which would tear the shell down for a file that never left.
			Ended::Failed(message) => {
				self.resumable = None;
				self.notice = Some(message);
				self.drain()
			}
			Ended::Skipped(path) => {
				self.resumable = None;
				self.notice = Some(format!(
					"Skipped {} — it appeared on the server",
					explorer::name(&path)
				));
				self.drain()
			}
		}
	}

	// ---- the session -------------------------------------------------------------------

	/// Forget everything: the session this queue belonged to has gone (§17, §21, §29), or the
	/// worker it was talking to did.
	///
	/// One call, so nothing survives into the NEXT session — not a Resume offer for a file on
	/// another server, not a folder queued for a shell that no longer exists, not a drop half
	/// gathered. That was a dozen hand-written clears before, and it missed six of the fields.
	pub fn reset(&mut self) {
		*self = Self::default();
	}

	/// The session has gone while something was still unfinished (§16): hand back the one thing a
	/// LATER session to the same server could pick up, then forget everything else exactly as
	/// `reset` does.
	///
	/// A dropped connection is not a cancel. A cancel deletes the partial it was writing, on
	/// purpose, and is final; a connection dying deletes nothing — the bytes that reached the far
	/// side are still there, still exactly as long as they got — so the one thing worth carrying
	/// out of a dead session is how far it got. Everything else belongs to the session that raised
	/// it: the queue behind the slot, the batch being set up, the question that was open.
	///
	/// What was ON THE WIRE outranks an older parked offer, because it is the one whose partial was
	/// growing a moment ago. A cancel has already cleared both, which is what keeps a deliberately
	/// cancelled transfer from reappearing on the next connection.
	pub fn abandon(&mut self, endpoint: &str) -> Option<Unfinished> {
		let what = self.in_flight.take().or_else(|| self.resumable.take());
		self.reset();
		what.map(|what| Unfinished {
			endpoint: endpoint.to_owned(),
			what,
		})
	}

	/// A fresh session opened: put back what the last one left unfinished (§16), if this is the
	/// same server it was left on. That is the whole of "resume across a dropped connection" —
	/// the resume itself already re-issues an absolute command and sizes the destination before it
	/// sends a byte, so it never cared WHICH connection carries it; only the memory of it was
	/// missing.
	///
	/// The notice comes with it, because a Resume button appearing on a freshly opened session
	/// would otherwise offer to finish something without saying what: the file is named, and so is
	/// the reason it stopped.
	///
	/// A different endpoint drops it silently, and the offer is spent either way — the caller
	/// `take`s it before calling, the same rule a carried directory follows (§52). A resume point
	/// that waited through a session on another machine is one nobody remembers making.
	pub fn adopt(&mut self, unfinished: Unfinished, endpoint: &str) {
		if unfinished.endpoint != endpoint {
			return;
		}
		self.notice = Some(format!(
			"{} stopped when the connection dropped",
			unfinished.what.name()
		));
		self.resumable = Some(unfinished.what);
	}

	// ---- the rules themselves ----------------------------------------------------------

	/// Turn the picked files, the destination and the collision answer into the upload queue
	/// (§17), then start it. The mapping is `plan_uploads` (pure, so it is tested on its own);
	/// this only records the batch-wide overwrite flag and pumps.
	fn queue_batch(&mut self, collisions: &[(String, String)], choice: ClashChoice) -> Effects {
		self.files = plan_uploads(&self.picked, &self.dest, collisions, choice).into();
		self.files_done = 0;
		self.overwrite = choice == ClashChoice::Replace;
		self.drain()
	}

	/// Start whatever is next and, if that left nothing running, close the batch out (§17). Every
	/// ending funnels through here: the file that just failed may have been the last one picked,
	/// and a batch that never starts anything (a Skip answer to an all-clashing batch) still has
	/// to stop disabling the Upload button.
	fn drain(&mut self) -> Effects {
		let effects = self.pump();
		self.close_if_drained();
		effects
	}

	/// Start the next queued transfer if the one slot is free (§17, §21, §29). THE only place that
	/// decides what runs next, which is what walks every queue there is: the batch's files first,
	/// then the folders behind them — the batch's collision question was answered up front (§17)
	/// while a tree asks its own as it walks — and then the downloads.
	fn pump(&mut self) -> Effects {
		if self.slot.is_some() {
			return Effects::default();
		}
		if let Some((local, remote)) = self.files.pop_front() {
			return self.start_upload(local, remote);
		}
		if let Some(local) = self.trees.pop_front() {
			let dir = self.dest.clone();
			return self.start_upload_tree(local, dir);
		}
		if let Some((remote, local)) = self.downloads.pop_front() {
			return self.start_download(remote, local);
		}
		Effects::default()
	}

	/// Put one file on the wire (§17). A fresh file starting means the previous transfer's resume
	/// offer, if any, is stale (§16); this file is remembered so its own failure can be resumed.
	fn start_upload(&mut self, local: PathBuf, remote: String) -> Effects {
		let total = std::fs::metadata(&local).map_or(0, |meta| meta.len());
		self.resumable = None;
		self.in_flight = Some(Resumable::Upload {
			local: local.clone(),
			remote: remote.clone(),
		});
		self.slot = Some(TransferProgress { sent: 0, total });
		Effects::send(SshCommand::Upload {
			local,
			remote,
			overwrite: self.overwrite,
			resume: false,
		})
	}

	/// Put one whole folder on the wire (§17, §29).
	fn start_upload_tree(&mut self, local: PathBuf, dir: String) -> Effects {
		self.resumable = None;
		self.in_flight = Some(Resumable::UploadTree {
			local: local.clone(),
			remote: dir.clone(),
		});
		self.notice = None;
		// Remembered so completion re-lists this folder if a pane is on it (§29) — the same
		// refresh a single-file upload gets. `close_batch` clears it at the end.
		self.dest.clone_from(&dir);
		self.slot = Some(TransferProgress::default());
		Effects::send(SshCommand::UploadTree {
			local,
			remote: dir,
			resume: false,
		})
	}

	/// Pull one file down (§19, §21). The bar starts at an unknown total the first progress event
	/// fills in — a download hears the size from the server, not from here.
	fn start_download(&mut self, remote: String, local: PathBuf) -> Effects {
		self.resumable = None;
		self.in_flight = Some(Resumable::Download {
			remote: remote.clone(),
			local: local.clone(),
		});
		self.notice = None;
		self.slot = Some(TransferProgress::default());
		Effects::send(SshCommand::Download {
			remote,
			local,
			resume: false,
		})
	}

	/// Something landed (§17, §21). Which counter it belongs to, and what the closing notice says,
	/// come from the direction it was going — a tree reports the same landing its files do, so
	/// `was` is what tells them apart.
	fn landed(&mut self, was: Option<&Resumable>, path: &str) -> Effects {
		if matches!(
			was,
			Some(Resumable::Download { .. } | Resumable::DownloadTree { .. })
		) {
			self.downloads_done += 1;
			self.notice = Some(format!("Saved to {path}"));
			let effects = self.pump();
			// A batch keeps going, and says how it went once the last file lands (§21).
			if self.slot.is_none() && self.downloads_done > 1 {
				self.notice = Some(format!("Saved {} files", self.downloads_done));
			}
			return effects;
		}
		if matches!(was, Some(Resumable::UploadTree { .. })) {
			self.trees_done += 1;
		} else {
			self.files_done += 1;
		}
		let mut effects = self.pump();
		if self.slot.is_none() && self.files.is_empty() {
			self.notice = Some(upload_summary(self.files_done, self.trees_done, path));
			// Show what just landed: if a pane is on the folder we uploaded into, re-list it so
			// the new file — or folder — appears without a manual Refresh (§29). Captured before
			// `close_batch`, which clears the destination.
			effects.refresh = Some(self.dest.clone());
			self.close_batch();
		}
		effects
	}

	/// Close a batch once it has fully drained (§17): nothing running and nothing left queued.
	/// The closing notice is set by whoever noticed the last item land.
	fn close_if_drained(&mut self) {
		if self.slot.is_none() && self.files.is_empty() && self.trees.is_empty() {
			self.close_batch();
		}
	}

	/// Drop the finished batch's leftovers (§17), keeping whatever notice is showing. Clearing the
	/// picked files is what disables the Upload button, so a stray click cannot re-send what just
	/// landed.
	fn close_batch(&mut self) {
		self.picked.clear();
		self.dest.clear();
		self.files.clear();
		self.trees.clear();
		self.files_done = 0;
		self.trees_done = 0;
		self.overwrite = false;
	}
}

/// What a drop onto the window should do (§29). Split out so the decision — is there a session,
/// is a transfer busy, is there anything to send, is there a directory to land in — is pure and
/// testable, the way `plan_uploads` is.
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
	/// tree-and-all, all of them queued behind the single slot (§29).
	Upload(String),
}

/// Decide a drop's fate from the state it depends on (§29), free of the queue so it is tested on
/// its own. `items` is a count rather than the paths themselves, because whether there is
/// anything at all is the whole of what this decides — sorting files from folders is the caller's
/// business.
///
/// The order is deliberate. No session outranks everything, so a drop onto a home tab is silent
/// whatever it held. Then a busy transfer, since nothing could start whatever the drop was. Then
/// an empty drop, which is silent for the same reason a drop with no session is. Only then does
/// the destination decide between `NoDir` and a real upload.
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

/// Build an upload batch's queue from the picked files, the destination folder and the answer to
/// the collision question (§17). `collisions` maps a name already in the folder to the free
/// `name-1` path the server pre-scan proposed; a file not in it is free and takes its own name.
/// `Replace` overwrites in place, `KeepBoth` writes to the free path, `Skip` drops the clashing
/// file (`Cancel` never reaches here — the batch is dropped before this). Pure, so the collision
/// logic is tested without a queue or a server.
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

/// The first free `name-1.ext`, `name-2.ext`… beside a local name already taken (§21) — the "save
/// alongside" answer to the collision question. Bounded by `explorer::FREE_NAME_TRIES`: after a
/// hundred tries the folder is telling us something, and the last candidate is returned rather
/// than spinning. Writing it is the download's problem, not this function's.
///
/// The queue asks the same shared rule the ssh layer asks, which is why the rule sits in
/// `explorer` beside `join` and `name` rather than in either of the two transfer modules: this one
/// deliberately knows nothing about `ssh::`, and the ssh one is a background-task spine the GUI
/// never links against.
fn free_name(dir: &Path, name: &str) -> PathBuf {
	for attempt in 1..=explorer::FREE_NAME_TRIES {
		let candidate = dir.join(explorer::free_candidate(name, attempt));
		if !candidate.exists() {
			return candidate;
		}
	}
	dir.join(explorer::free_candidate(name, explorer::FREE_NAME_TRIES))
}

/// A path's own file name, which is what the status bar shows and what the remote destination is
/// built from (§17). A path with no final component (a bare root) falls back to a placeholder
/// rather than an empty label.
fn file_name_of(path: &Path) -> &str {
	path.file_name()
		.and_then(std::ffi::OsStr::to_str)
		.unwrap_or("file")
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A queue with a batch planned and a folder queued behind it, as a drop of both kinds leaves
	/// it (§29). Built by hand rather than through `settle`, whose only use for the filesystem is
	/// telling a folder from a file — which is not what these tests are about.
	fn drop_of_both_kinds() -> Queue {
		let mut queue = Queue {
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		queue
			.files
			.push_back((PathBuf::from("/local/a.txt"), "/srv/a.txt".to_owned()));
		queue.trees.push_back(PathBuf::from("/local/photos"));
		queue
	}

	/// The remote path an `Upload` command is carrying, for asserting what actually went.
	fn upload_target(effects: &Effects) -> Option<&str> {
		match effects.commands.first()? {
			SshCommand::Upload { remote, .. } => Some(remote),
			_ => None,
		}
	}

	#[test]
	fn the_files_of_a_drop_go_before_its_folders() {
		// One queue each, one slot between them (§29). The batch drains first, because its
		// collision question was answered up front (§17) while a tree asks its own as it walks —
		// so the folder is still waiting when the file starts.
		let mut queue = drop_of_both_kinds();
		let effects = queue.pump();
		assert_eq!(upload_target(&effects), Some("/srv/a.txt"));
		assert_eq!(queue.trees.len(), 1, "the folder is still queued");
	}

	#[test]
	fn the_folders_of_a_drop_are_started_one_after_another() {
		// With the files through, the pump reaches the folder queue — and takes one at a time, so
		// the second waits for the first to report back rather than racing it (§29).
		let mut queue = Queue {
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		queue.trees.push_back(PathBuf::from("/local/one"));
		queue.trees.push_back(PathBuf::from("/local/two"));
		let _ = queue.pump();
		assert_eq!(queue.trees.len(), 1, "one folder started, one still queued");
		// The slot is taken now, so a second pump starts nothing at all.
		assert!(queue.pump().commands.is_empty());
		let _ = queue.ended(Ended::Done("/srv/one".to_owned()));
		assert!(queue.trees.is_empty(), "the landing started the second");
	}

	#[test]
	fn a_batch_does_not_close_while_folders_are_still_queued() {
		// Closing a batch clears the destination every queued folder is going to, so closing early
		// would strand them (§29).
		let mut queue = drop_of_both_kinds();
		queue.files.clear();
		queue.close_if_drained();
		assert_eq!(queue.dest, "/srv");
		assert_eq!(queue.trees.len(), 1);
	}

	#[test]
	fn a_batch_sends_one_file_at_a_time_and_the_landing_starts_the_next() {
		// The whole point of the queue: two files picked, one on the wire. What walks it is the
		// ending of the one before, not the queueing itself (§17).
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt"), PathBuf::from("/local/b.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let effects = queue.prescan(Vec::new());
		assert_eq!(upload_target(&effects), Some("/srv/a.txt"));
		assert!(queue.progress().is_some(), "the slot is taken");
		let effects = queue.ended(Ended::Done("/srv/a.txt".to_owned()));
		assert_eq!(upload_target(&effects), Some("/srv/b.txt"));
		let effects = queue.ended(Ended::Done("/srv/b.txt".to_owned()));
		assert!(effects.commands.is_empty(), "nothing left to send");
		assert_eq!(queue.notice(), Some("Uploaded 2 files"));
		assert_eq!(
			effects.refresh.as_deref(),
			Some("/srv"),
			"the folder it went into is re-listed"
		);
		assert!(!queue.busy(), "the batch closed itself out");
	}

	#[test]
	fn a_failure_moves_the_queue_on_rather_than_abandoning_the_batch() {
		// One file failing says what went wrong and takes the next (§17): a batch of twenty must
		// not be lost to one unreadable file.
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt"), PathBuf::from("/local/b.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let _ = queue.prescan(Vec::new());
		let effects = queue.ended(Ended::Failed("Permission denied".to_owned()));
		assert_eq!(upload_target(&effects), Some("/srv/b.txt"));
		assert_eq!(queue.notice(), Some("Permission denied"));
		assert!(!queue.can_resume(), "a plain failure kept no partial");
	}

	#[test]
	fn an_interruption_parks_the_transfer_and_resume_re_sends_the_same_one() {
		// A stop that KEPT its partial is the one case worth offering to finish (§16), and the
		// resume has to name the same two endpoints — otherwise it appends to the wrong file.
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let _ = queue.prescan(Vec::new());
		let effects = queue.ended(Ended::Interrupted("Connection reset".to_owned()));
		assert!(effects.commands.is_empty(), "nothing starts on its own");
		assert!(queue.can_resume());
		let effects = queue.resume();
		match effects.commands.first() {
			Some(SshCommand::Upload {
				local,
				remote,
				overwrite,
				resume,
			}) => {
				assert_eq!(local, &PathBuf::from("/local/a.txt"));
				assert_eq!(remote, "/srv/a.txt");
				// The partial is our own earlier work, not a clash, so the exists check is skipped.
				assert!(*overwrite);
				assert!(*resume);
			}
			other => panic!("expected a resumed upload, got {other:?}"),
		}
		assert!(
			!queue.can_resume(),
			"the offer is spent — a second press must not re-send it"
		);
	}

	#[test]
	fn a_resumed_transfer_is_itself_resumable() {
		// A flaky link may need more than one nudge, so a resume records itself the way a fresh
		// start does (§16).
		let mut queue = Queue {
			resumable: Some(Resumable::Download {
				remote: "/srv/big.iso".to_owned(),
				local: PathBuf::from("/local/big.iso"),
			}),
			..Queue::default()
		};
		let _ = queue.resume();
		let _ = queue.ended(Ended::Interrupted("Connection reset".to_owned()));
		assert!(queue.can_resume());
	}

	#[test]
	fn a_landing_forgets_the_previous_resume_offer() {
		// The offer belongs to ONE transfer. A file that lands after a failed one must clear it,
		// or Resume would relaunch something the user has already moved past (§16).
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			dest: "/srv".to_owned(),
			resumable: Some(Resumable::Upload {
				local: PathBuf::from("/local/stale.txt"),
				remote: "/srv/stale.txt".to_owned(),
			}),
			..Queue::default()
		};
		let _ = queue.prescan(Vec::new());
		assert!(!queue.can_resume(), "a fresh start already made it stale");
		let _ = queue.ended(Ended::Done("/srv/a.txt".to_owned()));
		assert!(!queue.can_resume());
	}

	#[test]
	fn a_lost_session_leaves_nothing_behind_to_act_on_the_next_one() {
		// Everything here belongs to the session that asked for it (§17, §21, §29). A Resume
		// button surviving a disconnect would relaunch a transfer against whatever server the tab
		// connected to next.
		let mut queue = drop_of_both_kinds();
		queue.picked = vec![PathBuf::from("/local/a.txt")];
		queue.resumable = Some(Resumable::Upload {
			local: PathBuf::from("/local/a.txt"),
			remote: "/srv/a.txt".to_owned(),
		});
		queue.caught(PathBuf::from("/local/late.txt"));
		queue.reset();
		assert!(!queue.busy());
		assert!(!queue.can_resume());
		assert!(!queue.settling());
		assert!(queue.asking().is_none());
	}

	#[test]
	fn a_transfer_a_dropped_session_took_with_it_is_offered_again_on_the_next_one() {
		// The commonest reason a big transfer stops is the link itself, and that used to be the
		// one reason cmote could not offer to pick it up from (§16): the teardown cleared the slot
		// along with everything else. The partial is still on the far side, so what the dead
		// session hands over is how far it got — and nothing else.
		let mut queue = drop_of_both_kinds();
		let _ = queue.pump();
		let unfinished = queue
			.abandon("u@h:22")
			.expect("the file on the wire is the offer");
		assert!(!queue.busy(), "everything else went with the session");
		assert!(
			queue.trees.is_empty(),
			"including the folders queued behind"
		);

		// The next session to the SAME server picks it up, says so, and resumes the very same two
		// endpoints — a resume that named different ones would append to the wrong file.
		let mut fresh = Queue::default();
		fresh.adopt(unfinished, "u@h:22");
		assert!(fresh.can_resume());
		assert_eq!(
			fresh.notice(),
			Some("a.txt stopped when the connection dropped")
		);
		match fresh.resume().commands.first() {
			Some(SshCommand::Upload {
				local,
				remote,
				resume,
				..
			}) => {
				assert_eq!(local, &PathBuf::from("/local/a.txt"));
				assert_eq!(remote, "/srv/a.txt");
				assert!(*resume);
			}
			other => panic!("expected the same upload, resumed: {other:?}"),
		}
	}

	#[test]
	fn an_offer_made_on_one_server_is_never_shown_on_another() {
		// Both its paths belong to the machine it was made on, and the partial it would append to
		// is over there (§16). A tab that reconnects somewhere else must therefore be offered
		// nothing at all — silently, since it is not a failure the user needs telling about.
		let mut queue = drop_of_both_kinds();
		let _ = queue.pump();
		let unfinished = queue.abandon("u@h:22").expect("something was moving");
		let mut elsewhere = Queue::default();
		elsewhere.adopt(unfinished, "u@other:22");
		assert!(!elsewhere.can_resume());
		assert_eq!(elsewhere.notice(), None);
	}

	#[test]
	fn an_offer_the_user_had_not_taken_up_yet_outlives_the_session_too() {
		// A transfer that failed mid-flight, then a connection that dropped before the user
		// reached the Resume button: the offer was already parked rather than in flight, and it is
		// just as good (§16).
		let mut queue = Queue {
			resumable: Some(Resumable::Download {
				remote: "/srv/big.iso".to_owned(),
				local: PathBuf::from("/local/big.iso"),
			}),
			..Queue::default()
		};
		let unfinished = queue.abandon("u@h:22").expect("the parked offer travels");
		let mut fresh = Queue::default();
		fresh.adopt(unfinished, "u@h:22");
		assert_eq!(
			fresh.notice(),
			Some("big.iso stopped when the connection dropped")
		);
	}

	#[test]
	fn a_cancelled_transfer_is_not_offered_after_a_reconnect() {
		// A cancel is final and its partial is deleted (§16), so there is nothing over there to
		// append to. Carrying one across a reconnect would offer to finish a file the user
		// deliberately stopped — and would restart it whole, since nothing is left of it.
		let mut queue = drop_of_both_kinds();
		let _ = queue.pump();
		let _ = queue.cancel();
		assert!(queue.abandon("u@h:22").is_none());
	}

	#[test]
	fn a_session_that_ended_with_nothing_moving_hands_nothing_on() {
		// Which is every ordinary disconnect. The next session gets a clean bar rather than a
		// Resume button for a transfer that finished perfectly well.
		let mut queue = Queue::default();
		assert!(queue.abandon("u@h:22").is_none());
	}

	#[test]
	fn a_cancel_takes_every_queue_with_it_but_leaves_the_bar_running() {
		// A deliberate cancel is final and takes the whole drop, not just the item on the wire
		// (§16, §29). The slot stays occupied until the worker's outcome lands, so the bar does
		// not flicker between the click and the wind-down.
		let mut queue = drop_of_both_kinds();
		let _ = queue.pump();
		let effects = queue.cancel();
		assert!(matches!(
			effects.commands.first(),
			Some(SshCommand::CancelTransfer)
		));
		assert!(queue.files.is_empty());
		assert!(queue.trees.is_empty());
		assert!(queue.progress().is_some(), "the bar is still up");
		assert!(!queue.can_resume(), "a cancel is not resumable");
		// The neutral outcome closes it out, and nothing queued starts behind it.
		let effects = queue.ended(Ended::Failed("Cancelled".to_owned()));
		assert!(effects.commands.is_empty());
		assert!(!queue.busy());
	}

	#[test]
	fn the_one_slot_rule_is_one_question() {
		// Every entrance asks this and only this, so a drop, a download and a folder upload refuse
		// on the same condition (§17). Each of the four states below used to be checked by a
		// different subset of the guards.
		let picked = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			..Queue::default()
		};
		assert!(picked.busy(), "a batch picked but not yet confirmed");
		let asking = Queue {
			ask: Some(Ask::Conflict),
			..Queue::default()
		};
		assert!(asking.busy(), "a transfer parked on a question");
		let queued = drop_of_both_kinds();
		assert!(queued.busy(), "folders waiting their turn");
		let running = Queue {
			slot: Some(TransferProgress::default()),
			..Queue::default()
		};
		assert!(running.busy(), "bytes on the wire");
		assert!(!Queue::default().busy());
	}

	#[test]
	fn a_batch_declines_while_something_else_is_queued() {
		// The confirmation cannot open over a running batch — but the batch it is ABOUT must not
		// count against it, which is the one place the rule is asked from the inside (§17).
		let mut queue = drop_of_both_kinds();
		queue.picked = vec![PathBuf::from("/local/b.txt")];
		let effects = queue.open_confirm("/srv".to_owned());
		assert!(effects.body.is_none(), "no question opened");
		assert_eq!(queue.notice(), Some(BUSY_NOTICE));
		// With the queues empty the same call opens, though the batch is still picked.
		queue.files.clear();
		queue.trees.clear();
		let effects = queue.open_confirm("/srv".to_owned());
		assert_eq!(queue.asking(), Some(Question::Dest));
		assert!(
			effects.focus_dest,
			"the destination field takes the keyboard"
		);
	}

	#[test]
	fn an_upload_clash_answered_with_cancel_drops_the_whole_batch() {
		// Nothing has been sent when the question is up, so cancelling costs nothing — and it must
		// take the picked files with it, or the Upload button would still offer to re-send them.
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let effects = queue.prescan(vec![("a.txt".to_owned(), "/srv/a-1.txt".to_owned())]);
		assert!(effects.body.is_some(), "the question was asked");
		assert_eq!(queue.asking(), Some(Question::UploadClash));
		queue.answer_clash(ClashChoice::Cancel);
		assert!(queue.asking().is_none());
		assert_eq!(queue.picked_count(), 0);
		assert!(!queue.busy());
	}

	#[test]
	fn one_answer_serves_whichever_clash_question_is_open() {
		// Only one can ever be open, so the queue reads off which it is holding rather than making
		// two messages carry the distinction (§17, §21).
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let _ = queue.prescan(vec![("a.txt".to_owned(), "/srv/a-1.txt".to_owned())]);
		let effects = queue.answer_clash(ClashChoice::KeepBoth);
		assert_eq!(
			upload_target(&effects),
			Some("/srv/a-1.txt"),
			"the free name the pre-scan proposed"
		);
		assert!(queue.asking().is_none());
	}

	#[test]
	fn the_keyboard_is_the_dialogs_while_a_question_or_a_transfer_holds_it() {
		// While one of these is up a keystroke belongs to cmote's own UI, not the session (§20).
		let mut queue = Queue::default();
		assert!(!queue.holds_keyboard());
		queue.ask = Some(Ask::Dest);
		assert!(queue.holds_keyboard());
		// The six-way conflict prompt is answered by its buttons alone.
		queue.ask = Some(Ask::Conflict);
		assert!(!queue.holds_keyboard());
		queue.ask = None;
		queue.slot = Some(TransferProgress::default());
		assert!(queue.holds_keyboard());
	}

	#[test]
	fn escape_backs_out_of_a_question_but_not_out_of_a_running_transfer() {
		// Esc is a dismissal, not a stop: the ✕ in the bar is how a transfer is cancelled (§16).
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt")],
			..Queue::default()
		};
		queue.ask = Some(Ask::Dest);
		queue.escape();
		assert!(queue.asking().is_none());
		assert_eq!(queue.picked_count(), 0, "the batch went with the question");

		let mut queue = Queue {
			slot: Some(TransferProgress {
				sent: 10,
				total: 99,
			}),
			..Queue::default()
		};
		queue.escape();
		assert_eq!(
			queue.progress(),
			Some(TransferProgress {
				sent: 10,
				total: 99
			})
		);
	}

	#[test]
	fn progress_after_a_transfer_has_stopped_does_not_revive_the_bar() {
		// The worker's last progress event can arrive behind the failure that ended the transfer;
		// showing it would leave a bar moving under a session with nothing on the wire (§17).
		let mut queue = Queue::default();
		queue.progressed(10, 100);
		assert_eq!(queue.progress(), None);
	}

	#[test]
	fn the_closing_notice_counts_files_and_folders_apart() {
		// A drop of both kinds says what actually went (§29) — "3 files and 2 folders" rather than
		// five of something. Which counter each landing belongs to comes from the direction it was
		// going, which is why a tree and a file can report the same ending.
		let mut queue = Queue {
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		queue
			.files
			.push_back((PathBuf::from("/local/a.txt"), "/srv/a.txt".to_owned()));
		queue.trees.push_back(PathBuf::from("/local/photos"));
		let _ = queue.pump();
		let _ = queue.ended(Ended::Done("/srv/a.txt".to_owned()));
		let _ = queue.ended(Ended::Done("/srv/photos".to_owned()));
		assert_eq!(queue.notice(), Some("Uploaded 1 file and 1 folder"));
	}

	#[test]
	fn a_download_batch_names_one_file_and_counts_several() {
		// One file: the path says more than a count of one does (§21).
		let mut queue = Queue::default();
		queue
			.downloads
			.push_back(("/srv/a.txt".to_owned(), PathBuf::from("/local/a.txt")));
		let _ = queue.pump();
		let _ = queue.ended(Ended::Done("/local/a.txt".to_owned()));
		assert_eq!(queue.notice(), Some("Saved to /local/a.txt"));

		// Several: each landing names itself, but the next one starting immediately clears the
		// line for its own progress bar — so what the user is left reading is the count, once the
		// last has landed.
		let mut queue = Queue::default();
		queue
			.downloads
			.push_back(("/srv/a.txt".to_owned(), PathBuf::from("/local/a.txt")));
		queue
			.downloads
			.push_back(("/srv/b.txt".to_owned(), PathBuf::from("/local/b.txt")));
		let _ = queue.pump();
		let _ = queue.ended(Ended::Done("/local/a.txt".to_owned()));
		assert_eq!(queue.notice(), None, "the second is already running");
		let _ = queue.ended(Ended::Done("/local/b.txt".to_owned()));
		assert_eq!(queue.notice(), Some("Saved 2 files"));
	}

	#[test]
	fn a_skipped_file_names_itself_and_moves_the_queue_on() {
		// A file that appeared on the server after the pre-scan is skipped rather than reopening
		// the question mid-batch (§17).
		let mut queue = Queue {
			picked: vec![PathBuf::from("/local/a.txt"), PathBuf::from("/local/b.txt")],
			dest: "/srv".to_owned(),
			..Queue::default()
		};
		let _ = queue.prescan(Vec::new());
		let effects = queue.ended(Ended::Skipped("/srv/a.txt".to_owned()));
		assert_eq!(
			queue.notice(),
			Some("Skipped a.txt — it appeared on the server")
		);
		assert_eq!(upload_target(&effects), Some("/srv/b.txt"));
	}

	#[test]
	fn the_paths_of_one_drop_gather_until_the_frame_reads_them() {
		// The OS reports a multi-file drop as one event per path and never says which is the last
		// (§29), so each event only gathers. The frame that follows takes the lot and leaves
		// nothing behind — otherwise the next drop would inherit these paths and upload them again.
		let mut queue = Queue::default();
		queue.caught(PathBuf::from("/local/a.txt"));
		queue.caught(PathBuf::from("/local/b.txt"));
		assert!(queue.settling(), "both paths waited for the frame");
		// No session here, so the decision itself is `Ignore` — what this pins is that the settle
		// consumes the set either way.
		let _ = queue.settle(false, Some("/home/user"));
		assert!(!queue.settling());
	}

	#[test]
	fn a_drop_puts_the_target_highlight_out() {
		// The drag is over the moment a path lands, whatever is then decided about it (§29).
		let mut queue = Queue::default();
		queue.hover(true);
		queue.caught(PathBuf::from("/local/a.txt"));
		assert!(!queue.hovering());
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
		// Not a mistake the user made — every dropped path would have had to vanish between the
		// drop and the frame that reads it — so it is silent rather than a notice about an empty
		// drop.
		assert_eq!(
			drop_outcome(true, false, 0, Some("/home/user")),
			DropOutcome::Ignore
		);
	}

	#[test]
	fn one_thing_uploaded_is_named_by_its_path() {
		// With a single item the path says more than a count does — "Uploaded 1 file" tells the
		// user nothing they did not just watch happen (§29).
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
	fn a_mixed_drop_names_both_kinds() {
		// Adding them up would make "5 things" of three files and two folders, which is not what
		// the user dropped (§29).
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

	#[test]
	fn a_free_batch_keeps_every_name() {
		// Nothing clashes, so every file lands under its own name in the destination folder.
		let files = vec![PathBuf::from("/local/a.txt"), PathBuf::from("/local/b.txt")];
		let queue = plan_uploads(&files, "/remote/dir", &[], ClashChoice::Skip);
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
	fn a_clashing_batch_follows_the_answer_it_was_given() {
		// One of three clashes; the other two are free whatever the answer, which is what makes
		// the one-question-per-batch model safe (§17).
		let files = vec![
			PathBuf::from("/local/a.txt"),
			PathBuf::from("/local/b.txt"),
			PathBuf::from("/local/c.txt"),
		];
		let clashing = vec![("b.txt".to_owned(), "/remote/dir/b-1.txt".to_owned())];

		// Replace: the clashing file keeps its own name and is overwritten in place.
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

		// Keep both: it goes to the free path the server proposed.
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

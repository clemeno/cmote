// targets.rs — the saved connection targets (PLAN §14, v1.3).
//
// The home screen lists targets the user has connected to before, so a return
// visit is one click instead of re-typing host / port / user / auth. What we
// persist is deliberately narrow:
//
//   TARGET METADATA ONLY — name, host, port, user, auth kind, key-file path.
//   NEVER a secret. No password, no key passphrase (§12). The user still enters
//   the secret at connect time; only the "how to reach it" part is remembered.
//
// That keeps the §12 guarantee ("the safest secret is the one never persisted")
// intact for THIS file while still making reconnecting convenient, and it keeps the
// store fully portable — a `targets.json` copied to another machine leaks nothing.
//
// Opt-in secret persistence, encrypted at rest, now exists as a SEPARATE, deliberate
// relaxation (§16, `vault.rs`): a saved password / key passphrase lives only in the
// encrypted `secrets.age`, never here. All this file gains is a `remember_secret` flag
// (metadata) so the home list and form know a secret can be pre-filled from that vault.
//
// The file is `targets.json` in the shared data directory (§11, `paths::data_dir`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::change::Change;
use crate::elevate::ElevateKind;
use crate::files::{SortDir, SortKey};
use crate::forward::ForwardSpec;
use crate::ui::connect::AuthKind;

/// One saved connection target — metadata only, no secret material (§12).
/// `name` is a free display label (defaults to the endpoint, renamed by the user);
/// the rest is exactly what the connect form needs to be pre-filled.
// `Eq` is deliberately not derived: the pane sizes are `f32`, which is `PartialEq` but not
// `Eq` (NaN). `PartialEq` is all the tests and the change-detection in `set_session` need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
	/// The display label shown in the home list and used for the alphabetical sort.
	pub name: String,
	pub host: String,
	pub port: u16,
	pub user: String,
	/// Which auth method to pre-select. The secret itself is never stored (§12).
	pub auth_kind: AuthKind,
	/// The private-key file for key auth, if this target uses one. Absent for
	/// password auth; omitted from the JSON when `None` to keep the file tidy.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub key_path: Option<PathBuf>,
	/// The OpenSSH certificate file for key auth, if this target presents one alongside its key
	/// (§7). A certificate is public data (like the key *path*), never a secret, so it is
	/// remembered here; absent for plain key or password auth, and omitted from the JSON when
	/// `None` so an older store and a cert-less target both stay tidy.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub cert_path: Option<PathBuf>,
	/// Whether the folder tree and the files pane list dot-prefixed entries (§18, §19).
	/// A per-target display preference, not a secret: which server this is decides
	/// whether its dotfiles are the point or the noise, so the `.*` toggle is
	/// remembered here rather than globally.
	#[serde(default = "shown_by_default")]
	pub show_hidden: bool,
	/// Where the remote shell's working directory was when the session last ended (§22),
	/// replayed as a `cd` on the next connection so the shell resumes where it was. Absent
	/// until a session that actually announced a cwd has ended (a shell that emits no OSC
	/// directory sequence never fills it), and a plain resume point — not a secret — so it
	/// rides here beside `show_hidden`. Omitted from the JSON when `None`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub terminal_path: Option<String>,
	/// Where the files pane was pointed when the session last ended (§22), reopened in the
	/// pane on the next connection. Kept apart from `terminal_path` because a tree click can
	/// point the pane somewhere the shell is not, so the two can legitimately differ.
	/// Omitted from the JSON when `None`.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub files_path: Option<String>,
	/// The explorer pane's width and the files pane's height when the session last ended
	/// (§22), so the layout reopens as it was left. Absent until a session has closed;
	/// omitted from the JSON when `None`, so the panes then take their built-in defaults.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub explorer_width: Option<f32>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub files_height: Option<f32>,
	/// The files pane's sort when the session last ended (§19, §22), reapplied on the next
	/// connection so the grid reopens in the order this target was left in. A per-target display
	/// preference, not a secret, so it rides here beside `show_hidden`: which server this is decides
	/// how its files are best read. Both halves are optional and independent — `sort` is the key (a
	/// missing one is the default dirs-first-by-name order) and `sort_dir` the direction (a missing
	/// one sorts ascending) — and each is omitted from the JSON when unset, so an older file loads
	/// with no sort and a tidy one stays tidy.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sort: Option<SortKey>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub sort_dir: Option<SortDir>,
	/// Whether an encrypted secret (a password or a key passphrase) is stored for this target
	/// in the portable vault (§16, `vault.rs`). Metadata ONLY — this flag rides here so the
	/// home list and the connect form know a secret can be pre-filled, while the secret itself
	/// lives solely in `secrets.age`, never in this file (§12). Off by default; a `targets.json`
	/// written before opt-in persistence existed loads with it false and behaves as before.
	#[serde(default, skip_serializing_if = "crate::store::is_default")]
	pub remember_secret: bool,
	/// The port forwards to re-establish on the next connection to this target (§27). Persisted
	/// because a tunnel set (a database on a bastion, a SOCKS proxy) is part of "how I use this
	/// server", not a secret — so it rides here beside the resume paths, and reconnecting sets
	/// them up again automatically. Empty by default and omitted from the JSON when empty, so an
	/// older `targets.json` loads with no forwards and behaves exactly as before.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub forwards: Vec<ForwardSpec>,
	/// Which account this target's sessions become, and whether they do it by themselves (§47).
	/// `None` — the default, and what every `targets.json` written before §47 loads as — means the
	/// session stays the account it logged in as until the user asks for another.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub elevate: Option<Elevation>,
}

/// What a target's sessions become after logging in, and how (§47).
///
/// A saved elevation is "how I use this server" in the same sense the forwards and the resume paths
/// are: on `rec.michoacan` the work is done as root and the login account is a doorway. So it rides
/// in `targets.json` beside them — and, like them, it is METADATA ONLY. The `account` is a login
/// name and the `kind` is a program name; neither is a secret. The password, if the user opts into
/// keeping one at all, lives in the sealed vault and nowhere else (§12, §16).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elevation {
	/// Which program does it — `sudo`, which asks for the caller's own password, or `su`, which
	/// asks for the target account's (§45).
	pub kind: ElevateKind,
	/// The account to become. Checked by `elevate::valid_user` before it can reach a command line,
	/// and checked again on the way OUT of this file: a `targets.json` edited by hand is remote
	/// input as far as that check is concerned (§12).
	pub account: String,
	/// Whether to start the elevation as soon as the session's shell is live, rather than waiting
	/// for the user to ask. Off by default, so a target can remember an account without the
	/// session acting on it — which is what the elevate dialog's own field is for.
	#[serde(default, skip_serializing_if = "crate::store::is_default")]
	pub on_connect: bool,
	/// Whether a password for this elevation is in the vault (§16). The same shape as
	/// `remember_secret` above and for the same reason: the flag is the hint the form and the
	/// dialog read, so neither promises a hands-free elevation the vault cannot deliver, while the
	/// secret itself lives only in `secrets.age`.
	#[serde(default, skip_serializing_if = "crate::store::is_default")]
	pub remember_password: bool,
}

impl Elevation {
	/// Whether this elevation is one cmote will act on, which is not the same as one that parsed.
	///
	/// `targets.json` is a file the user is invited to edit (§22), so an account read back out of it
	/// gets the same check as one typed into the dialog — `elevate::valid_user`, the rule that keeps
	/// anything but a plain login name out of the command line `ElevateKind::command` composes. An
	/// account this refuses is not an error to report: it is a stored preference cmote declines to
	/// act on, and the session stays the account it logged in as.
	pub fn usable(&self) -> bool {
		crate::elevate::valid_user(&self.account)
	}
}

/// The slice of a target's state that one session updates and the next restores (§22): where
/// the shell and files pane were, the `.*` filter, and the two pane sizes. A transfer
/// struct, not stored directly — `Target` keeps these as flat fields (so the JSON stays flat
/// and older files still load), and this is what `capture` fills, `restore` reads and
/// `set_session` writes. Every field means "leave what is stored" when it is absent — `None` for the
/// two-valued ones, `Change::Keep` for the sort — so a value a session could not determine (a shell
/// that announced no cwd) never erases a good one an earlier session recorded. Adding another
/// remembered value is one field here, one on `Target`, and one line each in capture / restore /
/// `set_session`.
///
/// **Was `SessionState`, which is what it is not** (§135). It is not a session's state — `app`'s
/// `SessionPhase` holds that — but what a TARGET remembers about the last one, and
/// `CONTEXT.md`'s **Target** entry already had the phrase for it: *"where the last session left off"*.
/// The glossary named this correctly before the type did.
#[derive(Debug, Default, Clone)]
pub struct LeftOff {
	pub terminal_path: Option<String>,
	pub files_path: Option<String>,
	pub show_hidden: Option<bool>,
	pub explorer_width: Option<f32>,
	pub files_height: Option<f32>,
	/// The pane's sort, both halves (§19). Not an `Option` like the fields above, because a sort has
	/// THREE answers rather than two: `Keep` leaves the stored sort alone, `Clear` records that this
	/// session had no sort, and `Set` names one. A real capture never says `Keep` — the pane always
	/// knows its own sort, even when that sort is unset (§111, `change::Change`).
	pub sort: Change<SortKey>,
	pub sort_dir: Change<SortDir>,
}

/// Serde default for `show_hidden` — matches the panes' own default (shown), so a
/// `targets.json` written before this field existed keeps behaving as it did.
fn shown_by_default() -> bool {
	true
}

impl Target {
	/// The endpoint string `user@host:port`. This doubles as the target's stable
	/// identity: the store keeps one target per endpoint (see `upsert_on_connect`),
	/// so this is what the home screen tracks a selection / rename by, and it never
	/// changes under a rename (which only touches `name`).
	pub fn endpoint(&self) -> String {
		endpoint_of(&self.user, &self.host, self.port)
	}

	/// Does this target survive the home screen's filter box (§49)? The pattern is tried against
	/// BOTH halves of the row — the name the user gave it and the `user@host:port` it stands for
	/// — and either hit keeps the row. Both are on screen, so filtering by only one of them
	/// would hide rows whose match the user can see; and the two are searched for different
	/// reasons: the name for the machine's role ("build", "staging"), the endpoint for where it
	/// actually is (a subnet, a login, a port).
	///
	/// The rule itself is `glob::matches` — a fragment until a `*` or `?` is typed, then a
	/// whole-text glob. Living here rather than in the view is what lets `app` ask the same
	/// question of the selected row that the list asked of every row.
	pub fn matches(&self, pattern: &str) -> bool {
		crate::glob::matches(pattern, &self.name) || crate::glob::matches(pattern, &self.endpoint())
	}

	/// This target's remembered session state (§22), read out for the next connection to
	/// restore. `show_hidden` is always known (it is a plain flag), so it comes back as
	/// `Some`; the rest are as absent or present as they were stored.
	pub fn session(&self) -> LeftOff {
		LeftOff {
			terminal_path: self.terminal_path.clone(),
			files_path: self.files_path.clone(),
			show_hidden: Some(self.show_hidden),
			explorer_width: self.explorer_width,
			files_height: self.files_height,
			// The stored sort is always known (both halves may be unset), so `reported` is the right
			// constructor: it carries the exact tri-state — key and direction — for the next
			// connection, and never says `Keep`.
			sort: Change::reported(self.sort),
			sort_dir: Change::reported(self.sort_dir),
		}
	}
}

/// Build the `user@host:port` endpoint key without a `Target` in hand — used when
/// upserting from raw connect fields.
pub fn endpoint_of(user: &str, host: &str, port: u16) -> String {
	format!("{user}@{host}:{port}")
}

/// The set of saved targets, kept sorted for display. Small enough that a `Vec`
/// re-sorted on every mutation is simpler and clearer than a keyed map plus a
/// separate order — there will be a handful of targets, not thousands.
#[derive(Debug, Default, Clone)]
pub struct Targets {
	/// Always kept in display order (see `sort`): by name (case-insensitively),
	/// ties broken by endpoint so the order is deterministic.
	items: Vec<Target>,
	/// Why this store was refused, when it was (§110): the file on disk declares a format version
	/// this build does not know, which means a NEWER cmote wrote it — the expected case for a
	/// portable install carried between machines on a stick. Set means two things: the list is
	/// empty because nothing was read, and `save_to` must refuse, because writing this build's
	/// shape over a newer one would destroy whatever the newer fields hold.
	refusal: Option<String>,
	/// Whether what was loaded came from an older shape, so the first save preserves the original
	/// as `targets.json.bak` before replacing it (§110).
	migrated: bool,
}

/// The format version this build writes (§110).
///
/// A STRING rather than a number, because `targets.json` is meant to be readable and editable by
/// hand (§22) and `"2"` beside the other quoted values reads as one of them. Version 1 is the
/// UNVERSIONED shape — a bare JSON array — which is what every cmote up to §110 wrote; calling it 1
/// keeps "version N" and "the Nth format" the same number for good.
const FORMAT: &str = "2";

/// What version 2 looks like on disk: the array, under a version that says which array it is.
///
/// Deserialize-only. Writing goes through [`Written`], which borrows instead of cloning the whole
/// list to serialize it.
#[derive(Deserialize)]
struct Stored {
	version: String,
	targets: Vec<Target>,
}

/// The write side of [`Stored`], borrowing what it serializes.
#[derive(Serialize)]
struct Written<'a> {
	version: &'a str,
	targets: &'a [Target],
}

impl Targets {
	/// Why this store was refused, for the home screen to show instead of "no saved targets yet"
	/// (§110). An empty list and a refusal look identical otherwise, and they must not: one means
	/// "connect somewhere to add one" and the other means "your targets are still on disk, and this
	/// cmote is too old to read them".
	pub fn refusal(&self) -> Option<&str> {
		self.refusal.as_deref()
	}

	/// Read `text` as a store, whichever shape it is in (§110).
	///
	/// The shape is told apart by the first non-whitespace byte, which is all it takes: `[` is the
	/// unversioned array, `{` is the envelope. That is why the envelope was worth the change — a
	/// format that says what it is can be recognised without guessing, forever.
	fn parse(text: &str, path: &Path) -> Self {
		match text.trim_start().as_bytes().first() {
			// Version 1: the bare array every cmote before §110 wrote. Loaded and marked for
			// migration, so the next save writes the envelope and keeps the original beside it.
			Some(b'[') => match serde_json::from_str::<Vec<Target>>(text) {
				Ok(items) => Self {
					items,
					refusal: None,
					migrated: true,
				},
				Err(error) => {
					eprintln!(
						"ignoring an unreadable targets file {}: {error}",
						path.display()
					);
					Self::default()
				}
			},
			Some(b'{') => match serde_json::from_str::<Stored>(text) {
				Ok(stored) if stored.version == FORMAT => Self {
					items: stored.targets,
					refusal: None,
					migrated: false,
				},
				// Any other version in an envelope is one this build does not know. Refused
				// rather than guessed at: the fields it cannot see are the ones a newer cmote
				// is relying on, and a save would drop them silently (§110).
				Ok(stored) => Self {
					items: Vec::new(),
					refusal: Some(format!(
						"{} is version {} — this cmote reads version {FORMAT}. Nothing was loaded, 						 and nothing will be written over it. Use a newer cmote, or move the file 						 aside.",
						path.display(),
						stored.version
					)),
					migrated: false,
				},
				Err(error) => {
					eprintln!(
						"ignoring an unreadable targets file {}: {error}",
						path.display()
					);
					Self::default()
				}
			},
			_ => {
				eprintln!(
					"ignoring a targets file that is neither an array nor an object: {}",
					path.display()
				);
				Self::default()
			}
		}
	}

	/// The targets in display order. The home screen renders these top-to-bottom and
	/// indexes into this slice for click/selection.
	pub fn items(&self) -> &[Target] {
		&self.items
	}

	/// Look a target up by its endpoint key (`user@host:port`).
	pub fn find(&self, endpoint: &str) -> Option<&Target> {
		self.items
			.iter()
			.find(|target| target.endpoint() == endpoint)
	}

	/// Record a successful connection (§14): if a target already exists for this
	/// endpoint, refresh its auth kind / key path / certificate but KEEP its custom name;
	/// otherwise add a new one named after the endpoint. Returns the endpoint key so the caller
	/// can select the row that was just saved. Re-sorts so the list stays ordered.
	pub fn upsert_on_connect(
		&mut self,
		host: &str,
		port: u16,
		user: &str,
		auth_kind: AuthKind,
		key_path: Option<PathBuf>,
		cert_path: Option<PathBuf>,
	) -> String {
		let endpoint = endpoint_of(user, host, port);
		match self.items.iter_mut().find(|t| t.endpoint() == endpoint) {
			Some(existing) => {
				// Endpoint already known: update how we authenticate, leave the name alone.
				existing.auth_kind = auth_kind;
				existing.key_path = key_path;
				existing.cert_path = cert_path;
			}
			None => {
				self.items.push(Target {
					name: endpoint.clone(),
					host: host.to_string(),
					port,
					user: user.to_string(),
					auth_kind,
					key_path,
					cert_path,
					show_hidden: shown_by_default(),
					// A brand-new target has no session behind it yet, so there is nowhere to
					// resume to — the first connect uses the fallbacks (root / login dir, and
					// the panes' default sizes).
					terminal_path: None,
					files_path: None,
					explorer_width: None,
					files_height: None,
					// A brand-new target opens in the default order — no key, no direction — until
					// the user picks one on a live session (§19).
					sort: None,
					sort_dir: None,
					// A brand-new target has stored no secret yet; the flag is set later, only
					// if a connect actually persists one to the vault (§16).
					remember_secret: false,
					// No forwards until the user adds some on a live session (§27); an existing
					// endpoint's saved forwards are left untouched by the auth-only branch above.
					forwards: Vec::new(),
					// And no elevation: a brand-new target stays the account it logs in as until the
					// user says otherwise, on the form or in the dialog (§47).
					elevate: None,
				});
			}
		}
		self.sort();
		endpoint
	}

	/// Set (or clear) the "a secret is stored for this target" flag (§16), returning whether it
	/// changed. Called after a successful connect to keep the flag in step with what the vault
	/// actually holds — the source of truth is the vault, this is only the hint the home list
	/// and form read so they never promise a pre-fill that is not there. A missing endpoint is
	/// a no-op.
	pub fn set_remembered(&mut self, endpoint: &str, remember: bool) -> bool {
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		if target.remember_secret == remember {
			return false;
		}
		target.remember_secret = remember;
		true
	}

	/// Replace the saved forwards for this target (§27), returning whether they changed. The
	/// forward list is add/remove, not a fold of optional fields like `set_session`, so it is
	/// written whole: the app hands the current set after every add or removal on a live
	/// session, and an unchanged set reports `false` so the caller skips the disk write. A
	/// missing endpoint is a no-op.
	pub fn set_forwards(&mut self, endpoint: &str, forwards: Vec<ForwardSpec>) -> bool {
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		if target.forwards == forwards {
			return false;
		}
		target.forwards = forwards;
		true
	}

	/// Remember what this target's sessions should become (§47), returning whether anything changed.
	///
	/// The password flag is deliberately NOT a parameter: it follows what the vault actually holds
	/// and is set by `set_elevation_remembered` below, so a caller recording a preference cannot
	/// promise a stored password by accident. An existing elevation for a DIFFERENT account is
	/// replaced — one target remembers one account, which is the whole of what "on every connection
	/// to this target" can mean.
	pub fn set_elevation(
		&mut self,
		endpoint: &str,
		account: &str,
		kind: ElevateKind,
		on_connect: bool,
	) -> bool {
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		// The flag is carried over only for the SAME account: a password stored for `root` says
		// nothing about `deploy`, and the vault key says so too (`vault::elevation_key`).
		let remember_password = target
			.elevate
			.as_ref()
			.is_some_and(|saved| saved.account == account && saved.remember_password);
		let wanted = Elevation {
			kind,
			account: account.to_owned(),
			on_connect,
			remember_password,
		};
		if target.elevate.as_ref() == Some(&wanted) {
			return false;
		}
		target.elevate = Some(wanted);
		true
	}

	/// Set (or clear) the "a password is stored for this elevation" flag (§47), returning whether it
	/// changed.
	///
	/// The same shape as `set_remembered` for the connect secret, and the same reason: the vault is
	/// the source of truth and this is only the hint the dialog reads, so it never opens promising a
	/// hands-free elevation the vault cannot deliver. A target with no stored elevation, or one
	/// stored for another account, is a no-op — the flag belongs to the account the password was for.
	pub fn set_elevation_remembered(
		&mut self,
		endpoint: &str,
		account: &str,
		remembered: bool,
	) -> bool {
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		let Some(saved) = target.elevate.as_mut() else {
			return false;
		};
		if saved.account != account || saved.remember_password == remembered {
			return false;
		}
		saved.remember_password = remembered;
		true
	}

	/// Rename the target with this endpoint key. A blank/whitespace-only name is
	/// rejected (the row would be unlabelled), so the caller can keep the old name.
	/// Re-sorts on success so the list reflects the new name immediately. Returns
	/// whether anything changed.
	pub fn rename(&mut self, endpoint: &str, new_name: &str) -> bool {
		let new_name = new_name.trim();
		if new_name.is_empty() {
			return false;
		}
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		if target.name == new_name {
			return false;
		}
		target.name = new_name.to_string();
		self.sort();
		true
	}

	/// Fold a session's snapshot into the stored target (§22) — the write side of
	/// `Target::session`, and the one place every remembered per-target value lands. Each
	/// `Some` field overwrites, each `None` leaves the stored value alone, so a snapshot that
	/// could not determine a value (a shell that announced no cwd) preserves whatever an
	/// earlier session wrote rather than clearing it. Returns whether anything actually moved,
	/// so the caller only rewrites the file when it did.
	pub fn set_session(&mut self, endpoint: &str, session: LeftOff) -> bool {
		let Some(target) = self.items.iter_mut().find(|t| t.endpoint() == endpoint) else {
			return false;
		};
		let mut changed = false;
		if let Some(path) = session.terminal_path
			&& target.terminal_path.as_deref() != Some(path.as_str())
		{
			target.terminal_path = Some(path);
			changed = true;
		}
		if let Some(path) = session.files_path
			&& target.files_path.as_deref() != Some(path.as_str())
		{
			target.files_path = Some(path);
			changed = true;
		}
		if let Some(show_hidden) = session.show_hidden
			&& target.show_hidden != show_hidden
		{
			target.show_hidden = show_hidden;
			changed = true;
		}
		if let Some(width) = session.explorer_width
			&& target.explorer_width != Some(width)
		{
			target.explorer_width = Some(width);
			changed = true;
		}
		if let Some(height) = session.files_height
			&& target.files_height != Some(height)
		{
			target.files_height = Some(height);
			changed = true;
		}
		// The sort's two halves fold in like the rest, except that their tri-state carries the
		// difference the `Option` fields above cannot express: `Clear` writes "no key / no
		// direction", which is how a session that cleared its sort is remembered as cleared, while
		// `Keep` leaves the stored value alone. `|=` rather than `||`, so the second half is folded
		// in even when the first already reported a change.
		changed |= session.sort.fold_into(&mut target.sort);
		changed |= session.sort_dir.fold_into(&mut target.sort_dir);
		changed
	}

	/// Remove the target with this endpoint key. Returns whether one was removed.
	pub fn remove(&mut self, endpoint: &str) -> bool {
		let before = self.items.len();
		self.items.retain(|t| t.endpoint() != endpoint);
		self.items.len() != before
	}

	/// Sort into display order: by name, case-insensitively (so "alpha" and "Beta"
	/// order naturally rather than all-lowercase after all-uppercase), with the
	/// endpoint as a stable tie-breaker.
	fn sort(&mut self) {
		self.items.sort_by(|a, b| {
			a.name
				.to_lowercase()
				.cmp(&b.name.to_lowercase())
				.then_with(|| a.endpoint().cmp(&b.endpoint()))
		});
	}

	/// Load the targets from `path`. A missing file is not an error — it just means
	/// no targets yet (empty). A file we cannot read or parse is logged and treated
	/// as empty rather than crashing the app: a corrupt store must never stop the
	/// user from connecting (`ponytail:` we do not try to recover partial entries).
	pub fn load_from(path: &Path) -> Self {
		let mut targets = match std::fs::read_to_string(path) {
			Ok(text) => Self::parse(&text, path),
			// `NotFound` is the normal first-run case; anything else is worth a line.
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
			Err(error) => {
				eprintln!("could not read targets file {}: {error}", path.display());
				Self::default()
			}
		};
		// Trust nothing about the on-disk order — sort on load so a hand-edited file
		// still displays correctly.
		targets.sort();
		targets
	}

	/// Write the targets to `path` as pretty JSON (readable, since it is a plain
	/// config file). Creates the parent directory if needed.
	pub fn save_to(&self, path: &Path) -> Result<()> {
		if let Some(reason) = &self.refusal {
			anyhow::bail!("refusing to write the targets file: {reason}");
		}
		// The original is preserved before the FIRST save that changes its shape, and only then:
		// `back_up_once` leaves an existing `.bak` alone, so what is kept is the version-1 file and
		// not a migrated copy of it (§110).
		if self.migrated {
			crate::store::back_up_once(path)?;
		}
		let written = Written {
			version: FORMAT,
			targets: &self.items,
		};
		let json = serde_json::to_string_pretty(&written).context("failed to encode targets")?;
		crate::store::write_atomically(path, json.as_bytes())
	}

	/// Load from the resolved store location (§11). Convenience over `load_from`.
	pub fn load() -> Self {
		match targets_path() {
			Ok(path) => Self::load_from(&path),
			Err(error) => {
				eprintln!("could not resolve the targets path: {error:#}");
				Self::default()
			}
		}
	}

	/// Save to the resolved store location (§11). Convenience over `save_to`.
	pub fn save(&self) -> Result<()> {
		self.save_to(&targets_path()?)
	}
}

/// The path of the saved-targets file: `targets.json` in the shared data directory.
fn targets_path() -> Result<PathBuf> {
	Ok(crate::paths::data_dir()?.join("targets.json"))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// What a target remembers about becoming another account (§47), and the two rules that keep the
	/// password flag honest: it is not a parameter of the preference, and it belongs to the ACCOUNT
	/// the password was for.
	#[test]
	fn a_saved_elevation_keeps_its_password_flag_only_for_the_same_account() {
		let mut targets = Targets::default();
		targets.upsert_on_connect("rec", 22, "cme", AuthKind::Password, None, None);

		// Recording the preference cannot promise a stored password.
		assert!(targets.set_elevation("cme@rec:22", "root", ElevateKind::Sudo, true));
		let saved = targets.find("cme@rec:22").unwrap().elevate.clone().unwrap();
		assert_eq!(saved.account, "root");
		assert!(saved.on_connect);
		assert!(!saved.remember_password);

		// The vault settles the flag afterwards.
		assert!(targets.set_elevation_remembered("cme@rec:22", "root", true));
		assert!(
			targets
				.find("cme@rec:22")
				.unwrap()
				.elevate
				.as_ref()
				.unwrap()
				.remember_password
		);

		// Re-recording the SAME account keeps it — turning "on every connection" off is not a
		// statement about the password.
		assert!(targets.set_elevation("cme@rec:22", "root", ElevateKind::Sudo, false));
		assert!(
			targets
				.find("cme@rec:22")
				.unwrap()
				.elevate
				.as_ref()
				.unwrap()
				.remember_password
		);

		// Recording ANOTHER account drops it: a password stored for `root` says nothing about
		// `deploy`, and the vault key says so too.
		assert!(targets.set_elevation("cme@rec:22", "deploy", ElevateKind::Su, true));
		let saved = targets.find("cme@rec:22").unwrap().elevate.clone().unwrap();
		assert_eq!(saved.account, "deploy");
		assert_eq!(saved.kind, ElevateKind::Su);
		assert!(!saved.remember_password);

		// And the flag for an account this target no longer remembers is nobody's to set.
		assert!(!targets.set_elevation_remembered("cme@rec:22", "root", true));
	}

	/// An account read back out of `targets.json` gets the same check as one typed into the dialog
	/// (§12, §47): the file is one the user is invited to edit, so it is remote input to the check
	/// that keeps anything but a login name out of a command line.
	#[test]
	fn a_stored_account_is_vetted_on_the_way_out_as_well_as_in() {
		let usable = |account: &str| {
			Elevation {
				kind: ElevateKind::Sudo,
				account: account.to_owned(),
				on_connect: true,
				remember_password: false,
			}
			.usable()
		};
		assert!(usable("root"));
		assert!(usable("deploy-2"));
		assert!(!usable("root; id"));
		assert!(!usable("-froot"));
		assert!(!usable(""));
	}

	/// A `targets.json` written before §47 loads with no elevation and behaves as it did, and one
	/// written with the default preference stays out of the file entirely.
	#[test]
	fn an_older_store_loads_with_no_elevation_and_a_tidy_one_stays_tidy() {
		let text = r#"{"version":"2","targets":[{"name":"rec","host":"rec","port":22,"user":"cme","auth_kind":"password"}]}"#;
		let loaded = Targets::parse(text, Path::new("targets.json"));
		assert_eq!(loaded.items().len(), 1);
		assert!(loaded.items()[0].elevate.is_none());

		// The `sudo` / `su` words are the ones the user picked, spelled as they are in the dialog.
		let mut targets = loaded;
		targets.set_elevation("cme@rec:22", "root", ElevateKind::Sudo, true);
		let json = serde_json::to_string(&targets.items()[0]).expect("serialize");
		assert!(json.contains(r#""kind":"sudo""#), "{json}");
		assert!(json.contains(r#""account":"root""#), "{json}");
		assert!(json.contains(r#""on_connect":true"#), "{json}");
		// The password flag is false, so it is not in the file at all.
		assert!(!json.contains("remember_password"), "{json}");
	}

	// A password target with the given name/endpoint parts.
	fn pw_target(name: &str, user: &str, host: &str, port: u16) -> Target {
		Target {
			name: name.to_string(),
			host: host.to_string(),
			port,
			user: user.to_string(),
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

	// A snapshot carrying just the two paths, the common capture.
	fn paths(terminal: &str, files: &str) -> LeftOff {
		LeftOff {
			terminal_path: Some(terminal.to_owned()),
			files_path: Some(files.to_owned()),
			..LeftOff::default()
		}
	}

	#[test]
	fn endpoint_is_user_at_host_port() {
		let target = pw_target("prod", "root", "example.com", 2222);
		assert_eq!(target.endpoint(), "root@example.com:2222");
	}

	#[test]
	fn upsert_adds_a_new_target_named_after_the_endpoint() {
		// Arrange
		let mut targets = Targets::default();

		// Act
		let key =
			targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None, None);

		// Assert
		assert_eq!(key, "root@example.com:22");
		assert_eq!(targets.items().len(), 1);
		assert_eq!(targets.items()[0].name, "root@example.com:22");
	}

	#[test]
	fn upsert_same_endpoint_updates_auth_but_keeps_the_name() {
		// Arrange: a renamed password target.
		let mut targets = Targets::default();
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None, None);
		targets.rename("root@example.com:22", "prod");

		// Act: reconnect to the same endpoint, this time with a key.
		let path = Some(PathBuf::from("/keys/id_ed25519"));
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Key, path.clone(), None);

		// Assert: still one target, name preserved, auth refreshed.
		assert_eq!(targets.items().len(), 1);
		let target = &targets.items()[0];
		assert_eq!(target.name, "prod");
		assert_eq!(target.auth_kind, AuthKind::Key);
		assert_eq!(target.key_path, path);
	}

	#[test]
	fn upsert_refreshes_and_clears_the_certificate() {
		// A reconnect that adds a certificate records it; a later reconnect without one clears it
		// — the certificate tracks the last successful connect, just like the key path (§14).
		let mut targets = Targets::default();
		let key = Some(PathBuf::from("/keys/id_ed25519"));
		let cert = Some(PathBuf::from("/keys/id_ed25519-cert.pub"));
		targets.upsert_on_connect("h", 22, "u", AuthKind::Key, key.clone(), cert.clone());
		assert_eq!(targets.find("u@h:22").unwrap().cert_path, cert);

		// Reconnect with the same key but no certificate: the stored certificate is dropped.
		targets.upsert_on_connect("h", 22, "u", AuthKind::Key, key, None);
		assert_eq!(targets.find("u@h:22").unwrap().cert_path, None);
	}

	#[test]
	fn items_are_sorted_case_insensitively_by_name() {
		// Arrange
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 22, "zoe", AuthKind::Password, None, None); // endpoint "zoe@h:22"
		targets.upsert_on_connect("h", 22, "amy", AuthKind::Password, None, None); // endpoint "amy@h:22"
		targets.rename("zoe@h:22", "Alpha");
		targets.rename("amy@h:22", "beta");

		// Act / Assert: "Alpha" sorts before "beta" despite the capital A.
		let names: Vec<&str> = targets.items().iter().map(|t| t.name.as_str()).collect();
		assert_eq!(names, vec!["Alpha", "beta"]);
	}

	#[test]
	fn rename_reorders_and_rejects_blank() {
		// Arrange: two targets, "aaa" then "zzz".
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None); // u@h:1
		targets.upsert_on_connect("h", 2, "u", AuthKind::Password, None, None); // u@h:2
		targets.rename("u@h:1", "aaa");
		targets.rename("u@h:2", "zzz");
		assert_eq!(targets.items()[0].name, "aaa");

		// Act: rename "aaa" to "zebra" — it should move after "zzz"? No: "zebra" < "zzz".
		assert!(targets.rename("u@h:1", "zebra"));
		let names: Vec<&str> = targets.items().iter().map(|t| t.name.as_str()).collect();
		assert_eq!(names, vec!["zebra", "zzz"]);

		// A blank rename is rejected and changes nothing.
		assert!(!targets.rename("u@h:1", "   "));
	}

	#[test]
	fn remove_drops_the_matching_target() {
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None);
		assert!(targets.remove("u@h:1"));
		assert!(targets.items().is_empty());
		assert!(!targets.remove("u@h:1")); // already gone
	}

	#[test]
	fn save_then_load_round_trips_through_a_file() {
		// Arrange
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None, None);
		targets.upsert_on_connect(
			"box",
			2222,
			"me",
			AuthKind::Key,
			Some(PathBuf::from("/keys/id")),
			Some(PathBuf::from("/keys/id-cert.pub")),
		);
		targets.rename("root@example.com:22", "prod");

		// Act
		targets.save_to(&path).expect("save");
		let loaded = Targets::load_from(&path);

		// Assert
		assert_eq!(loaded.items(), targets.items());
	}

	#[test]
	fn the_resume_paths_are_remembered_and_never_wiped_by_a_silent_session() {
		// Arrange: one target, no session behind it yet.
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None);

		// A session ends knowing both where the shell and the pane were.
		assert!(targets.set_session("u@h:1", paths("/var/log", "/etc")));
		let target = targets.find("u@h:1").unwrap();
		assert_eq!(target.terminal_path.as_deref(), Some("/var/log"));
		assert_eq!(target.files_path.as_deref(), Some("/etc"));

		// A later session whose shell never announced a cwd (`None`) moves only the pane —
		// the known-good terminal path must survive rather than be cleared.
		let only_pane = LeftOff {
			files_path: Some("/tmp".to_owned()),
			..LeftOff::default()
		};
		assert!(targets.set_session("u@h:1", only_pane.clone()));
		let target = targets.find("u@h:1").unwrap();
		assert_eq!(
			target.terminal_path.as_deref(),
			Some("/var/log"),
			"kept, not wiped"
		);
		assert_eq!(target.files_path.as_deref(), Some("/tmp"));

		// Setting the same values again reports "nothing changed" so the caller skips the
		// write, and an unknown endpoint is simply ignored.
		assert!(!targets.set_session("u@h:1", only_pane));
		assert!(!targets.set_session("nobody@nowhere:22", paths("/", "/")));
	}

	#[test]
	fn a_targets_file_without_the_session_fields_round_trips() {
		// A store written before the resume fields existed must load (all absent) and keep
		// working — the round trip is what proves the serde defaults hold.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None, None);
		targets.set_session(
			"root@example.com:22",
			LeftOff {
				terminal_path: Some("/srv".to_owned()),
				files_path: Some("/srv/www".to_owned()),
				explorer_width: Some(320.0),
				files_height: Some(260.0),
				..LeftOff::default()
			},
		);

		targets.save_to(&path).expect("save");
		let loaded = Targets::load_from(&path);
		assert_eq!(loaded.items(), targets.items());
		let target = &loaded.items()[0];
		assert_eq!(target.terminal_path.as_deref(), Some("/srv"));
		assert_eq!(target.files_path.as_deref(), Some("/srv/www"));
		assert_eq!(target.explorer_width, Some(320.0));
		assert_eq!(target.files_height, Some(260.0));
	}

	#[test]
	fn a_whole_snapshot_writes_and_reads_back_through_session() {
		// The full snapshot round-trips through `set_session` and back out of `session`.
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None);
		targets.set_session(
			"u@h:1",
			LeftOff {
				terminal_path: Some("/opt".to_owned()),
				files_path: Some("/opt/app".to_owned()),
				show_hidden: Some(false),
				explorer_width: Some(300.0),
				files_height: Some(240.0),
				sort: Change::Set(SortKey::Size),
				sort_dir: Change::Set(SortDir::Descending),
			},
		);
		let session = targets.find("u@h:1").unwrap().session();
		assert_eq!(session.terminal_path.as_deref(), Some("/opt"));
		assert_eq!(session.files_path.as_deref(), Some("/opt/app"));
		assert_eq!(session.show_hidden, Some(false));
		assert_eq!(session.explorer_width, Some(300.0));
		assert_eq!(session.files_height, Some(240.0));
		assert_eq!(session.sort, Change::Set(SortKey::Size));
		assert_eq!(session.sort_dir, Change::Set(SortDir::Descending));

		// A snapshot that says nothing about anything changes nothing and skips the write.
		assert!(!targets.set_session("u@h:1", LeftOff::default()));
	}

	#[test]
	fn the_hidden_toggle_is_remembered_per_target() {
		// Arrange: two targets, both starting from the default (shown).
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None);
		targets.upsert_on_connect("h", 2, "u", AuthKind::Password, None, None);

		// Act: hide dotfiles on the first one only, through the one snapshot setter.
		let hide = LeftOff {
			show_hidden: Some(false),
			..LeftOff::default()
		};
		assert!(targets.set_session("u@h:1", hide.clone()));

		// Assert: it stuck, its neighbour is untouched, and setting the same value again
		// reports "nothing changed" so the caller skips the write.
		assert!(!targets.find("u@h:1").unwrap().show_hidden);
		assert!(targets.find("u@h:2").unwrap().show_hidden);
		assert!(!targets.set_session("u@h:1", hide));
	}

	#[test]
	fn the_sort_is_remembered_per_target_and_round_trips() {
		// Arrange: two targets, both starting unsorted.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None, None);
		targets.upsert_on_connect("h", 2, "u", AuthKind::Password, None, None);
		assert_eq!(targets.find("u@h:1").unwrap().sort, None);
		assert_eq!(targets.find("u@h:1").unwrap().sort_dir, None);

		// Act: sort the first one by size, descending, through the one snapshot setter.
		let by_size_desc = LeftOff {
			sort: Change::Set(SortKey::Size),
			sort_dir: Change::Set(SortDir::Descending),
			..LeftOff::default()
		};
		assert!(targets.set_session("u@h:1", by_size_desc.clone()));

		// Assert: it stuck, its neighbour is untouched, and the same values again report "nothing
		// changed" so the caller skips the write.
		assert_eq!(targets.find("u@h:1").unwrap().sort, Some(SortKey::Size));
		assert_eq!(
			targets.find("u@h:1").unwrap().sort_dir,
			Some(SortDir::Descending)
		);
		assert_eq!(targets.find("u@h:2").unwrap().sort, None);
		assert!(!targets.set_session("u@h:1", by_size_desc));

		// Clearing the sort back to the default order is a real change (`Change::Clear`), not "leave
		// it alone" (`Change::Keep`): a session that cleared its sort must be remembered as cleared.
		let cleared = LeftOff {
			sort: Change::Clear,
			sort_dir: Change::Clear,
			..LeftOff::default()
		};
		assert!(targets.set_session("u@h:1", cleared));
		assert_eq!(targets.find("u@h:1").unwrap().sort, None);
		assert_eq!(targets.find("u@h:1").unwrap().sort_dir, None);

		// A key with the direction left unset round-trips through a save/load: the grid reopens
		// sorted by that key, ascending (an unset direction sorts ascending in the pane).
		targets.set_session(
			"u@h:1",
			LeftOff {
				sort: Change::Set(SortKey::Extension),
				..LeftOff::default()
			},
		);
		targets.save_to(&path).expect("save");
		let loaded = Targets::load_from(&path);
		let restored = loaded.find("u@h:1").unwrap();
		assert_eq!(restored.sort, Some(SortKey::Extension));
		assert_eq!(restored.sort_dir, None);
	}

	#[test]
	fn a_targets_file_without_the_sort_fields_defaults_to_unset() {
		// A store written before the sort was remembered must load with none and behave as before.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(
			&path,
			r#"[{"name":"prod","host":"h","port":22,"user":"u","auth_kind":"password"}]"#,
		)
		.unwrap();
		let loaded = Targets::load_from(&path);
		let target = &loaded.items()[0];
		assert_eq!(target.sort, None);
		assert_eq!(target.sort_dir, None);
	}

	#[test]
	fn a_targets_file_without_the_hidden_field_still_shows_dotfiles() {
		// A store written before the preference existed must keep its old behaviour.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(
			&path,
			r#"[{"name":"prod","host":"h","port":22,"user":"u","auth_kind":"password"}]"#,
		)
		.unwrap();
		assert!(Targets::load_from(&path).items()[0].show_hidden);
	}

	#[test]
	fn the_remember_flag_is_set_cleared_and_round_trips() {
		// Arrange: a target with no secret stored yet.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		assert!(!targets.find("u@h:22").unwrap().remember_secret);

		// Act / Assert: setting it reports a change and sticks; setting the same value again
		// reports none; an unknown endpoint is a no-op.
		assert!(targets.set_remembered("u@h:22", true));
		assert!(targets.find("u@h:22").unwrap().remember_secret);
		assert!(!targets.set_remembered("u@h:22", true));
		assert!(!targets.set_remembered("nobody@nowhere:22", true));

		// It survives a save/load round trip (the flag is metadata, no secret is written here).
		targets.save_to(&path).expect("save");
		assert!(
			Targets::load_from(&path)
				.find("u@h:22")
				.unwrap()
				.remember_secret
		);

		// Clearing it drops it back to the tidy default (skipped from the JSON).
		assert!(targets.set_remembered("u@h:22", false));
		targets.save_to(&path).expect("save");
		assert!(
			!Targets::load_from(&path)
				.find("u@h:22")
				.unwrap()
				.remember_secret
		);
	}

	#[test]
	fn forwards_are_set_and_round_trip_but_default_empty() {
		use crate::forward::{ForwardKind, ForwardSpec};

		// Arrange: a target with no forwards yet.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 22, "u", AuthKind::Password, None, None);
		assert!(targets.find("u@h:22").unwrap().forwards.is_empty());

		// Act / Assert: setting them reports a change and sticks; setting the same set again
		// reports none; an unknown endpoint is a no-op.
		let specs = vec![
			ForwardSpec::parse(ForwardKind::Local, "8080", "db:5432").unwrap(),
			ForwardSpec::parse(ForwardKind::Dynamic, "1080", "").unwrap(),
		];
		assert!(targets.set_forwards("u@h:22", specs.clone()));
		assert_eq!(targets.find("u@h:22").unwrap().forwards, specs);
		assert!(!targets.set_forwards("u@h:22", specs.clone()));
		assert!(!targets.set_forwards("nobody@nowhere:22", specs.clone()));

		// They survive a save/load round trip.
		targets.save_to(&path).expect("save");
		assert_eq!(
			Targets::load_from(&path).find("u@h:22").unwrap().forwards,
			specs
		);
	}

	#[test]
	fn a_targets_file_without_the_cert_field_defaults_to_none() {
		// A store written before certificates were remembered must load with none and behave as
		// before — a key target simply presents no certificate.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(
			&path,
			r#"[{"name":"prod","host":"h","port":22,"user":"u","auth_kind":"key","key_path":"/keys/id"}]"#,
		)
		.unwrap();
		assert_eq!(Targets::load_from(&path).items()[0].cert_path, None);
	}

	#[test]
	fn a_targets_file_without_the_forwards_field_defaults_to_empty() {
		// A store written before forwards existed must load with none and behave as before.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(
			&path,
			r#"[{"name":"prod","host":"h","port":22,"user":"u","auth_kind":"password"}]"#,
		)
		.unwrap();
		assert!(Targets::load_from(&path).items()[0].forwards.is_empty());
	}

	#[test]
	fn a_targets_file_without_the_remember_field_defaults_to_off() {
		// A store written before opt-in persistence existed must load with no secret promised.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(
			&path,
			r#"[{"name":"prod","host":"h","port":22,"user":"u","auth_kind":"password"}]"#,
		)
		.unwrap();
		assert!(!Targets::load_from(&path).items()[0].remember_secret);
	}

	#[test]
	fn load_missing_file_is_empty() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("does-not-exist.json");
		assert!(Targets::load_from(&path).items().is_empty());
	}

	#[test]
	fn load_corrupt_file_is_empty_not_a_panic() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(&path, "{ this is not json").unwrap();
		assert!(Targets::load_from(&path).items().is_empty());
	}

	// ---- §110: the format version, the migration, and the refusal ----

	/// A real version-1 file: the bare array every cmote up to §110 wrote. Kept as a literal rather
	/// than a checked-in fixture so the shape being migrated FROM is readable beside the test that
	/// migrates it.
	const VERSION_1: &str = r#"[
  {
    "name": "bastion",
    "host": "10.0.0.9",
    "port": 22,
    "user": "tester",
    "auth_kind": "password",
    "show_hidden": true
  }
]"#;

	#[test]
	fn an_unversioned_array_still_loads() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(&path, VERSION_1).unwrap();
		let targets = Targets::load_from(&path);
		assert_eq!(targets.items().len(), 1);
		assert_eq!(targets.items()[0].name, "bastion");
		assert!(targets.refusal().is_none());
		assert!(
			targets.migrated,
			"a version-1 file must be marked for migration"
		);
	}

	#[test]
	fn the_first_save_after_a_migration_keeps_the_original_beside_it() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(&path, VERSION_1).unwrap();
		let targets = Targets::load_from(&path);
		targets.save_to(&path).unwrap();

		// The store is now the envelope, and says which shape it is.
		let written = std::fs::read_to_string(&path).unwrap();
		assert!(
			written.starts_with('{'),
			"the migrated store is an object: {written}"
		);
		assert!(written.contains(r#""version": "2""#), "{written}");
		// The original is preserved verbatim — this is the only copy of it that will ever exist.
		let backup = std::fs::read_to_string(dir.path().join("targets.json.bak")).unwrap();
		assert_eq!(backup, VERSION_1);
	}

	#[test]
	fn a_version_2_store_round_trips() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		let mut targets = Targets::default();
		targets.upsert_on_connect("10.0.0.9", 22, "tester", AuthKind::Password, None, None);
		targets.save_to(&path).unwrap();

		let reloaded = Targets::load_from(&path);
		assert_eq!(reloaded.items().len(), 1);
		assert!(reloaded.refusal().is_none());
		assert!(
			!reloaded.migrated,
			"an envelope this build wrote needs no migration, so no backup"
		);
		assert!(!dir.path().join("targets.json.bak").exists());
	}

	#[test]
	fn a_newer_version_is_refused_and_never_written_over() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		// What a newer cmote might write: a version this build does not know, carrying a field it
		// cannot see. Both must survive.
		let newer = r#"{"version": "3", "targets": [], "tunnels_by_default": true}"#;
		std::fs::write(&path, newer).unwrap();

		let targets = Targets::load_from(&path);
		assert!(
			targets.items().is_empty(),
			"nothing may be loaded from a shape we cannot read"
		);
		let refusal = targets
			.refusal()
			.expect("a refusal must be reported, not logged and dropped");
		assert!(refusal.contains("version 3"), "{refusal}");
		assert!(refusal.contains("version 2"), "{refusal}");

		// And the save side refuses too, which is the half that protects the data.
		assert!(targets.save_to(&path).is_err());
		assert_eq!(
			std::fs::read_to_string(&path).unwrap(),
			newer,
			"the file must be byte-identical after a refused save"
		);
		assert!(
			!dir.path().join("targets.json.bak").exists(),
			"a refusal is not a migration, so nothing is backed up"
		);
	}

	#[test]
	fn a_file_that_is_neither_shape_is_treated_as_empty() {
		// The pre-existing rule (§14) survives the envelope: a corrupt store must never stop the
		// user from connecting, so it reads as "no targets" rather than as an error.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("targets.json");
		std::fs::write(&path, "this is not JSON at all").unwrap();
		let targets = Targets::load_from(&path);
		assert!(targets.items().is_empty());
		assert!(
			targets.refusal().is_none(),
			"garbage is not a version refusal — it must stay overwritable"
		);
	}
}

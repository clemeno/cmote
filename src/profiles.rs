// profiles.rs — the saved connection targets (PLAN §14, v1.3).
//
// The home screen lists targets the user has connected to before, so a return
// visit is one click instead of re-typing host / port / user / auth. What we
// persist is deliberately narrow:
//
//   PROFILE METADATA ONLY — name, host, port, user, auth kind, key-file path.
//   NEVER a secret. No password, no key passphrase (§12). The user still enters
//   the secret at connect time; only the "how to reach it" part is remembered.
//
// That keeps the §12 guarantee ("the safest secret is the one never persisted")
// intact while still making reconnecting convenient, and it keeps the store fully
// portable — a `targets.json` copied to another machine leaks nothing. (Opt-in
// secret persistence, encrypted at rest, is a separate later investigation.)
//
// The file is `targets.json` in the shared data directory (§11, `paths::data_dir`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ui::connect::AuthKind;

/// One saved connection target — profile metadata only, no secret material (§12).
/// `name` is a free display label (defaults to the endpoint, renamed by the user);
/// the rest is exactly what the connect form needs to be pre-filled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

impl Target {
	/// The endpoint string `user@host:port`. This doubles as the target's stable
	/// identity: the store keeps one target per endpoint (see `upsert_on_connect`),
	/// so this is what the home screen tracks a selection / rename by, and it never
	/// changes under a rename (which only touches `name`).
	pub fn endpoint(&self) -> String {
		endpoint_of(&self.user, &self.host, self.port)
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
}

impl Targets {
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
	/// endpoint, refresh its auth kind / key path but KEEP its custom name; otherwise
	/// add a new one named after the endpoint. Returns the endpoint key so the caller
	/// can select the row that was just saved. Re-sorts so the list stays ordered.
	pub fn upsert_on_connect(
		&mut self,
		host: &str,
		port: u16,
		user: &str,
		auth_kind: AuthKind,
		key_path: Option<PathBuf>,
	) -> String {
		let endpoint = endpoint_of(user, host, port);
		match self.items.iter_mut().find(|t| t.endpoint() == endpoint) {
			Some(existing) => {
				// Endpoint already known: update how we authenticate, leave the name alone.
				existing.auth_kind = auth_kind;
				existing.key_path = key_path;
			}
			None => {
				self.items.push(Target {
					name: endpoint.clone(),
					host: host.to_string(),
					port,
					user: user.to_string(),
					auth_kind,
					key_path,
				});
			}
		}
		self.sort();
		endpoint
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
			Ok(text) => match serde_json::from_str::<Vec<Target>>(&text) {
				Ok(items) => Self { items },
				Err(error) => {
					eprintln!(
						"ignoring unreadable targets file {}: {error}",
						path.display()
					);
					Self::default()
				}
			},
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
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent).context("failed to create the data directory")?;
		}
		let json = serde_json::to_string_pretty(&self.items).context("failed to encode targets")?;
		std::fs::write(path, json).context("failed to write the targets file")
	}

	/// Load from the resolved store location (§11). Convenience over `load_from`.
	pub fn load() -> Self {
		match profiles_path() {
			Ok(path) => Self::load_from(&path),
			Err(error) => {
				eprintln!("could not resolve the targets path: {error:#}");
				Self::default()
			}
		}
	}

	/// Save to the resolved store location (§11). Convenience over `save_to`.
	pub fn save(&self) -> Result<()> {
		self.save_to(&profiles_path()?)
	}
}

/// The path of the saved-targets file: `targets.json` in the shared data directory.
fn profiles_path() -> Result<PathBuf> {
	Ok(crate::paths::data_dir()?.join("targets.json"))
}

#[cfg(test)]
mod tests {
	use super::*;

	// A password target with the given name/endpoint parts.
	fn pw_target(name: &str, user: &str, host: &str, port: u16) -> Target {
		Target {
			name: name.to_string(),
			host: host.to_string(),
			port,
			user: user.to_string(),
			auth_kind: AuthKind::Password,
			key_path: None,
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
		let key = targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None);

		// Assert
		assert_eq!(key, "root@example.com:22");
		assert_eq!(targets.items().len(), 1);
		assert_eq!(targets.items()[0].name, "root@example.com:22");
	}

	#[test]
	fn upsert_same_endpoint_updates_auth_but_keeps_the_name() {
		// Arrange: a renamed password target.
		let mut targets = Targets::default();
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None);
		targets.rename("root@example.com:22", "prod");

		// Act: reconnect to the same endpoint, this time with a key.
		let path = Some(PathBuf::from("/keys/id_ed25519"));
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Key, path.clone());

		// Assert: still one target, name preserved, auth refreshed.
		assert_eq!(targets.items().len(), 1);
		let target = &targets.items()[0];
		assert_eq!(target.name, "prod");
		assert_eq!(target.auth_kind, AuthKind::Key);
		assert_eq!(target.key_path, path);
	}

	#[test]
	fn items_are_sorted_case_insensitively_by_name() {
		// Arrange
		let mut targets = Targets::default();
		targets.upsert_on_connect("h", 22, "zoe", AuthKind::Password, None); // endpoint "zoe@h:22"
		targets.upsert_on_connect("h", 22, "amy", AuthKind::Password, None); // endpoint "amy@h:22"
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
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None); // u@h:1
		targets.upsert_on_connect("h", 2, "u", AuthKind::Password, None); // u@h:2
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
		targets.upsert_on_connect("h", 1, "u", AuthKind::Password, None);
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
		targets.upsert_on_connect("example.com", 22, "root", AuthKind::Password, None);
		targets.upsert_on_connect(
			"box",
			2222,
			"me",
			AuthKind::Key,
			Some(PathBuf::from("/keys/id")),
		);
		targets.rename("root@example.com:22", "prod");

		// Act
		targets.save_to(&path).expect("save");
		let loaded = Targets::load_from(&path);

		// Assert
		assert_eq!(loaded.items(), targets.items());
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
}

// settings.rs — the app-wide layout cmote remembers between runs (PLAN §31).
//
// Almost everything cmote persists is PER-TARGET (§14, §22): where the shell and the two
// panes were for THAT server. What lives here instead is app-wide — a preference that is the
// same whatever connection is on show. The window itself is one such: there is one OS window,
// shown on the home screen before any target is chosen, so its size is an app-wide preference,
// not a property of any one connection. The per-extension editor theme (§32) is another: "CME
// for `.rs`" is a preference about a file type, the same on every server. Both sit in
// `settings.json` beside `targets.json` in the shared data directory (§11).
//
// The rule that shapes it (borrowed from a sister iced app): a settings file must never be
// able to stop the app from starting. Absent, empty, truncated, wrong types, hand-edited
// nonsense — every one of them reads as "no preference remembered" and a line on stderr,
// never an error the caller has to handle. So neither `load` nor `save` returns a `Result`:
// there is nothing the caller could usefully do with one.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::editor::EditorTheme;
use crate::paths;

/// The settings file's name inside the data directory (§11).
const FILE: &str = "settings.json";

/// The bounds a remembered window size is held to. The floor keeps a hand-edited tiny value
/// from opening an unusable sliver of a window — and, since §48 lets the app resize its own
/// window when a split closes, it is public so that path can clamp to the same floor instead
/// of shrinking to a size this file would then refuse to remember.
///
/// The ceiling is not cosmetic: it is a hard renderer limit. wgpu guarantees a maximum
/// texture dimension of only 8192 PHYSICAL pixels, and a surface is measured in physical
/// pixels, so a 2× (HiDPI) display doubles whatever is asked for here. 4096 LOGICAL points
/// leaves that margin and is already larger than any real display — so a stored size, however
/// it got there, can never crash the renderer at launch inside `Surface::configure`.
pub const MIN_WINDOW: f32 = 480.0;
const MAX_WINDOW: f32 = 4096.0;

/// What survives a restart, app-wide: the OS window size (§31) and the per-extension editor theme
/// (§32). The per-target pane sizes and resume paths live in `targets.json` (§22) instead, because
/// they belong to a connection, not to the app. `#[serde(default)]` fills in anything an older or
/// hand-edited file is missing, so adding a field here later can never invalidate an existing file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
	/// Which format this file is (§110). Version 1 is the UNVERSIONED object every cmote up to §110
	/// wrote, which is why the serde default here is `"1"` and NOT `Default`: a file with no version
	/// key is a version-1 file, while a `Settings` this build creates from nothing is current. Those
	/// are two different answers to "what version is this", so the struct's `Default` is written out
	/// by hand below rather than derived.
	///
	/// Without this, an older cmote reading a newer file would deserialize what it understands,
	/// ignore the rest, and then write its own shape back — dropping whatever the newer build had
	/// stored. The `#[serde(default)]` tolerance above protects a file from a MISSING field; nothing
	/// protected it from an unknown one.
	#[serde(default = "version_one")]
	version: String,

	/// Why this file was not read, when it was not (§110): it declares a version this build does not
	/// know. Never serialized — it describes the file, not the settings. Set means `save` must do
	/// nothing, because writing this shape over a newer one would drop what it holds.
	#[serde(skip)]
	refusal: Option<String>,

	/// Whether what was loaded came from the unversioned shape, so the first save preserves the
	/// original as `settings.json.bak` before replacing it (§110).
	#[serde(skip)]
	migrated: bool,

	/// The OS window's logical size as `(width, height)`, or `None` on a first run — the app
	/// then opens at its built-in default (a full-width terminal plus the browser strip). NOT
	/// the window POSITION: a window restored onto a monitor that has since been unplugged is
	/// worse than a centred one, so only the size is kept. Omitted from the JSON while `None`,
	/// so a first-run file stays empty (`{}`) rather than carrying a null.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub window: Option<(f32, f32)>,

	/// The colour scheme last chosen for each file extension in the in-tab editor (§32), keyed by
	/// `editor::extension_key` (lower-cased, no dot; "" for no extension). This is app-wide, not
	/// per-target: "CME for `.rs`" is a preference about a file TYPE, the same wherever the file
	/// lives, so it belongs here and not in `targets.json`. Empty until a type is themed, and
	/// skipped from the JSON while empty so a first-run file stays `{}`. `#[serde(default)]` fills
	/// it in for an older file written before the field existed.
	#[serde(default, skip_serializing_if = "HashMap::is_empty")]
	pub editor_theme_by_ext: HashMap<String, EditorTheme>,
}

/// The format version this build writes (§110). A string, for the same reason `targets.json`'s is.
const FORMAT: &str = "2";

/// Serde's default for a missing `version` key: the unversioned shape this file used to have.
fn version_one() -> String {
	"1".to_owned()
}

impl Default for Settings {
	/// Written out rather than derived, because `version` must default to the CURRENT format here
	/// while defaulting to `"1"` when serde fills in a missing key. A derived `Default` would give
	/// the empty string and this build would write `"version": ""`.
	fn default() -> Self {
		Self {
			version: FORMAT.to_owned(),
			refusal: None,
			migrated: false,
			window: None,
			editor_theme_by_ext: HashMap::new(),
		}
	}
}

impl Settings {
	/// Read the settings file, or return defaults. A missing file is the normal first-run
	/// case and not worth a line; anything else that goes wrong is logged and treated as
	/// defaults, so a corrupt file never stops the app from opening.
	pub fn load() -> Self {
		let path = match paths::data_dir() {
			Ok(dir) => dir.join(FILE),
			Err(error) => {
				eprintln!("cmote: cannot resolve the settings path: {error:#}");
				return Self::default();
			}
		};
		match std::fs::read_to_string(&path) {
			Ok(text) => Self::from_json(&text),
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
			Err(error) => {
				eprintln!("cmote: cannot read {}: {error}", path.display());
				Self::default()
			}
		}
	}

	/// Remember the OS window's size, clamped to the sane range. Returns whether anything
	/// changed, so the caller can skip a needless write. A degenerate size — a minimize can
	/// report 0 × 0, and a non-finite value should never be trusted — is ignored so the last
	/// good size survives it; a size over the ceiling is clamped rather than dropped.
	pub fn set_window(&mut self, width: f32, height: f32) -> bool {
		if !(width.is_finite() && height.is_finite()) || width < MIN_WINDOW || height < MIN_WINDOW {
			return false;
		}
		let next = Some((width.min(MAX_WINDOW), height.min(MAX_WINDOW)));
		let changed = self.window != next;
		self.window = next;
		changed
	}

	/// The remembered window size as an iced `Size`, or `None` on a first run — `run` uses it
	/// for the opening window and falls back to the built-in default when it is absent (§31).
	pub fn window_size(&self) -> Option<iced::Size> {
		self.window
			.map(|(width, height)| iced::Size::new(width, height))
	}

	/// The scheme a file with this extension key should open in (§32), or the default for a type
	/// no one has themed yet. Keeps the read side in one place, the same way `window_size` does.
	pub fn editor_theme(&self, ext: &str) -> EditorTheme {
		self.editor_theme_by_ext
			.get(ext)
			.copied()
			.unwrap_or_default()
	}

	/// Remember the scheme chosen for an extension key, reporting whether it actually changed so a
	/// no-op re-select of the current scheme is not counted as a change. Persisted with the rest of
	/// the layout on the way out (§31) — no separate write, since a theme pick is rare and
	/// deliberate, not the per-frame churn a window resize is.
	pub fn set_editor_theme(&mut self, ext: String, theme: EditorTheme) -> bool {
		if self.editor_theme_by_ext.get(&ext) == Some(&theme) {
			return false;
		}
		self.editor_theme_by_ext.insert(ext, theme);
		true
	}

	/// Write the settings file, reporting a failure to stderr and carrying on. Creates the
	/// data directory if it is not there yet.
	///
	/// `ponytail:` a plain write, not write-to-temp-then-rename. Losing a window size to a
	/// crash mid-write is not worth an atomic-replace dance — the next run reads the default,
	/// which is where a first run started anyway.
	pub fn save(&self) {
		// Nothing remembered yet (a session that exited before the window ever reported a size)
		// is nothing worth writing — and, crucially, writing an empty `{}` would clobber a good
		// settings file a previous run left. So a default-valued settings never touches the disk.
		if *self == Self::default() {
			return;
		}
		let dir = match paths::data_dir() {
			Ok(dir) => dir,
			Err(error) => {
				eprintln!("cmote: cannot resolve the settings path: {error:#}");
				return;
			}
		};
		if let Some(version) = &self.refusal {
			eprintln!(
				"cmote: not writing {FILE} — it is version {version} and this cmote is older"
			);
			return;
		}
		let text = match serde_json::to_string_pretty(self) {
			Ok(text) => text,
			Err(error) => {
				eprintln!("cmote: cannot encode settings: {error}");
				return;
			}
		};
		let path = dir.join(FILE);
		// Preserve the unversioned original before the first save that changes its shape, once
		// (`store::back_up_once`) — the same rule `targets.json` follows (§110).
		if self.migrated
			&& let Err(error) = crate::store::back_up_once(&path)
		{
			eprintln!("cmote: cannot back up {FILE}: {error:#}");
			return;
		}
		if let Err(error) = crate::store::write_atomically(&path, text.as_bytes()) {
			eprintln!("cmote: cannot write {FILE}: {error:#}");
		}
	}

	/// Parse, keeping only what makes sense. Split out from `load` so the whole "never let a
	/// bad file through" rule is testable without touching the filesystem.
	fn from_json(text: &str) -> Self {
		match serde_json::from_str::<Self>(text) {
			Ok(settings) if settings.version == FORMAT => settings.sanitized(),
			// The unversioned shape: read as before, and marked so the first save preserves it.
			Ok(settings) if settings.version == version_one() => Self {
				migrated: true,
				..settings.sanitized()
			},
			// Any other version was written by a cmote newer than this one. Nothing is taken from
			// it and nothing will be written over it. There is no user-facing surface for this the
			// way there is for a refused `targets.json`, and it needs none: this file holds the
			// window size and the editor themes, so a refusal costs a remembered window — while
			// overwriting it would cost whatever the newer build keeps here (§110).
			Ok(settings) => {
				eprintln!(
					"cmote: {FILE} is version {} — this cmote writes version {FORMAT}. Leaving it 					 alone; the window size will not be remembered this run.",
					settings.version
				);
				Self {
					refusal: Some(settings.version),
					..Self::default()
				}
			}
			Err(error) => {
				eprintln!("cmote: ignoring an unreadable settings file: {error}");
				Self::default()
			}
		}
	}

	/// Replace any value the UI could not have produced. The file is plain text a user can
	/// edit, so this is a trust boundary, not mere deserialization: a window size that is
	/// non-finite or outside the sane range is dropped whole (back to the first-run default)
	/// rather than half-trusted, which is the safest reading of nonsense.
	fn sanitized(mut self) -> Self {
		self.window = self.window.filter(|&(width, height)| {
			let ok = |value: f32| value.is_finite() && (MIN_WINDOW..=MAX_WINDOW).contains(&value);
			ok(width) && ok(height)
		});
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_first_run_has_no_remembered_window() {
		// The default is "no preference", so the app opens at its built-in size.
		assert_eq!(Settings::default().window, None);
		assert_eq!(Settings::default().window_size(), None);
	}

	#[test]
	fn set_window_clamps_and_reports_change() {
		let mut settings = Settings::default();
		// A sane size is remembered and reported as a change.
		assert!(settings.set_window(1200.0, 800.0));
		assert_eq!(settings.window, Some((1200.0, 800.0)));
		// The same size again is no change, so the caller skips the write.
		assert!(!settings.set_window(1200.0, 800.0));
		// Over the ceiling is clamped, not dropped.
		assert!(settings.set_window(9000.0, 9000.0));
		assert_eq!(settings.window, Some((MAX_WINDOW, MAX_WINDOW)));
	}

	#[test]
	fn a_degenerate_size_is_ignored_so_the_last_good_one_survives() {
		let mut settings = Settings::default();
		settings.set_window(1000.0, 700.0);
		// A minimize (0 × 0), a sliver, and a non-finite value all leave the last size intact.
		assert!(!settings.set_window(0.0, 0.0));
		assert!(!settings.set_window(100.0, 700.0));
		assert!(!settings.set_window(f32::NAN, 700.0));
		assert_eq!(settings.window, Some((1000.0, 700.0)));
	}

	#[test]
	fn an_empty_object_reads_as_version_one_defaults() {
		// `from_json` is the whole parse path. `{}` is what an older cmote wrote on a first run, so
		// it carries no version key — which makes it a version-1 file with default values, not a
		// current one (§110). The values match `default`; the format does not.
		let settings = Settings::from_json("{}");
		assert_eq!(settings.window, None);
		assert!(settings.editor_theme_by_ext.is_empty());
		assert_eq!(settings.version, "1");
		assert!(
			settings.migrated,
			"a file with no version key is one to migrate"
		);
		assert!(settings.refusal.is_none());
	}

	#[test]
	fn a_corrupt_file_reads_as_defaults_not_a_panic() {
		assert_eq!(
			Settings::from_json("{ this is not json"),
			Settings::default()
		);
	}

	#[test]
	fn an_out_of_range_stored_size_falls_back_to_the_default() {
		// A hand-edited file with a tiny, huge or non-finite size must not reach the renderer.
		assert_eq!(
			Settings::from_json(r#"{"window":[10.0,10.0]}"#).window,
			None
		);
		assert_eq!(
			Settings::from_json(r#"{"window":[99999.0,800.0]}"#).window,
			None
		);
		// A size inside the range survives the round trip.
		assert_eq!(
			Settings::from_json(r#"{"window":[1280.0,720.0]}"#).window,
			Some((1280.0, 720.0))
		);
	}

	#[test]
	fn a_remembered_editor_theme_survives_the_round_trip() {
		let mut settings = Settings::default();
		// An unseen type opens on the default, and picking a scheme reports a real change.
		assert_eq!(settings.editor_theme("rs"), EditorTheme::Default);
		assert!(settings.set_editor_theme("rs".into(), EditorTheme::Cme));
		// Re-picking the same scheme is no change, so the caller can tell it apart from a real pick.
		assert!(!settings.set_editor_theme("rs".into(), EditorTheme::Cme));
		// The choice reads back, and rides `settings.json` through a save/load round trip.
		assert_eq!(settings.editor_theme("rs"), EditorTheme::Cme);
		let json = serde_json::to_string(&settings).unwrap();
		assert_eq!(
			Settings::from_json(&json).editor_theme("rs"),
			EditorTheme::Cme
		);
	}

	#[test]
	fn no_remembered_theme_leaves_the_first_run_file_empty() {
		// The map is skipped from the JSON while empty, so a first run stays `{}` (below) — a themed
		// file, though, carries the map, so `save` is no longer a no-op once a scheme is picked.
		let mut settings = Settings::default();
		settings.set_editor_theme("php".into(), EditorTheme::Cme);
		assert_ne!(settings, Settings::default());
	}

	#[test]
	fn a_first_run_serializes_to_its_version_and_nothing_else() {
		// `window` and the theme map are both skipped while empty, so a first-run file carries only
		// the one thing it must always say: which format it is (§110). It used to be `{}`, which is
		// tidier and is exactly the problem — a file that does not say what it is cannot be told
		// apart from a newer one.
		let json = serde_json::to_string(&Settings::default()).unwrap();
		assert_eq!(json, r#"{"version":"2"}"#);
	}

	#[test]
	fn a_version_this_build_does_not_know_is_refused_and_not_written_back() {
		// What a newer cmote might leave behind: a version key this build has never heard of, plus a
		// setting it cannot see. Nothing is taken from it.
		let newer = r#"{"version": "3", "window": [1234.0, 900.0], "theme": "solarized"}"#;
		let settings = Settings::from_json(newer);
		assert_eq!(settings.refusal.as_deref(), Some("3"));
		assert_eq!(
			settings.window, None,
			"a refused file must not hand over even the values this build understands"
		);
		// `save` returns early on a refusal, so there is nothing to assert about the disk here —
		// the guard is the first statement in it, and this is the state that trips it.
		assert!(!settings.migrated, "a refusal is not a migration");
	}

	#[test]
	fn a_current_file_is_read_as_current() {
		let current = r#"{"version": "2", "window": [1000.0, 700.0]}"#;
		let settings = Settings::from_json(current);
		assert_eq!(settings.window, Some((1000.0, 700.0)));
		assert_eq!(settings.version, "2");
		assert!(
			!settings.migrated,
			"nothing to migrate, so nothing to back up"
		);
		assert!(settings.refusal.is_none());
	}
}

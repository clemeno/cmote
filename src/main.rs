// cmote — a portable Windows 11 SSH client written in Rust.
//
// `main.rs` is deliberately tiny: it only wires the module tree together and
// hands control to `app::run`. Keeping the entry point thin is a common Rust
// pattern — the binary crate is just a launcher around library-style modules.

// In a release build we hide the console window (this is a GUI app that renders
// its own terminal). In a debug build we KEEP the console so `eprintln!`/panics
// are visible while developing. `cfg_attr` applies the attribute conditionally.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// How close two pixel measurements have to be for a test to call them the same (§111).
///
/// A tenth of a pixel is not a layout difference — nothing on screen can express one — while an `f32`
/// carrying a few thousand pixels resolves to about a ten-thousandth, so this is four orders of
/// magnitude above the arithmetic's noise and four below anything a user could see.
#[cfg(test)]
const PIXEL_TOLERANCE: f32 = 0.1;

/// Assert that two pixel measurements agree (§111).
///
/// Layout numbers are `f32`, and the tests that check them compare a measurement against the
/// arithmetic that produced it — `1000.0 * MAX_PANE_FRACTION`, `(1000.0 - DIALOG_WIDTH) / 2.0`. Those
/// comparisons were `assert_eq!`, which asks for identical bits and got them, because both sides are
/// the same operations in the same order. That is luck rather than a property: reorder the
/// multiplication, or let one side arrive via a widget that rounds, and an equality that never had a
/// reason to hold starts failing for a difference no screen can show.
///
/// So the tests say what they mean instead. Declared here at the crate root, before the modules, so
/// every `mod` below sees it by textual scope without an import — seven test modules use it.
#[cfg(test)]
macro_rules! assert_px {
	($actual:expr, $expected:expr) => {
		assert_px!($actual, $expected, "")
	};
	($actual:expr, $expected:expr, $note:expr) => {{
		let (actual, expected): (f32, f32) = ($actual, $expected);
		assert!(
			(actual - expected).abs() < $crate::PIXEL_TOLERANCE,
			"{} px is not {} px (to within {} px) {}",
			actual,
			expected,
			$crate::PIXEL_TOLERANCE,
			$note
		);
	}};
}

// Module declarations. Each `mod` maps to a file (or a folder with `mod.rs`)
// under `src/`. See PLAN.md §5 for the responsibility of each module.
mod app; // iced application: State, Message, update, view, subscription
mod bridge; // channel message types that cross the GUI <-> tokio boundary (§4)
mod change; // a value that may be absent for two different reasons: nothing said, or said empty (§111)
mod cursor; // the open/closed hand over a draggable tab, drawn here because Windows has none (§51)
mod editor; // the in-tab text editor's model: encoding, changed-line diff, buffer state (§32)
mod elevate; // becoming another account on the machine we are already logged in to (§45)
mod explorer; // the remote folder tree's model: nodes, expansion, path arithmetic (§18)
mod files; // the remote file browser's model: one directory, batched listings (§19)
mod forward; // the pure port-forward spec: kind + bind/target, parse/validate/label (§27)
mod glob; // the home filter's text rule: a fragment, or a whole-text glob once * or ? is typed (§49)
mod human; // how a byte count is spelled for a person, once rather than twice (§17, §109)
mod integration; // the shell-integration block a remote's rc file can be given, so it announces its cwd (§17)
mod link; // opening an OSC 8 hyperlink safely: scheme policy + the OS browser launch (§24)
mod local; // a session on THIS machine: a local shell in the grid, local files in the panes (§103)
mod mru; // the tabs' activation order, so closing one falls back to the previous visit (§37)
mod palette; // the terminal colour scheme, shared by the renderer and the query answerer (§9, §23)
mod panes; // the tree and the pane as one pair, and the rules that span them (§18, §19, §22)
mod paths; // where on-disk data lives: known_hosts + saved targets (§11, §14)
mod preview; // the picture tab's model: which files are pictures, and the fenced decode (§53)
mod secret; // in-memory, zeroized, redacting wrapper for passwords/passphrases (§12)
mod settings; // app-wide layout remembered between runs: the window size (§31)
mod ssh; // SSH client, auth, host-key verification, key loading (§6-§8)
mod store; // how the on-disk files are written: atomic replace, one backup (§110)
mod targets; // saved connection targets, metadata only — no secrets (§14)
mod taskbar; // mirror the active tab's command progress onto the Windows taskbar button (§54)
mod term; // VT/ANSI terminal emulator wrapping the engine, behind a small surface (§9, §23)
mod transfer; // the one transfer slot and everything queued behind it (§16, §17, §19, §21, §29)
mod ui; // view helpers: the home list, the connect form and the terminal grid (§10)
mod vault; // opt-in, portable, master-passphrase-encrypted store for remembered secrets (§16)

// `main` returns `iced::Result` so any startup error propagates with a clean
// process exit code.
fn main() -> iced::Result {
	app::run()
}

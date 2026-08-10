// cmote — a portable Windows 11 SSH client written in Rust.
//
// `main.rs` is deliberately tiny: it only wires the module tree together and
// hands control to `app::run`. Keeping the entry point thin is a common Rust
// pattern — the binary crate is just a launcher around library-style modules.

// In a release build we hide the console window (this is a GUI app that renders
// its own terminal). In a debug build we KEEP the console so `eprintln!`/panics
// are visible while developing. `cfg_attr` applies the attribute conditionally.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Module declarations. Each `mod` maps to a file (or a folder with `mod.rs`)
// under `src/`. See PLAN.md §5 for the responsibility of each module.
mod app; // iced application: State, Message, update, view, subscription
mod bridge; // channel message types that cross the GUI <-> tokio boundary (§4)
mod cursor; // the open/closed hand over a draggable tab, drawn here because Windows has none (§51)
mod editor; // the in-tab text editor's model: encoding, changed-line diff, buffer state (§32)
mod elevate; // becoming another account on the machine we are already logged in to (§45)
mod explorer; // the remote folder tree's model: nodes, expansion, path arithmetic (§18)
mod files; // the remote file browser's model: one directory, batched listings (§19)
mod forward; // the pure port-forward spec: kind + bind/target, parse/validate/label (§27)
mod glob; // the home filter's text rule: a fragment, or a whole-text glob once * or ? is typed (§49)
mod link; // opening an OSC 8 hyperlink safely: scheme policy + the OS browser launch (§24)
mod mru; // the tabs' activation order, so closing one falls back to the previous visit (§37)
mod palette; // the terminal colour scheme, shared by the renderer and the query answerer (§9, §23)
mod paths; // where on-disk data lives: known_hosts + saved targets (§11, §14)
mod profiles; // saved connection targets, persisted as profiles only — no secrets (§14)
mod secret; // in-memory, zeroized, redacting wrapper for passwords/passphrases (§12)
mod settings; // app-wide layout remembered between runs: the window size (§31)
mod ssh; // SSH client, auth, host-key verification, key loading (§6-§8)
mod term; // VT/ANSI terminal emulator wrapping the engine, behind a small surface (§9, §23)
mod transfer; // the one transfer slot and everything queued behind it (§16, §17, §19, §21, §29)
mod ui; // view helpers: the home list, the connect form and the terminal grid (§10)
mod vault; // opt-in, portable, master-passphrase-encrypted store for remembered secrets (§16)

// `main` returns `iced::Result` so any startup error propagates with a clean
// process exit code.
fn main() -> iced::Result {
	app::run()
}

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
mod ssh; // SSH client, auth, host-key verification, key loading (§6-§8)
mod term; // VT/ANSI terminal emulator wrapper around vt100 (§9)
mod ui; // view helpers: the connect form and the terminal grid (§10)

// `main` returns `iced::Result` so any startup error propagates with a clean
// process exit code.
fn main() -> iced::Result {
	app::run()
}

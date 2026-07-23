// term/mod.rs — the VT/ANSI terminal emulator wrapper (PLAN §9).
//
// A remote shell speaks a byte stream peppered with escape sequences: "move the
// cursor", "set the color to red", "clear the line". `vt100::Parser` interprets
// that stream and maintains a `Screen` — a fixed grid of cells, each holding a
// glyph plus its colors and attributes, together with the cursor position. The
// UI never sees raw bytes; it reads the grid and draws it (see ui/terminal.rs).
//
// This wrapper exists so the rest of the app depends on a tiny, intention-named
// surface (`process`, `resize`, `screen`) instead of the parser's full API, and
// so the emulator can be swapped later without touching the GUI.

pub mod keymap; // maps GUI key events to the bytes a terminal sends

/// The pty size the client requests and the emulator starts at, before the first
/// window measurement arrives (§9). Kept here as the single source of truth so
/// the ssh client (which requests the initial pty) and the emulator (which lays
/// out the grid) can never disagree; the grid is then reflowed to the real window
/// size via `resize` + `SshCommand::Resize`.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How many scrolled-off lines to retain. v1 shows only the visible screen, so
/// none — scrollback is a later feature.
const SCROLLBACK: usize = 0;

/// The terminal emulator: a `vt100::Parser` plus the small API the app needs.
pub struct Terminal {
	parser: vt100::Parser,
}

impl Terminal {
	/// Create an emulator with a `rows`×`cols` grid, matching the remote pty.
	pub fn new(rows: u16, cols: u16) -> Self {
		Self {
			parser: vt100::Parser::new(rows, cols, SCROLLBACK),
		}
	}

	/// Feed a chunk of raw output from the shell. The parser applies every escape
	/// sequence and glyph in `bytes` to the grid; partial sequences split across
	/// chunks are buffered internally, so any chunk boundary is safe.
	pub fn process(&mut self, bytes: &[u8]) {
		self.parser.process(bytes);
	}

	/// Resize the grid when the window changes (§9). This only reflows our local
	/// view; the remote pty is told separately via `SshCommand::Resize`, so the
	/// two are kept in step by the caller (`app::on_window_resized`).
	pub fn resize(&mut self, rows: u16, cols: u16) {
		self.parser.screen_mut().set_size(rows, cols);
	}

	/// Borrow the current screen grid for rendering.
	pub fn screen(&self) -> &vt100::Screen {
		self.parser.screen()
	}
}

// `vt100::Parser` is not `Debug`, and `App` derives `Debug`; give a terse,
// content-free representation so nothing from the remote session leaks into logs.
impl std::fmt::Debug for Terminal {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let (rows, cols) = self.screen().size();
		write!(formatter, "Terminal({rows}x{cols})")
	}
}

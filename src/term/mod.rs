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

pub mod compat; // rewrites escape sequences the parser lacks into the ones it has (§9)
pub mod cwd; // tracks the remote working directory announced by the shell (§17)
pub mod keymap; // maps GUI key events to the bytes a terminal sends
pub mod mouse; // maps pointer events to the reports a mouse-aware program expects

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
	/// The remote working directory, learned from the OSC sequences the shell emits on
	/// each prompt (§17). vt100 ignores those codes, so the same bytes are scanned here
	/// before they reach the parser.
	cwd: cwd::Cwd,
	/// Rewrites the escape sequences vt100 has no arm for into the equivalent ones it does
	/// (§9) — without this a program that spells its cursor moves the other way, btop
	/// among them, has every move dropped and its output streams out as plain text.
	compat: compat::Aliases,
	/// The rewritten chunk, reused between calls so a busy full-screen program does not
	/// allocate a fresh buffer for every packet of output.
	rewritten: Vec<u8>,
}

impl Terminal {
	/// Create an emulator with a `rows`×`cols` grid, matching the remote pty.
	pub fn new(rows: u16, cols: u16) -> Self {
		Self {
			parser: vt100::Parser::new(rows, cols, SCROLLBACK),
			cwd: cwd::Cwd::default(),
			compat: compat::Aliases::default(),
			rewritten: Vec::new(),
		}
	}

	/// Feed a chunk of raw output from the shell. The parser applies every escape
	/// sequence and glyph in `bytes` to the grid; partial sequences split across
	/// chunks are buffered internally, so any chunk boundary is safe. The same bytes
	/// also go to the cwd tracker (§17), which both tolerate being handed the other's
	/// sequences — the tracker reads the stream as it came off the wire, before the
	/// alias rewrite (§9), which touches no OSC sequence anyway.
	pub fn process(&mut self, bytes: &[u8]) {
		self.cwd.feed(bytes);
		self.rewritten.clear();
		self.compat.rewrite(bytes, &mut self.rewritten);
		self.parser.process(&self.rewritten);
	}

	/// The remote shell's working directory, if it has announced one (§17). `None`
	/// until the first announcement — and on a shell that emits neither OSC 7 nor
	/// OSC 9;9, forever.
	pub fn cwd(&self) -> Option<&str> {
		self.cwd.path()
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_full_screen_program_lands_where_it_aims() {
		// The btop shape: position with the `f` spelling, write, position again. Before the
		// rewrite (§9) vt100 dropped both moves and the two words ran together on row 0,
		// which is what wrapped and scrolled the whole screen. Each must now land in its
		// own cell.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b[3;5fleft\x1b[7;20fright");
		let screen = terminal.screen();
		let read = |row: u16, col: u16, len: u16| -> String {
			(col..col + len)
				.filter_map(|col| screen.cell(row, col))
				.map(|cell| cell.contents())
				.collect()
		};
		assert_eq!(read(2, 4, 4), "left");
		assert_eq!(read(6, 19, 5), "right");
		// Nothing spilled onto the first row on the way.
		assert_eq!(read(0, 0, 10).trim(), "");
	}
}

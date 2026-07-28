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

pub mod answer; // answers the status/identity queries the parser leaves silent (§9)
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
	/// Finds the status/identity queries `vt100` has no arm for (DSR, DA) so `process` can
	/// answer them (§9) — a program that sent one blocks reading its stdin until the reply
	/// arrives, so leaving them silent stalls btop, vim, tmux and shell size-probes.
	answers: answer::Queries,
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
			answers: answer::Queries::default(),
			rewritten: Vec::new(),
		}
	}

	/// Feed a chunk of raw output from the shell, and return the bytes to send BACK to it
	/// as replies to any status/identity queries it carried (§9) — usually empty. The
	/// parser applies every escape sequence and glyph to the grid; partial sequences split
	/// across chunks are buffered internally, so any chunk boundary is safe. The same bytes
	/// also go to the cwd tracker (§17); both it and the query scanner tolerate being handed
	/// the other's sequences — the tracker reads the stream as it came off the wire, before
	/// the alias rewrite (§9), which touches no OSC sequence anyway.
	///
	/// Queries are answered by splitting the parser feed at each one: everything up to and
	/// including the query goes to the parser (a query is a no-op there), then the reply is
	/// built from whatever live state it needs. A cursor-position report must reflect the
	/// cursor WHERE THE QUERY SAT — reading it after the whole chunk would catch a later
	/// move (a save/jump/report/restore size-probe restores before the chunk ends), so the
	/// split keeps the answer honest. The scanner runs on the REWRITTEN stream, so its cut
	/// indices line up with what the parser sees; compat never rewrites a query anyway.
	pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
		self.cwd.feed(bytes);
		self.rewritten.clear();
		self.compat.rewrite(bytes, &mut self.rewritten);

		let cuts = self.answers.scan(&self.rewritten);
		let mut replies = Vec::new();
		let mut start = 0;
		for (end, query) in cuts {
			self.parser.process(&self.rewritten[start..end]);
			start = end;
			// vt100's cursor is 0-based; the reports are 1-based.
			let (row, col) = self.parser.screen().cursor_position();
			answer::reply(query, (row + 1, col + 1), &mut replies);
		}
		self.parser.process(&self.rewritten[start..]);
		replies
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

	#[test]
	fn plain_output_prompts_no_reply() {
		// Ordinary text and moves carry no query, so nothing is sent back.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"hello\r\n\x1b[2;3Hworld").is_empty());
	}

	#[test]
	fn a_status_query_is_answered() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[5n"), b"\x1b[0n".to_vec());
	}

	#[test]
	fn a_device_attributes_query_is_answered() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[c"), answer::PRIMARY_DA.to_vec());
	}

	#[test]
	fn a_cursor_report_uses_the_live_position() {
		// Move to row 3, column 5 (1-based input), then ask: the report names that cell.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[3;5H\x1b[6n"), b"\x1b[3;5R".to_vec());
	}

	#[test]
	fn the_size_probe_reports_the_corner_not_the_restore() {
		// The idiom a program uses to measure the terminal: save the cursor, jump to a corner
		// far past the edge (clamped to the real size), ask where it landed, restore. The
		// report must be the clamped corner — 10 rows by 40 columns — proving the cursor is
		// read AT the query and not after the restore has already undone the jump. Reading
		// after the whole chunk would answer 1;1 and the program would think the screen is
		// one cell wide.
		let mut terminal = Terminal::new(10, 40);
		let reply = terminal.process(b"\x1b7\x1b[999;999H\x1b[6n\x1b8");
		assert_eq!(reply, b"\x1b[10;40R".to_vec());
	}

	#[test]
	fn a_query_split_across_chunks_is_answered_on_completion() {
		// The query arrives in two packets; the reply comes with the packet that finishes it.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"\x1b[6").is_empty());
		assert_eq!(terminal.process(b"n"), b"\x1b[1;1R".to_vec());
	}
}

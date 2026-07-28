// term/mod.rs — the VT/ANSI terminal emulator wrapper (PLAN §9, §16, §23).
//
// A remote shell speaks a byte stream peppered with escape sequences: "move the
// cursor", "set the color to red", "clear the line". `alacritty_terminal` interprets
// that stream and maintains a grid of cells, each holding a glyph plus its colours and
// attributes, together with the cursor position and the terminal modes. The UI never sees
// raw bytes; it reads the grid — through the engine-agnostic `screen::Screen` view — and
// draws it (see ui/terminal.rs).
//
// This wrapper exists so the rest of the app depends on a tiny, intention-named surface
// (`process`, `resize`, `screen`, `cwd`, `title`) instead of the engine's full API, and so the
// engine stays swappable (§23 replaced `vt100` with `alacritty_terminal` behind exactly
// this surface). The engine also answers the host's queries itself — the status/identity ones
// (DSR, DA, DECRQM, cursor-position reports) it formats whole, and the ones about the terminal's
// own colours and pixel size (OSC 10/11/12/4, CSI 14t) it hands us a slot plus a formatter for,
// which we resolve against cmote's colour scheme (`palette`) and cell metrics (§23). Every reply
// comes back as bytes `process` returns for the caller to send. That retired the hand-rolled
// `term::compat` (the engine parses every cursor-move spelling) and `term::answer` (the engine
// answers every query).

pub mod cwd; // tracks the remote working directory announced by the shell (§17)
pub mod keymap; // maps GUI key events to the bytes a terminal sends
pub mod mouse; // maps pointer events to the reports a mouse-aware program expects
pub mod screen; // the engine-agnostic view of the screen the app reads through (§9, §16, §23)

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{NamedColor, Processor, Rgb};

use crate::palette;

/// The pty size the client requests and the emulator starts at, before the first
/// window measurement arrives (§9). Kept here as the single source of truth so
/// the ssh client (which requests the initial pty) and the emulator (which lays
/// out the grid) can never disagree; the grid is then reflowed to the real window
/// size via `resize` + `SshCommand::Resize`.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// How many scrolled-off lines to retain (§23). Deep enough to scroll back over a long build
/// or a `cat` of a big file; the engine grows the buffer lazily up to this cap, so the memory
/// is bounded at roughly this many rows of cells. On the alternate screen (vim, tmux, less) the
/// engine keeps no history at all, so scrolling there is inert by construction — a full-screen
/// program manages its own pages. Because the viewport can now sit above the active screen,
/// `screen` offsets every read by the engine's display offset (`Screen::cell` / `display_offset`)
/// rather than mapping a viewport row straight to a grid line.
const SCROLLBACK: usize = 10_000;

/// The engine, specialised to our reply-collecting listener. `screen::Screen` borrows this
/// to read the grid, so the alias is the one name that would change under another engine.
pub(super) type Engine = Term<Replies>;

/// A scrollback movement the GUI asks for (§23). cmote's own small vocabulary so the engine's
/// `Scroll` type stays behind `term/`, the same way `screen` hides the rest of the engine. The
/// wheel sends `Lines`, Shift+PageUp/PageDown send `PageUp`/`PageDown`, Shift+Home/End send
/// `Top`/`Bottom`, and every keystroke sends `Bottom` to snap the view back to the live prompt.
pub enum ScrollMotion {
	/// By a signed number of lines — positive scrolls up into history, negative back down.
	Lines(i32),
	/// By a whole viewport, up into history or back down toward the live screen.
	PageUp,
	PageDown,
	/// All the way to the oldest retained line, or back to the live bottom.
	Top,
	Bottom,
}

impl Terminal {
	/// Create an emulator with a `rows`×`cols` grid, matching the remote pty.
	pub fn new(rows: u16, cols: u16) -> Self {
		let replies = Arc::new(Mutex::new(ReplyBuffer {
			rows,
			cols,
			..ReplyBuffer::default()
		}));
		let config = Config {
			scrolling_history: SCROLLBACK,
			..Config::default()
		};
		let term = Term::new(
			config,
			&GridSize {
				rows: rows as usize,
				cols: cols as usize,
			},
			Replies(Arc::clone(&replies)),
		);
		Self {
			term,
			parser: Processor::new(),
			replies,
			cwd: cwd::Cwd::default(),
		}
	}

	/// Feed a chunk of raw output from the shell, and return the bytes to send BACK to it as
	/// replies to any status/identity queries it carried (§9, §23) — usually empty. The
	/// engine applies every escape sequence and glyph to the grid; partial sequences split
	/// across chunks are buffered internally, so any chunk boundary is safe. It answers the
	/// host queries itself (DSR, DA, DECRQM, cursor-position reports), handing each reply to
	/// our listener as an `Event::PtyWrite`, which accumulates in `replies`; we drain it here.
	/// A cursor-position report is emitted at the moment the query is parsed, so it reflects
	/// the cursor where the query sat — the engine gets that right where the old hand-rolled
	/// answerer had to split the feed to. The same bytes also feed the cwd tracker (§17),
	/// which reads the stream as it came off the wire.
	pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
		self.cwd.feed(bytes);
		self.parser.advance(&mut self.term, bytes);
		let mut buffer = self.replies.lock().expect("reply buffer mutex poisoned");
		std::mem::take(&mut buffer.bytes)
	}

	/// The remote shell's working directory, if it has announced one (§17). `None`
	/// until the first announcement — and on a shell that emits neither OSC 7 nor
	/// OSC 9;9, forever.
	pub fn cwd(&self) -> Option<&str> {
		self.cwd.path()
	}

	/// The window title the remote program last set (OSC 0/2), if any (§23). `None` until a
	/// program sets one — and again after a full reset clears it; a shell that sets none, forever.
	/// The GUI shows it in the title bar. Cloned out from behind the reply lock — a title is
	/// short and read at most once per frame.
	pub fn title(&self) -> Option<String> {
		self.replies
			.lock()
			.expect("reply buffer mutex poisoned")
			.title
			.clone()
	}

	/// Resize the grid when the window changes (§9). This only reflows our local
	/// view; the remote pty is told separately via `SshCommand::Resize`, so the
	/// two are kept in step by the caller (`app::on_window_resized`).
	pub fn resize(&mut self, rows: u16, cols: u16) {
		self.term.resize(GridSize {
			rows: rows as usize,
			cols: cols as usize,
		});
		let mut buffer = self.replies.lock().expect("reply buffer mutex poisoned");
		buffer.rows = rows;
		buffer.cols = cols;
	}

	/// Move the scrollback viewport (§23). The engine owns both the retained history and the
	/// display offset into it; cmote only says which way to move, then reads the offset back
	/// through `screen` to lay out the grid. On the alternate screen there is no history, so the
	/// engine clamps every motion to a no-op — which is why scrolling never fights a full-screen
	/// program (vim, tmux, less), each of which manages its own pages. The only engine event this
	/// raises is `MouseCursorDirty`, which our reply listener drops.
	pub fn scroll(&mut self, motion: ScrollMotion) {
		let scroll = match motion {
			ScrollMotion::Lines(lines) => Scroll::Delta(lines),
			ScrollMotion::PageUp => Scroll::PageUp,
			ScrollMotion::PageDown => Scroll::PageDown,
			ScrollMotion::Top => Scroll::Top,
			ScrollMotion::Bottom => Scroll::Bottom,
		};
		self.term.scroll_display(scroll);
	}

	/// Tell the emulator the pixel size of one cell, so it can answer a program that asks for
	/// its text area in pixels (CSI 14t). The GUI owns the cell metrics (§9), so it sets this
	/// once after construction; the emulator treats the numbers as opaque and only echoes them
	/// back. Until set they are zero, so the reply reads as a zero-sized area — harmless, since
	/// only graphics-capable programs ask and cmote draws no graphics.
	pub fn set_cell_pixels(&mut self, width: u16, height: u16) {
		let mut buffer = self.replies.lock().expect("reply buffer mutex poisoned");
		buffer.cell_width = width;
		buffer.cell_height = height;
	}

	/// The current screen, as cmote's engine-agnostic view (§9, §16, §23). The rest of the
	/// app reads the grid only through this, so the engine stays behind `term/`.
	pub fn screen(&self) -> screen::Screen<'_> {
		screen::Screen::new(&self.term)
	}
}

/// The terminal emulator: the engine, the byte parser that feeds it, the buffer its replies
/// land in, and the cwd scanner.
pub struct Terminal {
	term: Engine,
	/// Drives `term` from the raw byte stream (`Processor::advance`). Owns the VT state
	/// machine, so a sequence split across `process` calls is buffered here between them.
	parser: Processor,
	/// Reply bytes the engine produced for host queries this round, plus the few numbers a
	/// colour or size answer needs (the grid size and cell pixel size). Written by the `Replies`
	/// listener during `advance` and drained by `process`. An `Arc<Mutex<_>>` because the engine
	/// owns its own clone of the handle; the mutex is never contended (only the GUI thread
	/// touches it).
	replies: Arc<Mutex<ReplyBuffer>>,
	/// The remote working directory, learned from the OSC sequences the shell emits on each
	/// prompt (§17). The engine ignores those codes, so the same bytes are scanned here.
	cwd: cwd::Cwd,
}

/// The shared buffer the engine's replies collect in. Besides the bytes it holds the few
/// numbers a colour or size answer needs — the grid size and one cell's pixel size — so the
/// listener can resolve every query the instant it arrives, in the exact order the host sent
/// them, without reaching back into the engine. It also keeps the window title the remote set,
/// which is not a reply but state the GUI reads.
#[derive(Default)]
struct ReplyBuffer {
	bytes: Vec<u8>,
	rows: u16,
	cols: u16,
	cell_width: u16,
	cell_height: u16,
	title: Option<String>,
}

/// The engine's event sink. The engine reports everything the emulation layer cannot handle
/// itself as an `Event`; we answer the ones that expect a reply and drop the rest. In
/// particular the OSC 52 clipboard events (`ClipboardLoad`/`ClipboardStore`) are deliberately
/// ignored: a remote must not read or poison the local clipboard, and cmote only touches it on
/// an explicit local action (§12).
#[derive(Clone)]
pub(crate) struct Replies(Arc<Mutex<ReplyBuffer>>);

impl EventListener for Replies {
	fn send_event(&self, event: Event) {
		let mut buffer = self.0.lock().expect("reply buffer mutex poisoned");
		match event {
			// A reply the engine already formatted whole (DSR / DA / DECRQM / cursor-position
			// report, the character-cell size CSI 18t): its bytes go back verbatim.
			Event::PtyWrite(text) => buffer.bytes.extend_from_slice(text.as_bytes()),
			// "What colour is X?" — OSC 10 / 11 / 12 (foreground / background / cursor) or OSC
			// 4;n (palette slot n). The engine gives us the slot and a closure that frames the
			// reply; we resolve the slot against cmote's own scheme so the answer is exactly what
			// the grid paints (`palette`), then let the closure format it.
			Event::ColorRequest(index, format) => {
				let (r, g, b) = report_color(index);
				let reply = format(Rgb { r, g, b });
				buffer.bytes.extend_from_slice(reply.as_bytes());
			}
			// "How big is your text area in pixels?" — CSI 14t. Answered from the grid size and
			// the cell pixel size the GUI set (`set_cell_pixels`); the closure multiplies them.
			Event::TextAreaSizeRequest(format) => {
				let reply = format(WindowSize {
					num_lines: buffer.rows,
					num_cols: buffer.cols,
					cell_width: buffer.cell_width,
					cell_height: buffer.cell_height,
				});
				buffer.bytes.extend_from_slice(reply.as_bytes());
			}
			// The window title the remote set (OSC 0/2), or a reset to none. Not a reply —
			// stored for the GUI to show in the title bar (§23). Control characters are
			// stripped: the title bar is chrome cmote owns, so a remote must not be able to
			// smuggle newlines or escapes into it.
			Event::Title(title) => buffer.title = Some(sanitize_title(&title)),
			Event::ResetTitle => buffer.title = None,
			// Everything else — the clipboard pair, the bell, a colour *set* — needs no reply
			// and carries nothing we surface, so it is dropped.
			_ => {}
		}
	}
}

/// A remote-set window title, reduced to one line of printable text. The title bar is chrome
/// cmote owns (§23), so control characters — newlines, escapes, tabs — are dropped rather than
/// passed through where they could disrupt or spoof it.
fn sanitize_title(title: &str) -> String {
	title
		.chars()
		.filter(|character| !character.is_control())
		.collect()
}

/// The RGB cmote reports for a colour-query slot. A palette index (0-255) resolves through the
/// shared xterm-256 table; the named background role reports the scheme's background, and every
/// other role (foreground, cursor, the bright/dim foregrounds) the foreground — the cursor is
/// drawn by inverting the cell, so its ink is the foreground. Only the foreground / background /
/// cursor roles and 0-255 are reachable through OSC 10 / 11 / 12 / 4.
fn report_color(index: usize) -> (u8, u8, u8) {
	match index {
		0..=255 => palette::xterm_256(index as u8),
		i if i == NamedColor::Background as usize => palette::DEFAULT_BG,
		_ => palette::DEFAULT_FG,
	}
}

/// The grid dimensions handed to the engine. Our own `Dimensions` impl rather than the
/// engine's test-only `TermSize`, and with no scrollback `total_lines == screen_lines`.
struct GridSize {
	rows: usize,
	cols: usize,
}

impl Dimensions for GridSize {
	fn total_lines(&self) -> usize {
		self.rows
	}

	fn screen_lines(&self) -> usize {
		self.rows
	}

	fn columns(&self) -> usize {
		self.cols
	}
}

// The engine is not `Debug`, and `App` derives `Debug`; give a terse, content-free
// representation so nothing from the remote session leaks into logs.
impl std::fmt::Debug for Terminal {
	fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let (rows, cols) = self.screen().size();
		write!(formatter, "Terminal({rows}x{cols})")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// Read `len` cells of one row into a string, through the screen view.
	fn read(terminal: &Terminal, row: u16, col: u16, len: u16) -> String {
		let screen = terminal.screen();
		(col..col + len)
			.filter_map(|col| screen.cell(row, col))
			.map(|cell| cell.contents().to_owned())
			.collect()
	}

	#[test]
	fn a_full_screen_program_lands_where_it_aims() {
		// The btop shape: position with the `f` spelling (HVP), write, position again. The
		// engine parses that spelling natively — the reason the hand-rolled rewriter is gone
		// (§23) — so each word lands in its own cell.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b[3;5fleft\x1b[7;20fright");
		assert_eq!(read(&terminal, 2, 4, 4), "left");
		assert_eq!(read(&terminal, 6, 19, 5), "right");
		// Nothing spilled onto the first row on the way.
		assert_eq!(read(&terminal, 0, 0, 10).trim(), "");
	}

	#[test]
	fn a_wide_glyph_reserves_two_columns() {
		// A CJK glyph claims its own cell and marks the next as its continuation, which the
		// view reports so the renderer can skip it.
		let mut terminal = Terminal::new(2, 10);
		terminal.process("a世b".as_bytes());
		let screen = terminal.screen();
		assert_eq!(screen.cell(0, 0).unwrap().contents(), "a");
		assert!(screen.cell(0, 1).unwrap().is_wide());
		assert!(screen.cell(0, 2).unwrap().is_wide_continuation());
		assert_eq!(screen.cell(0, 3).unwrap().contents(), "b");
	}

	#[test]
	fn plain_output_prompts_no_reply() {
		// Ordinary text and moves carry no query, so nothing is sent back.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"hello\r\n\x1b[2;3Hworld").is_empty());
	}

	#[test]
	fn a_status_query_is_answered() {
		// DSR "are you ok?" -> the fixed "ok" report, produced by the engine and drained here.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[5n"), b"\x1b[0n".to_vec());
	}

	#[test]
	fn a_device_attributes_query_gets_a_reply() {
		// DA1 "what terminal are you?" -> a primary-device-attributes report. The exact
		// capabilities are the engine's to state; we only assert it answered in the right
		// shape (`CSI ? … c`), since leaving it silent is what stalls vim / tmux at startup.
		let mut terminal = Terminal::new(10, 40);
		let reply = terminal.process(b"\x1b[c");
		assert!(
			reply.starts_with(b"\x1b[?"),
			"unexpected DA reply: {reply:?}"
		);
		assert_eq!(reply.last(), Some(&b'c'));
	}

	#[test]
	fn a_cursor_report_uses_the_live_position() {
		// Move to row 3, column 5 (1-based), then ask: the report names that cell.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[3;5H\x1b[6n"), b"\x1b[3;5R".to_vec());
	}

	#[test]
	fn the_size_probe_reports_the_corner_not_the_restore() {
		// The idiom a program uses to measure the terminal: save the cursor, jump to a corner
		// far past the edge (clamped to the real size), ask where it landed, restore. The
		// report must be the clamped corner — 10 rows by 40 columns — proving the reply is
		// emitted at the query, before the restore undoes the jump.
		let mut terminal = Terminal::new(10, 40);
		let reply = terminal.process(b"\x1b7\x1b[999;999H\x1b[6n\x1b8");
		assert_eq!(reply, b"\x1b[10;40R".to_vec());
	}

	#[test]
	fn a_query_split_across_chunks_is_answered_on_completion() {
		// The query arrives in two packets; the parser buffers across them and the reply
		// comes with the packet that finishes it.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"\x1b[6").is_empty());
		assert_eq!(terminal.process(b"n"), b"\x1b[1;1R".to_vec());
	}

	#[test]
	fn a_background_colour_query_reports_the_scheme_background() {
		// OSC 11 "what is your background?" -> the scheme's background (palette::DEFAULT_BG,
		// 0x1e) in the rgb:RRRR/GGGG/BBBB form, echoing the BEL terminator the query used. This
		// is what lets a program pick a light/dark colourscheme to suit the terminal.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b]11;?\x07"),
			b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07".to_vec()
		);
	}

	#[test]
	fn a_foreground_colour_query_reports_the_scheme_foreground() {
		// OSC 10 "what is your foreground?" -> the scheme's foreground (0xd0).
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b]10;?\x07"),
			b"\x1b]10;rgb:d0d0/d0d0/d0d0\x07".to_vec()
		);
	}

	#[test]
	fn a_palette_colour_query_reports_that_slot() {
		// OSC 4;3 "what is palette slot 3?" -> ANSI yellow (0x808000), resolved through the
		// same shared table the grid paints from, so the answer never disagrees with the screen.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b]4;3;?\x07"),
			b"\x1b]4;3;rgb:8080/8080/0000\x07".to_vec()
		);
	}

	#[test]
	fn a_pixel_size_query_multiplies_the_cell_size_by_the_grid() {
		// CSI 14t "text area in pixels?" -> rows*cell_height by cols*cell_width, from the cell
		// pixel size the GUI set: 10 rows * 17 = 170 high, 40 cols * 8 = 320 wide.
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(8, 17);
		assert_eq!(terminal.process(b"\x1b[14t"), b"\x1b[4;170;320t".to_vec());
	}

	#[test]
	fn a_character_size_query_still_reports_rows_and_columns() {
		// CSI 18t "text area in characters?" is answered by the engine itself as a plain
		// report; it must keep working alongside the queries we resolve. 10 rows by 40 columns.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[18t"), b"\x1b[8;10;40t".to_vec());
	}

	#[test]
	fn a_window_title_is_captured_and_emptied() {
		// OSC 2 sets the window title; the GUI reads it back through `title`. None until a
		// program sets one, then whatever it set, and empty once it clears the text.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.title(), None);
		terminal.process(b"\x1b]2;build\x07");
		assert_eq!(terminal.title().as_deref(), Some("build"));
		terminal.process(b"\x1b]2;\x07");
		assert_eq!(terminal.title().as_deref(), Some(""));
	}

	#[test]
	fn a_title_is_reduced_to_one_line_of_plain_text() {
		// A remote must not smuggle control characters into cmote's own title bar: an embedded
		// tab is stripped, leaving only the printable text.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]2;a\tb\x07");
		assert_eq!(terminal.title().as_deref(), Some("ab"));
	}

	#[test]
	fn setting_a_title_is_not_mistaken_for_a_reply() {
		// A title is state, not a host reply, so `process` must return no bytes for it — the
		// input channel is for keystrokes and query answers only.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"\x1b]2;build\x07").is_empty());
	}

	#[test]
	fn scrolling_back_reveals_lines_that_left_the_top() {
		// A three-row screen fed six lines: the first three have scrolled off into history (§23).
		// At the live bottom (offset 0) the viewport shows the last three; scrolling up one line
		// brings the line just above the top back into view, and a scroll to the bottom returns
		// to the live tail. This is the whole point of the display-offset arithmetic in `screen`.
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
		assert_eq!(terminal.screen().display_offset(), 0);
		assert_eq!(read(&terminal, 0, 0, 4).trim(), "four");
		assert_eq!(read(&terminal, 2, 0, 3).trim(), "six");
		// Up one line: the top row is now "three", the line that had just scrolled off.
		terminal.scroll(ScrollMotion::Lines(1));
		assert_eq!(terminal.screen().display_offset(), 1);
		assert_eq!(read(&terminal, 0, 0, 5).trim(), "three");
		// Back to the bottom: the live tail again, offset zero.
		terminal.scroll(ScrollMotion::Bottom);
		assert_eq!(terminal.screen().display_offset(), 0);
		assert_eq!(read(&terminal, 2, 0, 3).trim(), "six");
	}

	#[test]
	fn output_leaves_a_scrolled_back_viewport_where_it_is() {
		// Reading history must not be yanked to the bottom by activity (§23): with the viewport
		// scrolled up, a fresh line of output leaves the SAME lines on screen. The engine keeps
		// the display stationary in content — it tracks new output by growing the offset
		// underneath, so what the user is reading does not slide out from under them.
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour");
		terminal.scroll(ScrollMotion::Lines(1));
		assert_eq!(read(&terminal, 0, 0, 3).trim(), "one");
		terminal.process(b"\r\nfive");
		assert_eq!(read(&terminal, 0, 0, 3).trim(), "one");
	}

	#[test]
	fn the_alternate_screen_has_no_scrollback() {
		// A full-screen program (DECSET ?1049) swaps to the alternate screen, which keeps no
		// history — so a scroll-to-top there is a no-op and the viewport stays at the bottom.
		// That is what makes scrolling inert while vim / tmux / less own the screen (§23).
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"\x1b[?1049h");
		terminal.scroll(ScrollMotion::Top);
		assert_eq!(terminal.screen().display_offset(), 0);
	}
}

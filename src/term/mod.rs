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
//
// Five identity queries the engine does NOT answer — its VT parser treats every DCS as a no-op,
// has no arm for the version request, its device-attributes handler covers only DA1 and DA2, and it
// has none for the graphics-capability request — so cmote sniffs them out of the same stream and
// answers them itself (`query`, §33, §36, §41): XTVERSION (`CSI > q`), DECRQSS (`DCS $ q … ST`),
// XTGETTCAP (`DCS + q … ST`), DA3 (`CSI = c`) and XTSMGRAPHICS (`CSI ? Pi;Pa;Pv S`). Only DECRQSS's
// SGR request needs live state; `process` fills it from the pen. The engine's OWN DA1 answer is then
// amended on its way out to advertise sixel (`query::with_sixel_attribute`), because cmote draws
// images the engine knows nothing about.
//
// Those images are the other thing the engine drops and cmote reads: a sixel picture arrives as a
// DCS its parser ignores, so `graphics` scans it out of the same bytes, `sixel` decodes it, and
// `process` reserves the cells it covers in the engine so the picture rides the grid as text does
// (§41).

pub mod cwd; // tracks the remote working directory announced by the shell (§17)
pub mod graphics; // finds the inline images the engine drops, and anchors them to the document (§41)
pub mod iterm; // reads the parts of iTerm2's OSC 1337 namespace cmote honours — an allow-list (§55)
pub mod keymap; // maps GUI key events to the bytes a terminal sends
pub mod kitty; // encodes key events in the kitty keyboard protocol's CSI u form (§25)
pub mod modkeys; // tracks the remote's xterm modifyOtherKeys mode for the key encoder (§9)
pub mod mouse; // maps pointer events to the reports a mouse-aware program expects
mod osc; // frames OSC strings out of the stream for the scanners below to read (§17, §34, §54, §55)
pub mod osc133; // reads the shell-integration prompt marks the engine ignores (§34)
pub mod progress; // reads the progress a remote command reports, OSC 9;4 (§54)
mod query; // answers the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS (§33, §36, §41)
pub mod screen; // the engine-agnostic view of the screen the app reads through (§9, §16, §23)
pub mod search; // finds text anywhere in the scrollback for the find bar (§35)
pub mod sixel; // decodes a sixel image's payload into pixels (§41)

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::Config;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

use crate::palette;

/// The pty size the client requests and the emulator starts at, before the first
/// window measurement arrives (§9). Kept here as the single source of truth so
/// the ssh client (which requests the initial pty) and the emulator (which lays
/// out the grid) can never disagree; the grid is then reflowed to the real window
/// size via `resize` + `SshCommand::Resize`.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// cmote's terminal identity, reported to a program that sends XTVERSION (§33). The `name(version)`
/// form is what xterm and kitty use and what a fingerprinting program pattern-matches on; the
/// version is the crate's, stamped in at build time so the reply never drifts from the binary.
const VERSION: &str = concat!("cmote(", env!("CARGO_PKG_VERSION"), ")");

/// cmote's unit id, reported to a program that sends DA3 (`CSI = c`, §36). DECRPTUI wants eight hex
/// digits: a two-digit manufacturing site code then a six-digit terminal id. cmote has no
/// DEC-assigned site, so the site is `00` and the id spells `CME` in ASCII (43 4D 45) — a stable,
/// recognisable constant. It is deliberately NOT derived from the machine (no serial, MAC or
/// install id): a per-machine unit id would be a free fingerprint for every host the user logs into
/// (see `query::da3_reply`). Kept beside `VERSION` so both identity facts live in one place.
const UNIT_ID: &str = "00434D45";

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

/// Where a command's output sits in the DOCUMENT (§34, §40), as the caller needs it to build a text
/// selection: the absolute first and last lines it occupies — inclusive, which is what a selection's
/// head wants — and the grid's last column, since a whole-output selection runs the full width.
/// Plain numbers, so `term/` hands the UI what it needs without depending on the UI's own selection
/// type.
///
/// These are absolute lines rather than viewport rows (as this was until §40), so an output taller
/// than the screen is selected WHOLE: the copy path reads the document, not the visible grid. `start_line
/// <= end_line` always, and revealing only decides what the user is looking at, never what is selected.
pub struct OutputSpan {
	pub start_line: u64,
	pub end_line: u64,
	pub last_col: u16,
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
			// Let the engine accept, track and answer the kitty keyboard protocol (§25). Unlike
			// modifyOtherKeys — which the engine ignores, so `modkeys` scans it out of the stream —
			// the engine fully implements kitty: it keeps the pushed-flags stack (`CSI > flags u` /
			// `CSI < n u` / `CSI = flags ; mode u`), swaps it across the alternate screen, and
			// answers the `CSI ? u` query itself. All of that is gated behind this flag, off in
			// `Config::default()`. With it on, cmote's only job is the *encoding*: it reads the
			// active flags off the seam (`Screen::kitty_flags`) and `keymap`/`kitty` turn a key press
			// into the matching CSI u report. The query reply comes back as an `Event::PtyWrite`,
			// which the `Replies` listener already drains — so no extra reply path is needed.
			kitty_keyboard: true,
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
			modkeys: modkeys::ModKeys::default(),
			queries: query::Queries::default(),
			prompts: osc133::Prompts::default(),
			iterm: iterm::Iterm::default(),
			progress: progress::Reports::default(),
			graphics: graphics::Images::default(),
			on_alternate: false,
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
	/// answerer had to split the feed to. The same bytes also feed the cwd tracker (§17), which
	/// reads the stream as it came off the wire for the working directory the shell announces, and
	/// the OSC 133 prompt-mark scanner (§34) and the inline-image scanner (§41) — for both of which
	/// `process` DOES split the advance, so each mark is applied at the grid line the cursor is on
	/// when it arrives and each picture is placed where the stream put it.
	pub fn process(&mut self, bytes: &[u8]) -> Vec<u8> {
		self.cwd.feed(bytes);
		self.modkeys.feed(bytes);
		// The progress a remote command reports (§54). Like the cwd, it is a latest-value reading
		// with no position on the grid, so it needs no split in the advance below — only the order
		// the reports arrive in, which its own scanner keeps.
		self.progress.feed(bytes);
		// Sniff the identity queries the engine drops (§33). Parse them BEFORE advancing, but reply
		// AFTER: a DECRQSS SGR report then reflects the pen as this chunk left it, which is right
		// for the usual flow where a program sets attributes and then queries in the same write.
		let queries = self.queries.feed(bytes);
		// OSC 133 shell-integration marks (§34) and inline images (§41): the engine ignores both, so
		// scan them out and apply each at the point in the stream it sits. A prompt-start anchors to a
		// grid line and an image anchors to the cursor's line and column, so the engine is advanced up
		// to the event's offset FIRST — then the cursor is read exactly where the mark or the picture
		// belongs. The common case (a chunk carrying neither) is a single advance, so only a chunk
		// that actually carries one pays for the split.
		let marks = self.prompts.feed(bytes);
		let images = self.graphics.feed(bytes);
		// The explicit bookmarks a script dropped (§55). Grid-anchored like a prompt mark, and for the
		// same reason: the event's whole content is the line it arrived on.
		let bookmarks = self.iterm.feed(bytes);
		// Whether this chunk put a picture on the alternate page — the one thing that makes the
		// covered-cell sweep below sit the chunk out (see `retire_covered_images`).
		let mut placed_on_alternate = false;
		if marks.is_empty() && images.is_empty() && bookmarks.is_empty() {
			self.parser.advance(&mut self.term, bytes);
		} else {
			let mut start = 0;
			for (offset, split) in splits(marks, images, bookmarks) {
				self.parser.advance(&mut self.term, &bytes[start..offset]);
				start = offset;
				match split {
					Split::Prompt(mark) => {
						let history = self.term.grid().history_size();
						let (row, _) = self.screen().cursor_position();
						self.prompts.apply(mark, history, row);
					}
					Split::Graphics(event) => placed_on_alternate |= self.apply_graphics(event),
					// A bookmark is read the same way a prompt mark is — the cursor, now that the
					// engine has been advanced to the sequence, names the line the script meant.
					Split::UserMark => {
						let history = self.term.grid().history_size();
						let (row, _) = self.screen().cursor_position();
						self.prompts.record_user_mark(history, row);
					}
				}
			}
			self.parser.advance(&mut self.term, &bytes[start..]);
		}
		// The chunk is applied, so this is where a swap on or off the alternate screen is noticed —
		// including one that carried no picture with it, which the split loop above never sees (§41).
		self.sync_alternate();
		if !placed_on_alternate {
			self.retire_covered_images();
		}
		let mut buffer = self.replies.lock().expect("reply buffer mutex poisoned");
		let mut out = std::mem::take(&mut buffer.bytes);
		drop(buffer);
		for query in queries {
			match query {
				// XTVERSION: cmote's fixed name and version.
				query::Query::Version => out.extend_from_slice(&query::version_reply(VERSION)),
				// DECRQSS for SGR: rebuild the current pen — exactly what the grid paints — as an
				// SGR string and report it valid.
				query::Query::Decrqss(query::Decrqss::Sgr) => {
					let sgr = pen_sgr(&self.term.grid().cursor.template);
					out.extend_from_slice(&query::decrqss_sgr_reply(&sgr));
				}
				// Every other DECRQSS setting: an honest "unsupported".
				query::Query::Decrqss(query::Decrqss::Unsupported) => {
					out.extend_from_slice(&query::decrqss_unsupported_reply());
				}
				// XTGETTCAP: answer each requested capability from cmote's small map of facts.
				query::Query::Capabilities(names) => {
					out.extend_from_slice(&query::gettcap_reply(&names));
				}
				// DA3: cmote's constant unit id — an answer that identifies the program, never
				// the machine (§36).
				query::Query::UnitId => out.extend_from_slice(&query::da3_reply(UNIT_ID)),
				// XTSMGRAPHICS: the limits the sixel decoder actually enforces (§41), so a program
				// sizing a picture for cmote is told the truth rather than a hopeful maximum.
				query::Query::Graphics(request) => {
					out.extend_from_slice(&query::graphics_reply(
						&request,
						sixel::COLOR_REGISTERS as u16,
						(sixel::MAX_WIDTH, sixel::MAX_HEIGHT),
					));
				}
			}
		}
		// Last, amend the engine's own DA1 answer if this chunk asked for one: cmote draws sixels, so
		// its device attributes have to say so (§41). Nothing else in `out` is touched.
		query::with_sixel_attribute(out)
	}

	/// Act on something the image scanner found, at the point in the stream it sits (§41).
	///
	/// A picture is anchored to the cursor's own line and column and then RESERVES that many cells:
	/// the engine knows nothing about images, so unless the cells are claimed the shell's next line of
	/// output would be written straight over the picture. Reserving is `reserve_cells` — erase the box,
	/// then feed the line feeds — which leaves the cursor below the image exactly as a terminal that
	/// implements sixel natively does, so a prompt lands under the picture and not on it.
	///
	/// The ALTERNATE screen has its own page of pictures (§41), which is what lets `ranger` show a
	/// preview and `mpv --vo=sixel` play. Everything above holds there too — the anchor is still the
	/// cursor's row, the cells are still reserved — with one substitution: that page keeps no history,
	/// so `history_size` is 0 and the absolute line of row `r` is just `r`. Nothing new is needed to
	/// say where the picture goes; what the page needs is its own LIFETIME, which is what the second
	/// store gives it.
	///
	/// Returns whether a picture was placed on the alternate page, which `process` uses to leave that
	/// page's covered-cell sweep alone for the chunk.
	fn apply_graphics(&mut self, event: graphics::Event) -> bool {
		// A swap earlier in this same chunk has already been applied to the engine by the split
		// advance, so ask before doing anything: the picture arriving belongs to the page that is up
		// NOW, and the page it swapped away from should already have been emptied.
		self.sync_alternate();
		match event {
			graphics::Event::Image(image) => {
				// Read the cursor before the reservation moves it. On the primary screen the absolute
				// line is history + the row (§40) — safe to read first, because scrolling grows the
				// history by exactly as much as it moves the content up, so the line goes on naming the
				// same text either way.
				let (row, col) = self.screen().cursor_position();
				let (rows, cols) = if self.on_alternate {
					self.graphics.place_alternate(image, row, col)
				} else {
					let history = self.term.grid().history_size() as u64;
					self.graphics.place(image, history + u64::from(row), col)
				};
				self.reserve_cells(rows, cols);
				return self.on_alternate;
			}
			// On the primary screen the two erases take only the pictures whose lines they erase, so a
			// `CSI 2 J` at a prompt leaves the plots further up the scrollback alone. On the alternate
			// page there is no such split — it is one screen with no history behind it — so `CSI 2 J`
			// takes all of its pictures and `CSI 3 J` says nothing about a scrollback that is not
			// there.
			graphics::Event::ClearScreen => {
				if self.on_alternate {
					self.graphics.clear_alternate();
				} else {
					self.graphics
						.clear_screen(self.term.grid().history_size() as u64);
				}
			}
			graphics::Event::ClearScrollback => {
				if !self.on_alternate {
					self.graphics
						.clear_scrollback(self.term.grid().history_size() as u64);
				}
			}
			// RIS resets the terminal itself, so it takes everything wherever the session is.
			graphics::Event::Reset => self.graphics.clear(),
		}
		false
	}

	/// Notice a swap between the primary and the alternate screen, and empty the alternate page's
	/// pictures whenever one happens (§41).
	///
	/// Either direction clears, which is the whole rule and is why it is not written as two. Swapping
	/// OFF the page ends the program that drew the pictures, so nothing it left survives to be painted
	/// over the shell — and that alone leaves the page empty, so the clear on the way back ON is the
	/// belt to its braces: a program is shown its own blank screen and never the last one's, whatever
	/// route got it there. The primary screen's pictures are untouched either way — a `vim` session in
	/// the middle of a scrollback of plots leaves every one of them where it was, which is exactly what
	/// a user expects on quitting it.
	fn sync_alternate(&mut self) {
		let alternate = self.screen().is_alternate();
		if alternate != self.on_alternate {
			self.on_alternate = alternate;
			self.graphics.clear_alternate();
		}
	}

	/// Drop the alternate page's pictures the program has drawn text over since the last chunk (§41).
	///
	/// A terminal with native graphics gets this free: the picture lives in the cells, so writing a
	/// character erases the pixels under it. cmote's pictures sit BESIDE the grid, so the cells they
	/// reserved were blanked when they were placed — and a glyph appearing in that box since means the
	/// program has repainted over the picture. `ranger` moving from an image preview to a text one is
	/// exactly this, and it is the only signal there is: it repaints the pane in place, with no erase
	/// and no swap to say so.
	///
	/// A chunk that placed a picture sits the sweep out, so a program that draws its image and then
	/// the rest of its frame in one write does not blank its own picture the instant it arrives. It
	/// costs a chunk's delay on noticing, which is a frame nobody sees.
	fn retire_covered_images(&mut self) {
		if !self.on_alternate || self.graphics.alternate().is_empty() {
			return;
		}
		// The screen borrows the engine and the store is borrowed mutably, so they are taken as
		// separate fields rather than through `self.screen()`, which would borrow all of `self`.
		let screen = screen::Screen::new(&self.term);
		self.graphics
			.retire_covered_alternate(|placement| covered(&screen, placement));
	}

	/// Claim a `rows`×`cols` box of cells for an image just placed at the cursor (§41), leaving the
	/// cursor at the left margin on the line below it.
	///
	/// Two sequences per row, fed to the engine as if the remote had sent them: ECH (`CSI Pn X`)
	/// erases exactly `cols` characters from the cursor rightward — so a picture drawn over existing
	/// text blanks what it covers and no more — and LF drops a line at the same column, scrolling the
	/// screen when the cursor is already at the bottom, which is how the picture's cells become
	/// ordinary scrollback. A closing CR puts the cursor at the left margin.
	///
	/// Injecting VT sequences rather than reaching into the grid is deliberate: erasing and scrolling
	/// are the engine's business, and doing it this way means the reservation obeys the scroll region,
	/// the autowrap mode and the character-set state exactly as the program's own output would.
	///
	/// On the ALTERNATE page the rows are stepped with CUD (`CSI B`) instead of LF, and that is the
	/// one difference between the two (§41). LF at the bottom of the screen SCROLLS, which on the
	/// primary screen is right — that is how a picture's cells become scrollback — and on the
	/// alternate page is ruin: the page keeps no history, so a scroll throws a row away for good, it
	/// shifts every other picture's anchor row out from under it, and a full-screen image (`mpv
	/// --vo=sixel` draws one every frame) reaches the bottom by definition. CUD stops at the margin
	/// instead, so the reservation can never move the page. The cost is that the cursor is left on the
	/// last row rather than below the picture, which no full-screen program notices: they all position
	/// absolutely.
	fn reserve_cells(&mut self, rows: u16, cols: u16) {
		let down: &[u8] = if self.on_alternate { b"\x1b[B" } else { b"\n" };
		let mut feed = Vec::new();
		for _ in 0..rows {
			feed.extend_from_slice(format!("\x1b[{cols}X").as_bytes());
			feed.extend_from_slice(down);
		}
		feed.push(b'\r');
		self.parser.advance(&mut self.term, &feed);
	}

	/// The remote shell's working directory, if it has announced one (§17). `None`
	/// until the first announcement — and on a shell that emits neither OSC 7 nor
	/// OSC 9;9, forever.
	pub fn cwd(&self) -> Option<&str> {
		self.cwd.path()
	}

	/// The xterm `modifyOtherKeys` level the remote last selected (§9). `Off` until a program
	/// asks for it. The engine does not interpret the mode (it is an input-encoding hint, not a
	/// screen operation), so cmote scans the stream for it (`modkeys`) and the key encoder reads
	/// it here to decide whether a Ctrl/Alt combo becomes the `CSI 27 ; mod ; code ~` form.
	pub fn modify_other_keys(&self) -> modkeys::ModifyOtherKeys {
		self.modkeys.level()
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
		// A resize reflows the grid: wrapped lines re-wrap at the new width, so the line count of
		// the history changes and the absolute positions the prompt marks were recorded at no
		// longer line up (§34). Rather than point a jump at the wrong reflowed line, drop the marks
		// — a session keeps its scrollback, only the prompt ticks are relearned from the next
		// prompt on. `ponytail:` cleared on any resize, including a height-only one that would not
		// actually reflow the columns.
		self.prompts.clear();
		// The inline images are anchored the same way and go for the same reason (§41): a reflowed
		// document would leave each picture floating over whatever text ended up on its old line.
		// `ponytail:` a terminal with native graphics reflows its images instead of dropping them.
		self.graphics.clear();
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

	/// Tell the emulator the pixel size of one cell. The GUI owns the cell metrics (§9), so it sets
	/// this once after construction, and two things read it back: a program asking for its text area
	/// in pixels (CSI 14t) is answered from it, and an inline image's pixels are turned into the cells
	/// it reserves by it (§41). Until set, the reply reads as a zero-sized area and the image store
	/// falls back to its own approximation of a cell — the GUI measures before any output arrives, so
	/// that window is a test's concern rather than a session's.
	pub fn set_cell_pixels(&mut self, width: u16, height: u16) {
		self.graphics.set_cell_pixels(width, height);
		let mut buffer = self.replies.lock().expect("reply buffer mutex poisoned");
		buffer.cell_width = width;
		buffer.cell_height = height;
	}

	/// The current screen, as cmote's engine-agnostic view (§9, §16, §23). The rest of the
	/// app reads the grid only through this, so the engine stays behind `term/`.
	pub fn screen(&self) -> screen::Screen<'_> {
		screen::Screen::new(&self.term)
	}

	/// The inline images the page ON SHOW is holding, oldest first (§41). Each names the absolute
	/// document line and column its top-left corner sits on, so the renderer resolves them against
	/// wherever the viewport is parked — exactly as it does the selection (§40). Empty until a program
	/// sends a picture, and on a session that never does, forever.
	///
	/// The two pages have separate stores and only the one being drawn is handed over, so the
	/// renderer never has to ask which screen it is on: while a full-screen program is up it gets that
	/// program's pictures, and the moment it quits it gets the scrollback's again, untouched. On the
	/// alternate page there is no history and no scrollback offset, so a placement's absolute line is
	/// its row and the renderer's own arithmetic resolves it without a special case (§40).
	pub fn images(&self) -> &[graphics::Placement] {
		if self.screen().is_alternate() {
			self.graphics.alternate()
		} else {
			self.graphics.placements()
		}
	}

	/// Where the remote command cycle stands (§34), from the OSC 133 marks the shell emits. `Idle`
	/// on a shell without integration configured, forever — nothing is ever guessed. The GUI pairs
	/// it with `last_exit` for the per-tab status glyph.
	pub fn command_state(&self) -> osc133::CommandState {
		self.prompts.state()
	}

	/// The exit code of the last command the shell reported finishing (§34), or `None` when none
	/// has yet — a fresh session, or a shell that never emits the `D` mark.
	pub fn last_exit(&self) -> Option<i32> {
		self.prompts.last_exit()
	}

	/// What the remote command last reported about its progress (OSC 9;4, §54). `Progress::None` on
	/// a shell whose commands never report — which is most of them, most of the time, so the GUI
	/// draws nothing at all unless something asked it to.
	pub fn progress(&self) -> progress::Progress {
		self.progress.current()
	}

	/// The viewport rows (0-based from the top of the visible screen) a prompt mark sits on right
	/// now (§34), so the grid can draw a tick beside each. Empty on a shell without integration and
	/// on the alternate screen, which keeps no history and shows no shell prompts.
	pub fn prompt_rows(&self) -> Vec<u16> {
		let grid = self.term.grid();
		self.prompts.visible_rows(
			grid.history_size(),
			grid.display_offset(),
			self.term.screen_lines(),
		)
	}

	/// The viewport rows holding an explicit bookmark a script dropped (§55), so the grid can tick
	/// those too — in their own colour, since a bookmark is a place the SCRIPT chose and a prompt is a
	/// place the shell was. Empty unless something actually sent `OSC 1337 ; SetMark`.
	pub fn user_mark_rows(&self) -> Vec<u16> {
		let grid = self.term.grid();
		self.prompts.visible_user_rows(
			grid.history_size(),
			grid.display_offset(),
			self.term.screen_lines(),
		)
	}

	/// Scroll the nearest prompt above or below the viewport into view (§34), returning whether
	/// there was one to move to (so the caller can leave the view be when there is not). The target
	/// offset is `osc133`'s to choose; here it is turned into the signed delta the engine scrolls
	/// by — positive climbs into history — relative to where the viewport sits now.
	pub fn jump_prompt(&mut self, direction: osc133::Direction) -> bool {
		let grid = self.term.grid();
		let history = grid.history_size();
		let offset = grid.display_offset();
		let Some(target) = self.prompts.jump(direction, history, offset) else {
			return false;
		};
		let delta = target as i32 - offset as i32;
		if delta != 0 {
			self.term.scroll_display(Scroll::Delta(delta));
		}
		true
	}

	/// Reveal and locate a finished command's output for a text selection (§34) — the Ctrl+Shift+O
	/// keybind. The first press takes the most recent command; each press after it steps one command
	/// further back through the session, which is what makes the key a way of reading BACK through
	/// what has been run rather than a way of grabbing the last thing only.
	///
	/// Returns the document lines the output occupies (after scrolling its start into view if it was
	/// above the live screen), or `None` when no command has finished, or once the walk has reached
	/// the oldest one held — the selection then stays on it. The caller turns the span into a
	/// selection the ordinary Copy then grabs — all of it, however tall it is (§40).
	pub fn select_output_back(&mut self) -> Option<OutputSpan> {
		let (start, end) = self.prompts.walk_output()?;
		Some(self.locate_output(start, end))
	}

	/// Start the walk over, so the next Ctrl+Shift+O takes the most recent command again (§34). The
	/// app calls this when the user does anything else with the grid: the walk is one gesture, and
	/// pressing on the grid — a selection, a click, a drag — is the start of another.
	pub fn restart_output_walk(&mut self) {
		self.prompts.restart_walk();
	}

	/// The same for the command whose prompt tick sits on viewport `row` (§34) — the gutter-click
	/// path. The row is turned into the absolute prompt line it currently shows before the command
	/// is looked up, so the click resolves against the scrollback-stable coordinate rather than the
	/// scroll position. `None` when that row carries no finished command's prompt.
	pub fn select_output_at_row(&mut self, row: u16) -> Option<OutputSpan> {
		let grid = self.term.grid();
		// Viewport row -> absolute line: absolute = row - display_offset + history_size. Done in
		// i64 so a nonsensical row (offset deeper than row + history) yields no command rather than
		// underflowing; an on-screen tick can never hit that.
		let prompt = i64::from(row) + grid.history_size() as i64 - grid.display_offset() as i64;
		if prompt < 0 {
			return None;
		}
		let (start, end) = self.prompts.output_at_prompt(prompt as u64)?;
		Some(self.locate_output(start, end))
	}

	/// Reveal an absolute output line range `[start, end)` and return it as the span to select (§34,
	/// §40). The viewport is scrolled so the output's first line is at the top ONLY when that line is
	/// not already visible, so a command already on screen is selected in place rather than jerking
	/// the view.
	///
	/// Revealing and selecting are now separate concerns: the span handed back is the range itself,
	/// in document coordinates, so an output taller than the screen is selected — and copied —
	/// whole, while the scroll only decides which screenful of it the user is looking at. Until §40
	/// the span was viewport rows, which forced the two together and clipped a long output to the
	/// screenful that showed.
	fn locate_output(&mut self, start: u64, end: u64) -> OutputSpan {
		let grid = self.term.grid();
		let history = grid.history_size() as i64;
		let screen_lines = self.term.screen_lines() as i64;
		// Absolute line -> viewport row for a given display offset.
		let to_row = |offset: i64, absolute: u64| absolute as i64 - history + offset;

		// Scroll the first output line to the top only when it is not already on screen.
		let offset = grid.display_offset() as i64;
		if !(0..screen_lines).contains(&to_row(offset, start)) {
			let target = (history - start as i64).clamp(0, history);
			let delta = target - offset;
			if delta != 0 {
				self.term.scroll_display(Scroll::Delta(delta as i32));
			}
		}

		// The range is half-open, so its last line is `end - 1`; `max(start)` keeps the span ordered
		// even for the degenerate `end == start` a shell could in principle mark.
		OutputSpan {
			start_line: start,
			end_line: end.saturating_sub(1).max(start),
			last_col: (self.term.columns() as u16).saturating_sub(1),
		}
	}

	/// Every occurrence of `query` in the whole document — the retained history AND the live screen
	/// (§35) — in document order, oldest line first. Each hit carries an ABSOLUTE line index
	/// (`history_size + row`, the coordinate the OSC 133 marks also use, §34) so it keeps pointing
	/// at its own text as later output pushes the viewport down, plus the grid columns it covers,
	/// which is what a selection addresses.
	///
	/// A row is flattened one glyph per cell, skipping a wide glyph's trailing cell (it holds no
	/// glyph of its own) and trimming the width-padding at the end, then searched ASCII
	/// case-insensitively by `search::Row` — the pure half of this, tested without an engine. The
	/// scan is a full walk of the grid, so it costs `history + rows` × `columns` cell reads; that is
	/// a few million at the SCROLLBACK cap, cheap enough to redo on each keystroke in the find bar
	/// and far simpler than maintaining an index that every scroll and reflow could invalidate.
	///
	/// `ponytail:` matches are found within one grid ROW, so a hit that straddles the wrap of a
	/// long logical line is not found (the two halves are separate rows), and a cell's combining
	/// marks are not searched — only its base glyph. An empty query finds nothing.
	pub fn find(&self, query: &str) -> Vec<search::Match> {
		if query.is_empty() {
			return Vec::new();
		}
		let grid = self.term.grid();
		let history = grid.history_size() as i32;
		let screen_lines = self.term.screen_lines() as i32;
		let columns = self.term.columns();
		let mut out = Vec::new();
		// The engine stores history on the NEGATIVE lines below the active screen's line 0, so the
		// whole document is `-history ..= the last screen line`; absolute = history + line puts line
		// 0 (the top of the active screen) at absolute `history_size`, as `osc133` records it.
		for line in -history..screen_lines {
			let mut row = search::Row::new((history + line) as u64);
			for col in 0..columns {
				let cell = &grid[Line(line)][Column(col)];
				// A wide glyph's trailing half carries no glyph — skipping it (rather than pushing a
				// blank) is why the row keeps a byte -> column map instead of assuming they line up.
				if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
					continue;
				}
				row.push(cell.c, col as u16);
			}
			row.trim_end();
			out.extend(row.find(query));
		}
		out
	}

	/// Scroll the absolute line `absolute` into view, returning whether it could be placed on screen
	/// at all — `false` when its line has scrolled past the retained history, so the caller can leave
	/// both the view and the selection as they were. Used to reveal a search match; the match's own
	/// absolute coordinates are what the caller then selects (§35, §40), so no row comes back.
	///
	/// A line already on screen is left exactly where it is — a step between two matches on the
	/// same screenful must not jerk the view. One that is not is CENTRED rather than put at the
	/// top, so a match arrives with its surrounding output visible on both sides; the target offset
	/// is clamped to the history the engine actually retains, so a match near either end simply
	/// lands as close to the middle as the document allows.
	pub fn reveal_line(&mut self, absolute: u64) -> bool {
		let grid = self.term.grid();
		let history = grid.history_size() as i64;
		let screen_lines = self.term.screen_lines() as i64;
		// Absolute line -> viewport row for a given display offset, the inverse of the mapping
		// `Screen::cell` reads with (§23).
		let to_row = |offset: i64| absolute as i64 - history + offset;

		let offset = grid.display_offset() as i64;
		if !(0..screen_lines).contains(&to_row(offset)) {
			// The offset that puts this line in the middle of the screen: the offset that would put
			// it at the top (history - absolute) plus half a screen of extra climb.
			let target = (history - absolute as i64 + screen_lines / 2).clamp(0, history);
			let delta = target - offset;
			if delta != 0 {
				self.term.scroll_display(Scroll::Delta(delta as i32));
			}
		}

		// Map with the (possibly new) offset — the engine clamps a scroll, so this is the truth
		// about where the line ended up rather than where we aimed it.
		let row = to_row(self.term.grid().display_offset() as i64);
		(0..screen_lines).contains(&row)
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
	/// The remote's xterm `modifyOtherKeys` level (§9), learned from the private-CSI a program
	/// writes to ask for unambiguous Ctrl/Alt key reports. The engine ignores that sequence, so —
	/// like the cwd — the same bytes are scanned here for the key encoder to read.
	modkeys: modkeys::ModKeys,
	/// Sniffs the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP (§33). Its VT
	/// parser treats those as no-ops, so — like the cwd and modkeys — the same bytes are scanned
	/// here, and `process` turns each completed query into a reply.
	queries: query::Queries,
	/// Reads the OSC 133 shell-integration prompt marks (§34), which the engine also ignores. Holds
	/// the command-cycle state, the last exit code, and where each prompt sits, so the GUI can show
	/// a per-tab status glyph and jump between prompts. Unlike the other scanners, `process` feeds
	/// this one by splitting the advance at each mark, so a prompt is recorded at the grid line the
	/// cursor is on when the mark arrives.
	prompts: osc133::Prompts,
	/// Reads the honoured parts of iTerm2's OSC 1337 namespace (§55) — today, the explicit bookmarks
	/// `SetMark` drops. Fed by the same split advance as the prompt marks and for the same reason: a
	/// bookmark's whole meaning is the line it arrived on. Its own module is an ALLOW-LIST, which is
	/// what keeps the dangerous keys of that namespace (a clipboard write, a theme repaint) out.
	iterm: iterm::Iterm,
	/// Reads the progress a remote command reports (OSC 9;4, §54) — another OSC the engine ignores.
	/// Like the cwd, it is a latest-value reading with no place on the grid, so `process` feeds it
	/// the whole chunk and never splits the advance for it.
	progress: progress::Reports,
	/// Finds the inline sixel images the engine drops, decodes them and holds where each one sits
	/// (§41). Fed by the same split advance as the prompt marks, and for the same reason: a picture
	/// belongs at the cursor's line and column at the moment it arrived in the stream.
	graphics: graphics::Images,
	/// Whether the ALTERNATE screen was the one up last time this was looked at (§41). The engine
	/// tracks the swap itself and `Screen::is_alternate` reads it back at any moment; what this adds
	/// is the EDGE. A full-screen program's pictures belong to that program, so both swaps — on and
	/// off the page — throw them away, and noticing that needs the previous answer as well as the
	/// current one.
	on_alternate: bool,
}

/// One thing `process` has to do part-way through a chunk (§34, §41, §55). Each scanner reports the
/// byte offset its event sits at, and the engine can only be advanced forwards, so the lists are
/// merged into this single ordered one — otherwise applying all the marks and then all the images
/// would place the later kinds at the wrong point in the stream.
enum Split {
	Prompt(osc133::Mark),
	Graphics(graphics::Event),
	/// An explicit bookmark a script dropped with `OSC 1337 ; SetMark` (§55). Carries nothing: the
	/// whole content of the event is the line it arrived on, which is why it has to be applied here
	/// rather than after the chunk.
	UserMark,
}

/// Merge one chunk's prompt marks, image events and bookmarks into offset order. Every list arrives
/// ascending, and the sort is stable, so two events at the very same offset keep the order they were
/// scanned in — which is the only sensible tie-break, since no scanner can see another's.
fn splits(
	marks: Vec<(usize, osc133::Mark)>,
	images: Vec<(usize, graphics::Event)>,
	bookmarks: Vec<(usize, iterm::Report)>,
) -> Vec<(usize, Split)> {
	let mut merged: Vec<(usize, Split)> =
		Vec::with_capacity(marks.len() + images.len() + bookmarks.len());
	merged.extend(
		marks
			.into_iter()
			.map(|(offset, mark)| (offset, Split::Prompt(mark))),
	);
	merged.extend(
		images
			.into_iter()
			.map(|(offset, event)| (offset, Split::Graphics(event))),
	);
	merged.extend(bookmarks.into_iter().map(|(offset, report)| {
		let split = match report {
			iterm::Report::Mark => Split::UserMark,
		};
		(offset, split)
	}));
	merged.sort_by_key(|(offset, _)| *offset);
	merged
}

/// Whether any cell of an alternate-page picture's reserved box now holds a glyph (§41) — the test
/// `retire_covered_images` retires a stale picture on.
///
/// The box was blanked when the picture was placed, so anything in it was put there afterwards by the
/// program. The FULL reserved box counts, fringe included: the last row and column are only partly
/// covered by pixels, but they are cells the picture was given, and a terminal drawing the picture
/// into its cells would have erased whatever a program then wrote across them.
///
/// A row off the bottom of the page reads as no cell at all, which is right — a picture reserved
/// against a page that has since been resized smaller has nothing to be covered by.
fn covered(screen: &screen::Screen<'_>, placement: &graphics::Placement) -> bool {
	// The alternate page keeps no history and cannot be scrolled back, so the placement's absolute
	// line IS its viewport row and the cast cannot lose anything: it was built from a `u16` row.
	let top = placement.line as u16;
	(top..top.saturating_add(placement.rows)).any(|row| {
		(placement.col..placement.col.saturating_add(placement.cols)).any(|col| {
			screen
				.cell(row, col)
				.is_some_and(|cell| cell.has_contents())
		})
	})
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

/// Rebuild the current SGR pen as a parameter string for a DECRQSS reply (§33). The pen is the
/// template cell the engine stamps onto every glyph, so reading it back is authoritative — it is
/// exactly what the grid paints, not a guess. The string always opens with `0` (a full reset) and
/// then lists only what is set, so a fresh pen reports `0` and bold-red reports `0;1;31`; the
/// caller frames it as `DCS 1 $ r <this> m ST`.
fn pen_sgr(cell: &Cell) -> String {
	let flags = cell.flags;
	// Open from a clean slate, then append a code for each attribute the pen carries.
	let mut codes = vec![String::from("0")];
	if flags.contains(Flags::BOLD) {
		codes.push("1".to_string());
	}
	if flags.contains(Flags::DIM) {
		codes.push("2".to_string());
	}
	if flags.contains(Flags::ITALIC) {
		codes.push("3".to_string());
	}
	// A double underline has its own code; every other underline variant (curl, dotted, dashed)
	// reports as a plain underline — truthful that the text is underlined, without claiming a
	// substyle that is a terminal-specific extension.
	if flags.contains(Flags::DOUBLE_UNDERLINE) {
		codes.push("21".to_string());
	} else if flags.intersects(
		Flags::UNDERLINE | Flags::UNDERCURL | Flags::DOTTED_UNDERLINE | Flags::DASHED_UNDERLINE,
	) {
		codes.push("4".to_string());
	}
	if flags.contains(Flags::INVERSE) {
		codes.push("7".to_string());
	}
	if flags.contains(Flags::HIDDEN) {
		codes.push("8".to_string());
	}
	if flags.contains(Flags::STRIKEOUT) {
		codes.push("9".to_string());
	}
	if let Some(foreground) = sgr_color(cell.fg, false) {
		codes.push(foreground);
	}
	if let Some(background) = sgr_color(cell.bg, true) {
		codes.push(background);
	}
	codes.join(";")
}

/// One channel of the pen as its SGR colour codes, or `None` when it is the default (which the
/// leading reset already covers). Named 0-7 map to 30-37 / 40-47, bright 8-15 to 90-97 / 100-107,
/// a palette index to `38;5;n` / `48;5;n`, and a truecolor spec to `38;2;r;g;b` / `48;2;r;g;b`.
fn sgr_color(color: Color, is_background: bool) -> Option<String> {
	let (base, bright_base, extended) = if is_background {
		(40, 100, 48)
	} else {
		(30, 90, 38)
	};
	match color {
		// The default foreground/background need no explicit code — the reset stands in for them.
		Color::Named(NamedColor::Foreground | NamedColor::Background) => None,
		Color::Named(named) => {
			let index = named as usize;
			if index <= 7 {
				Some((base + index).to_string())
			} else if (8..=15).contains(&index) {
				Some((bright_base + index - 8).to_string())
			} else {
				// A special role (cursor, dim/bright foreground): not an SGR colour, so default.
				None
			}
		}
		Color::Indexed(index) => Some(format!("{extended};5;{index}")),
		Color::Spec(rgb) => Some(format!("{extended};2;{};{};{}", rgb.r, rgb.g, rgb.b)),
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
	fn the_kitty_keyboard_query_is_answered_once_a_program_enables_it() {
		// With `config.kitty_keyboard` on, the engine tracks the pushed-flags stack and answers
		// the `CSI ? u` query itself (§25). Before any program pushes a mode the active set is
		// empty, so the report is flags 0; after pushing the disambiguate flag (`CSI > 1 u`) the
		// report names it. This is the whole reason cmote needs no scanner of its own here — the
		// engine owns the state, and cmote only reads it back through the seam to drive encoding.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[?u"), b"\x1b[?0u".to_vec());
		terminal.process(b"\x1b[>1u");
		assert_eq!(terminal.process(b"\x1b[?u"), b"\x1b[?1u".to_vec());
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

	#[test]
	fn a_version_query_is_answered_with_cmote_identity() {
		// XTVERSION `CSI > q` -> `DCS > | cmote(<version>) ST`. The engine drops the query, so this
		// reply comes entirely from cmote's out-of-band scanner (§33).
		let mut terminal = Terminal::new(10, 40);
		let mut expected = b"\x1bP>|".to_vec();
		expected.extend_from_slice(VERSION.as_bytes());
		expected.extend_from_slice(b"\x1b\\");
		assert_eq!(terminal.process(b"\x1b[>q"), expected);
	}

	#[test]
	fn a_decrqss_sgr_query_reports_the_current_pen() {
		// A fresh pen is a full reset, so DECRQSS for SGR reports `0`: `DCS 1 $ r 0 m ST`.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0m\x1b\\".to_vec()
		);
		// After a program sets bold + red foreground, the same query reports `0;1;31`, rebuilt
		// from the very pen the grid paints with.
		terminal.process(b"\x1b[1;31m");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	#[test]
	fn a_decrqss_sgr_query_sees_attributes_set_in_the_same_chunk() {
		// The reply is built AFTER the chunk is advanced, so an SGR change that precedes the query
		// in the SAME write is reflected — the common case of a program setting a pen then asking.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b[1;31m\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	#[test]
	fn an_unsupported_decrqss_query_is_answered_with_a_zero_status() {
		// Scroll margins (`r`) are not exposed by the engine, so the honest reply is the invalid
		// status `DCS 0 $ r ST` — enough to stop the program waiting, without lying about state.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1bP$qr\x1b\\"),
			b"\x1bP0$r\x1b\\".to_vec()
		);
	}

	#[test]
	fn a_tertiary_attributes_query_is_answered_with_a_constant_unit_id() {
		// DA3 `CSI = c` -> `DCS ! | 00434D45 ST` (§36). The engine answers DA1 and DA2 but drops
		// the `=` form, so this reply comes from cmote's scanner — and the id is the same constant
		// on every install, deliberately not a per-machine value a host could fingerprint.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b[=c"),
			b"\x1bP!|00434D45\x1b\\".to_vec()
		);
		// A second query gets the identical answer — nothing about it drifts with state.
		assert_eq!(
			terminal.process(b"\x1b[=c"),
			b"\x1bP!|00434D45\x1b\\".to_vec()
		);
	}

	#[test]
	fn an_xtgettcap_query_reports_the_terminal_name() {
		// `TN` (hex 544E) -> `xterm-256color`, the name cmote requested for the remote pty, framed
		// as a valid `DCS 1 + r <name>=<value> ST` with both sides upper-case hex.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1bP+q544E\x1b\\"),
			b"\x1bP1+r544E=787465726D2D323536636F6C6F72\x1b\\".to_vec()
		);
	}

	#[test]
	fn a_shell_integration_cycle_tracks_state_and_exit() {
		// One command bracketed by OSC 133 marks (§34): a prompt shows, the command runs, then it
		// finishes with an exit code — each mark moving the state the status glyph reads.
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.command_state(), osc133::CommandState::Idle);
		terminal.process(b"\x1b]133;A\x07user@host:~$ ");
		assert_eq!(terminal.command_state(), osc133::CommandState::Prompt);
		terminal.process(b"\x1b]133;B\x07ls\r\n\x1b]133;C\x07");
		assert_eq!(terminal.command_state(), osc133::CommandState::Running);
		terminal.process(b"a.txt\r\n\x1b]133;D;0\x07");
		assert_eq!(terminal.command_state(), osc133::CommandState::Idle);
		assert_eq!(terminal.last_exit(), Some(0));
	}

	#[test]
	fn a_prompt_is_anchored_to_the_line_the_cursor_is_on() {
		// The split-advance is the whole point: the prompt mark must land on the line the cursor
		// is on when OSC 133;A arrives, not where the chunk happens to end. Two banner lines, then
		// a prompt on the third — its tick sits at viewport row 2.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"line one\r\nline two\r\n\x1b]133;A\x07$ ");
		assert_eq!(terminal.prompt_rows(), vec![2]);
	}

	#[test]
	fn a_bookmark_is_anchored_to_the_line_the_cursor_is_on() {
		// §55, and the same split-advance point as the prompt test above: `OSC 1337 ; SetMark` has to
		// land on the line the cursor is on when it ARRIVES. Two lines of build output, then a mark
		// before the third stage — its tick sits at viewport row 2, in its own list.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"stage one\r\nstage two\r\n\x1b]1337;SetMark\x07stage three");
		assert_eq!(terminal.user_mark_rows(), vec![2]);
		// And it is not mistaken for a prompt, which would tick it in the wrong colour and give it a
		// command's output span it does not have.
		assert!(terminal.prompt_rows().is_empty());
	}

	#[test]
	fn a_bookmark_and_a_prompt_in_one_chunk_each_land_on_their_own_line() {
		// The ordering the merged split list exists for: both scanners report offsets into the same
		// chunk, and the engine only advances forwards, so applying all of one kind and then the
		// other would put the second kind on the wrong line.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]133;A\x07$ make\r\nbuilding\r\n\x1b]1337;SetMark\x07linking");
		assert_eq!(terminal.prompt_rows(), vec![0]);
		assert_eq!(terminal.user_mark_rows(), vec![2]);
	}

	#[test]
	fn iterm2s_current_dir_is_followed_like_the_other_two_spellings() {
		// §55: a dotfile written for iTerm2 announces the cwd with OSC 1337 and nothing else.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]1337;CurrentDir=/srv/app\x07$ ");
		assert_eq!(terminal.cwd(), Some("/srv/app"));
	}

	#[test]
	fn a_finished_commands_output_is_located_for_selection() {
		// One command bracketed by OSC 133 marks with two lines of output (§34): the prompt and
		// its echoed input on line 0, then `\r\n`, the C mark, output on lines 1 and 2, `\r\n`, the D
		// mark on line 3. Nothing has scrolled off, so the output span [1, 3) is document lines 1..=2.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07one\r\ntwo\r\n\x1b]133;D;0\x07",
		);
		let span = terminal
			.select_output_back()
			.expect("a finished command with output");
		assert_eq!((span.start_line, span.end_line), (1, 2));
		assert_eq!(span.last_col, 39);
		// Clicking that command's prompt tick (viewport row 0) resolves to the same output.
		let clicked = terminal
			.select_output_at_row(0)
			.expect("the prompt tick's command");
		assert_eq!((clicked.start_line, clicked.end_line), (1, 2));
	}

	/// An output taller than the screen is located WHOLE (§40): the span is document lines, so the
	/// selection covers every line the command printed and not just the screenful that shows. This is
	/// the limit §34 shipped with and had to write down as deferred.
	#[test]
	fn an_output_taller_than_the_screen_is_located_whole() {
		// A four-row screen; the command prints ten lines, so most of its output is above the top by
		// the time the D mark lands.
		let mut terminal = Terminal::new(4, 40);
		terminal.process(b"\x1b]133;A\x07$ \x1b]133;B\x07seq\r\n\x1b]133;C\x07");
		let output: Vec<u8> = (0..10).flat_map(|_| b"line\r\n".to_vec()).collect();
		terminal.process(&output);
		terminal.process(b"\x1b]133;D;0\x07");

		let span = terminal
			.select_output_back()
			.expect("a finished command with output");
		assert_eq!(
			span.end_line - span.start_line + 1,
			10,
			"every printed line is in the span, not just the visible four"
		);
	}

	#[test]
	fn a_second_press_walks_back_to_the_command_before_it() {
		// Two commands run one after the other (§34). Ctrl+Shift+O takes the newer, pressing again
		// takes the older, and a press on the grid — `restart_output_walk` — puts the key back at
		// the newest, which is what stops the walk quietly carrying on into a later session's work.
		let mut terminal = Terminal::new(20, 40);
		terminal.process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07one\r\n\x1b]133;C\x07first\r\n\x1b]133;D;0\x07",
		);
		terminal.process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07two\r\n\x1b]133;C\x07second\r\n\x1b]133;D;0\x07",
		);
		let newest = terminal
			.select_output_back()
			.expect("the command just finished");
		let older = terminal
			.select_output_back()
			.expect("the command before it");
		assert!(
			older.start_line < newest.start_line,
			"the second press steps BACK: line {} then {}",
			newest.start_line,
			older.start_line
		);
		terminal.restart_output_walk();
		let again = terminal.select_output_back().expect("the newest again");
		assert_eq!(
			(again.start_line, again.end_line),
			(newest.start_line, newest.end_line)
		);
	}

	#[test]
	fn a_command_that_printed_nothing_locates_no_output() {
		// A bare Enter at the prompt: A, B, then D with no output in between (§34). There is no
		// output line-span, so nothing is offered to select.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]133;A\x07$ \x1b]133;B\x07\r\n\x1b]133;D;0\x07");
		assert!(terminal.select_output_back().is_none());
	}

	#[test]
	fn revealing_output_scrolls_it_into_view_when_it_is_off_screen() {
		// A command's output on a small screen, then enough later output to push it up into history.
		// Selecting it from the live bottom must scroll the viewport up so the output shows — and the
		// span's first line must be one of the visible rows once it has (§40: the span itself is
		// document lines, so "revealed" is a fact about the viewport, not about the span).
		let mut terminal = Terminal::new(4, 40);
		terminal.process(
			b"\x1b]133;A\x07$ \x1b]133;B\x07cmd\r\n\x1b]133;C\x07result\r\n\x1b]133;D;0\x07",
		);
		let filler: Vec<u8> = (0..20).flat_map(|_| b"later\r\n".to_vec()).collect();
		terminal.process(&filler);
		assert_eq!(
			terminal.screen().display_offset(),
			0,
			"starts at the live bottom"
		);
		let span = terminal
			.select_output_back()
			.expect("the earlier command's output");
		assert!(
			terminal.screen().display_offset() > 0,
			"scrolled up to reveal the output"
		);
		let screen = terminal.screen();
		assert!(
			(0..4).any(|row| screen.line_at(row) == span.start_line),
			"the output's first line is showing on one of the visible rows"
		);
	}

	/// The find bar searches the WHOLE document, not just what is visible (§35): on a 2-row screen
	/// fed five lines, a hit on the first (long since scrolled off) is still found, at the absolute
	/// line it sits on and the columns it covers.
	#[test]
	fn the_search_reaches_lines_that_scrolled_off_the_screen() {
		let mut terminal = Terminal::new(2, 20);
		terminal.process(b"alpha\r\nbravo\r\ncharlie\r\ndelta\r\nalpha again");

		// Three lines scrolled off, so the retained history is 3 and absolute line 0 is "alpha".
		assert_eq!(terminal.screen().history_size(), 3);
		let hits = terminal.find("alpha");
		assert_eq!(
			hits,
			vec![
				search::Match {
					line: 0,
					start_col: 0,
					end_col: 4
				},
				search::Match {
					line: 4,
					start_col: 0,
					end_col: 4
				},
			]
		);
		// The query is matched case-insensitively, and a query nothing carries finds nothing.
		assert_eq!(terminal.find("BRAVO").len(), 1);
		assert!(terminal.find("nowhere").is_empty());
	}

	/// A sixel DCS painting a solid red rectangle `width` pixels wide and `bands * 6` tall — the
	/// smallest real picture a program could send, and enough to pin down every bit of geometry the
	/// placement and the reservation depend on.
	fn sixel_image(width: u16, bands: u16) -> Vec<u8> {
		let mut out = b"\x1bPq#0;2;100;0;0".to_vec();
		for band in 0..bands {
			if band > 0 {
				out.push(b'-');
			}
			out.extend_from_slice(format!("!{width}~").as_bytes());
		}
		out.extend_from_slice(b"\x1b\\");
		out
	}

	/// A picture is anchored where the cursor was and reserves the cells it covers (§41), leaving the
	/// cursor on the line below it — so the shell's next prompt lands under the image, not over it.
	#[test]
	fn a_sixel_image_is_placed_where_the_cursor_was_and_reserves_its_box() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"top\r\n");
		// 21×30 pixels in a 7×14 cell: three columns and three rows, both rounded up.
		terminal.process(&sixel_image(21, 5));

		let placement = &terminal.images()[0];
		assert_eq!((placement.line, placement.col), (1, 0));
		assert_eq!((placement.rows, placement.cols), (3, 3));
		assert_eq!((placement.width, placement.height), (21, 30));
		// The reservation moved the cursor past the picture's three rows, back at the left margin.
		terminal.process(b"after");
		assert_eq!(read(&terminal, 4, 0, 5), "after");
	}

	/// The reserved box is ERASED, and only the box (§41): a picture drawn over existing text blanks
	/// the cells it covers and leaves the rest of those rows alone.
	#[test]
	fn the_cells_an_image_covers_are_erased() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"AAAAAAAAAA\r\nBBBBBBBBBB\r\nCCCCCCCCCC\x1b[H");
		terminal.process(&sixel_image(21, 5));

		// Three columns of each of the three covered rows are gone; the seven after them are not.
		assert_eq!(read(&terminal, 0, 0, 10), "AAAAAAA");
		assert_eq!(read(&terminal, 2, 0, 10), "CCCCCCC");
	}

	/// An image's anchor is a DOCUMENT line (§40, §41), so later output scrolls the picture up the
	/// screen and into the scrollback without anything having to move it.
	#[test]
	fn an_image_keeps_its_line_as_output_pushes_it_into_history() {
		let mut terminal = Terminal::new(4, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(&sixel_image(7, 1));
		assert_eq!(terminal.images()[0].line, 0);

		let filler: Vec<u8> = (0..20).flat_map(|_| b"line\r\n".to_vec()).collect();
		terminal.process(&filler);
		assert_eq!(
			terminal.images()[0].line,
			0,
			"the anchor names the document, which has not changed"
		);
		assert!(
			terminal.screen().line_at(0) > 0,
			"while the viewport has moved a long way past it"
		);
	}

	/// The alternate screen has its own page of pictures (§41). It keeps no history, so a placement's
	/// absolute line there is simply the row the cursor was on — the same coordinate, read against a
	/// document one screen tall.
	///
	/// The session is given a deep scrollback FIRST, because that is the claim the whole design rests
	/// on: the renderer resolves a picture against `line_at(0)`, so if the alternate page reported the
	/// primary screen's history rather than none of its own, every picture on it would be drawn
	/// hundreds of rows adrift.
	#[test]
	fn a_sixel_on_the_alternate_screen_is_placed_on_its_row() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		let filler: Vec<u8> = (0..40).flat_map(|_| b"line\r\n".to_vec()).collect();
		terminal.process(&filler);
		assert!(terminal.screen().history_size() > 0, "a real scrollback");

		terminal.process(b"\x1b[?1049h");
		assert_eq!(
			terminal.screen().line_at(0),
			0,
			"the page's top row IS document line 0 — no history behind it"
		);
		// Row 3, column 5 in the program's own 1-based coordinates, so row 2, column 4 in ours.
		terminal.process(b"\x1b[3;5H");
		terminal.process(&sixel_image(21, 5));

		let placement = &terminal.images()[0];
		assert_eq!((placement.line, placement.col), (2, 4));
		assert_eq!((placement.rows, placement.cols), (3, 3));
	}

	/// Reserving cells on the alternate page must never SCROLL it (§41). A page with no history throws
	/// a scrolled-off row away for good and drags every other picture's anchor row out from under it,
	/// and a picture reaching the bottom is the normal case, not the corner one — `mpv --vo=sixel`
	/// draws a full-screen image every frame. So the rows are stepped with CUD, which stops at the
	/// margin, rather than with LF, which scrolls at it.
	#[test]
	fn reserving_on_the_alternate_page_never_scrolls_it() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"\x1b[?1049h");
		terminal.process(b"\x1b[1;1Htop");
		// A three-row picture anchored on row 8 of ten: its box runs off the bottom of the page.
		terminal.process(b"\x1b[9;1H");
		terminal.process(&sixel_image(21, 5));

		assert_eq!(read(&terminal, 0, 0, 3), "top", "the page did not move");
		assert_eq!(terminal.images()[0].line, 8);
	}

	/// A full-screen program's pictures belong to that program (§41): both swaps take them, and
	/// neither touches the scrollback's. Quitting `ranger` leaves every plot in the session's history
	/// exactly where it was — and starting it again shows it an empty page, not the last program's.
	#[test]
	fn a_screen_swap_takes_the_alternate_pictures_and_leaves_the_others() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(&sixel_image(21, 5));
		assert_eq!(terminal.images().len(), 1, "one on the primary screen");

		terminal.process(b"\x1b[?1049h");
		assert!(terminal.images().is_empty(), "the page starts blank");
		terminal.process(&sixel_image(21, 5));
		assert_eq!(terminal.images().len(), 1, "the program's own picture");

		terminal.process(b"\x1b[?1049l");
		assert_eq!(
			terminal.images().len(),
			1,
			"the primary screen's, still there"
		);
		assert_eq!(terminal.images()[0].line, 0);

		terminal.process(b"\x1b[?1049h");
		assert!(
			terminal.images().is_empty(),
			"and the next program is shown nobody else's screen"
		);
	}

	/// `CSI 2 J` on the alternate page takes ALL of its pictures — there is no history there for the
	/// erase to spare, which is the one place the two pages' rules differ (§41). `CSI 3 J` says nothing
	/// about a scrollback that does not exist, so the page is left alone.
	#[test]
	fn erasing_the_alternate_screen_takes_every_picture_on_it() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"\x1b[?1049h");
		terminal.process(&sixel_image(21, 5));
		terminal.process(b"\x1b[3J");
		assert_eq!(terminal.images().len(), 1, "no scrollback to clear");
		terminal.process(b"\x1b[2J");
		assert!(terminal.images().is_empty());
	}

	/// Text drawn over an alternate-page picture retires it (§41) — the closest cmote gets to what a
	/// terminal with native graphics has for free, where the pixels live in the cells and writing a
	/// character erases them. `ranger` moving from an image preview to a text one is exactly this: it
	/// repaints the pane in place, with no erase and no swap to announce it.
	#[test]
	fn text_drawn_over_an_alternate_picture_retires_it() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"\x1b[?1049h");
		terminal.process(b"\x1b[1;1H");
		terminal.process(&sixel_image(21, 5));
		// Just outside the three-by-three box the picture reserved: not its business.
		terminal.process(b"\x1b[4;1Hbelow");
		terminal.process(b"\x1b[1;4Hright");
		assert_eq!(terminal.images().len(), 1, "neither one covers it");

		terminal.process(b"\x1b[2;2Hx");
		assert!(terminal.images().is_empty());
	}

	/// A chunk that PLACED a picture sits the sweep out, so a program writing its image and the rest of
	/// its frame in one go does not blank its own picture the instant it arrives (§41). The cost is a
	/// chunk's delay in noticing — a frame nobody sees.
	#[test]
	fn a_picture_is_not_retired_by_the_chunk_that_placed_it() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"\x1b[?1049h");
		let mut frame = b"\x1b[1;1H".to_vec();
		frame.extend_from_slice(&sixel_image(21, 5));
		frame.extend_from_slice(b"\x1b[2;2Hx");
		terminal.process(&frame);
		assert_eq!(terminal.images().len(), 1, "drawn, then written over");

		terminal.process(b"");
		assert!(
			terminal.images().is_empty(),
			"and retired on the next chunk"
		);
	}

	/// An erase takes only the pictures whose lines it erases (§41): `CSI 2 J` clears the screen, so a
	/// plot further up the scrollback survives it, and `CSI 3 J` clears the scrollback, so that one
	/// goes instead. A shell's `clear` sends both and leaves nothing.
	#[test]
	fn erasing_the_screen_leaves_the_pictures_in_history_alone() {
		let mut terminal = Terminal::new(4, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(&sixel_image(7, 1));
		let filler: Vec<u8> = (0..10).flat_map(|_| b"line\r\n".to_vec()).collect();
		terminal.process(&filler);
		terminal.process(&sixel_image(7, 1));
		assert_eq!(terminal.images().len(), 2);

		terminal.process(b"\x1b[2J");
		assert_eq!(terminal.images().len(), 1, "only the one on screen went");
		assert_eq!(terminal.images()[0].line, 0, "the one in history stayed");
		terminal.process(b"\x1b[3J");
		assert!(terminal.images().is_empty());
	}

	/// A reset starts the session over, and a resize reflows the document out from under every
	/// anchor: both drop the pictures whole (§41, the trade-off §34's prompt marks already make).
	#[test]
	fn a_reset_or_a_resize_drops_every_picture() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(&sixel_image(21, 5));
		assert_eq!(terminal.images().len(), 1);
		terminal.process(b"\x1bc");
		assert!(terminal.images().is_empty(), "RIS clears them");

		terminal.process(&sixel_image(21, 5));
		assert_eq!(terminal.images().len(), 1);
		terminal.resize(20, 60);
		assert!(terminal.images().is_empty(), "a reflow clears them");
	}

	/// Two scanners can both fire inside one chunk, and the engine can only be advanced forwards, so
	/// their events are applied in stream order (§41): the prompt mark after this picture lands on the
	/// line BELOW it, which is only true if the image's reservation was applied first.
	#[test]
	fn a_picture_and_a_prompt_mark_in_one_chunk_are_applied_in_order() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		let mut chunk = b"top\r\n".to_vec();
		chunk.extend_from_slice(&sixel_image(21, 5));
		chunk.extend_from_slice(b"\x1b]133;A\x07$ ");
		terminal.process(&chunk);

		assert_eq!(terminal.images()[0].line, 1);
		assert_eq!(
			terminal.prompt_rows(),
			vec![4],
			"the prompt is below the picture, not on it"
		);
	}

	/// DA1 now advertises sixel (§41). The engine writes `CSI ? 6 c` and knows nothing of images, so
	/// cmote amends its own reply on the way out — without that `4`, the programs that pick a picture
	/// format at startup fall back to text art and the images go unused.
	#[test]
	fn the_device_attributes_reply_advertises_sixel() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[c"), b"\x1b[?6;4c".to_vec());
	}

	/// XTSMGRAPHICS is answered from the limits the decoder actually enforces (§41), so a program
	/// sizing a picture for cmote is told the truth: the colour register count, then the largest image
	/// it will accept.
	#[test]
	fn a_graphics_capability_query_reports_the_decoders_limits() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(
			terminal.process(b"\x1b[?1;1S"),
			format!("\x1b[?1;0;{}S", sixel::COLOR_REGISTERS).into_bytes()
		);
		assert_eq!(
			terminal.process(b"\x1b[?2;4S"),
			format!("\x1b[?2;0;{};{}S", sixel::MAX_WIDTH, sixel::MAX_HEIGHT).into_bytes()
		);
	}

	/// A picture cmote will not decode leaves the screen exactly as it was (§41): no placement, and —
	/// because the reservation is driven by the placement — no reserved cells either, so nothing about
	/// the text moves.
	#[test]
	fn a_refused_picture_reserves_nothing() {
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(7, 14);
		terminal.process(b"top\r\n");
		// A raster attribute far past the decoder's caps: refused whole (§12).
		terminal.process(b"\x1bPq\"1;1;9000;9000#0;2;100;0;0~\x1b\\");
		assert!(terminal.images().is_empty());
		terminal.process(b"after");
		assert_eq!(read(&terminal, 1, 0, 5), "after", "the cursor never moved");
	}

	/// Revealing a match scrolls it onto the screen when it is off it, and leaves the view alone
	/// when it is already showing (§35) — a step between two hits on one screenful must not jerk.
	#[test]
	fn revealing_a_match_scrolls_only_when_it_is_off_screen() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"needle\r\n");
		let filler: Vec<u8> = (0..30).flat_map(|_| b"filler\r\n".to_vec()).collect();
		terminal.process(&filler);
		let hit = *terminal.find("needle").first().expect("the hit is found");
		assert_eq!(terminal.screen().display_offset(), 0, "at the live bottom");

		// Off screen: the view climbs into history until the hit's line is one of the visible rows.
		assert!(terminal.reveal_line(hit.line), "revealed on screen");
		let offset = terminal.screen().display_offset();
		assert!(offset > 0, "scrolled up to reveal the match");
		let screen = terminal.screen();
		assert!(
			(0..4).any(|row| screen.line_at(row) == hit.line),
			"the hit's line is showing on one of the visible rows"
		);

		// Already visible: nothing moves.
		assert!(terminal.reveal_line(hit.line));
		assert_eq!(
			terminal.screen().display_offset(),
			offset,
			"a visible match is revealed in place"
		);
	}
}

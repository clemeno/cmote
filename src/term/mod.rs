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
//
// One sequence goes the other way — the engine does NOT drop it, and should. `CSI Pl;Pr s` sets the
// left and right margins on a VT420, and the engine's arm for the final `s` is save-cursor, which
// ignores its parameters; the margins cmote does not offer would come at the cost of a saved cursor
// the program never asked it to overwrite. So `cancel` finds that final byte and `process` feeds the
// engine a CAN in place of it (§57).

mod cancel; // stops the one sequence the engine would read as something else — DECSLRM (§57)
pub mod cwd; // tracks the remote working directory announced by the shell (§17)
pub mod graphics; // finds the inline images the engine drops, and anchors them to the document (§41)
mod icon; // reads the icon name a remote sets, OSC 1, for the tab chip to wear (§69)
pub mod iterm; // reads the parts of iTerm2's OSC 1337 namespace cmote honours — an allow-list (§55)
pub mod keymap; // maps GUI key events to the bytes a terminal sends
pub mod kitty; // encodes key events in the kitty keyboard protocol's CSI u form (§25)
pub mod modkeys; // tracks the remote's xterm modifyOtherKeys mode for the key encoder (§9)
pub mod mouse; // maps pointer events to the reports a mouse-aware program expects
mod osc; // frames OSC strings out of the stream for the scanners below, and sanitises what they keep (§17, §34, §54, §55, §69)
pub mod osc133; // reads the shell-integration prompt marks the engine ignores (§34)
pub mod progress; // reads the progress a remote command reports, OSC 9;4 (§54)
mod protect; // reads the selective-erase sequences the engine drops — DECSCA, DECSED, DECSEL (§56)
mod query; // answers the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS (§33, §36, §41)
mod rect; // reads the VT420 rectangular area operations the engine drops — DECERA, DECSERA, DECFRA, DECCRA (§58), DECCARA, DECRARA, DECSACE (§59)
pub mod screen; // the engine-agnostic view of the screen the app reads through (§9, §16, §23)
pub mod search; // finds text anywhere in the scrollback for the find bar (§35)
pub mod sixel; // decodes a sixel image's payload into pixels (§41)

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Osc52, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

use crate::palette;

/// The pty size the client requests and the emulator starts at, before the first
/// window measurement arrives (§9). Kept here as the single source of truth so
/// the ssh client (which requests the initial pty) and the emulator (which lays
/// out the grid) can never disagree; the grid is then reflowed to the real window
/// size via `resize` + `SshCommand::Resize`.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

/// A build-time guard on the one bit cmote borrows inside the engine's per-cell flag word to mean
/// "the program protected this cell from a selective erase" (§56, `protect::PROTECTED_BIT`).
///
/// The engine names fifteen flags in a `u16` and leaves bit 15 free, which is what lets protection
/// ride the grid as an ordinary attribute (see `term/protect.rs`). Nothing stops a future
/// `alacritty_terminal` from adding a sixteenth, so the collision is caught here instead: a version
/// bump that claims the bit fails the BUILD, rather than shipping as text that cannot be erased and
/// a colour that comes out wrong — a symptom nobody would trace back to this line.
const _: () = assert!(
	Flags::all().bits() & protect::PROTECTED_BIT == 0,
	"the engine has claimed the flag bit cmote borrows for DECSCA protection — pick another in term/protect.rs"
);

/// The engine flag behind each attribute DECCARA and DECRARA can name (§59).
///
/// `term/rect.rs` folds a selector list into a mask of its own four bits, and this is the one place
/// those bits meet the engine's names — which keeps the grammar testable without a terminal and puts
/// the translation where the engine types already live.
///
/// **Blink is missing on purpose.** `alacritty_terminal`'s flag word has no bit for it: the fifteen
/// it names cover inverse, bold, italic, dim, hidden, strikeout, five underline styles and the
/// wide-character marks, and nothing blinks. So DECCARA's `5` / `25` and DECRARA's `5` are parsed,
/// accepted and then quietly dropped here — the same call cmote already makes for DECSCUSR's
/// blinking cursor shapes (§2), and the honest one while there is nothing to store it in. A program
/// that asks for blink and underline together still gets its underline.
const RECT_ATTRIBUTES: [(u8, Flags); 3] = [
	(rect::BOLD, Flags::BOLD),
	(rect::UNDERLINE, Flags::UNDERLINE),
	(rect::REVERSE, Flags::INVERSE),
];

/// What each attribute adds to a cell's weight in a DECRQCRA checksum (§60), in xterm's numbers.
///
/// A different table from `RECT_ATTRIBUTES` above and deliberately so: that one is the four
/// attributes DECCARA can NAME, this one is the six that CHANGE THE NUMBER, and the two sets only
/// look alike. Folding them together would tie a report to a request, and the next time either moved
/// the other would follow it silently.
///
/// These weights are not cmote's to choose. They are `xtermCheckRect`'s, which is where every
/// conformance suite's expected digits come from — so they are copied, and the reason they are
/// copied is written down in `term/rect.rs` rather than repeated here. DECSCA protection is the one
/// missing from this list, because it does not live in `Flags`: it rides bit 15 and is read through
/// `protect::is_protected`, weighing 0x04. Blink is missing because the engine has no bit for it at
/// all (§59), so 0x40 can never land — the honest hole, in the same place as the last one.
const CHECKSUM_ATTRIBUTES: [(Flags, u32); 4] = [
	(Flags::HIDDEN, 0x08),
	(Flags::UNDERLINE, 0x10),
	(Flags::INVERSE, 0x20),
	(Flags::BOLD, 0x80),
];

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

/// DECSTR, the soft reset, written in the sequences the engine itself handles (§72).
///
/// `CSI ! p` reaches nothing in `vte`, so `Terminal::soft_reset` feeds the engine this instead: the
/// pen, the cursor's visibility, insert/replace, origin, autowrap, the cursor-key mode, the keypad,
/// all four character-set slots plus the active one, the scrolling region, and finally the SAVED
/// cursor — which `ESC 7` puts at home with the pen this string has just reset, DEC's own definition
/// of the item. `soft_reset` appends the CUP that puts the real cursor back, since `CSI r` homes it.
/// Every byte of it is a sequence the engine has an arm for, which is the point: the engine remains
/// the only writer of its own state (see `soft_reset` for the two departures from DEC's list).
const SOFT_RESET: &[u8] =
	b"\x1b[0m\x1b[?25h\x1b[4l\x1b[?6l\x1b[?7h\x1b[?1l\x1b>\x1b(B\x1b)B\x1b*B\x1b+B\x0f\x1b[r\x1b[H\x1b7";

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

/// The settings cmote runs the engine with. A named function rather than a literal buried inside
/// `Terminal::new`, so that a test can read it back: every field here overrides an
/// `alacritty_terminal` default on purpose, and two of them are decisions this project argued at
/// length. A decision nothing checks is a decision that leaves quietly on the next crate bump —
/// which is exactly the failure §62 of TERMINAL_COMPATIBILITY_PLAN went looking for.
fn engine_config() -> Config {
	Config {
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
		// Refuse the remote clipboard AT THE BOUNDARY, in both directions (§6, §12, §63).
		//
		// `Config::default()` leaves this at `Osc52::OnlyCopy`, which upstream chose as "a
		// compromise between entirely disabling it (the most secure) and allowing paste". A
		// compromise is not a refusal. Under that default a remote's `OSC 52` *write* was parsed,
		// base64 and all, and raised as an `Event::ClipboardStore`; the only thing that kept it
		// off the user's clipboard was the catch-all arm of `Replies::send_event` discarding an
		// event it does not recognise. That drop is still there and still correct, but a
		// fall-through cannot be read as a decision — nothing in it says "this is refused", so
		// nothing fails if a future edit starts handling the event.
		//
		// `Disabled` makes the engine return before an event exists: a remote may not poison the
		// local clipboard, and may not read what the user's other applications put there. cmote
		// touches the clipboard only on an explicit LOCAL action. The refusal is now stated in one
		// place, in the same file as the listener that used to carry it alone, and pinned by a test.
		osc52: Osc52::Disabled,
		..Config::default()
	}
}

impl Terminal {
	/// Create an emulator with a `rows`×`cols` grid, matching the remote pty.
	pub fn new(rows: u16, cols: u16) -> Self {
		let replies = Arc::new(Mutex::new(ReplyBuffer {
			rows,
			cols,
			..ReplyBuffer::default()
		}));
		let term = Term::new(
			engine_config(),
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
			icon: icon::Icon::default(),
			protect: protect::Protect::default(),
			cancels: cancel::Cancel::default(),
			rectangles: rect::Rectangles::default(),
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
		// The modifyOtherKeys level, and any question about it (§9, §61). The question is answered
		// by the tracker itself, at the point in the stream it sits, because the tracker is the one
		// thing that holds the answer — so the bytes come back here already formatted.
		let modkeys_reply = self.modkeys.feed(bytes);
		// The progress a remote command reports (§54). Like the cwd, it is a latest-value reading
		// with no position on the grid, so it needs no split in the advance below — only the order
		// the reports arrive in, which its own scanner keeps.
		self.progress.feed(bytes);
		// The icon name a program set for its tab (OSC 1, §69). Another latest-value reading with no
		// position on the grid, so it needs no split either — and `vte` has no arm for the code, so
		// this scanner is the only thing in cmote that ever sees it.
		self.icon.feed(bytes);
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
		// The selective-erase sequences the engine drops (§56). Split-fed like the marks, but its
		// offsets sit one PAST each sequence, because a pen change has to be applied after the SGR
		// that wiped it and an erase after the engine has ignored it. An unarmed stream that sends no
		// `?`-erase reports nothing, so the common case still pays for no split.
		let protections = self.protect.feed(bytes);
		// The one sequence the engine reads as something else (§57). Unlike every scanner above, this
		// one is not reporting something to apply — it is reporting a byte the engine must not be let
		// near, so its offset is the final byte itself and the split loop steps OVER it.
		let cancels = self.cancels.feed(bytes);
		// The VT420 rectangular operations the engine drops (§58, §59). Split-fed like the selective
		// erase and with the same one-past offsets: these name their own coordinates and never touch
		// the cursor, so the split is only about the order they land in against the text around them.
		let rectangles = self.rectangles.feed(bytes);
		// Whether this chunk put a picture on the alternate page — the one thing that makes the
		// covered-cell sweep below sit the chunk out (see `retire_covered_images`).
		let mut placed_on_alternate = false;
		if marks.is_empty()
			&& images.is_empty()
			&& bookmarks.is_empty()
			&& protections.is_empty()
			&& cancels.is_empty()
			&& rectangles.is_empty()
		{
			self.parser.advance(&mut self.term, bytes);
		} else {
			let mut start = 0;
			for (offset, split) in
				splits(marks, images, bookmarks, protections, cancels, rectangles)
			{
				// `start` can already be past this offset, because a cancelled final byte was stepped
				// over just now (see `Split::Cancel`). No scanner can report an event INSIDE a CSI
				// sequence, so nothing is ever skipped by this clamp — it only keeps the slice below
				// from being built backwards.
				let offset = offset.max(start);
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
					Split::Protect(request) => self.apply_protection(request),
					// End the sequence instead of letting the engine dispatch it, and step over the
					// final byte so it is neither dispatched nor printed. Feeding nothing here would
					// leave the engine's parser mid-CSI, waiting to take the next final byte in the
					// stream as this sequence's — see `term/cancel.rs` for what that costs.
					Split::Cancel => {
						self.parser.advance(&mut self.term, &[cancel::CANCEL]);
						start += 1;
					}
					Split::Rect(request) => self.apply_rectangle(request),
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
		// The XTQMODKEYS answer, already built by the tracker that owns the level (§61). It joins
		// the other cmote-originated replies rather than the engine's, which is where it belongs:
		// the engine parsed the question and dropped it.
		out.extend_from_slice(&modkeys_reply);
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

	/// Carry out one selective-erase request (§56), with the engine already advanced past the
	/// sequence that carried it.
	fn apply_protection(&mut self, request: protect::Request) {
		match request {
			protect::Request::Protect(on) => self.set_pen_protection(on),
			// The SGR just applied may have assigned the pen's whole flag word, so put the bit back.
			// Idempotent, which is why the scanner is free to over-report (see `term/protect.rs`).
			protect::Request::Reassert => self.set_pen_protection(true),
			protect::Request::Erase(erase) => self.selective_erase(erase),
			// DECSTR (§72). The borrowed protection bit goes first and on its own, so that clearing
			// it does not depend on where the SGR sits inside the reset below — the two are separate
			// mechanisms and only one of them is the engine's.
			protect::Request::SoftReset => {
				self.set_pen_protection(false);
				self.soft_reset();
			}
		}
	}

	/// Carry out DECSTR, the soft reset (`CSI ! p`), by feeding the engine the same reset spelled in
	/// sequences it does handle (§72).
	///
	/// `vte`'s CSI dispatch has `('p', ['$'])` and `('p', ['?', '$'])` — both DECRQM — and no arm for
	/// `('p', ['!'])`, so the sequence reached nothing at all. That is a gap worth closing rather than
	/// a policy: cmote asks the remote for `TERM=xterm-256color`, and that terminfo entry opens both
	/// `is2` and `rs2` with `\E[!p`. Every `tput init`, every `reset`, every ncurses startup sends
	/// this — and a program that died leaving a scrolling region and origin mode set was, until now,
	/// not put right by the sequence whose whole job is to put it right.
	///
	/// Fed rather than performed, which is the same choice `reserve_cells` makes and for the same
	/// reason: the engine stays the ONLY writer of its own state, so there is no second source to
	/// disagree with it later. Nothing here reaches into the grid, and the fed bytes go straight to
	/// the parser, so cmote's own scanners never see them and this cannot feed itself.
	///
	/// What is sent, in order, against DEC's published DECSTR list:
	///
	///   CSI 0 m       the pen back to default (SGR)
	///   CSI ? 25 h    cursor visible (DECTCEM)
	///   CSI 4 l       replace rather than insert (IRM)
	///   CSI ? 6 l     absolute origin (DECOM)
	///   CSI ? 7 h     autowrap (DECAWM) — see below
	///   CSI ? 1 l     normal cursor keys (DECCKM)
	///   ESC >         numeric keypad (DECNKM)
	///   ESC ( B …     G0–G3 all ASCII, then SI to make G0 the active set
	///   CSI r         the scrolling region back to the whole page (DECSTBM)
	///   CSI H         home, so the save below is of the corner
	///   ESC 7         the SAVED cursor to home, carrying the pen just reset (DECSC)
	///
	/// The `CSI H` is not redundant, though it looks it: `set_scrolling_region` homes the cursor
	/// itself, so the save would land on the corner without it. Dropping `CSI r` from this string
	/// once broke the saved-cursor test as well as the region one, which is the shape of a hidden
	/// dependency — one item's correctness resting on another item's side effect. Saying it costs
	/// three bytes and means each line above is answerable on its own.
	///
	/// DECSCA, the eleventh item, is cleared by the caller above. The rest of DEC's list — KAM,
	/// DECNRCM, DECAUPSS, DECSASD, DECKPM, DECRLM, DECPCTERM — names state that neither `vte`, nor
	/// the engine, nor cmote models at all, so there is nothing left stale by not sending it.
	///
	/// Two deliberate departures, both worth stating so they are not later "fixed":
	///
	/// **Autowrap goes ON, where the VT510 manual says a soft reset turns it off.** The manual
	/// describes hardware nobody is emulating here. `xterm-256color` declares `am` — this terminal
	/// wraps — and its `rs2` sends this sequence WITHOUT a following `\E[?7h`, so on the terminal
	/// cmote claims to be, a soft reset cannot be what leaves wrapping off, or `tput init` would
	/// break every program that ran it. Power-on default is the honest reading, and the engine's
	/// power-on default (`TermMode::default()`) has `LINE_WRAP` in it.
	///
	/// **The cursor is put back where the reset found it.** DECSTR does not move the cursor, but the
	/// engine's `set_scrolling_region` ends in `goto(0, 0)` — right for DECSTBM, which is defined to
	/// home the cursor, and wrong for a reset that borrows it. So the position is read first and
	/// restored with CUP once origin mode is off and the coordinates are absolute again. The one
	/// thing that does not survive the round trip is the pending-wrap flag, which a reset has no
	/// business preserving anyway.
	fn soft_reset(&mut self) {
		// Read before anything moves. A cursor waiting to wrap sits one past the last column, and
		// CUP clamps it back onto the grid, which is where the next glyph would have gone regardless.
		let (row, col) = self.screen().cursor_position();
		let mut feed = SOFT_RESET.to_vec();
		feed.extend_from_slice(format!("\x1b[{};{}H", row + 1, col + 1).as_bytes());
		self.parser.advance(&mut self.term, &feed);
	}

	/// Arm or disarm DECSCA by setting cmote's borrowed flag bit on the engine's PEN (§56).
	///
	/// This one line is the whole trick. Every cell the engine prints is stamped from
	/// `grid.cursor.template`, so from here on each printed cell carries the bit — and then rides
	/// scrolling, insert/delete, reflow and the alternate-screen swap on the engine's back, with no
	/// map on cmote's side to keep aligned with the grid. `from_bits_retain` is what allows a bit the
	/// engine has no name for; the build-time assertion beside `DEFAULT_ROWS` is what keeps that from
	/// becoming a silent collision if the engine ever claims it.
	fn set_pen_protection(&mut self, on: bool) {
		let flags = &mut self.term.grid_mut().cursor.template.flags;
		let bits = if on {
			protect::mark(flags.bits())
		} else {
			protect::unmark(flags.bits())
		};
		*flags = Flags::from_bits_retain(bits);
	}

	/// Erase the cells the request covers, leaving the protected ones standing (§56).
	///
	/// Written straight into the grid, which is a deliberate break with `reserve_cells` above — that
	/// one injects VT sequences precisely BECAUSE erasing and scrolling are the engine's business.
	/// Here the engine cannot be asked, for two separate reasons. Its plain `CSI 2 J` on the primary
	/// screen does not blank the viewport at all, it scrolls it into history (`Grid::clear_viewport`),
	/// which would carry the protected cells away with everything else. And the per-run alternative —
	/// position with CUP, blank with ECH — would have to move the cursor across a screen the erase is
	/// defined never to move it on, which drags in origin mode and clears the pending-wrap flag. So
	/// the honest version of "blank these cells and nothing else" is to blank these cells.
	///
	/// What is written is what the engine's own erase writes: the PEN's background colour and no
	/// glyph (`Cell: From<Color>` is the same conversion `clear_screen` uses), so a program that
	/// erases with a colour set gets that colour, exactly as a plain erase would give it.
	fn selective_erase(&mut self, erase: protect::Erase) {
		let grid = self.term.grid();
		let point = grid.cursor.point;
		// The cursor's line is counted from the top of the screen, and history sits at negative
		// lines — but a cursor is never in history, so the clamp is only for the type.
		let row = point.line.0.max(0) as usize;
		let spans = protect::spans(
			erase,
			row,
			point.column.0,
			grid.screen_lines(),
			grid.columns(),
		);
		let background = grid.cursor.template.bg;
		let grid = self.term.grid_mut();
		for (row, columns) in spans {
			for column in columns {
				let cell = &mut grid[Line(row as i32)][Column(column)];
				if protect::is_protected(cell.flags.bits()) {
					continue;
				}
				*cell = background.into();
			}
		}
	}

	/// Perform one rectangular area operation (§58, §59) — erase, fill, copy or restyle a box of
	/// cells.
	///
	/// Written straight into the grid, for the reason `selective_erase` above spells out: the engine
	/// has no arm for any of these, and every one of them is defined never to move the cursor, so
	/// there is nothing to delegate and no mode interaction to inherit.
	///
	/// `ponytail:` **origin mode is refused rather than approximated.** With DECOM set, these corners
	/// are counted from the top of the scrolling region instead of the top of the page — and the
	/// engine keeps its `scroll_region` private, with no accessor to read it back through. Placing the
	/// rectangle at the page's rows anyway would put it on the wrong lines, so the operation is
	/// dropped instead: doing nothing is a correct refusal where acting on a guess is a wrong action
	/// (§57). Lifting this means cmote tracking DECSTBM itself, including the engine's own clamping
	/// rules and every reset that widens the region back out — a second copy of state the engine
	/// already owns, which is the shape §56 turned down.
	fn apply_rectangle(&mut self, request: rect::Request) {
		// Origin mode refuses every operation that ACTS, for the reason above. The one that ASKS is
		// not let off it — it cannot place its rectangle either — but it is still let through, because
		// a question dropped on the floor leaves the program that asked waiting on a terminal that has
		// already moved on (§33). It answers for the cells it could reach, which under origin mode is
		// none of them.
		let origin = self.term.mode().contains(TermMode::ORIGIN);
		if origin && !matches!(request, rect::Request::Checksum { .. }) {
			return;
		}
		let (rows, cols) = {
			let grid = self.term.grid();
			(grid.screen_lines(), grid.columns())
		};
		// The four content operations are always the box, and so is the checksum. DECSACE picks
		// between the box and the wrapped run for the attribute pair alone (§59), which is why the
		// extent is a parameter of `area` rather than a mode it reads: the call site is what says
		// which family it belongs to.
		match request {
			rect::Request::Erase(corners) => {
				if let Some(area) = rect::area(corners, rect::Extent::Rectangle, rows, cols) {
					self.erase_area(area, false);
				}
			}
			rect::Request::SelectiveErase(corners) => {
				if let Some(area) = rect::area(corners, rect::Extent::Rectangle, rows, cols) {
					self.erase_area(area, true);
				}
			}
			rect::Request::Fill(glyph, corners) => {
				if let Some(area) = rect::area(corners, rect::Extent::Rectangle, rows, cols) {
					self.fill_area(glyph, area);
				}
			}
			rect::Request::Attributes {
				corners,
				extent,
				change,
			} => {
				if change.is_empty() {
					return;
				}
				if let Some(area) = rect::area(corners, extent, rows, cols) {
					self.attribute_area(area, extent, change, cols);
				}
			}
			rect::Request::Copy { source, top, left } => {
				let Some(source) = rect::area(source, rect::Extent::Rectangle, rows, cols) else {
					return;
				};
				if let Some((source, to_row, to_col)) =
					rect::copy_extent(source, top, left, rows, cols)
				{
					self.copy_area(source, to_row, to_col);
				}
			}
			rect::Request::Checksum { id, corners } => {
				// A rectangle that holds no cells — crossed corners, a corner off the page, or the
				// origin-mode refusal above — is answered with the checksum of nothing, which is
				// what a real terminal reports for an empty area and is not a special case here:
				// `Checksum::default().finish()` is 0 because no cell was ever weighed.
				let area = if origin {
					None
				} else {
					rect::area(corners, rect::Extent::Rectangle, rows, cols)
				};
				let checksum = area.map_or(0, |area| self.checksum_area(area));
				// Into the same buffer the engine's own replies land in, at the point in the stream
				// the question sat — so a DSR and a checksum asked for in one write come back in the
				// order they were asked, without a second reply path to keep in step.
				let reply = rect::checksum_reply(id, checksum);
				self.replies
					.lock()
					.expect("reply buffer mutex poisoned")
					.bytes
					.extend_from_slice(&reply);
			}
		}
	}

	/// Weigh a rectangle for DECRQCRA (§60) — the one rectangular operation that reads the grid
	/// instead of writing it.
	///
	/// Every cell's character code plus its attribute weights, trimmed and negated by
	/// `rect::Checksum`. The weights are xterm's and the reason they are is in `term/rect.rs`; this
	/// is only the half that needs the engine's types, which is why it lives here beside the writers
	/// rather than beside the arithmetic.
	///
	/// The rectangle, not the extent: DECSACE selects a shape for the attribute pair alone, and
	/// xterm's own checksum walks `left..=right` on every row regardless of it.
	fn checksum_area(&self, area: rect::Area) -> u16 {
		let grid = self.term.grid();
		let mut checksum = rect::Checksum::default();
		for row in area.rows() {
			for column in area.columns() {
				let cell = &grid[Line(row as i32)][Column(column)];
				let mut value = u32::from(cell.c);
				// Protection is not in `Flags` — it rides bit 15 (§56) — so it is read through the
				// same helper the selective erase uses rather than by naming the bit twice.
				if protect::is_protected(cell.flags.bits()) {
					value += 0x04;
				}
				for (flag, weight) in CHECKSUM_ATTRIBUTES {
					if cell.flags.contains(flag) {
						value += weight;
					}
				}
				checksum.cell(value);
			}
		}
		checksum.finish()
	}

	/// Blank every cell of a rectangle (DECERA, §58), or every unprotected one (DECSERA).
	///
	/// What lands in each cell is what the engine's own erase writes: the pen's background colour and
	/// no glyph, so an erased cell is blank rather than merely overwritten — flags included, which is
	/// what lets a cell erased here be protected again later (§56).
	fn erase_area(&mut self, area: rect::Area, selective: bool) {
		let background = self.term.grid().cursor.template.bg;
		let grid = self.term.grid_mut();
		for row in area.rows() {
			for column in area.columns() {
				let cell = &mut grid[Line(row as i32)][Column(column)];
				if selective && protect::is_protected(cell.flags.bits()) {
					continue;
				}
				*cell = background.into();
			}
		}
	}

	/// Fill every cell of a rectangle with one character (DECFRA, §58).
	///
	/// Stamped from the PEN, so the fill carries the colours and attributes a printed character would
	/// have had at that moment — which is what DECFRA is defined to do, and what makes it worth
	/// having over a rectangle of spaces. Protection rides along with the rest of the pen, so a fill
	/// inside a DECSCA run is protected exactly as typed text would be.
	fn fill_area(&mut self, glyph: char, area: rect::Area) {
		let mut template = self.term.grid().cursor.template.clone();
		template.c = glyph;
		let grid = self.term.grid_mut();
		for row in area.rows() {
			for column in area.columns() {
				grid[Line(row as i32)][Column(column)] = template.clone();
			}
		}
	}

	/// Change or flip attributes across an area, leaving every character where it stands (DECCARA
	/// and DECRARA, §59).
	///
	/// The one thing this must not do is assign the flag word. Only the three bits `RECT_ATTRIBUTES`
	/// names are touched, one at a time, so a cell keeps its italics, its wide-character marking, its
	/// underline STYLE — and, the reason this is a rule rather than a nicety, cmote's DECSCA
	/// protection bit (§56). A form drawn in a protected run stays protected after a program
	/// underlines it, which is what a VT420 does and what the flag word being shared makes easy to
	/// get wrong.
	///
	/// Protection is otherwise ignored here, unlike in the erases: DECSCA marks a cell unerasable,
	/// and changing how it looks does not erase it.
	fn attribute_area(
		&mut self,
		area: rect::Area,
		extent: rect::Extent,
		change: rect::Change,
		cols: usize,
	) {
		let grid = self.term.grid_mut();
		for row in area.rows() {
			for column in area.columns_on(row, extent, cols) {
				let flags = &mut grid[Line(row as i32)][Column(column)].flags;
				let mut before = 0u8;
				for (attribute, flag) in RECT_ATTRIBUTES {
					if flags.contains(flag) {
						before |= attribute;
					}
				}
				let after = change.apply(before);
				// Nothing to write when the fold changed nothing, which is the common case for a
				// selector the cell already satisfies.
				if after == before {
					continue;
				}
				for (attribute, flag) in RECT_ATTRIBUTES {
					flags.set(flag, after & attribute != 0);
				}
			}
		}
	}

	/// Copy a rectangle of cells to a new top-left corner (DECCRA, §58).
	///
	/// The source is read out WHOLE before anything is written, because the two rectangles may
	/// overlap — scrolling a sub-window by one row is the point of the sequence, and it is also the
	/// maximally overlapping case. DECCRA is defined as if the copy went through a buffer, so cmote
	/// uses a buffer rather than working out which direction to walk in.
	///
	/// Whole cells move, so the glyph's colours, its attributes, its OSC 8 link and its DECSCA
	/// protection all travel with it. That is right on its own terms: protection makes a cell
	/// unerasable, not immovable (§56).
	fn copy_area(&mut self, source: rect::Area, to_row: usize, to_col: usize) {
		let cells: Vec<Cell> = {
			let grid = self.term.grid();
			source
				.rows()
				.flat_map(|row| {
					source
						.columns()
						.map(move |column| grid[Line(row as i32)][Column(column)].clone())
				})
				.collect()
		};
		let width = source.width();
		let grid = self.term.grid_mut();
		for (index, cell) in cells.into_iter().enumerate() {
			let row = to_row + index / width;
			let column = to_col + index % width;
			grid[Line(row as i32)][Column(column)] = cell;
		}
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

	/// The icon name a program last set for this tab (OSC 1, §69), if any. `None` until one does,
	/// and again once one clears it with an empty name.
	///
	/// A borrow rather than a clone, unlike `title` above: the scanner is a plain field on this
	/// struct and not shared with the engine, so there is no lock to get out from behind. The tab
	/// strip draws it AFTER the endpoint and never in place of it — see the module for why that is
	/// load-bearing rather than cosmetic.
	pub fn icon_name(&self) -> Option<&str> {
		self.icon.name()
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

	/// The branch the remote shell last announced (§55), or `None` when it announced none — which is
	/// every shell that does not set iTerm2's `gitBranch` user variable, so most of them. Already safe
	/// to draw: decoded, UTF-8, control characters stripped, length capped.
	pub fn branch(&self) -> Option<&str> {
		self.iterm.branch()
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
	/// Reads the icon name a remote sets (OSC 1, §69), which the tab chip wears after the endpoint.
	/// `vte` has no arm for that code at all, so — like the cwd, and with the same latest-value
	/// shape and no place on the grid — the same bytes are scanned here. Its module also declines
	/// the icon half of OSC 0, which is a decision and not an omission; the reasoning is there.
	icon: icon::Icon,
	/// Reads the selective-erase sequences the engine drops — DECSCA, DECSED and DECSEL (§56). Fed by
	/// the split advance, but for the opposite reason to the marks above: each of its requests has to
	/// be applied with the engine advanced PAST the sequence, not up to it. Protection itself is not
	/// held here at all — it rides the engine's pen and then each printed cell, so there is no map to
	/// keep aligned with the grid.
	protect: protect::Protect,
	/// Finds the sequences the engine would read as something ELSE — today only DECSLRM, whose `s`
	/// the engine takes for a save-cursor (§57). The odd one out among these scanners: the others are
	/// here because the engine ignores something, this one because it does not. Fed by the split
	/// advance so the offending final byte can be swapped for a CAN on its way past.
	cancels: cancel::Cancel,
	/// Reads the VT420 rectangular area operations the engine drops — DECERA, DECSERA, DECFRA and
	/// DECCRA (§58), then DECCARA, DECRARA and DECSACE (§59). Fed by the split advance for the same
	/// reason as the selective erase: each one is applied with the engine advanced PAST the sequence
	/// it ignored. The module is the grammar and the geometry, and the cells are written below; the
	/// one thing it does hold is DECSACE's extent, because only the scanner sees a mode and the
	/// requests it governs in stream order.
	rectangles: rect::Rectangles,
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
	/// A selective-erase request (§56). The odd one out in this list: every other kind is applied
	/// with the engine advanced UP TO its offset, because the cursor then names the line the event
	/// belongs on, while `protect` reports offsets one past the sequence so its requests land on the
	/// far side of it. Both work through the same loop because a split is a split — the difference is
	/// only which side of the boundary the scanner asked for.
	Protect(protect::Request),
	/// A final byte the engine must not dispatch (§57). The only split that is not something to
	/// apply: it marks the byte itself, and the loop replaces it with a CAN rather than feeding it.
	Cancel,
	/// A rectangular area operation (§58, §59) — erase, fill, copy or restyle a box of cells. Applied
	/// on the far side of its sequence, as a selective erase is, and for the same reason.
	Rect(rect::Request),
}

/// Merge one chunk's prompt marks, image events, bookmarks, selective-erase requests and cancelled
/// final bytes into offset order. Every list arrives ascending, and the sort is stable, so two events
/// at the very same offset keep the order they were scanned in — which is the only sensible
/// tie-break, since no scanner can see another's.
fn splits(
	marks: Vec<(usize, osc133::Mark)>,
	images: Vec<(usize, graphics::Event)>,
	bookmarks: Vec<(usize, iterm::Report)>,
	protections: Vec<(usize, protect::Request)>,
	cancels: Vec<usize>,
	rectangles: Vec<(usize, rect::Request)>,
) -> Vec<(usize, Split)> {
	let mut merged: Vec<(usize, Split)> = Vec::with_capacity(
		marks.len()
			+ images.len()
			+ bookmarks.len()
			+ protections.len()
			+ cancels.len()
			+ rectangles.len(),
	);
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
	merged.extend(
		protections
			.into_iter()
			.map(|(offset, request)| (offset, Split::Protect(request))),
	);
	merged.extend(cancels.into_iter().map(|offset| (offset, Split::Cancel)));
	merged.extend(
		rectangles
			.into_iter()
			.map(|(offset, request)| (offset, Split::Rect(request))),
	);
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
///
/// Since §63 that pair no longer arrives at all — `engine_config` sets `osc52: Osc52::Disabled`,
/// so the engine returns before an event exists. The catch-all below is kept as the second line
/// rather than the only one: if an engine bump changed the meaning of that field, or a `Config`
/// edit dropped it, the events would start arriving again and would still be discarded here.
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
	fn the_engine_is_told_to_refuse_the_remote_clipboard() {
		// The refusal that has to be STATED rather than fallen into (§63). Left at its default
		// this field would be `OnlyCopy`, which is enough for a remote's OSC 52 write to become an
		// `Event::ClipboardStore` that only the listener's catch-all discards. This test exists to
		// fail if the field is ever dropped from `engine_config` — a fall-through cannot say
		// "refused", so the field has to, and something has to check the field.
		assert_eq!(engine_config().osc52, Osc52::Disabled);
	}

	#[test]
	fn a_remote_clipboard_request_draws_no_reply() {
		// Both directions on the wire: a write carrying base64, and a read (`?`), which is the
		// reply-bearing one — answering it would hand the remote the local clipboard's contents.
		// Neither draws a byte back. The text after them still lands, which is what says the
		// sequences were consumed whole rather than half-parsed with their tail spilling onto the
		// screen.
		let mut terminal = Terminal::new(4, 20);
		assert!(terminal.process(b"\x1b]52;c;aGVsbG8=\x07").is_empty());
		assert!(terminal.process(b"\x1b]52;c;?\x07").is_empty());
		terminal.process(b"after");
		assert_eq!(read(&terminal, 0, 0, 5), "after");
	}

	#[test]
	fn the_engine_is_told_to_speak_the_kitty_keyboard_protocol() {
		// The other decision in `engine_config`, pinned for the same reason: §25 leaves the whole
		// control plane to the engine and only encodes key presses, so turning this off would
		// strand `keymap`/`kitty` reading flags nothing maintains — and it would fail silently,
		// with programs simply never being told the protocol is available.
		assert!(engine_config().kitty_keyboard);
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
	fn a_palette_colour_set_does_not_move_the_query_answer() {
		// A remote sets slot 3 to red, then asks what slot 3 is: the answer is still the scheme's
		// yellow (§6, §64). The set is not discarded on the way IN — the assertion in the middle
		// proves the engine did record it — it is discarded on every way OUT: `report_color`
		// resolves through the shared const palette, and `ui/grid.rs` paints from that same table
		// via a style resolver that is never handed a terminal to ask. That makes the renderer's
		// half of the refusal structural, but the reply's half was resting on nobody happening to
		// wire `Term::colors` into `report_color`. This test is what would notice.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"\x1b]4;3;rgb:ff/00/00\x07").is_empty());
		assert!(terminal.term.colors()[3].is_some());
		assert_eq!(
			terminal.process(b"\x1b]4;3;?\x07"),
			b"\x1b]4;3;rgb:8080/8080/0000\x07".to_vec()
		);
	}

	#[test]
	fn a_default_colour_set_does_not_move_the_query_answer() {
		// The same for the named roles: OSC 11 sets the background to red, and OSC 11 ? still
		// reports the scheme's 0x1e. This is the direction that matters most to a program, because
		// the background is what a colourscheme picker reads (see the query test above). An answer
		// that followed the set would promise a colour the grid does not paint, and the program
		// would then choose its contrast against a background that does not exist — which is why
		// honouring the set in the reply alone would be worse than either honouring it fully or
		// refusing it fully.
		let mut terminal = Terminal::new(10, 40);
		assert!(terminal.process(b"\x1b]11;rgb:ff/00/00\x07").is_empty());
		assert!(terminal.term.colors()[NamedColor::Background as usize].is_some());
		assert_eq!(
			terminal.process(b"\x1b]11;?\x07"),
			b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07".to_vec()
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
	fn the_iterm_spelling_of_the_cell_size_question_gets_no_answer() {
		// §71. A reply is an advertisement, and this question is asked in iTerm2 in order to size an
		// inline image — the `File=` protocol cmote refuses (§70). Answering precisely and then
		// dropping the picture is worse for the sender than not answering: silence is what lets it
		// fall back.
		//
		// The silence has to be shown to be a DECISION rather than ignorance, so the same terminal
		// answers the standard form of the same question on the next line, from numbers it plainly
		// holds. 8 x 17 pixel cells over a 10 x 40 grid.
		let mut terminal = Terminal::new(10, 40);
		terminal.set_cell_pixels(8, 17);
		assert!(terminal.process(b"\x1b]1337;ReportCellSize\x07").is_empty());
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
	fn an_icon_name_reaches_the_tab_strip_without_touching_the_title() {
		// §69. OSC 1 is a code `vte` has no arm for, so this whole path is cmote's own scanner —
		// and it must stay in ITS lane: the icon name goes to the chip and the title bar is left
		// exactly as it was. Setting one is state, not a reply, so no bytes go back either.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]2;window\x07");
		assert!(terminal.process(b"\x1b]1;vim\x07").is_empty());
		assert_eq!(terminal.icon_name(), Some("vim"));
		assert_eq!(terminal.title().as_deref(), Some("window"));
	}

	#[test]
	fn osc_0_moves_the_title_and_leaves_the_icon_name_alone() {
		// The refusal, pinned at the boundary (§69). OSC 0 sets icon name and window title to the
		// SAME string, and cmote takes only the title half — so a stock Debian prompt, which fires
		// this sequence on every prompt, cannot end up printed on every chip forever.
		//
		// The order of the two assertions is the argument: the title moving is what proves the
		// sequence was parsed and applied, so the icon name staying `None` is a decision cmote
		// made about bytes it HAD, not a sequence that never arrived.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]1;vim\x07");
		terminal.process(b"\x1b]0;user@host: ~\x07");
		assert_eq!(terminal.title().as_deref(), Some("user@host: ~"));
		assert_eq!(terminal.icon_name(), Some("vim"));
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

	/// The form a VT220 program draws: labels inside a DECSCA run, the user's data outside it, then
	/// one selective erase to start the next record (§56).
	#[test]
	fn a_selective_erase_leaves_the_protected_labels_standing() {
		let mut terminal = Terminal::new(4, 20);
		// No space between the two runs: a blank cell reads as an empty string through the view, so a
		// gap would be invisible here and the test would be asserting less than it looks like.
		terminal.process(b"\x1b[1\"qName:\x1b[0\"qBob");
		assert_eq!(read(&terminal, 0, 0, 9), "Name:Bob");
		terminal.process(b"\x1b[?2J");
		assert_eq!(
			read(&terminal, 0, 0, 9),
			"Name:",
			"the label survives and the typed value does not"
		);
	}

	/// The two erases are different verbs, and the plain one is the stronger: protection only holds
	/// against the `?` spelling. A program that means to wipe everything still can.
	#[test]
	fn a_plain_erase_takes_the_protected_labels_too() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1\"qName:\x1b[0\"q Bob");
		terminal.process(b"\x1b[2J");
		assert_eq!(read(&terminal, 0, 0, 9).trim(), "");
	}

	/// DECSCA is independent of SGR on a real terminal, so a colour reset inside a protected run must
	/// not quietly unprotect the rest of it. This is the case the pen trick has to be told about: the
	/// engine's SGR 0 assigns the whole flag word, borrowed bit included (§56).
	#[test]
	fn a_colour_reset_inside_a_protected_run_does_not_unprotect_it() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1\"q\x1b[31mRed\x1b[0mPlain\x1b[0\"q gone");
		terminal.process(b"\x1b[?2J");
		assert_eq!(
			read(&terminal, 0, 0, 13).trim_end(),
			"RedPlain",
			"both halves of the run are protected, across the reset"
		);
	}

	/// The payoff of carrying protection on the pen rather than in a map beside the grid (§56): the
	/// cells move and their protection moves with them, because to the engine it is just another
	/// attribute. A map would have had to be re-aligned here, and this is where it would have drifted.
	#[test]
	fn protection_rides_a_scroll() {
		let mut terminal = Terminal::new(3, 20);
		// Draw the row at the very bottom, then step off the end so the screen scrolls under it.
		terminal.process(b"\x1b[3;1H\x1b[1\"qName:\x1b[0\"qvalue\n");
		assert_eq!(
			read(&terminal, 1, 0, 10),
			"Name:value",
			"one row up after the scroll"
		);
		terminal.process(b"\x1b[?2J");
		assert_eq!(read(&terminal, 1, 0, 10).trim_end(), "Name:");
	}

	/// A selective erase in the LINE is confined to the cursor's row, exactly as the plain EL is.
	#[test]
	fn a_selective_erase_in_the_line_leaves_the_other_rows_alone() {
		let mut terminal = Terminal::new(3, 20);
		terminal.process(b"first\r\nsecond\x1b[1;1H\x1b[?2K");
		assert_eq!(read(&terminal, 0, 0, 5).trim(), "");
		assert_eq!(read(&terminal, 1, 0, 6), "second");
	}

	/// A full reset rebuilds the pen, so protection cannot outlive it — otherwise a program that
	/// resets and moves on would leave cmote holding text nothing could erase.
	#[test]
	fn a_full_reset_stops_the_pen_protecting() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1\"q\x1bcAfter");
		terminal.process(b"\x1b[?2J");
		assert_eq!(read(&terminal, 0, 0, 5).trim(), "");
	}

	/// The soft reset's share of the same rule (§72). DECSTR clears the pen, and the borrowed bit is
	/// cleared beside it rather than left to the SGR inside the fed reset — this is what says so.
	#[test]
	fn a_soft_reset_stops_the_pen_protecting() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1\"q\x1b[!pAfter");
		terminal.process(b"\x1b[?2J");
		assert_eq!(read(&terminal, 0, 0, 5).trim(), "");
	}

	/// The five modes DECSTR names that the engine actually models, asked for by the engine's own
	/// DECRQM (§72). Each is first moved AWAY from where the reset must leave it, so a reply of "2"
	/// is the reset's doing and not the power-on default sitting still.
	#[test]
	fn a_soft_reset_puts_back_the_modes_it_is_defined_to() {
		let mut terminal = Terminal::new(6, 20);
		// Origin on, autowrap off, cursor hidden, application cursor keys on, insert mode on.
		terminal.process(b"\x1b[?6h\x1b[?7l\x1b[?25l\x1b[?1h\x1b[4h");
		assert_eq!(
			terminal.process(b"\x1b[?6$p\x1b[?7$p\x1b[?25$p\x1b[?1$p\x1b[4$p"),
			b"\x1b[?6;1$y\x1b[?7;2$y\x1b[?25;2$y\x1b[?1;1$y\x1b[4;1$y".to_vec(),
			"the five modes have to move before the reset can be shown to move them back"
		);
		terminal.process(b"\x1b[!p");
		assert_eq!(
			terminal.process(b"\x1b[?6$p\x1b[?7$p\x1b[?25$p\x1b[?1$p\x1b[4$p"),
			b"\x1b[?6;2$y\x1b[?7;1$y\x1b[?25;1$y\x1b[?1;2$y\x1b[4;2$y".to_vec()
		);
	}

	/// The item with real consequences: a program that died inside `CSI 2;3 r` leaves every later
	/// scroll trapped in three rows, and DECSTR is the sequence `reset` sends to undo it (§72).
	/// Read through origin mode, which is the only way the region is observable from outside.
	#[test]
	fn a_soft_reset_clears_the_scrolling_region() {
		let mut terminal = Terminal::new(6, 20);
		// With a region set, origin mode makes `CSI 1;1H` mean the region's top rather than the
		// page's — so this first assertion is what proves the region was really in force.
		terminal.process(b"\x1b[2;4r\x1b[?6h\x1b[1;1H");
		assert_eq!(terminal.screen().cursor_position(), (1, 0));
		terminal.process(b"\x1b[!p");
		terminal.process(b"\x1b[?6h\x1b[1;1H");
		assert_eq!(terminal.screen().cursor_position(), (0, 0));
	}

	/// DECSTR does not move the cursor — but the engine's `set_scrolling_region` homes it, and the
	/// fed reset has to send `CSI r` to clear the region. The CUP that puts it back is what this
	/// pins (§72); without it a `tput init` mid-screen would jump the shell's cursor to the corner.
	#[test]
	fn a_soft_reset_leaves_the_cursor_where_it_found_it() {
		let mut terminal = Terminal::new(6, 20);
		terminal.process(b"\x1b[4;6H");
		assert_eq!(terminal.screen().cursor_position(), (3, 5));
		terminal.process(b"\x1b[!p");
		assert_eq!(terminal.screen().cursor_position(), (3, 5));
	}

	/// The SAVED cursor is a different matter: DEC's list puts it at home with a default pen, so a
	/// restore after a soft reset lands at the corner rather than wherever the program last saved.
	#[test]
	fn a_soft_reset_homes_the_saved_cursor() {
		let mut terminal = Terminal::new(6, 20);
		terminal.process(b"\x1b[4;6H\x1b7");
		terminal.process(b"\x1b[!p");
		terminal.process(b"\x1b8");
		assert_eq!(terminal.screen().cursor_position(), (0, 0));
	}

	/// The pen, read back through DECRQSS — cmote's own answer, built from the very template the
	/// grid paints with (§33), so this checks the reset against what the engine believes rather
	/// than against the bytes that were fed.
	#[test]
	fn a_soft_reset_puts_the_pen_back() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;4;7;31m");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;4;7;31m\x1b\\".to_vec()
		);
		terminal.process(b"\x1b[!p");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0m\x1b\\".to_vec()
		);
	}

	/// All four character-set slots go back to ASCII, and G0 becomes the active one again. A program
	/// left in line-drawing mode prints boxes instead of letters, which is the classic wedged
	/// terminal `reset` exists for — and the wedge DECSTR alone was not clearing (§72).
	#[test]
	fn a_soft_reset_puts_the_character_sets_back() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b(0q");
		assert_eq!(read(&terminal, 0, 0, 1), "─", "line drawing has to be on");
		terminal.process(b"\x1b[!p\x1b[2;1Hq");
		assert_eq!(read(&terminal, 1, 0, 1), "q");
	}

	/// The keypad back to numeric, the second half of what terminfo's `rs2` is for: a program that
	/// exits without its `rmkx` leaves the numpad sending SS3 sequences the shell shows as letters.
	#[test]
	fn a_soft_reset_puts_the_keypad_back_to_numeric() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b=");
		assert!(terminal.screen().application_keypad());
		terminal.process(b"\x1b[!p");
		assert!(!terminal.screen().application_keypad());
	}

	/// The near miss that would be a disaster: DECRQM shares this final byte, and `vte` DOES have
	/// arms for it. A scanner that matched on `p` alone would soft-reset the terminal every time a
	/// program asked what a mode was — so the request must still be answered, and change nothing.
	#[test]
	fn a_mode_request_is_not_read_as_a_soft_reset() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[?7l");
		assert_eq!(terminal.process(b"\x1b[?7$p"), b"\x1b[?7;2$y".to_vec());
		// Still off: asking the question did not reset it.
		assert_eq!(terminal.process(b"\x1b[?7$p"), b"\x1b[?7;2$y".to_vec());
	}

	/// Protection dies with the cell it was on. A plain erase writes the cell fresh — flags and all —
	/// so a cell reused after one is erasable again, and a form cannot accumulate unerasable ground.
	#[test]
	fn a_cell_erased_the_plain_way_comes_back_unprotected() {
		let mut terminal = Terminal::new(3, 20);
		terminal.process(b"\x1b[1\"qName:\x1b[0\"q");
		// Plain EL blanks the row in place, which resets each cell's flags with its glyph.
		terminal.process(b"\x1b[2K\x1b[1;1Hnew");
		terminal.process(b"\x1b[?2K");
		assert_eq!(read(&terminal, 0, 0, 5).trim(), "");
	}

	/// The borrowed flag bit must be invisible in the other direction too: the view reports the
	/// attributes the program actually set, and nothing extra (§56).
	#[test]
	fn protection_is_invisible_to_the_screen_view() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"\x1b[1\"q\x1b[1mBold");
		let screen = terminal.screen();
		let cell = screen.cell(0, 0).expect("the first cell is on screen");
		assert_eq!(cell.contents(), "B");
		assert!(cell.bold(), "the attribute the program set");
		assert!(!cell.italic());
		assert!(!cell.inverse());
		assert_eq!(cell.underline(), screen::UnderlineStyle::None);
	}

	/// The collision §57 is about: `CSI Pl;Pr s` is a margin request, and the engine's arm for the
	/// final `s` is save-cursor with its parameters ignored. Unhandled, it silently overwrites the one
	/// saved-cursor slot, and the program's own restore then lands wherever the margin request sat.
	#[test]
	fn a_margin_request_does_not_move_the_saved_cursor() {
		let mut terminal = Terminal::new(4, 20);
		// The status-line shape: save, go somewhere, write, come back.
		terminal.process(b"\x1b[1;1Hhome\x1b[s");
		terminal.process(b"\x1b[3;1Hstatus\x1b[5;70s");
		terminal.process(b"\x1b[uBACK");
		assert_eq!(
			read(&terminal, 0, 0, 8),
			"homeBACK",
			"the restore went where the program saved, not where the margin request sat"
		);
		assert_eq!(
			read(&terminal, 2, 0, 10),
			"status",
			"and nothing was written over the status line it had moved away from"
		);
	}

	/// The cancel has to END the sequence, not merely withhold its final byte: the parameters have
	/// already reached the engine's parser, so without one the next final byte in the stream would be
	/// taken as this sequence's. Here that is the `h` of `hello`, which would dispatch as set-mode
	/// with parameters 5 and 70 and swallow four characters of output.
	#[test]
	fn the_text_after_a_margin_request_is_printed_and_not_eaten() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[5;70shello");
		assert_eq!(read(&terminal, 0, 0, 5), "hello");
	}

	/// And the CAN itself must leave nothing behind. A substitute glyph, or the `s` printed as a
	/// letter, would both show up here.
	#[test]
	fn a_cancelled_margin_request_prints_nothing_at_all() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"a\x1b[5;70sb");
		assert_eq!(read(&terminal, 0, 0, 4), "ab");
	}

	/// The other meaning of the same final byte, which cmote keeps: a bare `CSI s` is the universal
	/// save-cursor spelling and still works.
	#[test]
	fn a_bare_save_and_restore_still_works() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2;5H\x1b[s\x1b[4;1Helsewhere\x1b[uX");
		assert_eq!(read(&terminal, 1, 4, 1), "X");
	}

	/// DECERA blanks a box and leaves everything around it standing — the one thing that separates it
	/// from every erase the engine already has, all of which work in lines (§58).
	#[test]
	fn a_rectangular_erase_takes_a_box_out_of_the_middle() {
		let mut terminal = Terminal::new(4, 10);
		terminal.process(b"aaaaaaaaaa\r\nbbbbbbbbbb\r\ncccccccccc\r\ndddddddddd");
		// Rows 2-3, columns 3-5.
		terminal.process(b"\x1b[2;3;3;5$z");
		assert_eq!(
			read(&terminal, 0, 0, 10),
			"aaaaaaaaaa",
			"row above untouched"
		);
		assert_eq!(
			read(&terminal, 1, 0, 10),
			"bbbbbbb",
			"three cells gone from the middle"
		);
		assert_eq!(read(&terminal, 2, 0, 10), "ccccccc");
		assert_eq!(
			read(&terminal, 3, 0, 10),
			"dddddddddd",
			"row below untouched"
		);
		// The gap is where it was asked for, not at the end of the row.
		let screen = terminal.screen();
		assert_eq!(screen.cell(1, 1).unwrap().contents(), "b");
		assert!(screen.cell(1, 2).unwrap().contents().is_empty());
		assert!(screen.cell(1, 4).unwrap().contents().is_empty());
		assert_eq!(screen.cell(1, 5).unwrap().contents(), "b");
	}

	/// DECERA is the plain verb and DECSERA the selective one, exactly as `CSI J` and `CSI ? J` are
	/// (§56). Having both is the point: the plain one is the stronger.
	#[test]
	fn a_rectangular_erase_and_its_selective_twin_differ_over_protection() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"\x1b[1\"qKEEP\x1b[0\"qgone");
		// The selective one leaves the protected label standing.
		terminal.process(b"\x1b[1;1;1;10${");
		assert_eq!(read(&terminal, 0, 0, 10), "KEEP");
		// The plain one takes it.
		terminal.process(b"\x1b[1;1;1;10$z");
		assert_eq!(read(&terminal, 0, 0, 10).trim(), "");
	}

	/// DECFRA rules a box with one character, in the attributes the pen holds — which is what makes it
	/// worth having over a rectangle of spaces (§58).
	#[test]
	fn a_rectangular_fill_paints_the_box_in_the_pen() {
		let mut terminal = Terminal::new(3, 8);
		// 45 is `-`. Fill row 2, columns 2 to 4, in bold.
		terminal.process(b"\x1b[1m\x1b[45;2;2;2;4$x");
		assert_eq!(read(&terminal, 1, 0, 8), "---");
		let screen = terminal.screen();
		assert!(screen.cell(1, 0).unwrap().contents().is_empty());
		assert_eq!(screen.cell(1, 1).unwrap().contents(), "-");
		assert!(
			screen.cell(1, 1).unwrap().bold(),
			"filled in the pen's attributes"
		);
		assert!(screen.cell(1, 4).unwrap().contents().is_empty());
		assert_eq!(
			read(&terminal, 0, 0, 8).trim(),
			"",
			"and only the row it named"
		);
	}

	/// DECCRA's overlapping case, which is the one it exists for: scroll a sub-window up a row by
	/// copying it over itself. Reading the source out whole first is what makes this come out right.
	#[test]
	fn a_rectangular_copy_over_itself_scrolls_the_box() {
		let mut terminal = Terminal::new(4, 6);
		terminal.process(b"one\r\ntwo\r\nsix\r\nten");
		// Copy rows 2-4 up to row 1: the source and the destination overlap by two rows.
		terminal.process(b"\x1b[2;1;4;3;1;1;1;1$v");
		assert_eq!(read(&terminal, 0, 0, 6), "two");
		assert_eq!(read(&terminal, 1, 0, 6), "six");
		assert_eq!(read(&terminal, 2, 0, 6), "ten");
		assert_eq!(
			read(&terminal, 3, 0, 6),
			"ten",
			"the last row is the source, left as it was"
		);
	}

	/// A copy carries the whole cell, attributes and all — which is what DECCRA is for, and comes free
	/// from copying cells rather than characters.
	#[test]
	fn a_rectangular_copy_brings_the_attributes_with_it() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"\x1b[1;1H\x1b[1;31mRED\x1b[0m");
		terminal.process(b"\x1b[1;1;1;3;1;3;1;1$v");
		let screen = terminal.screen();
		assert_eq!(read(&terminal, 2, 0, 3), "RED");
		let cell = screen.cell(2, 0).expect("the copy landed on the last row");
		assert!(cell.bold(), "bold travelled with the glyph");
	}

	/// A copy that runs off the page is trimmed to what fits rather than refused, which is what makes
	/// a scroll-by-copy work against the bottom edge.
	#[test]
	fn a_rectangular_copy_off_the_page_keeps_what_fits() {
		let mut terminal = Terminal::new(3, 4);
		terminal.process(b"ab\r\ncd\r\nef");
		// Three rows of source, one row of room: only the first row lands.
		terminal.process(b"\x1b[1;1;3;2;1;3;1;1$v");
		assert_eq!(read(&terminal, 2, 0, 4), "ab");
		assert_eq!(
			read(&terminal, 0, 0, 4),
			"ab",
			"and the source is untouched"
		);
		assert_eq!(read(&terminal, 1, 0, 4), "cd");
	}

	/// Origin mode counts these corners from the top of the scrolling region, and the engine keeps
	/// that region private — so the operation is refused rather than placed on the wrong rows (§58).
	#[test]
	fn a_rectangle_is_refused_while_origin_mode_is_set() {
		let mut terminal = Terminal::new(3, 6);
		terminal.process(b"aaaaaa\r\nbbbbbb\r\ncccccc");
		terminal.process(b"\x1b[?6h\x1b[1;1;3;6$z");
		assert_eq!(read(&terminal, 0, 0, 6), "aaaaaa", "nothing was erased");
		// Reset origin mode and the same request goes through.
		terminal.process(b"\x1b[?6l\x1b[1;1;3;6$z");
		assert_eq!(read(&terminal, 0, 0, 6).trim(), "");
	}

	/// A rectangle a program described backwards, or one that starts off the page, is a no-op — never
	/// a rectangle cmote invented by swapping or clamping the corners.
	#[test]
	fn a_rectangle_nobody_could_draw_does_nothing() {
		let mut terminal = Terminal::new(3, 6);
		terminal.process(b"aaaaaa\r\nbbbbbb\r\ncccccc");
		// Bottom above top, right left of left, and a top-left past the last row.
		terminal.process(b"\x1b[3;1;1;6$z\x1b[1;6;3;2$z\x1b[9;1;9;6$z");
		assert_eq!(read(&terminal, 0, 0, 6), "aaaaaa");
		assert_eq!(read(&terminal, 1, 0, 6), "bbbbbb");
		assert_eq!(read(&terminal, 2, 0, 6), "cccccc");
	}

	/// A margin request in the middle of a chunk that also carries a split of another kind — the two
	/// conventions meet here, since a selective erase reports the byte one PAST its sequence and a
	/// cancel reports the final byte itself.
	#[test]
	fn a_margin_request_beside_a_selective_erase_keeps_both() {
		let mut terminal = Terminal::new(3, 20);
		terminal.process(b"\x1b[1\"qName:\x1b[0\"qBob\x1b[5;70s\x1b[?2J");
		assert_eq!(read(&terminal, 0, 0, 9), "Name:");
	}

	/// DECCARA changes how an area LOOKS and leaves every character where it stands — the one thing
	/// that separates it from the fill and the erase of §58 (§59).
	#[test]
	fn an_attribute_change_repaints_without_moving_a_character() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"abcdefgh\r\nijklmnop\r\nqrstuvwx");
		// Rectangle extent, so the box alone: row 2, columns 3 to 5, bold and underlined.
		terminal.process(b"\x1b[2*x\x1b[2;3;2;5;1;4$r");
		assert_eq!(
			read(&terminal, 1, 0, 8),
			"ijklmnop",
			"the text is untouched"
		);
		let screen = terminal.screen();
		assert!(!screen.cell(1, 1).unwrap().bold(), "left of the box");
		assert!(screen.cell(1, 2).unwrap().bold());
		assert_eq!(
			screen.cell(1, 2).unwrap().underline(),
			screen::UnderlineStyle::Single
		);
		assert!(screen.cell(1, 4).unwrap().bold());
		assert!(!screen.cell(1, 5).unwrap().bold(), "right of the box");
		assert!(!screen.cell(0, 2).unwrap().bold(), "the row above");
	}

	/// DECRARA is the same shape by the flipping verb: a cell that was bold comes out plain and a
	/// plain one comes out bold, in a single pass.
	#[test]
	fn an_attribute_reversal_flips_each_cell_on_its_own() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"\x1b[1mAB\x1b[0mCD");
		terminal.process(b"\x1b[2*x\x1b[1;1;1;4;1$t");
		let screen = terminal.screen();
		assert!(!screen.cell(0, 0).unwrap().bold(), "bold became plain");
		assert!(screen.cell(0, 2).unwrap().bold(), "plain became bold");
		assert_eq!(read(&terminal, 0, 0, 4), "ABCD");
	}

	/// DECSACE is the whole difference between a box and a wrapped run, and cmote defaults to the
	/// run because a terminal powers up that way (§59).
	#[test]
	fn the_extent_decides_whether_the_change_wraps() {
		let mut terminal = Terminal::new(3, 6);
		terminal.process(b"aaaaaa\r\nbbbbbb\r\ncccccc");
		// The default extent: row 1 column 5 through row 2 column 2, round the wrap.
		terminal.process(b"\x1b[1;5;2;2;1$r");
		let screen = terminal.screen();
		assert!(!screen.cell(0, 3).unwrap().bold(), "before the run starts");
		assert!(screen.cell(0, 4).unwrap().bold());
		assert!(screen.cell(0, 5).unwrap().bold(), "out to the page edge");
		assert!(screen.cell(1, 0).unwrap().bold(), "in from the page edge");
		assert!(screen.cell(1, 1).unwrap().bold());
		assert!(!screen.cell(1, 2).unwrap().bold(), "after the run ends");
	}

	/// The same corners as a RECTANGLE are drawn backwards — right corner 2 is left of left corner
	/// 5 — so they name no box at all and nothing happens. The extent is not a detail of how the
	/// same area is walked; it decides whether there is an area.
	#[test]
	fn the_same_corners_as_a_box_can_be_undrawable() {
		let mut terminal = Terminal::new(3, 6);
		terminal.process(b"aaaaaa\r\nbbbbbb\r\ncccccc");
		terminal.process(b"\x1b[2*x\x1b[1;5;2;2;1$r");
		let screen = terminal.screen();
		assert!(!screen.cell(0, 4).unwrap().bold());
		assert!(!screen.cell(1, 0).unwrap().bold());
	}

	/// A protected form stays protected after a program underlines it. The DECSCA bit rides the
	/// engine's flag word (§56), so an attribute change that ASSIGNED the word instead of setting
	/// named bits would silently unprotect the labels.
	#[test]
	fn an_attribute_change_leaves_protection_alone() {
		let mut terminal = Terminal::new(2, 10);
		terminal.process(b"\x1b[1\"qKEEP\x1b[0\"qgone");
		// Underline everything, then selectively erase the row.
		terminal.process(b"\x1b[2*x\x1b[1;1;1;10;4$r\x1b[1;1;1;10${");
		assert_eq!(read(&terminal, 0, 0, 10), "KEEP", "still protected");
		assert_eq!(
			terminal.screen().cell(0, 0).unwrap().underline(),
			screen::UnderlineStyle::Single,
			"and underlined"
		);
	}

	/// Selector `0` names the four attributes DEC gave these sequences, and nothing else. Italics
	/// are not among them, so they survive an "all off".
	#[test]
	fn all_attributes_off_means_the_four_it_can_name() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"\x1b[1;3mXY");
		terminal.process(b"\x1b[2*x\x1b[1;1;1;2;0$r");
		let screen = terminal.screen();
		let cell = screen.cell(0, 0).expect("the first cell");
		assert!(!cell.bold(), "bold is one of the four");
		assert!(cell.italic(), "italic is not");
	}

	/// Blink has no bit in the engine's flag word, so it is parsed and dropped — and dropping it
	/// must not cost the rest of the list.
	#[test]
	fn a_blink_request_still_delivers_the_rest_of_the_list() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"XY");
		terminal.process(b"\x1b[2*x\x1b[1;1;1;2;5;4$r");
		assert_eq!(
			terminal.screen().cell(0, 0).unwrap().underline(),
			screen::UnderlineStyle::Single
		);
	}

	/// The attribute pair inherits §58's origin-mode refusal, for the same reason: with DECOM set
	/// the corners are counted from a scrolling region the engine keeps private.
	#[test]
	fn an_attribute_change_is_refused_while_origin_mode_is_set() {
		let mut terminal = Terminal::new(3, 6);
		terminal.process(b"aaaaaa\r\nbbbbbb\r\ncccccc");
		terminal.process(b"\x1b[?6h\x1b[2*x\x1b[1;1;3;6;1$r");
		assert!(!terminal.screen().cell(0, 0).unwrap().bold());
		terminal.process(b"\x1b[?6l\x1b[1;1;3;6;1$r");
		assert!(terminal.screen().cell(0, 0).unwrap().bold());
	}

	/// DECRQCRA reports the negated sum of what is on the cells, as four hex digits, with the
	/// request's own id echoed back (§60). `A` + `B` is 0x83, and 0x10000 − 0x83 is 0xFF7D.
	#[test]
	fn a_checksum_reports_the_negated_sum_of_the_rectangle() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"AB");
		assert_eq!(
			terminal.process(b"\x1b[1;1;1;1;1;2*y"),
			b"\x1bP1!~FF7D\x1b\\".to_vec()
		);
	}

	/// The answer is the page as it stood WHERE THE QUESTION SAT, not as the rest of the chunk left
	/// it. That is what the split-fed offset buys, and the only rectangular operation that needs it
	/// for anything but ordering (§60).
	#[test]
	fn a_checksum_answers_from_the_page_the_question_arrived_on() {
		let mut terminal = Terminal::new(2, 4);
		// One write: print `AB`, ask about it, then overwrite it with `ZZ`. Answering at the end of
		// the chunk would report 0xFF4C, the checksum of `ZZ`.
		assert_eq!(
			terminal.process(b"AB\x1b[1;1;1;1;1;2*y\x1b[1;1HZZ"),
			b"\x1bP1!~FF7D\x1b\\".to_vec()
		);
		assert_eq!(read(&terminal, 0, 0, 4), "ZZ");
	}

	/// Attributes weigh into the number, in xterm's amounts: bold 0x80, underline 0x10, reverse 0x20.
	/// A checksum of the characters alone would be a different number for every styled screen.
	#[test]
	fn attributes_weigh_into_the_checksum() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"\x1b[1mA");
		// 0x41 + 0x80 = 0xC1, negated 0xFF3F.
		assert_eq!(
			terminal.process(b"\x1b[1;1;1;1;1;1*y"),
			b"\x1bP1!~FF3F\x1b\\".to_vec()
		);
		let mut styled = Terminal::new(2, 4);
		styled.process(b"\x1b[4;7mA");
		// 0x41 + 0x10 + 0x20 = 0x71, negated 0xFF8F.
		assert_eq!(
			styled.process(b"\x1b[1;1;1;1;1;1*y"),
			b"\x1bP1!~FF8F\x1b\\".to_vec()
		);
	}

	/// DECSCA protection is one of the weights (0x04), and it does not live in the engine's `Flags` —
	/// it rides bit 15 (§56), so a checksum that read `Flags` alone would miss it.
	#[test]
	fn a_protected_cell_weighs_four_more() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"\x1b[1\"qA\x1b[0\"q");
		// 0x41 + 0x04 = 0x45, negated 0xFFBB.
		assert_eq!(
			terminal.process(b"\x1b[9;1;1;1;1;1*y"),
			b"\x1bP9!~FFBB\x1b\\".to_vec()
		);
	}

	/// A plain space is trimmed out of the sum, except the very first cell of the rectangle — which
	/// is why an empty page reports one space rather than nothing.
	///
	/// This is also cmote's one disclosed divergence from xterm: xterm knows which cells a program
	/// actually wrote and skips the rest, where the engine's grid starts out full of blanks that
	/// read the same either way, so xterm answers 0x0000 for a page it has never painted (§60).
	#[test]
	fn a_blank_page_reports_a_single_trimmed_space() {
		let mut terminal = Terminal::new(2, 4);
		assert_eq!(
			terminal.process(b"\x1b[3*y"),
			b"\x1bP3!~FFE0\x1b\\".to_vec()
		);
	}

	/// The rectangle resolves against the VISIBLE PAGE, so the scrollback cannot be read through it —
	/// the security property that lets cmote answer this at all (§60). A bottom corner far past the
	/// page clamps to it and reports the same number as the page itself.
	#[test]
	fn a_checksum_never_reaches_the_scrollback() {
		let mut terminal = Terminal::new(2, 4);
		// `AB` scrolls into the history; `CD` and `EF` are what is left on screen.
		terminal.process(b"AB\r\nCD\r\nEF");
		// 0x43 + 0x44 + 0x45 + 0x46 = 0x112, negated 0xFEEE. `AB` would have added 0x83 more.
		let page = terminal.process(b"\x1b[1*y");
		assert_eq!(page, b"\x1bP1!~FEEE\x1b\\".to_vec());
		assert_eq!(
			terminal.process(b"\x1b[1;1;1;1;99;99*y"),
			page,
			"a corner past the page clamps to it and reaches no further"
		);
	}

	/// DECSACE picks a shape for the attribute pair alone. The checksum is always the box, as
	/// xterm's own walk is — so the same question gets the same answer under either extent.
	#[test]
	fn a_checksum_ignores_the_attribute_extent() {
		let mut terminal = Terminal::new(2, 6);
		terminal.process(b"abcdef\r\nghijkl");
		// Rows 1–2, columns 2–4: `bcd` and `hij`, summing to 0x264 and reported as 0xFD9C. Read as a
		// wrapped run the same corners would take `bcdef` and `ghij` instead.
		let stream = terminal.process(b"\x1b[0*x\x1b[1;1;1;2;2;4*y");
		let boxed = terminal.process(b"\x1b[2*x\x1b[1;1;1;2;2;4*y");
		assert_eq!(stream, b"\x1bP1!~FD9C\x1b\\".to_vec());
		assert_eq!(boxed, stream);
	}

	/// A rectangle that holds no cells is still a question, and gets the checksum of nothing rather
	/// than silence — a program waiting on an answer that never comes stalls (§33).
	#[test]
	fn a_rectangle_that_holds_nothing_is_still_answered() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"AB");
		// Right corner 1 is left of left corner 3: undrawable, so no cells were weighed.
		assert_eq!(
			terminal.process(b"\x1b[5;1;1;3;1;1*y"),
			b"\x1bP5!~0000\x1b\\".to_vec()
		);
	}

	/// XTQMODKEYS (`CSI ? 4 m`) is answered with the level cmote holds, in the set form (§61). The
	/// engine parses the question and drops it, so before §61 the program asking waited out its
	/// timeout — the exact failure §33 exists to prevent.
	#[test]
	fn a_modify_other_keys_question_is_answered() {
		let mut terminal = Terminal::new(2, 8);
		assert_eq!(terminal.process(b"\x1b[?4m"), b"\x1b[>4;0m".to_vec());
		assert_eq!(terminal.process(b"\x1b[>4;2m"), b"".to_vec());
		assert_eq!(terminal.process(b"\x1b[?4m"), b"\x1b[>4;2m".to_vec());
		assert_eq!(
			terminal.modify_other_keys(),
			modkeys::ModifyOtherKeys::Level2,
			"asking must not disturb the level"
		);
	}

	/// The private-mode sequences share the question's `?` marker and its parameter shape, and are
	/// orders of magnitude more common. None of them may draw a reply.
	#[test]
	fn a_private_mode_earns_no_modify_other_keys_reply() {
		let mut terminal = Terminal::new(2, 8);
		assert_eq!(
			terminal.process(b"\x1b[?1049h\x1b[?25l\x1b[?2004h"),
			b"".to_vec()
		);
	}

	/// Origin mode refuses every operation in this family that ACTS, because the corners would be
	/// counted from a scrolling region the engine keeps private (§58). The one that ASKS is refused
	/// the same rectangle and answered anyway, because the alternative is a program that waits.
	#[test]
	fn origin_mode_costs_the_rectangle_and_not_the_reply() {
		let mut terminal = Terminal::new(3, 4);
		terminal.process(b"AB");
		assert_eq!(
			terminal.process(b"\x1b[?6h\x1b[7;1;1;1;1;2*y"),
			b"\x1bP7!~0000\x1b\\".to_vec(),
			"answered, for no cells"
		);
		terminal.process(b"\x1b[?6l");
		assert_eq!(
			terminal.process(b"\x1b[7;1;1;1;1;2*y"),
			b"\x1bP7!~FF7D\x1b\\".to_vec(),
			"and for the real ones once the mode is off"
		);
	}
}

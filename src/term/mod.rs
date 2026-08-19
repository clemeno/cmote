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
// One more query is answered outside that module because it cannot wait for the end of the chunk:
// DECXCPR (`CSI ? 6 n`), the DEC-private spelling of "where is the cursor?", which `vte` has no arm
// for (§82). A position is only true where the question sat, so `dsr` reports offsets and the split
// advance below answers each one with the engine advanced exactly that far. The other nine values of
// `CSI ? Ps n` — printer, UDK, keyboard nationality, locator, macro space, memory checksum, data
// integrity, multi-session — are refused there by an allow-list, because each would advertise a piece
// of the user's machine (§71, and §36's rule that cmote's replies name the program, never the person).
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
mod csi; // the limits every CSI scanner has to agree with the engine about (§106)
pub mod cwd; // tracks the remote working directory announced by the shell (§17)
#[cfg(test)]
mod differential; // drives the engine's own parser beside cmote's scanners and compares them (§106)
mod dsr; // reads the DEC-private device status reports the engine drops — DECXCPR, and an allow-list over the rest (§82)
mod gate; // the one place cmote sits between the parser and the engine, so a decision can be pre-empted (§102)
#[cfg(test)]
mod gatediff; // drives a second, ungated engine beside cmote's and compares the grids they produce (§102, §106)
pub mod graphics; // finds the inline images the engine drops, and anchors them to the document (§41)
mod icon; // reads the icon name a remote sets, OSC 1, for the tab chip to wear (§69)
pub mod iterm; // reads the parts of iTerm2's OSC 1337 namespace cmote honours — an allow-list (§55)
pub mod keymap; // maps GUI key events to the bytes a terminal sends
pub mod kitty; // encodes key events in the kitty keyboard protocol's CSI u form (§25)
mod margins; // holds the left and right margins a program sets, and the arithmetic they imply (§102)
pub mod modkeys; // tracks the remote's xterm modifyOtherKeys mode for the key encoder (§9)
pub mod mouse; // maps pointer events to the reports a mouse-aware program expects
mod notify; // names the desktop-notification spellings cmote refuses, so the refusal is stated (§79)
mod osc; // frames OSC strings out of the stream for the scanners below, and sanitises what they keep (§17, §34, §54, §55, §69)
pub mod osc133; // reads the shell-integration prompt marks the engine ignores (§34)
pub mod pointer; // reads the mouse pointer shape a remote asks for, OSC 22 — an allow-list (§77)
pub mod progress; // reads the progress a remote command reports, OSC 9;4 (§54)
mod protect; // reads the selective-erase sequences the engine drops — DECSCA, DECSED, DECSEL (§56)
mod query; // answers the identity queries the engine drops — XTVERSION, DECRQSS, XTGETTCAP, DA3, XTSMGRAPHICS (§33, §36, §41)
mod rect; // reads the VT420 rectangular bounds operations the engine drops — DECERA, DECSERA, DECFRA, DECCRA (§58), DECCARA, DECRARA, DECSACE (§59)
mod region; // mirrors the engine's private vertical scrolling region, so cmote can read back what DECSTBM set (§102)
pub mod scp; // reads SCP, the direction a line's characters are laid down in (§76)
pub mod screen; // the engine-agnostic view of the screen the app reads through (§9, §16, §23)
pub mod search; // finds text anywhere in the scrollback for the find bar (§35)
mod sgrstack; // reads XTPUSHSGR / XTPOPSGR, the video-attribute stack the engine never sees (§85)
pub mod sixel; // decodes a sixel image's payload into pixels (§41)
mod tabs; // reads DECST8C, the tab-stop reset the engine parses and drops (§74)

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

/// A row count or page row index as the engine's signed line number (§111).
///
/// cmote counts rows in `usize` because that is what the engine's own `screen_lines()`,
/// `history_size()` and `display_offset()` return; the engine INDEXES rows with a signed `Line`,
/// where 0 is the top of the active page and -1 is the newest scrolled-off row. So every walk over the
/// grid crosses this boundary, and it used to cross it with a bare `as i32` in twenty-odd places.
///
/// Out of range is a bug rather than a condition, which is why this panics rather than clamping. The
/// numbers that reach here come from the engine's own geometry: a page is at most `u16::MAX` rows
/// because the pty size is a `u16` pair (§9), and the history is capped at `SCROLLBACK`. The largest
/// value possible is therefore about 75,000 against an `i32` that holds two billion, so a failure
/// here would mean the engine had reported a geometry it cannot itself hold — and clamping that to
/// `i32::MAX` would only move the panic into the grid's own indexing, with a worse message.
fn as_line_number(rows: usize) -> i32 {
	i32::try_from(rows).expect("a row count from the engine's own geometry fits in an i32")
}

/// The engine's `Line` for a row of the active page — 0 is the top of the screen.
fn page_line(row: usize) -> Line {
	Line(as_line_number(row))
}

/// The engine's `Line` for a scrolled-off row, counted back from the page: `back == 0` is the NEWEST
/// row in the scrollback, which the engine calls `Line(-1)`.
///
/// Worth a name of its own because the off-by-one goes the other way from every other row index, and
/// `-(1 + back as i32)` written out at each site is one typo away from reading the wrong row.
fn history_line(back: usize) -> Line {
	Line(-1 - as_line_number(back))
}

/// A row count as an absolute document line (§40, §111).
///
/// The document is numbered in `u64` from the oldest retained line, so that a position survives the
/// scrolling that makes every engine-relative row number stale. Counts arriving from the engine are
/// `usize`, and `usize` has no `From<usize> for u64` because a platform could in principle make it
/// wider — so the conversion is spelled out once here, saturating, with the observation that a
/// `usize` above `u64::MAX` would need a machine no version of this program will run on.
pub(super) fn as_document_line(rows: usize) -> u64 {
	u64::try_from(rows).unwrap_or(u64::MAX)
}

/// An absolute document line as a signed number, for arithmetic that can cross zero (§40, §111).
///
/// A mark's line minus the top of the viewport is NEGATIVE when the mark has scrolled off the top,
/// which is the case the projection has to detect — so the subtraction has to happen in a signed
/// domain and the operands have to get there first. Saturating rather than failing: `i64::MAX` lines
/// is more output than a session can produce — at one line per nanosecond it is nearly three
/// centuries — and a line pinned at that end sorts as "above everything", which is what the
/// arithmetic would have concluded anyway.
pub(super) fn as_signed_line(line: u64) -> i64 {
	i64::try_from(line).unwrap_or(i64::MAX)
}

/// A scroll distance in lines, as the engine's `Scroll::Delta` takes it (§23).
///
/// This one CLAMPS where `as_line_number` panics, and the difference is the direction of travel: a
/// row index that does not fit is a broken geometry, but a scroll distance that does not fit is
/// merely further than the document goes, and the engine clamps a delta to the document anyway. So
/// saturating here lands on exactly the same row the engine would have chosen.
fn scroll_delta(lines: i64) -> i32 {
	i32::try_from(lines.clamp(i64::from(i32::MIN), i64::from(i32::MAX)))
		.expect("clamped into an i32's range on the line above")
}

/// DECSTR, the soft reset, written in the sequences the engine itself handles (§72).
///
/// `CSI ! p` reaches nothing in `vte`, so `Terminal::soft_reset` feeds the engine this instead: the
/// pen, the cursor's visibility, insert/replace, origin, autowrap, the cursor-key mode, the keypad,
/// all four character-set slots plus the active one, the scrolling region, and finally the SAVED
/// cursor — which `ESC 7` puts at home with the pen this string has just reset, DEC's own definition
/// of the item. `soft_reset` appends the CUP that puts the real cursor back, since `CSI r` homes it.
/// Every byte of it is a sequence the engine has an arm for — with ONE exception since §102, the
/// `\E[?69l` that turns the left and right margins off, which is a sequence the GATE has an arm for.
/// The point holds either way: the reset is spelled in sequences something downstream already
/// implements, so nothing here becomes a second writer of state (see `soft_reset` for the departures
/// from DEC's list).
///
/// The margins are on this string for a reason worth stating, because DEC's published list does not
/// name them and §72 was careful not to widen that list. `CSI r` is on it — DECSTR puts the
/// scrolling region back to the whole page — and the margins are the same object's other axis. A
/// reset that freed the rows and left the columns walled off would hand the next program half a
/// page and no sequence to discover it with.
const SOFT_RESET: &[u8] = b"\x1b[0m\x1b[?25h\x1b[4l\x1b[?6l\x1b[?7h\x1b[?1l\x1b>\x1b(B\x1b)B\x1b*B\x1b+B\x0f\x1b[?69l\x1b[r\x1b[H\x1b7";

/// The engine, specialised to our reply-collecting listener. `screen::Screen` borrows this
/// to read the grid, so the alias is the one name that would change under another engine.
pub(super) type Engine = Term<Replies>;

/// A scrollback movement the GUI asks for (§23). cmote's own small vocabulary so the engine's
/// `Scroll` type stays behind `term/`, the same way `screen` hides the rest of the engine. The
/// wheel sends `Lines`, Shift+PageUp/PageDown send `PageUp`/`PageDown`, Shift+Home/End send
/// `Top`/`Bottom`, and every keystroke sends `Bottom` to snap the view back to the live prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputSpan {
	pub start_line: u64,
	pub end_line: u64,
	pub last_col: u16,
}

/// The settings cmote runs the engine with. A named function rather than a literal buried inside
/// `Terminal::new`, so that a test can read it back: every field here overrides an
/// `alacritty_terminal` default on purpose, and two of them are decisions this project argued at
/// length. A decision nothing checks is a decision that leaves quietly on the next crate bump —
/// which is exactly the failure §62 of `TERMINAL_COMPATIBILITY_PLAN` went looking for.
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

/// Stand up an engine and the reply buffer its listener writes into, the way `Terminal` stands them up.
///
/// A named function with two callers rather than a literal inside `Terminal::new`, for the same reason
/// `engine_config` is one: `term/gatediff.rs` builds a SECOND engine beside cmote's and compares the two
/// grids, and that comparison is worth nothing unless both engines were built identically. Written here,
/// the guarantee holds by construction; written twice, an edit to this one would silently turn the oracle
/// into a different terminal that agrees with cmote about nothing in particular.
fn new_engine(rows: u16, cols: u16) -> (Engine, Arc<Mutex<ReplyBuffer>>) {
	let replies = Arc::new(Mutex::new(ReplyBuffer {
		rows,
		cols,
		..ReplyBuffer::default()
	}));
	let engine = Term::new(
		engine_config(),
		&GridSize {
			rows: rows as usize,
			cols: cols as usize,
		},
		Replies(Arc::clone(&replies)),
	);
	(engine, replies)
}

impl Terminal {
	/// Create an emulator with a `rows`×`cols` grid, matching the remote pty.
	pub fn new(rows: u16, cols: u16) -> Self {
		let (term, replies) = new_engine(rows, cols);
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
			pointer: pointer::Pointer::default(),
			protect: protect::Protect::default(),
			cancels: cancel::Cancel::default(),
			rectangles: rect::Rectangles::default(),
			tabs: tabs::Tabs::default(),
			dsr: dsr::Dsr::default(),
			sgr_stack: sgrstack::SgrStack::default(),
			saved_pens: Vec::new(),
			dropped_pushes: 0,
			scp: scp::Scp::default(),
			paths: scp::Paths::default(),
			graphics: graphics::Images::default(),
			on_alternate: false,
			region: region::ScrollRegion::full(rows as usize),
			margins: margins::Margins::default(),
		}
	}

	/// Feed bytes to the engine, through the gate that stands between them (§102).
	///
	/// Every advance in this file goes through here, including the ones that feed sequences cmote
	/// SYNTHESISED — a soft reset's long spelling (§72), a tab-stop rebuild (§74), a restored pen
	/// (§85). Those have to pass the gate too: a synthesised DECSTBM changes the scrolling region
	/// just as a remote's does, and a mirror that only watched the wire would miss it.
	///
	/// The three borrows are disjoint fields of `self`, which is what lets the parser, the engine and
	/// the state kept beside it all be borrowed mutably at once.
	fn advance(&mut self, bytes: &[u8]) {
		let mut gate = gate::Gate::new(
			&mut self.term,
			&mut self.region,
			&mut self.margins,
			&self.replies,
		);
		self.parser.advance(&mut gate, bytes);
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
		// The mouse pointer shape a remote asks for over its own grid (OSC 22, §77). A third
		// latest-value reading with no place on the grid, so no split either — and like the icon
		// name, nothing else in the stack ever sees it: `vte` parses the sequence and hands it to a
		// `Handler` method left at its empty default body, which the engine never overrides.
		self.pointer.feed(bytes);
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
		// The selective-erase sequences the engine drops (§56). Interruption-fed like the marks, but its
		// offsets sit one PAST each sequence, because a pen change has to be applied after the SGR
		// that wiped it and an erase after the engine has ignored it. An unarmed stream that sends no
		// `?`-erase reports nothing, so the common case still pays for no split.
		let protections = self.protect.feed(bytes);
		// DECSLRM, the one sequence the engine reads as something else (§57, §102). Unlike every
		// scanner above, this one is not reporting something to apply — it is reporting a byte the
		// engine must not be let
		// near, so its offset is the final byte itself and the interruption loop steps OVER it.
		let cancels = self.cancels.feed(bytes);
		// The VT420 rectangular operations the engine drops (§58, §59). Interruption-fed like the selective
		// erase and with the same one-past offsets: these name their own coordinates and never touch
		// the cursor, so the split is only about the order they land in against the text around them.
		let rectangles = self.rectangles.feed(bytes);
		// DECST8C, the tab-stop reset the engine parses and then does nothing with (§74). Interruption-fed
		// with one-past offsets like the two above — and the order matters more here than for a
		// rectangle: a chunk that resets its stops and then prints tabs has to see the new stops,
		// so the reset cannot wait for the end of the chunk.
		let tab_resets = self.tabs.feed(bytes);
		// DECXCPR, the cursor-position question in DEC's spelling, which reaches no arm in the parser
		// (§82). Interruption-fed for a reason none of the scanners above share: this one ANSWERS, and the
		// answer is only true at the point in the stream the question sat. `term/query.rs` may collect
		// its queries and reply after the chunk because a version string and a unit id do not move.
		let cursor_requests = self.dsr.feed(bytes);
		// The character path (§76), and the RIS that empties the store of them. Interruption-fed like the
		// prompt marks and for the same reason: SCP names no line of its own, it acts on the one the
		// cursor is on, so the engine has to be where the sequence is before the cursor is read.
		let paths = self.scp.feed(bytes);
		// XTPUSHSGR / XTPOPSGR, the video-attribute stack (§85). Interruption-fed for the reason DECXCPR is:
		// a push must read the pen as it stood where the push was written, and a pop must restore it
		// there — a chunk that pushes, paints itself red and pops would otherwise save the red.
		let sgr_stack = self.sgr_stack.feed(bytes);
		let scanned = Scanned {
			marks,
			images,
			bookmarks,
			protections,
			cancels,
			rectangles,
			tab_resets,
			cursor_requests,
			paths,
			sgr_stack,
		};
		// Whether this chunk put a picture on the alternate page — the one thing that makes the
		// covered-cell sweep below sit the chunk out (see `retire_covered_images`).
		let mut placed_on_alternate = false;
		if scanned.is_empty() {
			self.advance(bytes);
		} else {
			let mut start = 0;
			for (offset, interruption) in interruptions(scanned) {
				// `start` can already be past this offset, because a cancelled final byte was stepped
				// over just now (see `Interruption::Cancel`). No scanner can report an event INSIDE a CSI
				// sequence, so nothing is ever skipped by this clamp — it only keeps the slice below
				// from being built backwards.
				let offset = offset.max(start);
				self.advance(&bytes[start..offset]);
				start = offset;
				match interruption {
					Interruption::Prompt(mark) => {
						let history = self.term.grid().history_size();
						let (row, _) = self.screen().cursor_position();
						self.prompts.apply(mark, history, row);
					}
					Interruption::Graphics(event) => {
						placed_on_alternate |= self.apply_graphics(event);
					}
					// A bookmark is read the same way a prompt mark is — the cursor, now that the
					// engine has been advanced to the sequence, names the line the script meant.
					Interruption::UserMark => {
						let history = self.term.grid().history_size();
						let (row, _) = self.screen().cursor_position();
						self.prompts.record_user_mark(history, row);
					}
					Interruption::Protect(request) => self.apply_protection(request),
					// A parametrised `CSI … s`, which is DECSLRM or SCOSC depending on a mode (§102).
					//
					// With mode 69 set it is a margin request: the margins are placed, and the final
					// byte is still ended rather than dispatched, because the engine's arm for it
					// reads no parameters and would save the cursor on the way past. Feeding NOTHING
					// in place of it would leave the engine's parser mid-CSI, waiting to take the
					// next final byte in the stream as this sequence's — `term/cancel.rs` says what
					// that costs and why the replacement is a CAN.
					//
					// Without the mode it is a save-cursor, and the byte is left alone: the engine's
					// reading of it is the right one, which is the whole reason §57's guess could be
					// retired.
					Interruption::Margins(request) => {
						if self.margins.enabled() {
							self.advance(&[cancel::CANCEL]);
							start += 1;
							self.set_margins(request);
						}
					}
					Interruption::Rect(request) => self.apply_rectangle(request),
					Interruption::TabStops => self.set_default_tabs(),
					Interruption::Dsr(request) => self.answer_dsr(request),
					Interruption::Path(request) => self.select_character_path(request),
					Interruption::SgrStack(request) => self.apply_sgr_stack(request),
				}
			}
			self.advance(&bytes[start..]);
		}
		// The chunk is applied, so this is where a swap on or off the alternate screen is noticed —
		// including one that carried no picture with it, which the interruption loop above never sees (§41).
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
	fn apply_graphics(&mut self, event: graphics::GraphicsEvent) -> bool {
		// A swap earlier in this same chunk has already been applied to the engine by the split
		// advance, so ask before doing anything: the picture arriving belongs to the page that is up
		// NOW, and the page it swapped away from should already have been emptied.
		self.sync_alternate();
		match event {
			graphics::GraphicsEvent::Image(image) => {
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
			graphics::GraphicsEvent::ClearScreen => {
				if self.on_alternate {
					self.graphics.clear_alternate();
				} else {
					self.graphics
						.clear_screen(self.term.grid().history_size() as u64);
				}
			}
			graphics::GraphicsEvent::ClearScrollback => {
				if !self.on_alternate {
					self.graphics
						.clear_scrollback(self.term.grid().history_size() as u64);
				}
			}
			// RIS resets the terminal itself, so it takes everything wherever the session is.
			graphics::GraphicsEvent::Reset => self.graphics.clear(),
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
			// The alternate page keeps no history, so its absolute lines start again from zero and would
			// collide with paths set on the main screen (§76). Both directions of the swap clear them,
			// exactly as the pictures above are cleared and for the same reason.
			self.paths.clear();
			// And the pointer goes back to cmote (§77). The swap is exactly the moment a full-screen
			// program starts or ends, so this is where a hand left hovering over a TUI's buttons
			// would otherwise be inherited by the shell prompt the user quit back to — or by the
			// program starting up, which has not asked for anything yet.
			self.pointer.clear();
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
		let screen = screen::Screen::new(&self.term, &self.paths);
		self.graphics
			.retire_covered_alternate(|placement| is_covered(&screen, placement));
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
		self.advance(&feed);
	}

	/// Carry out one selective-erase request (§56), with the engine already advanced past the
	/// sequence that carried it.
	fn apply_protection(&mut self, request: protect::ProtectRequest) {
		match request {
			protect::ProtectRequest::Protect(on) => self.set_pen_protection(on),
			// The SGR just applied may have assigned the pen's whole flag word, so put the bit back.
			// Idempotent, which is why the scanner is free to over-report (see `term/protect.rs`).
			protect::ProtectRequest::Reassert => self.set_pen_protection(true),
			protect::ProtectRequest::Erase(erase) => self.selective_erase(erase),
			// DECSTR (§72). The borrowed protection bit goes first and on its own, so that clearing
			// it does not depend on where the SGR sits inside the reset below — the two are separate
			// mechanisms and only one of them is the engine's.
			protect::ProtectRequest::SoftReset => {
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
		self.advance(&feed);
	}

	/// Place the left and right margins — DECSLRM, with mode 69 already known to be set (§102).
	///
	/// A rejected request moves nothing, not even the cursor: `Margins::set` applies xterm's test
	/// (the left margin strictly left of the right, the right clamped to the page) and says whether
	/// it took.
	///
	/// An accepted one HOMES the cursor, as DECSTBM does — and it is fed as a CUP rather than
	/// written, so the one piece of code that knows where "home" is under origin mode stays the
	/// gate's `goto`: the engine puts the row at the top of the scrolling region and the gate puts
	/// the column at the left margin. §72's route, on a sequence cmote has just started to own.
	fn set_margins(&mut self, request: cancel::CancelRequest) {
		let cols = self.term.grid().columns();
		if self.margins.set(request.left, request.right, cols) {
			self.advance(b"\x1b[H");
		}
	}

	/// Carry out DECST8C — clear every tab stop and set one every eight columns (§74).
	///
	/// The same answer §72 gave the soft reset, for the same reason: `vte` parses the sequence and
	/// `alacritty_terminal` leaves `Handler::set_tabs` at the trait's empty default, so it reached the
	/// engine and stopped. The engine's tab table is private, it is kept aligned across resize by the
	/// engine itself, and cmote declines to become a second writer of engine state (§71, §73) — so
	/// this feeds the engine the same request spelled in TBC, HTS and CUF and lets the engine write
	/// its own table. Every byte fed is a sequence the compatibility matrix already marks ✅.
	///
	/// Only two numbers are read out first, both from the seam rather than the engine's internals:
	/// the page's width, which says how many stops there are, and the cursor's column, which the walk
	/// has to give back. Its ROW is never read and never moved — see `term/tabs.rs` for why the walk
	/// is built out of CR and CUF rather than the CHA that would read more naturally.
	fn set_default_tabs(&mut self) {
		let (_, columns) = self.screen().size();
		let (_, col) = self.screen().cursor_position();
		let feed = tabs::every_eighth_column(columns, col);
		self.advance(&feed);
	}

	/// Answer DECXCPR — the cursor's position, in DEC's private spelling of the question (§82).
	///
	/// Read from the seam's `cursor_position`, which is the engine's own `grid.cursor.point` and so the
	/// very field `device_status` reports for the ANSI spelling. cmote is a second READER of the cursor
	/// here and never a second source for it, which is the property that keeps the two spellings of one
	/// question from ever disagreeing (§71, §73) — see `term/dsr.rs` for what that costs under origin
	/// mode, which is a divergence inherited on purpose rather than a second one invented here.
	///
	/// The reply goes into the same buffer the engine's own replies land in, at the point in the stream
	/// the question sat, exactly as DECRQCRA's checksum does (§60). So a program that writes `CSI 5 n`
	/// and `CSI ? 6 n` in one breath gets the two answers back in the order it asked for them, with no
	/// second reply path to keep in step.
	fn answer_dsr(&self, request: dsr::DsrRequest) {
		let reply = match request {
			dsr::DsrRequest::CursorPosition => {
				let (row, col) = self.screen().cursor_position();
				dsr::cursor_reply(row, col)
			}
			// The two honest negatives (§93). Constants, so unlike the cursor they would read the
			// same answered after the chunk — they are answered here because they arrive through
			// the same scanner, and one route is easier to keep right than two.
			dsr::DsrRequest::LocatorStatus => dsr::NO_LOCATOR.to_vec(),
			dsr::DsrRequest::LocatorType => dsr::NO_LOCATOR_TYPE.to_vec(),
			// Also a constant, and for a reason worth keeping in sight: cmote's colour scheme is
			// fixed (§6), so "dark" cannot go stale between the question and the answer (§98).
			dsr::DsrRequest::ColorScheme => dsr::DARK_SCHEME.to_vec(),
		};
		self.replies
			.lock()
			.expect("reply buffer mutex poisoned")
			.bytes
			.extend_from_slice(&reply);
	}

	/// Carry out one push or pop of the video-attribute stack (§85), with the engine advanced to the
	/// sequence that carried it.
	///
	/// A push READS the engine's template cell — the same field DECRQSS reports (§33) — and keeps a
	/// copy. A pop writes nothing: it feeds the engine the SGR that spells the pen being restored, so
	/// the engine stays the only writer of its own template (§71, §73). That is §72's route for DECSTR
	/// and §74's for DECST8C, and fed bytes go straight to the parser, so cmote's own scanners never see
	/// them and this cannot feed itself.
	///
	/// The protection bit is read across the restore and put back. cmote borrows a spare bit of the
	/// engine's flag word for DECSCA (§56), the `CSI 0 m` that opens a restore assigns that whole word,
	/// and a stack of VIDEO attributes has no business clearing a cell-protection setting — the same
	/// care `protect::ProtectRequest::Reassert` takes after an ordinary SGR, on the one path that does not go
	/// through the scanner.
	fn apply_sgr_stack(&mut self, request: sgrstack::SgrStackRequest) {
		match request {
			sgrstack::SgrStackRequest::Push(mask) => {
				// A remote may not make cmote hold more than xterm's ten. The drop is counted so the
				// pop that matches it is dropped too, keeping the levels below correctly paired.
				if self.saved_pens.len() >= sgrstack::DEPTH {
					self.dropped_pushes += 1;
				} else {
					let pen = self.term.grid().cursor.template.clone();
					self.saved_pens.push((mask, pen));
				}
			}
			// RIS. Everything back to power-on, and a power-on terminal has nothing pushed — the
			// counter with it, or the first pops after a reset would be swallowed by an overflow
			// that belonged to the session before it (§86).
			sgrstack::SgrStackRequest::Reset => {
				self.saved_pens.clear();
				self.dropped_pushes = 0;
			}
			sgrstack::SgrStackRequest::Pop => {
				if self.dropped_pushes > 0 {
					self.dropped_pushes -= 1;
					return;
				}
				// A pop with nothing pushed is not an error and does nothing, which is what a terminal
				// with an empty stack has to do.
				let Some((mask, saved)) = self.saved_pens.pop() else {
					return;
				};
				let protected =
					protect::is_protected(self.term.grid().cursor.template.flags.bits());
				let sgr = merged_pen(&self.term.grid().cursor.template, &saved, mask);
				self.advance(sgr.as_bytes());
				self.set_pen_protection(protected);
			}
		}
	}

	/// Carry out one character-path request (§76).
	///
	/// SCP acts on "the line the cursor is on", and the line is recorded as an ABSOLUTE document
	/// index — `history_size + cursor row`, the same arithmetic the prompt marks use (§34) — so it
	/// stays with its text as the screen scrolls under it, for free and for the same reason.
	///
	/// Nothing is written to the grid. The path is a rule the renderer applies when it derives a frame
	/// from the grid, which is what ECMA-48's data and presentation components mean, and what keeps the
	/// engine the only writer of its own state (§71, §73).
	fn select_character_path(&mut self, request: scp::ScpRequest) {
		match request {
			scp::ScpRequest::Select(path) => {
				let history = self.term.grid().history_size() as u64;
				let (row, _) = self.screen().cursor_position();
				self.paths.select(history + u64::from(row), path);
			}
			// RIS drops the history, which renumbers every line: a remembered index would then name
			// different text. The engine performs the reset itself; forgetting is cmote's share.
			scp::ScpRequest::Reset => self.paths.clear(),
		}
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
				let cell = &mut grid[page_line(row)][Column(column)];
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
	fn apply_rectangle(&mut self, request: rect::RectRequest) {
		// Origin mode refuses every operation that ACTS, for the reason above. The one that ASKS is
		// not let off it — it cannot place its rectangle either — but it is still let through, because
		// a question dropped on the floor leaves the program that asked waiting on a terminal that has
		// already moved on (§33).
		//
		// SL and SR USED to be caught by this guard, for a reason that was never quite the same one
		// (§100). They name no coordinates, so origin mode cannot misplace them; origin mode was
		// serving as EVIDENCE instead — DECOM only means anything once DECSTBM has cut a scrolling
		// region, a shift ought to stop at that region's edges, and cmote could not see the region. So
		// where the one signal in reach said a region was probably there, cmote did nothing rather
		// than shift rows the program had walled off. It was a partial guard and the row in §8 said
		// so: a region set WITHOUT origin mode was invisible from here.
		//
		// §102 removed the blindness rather than the guard. The scrolling region is mirrored now
		// (`term/region.rs`), so a shift can be BOUNDED by the real band instead of refused on a
		// proxy for it — which is both more sequences honoured and a stricter answer, since a region
		// set without origin mode used to be shifted straight through. So the shift leaves this guard
		// and takes its bound from the mirror, and the refusal it inherited is retired.
		let origin = self.term.mode().contains(TermMode::ORIGIN);
		if origin
			&& !matches!(
				request,
				rect::RectRequest::Checksum { .. }
					| rect::RectRequest::Shift { .. }
					| rect::RectRequest::Columns { .. }
			) {
			return;
		}
		let (rows, cols) = {
			let grid = self.term.grid();
			(grid.screen_lines(), grid.columns())
		};
		// The four content operations are always the box, and so is the checksum. DECSACE picks
		// between the box and the wrapped run for the attribute pair alone (§59), which is why the
		// extent is a parameter of `from_corners` rather than a mode it reads: the call site is what says
		// which family it belongs to.
		match request {
			rect::RectRequest::Erase(corners) => {
				if let Some(bounds) =
					rect::from_corners(corners, rect::RectExtent::Rectangle, rows, cols)
				{
					self.erase_rect(bounds, false);
				}
			}
			rect::RectRequest::SelectiveErase(corners) => {
				if let Some(bounds) =
					rect::from_corners(corners, rect::RectExtent::Rectangle, rows, cols)
				{
					self.erase_rect(bounds, true);
				}
			}
			rect::RectRequest::Fill(glyph, corners) => {
				if let Some(bounds) =
					rect::from_corners(corners, rect::RectExtent::Rectangle, rows, cols)
				{
					self.fill_rect(glyph, bounds);
				}
			}
			rect::RectRequest::Attributes {
				corners,
				extent,
				change,
			} => {
				if change.is_empty() {
					return;
				}
				if let Some(bounds) = rect::from_corners(corners, extent, rows, cols) {
					self.attribute_rect(bounds, extent, change, cols);
				}
			}
			rect::RectRequest::Copy { source, top, left } => {
				let Some(source) =
					rect::from_corners(source, rect::RectExtent::Rectangle, rows, cols)
				else {
					return;
				};
				if let Some((source, to_row, to_col)) =
					rect::copy_extent(source, top, left, rows, cols)
				{
					self.copy_rect(source, to_row, to_col);
				}
			}
			rect::RectRequest::Shift { direction, columns } => {
				self.shift_columns(direction, usize::from(columns).min(cols));
			}
			rect::RectRequest::Unscroll { lines } => {
				self.unscroll(usize::from(lines).min(rows));
			}
			rect::RectRequest::Columns { columns, insert } => {
				self.shift_band_columns(usize::from(columns), insert);
			}
			rect::RectRequest::Checksum { id, corners } => {
				// A rectangle that holds no cells — crossed corners, a corner off the page, or the
				// origin-mode refusal above — is answered with the checksum of nothing, which is
				// what a real terminal reports for an empty area and is not a special case here:
				// `Checksum::default().finish()` is 0 because no cell was ever weighed.
				let bounds = if origin {
					None
				} else {
					rect::from_corners(corners, rect::RectExtent::Rectangle, rows, cols)
				};
				let checksum = bounds.map_or(0, |bounds| self.checksum_rect(bounds));
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
	fn checksum_rect(&self, bounds: rect::Rect) -> u16 {
		let grid = self.term.grid();
		let mut checksum = rect::Checksum::default();
		for row in bounds.rows() {
			for column in bounds.columns() {
				let cell = &grid[page_line(row)][Column(column)];
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
	fn erase_rect(&mut self, bounds: rect::Rect, selective: bool) {
		let background = self.term.grid().cursor.template.bg;
		let grid = self.term.grid_mut();
		for row in bounds.rows() {
			for column in bounds.columns() {
				let cell = &mut grid[page_line(row)][Column(column)];
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
	fn fill_rect(&mut self, glyph: char, bounds: rect::Rect) {
		let mut template = self.term.grid().cursor.template.clone();
		template.c = glyph;
		let grid = self.term.grid_mut();
		for row in bounds.rows() {
			for column in bounds.columns() {
				grid[page_line(row)][Column(column)] = template.clone();
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
	fn attribute_rect(
		&mut self,
		bounds: rect::Rect,
		extent: rect::RectExtent,
		change: rect::AttributeChange,
		cols: usize,
	) {
		let grid = self.term.grid_mut();
		for row in bounds.rows() {
			for column in bounds.columns_on(row, extent, cols) {
				let flags = &mut grid[page_line(row)][Column(column)].flags;
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
	fn copy_rect(&mut self, source: rect::Rect, to_row: usize, to_col: usize) {
		let cells: Vec<Cell> = {
			let grid = self.term.grid();
			source
				.rows()
				.flat_map(|row| {
					source
						.columns()
						.map(move |column| grid[page_line(row)][Column(column)].clone())
				})
				.collect()
		};
		let width = source.width();
		let grid = self.term.grid_mut();
		for (index, cell) in cells.into_iter().enumerate() {
			let row = to_row + index / width;
			let column = to_col + index % width;
			grid[page_line(row)][Column(column)] = cell;
		}
	}

	/// Shift every row of the visible page sideways — SL and SR (§100).
	///
	/// Each row is read out whole before any of it is written, for DECCRA's reason (`copy_rect`): the
	/// source and destination overlap by definition here, so the copy is done through a buffer rather
	/// than by choosing a direction to walk in and hoping the arithmetic holds.
	///
	/// Whole cells move, so a glyph's colours, attributes, OSC 8 link and DECSCA protection travel
	/// with it — the rule DECCRA keeps, and for the same reason: protection makes a cell unerasable,
	/// not immovable (§56). The cells shifted off the edge are gone; the ones that arrive are the
	/// pen's background, which is what the erases write and what makes a shift over a coloured screen
	/// leave a strip in that colour rather than in the default one.
	///
	/// **The cursor does not move.** SL and SR shift the data under it, so a cursor at column 40 is
	/// still at column 40 afterwards, now over whatever slid into that cell. That is ECMA-48's and
	/// xterm's behaviour and it is also the reason this needs no cursor bookkeeping at all: unlike a
	/// translation into per-row DCH, which would have had to save and restore a cursor whose one saved
	/// slot belongs to the program (§57).
	///
	/// `columns` arrives already defaulted to at least 1 and clamped to the page width, so a shift by
	/// the whole width blanks the page and a shift by more than it cannot run off the end.
	fn shift_columns(&mut self, direction: rect::RectDirection, columns: usize) {
		let cols = self.term.grid().columns();
		if cols == 0 || columns == 0 {
			return;
		}
		// The rows the shift may touch: the vertical scrolling region, not the whole page (§102). A
		// shift is a scrolling operation, and DECSTBM walls off the rows a scrolling operation may
		// move — so a status line parked outside the band stays where it is while the band slides
		// under it. Until the region could be read back this was the argument for refusing the
		// sequence outright under origin mode (§100); now it is the argument for a loop bound.
		let band = self.region;
		let background = self.term.grid().cursor.template.bg;
		let grid = self.term.grid_mut();
		for row in band.first_row()..=band.last_row() {
			let line = page_line(row);
			let source: Vec<Cell> = (0..cols)
				.map(|column| grid[line][Column(column)].clone())
				.collect();
			for column in 0..cols {
				// Where this cell's new content comes from, or `None` for the edge the content moved
				// away from. `checked_sub` and the bound check are what make the edges fall out of
				// the same loop as the middle instead of being two more loops to keep in step.
				let from = match direction {
					rect::RectDirection::Left => Some(column + columns).filter(|from| *from < cols),
					rect::RectDirection::Right => column.checked_sub(columns),
				};
				grid[line][Column(column)] = match from {
					Some(from) => source[from].clone(),
					None => background.into(),
				};
			}
			// A wide glyph occupies two cells, and a shift can push exactly one of them off the page —
			// leaving a lead with no spacer at the right edge, or a spacer with no lead at the left.
			// Neither is a state the renderer or the reflow expects to meet, and the half that is left
			// is not a character anybody asked for, so it is blanked. Only the two edge columns can be
			// in this state: every other pair moved together.
			let stranded = match direction {
				rect::RectDirection::Left => grid[line][Column(0)]
					.flags
					.contains(Flags::WIDE_CHAR_SPACER)
					.then_some(0),
				rect::RectDirection::Right => grid[line][Column(cols - 1)]
					.flags
					.contains(Flags::WIDE_CHAR)
					.then_some(cols - 1),
			};
			if let Some(column) = stranded {
				grid[line][Column(column)] = background.into();
			}
		}
	}

	/// Insert or delete whole COLUMNS at the cursor — DECIC and DECDC (§102).
	///
	/// The vertical twins of IL and DL: where those open or close a row across the band's columns,
	/// these open or close a column across the region's rows. A column pushed past the right margin
	/// is gone; the columns that arrive are the pen's background, as they are for every other
	/// operation here that has to lay something down behind itself (§58, §100).
	///
	/// **Legal without margins**, and this is why they are applied here rather than in the gate: with
	/// no margins the band is the whole page, so the operation still means something — which is not
	/// true of anything the gate takes over, all of which are the engine's own until a margin
	/// narrows them. `Margins::band` is what resolves "no margins" to a band instead of a refusal.
	///
	/// Refused when the cursor sits outside the band or outside the scrolling region, which is
	/// xterm's test and the same one IL and DL apply: there is no column to open from out there, and
	/// guessing one would move text the program had walled off.
	fn shift_band_columns(&mut self, columns: usize, insert: bool) {
		let cols = self.term.grid().columns();
		let (left, right) = self.margins.band(cols);
		let (top, bottom) = (self.region.first_row(), self.region.last_row());
		let (row, column) = {
			let cursor = self.term.grid().cursor.point;
			(cursor.line.0.max(0) as usize, cursor.column.0)
		};
		if bottom < top || column < left || column > right || row < top || row > bottom {
			return;
		}
		let room = right - column + 1;
		let columns = columns.min(room);
		if columns == 0 {
			return;
		}
		let background = self.term.grid().cursor.template.bg;
		let grid = self.term.grid_mut();
		for row in top..=bottom {
			let line = page_line(row);
			if columns < room {
				// Read out and write back through a buffer, for `copy_rect`'s reason: the source and
				// destination overlap by definition, so choosing a walk direction and hoping the
				// arithmetic holds is the version of this that goes wrong quietly.
				let source: Vec<Cell> = (left..=right)
					.map(|column| grid[line][Column(column)].clone())
					.collect();
				for destination in column..=right {
					let from = if insert {
						destination
							.checked_sub(columns)
							.filter(|from| *from >= column)
					} else {
						Some(destination + columns).filter(|from| *from <= right)
					};
					grid[line][Column(destination)] = match from {
						Some(from) => source[from - left].clone(),
						None => background.into(),
					};
				}
			} else {
				for destination in column..=right {
					grid[line][Column(destination)] = background.into();
				}
			}
		}
	}

	/// Scroll the page down and fill the top from the SCROLLBACK — kitty's UNSCROLL (§101).
	///
	/// The sequence exists for one situation: a shell prints a block of completions under the cursor,
	/// the screen scrolls, and text the user was reading is pushed into the scrollback. When the
	/// completion is over the shell asks for those lines back. Plain SD would scroll the page down and
	/// fill with BLANKS, which erases exactly the text this is meant to restore, so the fill has to
	/// come from the history or not happen at all.
	///
	/// The lines are **moved**, not copied — kitty's specification is explicit, and it is the only
	/// reading that leaves the document coherent: a copy would leave the user scrolling back over the
	/// same text twice, once per completion, for as long as the session lasts.
	///
	/// WHAT THE ENGINE GIVES AND WHAT IT DOES NOT. Rows can be read and written at any line, history
	/// included (`Line(-1)` is the newest scrollback row). What has no accessor is *shortening* the
	/// history at the end nearest the page: `Grid::update_history` shrinks by dropping the OLDEST
	/// rows, because `Storage::shrink_lines` only lowers a length and the ring's index arithmetic puts
	/// the oldest row at the far end. So the newest rows cannot be dropped — they have to be
	/// **overwritten**. This walks the remaining history up over the consumed rows, leaving the
	/// spares at the oldest end, and then asks `update_history` to drop exactly that many from there.
	/// The limit is put straight back, since `update_history` sets it as well as shrinking.
	///
	/// Rows are MOVED rather than cloned, through `mem::replace` with a single row cloned up front as
	/// the placeholder supply. A deep copy per row would be a megabyte of cells on a full scrollback,
	/// on a sequence a shell may send on every tab press; moving a `Row` moves a `Vec` header.
	///
	/// The ALTERNATE screen needs no special case and gets none. That page keeps no history, so
	/// `from_history` is zero, every inserted line is blank, and what happens is exactly SD — which is
	/// what kitty's specification requires there ("if there is no scrollback buffer, the newly
	/// inserted lines must be empty").
	fn unscroll(&mut self, lines: usize) {
		let (rows, cols) = {
			let grid = self.term.grid();
			(grid.screen_lines(), grid.columns())
		};
		if lines == 0 || cols == 0 || rows == 0 {
			return;
		}
		let history = self.term.grid().history_size();
		// Only what the scrollback actually holds can be restored; the rest of the request is blanks,
		// which is the specification's own answer for a terminal with nothing to give back.
		let from_history = lines.min(history);
		let blanks = lines - from_history;
		let background = self.term.grid().cursor.template.bg;
		{
			let grid = self.term.grid_mut();
			// One deep copy, and the only one: every move below leaves this row behind as a
			// placeholder and takes the next one out, so a single spare circulates through the lot.
			let mut carry = grid[Line(0)].clone();
			// The page slides down, from the bottom up so a row is read before it is written over.
			for destination in (lines..rows).rev() {
				let source = destination - lines;
				let row = std::mem::replace(&mut grid[page_line(source)], carry);
				carry = std::mem::replace(&mut grid[page_line(destination)], row);
			}
			// The scrollback's newest rows come in above what was already on the page, newest
			// lowest, so the restored text joins the text it used to sit above with no seam.
			for taken in 0..from_history {
				let source = history_line(taken);
				let destination = page_line(lines - 1 - taken);
				let row = std::mem::replace(&mut grid[source], carry);
				carry = std::mem::replace(&mut grid[destination], row);
			}
			// Whatever the scrollback could not fill is blanked in the pen's background, the same
			// thing an erase writes — these rows are recycled and still hold their old text.
			for row in 0..blanks {
				for column in 0..cols {
					grid[page_line(row)][Column(column)] = background.into();
				}
			}
			// Close the gap the consumed rows left: the rest of the history walks up over them, and
			// the placeholders end up at the oldest end, which is the end that can be dropped.
			// `step` counts from 1 here, so each `history_line` argument is one less than the step —
			// `history_line(0)` being `Line(-1)`, the newest scrolled-off row.
			for step in 1..=(history - from_history) {
				let source = history_line(step + from_history - 1);
				let destination = history_line(step - 1);
				let row = std::mem::replace(&mut grid[source], carry);
				carry = std::mem::replace(&mut grid[destination], row);
			}
		}
		if from_history > 0 {
			let grid = self.term.grid_mut();
			// Drops exactly the placeholders parked at the oldest end. The second call restores the
			// retention limit the first one lowered — it only shrinks when the history is longer than
			// what it is given, so this adds nothing back.
			grid.update_history(history - from_history);
			grid.update_history(SCROLLBACK);
		}
		// And now the part that makes this more than a grid operation: every line number cmote
		// remembers about the session has to move with the text (§101).
		let moved = rect::Unscrolled::new(history, rows, lines, from_history);
		self.prompts.renumber(|line| moved.map(line));
		self.graphics.renumber(|line| moved.map(line));
		self.paths.renumber(|line| moved.map(line));
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

	/// The mouse pointer shape a program last asked for over this grid (OSC 22, §77).
	///
	/// `Shape::Default` until one does, and again once one hands the pointer back or a full-screen
	/// program starts or ends. Only ever one of the five shapes the module's allow-list passes, so a
	/// caller drawing this value cannot draw a refused one — the check is the parser, not the
	/// renderer. `ui::terminal` is where it becomes an iced `Interaction`, and it is scoped to the
	/// grid widget alone: the shape stops at the edge of the terminal's own rectangle.
	pub fn pointer_shape(&self) -> pointer::Shape {
		self.pointer.shape()
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
		// The engine throws the scrolling region away on every resize — `Term::resize` assigns the
		// full page back over it, whatever DECSTBM had set. That happens INSIDE the call above, with
		// no sequence on the wire and no `Handler` method to watch, so the mirror is corrected here.
		// One of the two writers `term/region.rs` names that this file, not the gate, is answerable
		// for (§102).
		self.region.reset(rows as usize);
		// And the margins go with it (§102). A band of columns 10 to 40 means nothing on a window
		// that is now 30 columns wide, and reflow makes it worse than arbitrary: the text those
		// columns held has moved. xterm drops margins on resize for the same reason.
		self.margins.reset();
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
		screen::Screen::new(&self.term, &self.paths)
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
	pub fn jump_prompt(&mut self, direction: osc133::Osc133Direction) -> bool {
		let grid = self.term.grid();
		let history = grid.history_size();
		let offset = grid.display_offset();
		let Some(target) = self.prompts.jump(direction, history, offset) else {
			return false;
		};
		// Both are viewport offsets in the same document, so the subtraction is done in the line-number
		// domain rather than on two `usize`s — where `target < offset`, a scroll DOWN, would wrap.
		let delta = as_line_number(target) - as_line_number(offset);
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
				self.term.scroll_display(Scroll::Delta(scroll_delta(delta)));
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
	/// case-insensitively by `search::SearchRow` — the pure half of this, tested without an engine. The
	/// scan is a full walk of the grid, so it costs `history + rows` × `columns` cell reads; that is
	/// a few million at the SCROLLBACK cap, cheap enough to redo on each keystroke in the find bar
	/// and far simpler than maintaining an index that every scroll and reflow could invalidate.
	///
	/// `ponytail:` matches are found within one grid ROW, so a hit that straddles the wrap of a
	/// long logical line is not found (the two halves are separate rows), and a cell's combining
	/// marks are not searched — only its base glyph. An empty query finds nothing.
	pub fn find(&self, query: &str) -> Vec<search::SearchMatch> {
		if query.is_empty() {
			return Vec::new();
		}
		let grid = self.term.grid();
		let history = as_line_number(grid.history_size());
		let screen_lines = as_line_number(self.term.screen_lines());
		let columns = self.term.columns();
		let mut out = Vec::new();
		// The engine stores history on the NEGATIVE lines below the active screen's line 0, so the
		// whole document is `-history ..= the last screen line`; absolute = history + line puts line
		// 0 (the top of the active screen) at absolute `history_size`, as `osc133` records it.
		for line in -history..screen_lines {
			let mut row = search::SearchRow::new((history + line) as u64);
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
				self.term.scroll_display(Scroll::Delta(scroll_delta(delta)));
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
	/// `SetMark` drops. Fed by the same interruption advance as the prompt marks and for the same reason: a
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
	/// Reads the mouse pointer shape a remote asks for over its own grid (OSC 22, §77). Same
	/// latest-value shape as the two above and read out of the same bytes for the same reason — the
	/// engine parses this one and then drops it into an empty default `Handler` method. Its module
	/// is an ALLOW-LIST of five shapes, which is what keeps a remote from painting cmote's own
	/// drag-and-resize vocabulary over the grid or claiming that cmote itself is busy.
	pointer: pointer::Pointer,
	/// Reads the selective-erase sequences the engine drops — DECSCA, DECSED and DECSEL (§56). Fed by
	/// the interruption advance, but for the opposite reason to the marks above: each of its requests has to
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
	/// DECCRA (§58), then DECCARA, DECRARA and DECSACE (§59). Fed by the interruption advance for the same
	/// reason as the selective erase: each one is applied with the engine advanced PAST the sequence
	/// it ignored. The module is the grammar and the geometry, and the cells are written below; the
	/// one thing it does hold is DECSACE's extent, because only the scanner sees a mode and the
	/// requests it governs in stream order.
	rectangles: rect::Rectangles,
	/// Finds DECST8C, the tab-stop reset `vte` parses and `alacritty_terminal` drops on the floor
	/// (§74). Fed by the interruption advance like the erases above and for the same reason — the engine
	/// has to be past the sequence it ignores before cmote answers it — and answered the way §72
	/// answers the soft reset: by feeding the engine the same request written in TBC, HTS and CUF,
	/// so the engine stays the only writer of its own tab table. Holds no state but the scan.
	tabs: tabs::Tabs,
	/// Finds DECXCPR, the DEC-private spelling of the cursor-position question, which `vte` has no arm
	/// for at all — its CSI table holds `('n', [])` and no `('n', [b'?'])` (§82). Fed by the split
	/// advance because a position report is only true where it sits: answered after the chunk, it would
	/// report where the cursor ENDED UP rather than where the question was asked. The other nine values
	/// of `CSI ? Ps n` are refused by the same scanner, on an allow-list one value wide.
	dsr: dsr::Dsr,
	/// Finds XTPUSHSGR / XTPOPSGR, which `vte` has no arm for either — `csi_dispatch` never matches a
	/// `#` intermediate at all (§84, §85). Fed by the interruption advance for DECXCPR's reason: the pen a push
	/// saves is the pen where the push was WRITTEN, not where the chunk ended.
	sgr_stack: sgrstack::SgrStack,
	/// The pens that stack has saved, innermost last, each with the mask of what its push named. Ten
	/// deep, as xterm's is (`sgrstack::DEPTH`), which is the whole of what a remote can make cmote hold
	/// here. Cleared by nothing: a stack outlives a chunk by definition, and a program that never pops
	/// simply leaves it standing.
	saved_pens: Vec<(sgrstack::Mask, Cell)>,
	/// How many pushes were dropped for overflow and are therefore owed a dropped POP. Without this the
	/// pop that matches a dropped push would restore the level ABOVE it and every pop after that would
	/// be one out — see `term/sgrstack.rs` for why xterm's own behaviour was not copied here.
	dropped_pushes: usize,
	/// Finds SCP, the character path, which no part of the engine has an arm for (§76). Reported by
	/// the split feed like the prompt marks, because the sequence acts on "the line the cursor is on"
	/// and that is only knowable at the point in the stream it sits.
	scp: scp::Scp,
	/// Which document lines that sequence put right to left. Held here rather than in the engine —
	/// the engine has no notion of a direction and does not need one: the grid stays in the order the
	/// host sent, and the mirroring is a rule the RENDERER applies when it derives a frame from it. So
	/// the scrollback, the search, the selection and a copy all go on reading data order, which is the
	/// whole reason this is buildable without becoming a second writer of the grid (§71, §73).
	paths: scp::Paths,
	/// Finds the inline sixel images the engine drops, decodes them and holds where each one sits
	/// (§41). Fed by the same interruption advance as the prompt marks, and for the same reason: a picture
	/// belongs at the cursor's line and column at the moment it arrived in the stream.
	graphics: graphics::Images,
	/// Whether the ALTERNATE screen was the one up last time this was looked at (§41). The engine
	/// tracks the swap itself and `Screen::is_alternate` reads it back at any moment; what this adds
	/// is the EDGE. A full-screen program's pictures belong to that program, so both swaps — on and
	/// off the page — throw them away, and noticing that needs the previous answer as well as the
	/// current one.
	on_alternate: bool,
	/// The engine's vertical scrolling region, mirrored (§102). The engine implements DECSTBM fully
	/// and then keeps the answer to itself — `Term::scroll_region` is private, and the sequence that
	/// would report it reaches an arm the engine leaves empty. Kept here because everything that
	/// writes it is observable: the two sequence-driven writes arrive through the gate, and the two
	/// resets are calls cmote itself makes. `term/region.rs` names all four and what would break the
	/// mirror.
	region: region::ScrollRegion,
	/// The left and right margins, DECSLRM's half of the same object the region above is (§102).
	/// Unlike the region this is not a mirror of anything — the engine has no margins, so cmote is
	/// their only holder and the gate performs every operation they change.
	margins: margins::Margins,
}

/// One thing `process` has to do part-way through a chunk (§34, §41, §55). Each scanner reports the
/// byte offset its event sits at, and the engine can only be advanced forwards, so the lists are
/// merged into this single ordered one — otherwise applying all the marks and then all the images
/// would place the later kinds at the wrong point in the stream.
enum Interruption {
	Prompt(osc133::Mark),
	Graphics(graphics::GraphicsEvent),
	/// An explicit bookmark a script dropped with `OSC 1337 ; SetMark` (§55). Carries nothing: the
	/// whole content of the event is the line it arrived on, which is why it has to be applied here
	/// rather than after the chunk.
	UserMark,
	/// A selective-erase request (§56). The odd one out in this list: every other kind is applied
	/// with the engine advanced UP TO its offset, because the cursor then names the line the event
	/// belongs on, while `protect` reports offsets one past the sequence so its requests land on the
	/// far side of it. Both work through the same loop because an interruption is an interruption — the difference is
	/// only which side of the boundary the scanner asked for.
	Protect(protect::ProtectRequest),
	/// A parametrised `CSI … s` — DECSLRM or SCOSC, depending on whether mode 69 is set (§57, §102).
	/// The only interruption whose offset is the final byte ITSELF rather than one side of the sequence,
	/// because with the mode on the loop replaces that byte with a CAN rather than feeding it.
	Margins(cancel::CancelRequest),
	/// A rectangular area operation (§58, §59) — erase, fill, copy or restyle a box of cells. Applied
	/// on the far side of its sequence, as a selective erase is, and for the same reason.
	Rect(rect::RectRequest),
	/// DECST8C, the tab stops put back every eight columns (§74). Carries nothing — the sequence has
	/// one meaning and no parameters beyond the `5` that identifies it, so the offset is the whole
	/// event. Applied on the far side of its sequence, as the two above are.
	TabStops,
	/// One of the DEC-private status reports cmote answers — DECXCPR, or a locator question (§82,
	/// §93). The only interruption in this list that produces a REPLY rather than an effect, which is why
	/// DECXCPR has to be here at all: the cursor it reports is the cursor with the engine advanced
	/// exactly to the question. The two locator answers are constants and ride along because they
	/// come out of the same scanner.
	Dsr(dsr::DsrRequest),
	/// A character path for the line the cursor is on, or the RIS that forgets them all (§76).
	Path(scp::ScpRequest),
	/// A push or pop of the video-attribute stack (§85). Applied on the far side of its sequence, and
	/// the only interruption whose effect is carried out by FEEDING the engine — a pop is spelled back as the
	/// SGR that restores the pen, so the engine stays the only writer of its own template (§71, §73).
	SgrStack(sgrstack::SgrStackRequest),
}

/// Everything one chunk's scanners found, before it is merged into stream order.
///
/// A struct rather than eight positional arguments, which is where this arrived once §76 added the
/// eighth: with lists this similar in type, an argument transposed at the call site would compile
/// and then apply the wrong event at the wrong offset. Named fields make that a build error, and the
/// emptiness test — the fast path every ordinary chunk takes — belongs with them rather than spelled
/// out at the one call site.
#[derive(Default)]
struct Scanned {
	marks: Vec<(usize, osc133::Mark)>,
	images: Vec<(usize, graphics::GraphicsEvent)>,
	bookmarks: Vec<(usize, iterm::Report)>,
	protections: Vec<(usize, protect::ProtectRequest)>,
	cancels: Vec<cancel::CancelRequest>,
	rectangles: Vec<(usize, rect::RectRequest)>,
	tab_resets: Vec<usize>,
	cursor_requests: Vec<(usize, dsr::DsrRequest)>,
	paths: Vec<(usize, scp::ScpRequest)>,
	sgr_stack: Vec<(usize, sgrstack::SgrStackRequest)>,
}

impl Scanned {
	/// Whether no scanner found anything — the overwhelmingly common chunk, which is then fed to the
	/// engine in one advance and pays for none of the machinery below.
	fn is_empty(&self) -> bool {
		self.marks.is_empty()
			&& self.images.is_empty()
			&& self.bookmarks.is_empty()
			&& self.protections.is_empty()
			&& self.cancels.is_empty()
			&& self.rectangles.is_empty()
			&& self.tab_resets.is_empty()
			&& self.cursor_requests.is_empty()
			&& self.paths.is_empty()
			&& self.sgr_stack.is_empty()
	}
}

/// Merge one chunk's prompt marks, image events, bookmarks, selective-erase requests and cancelled
/// final bytes into offset order. Every list arrives ascending, and the sort is stable, so two events
/// at the very same offset keep the order they were scanned in — which is the only sensible
/// tie-break, since no scanner can see another's.
fn interruptions(scanned: Scanned) -> Vec<(usize, Interruption)> {
	let Scanned {
		marks,
		images,
		bookmarks,
		protections,
		cancels,
		rectangles,
		tab_resets,
		cursor_requests,
		paths,
		sgr_stack,
	} = scanned;
	let mut merged: Vec<(usize, Interruption)> = Vec::with_capacity(
		marks.len()
			+ images.len()
			+ bookmarks.len()
			+ protections.len()
			+ cancels.len()
			+ rectangles.len()
			+ tab_resets.len()
			+ cursor_requests.len()
			+ paths.len()
			+ sgr_stack.len(),
	);
	merged.extend(
		marks
			.into_iter()
			.map(|(offset, mark)| (offset, Interruption::Prompt(mark))),
	);
	merged.extend(
		images
			.into_iter()
			.map(|(offset, event)| (offset, Interruption::Graphics(event))),
	);
	merged.extend(bookmarks.into_iter().map(|(offset, report)| {
		let interruption = match report {
			iterm::Report::Mark => Interruption::UserMark,
		};
		(offset, interruption)
	}));
	merged.extend(
		protections
			.into_iter()
			.map(|(offset, request)| (offset, Interruption::Protect(request))),
	);
	merged.extend(
		cancels
			.into_iter()
			.map(|request| (request.offset, Interruption::Margins(request))),
	);
	merged.extend(
		rectangles
			.into_iter()
			.map(|(offset, request)| (offset, Interruption::Rect(request))),
	);
	merged.extend(
		tab_resets
			.into_iter()
			.map(|offset| (offset, Interruption::TabStops)),
	);
	merged.extend(
		cursor_requests
			.into_iter()
			.map(|(offset, request)| (offset, Interruption::Dsr(request))),
	);
	merged.extend(
		paths
			.into_iter()
			.map(|(offset, request)| (offset, Interruption::Path(request))),
	);
	merged.extend(
		sgr_stack
			.into_iter()
			.map(|(offset, request)| (offset, Interruption::SgrStack(request))),
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
fn is_covered(screen: &screen::Screen<'_>, placement: &graphics::Placement) -> bool {
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

/// The SGR that restores a pen exactly — every attribute the engine can hold, spelled in sequences it
/// parses (§85).
///
/// Deliberately **not** `pen_sgr` above, though the two look alike. That one builds a DECRQSS *reply*
/// and reports a curly, dotted or dashed underline as a plain `4`, which is the truthful answer to
/// "am I underlined?" without claiming a substyle that is an extension. Here the string is fed back
/// into the engine, so a coarse answer is a LOSS: a pop would turn the program's curly underline
/// straight. So the substyles get their own spellings (`4:3` / `4:4` / `4:5`), and the underline
/// colour — which SGR 58 carries and DECRQSS never reports — comes back too.
///
/// It opens with `0`, a full reset, and then names everything set. That is what makes a restore exact
/// rather than additive: an attribute the saved pen did not have is cleared by the reset instead of
/// needing its own "off" code, which is where xterm's own `22` (neither bold nor faint) would take two
/// attributes out at once.
fn pen_restore(flags: Flags, fg: Color, bg: Color, underline: Option<Color>) -> String {
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
	// One underline at a time: the engine's flags are variants of one attribute, and the double is the
	// only one with a plain SGR code of its own.
	if flags.contains(Flags::DOUBLE_UNDERLINE) {
		codes.push("21".to_string());
	} else if flags.contains(Flags::UNDERCURL) {
		codes.push("4:3".to_string());
	} else if flags.contains(Flags::DOTTED_UNDERLINE) {
		codes.push("4:4".to_string());
	} else if flags.contains(Flags::DASHED_UNDERLINE) {
		codes.push("4:5".to_string());
	} else if flags.contains(Flags::UNDERLINE) {
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
	if let Some(foreground) = sgr_color(fg, false) {
		codes.push(foreground);
	}
	if let Some(background) = sgr_color(bg, true) {
		codes.push(background);
	}
	if let Some(colour) = underline.and_then(sgr_underline_color) {
		codes.push(colour);
	}
	format!("\x1b[{}m", codes.join(";"))
}

/// The underline's own colour as SGR 58 (§85). `None` when it is a role rather than a colour, which
/// the leading reset already puts back to "same as the text".
///
/// SGR 58 has no named form the way 30-37 do, so a named colour goes out as its palette index — which
/// is what it is.
fn sgr_underline_color(color: Color) -> Option<String> {
	match color {
		Color::Named(named) => {
			let index = named as usize;
			(index <= 15).then(|| format!("58;5;{index}"))
		}
		Color::Indexed(index) => Some(format!("58;5;{index}")),
		Color::Spec(rgb) => Some(format!("58;2;{};{};{}", rgb.r, rgb.g, rgb.b)),
	}
}

/// The pen a pop restores: the current one, with the attributes the matching push NAMED taken from the
/// pen it saved (§85).
///
/// A push with no parameters names everything, which is the ordinary case and reduces to "the saved pen
/// entire". A selective push is the interesting one — `CSI 4 # {` saves the underline and nothing else,
/// so a pop has to put the underline back while leaving whatever the program has done to the colours
/// since. Merging at POP time rather than emitting per-attribute "off" codes is what keeps that exact:
/// the target pen is computed first and then written once, so no code in the string can take an
/// attribute out that the merge meant to keep.
///
/// Two of xterm's eleven parameters cannot select anything here and are documented where they are
/// parsed: `Ps = 5`, blink, which the engine has no flag for at all, and the difference between `4` and
/// `21`, which name one underline field between them.
fn merged_pen(current: &Cell, saved: &Cell, mask: sgrstack::Mask) -> String {
	// Every underline variant moves together — they are one attribute in the engine, not five.
	let underlines = Flags::UNDERLINE
		| Flags::DOUBLE_UNDERLINE
		| Flags::UNDERCURL
		| Flags::DOTTED_UNDERLINE
		| Flags::DASHED_UNDERLINE;
	let groups = [
		(sgrstack::Mask::BOLD, Flags::BOLD),
		(sgrstack::Mask::FAINT, Flags::DIM),
		(sgrstack::Mask::ITALIC, Flags::ITALIC),
		(sgrstack::Mask::UNDERLINE, underlines),
		(sgrstack::Mask::DOUBLY_UNDERLINED, underlines),
		(sgrstack::Mask::INVERSE, Flags::INVERSE),
		(sgrstack::Mask::INVISIBLE, Flags::HIDDEN),
		(sgrstack::Mask::CROSSED_OUT, Flags::STRIKEOUT),
	];
	let mut flags = current.flags;
	for (attribute, group) in groups {
		if mask.contains(attribute) {
			flags.remove(group);
			flags.insert(saved.flags & group);
		}
	}
	// `underline_masked`, not `underlined`: it says whether the mask COVERS the underline attributes,
	// not whether anything is underlined. One letter apart from the `underline` value below otherwise.
	let underline_masked = mask.contains(sgrstack::Mask::UNDERLINE)
		|| mask.contains(sgrstack::Mask::DOUBLY_UNDERLINED);
	let foreground = if mask.contains(sgrstack::Mask::FOREGROUND) {
		saved.fg
	} else {
		current.fg
	};
	let background = if mask.contains(sgrstack::Mask::BACKGROUND) {
		saved.bg
	} else {
		current.bg
	};
	let underline = if underline_masked {
		saved.underline_color()
	} else {
		current.underline_color()
	};
	pen_restore(flags, foreground, background, underline)
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

	/// The engine's line numbering, pinned (§111): 0 is the top of the page, and the scrollback runs
	/// NEGATIVE from -1 downwards. `history_line`'s whole job is that off-by-one, since it is the one
	/// row index in the file that counts the opposite way from the rest.
	#[test]
	fn a_page_row_and_a_scrollback_row_number_in_opposite_directions() {
		assert_eq!(page_line(0), Line(0), "row 0 is the top of the page");
		assert_eq!(page_line(23), Line(23));
		assert_eq!(
			history_line(0),
			Line(-1),
			"the newest scrolled-off row sits at -1, not 0"
		);
		assert_eq!(history_line(9_999), Line(-10_000), "a full SCROLLBACK back");
		// The two never name the same row, which is what makes reading one for the other a visible bug
		// rather than an off-screen one.
		assert!(history_line(0) < page_line(0));
	}

	/// A scroll distance saturates rather than wrapping, because the engine clamps a delta to the
	/// document anyway — so the clamped value lands on the same row the engine would have chosen.
	#[test]
	fn a_scroll_distance_saturates_at_the_ends_of_an_i32() {
		assert_eq!(scroll_delta(0), 0);
		assert_eq!(scroll_delta(-42), -42);
		assert_eq!(scroll_delta(i64::MAX), i32::MAX);
		assert_eq!(scroll_delta(i64::MIN), i32::MIN);
		// The boundary itself, both sides: one past is where a plain `as` would have wrapped the sign.
		assert_eq!(scroll_delta(i64::from(i32::MAX) + 1), i32::MAX);
		assert_eq!(scroll_delta(i64::from(i32::MIN) - 1), i32::MIN);
	}

	/// The invariant `as_line_number` rests on, stated as a test: a row count that does not fit an
	/// `i32` cannot come from a real geometry, so it is a bug and it is loud. Before §111 the same
	/// value truncated silently and the engine indexed some other row.
	#[test]
	#[should_panic(expected = "fits in an i32")]
	fn a_row_count_too_large_to_be_a_geometry_is_refused() {
		let _ = as_line_number(usize::MAX);
	}

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
	fn a_desktop_notification_gets_nothing_in_any_of_its_three_spellings() {
		// §79. ConEmu's `9;<text>`, urxvt's `777;notify;…` and kitty's `99;…` are one refused
		// feature in three dialects (§6, §54), and since §79 cmote's own code declines each by
		// name (`term::notify`) instead of leaving them to fall between the scanners.
		//
		// Three things are asserted, and the second is the one that makes this more than a
		// restatement of the module's own tests. No reply goes back. The tab's own state is
		// untouched — a progress report set BEFORE the notifications is still exactly where it
		// was, which is what would break first if a notification were ever read as one, and the
		// title is still the title. And none of the notification TEXT reaches the grid: a
		// remote's title and body are consumed as OSC payloads, never printed as characters.
		let mut terminal = Terminal::new(4, 40);
		terminal.process(b"\x1b]9;4;1;30\x07\x1b]2;window\x07");
		assert!(terminal.process(b"\x1b]9;Build finished\x07").is_empty());
		assert!(
			terminal
				.process(b"\x1b]777;notify;Build;finished in 4s\x07")
				.is_empty()
		);
		assert!(
			terminal
				.process(b"\x1b]99;i=1:d=0:p=title;Build finished\x07")
				.is_empty()
		);
		assert_eq!(terminal.progress(), progress::Progress::Working(30));
		assert_eq!(terminal.title().as_deref(), Some("window"));
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

	/// One sequence may ask about several slots — `OSC 4` reads its parameters in `index ; spec`
	/// pairs — and each gets its own reply, in the order asked (§87). Written down because the row
	/// claims it: §84's finding was a matrix full of definitions nothing had ever exercised.
	#[test]
	fn a_palette_query_may_ask_about_several_slots_at_once() {
		let mut terminal = Terminal::new(10, 40);
		let mut expected = b"\x1b]4;1;rgb:8080/0000/0000\x07".to_vec();
		expected.extend_from_slice(b"\x1b]4;3;rgb:8080/8080/0000\x07");
		assert_eq!(terminal.process(b"\x1b]4;1;?;3;?\x07"), expected);
	}

	/// The dynamic colours are one run rather than three codes: a list walks UP from the code it
	/// started at, so `OSC 10 ; ? ; ?` asks for the foreground and then the background (§87).
	#[test]
	fn a_default_colour_query_walks_up_from_the_code_it_started_at() {
		let mut terminal = Terminal::new(10, 40);
		let reply = terminal.process(b"\x1b]10;?;?\x07");
		let reply = String::from_utf8(reply).expect("the reply is ASCII");
		assert!(
			reply.contains("\x1b]10;rgb:"),
			"the foreground answers first: {reply:?}"
		);
		assert!(
			reply.contains("\x1b]11;rgb:"),
			"and the second `?` is the background: {reply:?}"
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
	fn the_conemu_spelling_of_the_tab_name_reaches_the_same_chip() {
		// §90. ConEmu's `OSC 9;3` is the same field as OSC 1, through the same module, so the two
		// spellings must land in the same place and neither may touch the title. Asserted with the
		// title set FIRST, so what this shows is the title SURVIVING — §77's ordering.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]2;window\x07");
		assert!(terminal.process(b"\x1b]9;3;\"build\"\x07").is_empty());
		assert_eq!(terminal.icon_name(), Some("build"));
		assert_eq!(terminal.title().as_deref(), Some("window"));
		// And the standard spelling still wins the field afterwards — one writer, two doors.
		terminal.process(b"\x1b]1;vim\x07");
		assert_eq!(terminal.icon_name(), Some("vim"));
	}

	/// The two ConEmu sub-codes cmote refuses (§90) reach the screen as nothing at all — no tab
	/// name, no printed text, no reply. A remote must not be able to sleep the terminal or raise a
	/// dialog, and the test that says so is the only thing standing between the policy and a later
	/// hand widening an arm.
	#[test]
	fn a_remotes_sleep_and_message_box_get_nothing() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b]1;kept\x07");
		assert!(terminal.process(b"\x1b]9;1;5000\x07").is_empty());
		assert!(
			terminal
				.process(b"\x1b]9;2;\"are you sure?\"\x07")
				.is_empty()
		);
		terminal.process(b"X");
		assert_eq!(read(&terminal, 0, 0, 20), "X", "none of it was printed");
		assert_eq!(
			terminal.icon_name(),
			Some("kept"),
			"and none of it named the tab"
		);
	}

	/// The two refusals §98 states by name, at the boundary (`term/notify.rs`). A remote asking cmote
	/// to change every font face, or handing it a command line to run locally the next time the
	/// terminal starts, gets nothing back and changes nothing — and, the part worth pinning here
	/// rather than in the module's own tests, none of the payload is PRINTED either: an `arm` carrying
	/// base64 must not spill onto the grid as text.
	#[test]
	fn a_remotes_font_change_and_relaunch_specification_get_nothing() {
		let mut terminal = Terminal::new(4, 40);
		terminal.process(b"\x1b]1;kept\x07");
		assert!(
			terminal
				.process(b"\x1b]60;regular=Comic Sans\x07")
				.is_empty()
		);
		assert!(
			terminal
				.process(b"\x1b]88;arm;cmd=c3No;args=aG9zdA==\x07")
				.is_empty()
		);
		assert!(
			terminal.process(b"\x1b]88;query\x07").is_empty(),
			"not even 'supported', which is what would bring the arm"
		);
		terminal.process(b"X");
		assert_eq!(read(&terminal, 0, 0, 40).trim(), "X", "none of it printed");
		assert_eq!(terminal.icon_name(), Some("kept"), "and none of it renamed");
	}

	/// contour's spelling of the tab name (§98) — the third door to the one writer `term/icon.rs` is,
	/// asserted at the boundary because that is where a new spelling could collide with an old one.
	#[test]
	fn the_third_tab_name_spelling_reaches_the_same_chip() {
		let mut terminal = Terminal::new(4, 40);
		terminal.process(b"\x1b]2;window\x07");
		assert!(terminal.process(b"\x1b]30;build\x07").is_empty());
		assert_eq!(terminal.icon_name(), Some("build"));
		assert_eq!(
			terminal.title().as_deref(),
			Some("window"),
			"and stays in its own lane"
		);
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
	fn a_pointer_shape_reaches_the_grid_without_touching_anything_else() {
		// §77. OSC 22 is parsed by `vte` and dropped into an empty default `Handler` method, so
		// this whole path is cmote's own scanner — and like the icon name it must stay in its lane:
		// the shape is the grid's, and the title, the chip and the text caret are left alone.
		// Setting one is state and not a reply, so no bytes go back either.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]2;window\x07\x1b]1;vim\x07");
		assert!(terminal.process(b"\x1b]22;pointer\x07").is_empty());
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Pointer);
		assert_eq!(terminal.title().as_deref(), Some("window"));
		assert_eq!(terminal.icon_name(), Some("vim"));
	}

	#[test]
	fn the_pointer_shapes_that_are_cmotes_own_are_refused_at_the_boundary() {
		// The refusal (§77), pinned where a remote would actually make it. `grab` and `col-resize`
		// are what cmote's drag handles and splitters say, and `wait` would say cmote itself is
		// hung — none of the three may be reachable from the wire.
		//
		// The order is the argument, the same way §69's is: a shape is set FIRST and the assertion
		// is that it SURVIVED. That makes the refusal a decision about bytes cmote had, rather than
		// a scanner that happened to be asleep.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]22;text\x07");
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Text);
		terminal.process(b"\x1b]22;grab\x07\x1b]22;col-resize\x07\x1b]22;wait\x07");
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Text);
	}

	#[test]
	fn a_full_screen_program_gives_the_pointer_back_when_it_quits() {
		// The alternate-screen swap clears the shape in both directions (§77), which is the whole
		// of the lifetime management this row needs: a TUI's hand must not be left hovering over
		// the shell prompt the user quit back to, and a TUI starting up must not inherit one.
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b]22;crosshair\x07\x1b[?1049h");
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Default);
		terminal.process(b"\x1b]22;pointer\x07");
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Pointer);
		terminal.process(b"\x1b[?1049l");
		assert_eq!(terminal.pointer_shape(), pointer::Shape::Default);
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
		// The ordering the merged interruption list exists for: both scanners report offsets into the same
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
				search::SearchMatch {
					line: 0,
					start_col: 0,
					end_col: 4
				},
				search::SearchMatch {
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

	/// DECST8C end to end (§74): a program moves its stops, then asks for the power-on ones back
	/// and gets them. Read through a printed HT, which is the only way tab stops are observable
	/// from outside the engine — the table itself is private, which is the whole reason this is
	/// answered by feeding the engine sequences rather than by writing it.
	#[test]
	fn a_tab_reset_puts_the_stops_back_where_a_program_moved_them() {
		let mut terminal = Terminal::new(4, 40);
		// Clear every stop, then set a single one at column 3.
		terminal.process(b"\x1b[3g\r\x1b[3C\x1bH\r");
		terminal.process(b"\t");
		assert_eq!(
			terminal.screen().cursor_position().1,
			3,
			"the program's own stop has to be in force, or the reset below proves nothing"
		);
		terminal.process(b"\r\x1b[?5W\t");
		assert_eq!(terminal.screen().cursor_position().1, 8);
		// And the rest of the walk landed too, not only its first stop.
		terminal.process(b"\t\t");
		assert_eq!(terminal.screen().cursor_position().1, 24);
	}

	/// The walk crosses the whole page and has to give the cursor back. A `tput init` mid-screen
	/// that left the shell's cursor in column 0 would be worse than the gap it closed — the same
	/// hazard §72 found in the fed soft reset, and the same test.
	#[test]
	fn a_tab_reset_leaves_the_cursor_where_it_found_it() {
		let mut terminal = Terminal::new(6, 40);
		terminal.process(b"\x1b[3;14H\x1b[?5W");
		assert_eq!(terminal.screen().cursor_position(), (2, 13));
	}

	/// Under origin mode with a scrolling region, `alacritty_terminal` routes CHA and VPA through a
	/// `goto` that adds the region's top to the line it is handed — while handing it the line the
	/// cursor is already on. A walk spelled with CHA would therefore drag the cursor down the page
	/// once per stop. This is the test that says the walk is spelled with CR and CUF instead, and
	/// it is the reason `term/tabs.rs` refuses the more natural spelling.
	#[test]
	fn a_tab_reset_under_origin_mode_leaves_the_row_alone() {
		let mut terminal = Terminal::new(8, 40);
		// A region that does not start at the top of the page is what makes the drag visible.
		terminal.process(b"\x1b[3;7r\x1b[?6h\x1b[2;5H");
		let before = terminal.screen().cursor_position();
		assert_eq!(
			before,
			(3, 4),
			"origin mode makes row 2 the region's second line"
		);
		terminal.process(b"\x1b[?5W");
		assert_eq!(terminal.screen().cursor_position(), before);
	}

	/// The walk is movement only, so the page underneath it is untouched. Pinned because a walk
	/// built out of printed spaces — which is how ncurses' own `reset` lays its stops down — would
	/// have wiped the line the cursor was on.
	#[test]
	fn a_tab_reset_prints_nothing_at_all() {
		let mut terminal = Terminal::new(4, 40);
		// A solid run of glyphs across every column the walk visits, so anything the walk printed
		// — a space included — would show up as a hole in it.
		terminal.process("X".repeat(40).as_bytes());
		terminal.process(b"\x1b[1;5H\x1b[?5W");
		assert_eq!(read(&terminal, 0, 0, 40), "X".repeat(40));
	}

	/// DECXCPR end to end (§82): the DEC-private spelling of "where is the cursor?" is answered, in
	/// xterm's two-parameter form with the `?` kept — and answered with the SAME numbers the engine
	/// gives for the ANSI spelling, which is asserted on the same terminal so the two cannot silently
	/// drift apart. That agreement is the property `dsr::cursor_reply` is built to keep: cmote reads
	/// the engine's cursor here, it never holds one of its own.
	#[test]
	fn the_dec_spelling_of_the_cursor_question_is_answered() {
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b[4;5H");
		assert_eq!(terminal.process(b"\x1b[?6n"), b"\x1b[?4;5R".to_vec());
		assert_eq!(
			terminal.process(b"\x1b[6n"),
			b"\x1b[4;5R".to_vec(),
			"the ANSI spelling is the engine's and must still answer, with the same numbers"
		);
	}

	/// The reason this one is answered inside the interruption advance rather than after the chunk: a cursor
	/// report is only true where the question sat. The query here is followed by ten more columns of
	/// output in the SAME chunk, so an answer built at the end would report column 11 rather than 1.
	#[test]
	fn the_cursor_report_answers_from_where_the_question_sat() {
		let mut terminal = Terminal::new(10, 40);
		let reply = terminal.process(b"\x1b[3;1H\x1b[?6nabcdefghij");
		assert_eq!(reply, b"\x1b[?3;1R".to_vec());
		assert_eq!(
			read(&terminal, 2, 0, 10),
			"abcdefghij",
			"and the text landed"
		);
	}

	/// Two questions in one write come back as two answers in the order they were asked, because the
	/// reply goes into the buffer the engine's own replies land in rather than into a second path
	/// alongside it (§60's rule, first written for DECRQCRA).
	#[test]
	fn the_two_spellings_answer_in_the_order_they_were_asked() {
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b[2;3H");
		assert_eq!(
			terminal.process(b"\x1b[5n\x1b[?6n"),
			b"\x1b[0n\x1b[?2;3R".to_vec()
		);
		assert_eq!(
			terminal.process(b"\x1b[?6n\x1b[5n"),
			b"\x1b[?2;3R\x1b[0n".to_vec(),
			"asked the other way round, answered the other way round"
		);
	}

	/// The allow-list at the boundary (§82). The nine other values of `CSI ? Ps n` each describe a
	/// piece of the user's machine — a printer, a key store, the KEYBOARD's nationality — and none is
	/// answered. The test sets a cursor position first and answers DECXCPR last, so what it asserts is
	/// that the refusals are silent while the one allowed value SURVIVES them: a scanner that had
	/// simply stopped matching would pass a weaker version of this test.
	#[test]
	fn the_status_reports_that_would_speak_for_the_machine_get_no_answer() {
		let mut terminal = Terminal::new(10, 40);
		terminal.process(b"\x1b[6;7H");
		for parameter in [15, 25, 26, 62, 63, 75, 85] {
			let request = format!("\x1b[?{parameter}n");
			assert!(
				terminal.process(request.as_bytes()).is_empty(),
				"CSI ? {parameter} n must get no answer"
			);
		}
		assert_eq!(terminal.process(b"\x1b[?6n"), b"\x1b[?6;7R".to_vec());
		// And nothing any of them carried reached the page.
		assert_eq!(read(&terminal, 0, 0, 40), "");
	}

	/// The two that came back off that list in §93. Both are answered, both with the negative xterm
	/// sends for a terminal that has no locator — and asked in one write, so what this also shows is
	/// the two answers arriving in the order the questions did.
	#[test]
	fn the_locator_questions_are_answered_with_the_absence() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[?55n"), b"\x1b[?53n".to_vec());
		assert_eq!(terminal.process(b"\x1b[?56n"), b"\x1b[?57;0n".to_vec());
		let both = terminal.process(b"\x1b[?56n\x1b[?55n");
		assert_eq!(both, b"\x1b[?57;0n\x1b[?53n".to_vec());
		assert_eq!(read(&terminal, 0, 0, 40), "", "and none of it printed");
	}

	/// The fourth question on the allow-list (§98), and the only one whose sequence is not xterm's.
	/// Answered "dark" from a constant, and asserted here against the door cmote already had for the
	/// same fact: `OSC 11 ; ?` returns the background the grid paints, and dark-or-light is a function
	/// of that number. Two spellings of one writer — if they ever disagreed, one of them would be a
	/// second source (§71), which is the thing this project refuses rather than the answer itself.
	#[test]
	fn the_colour_scheme_question_is_answered_and_agrees_with_the_background() {
		let mut terminal = Terminal::new(10, 40);
		assert_eq!(terminal.process(b"\x1b[?996n"), b"\x1b[?997;1n".to_vec());
		let background = terminal.process(b"\x1b]11;?\x07");
		let background = String::from_utf8(background).expect("an rgb: reply is text");
		let (red, green, blue) = palette::DEFAULT_BG;
		// xterm's `rgb:` triplet, each channel doubled to sixteen bits.
		let expected =
			format!("rgb:{red:02x}{red:02x}/{green:02x}{green:02x}/{blue:02x}{blue:02x}");
		assert!(
			background.contains(&expected),
			"the background reply is not the scheme's: {background}"
		);
		assert_eq!(read(&terminal, 0, 0, 40), "", "and none of it printed");
	}

	/// A question is not text. Neither the sequence cmote answers nor the nine it refuses may leave a
	/// digit on the grid — the engine drops all ten, and the scanner is a reader, not a consumer.
	#[test]
	fn a_cursor_question_prints_nothing_at_all() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[?6n\x1b[?26nX");
		assert_eq!(read(&terminal, 0, 0, 20), "X");
	}

	/// XTPUSHSGR / XTPOPSGR end to end (§85): the pen a program pushes is the pen it gets back, read
	/// through DECRQSS so the assertion is made against the very template the grid paints with.
	#[test]
	fn a_pushed_pen_comes_back_on_the_pop() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;31m\x1b[#{");
		terminal.process(b"\x1b[0;3;44m");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;3;44m\x1b\\".to_vec(),
			"the program's own change lands while the push is outstanding"
		);
		terminal.process(b"\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	/// xterm's aliases, which exist "to work around language limitations of C#" — and which are the
	/// spelling this row carried under the colour stack's name until §84.
	#[test]
	fn the_lower_case_aliases_are_the_same_stack() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[4m\x1b[#p\x1b[0;7m\x1b[#q");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;4m\x1b\\".to_vec()
		);
	}

	/// A selective push saves what it names and nothing else, so the pop has to put that back while
	/// leaving everything the program has done since. Here only the FOREGROUND is pushed: the italic
	/// set afterwards survives the pop, and the bold that was set before it does not come back.
	#[test]
	fn a_selective_push_restores_only_what_it_named() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;31m\x1b[30#{");
		terminal.process(b"\x1b[0;3;32m\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;3;31m\x1b\\".to_vec()
		);
	}

	/// The pen is saved where the push SAT, which is why this is fed by the interruption advance. Both
	/// sequences and the change between them are in one write: an implementation that pushed after the
	/// chunk would save the italic too and restore it here.
	#[test]
	fn the_pen_is_saved_where_the_push_was_written() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1m\x1b[#{\x1b[3m\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1m\x1b\\".to_vec()
		);
	}

	/// A pop with nothing pushed is not an error and must not disturb the pen — a program that pops
	/// once too often would otherwise have its colours reset out from under it.
	#[test]
	fn a_pop_with_an_empty_stack_leaves_the_pen_alone() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;31m\x1b[#}\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	/// Ten levels, as xterm has it, and the eleventh push is dropped — together with the pop that
	/// matches it, which is where cmote departs from xterm deliberately. Eleven pushes and eleven pops
	/// therefore land back on the pen the FIRST push saw, rather than one level out.
	#[test]
	fn an_overflowing_push_drops_its_own_pop_so_the_levels_stay_paired() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[31m");
		for level in 0..=sgrstack::DEPTH {
			terminal.process(b"\x1b[#{");
			// A different pen at every level, so a mispaired pop restores a colour that is not 31.
			terminal.process(format!("\x1b[3{}m", level % 7 + 1).as_bytes());
		}
		for _ in 0..=sgrstack::DEPTH {
			terminal.process(b"\x1b[#}");
		}
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;31m\x1b\\".to_vec()
		);
	}

	/// The stack is video attributes, so a restore must not take cmote's borrowed DECSCA protection
	/// bit out of the pen with it (§56) — the `CSI 0 m` that opens a restore assigns the whole flag
	/// word. Text written after the pop is still protected, so a selective erase leaves it standing.
	#[test]
	fn a_pop_does_not_clear_the_pens_protection() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1\"q\x1b[#{\x1b[0;31m\x1b[#}Kept");
		terminal.process(b"\x1b[?2J");
		assert_eq!(read(&terminal, 0, 0, 4), "Kept");
	}

	/// Neither sequence puts a byte on the screen — the near-miss test every scanner in `term/` gets,
	/// because a final byte read wrongly shows up as text.
	#[test]
	fn the_stack_sequences_print_nothing_at_all() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[#{\x1b[1;4;31#{\x1b[#}\x1b[#qX");
		assert_eq!(read(&terminal, 0, 0, 20), "X");
	}

	/// A hard reset empties the stack (§86). Without this the pop below would hand the program a pen
	/// from before the reset — a remote's state outliving the sequence whose whole job is to remove it.
	#[test]
	fn a_hard_reset_throws_the_stack_away() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;31m\x1b[#{\x1bc");
		terminal.process(b"\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0m\x1b\\".to_vec(),
			"the pen is the one RIS left, not the one the push saved"
		);
	}

	/// The soft reset does NOT, which is the same split DECSACE has: RIS resets it, DECSTR does not,
	/// because DEC's published list for the soft reset does not name it and §72 honours that list
	/// rather than widening it.
	#[test]
	fn a_soft_reset_leaves_the_stack_standing() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[1;31m\x1b[#{\x1b[!p");
		terminal.process(b"\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	/// An underline substyle survives the round trip, which is the whole reason `pen_restore` exists
	/// beside `pen_sgr`: the DECRQSS reply coarsens every substyle to a plain `4`, and feeding that
	/// back would straighten a program's curly underline. Asserted on the string, since the reply
	/// cannot tell the two apart.
	#[test]
	fn the_restore_string_keeps_the_underline_substyle_and_its_colour() {
		let mut saved = Cell::default();
		saved.flags.insert(Flags::UNDERCURL);
		saved.set_underline_color(Some(Color::Indexed(196)));
		let restored = merged_pen(&Cell::default(), &saved, sgrstack::Mask::ALL);
		assert_eq!(restored, "\x1b[0;4:3;58;5;196m");
	}

	/// And the engine really does read that spelling back, so the string above is not written into a
	/// vacuum: a curly underline set, pushed and popped is still an underline afterwards.
	#[test]
	fn a_curly_underline_survives_the_round_trip() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[4:3m\x1b[#{\x1b[0m\x1b[#}");
		assert_eq!(
			terminal.process(b"\x1bP$qm\x1b\\"),
			b"\x1bP1$r0;4m\x1b\\".to_vec(),
			"DECRQSS reports every substyle as a plain 4 — what matters is that it is underlined"
		);
	}

	/// SCP end to end (§76): the sequence puts the line the cursor is on onto a right-to-left
	/// character path, and the seam the renderer reads says so. Nothing about the grid changes —
	/// asserted here, because "the data stays in the order the host sent" is the property that lets
	/// the search, the selection and a copy go on working.
	#[test]
	fn a_character_path_lands_on_the_line_the_cursor_is_on() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2;1Habc\x1b[2 k");
		assert!(terminal.screen().row_is_rtl(1));
		assert!(!terminal.screen().row_is_rtl(0), "only the cursor's line");
		assert_eq!(read(&terminal, 1, 0, 3), "abc", "the grid keeps data order");
	}

	/// Path 1 puts a line back, and path 0 — the implementation's default — means the same thing
	/// here. A line has to be able to stop being right to left, or a program can only ever wedge one.
	#[test]
	fn a_line_can_be_put_back_left_to_right() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2 k");
		assert!(terminal.screen().row_is_rtl(0));
		terminal.process(b"\x1b[1 k");
		assert!(!terminal.screen().row_is_rtl(0));
		terminal.process(b"\x1b[2 k\x1b[0 k");
		assert!(!terminal.screen().row_is_rtl(0));
	}

	/// The path is keyed by the ABSOLUTE document line (§40), like a prompt mark — so it stays with
	/// its text as the screen scrolls under it, rather than staying on the row it was set at. This is
	/// the test that says the store is anchored in the document rather than in the viewport.
	#[test]
	fn a_path_follows_its_line_as_the_screen_scrolls() {
		// Two rows, four lines fed: two scroll off, so the text set right-to-left ends up in history.
		let mut terminal = Terminal::new(2, 20);
		terminal.process(b"first\x1b[2 k\r\nsecond\r\nthird\r\nfourth");
		// Line 0 is "first", now two lines up in the scrollback.
		assert!(terminal.screen().line_is_rtl(0));
		assert!(!terminal.screen().line_is_rtl(1));
		// Scrolled back to it, the row showing that line reports the path.
		terminal.scroll(ScrollMotion::Lines(2));
		assert!(terminal.screen().row_is_rtl(0));
	}

	/// RIS drops the history, which renumbers every line — so a remembered path would land on text
	/// it was never set for. The store is emptied instead.
	#[test]
	fn a_full_reset_forgets_every_character_path() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2 k");
		assert!(terminal.screen().row_is_rtl(0));
		terminal.process(b"\x1bc");
		assert!(!terminal.screen().row_is_rtl(0));
	}

	/// The alternate page keeps no history, so its line numbering starts again from zero and would
	/// collide with paths set on the main screen. Both directions of the swap clear them — the same
	/// rule the inline pictures follow (§41).
	#[test]
	fn a_swap_to_the_alternate_page_forgets_every_character_path() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2 k");
		assert!(terminal.screen().row_is_rtl(0));
		terminal.process(b"\x1b[?1049h");
		assert!(!terminal.screen().row_is_rtl(0));
		terminal.process(b"\x1b[2 k");
		assert!(
			terminal.screen().row_is_rtl(0),
			"and the page can set its own"
		);
		terminal.process(b"\x1b[?1049l");
		assert!(
			!terminal.screen().row_is_rtl(0),
			"and loses them on the way back"
		);
	}

	/// `Ps2 = 2` — "presentation to data" — asks cmote to write the drawing back into the grid. That
	/// is engine state cmote does not write (§71, §73) and the only copy of what the host sent, so
	/// the whole sequence is a no-op rather than the path being taken and the update mode ignored.
	#[test]
	fn the_update_mode_that_would_rewrite_the_grid_changes_nothing() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2;2 k");
		assert!(!terminal.screen().row_is_rtl(0));
	}

	/// The near miss: `CSI Ps k` with no intermediate is a different sequence, and must not be read
	/// as this one — the rule §56 wrote down, applied to a final byte that is only SCP with a space
	/// in front of it.
	#[test]
	fn a_sequence_without_the_space_is_not_a_character_path() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[2k");
		assert!(!terminal.screen().row_is_rtl(0));
	}

	/// The near miss, and the reason the scanner tests marker, parameter and intermediates
	/// together: `CSI 5 W` without the private marker is CTC, a different sequence cmote does not
	/// implement and must not mistake for this one. Shown by moving the stops and watching a plain
	/// `CSI 5 W` leave them moved.
	#[test]
	fn a_tab_control_without_the_marker_is_not_a_tab_reset() {
		let mut terminal = Terminal::new(4, 40);
		terminal.process(b"\x1b[3g\r\x1b[3C\x1bH\r");
		terminal.process(b"\x1b[5W\t");
		assert_eq!(terminal.screen().cursor_position().1, 3);
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
	///
	/// Mode 69 is set here, because since §102 that is what makes the byte a margin request at all.
	#[test]
	fn a_margin_request_does_not_move_the_saved_cursor() {
		let mut terminal = Terminal::new(4, 20);
		// The status-line shape: save, go somewhere, write, come back.
		terminal.process(b"\x1b[1;1Hhome\x1b[s");
		terminal.process(b"\x1b[3;1Hstatus\x1b[?69h\x1b[5;70s");
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

	/// The other half of the rule §102 restored: WITHOUT mode 69 the same bytes are a save-cursor,
	/// parameters and all, which is what a real xterm makes of them.
	///
	/// §57 could not tell the two apart and cancelled both, because the mode that settles it was one
	/// the engine refused. cmote holds the mode now, so the guess is retired — and the program that
	/// means margins is not harmed by it, since every terminfo margin capability sets the mode first.
	#[test]
	fn a_parametrised_save_cursor_without_the_mode_is_a_save_cursor() {
		let mut terminal = Terminal::new(4, 20);
		terminal.process(b"\x1b[3;1Hhere\x1b[5;70s");
		terminal.process(b"\x1b[1;1H\x1b[uX");
		assert_eq!(
			read(&terminal, 2, 0, 6),
			"hereX",
			"the restore landed where the parametrised `s` sat"
		);
	}

	/// A terminal with DECLRMM set and the margins placed, one-based exactly as a program writes them.
	fn margined(rows: u16, cols: u16, left: u16, right: u16) -> Terminal {
		let mut terminal = Terminal::new(rows, cols);
		terminal.process(format!("\x1b[?69h\x1b[{left};{right}s").as_bytes());
		terminal
	}

	/// Write the same letter across a whole row, so a band operation's effect can be read off against
	/// the columns it was supposed to leave alone.
	///
	/// Called BEFORE the margins go on, in the tests that use it, which is both realistic — a program
	/// draws its page and then walls off the column it wants to scroll — and necessary: text starting
	/// left of the band crosses INTO it, and the band is exactly where the line then breaks.
	fn fill_row(terminal: &mut Terminal, row: u16, letter: char, cols: u16) {
		let text = std::iter::repeat_n(letter, cols as usize).collect::<String>();
		terminal.process(format!("\x1b[{};1H{text}", row + 1).as_bytes());
	}

	/// Put margins on a terminal that already has something on it, one-based as on the wire.
	fn set_margins(terminal: &mut Terminal, left: u16, right: u16) {
		terminal.process(format!("\x1b[?69h\x1b[{left};{right}s").as_bytes());
	}

	/// Fill every row of a small page with its own letter, then wall off a band of columns.
	fn banded_page(rows: u16, cols: u16, left: u16, right: u16) -> Terminal {
		let mut terminal = Terminal::new(rows, cols);
		for row in 0..rows {
			let letter = char::from(b'A' + row as u8);
			fill_row(&mut terminal, row, letter, cols);
		}
		set_margins(&mut terminal, left, right);
		terminal
	}

	/// The sequence's whole point: a line breaks at the RIGHT MARGIN and goes on at the LEFT one,
	/// instead of running to the screen edge and coming back to column 1 (§102).
	#[test]
	fn a_line_breaks_at_the_right_margin_and_goes_on_at_the_left_one() {
		// Columns 5 to 10 on the wire — a band six wide, from column 4 to column 9 counting from zero.
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[1;5Habcdefgh");
		assert_eq!(read(&terminal, 0, 4, 6), "abcdef", "the band filled");
		assert_eq!(
			read(&terminal, 1, 4, 6),
			"gh",
			"and went on at the left margin"
		);
		assert_eq!(
			read(&terminal, 0, 10, 10),
			"",
			"nothing past the right margin"
		);
	}

	/// A terminal does not wrap when the last usable column is filled — it leaves the cursor ON that
	/// column with the wrap OWED, so a program that fills the line and then moves never wraps at all.
	///
	/// The engine has its own flag for this and fires it at the screen edge, which is the wrong column
	/// and, for a band that stops short of the edge, never happens. So the flag becomes cmote's.
	#[test]
	fn filling_the_band_leaves_the_cursor_on_the_margin_with_the_wrap_owed() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[1;5Habcdef");
		assert_eq!(
			terminal.process(b"\x1b[6n"),
			b"\x1b[1;10R".to_vec(),
			"still on the right margin, not on the next row"
		);
		// And a carriage return cancels the owed wrap rather than performing it.
		terminal.process(b"\rX");
		assert_eq!(read(&terminal, 0, 4, 6), "Xbcdef");
		assert_eq!(
			read(&terminal, 1, 0, 20),
			"",
			"the second row was never reached"
		);
	}

	/// CR goes to the left margin — and to column 1 when the cursor is left of the band, so a program
	/// writing outside its own band is not dragged into it.
	#[test]
	fn a_carriage_return_goes_to_the_left_margin_only_from_inside_the_band() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[1;8H\rX");
		assert_eq!(read(&terminal, 0, 4, 1), "X", "from inside the band");
		terminal.process(b"\x1b[2;2H\rY");
		assert_eq!(read(&terminal, 1, 0, 1), "Y", "from left of it");
	}

	/// CUF and CUB stop at the margins when the cursor started between them.
	#[test]
	fn cursor_motion_stops_at_the_margins() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[1;6H\x1b[40CX");
		assert_eq!(
			read(&terminal, 0, 9, 1),
			"X",
			"forward, held at the right margin"
		);
		terminal.process(b"\x1b[2;8H\x1b[40DY");
		assert_eq!(
			read(&terminal, 1, 4, 1),
			"Y",
			"back, held at the left margin"
		);
	}

	/// Under origin mode the columns a program names are counted from the LEFT MARGIN — the same
	/// relationship origin mode already had with the scrolling region's rows, which the engine
	/// implements and, having no margins, never had to implement for columns.
	#[test]
	fn origin_mode_counts_columns_from_the_left_margin() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[?6h\x1b[1;1HX");
		assert_eq!(read(&terminal, 0, 4, 1), "X");
		// And cannot be aimed past the right one.
		terminal.process(b"\x1b[?6h\x1b[2;99HY");
		assert_eq!(read(&terminal, 1, 9, 1), "Y");
	}

	/// A scroll moves only the band. Everything either side of it is another column of the page and
	/// is not scrolling — which is the whole reason a program sets margins.
	#[test]
	fn a_line_feed_at_the_bottom_scrolls_only_the_band() {
		let mut terminal = banded_page(3, 12, 5, 8);
		// The cursor on the last row and inside the band, so the line feed scrolls rather than moves.
		terminal.process(b"\x1b[3;5H\n");
		assert_eq!(read(&terminal, 0, 0, 12), "AAAABBBBAAAA");
		assert_eq!(read(&terminal, 1, 0, 12), "BBBBCCCCBBBB");
		assert_eq!(
			read(&terminal, 2, 0, 12),
			"CCCCCCCC",
			"the band blanked, the rest standing"
		);
	}

	/// A row pushed out of a narrowed band is DISCARDED, never pushed to the scrollback: the history
	/// holds whole lines and this row is a slice of one. Half-lines interleaved with whole ones would
	/// make every search, selection and copy downstream of it wrong (§102).
	#[test]
	fn a_band_scroll_puts_nothing_in_the_scrollback() {
		let mut terminal = banded_page(3, 12, 5, 8);
		terminal.process(b"\x1b[3;5H\n\n\n\n");
		assert_eq!(terminal.screen().history_size(), 0);
	}

	/// SU and SD take the whole scrolling region, and only the band's columns of it.
	#[test]
	fn a_scroll_up_moves_only_the_bands_columns() {
		let mut terminal = banded_page(3, 12, 5, 8);
		terminal.process(b"\x1b[S");
		assert_eq!(read(&terminal, 0, 0, 12), "AAAABBBBAAAA");
		assert_eq!(read(&terminal, 2, 0, 12), "CCCCCCCC");
	}

	/// IL and DL open and close lines inside the band alone.
	#[test]
	fn a_line_insert_opens_a_gap_in_the_band_and_nowhere_else() {
		let mut terminal = banded_page(3, 12, 5, 8);
		terminal.process(b"\x1b[2;5H\x1b[L");
		assert_eq!(
			read(&terminal, 0, 0, 12),
			"AAAAAAAAAAAA",
			"above the cursor, untouched"
		);
		assert_eq!(read(&terminal, 1, 0, 12), "BBBBBBBB", "the band opened");
		assert_eq!(
			read(&terminal, 2, 0, 12),
			"CCCCBBBBCCCC",
			"and what was there moved down"
		);
	}

	/// Refused outright when the cursor is outside the margins, which is xterm's test: there is no
	/// band to open lines in from out there, and guessing one would move text the program walled off.
	#[test]
	fn a_line_insert_from_outside_the_band_does_nothing() {
		let mut terminal = banded_page(3, 12, 5, 8);
		terminal.process(b"\x1b[2;1H\x1b[L");
		assert_eq!(read(&terminal, 1, 0, 12), "BBBBBBBBBBBB");
		assert_eq!(read(&terminal, 2, 0, 12), "CCCCCCCCCCCC");
	}

	/// ICH and DCH push and pull within the band, so the neighbouring column's text does not slide.
	#[test]
	fn a_character_insert_and_delete_stop_at_the_right_margin() {
		let mut terminal = Terminal::new(2, 12);
		terminal.process(b"\x1b[1;1HABCDEFGHIJKL");
		terminal.process(b"\x1b[2;1HABCDEFGHIJKL");
		set_margins(&mut terminal, 5, 8);
		terminal.process(b"\x1b[1;5H\x1b[1@");
		assert_eq!(
			read(&terminal, 0, 0, 12),
			"ABCDEFGIJKL",
			"EFGH became _EFG, I stood still"
		);
		terminal.process(b"\x1b[2;5H\x1b[1P");
		assert_eq!(
			read(&terminal, 1, 0, 12),
			"ABCDFGHIJKL",
			"EFGH became FGH_, I stood still"
		);
	}

	/// The wrap column is the right margin WHEREVER the cursor is, not only inside the band (§102).
	///
	/// xterm's rule — its `ScrnRightMargin` reads the mode and never the cursor — so text starting
	/// left of the band flows rightward, meets the right margin, and continues at the LEFT margin. It
	/// looks wrong written down and it is what the terminal these programs were built against does;
	/// the tempting alternative, letting text outside the band keep the whole page, is nobody's
	/// behaviour and would be cmote inventing a dialect.
	///
	/// It is also why the tests above draw their page BEFORE the margins go on.
	#[test]
	fn text_written_outside_the_band_still_wraps_at_the_right_margin() {
		let mut terminal = margined(3, 12, 5, 8);
		terminal.process(b"\x1b[1;1HABCDEFGHIJKL");
		assert_eq!(
			read(&terminal, 0, 0, 12),
			"ABCDEFGH",
			"out to the right margin"
		);
		assert_eq!(read(&terminal, 1, 0, 12), "IJKL", "and on at the left one");
	}

	/// A two-cell glyph with only one column left before the right margin takes the wrap whole rather
	/// than putting its continuation in the next column of the page, which belongs to somebody else.
	#[test]
	fn a_wide_glyph_that_will_not_fit_takes_the_wrap_whole() {
		let mut terminal = margined(3, 12, 5, 8);
		terminal.process("\x1b[1;5Habc世".as_bytes());
		assert_eq!(read(&terminal, 0, 4, 4), "abc");
		assert_eq!(read(&terminal, 1, 4, 2), "世", "whole, on the next row");
	}

	/// DECRQM has to answer for a mode cmote implements. The engine's own answer is `0`, "not
	/// recognised" — which was true until §102 and is now a lie a program acts on.
	#[test]
	fn the_margin_mode_answers_a_request_about_itself() {
		let mut terminal = Terminal::new(4, 20);
		assert_eq!(
			terminal.process(b"\x1b[?69$p"),
			b"\x1b[?69;2$y".to_vec(),
			"reset"
		);
		terminal.process(b"\x1b[?69h");
		assert_eq!(
			terminal.process(b"\x1b[?69$p"),
			b"\x1b[?69;1$y".to_vec(),
			"set"
		);
	}

	/// A soft reset takes the margins with it (§102). DEC's published DECSTR list does not name them,
	/// and `CSI r` — the scrolling region — is on it: the margins are the same object's other axis, so
	/// a reset that freed the rows and left the columns walled off would hand the next program half a
	/// page and no sequence to discover it with.
	#[test]
	fn a_soft_reset_takes_the_margins_with_it() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.process(b"\x1b[!p");
		assert_eq!(terminal.process(b"\x1b[?69$p"), b"\x1b[?69;2$y".to_vec());
		terminal.process(b"\x1b[1;1H0123456789");
		assert_eq!(
			read(&terminal, 0, 0, 10),
			"0123456789",
			"the whole page again"
		);
	}

	/// And a resize does too: a band of columns 5 to 10 means nothing on a window that is now eight
	/// columns wide, and reflow makes it worse than arbitrary since the text those columns held has
	/// moved.
	#[test]
	fn a_resize_takes_the_margins_with_it() {
		let mut terminal = margined(4, 20, 5, 10);
		terminal.resize(4, 24);
		assert_eq!(terminal.process(b"\x1b[?69$p"), b"\x1b[?69;2$y".to_vec());
	}

	/// Turning the mode off gives the page back, so a program can put margins away without leaving
	/// the next one to guess.
	#[test]
	fn turning_the_mode_off_gives_the_page_back() {
		let mut terminal = margined(2, 10, 3, 6);
		terminal.process(b"\x1b[?69l\x1b[1;1H0123456789");
		assert_eq!(read(&terminal, 0, 0, 10), "0123456789");
	}

	/// Margins at the page edges are not margins: the band is the whole width, so every operation
	/// goes to the engine exactly as it did before §102 — including the one thing cmote's band scroll
	/// deliberately does not do, which is fill the scrollback.
	#[test]
	fn a_band_spanning_the_page_leaves_the_scrollback_working() {
		let mut terminal = margined(3, 12, 1, 12);
		terminal.process(b"\x1b[3;1Hlast\n\n");
		assert!(
			terminal.screen().history_size() > 0,
			"a full-width scroll still reaches the history"
		);
	}

	/// DECIC and DECDC open and close a whole COLUMN across every row — the vertical twins of IL and
	/// DL, and legal with no margins at all, where the band is the whole page (§102).
	#[test]
	fn a_column_insert_and_delete_take_every_row_of_the_page() {
		// Every row of the region, not the cursor's row alone — which is the whole difference from
		// ICH and DCH, and why the two halves below need a page each rather than one page twice.
		let mut inserted = Terminal::new(3, 8);
		for row in 1..=3 {
			inserted.process(format!("\x1b[{row};1HABCDEFGH").as_bytes());
		}
		// One column in at column 3, so `H` falls off the right edge of every row.
		inserted.process(b"\x1b[1;3H\x1b['}");
		for row in 0..3 {
			assert_eq!(read(&inserted, row, 0, 8), "ABCDEFG", "row {row}");
		}

		let mut deleted = Terminal::new(3, 8);
		for row in 1..=3 {
			deleted.process(format!("\x1b[{row};1HABCDEFGH").as_bytes());
		}
		deleted.process(b"\x1b[1;3H\x1b['~");
		for row in 0..3 {
			assert_eq!(read(&deleted, row, 0, 8), "ABDEFGH", "row {row}");
		}
	}

	/// With margins on they take the band and stop at the right one, so the neighbouring column's
	/// text does not slide.
	#[test]
	fn a_column_insert_stops_at_the_right_margin() {
		// Columns 4 to 7 on the wire, so the band is columns 3 to 6 counting from zero.
		let mut inserted = Terminal::new(1, 8);
		inserted.process(b"\x1b[1;1HABCDEFGH");
		set_margins(&mut inserted, 4, 7);
		inserted.process(b"\x1b[1;5H\x1b['}");
		assert_eq!(
			read(&inserted, 0, 0, 8),
			"ABCDEFH",
			"G fell off the margin, H stood still"
		);

		let mut deleted = Terminal::new(1, 8);
		deleted.process(b"\x1b[1;1HABCDEFGH");
		set_margins(&mut deleted, 4, 7);
		deleted.process(b"\x1b[1;5H\x1b['~");
		assert_eq!(read(&deleted, 0, 0, 8), "ABCDFGH", "and back the other way");
	}

	/// Refused from outside the band, which is xterm's test and the one IL and DL apply: there is no
	/// column to open from out there, and guessing one would move text the program walled off.
	#[test]
	fn a_column_insert_from_outside_the_band_does_nothing() {
		let mut terminal = Terminal::new(2, 8);
		terminal.process(b"\x1b[1;1HABCDEFGH");
		set_margins(&mut terminal, 4, 7);
		terminal.process(b"\x1b[1;1H\x1b['}");
		assert_eq!(read(&terminal, 0, 0, 8), "ABCDEFGH");
	}

	/// And bounded by the scrolling region's rows, since the region is the other half of the same
	/// wall — a row the program walled off vertically is not a row a column operation may move.
	#[test]
	fn a_column_insert_leaves_rows_outside_the_scrolling_region_alone() {
		let mut terminal = Terminal::new(4, 8);
		for row in 1..=4 {
			terminal.process(format!("\x1b[{row};1HABCDEFGH").as_bytes());
		}
		// Rows 2 and 3 on the wire, so rows 1 and 2 counting from zero.
		terminal.process(b"\x1b[2;3r");
		terminal.process(b"\x1b[2;3H\x1b['}");
		assert_eq!(read(&terminal, 0, 0, 8), "ABCDEFGH", "above the region");
		assert_eq!(read(&terminal, 1, 0, 8), "ABCDEFG", "inside it");
		assert_eq!(read(&terminal, 2, 0, 8), "ABCDEFG", "inside it");
		assert_eq!(read(&terminal, 3, 0, 8), "ABCDEFGH", "below the region");
	}

	/// The cancel has to END the sequence, not merely withhold its final byte: the parameters have
	/// already reached the engine's parser, so without one the next final byte in the stream would be
	/// taken as this sequence's. Here that is the `h` of `hello`, which would dispatch as set-mode
	/// with parameters 5 and 70 and swallow four characters of output.
	#[test]
	fn the_text_after_a_margin_request_is_printed_and_not_eaten() {
		let mut terminal = Terminal::new(4, 20);
		// Margins 5 to 20 on a twenty-column page. DECSLRM homes the cursor, and home without origin
		// mode is column 1 — outside the band, which is xterm's behaviour and not an accident here.
		terminal.process(b"\x1b[?69h\x1b[5;20shello");
		assert_eq!(read(&terminal, 0, 0, 5), "hello");
	}

	/// And the CAN itself must leave nothing behind. A substitute glyph, or the `s` printed as a
	/// letter, would both show up here.
	#[test]
	fn a_cancelled_margin_request_prints_nothing_at_all() {
		let mut terminal = Terminal::new(4, 20);
		// Each request homes the cursor, so both letters land at column 1 and the second overwrites
		// the first. What must not appear is a substitute glyph or a stray `s` anywhere on the row.
		terminal.process(b"\x1b[?69h\x1b[5;20sa\x1b[6;20sb");
		assert_eq!(read(&terminal, 0, 0, 20), "b");
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

	/// A margin request in the middle of a chunk that also carries an interruption of another kind — the two
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

	/// Read a whole DOCUMENT line — history included — which is the only way to see whether an
	/// unscroll left a line in the scrollback as well as on the page (§101).
	fn read_line(terminal: &Terminal, line: u64, len: u16) -> String {
		let screen = terminal.screen();
		(0..len)
			.filter_map(|col| screen.line_cell(line, col))
			.map(|cell| cell.contents().to_owned())
			.collect()
	}

	/// UNSCROLL brings the scrollback back onto the page (§101) — the whole point of the sequence.
	#[test]
	fn an_unscroll_brings_the_scrollback_back_onto_the_page() {
		let mut terminal = Terminal::new(3, 8);
		// Six lines through a three-row page: three scroll off, three are shown.
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
		assert_eq!(read(&terminal, 0, 0, 8), "four");
		assert_eq!(terminal.screen().history_size(), 3);

		terminal.process(b"\x1b[2+T");
		// The two lines that had scrolled off are back, the page has slid down, and the two rows
		// that fell off the bottom are gone.
		assert_eq!(read(&terminal, 0, 0, 8), "two");
		assert_eq!(read(&terminal, 1, 0, 8), "three");
		assert_eq!(read(&terminal, 2, 0, 8), "four");
	}

	/// The lines are MOVED, not copied. This is the assertion that separates a faithful unscroll from
	/// the cheap one: after it, the restored text must appear on the page and NOT still in the
	/// scrollback, or every completion would leave the user scrolling back over the same lines twice.
	#[test]
	fn an_unscroll_takes_the_lines_out_of_the_scrollback() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
		terminal.process(b"\x1b[2+T");

		assert_eq!(
			terminal.screen().history_size(),
			1,
			"three lines of history less the two taken back"
		);
		// The document reads straight through with no line said twice: one, then two on the page.
		assert_eq!(read_line(&terminal, 0, 8), "one");
		assert_eq!(read_line(&terminal, 1, 8), "two");
		assert_eq!(read_line(&terminal, 2, 8), "three");
		assert_eq!(read_line(&terminal, 3, 8), "four");
		// And nothing past the end of the shortened document.
		assert_eq!(read_line(&terminal, 4, 8), "");
	}

	/// The compaction loop, over a history long enough for an off-by-one to have somewhere to hide.
	/// Twenty lines through a four-row page, three taken back, and then the WHOLE document is read
	/// end to end: every line in order, once each, nothing repeated and nothing missing.
	#[test]
	fn an_unscroll_leaves_a_long_history_in_order() {
		let mut terminal = Terminal::new(4, 8);
		for line in 0..20 {
			terminal.process(format!("L{line}\r\n").as_bytes());
		}
		// Twenty lines and their trailing newline: nineteen scrolled off, the page holds the rest.
		let history = terminal.screen().history_size();
		terminal.process(b"\x1b[3+T");
		assert_eq!(terminal.screen().history_size(), history - 3);

		// `L0` through `L19` still read in order from the top of the document, wherever the seam
		// between the scrollback and the page now falls.
		for line in 0..17u64 {
			assert_eq!(
				read_line(&terminal, line, 8),
				format!("L{line}"),
				"document line {line}"
			);
		}
	}

	/// The retention limit is put back after the shrink. `Grid::update_history` sets the cap as well
	/// as trimming, so an unscroll that forgot to restore it would silently pin the scrollback at
	/// whatever length it had at that moment — a session that stops remembering, with nothing to show
	/// for it but a number.
	#[test]
	fn an_unscroll_leaves_the_scrollback_able_to_grow_again() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"a\r\nb\r\nc\r\nd\r\ne");
		terminal.process(b"\x1b[2+T");
		assert_eq!(terminal.screen().history_size(), 0, "both taken back");

		for line in 0..10 {
			terminal.process(format!("x{line}\r\n").as_bytes());
		}
		assert!(
			terminal.screen().history_size() > 2,
			"the scrollback was pinned at the length the unscroll left it"
		);
	}

	/// A request larger than the scrollback is filled as far as it goes and blanked for the rest —
	/// kitty's own rule for a terminal with nothing to give back, and exactly what happens on the
	/// alternate screen, which keeps no history at all.
	#[test]
	fn an_unscroll_with_nothing_to_give_back_inserts_blanks() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"one\r\ntwo\r\nthree");
		assert_eq!(terminal.screen().history_size(), 0);

		terminal.process(b"\x1b[2+T");
		assert_eq!(read(&terminal, 0, 0, 8), "", "blank");
		assert_eq!(read(&terminal, 1, 0, 8), "", "blank");
		assert_eq!(read(&terminal, 2, 0, 8), "one", "the page slid down");
	}

	/// The alternate screen reaches the same answer through the same code and no special case: that
	/// page keeps no scrollback, so an unscroll there is a plain SD with blank fill.
	#[test]
	fn an_unscroll_on_the_alternate_screen_is_a_plain_scroll_down() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix");
		terminal.process(b"\x1b[?1049h");
		terminal.process(b"\x1b[1;1Halt");
		terminal.process(b"\x1b[1+T");
		assert_eq!(
			read(&terminal, 0, 0, 8),
			"",
			"blank, not the primary history"
		);
		assert_eq!(read(&terminal, 1, 0, 8), "alt", "the page slid down");
	}

	/// The part that makes this more than a grid operation (§101): a prompt mark is an absolute line,
	/// and when the scrollback can fill the request no absolute line moves — so the mark must still be
	/// on the text it was recorded against, which is now three rows further down the page.
	#[test]
	fn an_unscroll_keeps_the_prompt_marks_on_their_own_lines() {
		let mut terminal = Terminal::new(4, 8);
		// A prompt on the first line, then enough output to scroll it into the scrollback.
		terminal.process(b"\x1b]133;A\x07$ ls\r\na\r\nb\r\nc\r\nd\r\ne");
		assert_eq!(terminal.screen().history_size(), 2);
		// The prompt is above the page now, so no tick is drawn.
		assert!(terminal.prompt_rows().is_empty());

		terminal.process(b"\x1b[2+T");
		// It came back onto the page, and the tick is beside it — on the row the text is actually on.
		assert_eq!(terminal.prompt_rows(), vec![0]);
		assert_eq!(
			read(&terminal, 0, 0, 8),
			"$ls",
			"a blank cell reads as nothing"
		);
	}

	/// And when blanks had to be inserted, the marks move with the text rather than staying on a
	/// number that now names a blank line.
	#[test]
	fn an_unscroll_that_inserts_blanks_moves_the_marks_with_the_text() {
		let mut terminal = Terminal::new(3, 8);
		terminal.process(b"\x1b]133;A\x07$ x\r\na\r\nb");
		assert_eq!(terminal.prompt_rows(), vec![0], "on the page, top row");

		// Two lines asked for with an empty scrollback: two blanks go in above everything.
		terminal.process(b"\x1b[2+T");
		assert_eq!(
			read(&terminal, 2, 0, 8),
			"$x",
			"a blank cell reads as nothing"
		);
		assert_eq!(
			terminal.prompt_rows(),
			vec![2],
			"the tick followed its line down"
		);
	}

	/// SL — the page shifts left, the right edge goes blank, and nothing else moves (§100).
	#[test]
	fn a_shift_left_moves_the_page_and_blanks_the_right_edge() {
		let mut terminal = Terminal::new(2, 8);
		terminal.process(b"abcdefgh\r\n12345678");
		terminal.process(b"\x1b[3 @");
		assert_eq!(read(&terminal, 0, 0, 8), "defgh");
		assert_eq!(read(&terminal, 1, 0, 8), "45678");
		// The blanks are cells, not merely absent text: reading them back gives nothing at all.
		assert!(!terminal.screen().cell(0, 5).unwrap().has_contents());
		assert!(!terminal.screen().cell(0, 7).unwrap().has_contents());
	}

	/// SR — the same the other way, and the direction is the one thing here that cannot be got wrong
	/// quietly: a shift that goes the wrong way is a screen no program can recover.
	#[test]
	fn a_shift_right_moves_the_page_and_blanks_the_left_edge() {
		let mut terminal = Terminal::new(2, 8);
		terminal.process(b"abcdefgh\r\n12345678");
		terminal.process(b"\x1b[3 A");
		// `read` yields a blank cell as nothing at all, so the three blanks show up as an absence in
		// front and the content is asserted where it actually landed, from column 3.
		assert_eq!(read(&terminal, 0, 0, 8), "abcde");
		assert_eq!(read(&terminal, 0, 3, 5), "abcde");
		assert_eq!(read(&terminal, 1, 3, 5), "12345");
		for column in 0..3 {
			assert!(
				!terminal.screen().cell(0, column).unwrap().has_contents(),
				"column {column} should be blank"
			);
		}
	}

	/// An omitted count is one column, and a count past the page's width blanks it rather than
	/// running off the end of the grid.
	#[test]
	fn a_shift_defaults_to_one_column_and_clamps_to_the_page() {
		let mut terminal = Terminal::new(1, 4);
		terminal.process(b"abcd\x1b[ @");
		assert_eq!(read(&terminal, 0, 0, 4), "bcd");

		let mut wide = Terminal::new(1, 4);
		wide.process(b"abcd\x1b[99 @");
		assert_eq!(read(&wide, 0, 0, 4), "", "shifted away whole");
	}

	/// Whole cells move, so what slid across keeps its colours and attributes — DECCRA's rule (§58),
	/// and the reason a shift is not the same as reprinting the text one column over. The blanks that
	/// arrive carry the PEN's background, which is what the erases write.
	#[test]
	fn a_shift_carries_the_attributes_and_lays_the_pen_down_behind_it() {
		let mut terminal = Terminal::new(1, 6);
		// Bold red-on-blue `ab`, then the pen left on a green background — so what arrives at the
		// blanked edge is green, and what moves keeps the red, the blue and the bold.
		terminal.process(b"\x1b[1;31;44mab\x1b[0;42m");
		terminal.process(b"\x1b[2 A");
		let screen = terminal.screen();
		let moved = screen.cell(0, 2).unwrap();
		assert_eq!(moved.contents(), "a", "shifted two columns right");
		assert_eq!(moved.fgcolor(), screen::Color::Indexed(1), "still red");
		assert_eq!(moved.bgcolor(), screen::Color::Indexed(4), "still on blue");
		assert!(moved.bold(), "and still bold");
		// The cells that arrived carry the pen's background, which is what an erase writes — a shift
		// over a coloured screen leaves a strip in that colour rather than in the default one.
		let arrived = screen.cell(0, 0).unwrap();
		assert!(!arrived.has_contents());
		assert_eq!(
			arrived.bgcolor(),
			screen::Color::Indexed(2),
			"the pen's green"
		);
	}

	/// A wide glyph is two cells, and a shift can push exactly one of them off the page. The half left
	/// behind is not a character anybody asked for, so it is blanked rather than drawn as a lead with
	/// nothing after it or a continuation with nothing before it.
	#[test]
	fn a_wide_glyph_cut_in_half_by_a_shift_leaves_no_half_behind() {
		// `世` occupies columns 0 and 1; shifting left by one would leave its continuation at column 0.
		let mut terminal = Terminal::new(1, 6);
		terminal.process("世ab".as_bytes());
		terminal.process(b"\x1b[1 @");
		let screen = terminal.screen();
		assert!(!screen.cell(0, 0).unwrap().is_wide_continuation());
		assert!(!screen.cell(0, 0).unwrap().has_contents());
		assert_eq!(screen.cell(0, 1).unwrap().contents(), "a");

		// And the mirror: shifting right leaves the LEAD at the last column with nothing to follow it.
		let mut right = Terminal::new(1, 6);
		right.process("ab世".as_bytes());
		right.process(b"\x1b[1 A");
		let screen = right.screen();
		assert!(!screen.cell(0, 5).unwrap().is_wide());
		assert!(!screen.cell(0, 5).unwrap().has_contents());
	}

	/// SL and SR move the data under the cursor and leave the cursor where it stands — so a program
	/// that shifts mid-line goes on writing at the same column, over whatever slid into it.
	#[test]
	fn a_shift_leaves_the_cursor_where_it_was() {
		let mut terminal = Terminal::new(2, 8);
		terminal.process(b"\x1b[2;5H");
		assert_eq!(terminal.process(b"\x1b[3 @\x1b[6n"), b"\x1b[2;5R".to_vec());
	}

	/// Fill every row of the page with the same eight letters, so a shift's effect on each row can be
	/// read off independently of the others.
	fn fill_rows(terminal: &mut Terminal, rows: u16) {
		for row in 1..=rows {
			terminal.process(format!("\x1b[{row};1Habcdefgh").as_bytes());
		}
	}

	/// A shift is a scrolling operation, so it stops at the scrolling region's edges — the rows
	/// DECSTBM walled off do not move (§102).
	///
	/// This is what §100 wanted and could not have: the region is private inside the engine, so the
	/// shift was refused on a proxy instead. With the region mirrored the bound is the real one.
	#[test]
	fn a_shift_leaves_the_rows_outside_the_scrolling_region_alone() {
		let mut terminal = Terminal::new(4, 8);
		fill_rows(&mut terminal, 4);
		// Rows 2 and 3 on the wire, so rows 1 and 2 counting from zero.
		terminal.process(b"\x1b[2;3r\x1b[3 @");
		assert_eq!(read(&terminal, 0, 0, 8), "abcdefgh", "above the band");
		assert_eq!(read(&terminal, 1, 0, 8), "defgh", "in the band");
		assert_eq!(read(&terminal, 2, 0, 8), "defgh", "in the band");
		assert_eq!(read(&terminal, 3, 0, 8), "abcdefgh", "below the band");
	}

	/// Origin mode no longer costs the shift (§102, retiring §100's refusal).
	///
	/// DECOM was standing in for a region cmote could not read, and refusing on it was both too much
	/// — a program with origin mode and no region got nothing — and too little, since a region set
	/// WITHOUT origin mode was shifted straight through. Now the region itself is the bound and
	/// origin mode is not consulted at all.
	#[test]
	fn a_shift_runs_under_origin_mode_and_still_stops_at_the_region() {
		let mut terminal = Terminal::new(4, 8);
		fill_rows(&mut terminal, 4);
		terminal.process(b"\x1b[2;3r\x1b[?6h\x1b[3 @");
		assert_eq!(read(&terminal, 0, 0, 8), "abcdefgh", "above the band");
		assert_eq!(read(&terminal, 1, 0, 8), "defgh", "shifted, not refused");
		assert_eq!(read(&terminal, 3, 0, 8), "abcdefgh", "below the band");
	}

	/// A shift with no region set still takes the whole page, which is the overwhelmingly common case
	/// and the one §100's tests already pinned. Kept as its own test because the bound moved.
	#[test]
	fn a_shift_with_no_region_set_still_takes_every_row() {
		let mut terminal = Terminal::new(4, 8);
		fill_rows(&mut terminal, 4);
		terminal.process(b"\x1b[3 @");
		for row in 0..4 {
			assert_eq!(read(&terminal, row, 0, 8), "defgh");
		}
	}

	/// RIS throws the scrolling region away INSIDE the engine — no sequence on the wire says so, and
	/// nothing but the `Handler` boundary can see it happen. The mirror has to follow, or a shift
	/// after a reset would go on obeying a band no longer there (§102).
	#[test]
	fn a_full_reset_puts_the_mirrored_region_back() {
		let mut terminal = Terminal::new(4, 8);
		terminal.process(b"\x1b[2;3r");
		// RIS clears the page as well, so the text goes down after it rather than before.
		terminal.process(b"\x1bc");
		fill_rows(&mut terminal, 4);
		terminal.process(b"\x1b[3 @");
		for row in 0..4 {
			assert_eq!(read(&terminal, row, 0, 8), "defgh", "the band is gone");
		}
	}

	/// A resize does the same thing, and from a place even further out of reach: `Term::resize`
	/// assigns the full page over the region with no `Handler` call at all, so this one is corrected
	/// by `Terminal::resize` itself (§102).
	#[test]
	fn a_resize_puts_the_mirrored_region_back() {
		let mut terminal = Terminal::new(4, 8);
		terminal.process(b"\x1b[2;3r");
		terminal.resize(4, 8);
		// Same height, different width, so the engine reflows and resets the region without the row
		// count moving under the test.
		terminal.resize(4, 9);
		fill_rows(&mut terminal, 4);
		terminal.process(b"\x1b[3 @");
		for row in 0..4 {
			assert_eq!(read(&terminal, row, 0, 8), "defgh", "the band is gone");
		}
	}

	/// XTCHECKSUM (`CSI Ps # y`) would move that calculation, and cmote answers **one** calculation —
	/// the DEC-compatible default xterm tuned against a real VT520 (§60, §99). This is the decision
	/// stated as behaviour rather than as a note: the request draws no reply of its own, and the
	/// checksum of an unchanged rectangle is the same number before and after it.
	///
	/// Why refuse rather than honour it. Four of the five bits are mechanical, and the fifth — "omit
	/// the checksum for cells never initialised" — is one cmote **cannot** perform: the engine's grid
	/// starts full of blanks that read identically to written ones, which §60 already discloses as the
	/// one place cmote's number can differ from xterm's. Honouring four of five would hand a program
	/// that set the fifth a number computed under rules it did not choose, which is exactly the harm
	/// §94 named when it opened this row — so the whole request is left alone instead.
	#[test]
	fn a_checksum_extension_request_leaves_the_calculation_alone() {
		let mut terminal = Terminal::new(2, 4);
		terminal.process(b"AB");
		let before = terminal.process(b"\x1b[1;1;1;1;1;2*y");
		assert_eq!(before, b"\x1bP1!~FF7D\x1b\\".to_vec());
		// Every bit xterm defines, asked for at once, then the same rectangle again.
		assert!(
			terminal.process(b"\x1b[31#y").is_empty(),
			"the extension request answers nothing itself"
		);
		assert_eq!(terminal.process(b"\x1b[1;1;1;1;1;2*y"), before);
		assert_eq!(read(&terminal, 0, 0, 4), "AB", "and printed nothing");
	}

	/// The answer is the page as it stood WHERE THE QUESTION SAT, not as the rest of the chunk left
	/// it. That is what the interruption-fed offset buys, and the only rectangular operation that needs it
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

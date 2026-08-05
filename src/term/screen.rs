// term/screen.rs — cmote's engine-agnostic view of the terminal screen (PLAN §9, §16, §23).
//
// The rest of the app must not know WHICH VT engine backs the terminal. `ui::grid`,
// `ui::selection` and `app` read the screen only through the types here — a `Screen`
// borrowed for one frame, its `Cell`s, a `Color`, and the mouse / paste / cursor modes.
// That is the seam PLAN §9 always intended, and §23 swaps the engine behind it: this file
// and `term::mod` translate `alacritty_terminal`'s grid into these cmote types, and no
// caller moved when the engine changed from `vt100`.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::{Cell as EngineCell, Flags};
use alacritty_terminal::vte::ansi::{
	Color as EngineColor, CursorShape as EngineCursorShape, NamedColor,
};

use super::Engine;
use super::kitty::KittyFlags;

/// A cell colour, independent of the engine. `Default` is the terminal's default foreground
/// or background (the renderer decides which); `Indexed` is a slot in the xterm-256 palette
/// (0-255); `Rgb` is a truecolor value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
	Default,
	Indexed(u8),
	Rgb(u8, u8, u8),
}

/// Which xterm mouse protocol the remote program turned on (§9), or `None` when it has not
/// asked for the mouse. The engine does not implement X10 (`?9`, press-only), so that mode
/// never appears — the three reporting modes below are mutually exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
	None,
	PressRelease,
	ButtonMotion,
	AnyMotion,
}

/// How a mouse report is encoded on the wire (§9): the classic single-byte form, its UTF-8
/// widening, or SGR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEncoding {
	Default,
	Utf8,
	Sgr,
}

/// The underline a cell asks for (§9, §23). A remote turns these on through the extended SGR
/// underline sub-parameters (`CSI 4 : n m`), and the engine tracks all five as distinct flags;
/// the grid draws each as its own rule, since no font we bundle carries any of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnderlineStyle {
	None,
	Single,
	Double,
	Dotted,
	Dashed,
	Curly,
}

/// The shape the cursor draws as (§23). A remote picks it with DECSCUSR (`CSI Ps SP q`) — a
/// steady or blinking block, underline, or bar — and the engine tracks the choice. `Bar` is
/// the engine's "beam"; `HollowBlock` is the outline a terminal shows when its window is not
/// focused. `Hidden` is a shape a program can select outright, distinct from DECTCEM hiding
/// the cursor (`hide_cursor`) — the grid draws neither. Blink is deliberately not carried:
/// cmote runs no animation timer, so the cursor is always steady.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
	Block,
	Underline,
	Bar,
	HollowBlock,
	Hidden,
}

/// One cell's glyph and the attributes the renderer reads (§9). Owned rather than borrowed,
/// so the type never exposes the engine's own cell representation — the glyph is
/// materialised once when the cell is read. `text` is empty for a blank cell, so it costs
/// no allocation and `has_contents` is simply "non-empty".
#[derive(Debug, Clone)]
pub struct Cell {
	text: String,
	fg: Color,
	bg: Color,
	bold: bool,
	dim: bool,
	italic: bool,
	hidden: bool,
	strikeout: bool,
	underline: UnderlineStyle,
	underline_color: Option<Color>,
	inverse: bool,
	wide: bool,
	wide_continuation: bool,
	hyperlink: Option<String>,
}

impl Cell {
	/// The cell's glyph — a grapheme, possibly empty (a blank cell).
	pub fn contents(&self) -> &str {
		&self.text
	}

	/// Whether the cell holds a glyph at all (a blank cell holds none).
	pub fn has_contents(&self) -> bool {
		!self.text.is_empty()
	}

	/// A double-width glyph (CJK, some emoji): it claims two columns.
	pub fn is_wide(&self) -> bool {
		self.wide
	}

	/// The trailing half of a wide glyph — it carries no glyph of its own; the lead cell
	/// reserved its column.
	pub fn is_wide_continuation(&self) -> bool {
		self.wide_continuation
	}

	/// The cell's foreground colour.
	pub fn fgcolor(&self) -> Color {
		self.fg
	}

	/// The cell's background colour.
	pub fn bgcolor(&self) -> Color {
		self.bg
	}

	/// Bold intensity.
	pub fn bold(&self) -> bool {
		self.bold
	}

	/// Faint intensity (SGR 2) — the renderer fades the foreground toward the background.
	pub fn dim(&self) -> bool {
		self.dim
	}

	/// Italic (SGR 3) — the renderer draws the glyph from a slanted face. Fira Mono has none,
	/// so the grid pulls italic cells from a bundled italic family instead (§23).
	pub fn italic(&self) -> bool {
		self.italic
	}

	/// Concealed (SGR 8) — the glyph is painted in its own background, so it holds its cell
	/// but shows nothing. Its `contents` are untouched, so a copy still yields the real text.
	pub fn hidden(&self) -> bool {
		self.hidden
	}

	/// Crossed out (SGR 9) — a rule through the cell's middle.
	pub fn strikeout(&self) -> bool {
		self.strikeout
	}

	/// Which underline style the cell carries, if any (§9, §23).
	pub fn underline(&self) -> UnderlineStyle {
		self.underline
	}

	/// The underline's own colour (SGR 58), or `None` when the cell set none — in which case
	/// the renderer draws the rule in the foreground colour.
	pub fn underline_color(&self) -> Option<Color> {
		self.underline_color
	}

	/// Reverse video — foreground and background swapped.
	pub fn inverse(&self) -> bool {
		self.inverse
	}

	/// The URI of the OSC 8 hyperlink covering this cell, or `None` when the cell is not part
	/// of a link (§24). A program sets it with `ESC ] 8 ; params ; URI ST`, and the engine
	/// records the same URI on every cell up to the closing `ESC ] 8 ; ; ST`; cmote reads it
	/// back so a click on the cell can open the link. The bytes are the remote's verbatim —
	/// what may actually be opened is `link`'s decision, not the cell's.
	pub fn hyperlink(&self) -> Option<&str> {
		self.hyperlink.as_deref()
	}
}

/// A borrowed view of the terminal screen for one read. Cheap to copy — it wraps a
/// reference to the engine's terminal — and every accessor hands back cmote's own types, so
/// the caller never sees the engine.
#[derive(Clone, Copy)]
pub struct Screen<'a> {
	engine: &'a Engine,
}

impl<'a> Screen<'a> {
	/// Wrap the engine's terminal. Built by `Terminal::screen`; the engine type is the one
	/// thing that changed when the emulator was swapped (§16, §23).
	pub(crate) fn new(engine: &'a Engine) -> Self {
		Self { engine }
	}

	/// The grid size as `(rows, cols)`.
	pub fn size(&self) -> (u16, u16) {
		(
			self.engine.screen_lines() as u16,
			self.engine.columns() as u16,
		)
	}

	/// The cursor's `(row, col)` within the active screen, zero-based. This is where the shell
	/// is writing — always on the live screen, never in history. When the viewport is scrolled
	/// back (`display_offset` > 0) the cursor's row ON SCREEN is this plus the offset, which the
	/// renderer works out so it can leave the cursor undrawn once it drops below the viewport (§23).
	pub fn cursor_position(&self) -> (u16, u16) {
		let point = self.engine.grid().cursor.point;
		(point.line.0.max(0) as u16, point.column.0 as u16)
	}

	/// How far the viewport is scrolled back into history, in lines — 0 at the live bottom, up
	/// to the retained history depth at the top (§23). The renderer adds it to a viewport row to
	/// reach the grid line to read (`cell`), and to the cursor's active-screen row to find where
	/// the cursor sits on screen. cmote moves it through `Terminal::scroll`; the engine keeps it
	/// stationary as new output arrives, so reading history is never yanked to the bottom by activity.
	pub fn display_offset(&self) -> u16 {
		self.engine.grid().display_offset() as u16
	}

	/// How many scrolled-off lines the engine is currently retaining — the depth the scroll
	/// indicator sizes its thumb against (§23). The engine grows this lazily up to the configured
	/// cap (`SCROLLBACK`) as output scrolls off the top, and it is zero on the alternate screen,
	/// which keeps no history. Paired with `display_offset`: the viewport can be scrolled back from
	/// 0 (the live bottom) up to this many lines, so `history_size + screen rows` is the whole
	/// document the indicator maps the thumb onto.
	pub fn history_size(&self) -> u16 {
		self.engine.grid().history_size() as u16
	}

	/// Whether the cursor is hidden (DECTCEM off).
	pub fn hide_cursor(&self) -> bool {
		!self.engine.mode().contains(TermMode::SHOW_CURSOR)
	}

	/// The shape the cursor should draw as (§23), as the remote last set it with DECSCUSR.
	/// Independent of `hide_cursor` (DECTCEM): a program can pick a shape and still hide the
	/// cursor, so the grid checks both. Blink is dropped on the way through — cmote draws a
	/// steady cursor whatever the remote asked (see `CursorShape`).
	pub fn cursor_shape(&self) -> CursorShape {
		match self.engine.cursor_style().shape {
			EngineCursorShape::Block => CursorShape::Block,
			EngineCursorShape::Underline => CursorShape::Underline,
			EngineCursorShape::Beam => CursorShape::Bar,
			EngineCursorShape::HollowBlock => CursorShape::HollowBlock,
			EngineCursorShape::Hidden => CursorShape::Hidden,
		}
	}

	/// Whether application-cursor mode (DECCKM) is on — arrows send SS3, not CSI (§9).
	pub fn application_cursor(&self) -> bool {
		self.engine.mode().contains(TermMode::APP_CURSOR)
	}

	/// Whether application-keypad mode (DECKPAM, `ESC =`) is on — the numpad's own keys should
	/// send SS3 sequences instead of their characters (§36). Every ncurses program turns it on as
	/// part of terminfo's `smkx`, together with DECCKM (`application_cursor`), which is why cmote
	/// honours it only for the numpad keys that carry no NumLock ambiguity: see
	/// `keymap::application_keypad_bytes` for exactly which, and why the digits are left alone.
	pub fn application_keypad(&self) -> bool {
		self.engine.mode().contains(TermMode::APP_KEYPAD)
	}

	/// Which kitty keyboard protocol flags the remote program currently has in effect (§25).
	/// The engine parses the push/pop/set sequences and folds the active flag set into its mode
	/// bits (it is told to by `config.kitty_keyboard`, set in `Terminal::new`); cmote reads them
	/// back here so `keymap`/`kitty` know how far to enhance the key encoding. All five are off
	/// until a program pushes them, so the default is the ordinary legacy encoding.
	pub fn kitty_flags(&self) -> KittyFlags {
		let mode = self.engine.mode();
		KittyFlags {
			disambiguate: mode.contains(TermMode::DISAMBIGUATE_ESC_CODES),
			report_events: mode.contains(TermMode::REPORT_EVENT_TYPES),
			report_alternates: mode.contains(TermMode::REPORT_ALTERNATE_KEYS),
			report_all: mode.contains(TermMode::REPORT_ALL_KEYS_AS_ESC),
			report_text: mode.contains(TermMode::REPORT_ASSOCIATED_TEXT),
		}
	}

	/// Whether bracketed paste (DECSET 2004) is on — a paste is framed so the shell inserts
	/// it literally (§9).
	pub fn bracketed_paste(&self) -> bool {
		self.engine.mode().contains(TermMode::BRACKETED_PASTE)
	}

	/// Whether focus reporting (DECSET 1004) is on — the remote asked to be told when the
	/// terminal gains or loses focus, and cmote answers with `CSI I` / `CSI O` (§23). It is
	/// cmote (not the engine) that watches the window and sends those bytes; this only reads
	/// back whether the program turned the mode on. What counts as focus is `app`'s call —
	/// cmote treats a pane switch off the shell as a focus-out too, since the remote knows
	/// nothing of cmote's own panels.
	pub fn focus_reporting(&self) -> bool {
		self.engine.mode().contains(TermMode::FOCUS_IN_OUT)
	}

	/// Which mouse protocol the remote program turned on (§9). The three reporting modes are
	/// mutually exclusive in the engine, so the most specific one set wins.
	pub fn mouse_mode(&self) -> MouseMode {
		let mode = self.engine.mode();
		if mode.contains(TermMode::MOUSE_MOTION) {
			MouseMode::AnyMotion
		} else if mode.contains(TermMode::MOUSE_DRAG) {
			MouseMode::ButtonMotion
		} else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
			MouseMode::PressRelease
		} else {
			MouseMode::None
		}
	}

	/// How mouse reports are encoded (§9).
	pub fn mouse_encoding(&self) -> MouseEncoding {
		let mode = self.engine.mode();
		if mode.contains(TermMode::SGR_MOUSE) {
			MouseEncoding::Sgr
		} else if mode.contains(TermMode::UTF8_MOUSE) {
			MouseEncoding::Utf8
		} else {
			MouseEncoding::Default
		}
	}

	/// The cell at `(row, col)`, or `None` when it is out of bounds. `row`/`col` are in the
	/// same visible-viewport coordinates the renderer walks.
	pub fn cell(&self, row: u16, col: u16) -> Option<Cell> {
		if row as usize >= self.engine.screen_lines() || col as usize >= self.engine.columns() {
			return None;
		}
		// A viewport row maps to a grid line by subtracting the display offset: at the live
		// bottom (offset 0) row 0 is grid line 0, and scrolling back walks the window into the
		// negative lines the engine stores history on (§23). The engine clamps the offset to the
		// retained history depth, so the resulting line is always within the stored grid.
		let line = i32::from(row) - i32::from(self.display_offset());
		let cell = &self.engine.grid()[Line(line)][Column(col as usize)];
		Some(build_cell(cell))
	}
}

/// Build the engine-agnostic cell from the engine's own.
fn build_cell(cell: &EngineCell) -> Cell {
	let mut text = String::new();
	text.push(cell.c);
	if let Some(zerowidth) = cell.zerowidth() {
		text.extend(zerowidth);
	}
	// A lone space is a blank cell: keep `text` empty so `has_contents` is false and the
	// renderer's blank-cell fast paths (skip the glyph, trim trailing blanks) behave exactly
	// as they did over the previous engine.
	if text == " " {
		text.clear();
	}

	Cell {
		text,
		fg: color(cell.fg),
		bg: color(cell.bg),
		bold: cell.flags.contains(Flags::BOLD),
		dim: cell.flags.contains(Flags::DIM),
		italic: cell.flags.contains(Flags::ITALIC),
		hidden: cell.flags.contains(Flags::HIDDEN),
		strikeout: cell.flags.contains(Flags::STRIKEOUT),
		underline: underline(cell.flags),
		underline_color: cell.underline_color().map(color),
		inverse: cell.flags.contains(Flags::INVERSE),
		wide: cell.flags.contains(Flags::WIDE_CHAR),
		wide_continuation: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
		// The engine shares one `Arc<Hyperlink>` across a link's cells; cmote's cell is owned
		// per read (like its `text`), so the URI is copied out here — links are rare, so a
		// blank cell still allocates nothing.
		hyperlink: cell.hyperlink().map(|link| link.uri().to_owned()),
	}
}

/// Read the underline style out of a cell's flags. The engine keeps each style as its own
/// flag and sets at most one at a time (turning an underline on clears the others first), but
/// a specific style is preferred over the plain one if both were somehow present, so exactly
/// one style is ever returned.
fn underline(flags: Flags) -> UnderlineStyle {
	if flags.contains(Flags::DOUBLE_UNDERLINE) {
		UnderlineStyle::Double
	} else if flags.contains(Flags::UNDERCURL) {
		UnderlineStyle::Curly
	} else if flags.contains(Flags::DOTTED_UNDERLINE) {
		UnderlineStyle::Dotted
	} else if flags.contains(Flags::DASHED_UNDERLINE) {
		UnderlineStyle::Dashed
	} else if flags.contains(Flags::UNDERLINE) {
		UnderlineStyle::Single
	} else {
		UnderlineStyle::None
	}
}

/// Map the engine's colour onto ours. A named colour is either a role the renderer resolves
/// to its own default (foreground / background / cursor) or one of the palette slots, which
/// we hand back as an index so the grid's xterm-256 palette stays the single source of the
/// actual RGB.
fn color(color: EngineColor) -> Color {
	match color {
		EngineColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
		EngineColor::Indexed(index) => Color::Indexed(index),
		// `NamedColor`'s discriminants are the palette indices: 0-15 are the ANSI and bright
		// slots, 259-266 the dim variants (mapped back onto the base ANSI slots since cmote
		// draws no dim palette), and everything else (foreground / background / cursor and
		// the bright/dim foreground roles) is the terminal default.
		EngineColor::Named(named) => match named as usize {
			index @ 0..=15 => Color::Indexed(index as u8),
			index @ 259..=266 => Color::Indexed((index - 259) as u8),
			_ => Color::Default,
		},
	}
}

/// A compile-time check that `NamedColor`'s discriminants are where `color` assumes: the 16
/// ANSI slots at 0-15 and the dim run beginning at 259. If a future engine version renumbers
/// them, this fails to build rather than silently mis-mapping a colour.
const _: () = {
	assert!(NamedColor::Black as usize == 0);
	assert!(NamedColor::BrightWhite as usize == 15);
	assert!(NamedColor::DimBlack as usize == 259);
};

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;

	// The cell at (0,0) after feeding `input` to a fresh 1-row emulator — the standard way to
	// probe how one SGR sequence lands, read back through the view the app actually sees.
	fn cell0(input: &str) -> Cell {
		let mut terminal = Terminal::new(1, 8);
		terminal.process(input.as_bytes());
		terminal
			.screen()
			.cell(0, 0)
			.expect("cell (0,0) is in bounds")
	}

	#[test]
	fn plain_text_carries_no_attributes() {
		let cell = cell0("x");
		assert!(!cell.dim());
		assert!(!cell.italic());
		assert!(!cell.hidden());
		assert!(!cell.strikeout());
		assert_eq!(cell.underline(), UnderlineStyle::None);
		assert_eq!(cell.underline_color(), None);
	}

	#[test]
	fn intensity_and_line_attributes_map_from_their_sgr_codes() {
		// SGR 2 faint, 3 italic, 8 conceal, 9 crossed-out — each its own flag on the cell.
		assert!(cell0("\x1b[2mx").dim());
		assert!(cell0("\x1b[3mx").italic());
		assert!(cell0("\x1b[8mx").hidden());
		assert!(cell0("\x1b[9mx").strikeout());
	}

	#[test]
	fn every_underline_style_maps_from_its_sub_parameter() {
		// The plain form and the four extended sub-parameters (`CSI 4 : n m`) the engine knows.
		assert_eq!(cell0("\x1b[4mx").underline(), UnderlineStyle::Single);
		assert_eq!(cell0("\x1b[4:2mx").underline(), UnderlineStyle::Double);
		assert_eq!(cell0("\x1b[4:3mx").underline(), UnderlineStyle::Curly);
		assert_eq!(cell0("\x1b[4:4mx").underline(), UnderlineStyle::Dotted);
		assert_eq!(cell0("\x1b[4:5mx").underline(), UnderlineStyle::Dashed);
		// And SGR 24 turns any of them back off.
		assert_eq!(cell0("\x1b[4m\x1b[24mx").underline(), UnderlineStyle::None);
	}

	#[test]
	fn an_osc_8_hyperlink_is_read_back_on_its_cells() {
		// `ESC ] 8 ; ; URI BEL` opens a link, the text after it is the link's cells, and
		// `ESC ] 8 ; ; BEL` closes it. The covered cell carries the URI; the cell after the
		// close does not (§24). A wider grid so both cells are in bounds.
		let mut terminal = Terminal::new(1, 8);
		terminal.process(b"\x1b]8;;https://example.com/\x07A\x1b]8;;\x07B");
		let linked = terminal
			.screen()
			.cell(0, 0)
			.expect("cell (0,0) is in bounds");
		assert_eq!(linked.contents(), "A");
		assert_eq!(linked.hyperlink(), Some("https://example.com/"));
		let plain = terminal
			.screen()
			.cell(0, 1)
			.expect("cell (0,1) is in bounds");
		assert_eq!(plain.contents(), "B");
		assert_eq!(plain.hyperlink(), None);
	}

	#[test]
	fn a_separate_underline_colour_is_read_back() {
		// SGR 58:5:n sets the underline's own colour to a palette slot, independent of the
		// foreground; the view hands it back as an indexed colour for the grid to resolve.
		let cell = cell0("\x1b[4;58:5:9mx");
		assert_eq!(cell.underline(), UnderlineStyle::Single);
		assert_eq!(cell.underline_color(), Some(Color::Indexed(9)));
	}

	// The cursor shape after feeding a DECSCUSR sequence to a fresh emulator.
	fn cursor_shape(input: &str) -> CursorShape {
		let mut terminal = Terminal::new(1, 8);
		terminal.process(input.as_bytes());
		terminal.screen().cursor_shape()
	}

	#[test]
	fn a_fresh_screen_shows_a_block_cursor() {
		// The engine's default, and cmote's: no DECSCUSR has run.
		assert_eq!(cursor_shape(""), CursorShape::Block);
	}

	#[test]
	fn decscusr_selects_the_cursor_shape() {
		// `CSI Ps SP q` — the space is the intermediate. Odd Ps blink, even steady; blink is
		// dropped, so each pair lands on one shape. 0 and 1 reset to the block default.
		assert_eq!(cursor_shape("\x1b[0 q"), CursorShape::Block);
		assert_eq!(cursor_shape("\x1b[2 q"), CursorShape::Block);
		assert_eq!(cursor_shape("\x1b[3 q"), CursorShape::Underline);
		assert_eq!(cursor_shape("\x1b[4 q"), CursorShape::Underline);
		assert_eq!(cursor_shape("\x1b[5 q"), CursorShape::Bar);
		assert_eq!(cursor_shape("\x1b[6 q"), CursorShape::Bar);
	}

	#[test]
	fn focus_reporting_follows_decset_1004() {
		// Off until a program asks; `?1004h` turns it on and `?1004l` back off. This is what
		// tells cmote whether to send `CSI I` / `CSI O` on a focus change (§23).
		let mut terminal = Terminal::new(1, 8);
		assert!(!terminal.screen().focus_reporting());
		terminal.process(b"\x1b[?1004h");
		assert!(terminal.screen().focus_reporting());
		terminal.process(b"\x1b[?1004l");
		assert!(!terminal.screen().focus_reporting());
	}

	#[test]
	fn application_keypad_follows_deckpam() {
		// Off until a program asks; `ESC =` (DECKPAM) turns it on and `ESC >` (DECKPNM) back off —
		// the pair terminfo's `smkx`/`rmkx` send around a full-screen session (§36). The engine
		// tracks the mode bit, so this is a plain read off the seam, like DECCKM.
		let mut terminal = Terminal::new(1, 8);
		assert!(!terminal.screen().application_keypad());
		terminal.process(b"\x1b=");
		assert!(terminal.screen().application_keypad());
		terminal.process(b"\x1b>");
		assert!(!terminal.screen().application_keypad());
	}

	#[test]
	fn kitty_flags_track_what_a_program_pushed() {
		// Off until a program asks (§25). `CSI > 1 u` pushes the disambiguate flag (bit 1); `CSI >
		// 9 u` (bits 1 + 8) sets disambiguate + report-all together; `CSI < u` pops back to the
		// previous entry. cmote reads the active set off the seam to drive the key encoder — the
		// engine owns the stack because `config.kitty_keyboard` is on.
		let mut terminal = Terminal::new(1, 8);
		assert!(!terminal.screen().kitty_flags().is_active());
		terminal.process(b"\x1b[>1u");
		let flags = terminal.screen().kitty_flags();
		assert!(flags.disambiguate);
		assert!(!flags.report_all);
		terminal.process(b"\x1b[>9u");
		let flags = terminal.screen().kitty_flags();
		assert!(flags.disambiguate);
		assert!(flags.report_all);
		terminal.process(b"\x1b[<u");
		assert!(!terminal.screen().kitty_flags().report_all);
	}

	#[test]
	fn history_grows_as_output_scrolls_off_the_top() {
		// A two-row screen starts with no history; feeding five lines pushes three off the top,
		// and the retained depth is what the scroll indicator measures itself against (§23). The
		// alternate screen keeps none, so a full-screen program shows no indicator at all.
		let mut terminal = Terminal::new(2, 8);
		assert_eq!(terminal.screen().history_size(), 0);
		terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive");
		assert_eq!(terminal.screen().history_size(), 3);
		terminal.process(b"\x1b[?1049h");
		assert_eq!(terminal.screen().history_size(), 0);
	}
}

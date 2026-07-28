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
use alacritty_terminal::vte::ansi::{Color as EngineColor, NamedColor};

use super::Engine;

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
	underline: bool,
	inverse: bool,
	wide: bool,
	wide_continuation: bool,
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

	/// Any underline style — the grid draws a single rule for all of them until §23's
	/// enrich stage teaches it the distinct styles.
	pub fn underline(&self) -> bool {
		self.underline
	}

	/// Reverse video — foreground and background swapped.
	pub fn inverse(&self) -> bool {
		self.inverse
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

	/// The cursor's `(row, col)`, zero-based, in visible-viewport coordinates. We keep no
	/// scrollback (§9), so the active screen's top row is always row 0.
	pub fn cursor_position(&self) -> (u16, u16) {
		let point = self.engine.grid().cursor.point;
		(point.line.0.max(0) as u16, point.column.0 as u16)
	}

	/// Whether the cursor is hidden (DECTCEM off).
	pub fn hide_cursor(&self) -> bool {
		!self.engine.mode().contains(TermMode::SHOW_CURSOR)
	}

	/// Whether application-cursor mode (DECCKM) is on — arrows send SS3, not CSI (§9).
	pub fn application_cursor(&self) -> bool {
		self.engine.mode().contains(TermMode::APP_CURSOR)
	}

	/// Whether bracketed paste (DECSET 2004) is on — a paste is framed so the shell inserts
	/// it literally (§9).
	pub fn bracketed_paste(&self) -> bool {
		self.engine.mode().contains(TermMode::BRACKETED_PASTE)
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
		let cell = &self.engine.grid()[Line(i32::from(row))][Column(col as usize)];
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
		underline: cell.flags.intersects(Flags::ALL_UNDERLINES),
		inverse: cell.flags.contains(Flags::INVERSE),
		wide: cell.flags.contains(Flags::WIDE_CHAR),
		wide_continuation: cell.flags.contains(Flags::WIDE_CHAR_SPACER),
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

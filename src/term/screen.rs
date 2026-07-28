// term/screen.rs — cmote's engine-agnostic view of the terminal screen (PLAN §9, §16).
//
// The rest of the app must not know WHICH VT engine backs the terminal. `ui::grid`,
// `ui::selection` and `app` read the screen only through the types here — a `Screen`
// borrowed for one frame, its `Cell`s, a `Color`, and the mouse / paste / cursor modes.
// That is the seam PLAN §9 always intended: the grid used to read `vt100::Cell` directly,
// which is the leak this closes. With it closed, swapping the engine (vt100 ->
// alacritty_terminal, §16) is a change to how this view is BUILT — only this file and
// `term::mod` — not a change to a single caller, because the method surface stays put.
//
// Today the view is built over `vt100`; `vt100` is named nowhere else outside `term/`.

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
/// asked for the mouse. Mirrors the set the engine tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseMode {
	None,
	Press,
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

	/// A single underline (the only style the current engine distinguishes).
	pub fn underline(&self) -> bool {
		self.underline
	}

	/// Reverse video — foreground and background swapped.
	pub fn inverse(&self) -> bool {
		self.inverse
	}
}

/// A borrowed view of the terminal screen for one read. Cheap to copy — it wraps a
/// reference to the engine's screen — and every accessor hands back cmote's own types, so
/// the caller never sees the engine.
#[derive(Clone, Copy)]
pub struct Screen<'a> {
	inner: &'a vt100::Screen,
}

impl<'a> Screen<'a> {
	/// Wrap the engine's screen. Built by `Terminal::screen`; the engine type is the one
	/// thing that changes when the emulator is swapped (§16).
	pub(crate) fn new(inner: &'a vt100::Screen) -> Self {
		Self { inner }
	}

	/// The grid size as `(rows, cols)`.
	pub fn size(&self) -> (u16, u16) {
		self.inner.size()
	}

	/// The cursor's `(row, col)`, zero-based.
	pub fn cursor_position(&self) -> (u16, u16) {
		self.inner.cursor_position()
	}

	/// Whether the cursor is hidden (DECTCEM off).
	pub fn hide_cursor(&self) -> bool {
		self.inner.hide_cursor()
	}

	/// Whether application-cursor mode (DECCKM) is on — arrows send SS3, not CSI (§9).
	pub fn application_cursor(&self) -> bool {
		self.inner.application_cursor()
	}

	/// Whether bracketed paste (DECSET 2004) is on — a paste is framed so the shell inserts
	/// it literally (§9).
	pub fn bracketed_paste(&self) -> bool {
		self.inner.bracketed_paste()
	}

	/// Which mouse protocol the remote program turned on (§9).
	pub fn mouse_mode(&self) -> MouseMode {
		mouse_mode(self.inner.mouse_protocol_mode())
	}

	/// How mouse reports are encoded (§9).
	pub fn mouse_encoding(&self) -> MouseEncoding {
		mouse_encoding(self.inner.mouse_protocol_encoding())
	}

	/// The cell at `(row, col)`, or `None` when it is out of bounds.
	pub fn cell(&self, row: u16, col: u16) -> Option<Cell> {
		self.inner.cell(row, col).map(build_cell)
	}
}

/// Build the engine-agnostic cell from the engine's own.
fn build_cell(cell: &vt100::Cell) -> Cell {
	Cell {
		text: cell.contents().to_owned(),
		fg: color(cell.fgcolor()),
		bg: color(cell.bgcolor()),
		bold: cell.bold(),
		underline: cell.underline(),
		inverse: cell.inverse(),
		wide: cell.is_wide(),
		wide_continuation: cell.is_wide_continuation(),
	}
}

/// Map the engine's colour onto ours.
fn color(color: vt100::Color) -> Color {
	match color {
		vt100::Color::Default => Color::Default,
		vt100::Color::Idx(index) => Color::Indexed(index),
		vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
	}
}

/// Map the engine's mouse mode onto ours.
fn mouse_mode(mode: vt100::MouseProtocolMode) -> MouseMode {
	match mode {
		vt100::MouseProtocolMode::None => MouseMode::None,
		vt100::MouseProtocolMode::Press => MouseMode::Press,
		vt100::MouseProtocolMode::PressRelease => MouseMode::PressRelease,
		vt100::MouseProtocolMode::ButtonMotion => MouseMode::ButtonMotion,
		vt100::MouseProtocolMode::AnyMotion => MouseMode::AnyMotion,
	}
}

/// Map the engine's mouse encoding onto ours.
fn mouse_encoding(encoding: vt100::MouseProtocolEncoding) -> MouseEncoding {
	match encoding {
		vt100::MouseProtocolEncoding::Default => MouseEncoding::Default,
		vt100::MouseProtocolEncoding::Utf8 => MouseEncoding::Utf8,
		vt100::MouseProtocolEncoding::Sgr => MouseEncoding::Sgr,
	}
}

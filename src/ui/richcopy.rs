// ui/richcopy.rs — serialise a terminal selection to styled HTML for a rich copy (PLAN §10).
//
// Ctrl+C copies the selection as HTML so a paste into a rich editor (a document, a browser, an
// email) keeps the terminal's colours and text attributes; a plain-text fallback is put on the
// clipboard alongside it (by `app`, through arboard) for editors — and the shell itself — that
// read only text. This module is the pure serialiser: given the selection geometry and a screen
// to read, it returns the HTML string. There is no clipboard and no widget here, so it is a
// plain testable function, mirroring how `selection` keeps the copy *geometry* free of I/O.
//
// The output wraps the whole selection in one <pre> whose default colours are the terminal's
// own, so a cell using the default foreground/background needs no per-cell markup at all — only
// cells that differ carry a <span>. Consecutive cells sharing a style are merged into one span,
// which keeps the HTML small for the common case of long same-colour runs.

use std::fmt::Write as _;

use crate::palette;
use crate::term::screen::{Cell, Color, Screen, UnderlineStyle};
use crate::ui::selection::Selection;

/// The resolved appearance of a run of cells: RGB foreground/background (already accounting for
/// reverse video, faint and conceal) and the boolean text attributes. Equality drives run
/// merging — adjacent cells with an equal `Style` share one <span>.
#[derive(PartialEq, Eq)]
struct Style {
	fg: (u8, u8, u8),
	bg: (u8, u8, u8),
	bold: bool,
	italic: bool,
	underline: bool,
	strikeout: bool,
}

/// Serialise the current `selection` over `screen` to a styled HTML fragment (PLAN §10). Returns
/// an empty string when nothing is selected. Rows are joined with a newline inside the <pre>, so
/// the pasted block keeps the terminal's line breaks and monospaced columns — except across a WRAP,
/// where the two rows are one logical line the terminal folded and are joined with nothing (§42),
/// exactly as the plain-text copy does.
pub fn to_html(selection: &Selection, screen: Screen<'_>) -> String {
	let rows = selection.selected_rows(screen);
	if rows.is_empty() {
		return String::new();
	}

	// The <pre> carries the terminal's default colours and a monospaced family, so a default
	// cell needs no markup and the whole block reads as the terminal shows it.
	let mut html = format!(
		"<pre style=\"font-family:'Courier New',monospace;color:{};background-color:{};\">",
		hex_color(palette::DEFAULT_FG),
		hex_color(palette::DEFAULT_BG),
	);

	for (index, row) in rows.iter().enumerate() {
		// The break belongs to the row BEFORE this one, and a wrapped row has none (§42).
		if index > 0 && !rows[index - 1].wrapped {
			html.push('\n');
		}
		emit_row(&mut html, &row.cells);
	}

	html.push_str("</pre>");
	html
}

/// Append one row's cells to `html`, merging neighbours that share a style into a single span.
fn emit_row(html: &mut String, cells: &[Cell]) {
	let mut run = String::new();
	let mut run_style: Option<Style> = None;

	for cell in cells {
		let style = style_of(cell);
		// A blank cell (no glyph) still occupies a column, so it copies as a space — keeping the
		// layout, and its style, exactly as the grid shows it.
		let glyph = if cell.has_contents() {
			cell.contents()
		} else {
			" "
		};

		// Flush the run in progress when the style changes, then start a new one on this cell.
		if run_style.as_ref() != Some(&style) {
			flush_run(html, &run, run_style.as_ref());
			run.clear();
			run_style = Some(style);
		}
		escape_into(&mut run, glyph);
	}
	flush_run(html, &run, run_style.as_ref());
}

/// Write one finished run: the escaped text wrapped in a <span> when its style differs from the
/// <pre> default, or bare text when it does not (the common case, so the HTML stays lean).
fn flush_run(html: &mut String, run: &str, style: Option<&Style>) {
	if run.is_empty() {
		return;
	}
	match style.map(style_css) {
		Some(css) if !css.is_empty() => {
			html.push_str("<span style=\"");
			html.push_str(&css);
			html.push_str("\">");
			html.push_str(run);
			html.push_str("</span>");
		}
		_ => html.push_str(run),
	}
}

/// Resolve a cell's appearance to concrete RGB and boolean attributes. Reverse video swaps
/// foreground and background; faint (dim) fades the foreground halfway to the background, the way
/// the grid renders it; conceal paints the glyph in its own background so copied-as-shown text
/// stays invisible (the plain-text fallback still carries the real characters).
fn style_of(cell: &Cell) -> Style {
	let mut fg = to_rgb(cell.fgcolor(), palette::DEFAULT_FG);
	let mut bg = to_rgb(cell.bgcolor(), palette::DEFAULT_BG);
	if cell.inverse() {
		std::mem::swap(&mut fg, &mut bg);
	}
	if cell.dim() {
		fg = blend(fg, bg);
	}
	if cell.hidden() {
		fg = bg;
	}
	Style {
		fg,
		bg,
		bold: cell.bold(),
		italic: cell.italic(),
		underline: cell.underline() != UnderlineStyle::None,
		strikeout: cell.strikeout(),
	}
}

/// The CSS for a style, empty when it is exactly the <pre> default (so the caller can skip the
/// span). Only the properties that differ from the default are emitted.
fn style_css(style: &Style) -> String {
	let mut css = String::new();
	// `write!` rather than `push_str(&format!(…))`: it formats straight into `css` instead of
	// building a throwaway `String` first. Writing to a `String` cannot fail, so the `Result` is
	// discarded — `fmt::Write` returns one only because the trait also covers fallible sinks.
	if style.fg != palette::DEFAULT_FG {
		let _ = write!(css, "color:{};", hex_color(style.fg));
	}
	if style.bg != palette::DEFAULT_BG {
		let _ = write!(css, "background-color:{};", hex_color(style.bg));
	}
	if style.bold {
		css.push_str("font-weight:bold;");
	}
	if style.italic {
		css.push_str("font-style:italic;");
	}
	// Underline and strike-through are the one CSS property, so combine them into one value.
	match (style.underline, style.strikeout) {
		(true, true) => css.push_str("text-decoration:underline line-through;"),
		(true, false) => css.push_str("text-decoration:underline;"),
		(false, true) => css.push_str("text-decoration:line-through;"),
		(false, false) => {}
	}
	css
}

/// Resolve a cell colour to RGB: the terminal default falls back to `default`, an indexed slot
/// goes through the shared xterm-256 palette, and a truecolor value passes through — the same
/// resolution the grid uses, so the copy matches what is on screen (PLAN §9).
fn to_rgb(color: Color, default: (u8, u8, u8)) -> (u8, u8, u8) {
	match color {
		Color::Default => default,
		Color::Indexed(index) => palette::xterm_256(index),
		Color::Rgb(r, g, b) => (r, g, b),
	}
}

/// Fade `fg` halfway toward `bg` — how the grid draws faint (SGR 2) text.
fn blend(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> (u8, u8, u8) {
	let mix = |a: u8, b: u8| u16::midpoint(u16::from(a), u16::from(b)) as u8;
	(mix(fg.0, bg.0), mix(fg.1, bg.1), mix(fg.2, bg.2))
}

/// Format an RGB triple as a CSS hex colour (`#rrggbb`).
fn hex_color((r, g, b): (u8, u8, u8)) -> String {
	format!("#{r:02x}{g:02x}{b:02x}")
}

/// Append `text` to `out`, escaping the three characters that are special in HTML body text, so
/// a glyph like `<` or `&` cannot break out of the markup.
fn escape_into(out: &mut String, text: &str) {
	for ch in text.chars() {
		match ch {
			'&' => out.push_str("&amp;"),
			'<' => out.push_str("&lt;"),
			'>' => out.push_str("&gt;"),
			_ => out.push(ch),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::Terminal;
	use crate::ui::selection::{DocSpot, Selection};

	// A document position (absolute line, column) — the space the selection endpoints live in (§40).
	// Nothing has scrolled off in these tests, so line N is the emulator's row N.
	fn grid_cell(line: u64, col: u16) -> DocSpot {
		DocSpot { line, col }
	}

	// A fresh emulator fed `input`, so a test can serialise real grid contents.
	fn terminal_with(rows: u16, cols: u16, input: &str) -> Terminal {
		let mut terminal = Terminal::new(rows, cols);
		terminal.process(input.as_bytes());
		terminal
	}

	// The HTML for a whole first-row selection over a fresh terminal fed `input`.
	fn html_of(cols: u16, input: &str) -> String {
		let terminal = terminal_with(1, cols, input);
		let selection = Selection::new(grid_cell(0, 0)).with_head(grid_cell(0, cols - 1));
		to_html(&selection, terminal.screen())
	}

	#[test]
	fn plain_text_needs_no_span_and_is_escaped() {
		// Default-styled text carries no <span>, and the three HTML specials are escaped so a
		// glyph can never break out of the markup.
		let html = html_of(10, "a<b>&c");
		assert!(html.starts_with("<pre "));
		assert!(html.ends_with("</pre>"));
		assert!(html.contains("\">a&lt;b&gt;&amp;c</pre>"));
		assert!(!html.contains("<span"));
	}

	#[test]
	fn a_coloured_cell_gets_a_span_with_its_rgb() {
		// SGR 31 = ANSI red (palette slot 1 = #800000). The run of red text is one span.
		let html = html_of(10, "\x1b[31mred\x1b[0m");
		assert!(html.contains("<span style=\"color:#800000;\">red</span>"));
	}

	#[test]
	fn text_attributes_map_to_css() {
		assert!(html_of(6, "\x1b[1mx").contains("font-weight:bold;"));
		assert!(html_of(6, "\x1b[3mx").contains("font-style:italic;"));
		assert!(html_of(6, "\x1b[4mx").contains("text-decoration:underline;"));
		assert!(html_of(6, "\x1b[9mx").contains("text-decoration:line-through;"));
	}

	#[test]
	fn reverse_video_swaps_foreground_and_background() {
		// SGR 7 with default colours: the foreground becomes the default background and vice
		// versa, so the span states both explicitly.
		let html = html_of(6, "\x1b[7mx");
		let fg = hex_color(palette::DEFAULT_BG);
		let bg = hex_color(palette::DEFAULT_FG);
		assert!(html.contains(&format!("color:{fg};background-color:{bg};")));
	}

	#[test]
	fn rows_are_joined_with_newlines() {
		let terminal = terminal_with(2, 6, "ab\r\ncd");
		let selection = Selection::new(grid_cell(0, 0)).with_head(grid_cell(1, 5));
		let html = to_html(&selection, terminal.screen());
		assert!(html.contains("\">ab\ncd</pre>"));
	}

	/// Except across a wrap, where the two rows are one logical line the window folded (§42) — the
	/// rich copy has to break in the same places the plain-text one does, or the two paste differently.
	#[test]
	fn a_wrapped_row_is_joined_to_the_next_without_a_break() {
		// Eight columns fed ten characters: row 0 wraps into row 1.
		let terminal = terminal_with(2, 8, "abcdefghij");
		let selection = Selection::new(grid_cell(0, 0)).with_head(grid_cell(1, 7));
		let html = to_html(&selection, terminal.screen());
		assert!(html.contains("\">abcdefghij</pre>"), "{html}");
	}

	#[test]
	fn an_empty_selection_serialises_to_nothing() {
		let terminal = terminal_with(1, 6, "hi");
		let selection = Selection::new(grid_cell(0, 0));
		assert_eq!(to_html(&selection, terminal.screen()), "");
	}

	#[test]
	fn trailing_blanks_are_trimmed() {
		// "hi" then blank padding to the grid width: the padding is trimmed, so no wall of
		// spaces is pasted.
		let html = html_of(10, "hi");
		assert!(html.contains("\">hi</pre>"));
		assert!(!html.contains("  "));
	}
}

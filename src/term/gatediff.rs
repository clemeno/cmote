// term/gatediff.rs — the gate compared with the engine it stands in front of (§102, §106).
//
// `term/gate.rs` is the one place cmote sits BETWEEN the parser and the engine, and it re-implements a
// dozen of the engine's own `Handler` methods so that left and right margins can bound them: LF, RI,
// SU, SD, IL, DL, ICH, DCH, CUF, CUB, CR, HT, and the glyph path itself. Re-implementing a method the
// engine already has is the most dangerous thing in this crate, because the two versions are only ever
// compared by hand.
//
// Every one of those arms opens the same way:
//
//     if !self.narrowed() { self.term.<the engine's own>(…); return }
//
// So with no margins set, the gate is supposed to be a pass-through: bytes in, the engine's own answer
// out, cmote's arithmetic never running. That is a property, not a hope, and this module is where it is
// checked against the only oracle that cannot be wrong about it — a SECOND engine, built with the same
// config and fed the same bytes, with no gate in front of it at all.
//
// §106 built the same kind of harness one layer lower, driving `vte`'s parser beside cmote's scanners
// (`term/differential.rs`), and its own limitation was written down at the time: it compares the
// PARSER, not the handler. This is that gap. Where §106 asks "would the engine have framed this
// sequence?", this asks "would the engine have produced this grid?".
//
// WHAT THIS DOES NOT ASK. cmote acts alone on purpose in a great many places — selective erase (§56),
// rectangular areas (§58), pictures (§41), the soft reset's long spelling (§72) — and `Terminal::process`
// also SYNTHESISES sequences of its own. A stream carrying any of those would differ from a bare engine
// by design, and this harness would be wrong to call that a defect. So the corpus below is deliberately
// narrow: scrolling, cursor motion and plain text, the sequences the gate claims to forward.

use std::sync::{Arc, Mutex};

use alacritty_terminal::Term;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::vte::ansi::{Color, Processor};

use super::{Engine, GridSize, Replies, ReplyBuffer, Terminal, engine_config};

/// The page the corpus runs on. Small on purpose: six rows means three line feeds reach the bottom, so
/// a stream that has to scroll — the whole point of the comparison — fits in a readable literal.
const ROWS: u16 = 6;
/// Ten columns, so a wrap is two words away rather than eighty.
const COLS: u16 = 10;

/// One cell as everything a caller could see of it: the character, the attribute bits, and the two
/// colours. Comparing only the character would let a divergence in what a scroll blanks WITH — the
/// background the cursor's pen carries — pass unnoticed, and that is exactly the kind of detail a
/// hand-written re-implementation gets wrong.
type Snapshot = (char, u16, Color, Color);

/// Everything observable about an engine after a stream: the cursor, the scrollback depth, and every
/// cell of the document — history rows included, which is the third of the three the seams were agreed
/// on. Scrollback matters here more than anywhere: a band scroll DISCARDS the row it pushes out while
/// the engine's own scroll PUSHES IT TO HISTORY, so a gate that performed a band scroll when it should
/// have forwarded would look right on screen and quietly lose the line.
struct Shot {
	/// The cursor's row and column, in the engine's own signed line space.
	cursor: (i32, usize),
	/// How many lines sit above the visible page.
	history: usize,
	/// The document top-down, starting at the oldest history line.
	rows: Vec<Vec<Snapshot>>,
}

/// Read an engine's whole document out.
fn shoot(engine: &Engine) -> Shot {
	let grid = engine.grid();
	let history = grid.total_lines() - grid.screen_lines();
	let cursor = grid.cursor.point;
	let first = -(history as i32);
	let last = grid.screen_lines() as i32;
	let rows = (first..last)
		.map(|line| {
			(0..grid.columns())
				.map(|column| {
					let cell = &grid[Line(line)][Column(column)];
					(cell.c, cell.flags.bits(), cell.fg, cell.bg)
				})
				.collect()
		})
		.collect();
	Shot {
		cursor: (cursor.line.0, cursor.column.0),
		history,
		rows,
	}
}

/// Feed a stream to cmote's terminal — parser, then gate, then engine — and read the result.
fn gated(bytes: &[u8]) -> Shot {
	let mut terminal = Terminal::new(ROWS, COLS);
	terminal.process(bytes);
	shoot(&terminal.term)
}

/// Feed the same stream to a bare engine, with the same config and no gate in front of it.
///
/// This is the oracle, and it is worth being precise about why it is one: nothing of cmote's is in this
/// path except the config, so whatever it produces is what `alacritty_terminal` alone would have
/// produced. The expected values are not derived, transcribed or reasoned about — they are measured.
fn ungated(bytes: &[u8]) -> Shot {
	let replies = Arc::new(Mutex::new(ReplyBuffer {
		rows: ROWS,
		cols: COLS,
		..ReplyBuffer::default()
	}));
	let mut engine = Term::new(
		engine_config(),
		&GridSize {
			rows: ROWS as usize,
			cols: COLS as usize,
		},
		Replies(Arc::clone(&replies)),
	);
	// Spelled out because `Processor`'s synchronised-update timeout is a type parameter with a default,
	// and the default is only chosen when something names it. `Terminal` names it the same way.
	let mut parser: Processor = Processor::new();
	parser.advance(&mut engine, bytes);
	shoot(&engine)
}

/// Where two shots first part company, in words a failure message can carry.
///
/// A whole-document diff would print sixty cells and bury the one that matters, so this reports the
/// FIRST disagreement and stops: the cursor, then the history depth, then the document in reading
/// order.
fn difference(gated: &Shot, bare: &Shot) -> Option<String> {
	if gated.cursor != bare.cursor {
		return Some(format!(
			"cursor: gated {:?}, engine alone {:?}",
			gated.cursor, bare.cursor
		));
	}
	if gated.history != bare.history {
		return Some(format!(
			"scrollback depth: gated {}, engine alone {}",
			gated.history, bare.history
		));
	}
	for (index, (left, right)) in gated.rows.iter().zip(bare.rows.iter()).enumerate() {
		for (column, (one, other)) in left.iter().zip(right.iter()).enumerate() {
			if one != other {
				return Some(format!(
					"row {} (of {}), column {column}: gated {:?}, engine alone {:?}",
					index,
					gated.rows.len(),
					one,
					other
				));
			}
		}
	}
	None
}

/// The streams the property is checked over — every one of them margin-free, so every gate arm in them
/// is meant to take its `!narrowed()` path and forward.
///
/// Each is named for what it makes the gate do, because a failure names the stream and nothing else.
fn margin_free_streams() -> Vec<(&'static str, &'static [u8])> {
	vec![
		(
			"a line feed past the last row, which has to reach the scrollback",
			b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight",
		),
		(
			"a line feed on the last row of a scrolling region",
			b"\x1b[2;5r\x1b[5;1Habc\r\n\r\nxyz",
		),
		(
			"SU and SD inside a region",
			b"\x1b[2;5r\x1b[3;1Habc\x1b[2S\x1b[1T",
		),
		(
			"IL and DL inside a region",
			b"\x1b[2;5rrow\x1b[4;1Hxy\x1b[2L\x1b[1M",
		),
		(
			"IL with the cursor outside the region, which the engine refuses",
			b"\x1b[2;5r\x1b[1;1H\x1b[3L",
		),
		(
			"a reverse index on the first row of a region",
			b"\x1b[2;5r\x1b[2;1H\x1bM\x1bM",
		),
		(
			"a region whose top is zero, the one the engine stores above the page",
			b"\x1b[0;3rA\r\n\x1b[3;1H\r\n\r\n",
		),
		(
			"a region as tall as the page, which is no region at all",
			b"\x1b[1;6rfull\r\n\r\n\r\n\r\n\r\n\r\ntail",
		),
		(
			"CNL and CPL, which move the row and the column at once",
			b"\x1b[2;5r\x1b[3;4HX\x1b[2EY\x1b[1FZ",
		),
		(
			"an autowrap over the right edge of the page",
			b"0123456789abcdefghij",
		),
		(
			"the deferred wrap, held on the last column until something moves",
			b"0123456789\x1b[1;1H\x1b[10CQ",
		),
		(
			"cursor motion that stops at the page edges",
			b"\x1b[3d\x1b[5GQ\x1b[99C\x1b[99B\x1b[99D\x1b[99AW",
		),
		(
			"a tab, whose stops are the engine's own table",
			b"\ta\tb\tc\td",
		),
		(
			"a region set, then set backwards, which the engine rejects",
			b"\x1b[2;5r\x1b[5;2r\x1b[5;1Hzz\r\n\r\n",
		),
		(
			"NEL, which the engine builds out of two methods the gate replaces",
			b"\x1b[2;5r\x1b[5;4Habc\x1bE\x1bEq",
		),
	]
}

/// Mode 69 on, with the band placed at the two edges of the page — margins that wall off nothing.
///
/// `Margins::narrowed` is `enabled && (left > 0 || right + 1 < cols)`, so this is deliberately NOT
/// narrowed and the gate is supposed to keep standing aside. It is the same rule `Region` has for a
/// vertical band as tall as the page, and it is worth a test of its own because the tempting
/// simplification — reading `narrowed()` as "is the mode set?" — would pass every margins test in
/// `mod.rs` while quietly moving every unmargined program onto cmote's arithmetic.
const FULL_WIDTH_MARGINS: &[u8] = b"\x1b[?69h\x1b[1;10s";

#[test]
fn with_no_margins_the_gate_is_the_engine() {
	// The property the whole margins design rests on: cmote's own arithmetic must not run at all until
	// a program asks for margins. Every stream here is margin-free, so every gate arm it reaches is
	// meant to hand the bytes straight to the engine — and a single arm that forgot its `!narrowed()`
	// guard would show up as a different grid, a different cursor or a lost scrollback line.
	//
	// The corpus is checked as a whole rather than one test per stream, so the failure names every
	// stream that disagrees instead of only the first alphabetically.
	let mut disagreed = Vec::new();
	for (name, stream) in margin_free_streams() {
		if let Some(where_) = difference(&gated(stream), &ungated(stream)) {
			disagreed.push(format!("  {name}: {where_}"));
		}
	}
	assert!(
		disagreed.is_empty(),
		"the gate parted company with the engine on {} of {} margin-free streams:\n{}",
		disagreed.len(),
		margin_free_streams().len(),
		disagreed.join("\n")
	);
}

/// The band the confinement sweep uses, one-based as a program writes it: columns 5 to 7, so the page
/// has four columns to the left of it and three to the right.
const BAND: (usize, usize) = (5, 7);

/// A page with every row filled with its own letter, then a narrow band walled off.
///
/// The drawing happens BEFORE the margins go on, which is both what a real program does and the only
/// order that puts known text OUTSIDE the band — the columns whose stillness is the whole property.
fn banded_page() -> Terminal {
	let mut terminal = Terminal::new(ROWS, COLS);
	for row in 0..ROWS {
		let letter = char::from(b'A' + row as u8);
		let text: String = std::iter::repeat_n(letter, COLS as usize).collect();
		terminal.process(format!("\x1b[{};1H{text}", row + 1).as_bytes());
	}
	let (left, right) = BAND;
	terminal.process(format!("\x1b[?69h\x1b[{left};{right}s").as_bytes());
	terminal
}

/// The band operations the confinement sweep runs, each with the cursor put inside the band first.
///
/// NOT oracle-backed, and that distinction is the reason this list is separate from the streams above.
/// A left or right margin is a thing the engine does not implement at all, so there is no second
/// implementation to measure against and these expectations come from xterm's own definition — an
/// operation bounded by the margins moves the columns between them and nothing else. Where the sweeps
/// above are measurements, this one is a reading of a specification, and it can be wrong in the way any
/// reading can.
fn band_operations() -> Vec<(&'static str, &'static [u8])> {
	vec![
		("SU, scrolling the band up", b"\x1b[2S"),
		("SD, scrolling the band down", b"\x1b[1T"),
		("IL, opening lines inside the band", b"\x1b[4;5H\x1b[2L"),
		("DL, closing lines inside the band", b"\x1b[4;5H\x1b[1M"),
		(
			"a line feed on the last row, which scrolls the band",
			b"\x1b[6;5H\n",
		),
		(
			"a reverse index on the first row, which scrolls it back",
			b"\x1b[1;5H\x1bM",
		),
		(
			"ICH, pushing cells right inside the band",
			b"\x1b[3;5H\x1b[3@",
		),
		(
			"DCH, pulling cells left inside the band",
			b"\x1b[3;5H\x1b[2P",
		),
		(
			"a vertical region as well, so both bands bound the scroll",
			b"\x1b[2;5r\x1b[3;5H\x1b[2S",
		),
		(
			"text long enough to wrap at the right margin",
			b"\x1b[3;5Hxyzwv",
		),
		(
			"IL with the cursor outside the band, which is refused",
			b"\x1b[3;1H\x1b[2L",
		),
		(
			"DL with the cursor past the right margin, also refused",
			b"\x1b[3;9H\x1b[2M",
		),
	]
}

#[test]
fn a_band_operation_leaves_the_columns_outside_it_alone() {
	// xterm's rule, and the one cmote's `scroll_band` and `shift_cells` are written to obey: the columns
	// outside the margins are another column of the page and are not part of the operation. Checked by
	// photographing the page, running one operation, and photographing it again — so the expected value
	// for every cell outside the band is the cell that was already there, which is a comparison rather
	// than a derivation even though the RULE is a reading.
	let (left, right) = (BAND.0 - 1, BAND.1 - 1);
	let mut disturbed = Vec::new();
	for (name, operation) in band_operations() {
		let mut terminal = banded_page();
		let before = shoot(&terminal.term);
		terminal.process(operation);
		let after = shoot(&terminal.term);
		for (row, (was, now)) in before.rows.iter().zip(after.rows.iter()).enumerate() {
			for column in (0..COLS as usize).filter(|column| *column < left || *column > right) {
				if was[column] != now[column] {
					disturbed.push(format!(
						"  {name}: row {row}, column {column} outside the band changed from {:?} to {:?}",
						was[column], now[column]
					));
				}
			}
		}
	}
	assert!(
		disturbed.is_empty(),
		"band operations reached outside the band in {} places:\n{}",
		disturbed.len(),
		disturbed.join("\n")
	);
}

#[test]
fn a_band_scroll_pushes_nothing_to_the_scrollback() {
	// The decision `scroll_band` states in prose and nothing checked: a row leaving the band is
	// DISCARDED rather than pushed to the history, because the history holds whole lines and this row is
	// a slice of one — the columns outside the band are not leaving. xterm does the same, and it is also
	// the only answer that keeps the scrollback readable, since half-lines interleaved with whole ones
	// would make every search, selection and copy wrong (§35, §40).
	//
	// So the depth after a band scroll must be the depth before it, and a `1` here would mean cmote had
	// started filing fragments in the history that the find bar would later show as lines.
	let mut filed = Vec::new();
	for (name, operation) in band_operations() {
		let mut terminal = banded_page();
		let before = shoot(&terminal.term).history;
		terminal.process(operation);
		let after = shoot(&terminal.term).history;
		if before != after {
			filed.push(format!(
				"  {name}: scrollback went from {before} to {after}"
			));
		}
	}
	assert!(
		filed.is_empty(),
		"band operations filed a slice of a line in the scrollback:\n{}",
		filed.join("\n")
	);
}

#[test]
fn margins_as_wide_as_the_page_are_not_margins() {
	// The same corpus, with the mode set and the band opened to the full width first. The oracle is fed
	// the identical bytes, including the request — a bare engine makes a save-cursor of `CSI 1;10s`
	// (parameters ignored) where cmote makes a margin request of it, which is §57's collision. That
	// difference is invisible here because nothing in the corpus restores the cursor, and it is the only
	// reason these streams can be handed to both sides unchanged.
	let mut disagreed = Vec::new();
	for (name, stream) in margin_free_streams() {
		let stream = [FULL_WIDTH_MARGINS, stream].concat();
		if let Some(where_) = difference(&gated(&stream), &ungated(&stream)) {
			disagreed.push(format!("  {name}: {where_}"));
		}
	}
	assert!(
		disagreed.is_empty(),
		"a band as wide as the page moved {} of {} streams onto cmote's own arithmetic:\n{}",
		disagreed.len(),
		margin_free_streams().len(),
		disagreed.join("\n")
	);
}

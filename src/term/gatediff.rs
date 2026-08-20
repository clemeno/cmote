// term/gatediff.rs — the gate compared with the engine it stands in front of (PLAN §107).
//
// `term/gate.rs` is the one place cmote sits BETWEEN the parser and the engine, and it re-implements a
// dozen of the engine's own `Handler` methods so that left and right margins can bound them: LF, RI, SU,
// SD, IL, DL, ICH, DCH, CUF, CUB, CR, HT, and the glyph path itself. Re-implementing a method the engine
// already has is the most dangerous thing in this crate, because the two versions are only ever compared
// by hand.
//
// This module compares them two ways, and the difference between the two is the thing to keep straight:
//
//   MEASURED, with no margins. Every gate arm opens `if !self.narrowed() { self.term.<the engine's own>();
//   return }`, so with no margins the gate is supposed to be a pass-through. That is checked against an
//   oracle that cannot be wrong about it — a SECOND engine, built by the same `new_engine` as cmote's and
//   fed the same bytes, with no gate in front of it at all. Nothing is derived; both answers are read off
//   a running terminal.
//
//   A READING, with margins on. DECSLRM is cmote's alone — the engine does not implement a left margin,
//   so there is no second implementation to ask. Those properties come from xterm's own definition (an
//   operation bounded by the margins moves the columns between them and nothing else; a row leaving the
//   band is discarded rather than filed in the history) and can be wrong in the way any reading can. They
//   are grouped separately below and labelled where they are declared.
//
// §106 built the same kind of harness one layer lower, driving `vte`'s parser beside cmote's scanners
// (`term/differential.rs`), and its own limitation was written down at the time: it compares the PARSER,
// not the handler. This is that gap. Where §106 asks "would the engine have framed this sequence?", this
// asks "would the engine have produced this grid?".
//
// WHAT THIS DOES NOT ASK. cmote acts alone on purpose in a great many places — selective erase (§56),
// rectangular areas (§58), pictures (§41), the soft reset's long spelling (§72) — and `Terminal::process`
// also SYNTHESISES sequences of its own. A stream carrying any of those would differ from a bare engine
// by design, and this harness would be wrong to call that a defect. So the oracle corpora are
// deliberately narrow: scrolling, cursor motion and plain text, the sequences the gate claims to forward.
// The gated side does run the other fifteen scanners on its way through `Terminal::process`, so a
// divergence is reported as the gate's when it could in principle be a scanner's — the corpus is chosen
// to keep that theoretical.
//
// Those paths are not left untested, though, which is §107's third Not done answered: they are compared
// CMOTE AGAINST CMOTE, once with margins never mentioned and once behind a band as wide as the page.
// Both runs make the same scanners do the same work, so the scanners cancel out and what is left of any
// difference is the gate's own `narrowed()` decision (`act_alone_streams`).
//
// THREE CORPORA, AND ONLY ONE OF THEM WAS WRITTEN BY HAND. §107 shipped fifteen streams chosen by
// reading the gate for what it forwards, and wrote down what that could not cover: an arm nobody thought
// of. `generated_streams` is the answer — every hand-written gate arm, at eight arrivals, under four
// scrolling regions — and `gate_arms` is checked against `gate.rs`'s own source, so an arm added there
// without a stream here is a failing test rather than a silent hole. It earns its keep: deleting
// `insert_blank`'s `!narrowed()` guard leaves the hand-picked corpus entirely green and fails 24 of the
// generated streams, because ICH was never in the fifteen (§111).

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::{Color, Processor};

use super::{Engine, Terminal, new_engine};

/// The page every corpus runs on. Small on purpose: six rows means three line feeds reach the bottom, so
/// a stream that has to scroll — the whole point of the comparison — fits in a readable literal.
const ROWS: u16 = 6;
/// Ten columns, so a wrap is two words away rather than eighty.
const COLS: u16 = 10;

/// The band the margins sweeps use, ONE-BASED as a program writes it on the wire: columns 5 to 7, so the
/// page has four columns to the left of the band and three to the right.
const BAND: (usize, usize) = (5, 7);

/// Whether a zero-based column lies outside the band — the columns whose stillness is the property.
///
/// The one place the wire's one-based numbering is turned into the grid's zero-based, so no caller has to
/// remember to subtract.
fn outside_the_band(column: usize) -> bool {
	column < BAND.0 - 1 || column > BAND.1 - 1
}

/// One cell as everything a caller could see of it: the character, the attribute FLAGS, and the two
/// colours.
///
/// Comparing only the character would let two whole classes of divergence pass. What a scroll blanks
/// WITH — the background the cursor's pen carries — is one. The other is `WRAPLINE`, the flag search,
/// selection and copy read to join a wrapped line back into one logical line (§35, §40), which the band
/// wrap deliberately does not set: a change that unstitched every wrapped line in the find bar would
/// leave every character exactly where it was. The flags are kept as `Flags` rather than as their bits so
/// that a failure prints `WRAPLINE` instead of a number to look up.
type GatediffCell = (char, Flags, Color, Color);

/// Everything observable about an engine after a stream: the cursor, the scrollback depth, and every cell
/// of the document — history rows included.
///
/// Scrollback matters here more than anywhere. A band scroll DISCARDS the row it pushes out while the
/// engine's own scroll PUSHES IT TO HISTORY, so a gate that performed a band scroll where it should have
/// forwarded would look perfect on screen and quietly lose every line that scrolled off.
struct Document {
	/// The cursor's row and column, in the engine's own signed line space.
	cursor: (i32, usize),
	/// How many lines sit above the visible page.
	history: usize,
	/// The document top-down, starting at the oldest history line.
	rows: Vec<Vec<GatediffCell>>,
}

/// Read an engine's whole document out.
fn photograph(engine: &Engine) -> Document {
	let grid = engine.grid();
	let history = grid.total_lines() - grid.screen_lines();
	let cursor = grid.cursor.point;
	let first = -super::as_line_number(history);
	let last = super::as_line_number(grid.screen_lines());
	let rows = (first..last)
		.map(|line| {
			(0..grid.columns())
				.map(|column| {
					let cell = &grid[Line(line)][Column(column)];
					(cell.c, cell.flags, cell.fg, cell.bg)
				})
				.collect()
		})
		.collect();
	Document {
		cursor: (cursor.line.0, cursor.column.0),
		history,
		rows,
	}
}

/// Feed a stream to cmote's terminal — parser, then gate, then engine — and read the result.
fn gated(bytes: &[u8]) -> Document {
	let mut terminal = Terminal::new(ROWS, COLS);
	terminal.process(bytes);
	photograph(&terminal.term)
}

/// Feed the same stream to a bare engine, built the same way, with no gate in front of it.
///
/// This is the oracle, and it is worth being precise about why it is one: nothing of cmote's is in this
/// path except `new_engine`, which is shared with `Terminal::new` so that the two cannot be configured
/// differently. Whatever comes out is what `alacritty_terminal` alone would have produced — measured, not
/// derived, transcribed or reasoned about.
fn ungated(bytes: &[u8]) -> Document {
	// The reply handle is bound rather than dropped: the listener the engine holds writes into it, and
	// letting it go would leave the engine talking to a buffer nothing owns.
	let (mut engine, _replies) = new_engine(ROWS, COLS);
	// Spelled out because `Processor`'s synchronised-update timeout is a type parameter with a default,
	// and the default is only chosen when something names it. `Terminal` names it the same way.
	let mut parser: Processor = Processor::new();
	parser.advance(&mut engine, bytes);
	photograph(&engine)
}

/// Where two documents first part company, in words a failure message can carry.
///
/// A whole-document diff would print sixty cells and bury the one that matters, so this reports the FIRST
/// disagreement and stops: the cursor, then the history depth, then the document in reading order.
fn difference(one: &Document, other: &Document) -> Option<String> {
	if one.cursor != other.cursor {
		return Some(format!(
			"cursor: {:?} against {:?}",
			one.cursor, other.cursor
		));
	}
	if one.history != other.history {
		return Some(format!(
			"scrollback depth: {} against {}",
			one.history, other.history
		));
	}
	for (index, (left, right)) in one.rows.iter().zip(other.rows.iter()).enumerate() {
		for (column, (cell, counterpart)) in left.iter().zip(right.iter()).enumerate() {
			if cell != counterpart {
				return Some(format!(
					"row {} (of {}), column {column}: {:?} against {:?}",
					index,
					one.rows.len(),
					cell,
					counterpart
				));
			}
		}
	}
	None
}

/// The streams the pass-through property is checked over — every one of them margin-free, so every gate
/// arm in them is meant to take its `!narrowed()` path and forward.
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
			// `ESC E` never reaches `Handler::newline` at all: the PARSER expands it into `linefeed()`
			// then `carriage_return()` (`vte-0.15.0/src/ansi.rs:1802-1805`), and nothing in either crate
			// dispatches `newline` outside alacritty's own tests. So this stream exercises the gate's
			// `linefeed` and `carriage_return` — which is the pair that matters, since the gate has
			// replaced both — while `Gate::newline` stays unreachable and exists only because
			// `#[deny(clippy::missing_trait_methods)]` requires every method to be written out.
			"NEL, which the parser expands into the two methods the gate replaces",
			b"\x1b[2;5r\x1b[5;4Habc\x1bE\x1bEq",
		),
	]
}

/// Mode 69 on, with the band opened to the two edges of the page — margins that wall off nothing.
///
/// `Margins::narrowed` is `enabled && (left > 0 || right + 1 < cols)`, so this is deliberately NOT
/// narrowed and the gate is supposed to keep standing aside. It is the same rule `ScrollRegion` has for a
/// vertical band as tall as the page, and it is worth a sweep of its own because the tempting
/// simplification — reading `narrowed()` as "is the mode set?" — would pass every margins test in
/// `mod.rs` while quietly moving every unmargined program onto cmote's arithmetic.
///
/// Built from `COLS` rather than written out, so that widening the page cannot turn this into a NARROW
/// band and leave the sweep below testing the opposite of its own name.
fn full_width_margins() -> Vec<u8> {
	format!("\x1b[?69h\x1b[1;{COLS}s").into_bytes()
}

/// Run the whole margin-free corpus against the oracle, optionally behind a prefix, and report every
/// stream that disagreed.
///
/// One helper for both sweeps because they differ in nothing but the prefix, and a failure that named
/// only the first disagreeing stream would hide how wide the damage was.
fn oracle_sweep(prefix: &[u8]) -> Vec<String> {
	let mut disagreed = Vec::new();
	for (name, stream) in margin_free_streams() {
		let stream = [prefix, stream].concat();
		if let Some(divergence) = difference(&gated(&stream), &ungated(&stream)) {
			disagreed.push(format!(
				"  {name}: {divergence} (gated against engine alone)"
			));
		}
	}
	disagreed
}

/// A page with three lines already scrolled into the history, every visible row filled with its own
/// letter, a narrow band walled off, and the cursor parked inside the band.
///
/// Three details, each load-bearing:
///
///   The HISTORY is filled first, so "a band scroll files nothing in the scrollback" can compare the
///   history's CONTENTS and not merely its depth. On a page with no history behind it, a rule about not
///   disturbing the history is satisfied by there being nothing to disturb.
///
///   The visible rows are drawn BEFORE the margins go on, which is what a real program does and the only
///   order that puts known text OUTSIDE the band — the columns whose stillness is the property.
///
///   The cursor is parked INSIDE the band last, so the operations that carry no cursor motion of their
///   own (SU, SD) start from a known place and their expected cursor below is a fact rather than a guess.
fn banded_page() -> Terminal {
	let mut terminal = Terminal::new(ROWS, COLS);
	terminal.process(b"old one\r\nold two\r\nold three\r\n");
	for row in 0..ROWS {
		let letter = char::from(b'A' + u8::try_from(row).expect("the test's own row count"));
		let text: String = std::iter::repeat_n(letter, COLS as usize).collect();
		terminal.process(format!("\x1b[{};1H{text}", row + 1).as_bytes());
	}
	let (left, right) = BAND;
	terminal.process(format!("\x1b[?69h\x1b[{left};{right}s\x1b[3;{left}H").as_bytes());
	terminal
}

/// One operation the margins bound, and where the cursor is supposed to end up after it.
struct BandOp {
	/// What the operation is, for the failure message.
	name: &'static str,
	/// The bytes, including any cursor motion the operation needs first.
	bytes: &'static [u8],
	/// The cursor afterwards, zero-based, hand-derived — see [`band_operations`] for why by hand.
	cursor: (i32, usize),
}

/// The band operations the confinement sweeps run, each starting with the cursor inside the band.
///
/// NOT ORACLE-BACKED, which is why this list is separate from the streams above. A left or right margin
/// is a thing the engine does not implement, so there is no second implementation to measure against and
/// these expectations are a reading of xterm's definition plus arithmetic on this page. Where the sweeps
/// above are measurements, these can be wrong in the way any reading can.
///
/// The cursor column is the part worth checking rather than assuming: every operation here starts inside
/// the band and none of them is a cursor-motion sequence, so an operation that deposited the cursor
/// outside the margins would be one that had escaped them. The wrapping row is the interesting one — five
/// glyphs into a three-column band fills columns 4, 5 and 6, breaks to the LEFT MARGIN one row down, and
/// leaves the cursor holding on the right margin with the next wrap owed.
fn band_operations() -> Vec<BandOp> {
	vec![
		BandOp {
			name: "SU, scrolling the band up",
			bytes: b"\x1b[2S",
			cursor: (2, 4),
		},
		BandOp {
			name: "SD, scrolling the band down",
			bytes: b"\x1b[1T",
			cursor: (2, 4),
		},
		BandOp {
			name: "IL, opening lines inside the band",
			bytes: b"\x1b[4;5H\x1b[2L",
			cursor: (3, 4),
		},
		BandOp {
			name: "DL, closing lines inside the band",
			bytes: b"\x1b[4;5H\x1b[1M",
			cursor: (3, 4),
		},
		BandOp {
			name: "a line feed on the last row, which scrolls the band",
			bytes: b"\x1b[6;5H\n",
			cursor: (5, 4),
		},
		BandOp {
			name: "a reverse index on the first row, which scrolls it back",
			bytes: b"\x1b[1;5H\x1bM",
			cursor: (0, 4),
		},
		BandOp {
			name: "ICH, pushing cells right inside the band",
			bytes: b"\x1b[3;5H\x1b[3@",
			cursor: (2, 4),
		},
		BandOp {
			name: "DCH, pulling cells left inside the band",
			bytes: b"\x1b[3;5H\x1b[2P",
			cursor: (2, 4),
		},
		BandOp {
			name: "a vertical region as well, so both bands bound the scroll",
			bytes: b"\x1b[2;5r\x1b[3;5H\x1b[2S",
			cursor: (2, 4),
		},
		BandOp {
			name: "text long enough to wrap at the right margin",
			bytes: b"\x1b[3;5Hxyzwv",
			cursor: (3, 6),
		},
	]
}

/// An operation the gate REFUSES because the cursor is outside the band, split so the refusal can be
/// photographed on its own.
struct GatediffRefused {
	/// What is being refused, for the failure message.
	name: &'static str,
	/// Where the cursor goes first. Run before the photograph, since moving the cursor is allowed.
	park: &'static [u8],
	/// The operation that must then do nothing whatsoever.
	bytes: &'static [u8],
}

/// The four operations `cursor_in_band` and `shift_cells` refuse outright.
///
/// A refusal needs its own sweep, and finding that out was the review's sharpest catch: these entries
/// used to sit in the list above, where the assertion looked only at the columns OUTSIDE the band — which
/// a refusal satisfies for free, and so does an operation that wrongly went ahead inside the band. Both
/// `cursor_in_band` guards could be deleted with every test still green. Nothing is exempt from having to
/// be able to fail, least of all a test of a guard.
fn refused_operations() -> Vec<GatediffRefused> {
	vec![
		GatediffRefused {
			name: "IL with the cursor left of the band",
			park: b"\x1b[3;1H",
			bytes: b"\x1b[2L",
		},
		GatediffRefused {
			name: "DL with the cursor past the right margin",
			park: b"\x1b[3;9H",
			bytes: b"\x1b[2M",
		},
		GatediffRefused {
			name: "ICH with the cursor left of the band",
			park: b"\x1b[3;1H",
			bytes: b"\x1b[3@",
		},
		GatediffRefused {
			name: "DCH with the cursor past the right margin",
			park: b"\x1b[3;9H",
			bytes: b"\x1b[2P",
		},
	]
}

/// One `Handler` method the gate re-implements by hand, paired with a stream that reaches it — or with
/// an EMPTY slice when the parser never dispatches it at all.
///
/// The hand-written arms are the whole risk this module exists for: the ones the `forward!` macro writes
/// cannot be wrong about anything, while each of these is a second implementation of a method the engine
/// already has. The name is spelled exactly as it is in `gate.rs`, which is what lets
/// [`hand_written_arms`] check the list against the source and fail when an arm is added without one.
type GateArm = (&'static str, &'static [u8]);

/// Every arm of the gate's `Handler` impl, with the sequence that reaches it (§107, §111).
///
/// This is the list the generated sweep walks, and its completeness is CHECKED rather than trusted: a
/// new arm in `gate.rs` that nobody adds here fails `every_gate_arm_is_in_the_sweep`. That was §107's
/// own Not done — "the `!narrowed()` guard is checked, not enforced; a new arm added without it fails
/// these tests only if a stream in the corpus reaches it" — and this is the half of it that can be
/// enforced without a second parser.
///
/// Two families of sequence are deliberately absent from the BYTES, though their arms are here:
/// anything cmote answers by synthesising sequences of its own (DECSTR's long spelling §72, DECST8C
/// §74, XTPOPSGR §85) and anything a scanner acts on alone (§56, §58, §41). Those differ from a bare
/// engine by design, so they belong to `an_act_alone_path_is_the_same_with_and_without_the_gate_narrowed`
/// below, which compares cmote against cmote instead.
fn gate_arms() -> Vec<GateArm> {
	vec![
		("set_scrolling_region", b"\x1b[2;5r"),
		("reset_state", b"\x1bc"),
		("input", b"XY"),
		("goto", b"\x1b[2;3H"),
		("goto_line", b"\x1b[4d"),
		("goto_col", b"\x1b[7G"),
		("insert_blank", b"\x1b[3@"),
		("delete_chars", b"\x1b[3P"),
		("move_forward", b"\x1b[4C"),
		("move_backward", b"\x1b[4D"),
		("move_down_and_cr", b"\x1b[2E"),
		("move_up_and_cr", b"\x1b[2F"),
		("put_tab", b"\t\t"),
		("backspace", b"\x08\x08"),
		("carriage_return", b"\r"),
		("linefeed", b"\n\n"),
		// The parser expands `ESC E` into `linefeed()` then `carriage_return()`
		// (`vte-0.15.0/src/ansi.rs:1802-1805`) and nothing in either crate dispatches `newline` outside
		// alacritty's own tests, so no stream can reach this arm. It exists because
		// `#[deny(clippy::missing_trait_methods)]` requires every method to be written out.
		("newline", b""),
		("reverse_index", b"\x1bM"),
		("scroll_up", b"\x1b[2S"),
		("scroll_down", b"\x1b[2T"),
		("insert_blank_lines", b"\x1b[2L"),
		("delete_lines", b"\x1b[2M"),
		// `ESC 7` and `ESC 8` rather than `CSI s` / `CSI u`: with mode 69 set, `CSI s` is DECSLRM and
		// `term/cancel.rs` feeds the engine a CAN in place of its final byte (§57), which is cmote acting
		// alone and would diverge from the oracle by design. The restore SAVES first, so the stream is
		// self-contained — a bare restore behind the full-width band would have the two sides restoring
		// different slots, since the band request itself is a save-cursor to the bare engine.
		("save_cursor_position", b"\x1b7"),
		("restore_cursor_position", b"\x1b7\x1b8"),
		("set_private_mode", b"\x1b[?69h"),
		("unset_private_mode", b"\x1b[?69l"),
		("report_private_mode", b"\x1b[?69$p"),
	]
}

/// The names of the methods `gate.rs` implements BY HAND, read out of the source.
///
/// Reading the source is unusual and it is the point: the alternative is a list in this file that
/// somebody keeps up to date, which is exactly the arrangement §107 wrote down as not good enough. The
/// slice runs from the `impl Handler` line to the `forward!` invocation, so the generated arms are
/// excluded — they cannot be wrong in the way a hand-written one can.
fn hand_written_arms() -> Vec<String> {
	const SOURCE: &str = include_str!("gate.rs");
	const OPENS: &str = "impl Handler for Gate<'_> {";
	const CLOSES: &str = "\n\tforward! {";

	let start = SOURCE
		.find(OPENS)
		.expect("gate.rs still implements Handler for Gate");
	let body = &SOURCE[start..];
	let end = body
		.find(CLOSES)
		.expect("gate.rs still hands the rest to the forward! macro");
	body[..end]
		.lines()
		.filter_map(|line| line.strip_prefix("\tfn "))
		.filter_map(|rest| rest.split('(').next())
		.map(str::to_owned)
		.collect()
}

/// Every stream the generated sweep runs, each one margin-free in the sense the pass-through property
/// needs: `Margins::narrowed` is false throughout, so every arm reached is meant to forward.
///
/// The generator §107 asked for, and the axes are the ones that make the code look wrong rather than the
/// ones that are easy to vary. Each stream is a filled history and a filled page (so an operation moves
/// KNOWN text — shuffling blank cells around is indistinguishable from doing nothing), then a scrolling
/// region, then a cursor position, then one operation.
fn generated_streams() -> Vec<(String, Vec<u8>)> {
	/// Where the cursor is parked before the operation, one-based as a program writes it. The edges are
	/// where an off-by-one in the gate's own arithmetic shows up, so all four corners are here along
	/// with a position in the middle.
	const POSITIONS: [(u16, u16); 5] = [(1, 1), (1, COLS), (ROWS, 1), (ROWS, COLS), (3, 5)];

	/// The rows a stream reaches the last column of by WRITING to it, rather than by asking to be put
	/// there — which leaves a wrap OWED, the state cmote keeps its own flag for because the engine
	/// fires its own at the screen edge instead of at the right margin.
	///
	/// This axis is here because a mutation got past the sweep without it: deleting `backspace`'s
	/// `!narrowed()` guard changed nothing on any stream, since every one of them arrived at its
	/// position by CUP, which clears the pending wrap. Cursor motion is not the only way to arrive
	/// somewhere, and the difference between the two ways is exactly what one gate arm exists for
	/// (§107, §111).
	const WRAPPED_ROWS: [u16; 3] = [1, 3, ROWS];

	/// What every stream ends with: one glyph, to make the operation's leftover state visible. A cursor
	/// one column out or a wrap still owed is not in the document until something has to be drawn.
	const TAIL: &[u8] = b"Z";

	let regions: [(&str, Vec<u8>); 4] = [
		("no region", Vec::new()),
		("a region inside the page", b"\x1b[2;5r".to_vec()),
		// The one the engine stores with its top above the first row, mirrored rather than clamped.
		("a region whose top is zero", b"\x1b[0;3r".to_vec()),
		(
			"a region as tall as the page",
			format!("\x1b[1;{ROWS}r").into_bytes(),
		),
	];

	// The page every generated stream starts from: three lines pushed into the history, then each
	// visible row filled with its own letter. Built once and reused, since it is the same prefix every
	// time.
	let mut page = b"old one\r\nold two\r\nold three\r\n".to_vec();
	for row in 0..ROWS {
		let letter = char::from(b'A' + u8::try_from(row).expect("the test's own row count"));
		let text: String = std::iter::repeat_n(letter, COLS as usize).collect();
		page.extend_from_slice(format!("\x1b[{};1H{text}", row + 1).as_bytes());
	}

	// How the cursor gets where it is before the operation runs. Both kinds land somewhere known; only
	// the second leaves a wrap owed.
	let mut arrivals: Vec<(String, Vec<u8>)> = POSITIONS
		.iter()
		.map(|&(row, col)| {
			(
				format!("parked at {row};{col}"),
				format!("\x1b[{row};{col}H").into_bytes(),
			)
		})
		.collect();
	for row in WRAPPED_ROWS {
		let filled: String = std::iter::repeat_n('w', COLS as usize).collect();
		arrivals.push((
			format!("written to the end of row {row}, wrap owed"),
			format!("\x1b[{row};1H{filled}").into_bytes(),
		));
	}

	let mut out = Vec::new();
	for (method, operation) in gate_arms() {
		if operation.is_empty() {
			continue;
		}
		for (region_name, region) in &regions {
			for (arrival_name, arrival) in &arrivals {
				let mut bytes = page.clone();
				bytes.extend_from_slice(region);
				bytes.extend_from_slice(arrival);
				bytes.extend_from_slice(operation);
				// A glyph AFTER the operation, so that state the operation left behind but did not
				// display becomes something the comparison can see. Without it the sweep missed a real
				// mutation: deleting `backspace`'s `!narrowed()` guard leaves the pending wrap owed
				// where the engine's own backspace clears it, and nothing shows until the next
				// character has to decide which cell it goes in (§107, §111).
				bytes.extend_from_slice(TAIL);
				out.push((
					format!("{method}, {arrival_name}, with {region_name}"),
					bytes,
				));
			}
		}
	}
	out
}

/// The streams whose whole point is that cmote does NOT match a bare engine: a scanner acts beside it,
/// or `Terminal::process` synthesises sequences of its own.
///
/// §107's third Not done — "`Terminal::process` is compared, so cmote's synthesised sequences are out of
/// scope … which means it says nothing about those paths". They can still be held to something, and it
/// is the property that matters here: whether the GATE changes what happens on them. Comparing cmote
/// with cmote — once with no margins mentioned, once behind a band as wide as the page — cancels every
/// scanner out, because both runs make the same scanner do the same thing. What is left of the
/// difference is the gate's own `narrowed()` decision, which must be "stand aside" in both.
fn act_alone_streams() -> Vec<(&'static str, &'static [u8])> {
	vec![
		(
			"a selective erase, which the engine drops and `protect` performs (§56)",
			b"\x1b[1\"qabc\x1b[\"qdef\x1b[2;1H\x1b[?1J",
		),
		(
			"a rectangular erase, which is `rect`'s alone (§58)",
			b"\x1b[3;1Hfilled\x1b[1;2;3;5$z",
		),
		(
			"a rectangular fill and a change of attributes over an area (§58, §59)",
			b"\x1b[2*x\x1b[65;1;1;3;4$x\x1b[1;1;3;4;7$r",
		),
		(
			"a sixel picture, whose cells `Terminal::process` reserves itself (§41)",
			b"\x1bPq#0;2;100;0;0~\x1b\\tail",
		),
		(
			"a soft reset, whose long spelling cmote synthesises (§72)",
			b"\x1b[2;5r\x1b[3;3Hxy\x1b[!p",
		),
		(
			"a tab-stop rebuild, which cmote spells out for the engine (§74)",
			b"\t\ta\x1b[?5W\tb",
		),
		(
			"an SGR push and pop, where the restored pen is synthesised (§85)",
			b"\x1b[31m\x1b[#{\x1b[32mgreen\x1b[#}red",
		),
		(
			"a hard reset, which clears state on both sides of the gate (§102)",
			b"\x1b[2;5r\x1b[3;3Hxy\x1bcafter",
		),
	]
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_gate_arm_is_in_the_sweep() {
		// §107's Not done, closed as far as it can be without a second parser: the coverage list is
		// checked against `gate.rs` itself, so an arm added there without a stream here is a failing
		// test rather than a silent hole. Both directions are checked — a listed arm that no longer
		// exists is just as wrong, because it would leave the sweep quietly testing nothing.
		let mut implemented = hand_written_arms();
		let mut covered: Vec<String> = gate_arms()
			.iter()
			.map(|&(method, _)| method.to_owned())
			.collect();
		implemented.sort();
		covered.sort();
		assert_eq!(
			implemented, covered,
			"the gate's hand-written arms and the sweep's list have parted company"
		);
		// And the list is not trivially empty, which is the way a source-reading check fails quietly.
		assert!(
			implemented.len() > 20,
			"only {} arms found in gate.rs — has the impl been renamed?",
			implemented.len()
		);
	}

	#[test]
	fn with_no_margins_the_gate_is_the_engine_for_every_arm() {
		// The generated sweep §107 asked for: every hand-written gate arm, at five cursor positions,
		// under four scrolling regions, against the second engine. Where the hand-picked corpus below
		// asks "do the streams I thought of still work", this asks "is there an arm nobody thought of",
		// which is the question that found §106's four defects.
		let streams = generated_streams();
		assert_eq!(
			streams.len(),
			26 * 4 * 8,
			"26 reachable arms x 4 regions x 8 arrivals (5 parked, 3 with a wrap owed)"
		);
		let mut disagreed = Vec::new();
		for (name, stream) in &streams {
			if let Some(divergence) = difference(&gated(stream), &ungated(stream)) {
				disagreed.push(format!("  {name}: {divergence}"));
			}
		}
		assert!(
			disagreed.is_empty(),
			"the gate parted company with the engine on {} of {} generated streams:\n{}",
			disagreed.len(),
			streams.len(),
			disagreed.join("\n")
		);
	}

	#[test]
	fn a_full_width_band_leaves_every_arm_with_the_engine() {
		// The generated sweep behind a band as wide as the page — mode 69 SET, margins at the page
		// edges — which is the combination that catches the tempting simplification: reading
		// `narrowed()` as "is the mode set?" instead of "does the band exclude a column?". The
		// hand-picked corpus catches that too, and §107 measured that it does so on only five of its
		// fifteen streams; this asks it of every gate arm at every position.
		let prefix = full_width_margins();
		let streams = generated_streams();
		let mut disagreed = Vec::new();
		for (name, stream) in &streams {
			let stream = [prefix.as_slice(), stream].concat();
			if let Some(divergence) = difference(&gated(&stream), &ungated(&stream)) {
				disagreed.push(format!("  {name}: {divergence}"));
			}
		}
		assert!(
			disagreed.is_empty(),
			"a band as wide as the page moved {} of {} generated streams onto cmote's arithmetic:\n{}",
			disagreed.len(),
			streams.len(),
			disagreed.join("\n")
		);
	}

	#[test]
	fn an_act_alone_path_is_the_same_with_and_without_the_gate_narrowed() {
		// The third of §107's gaps. These streams cannot be compared against a bare engine — cmote acts
		// alone on every one of them, on purpose — so they are compared against CMOTE, run twice: once
		// with margins never mentioned and once behind a band as wide as the page. Both runs make the
		// same scanners do the same work, so the scanners cancel and what is left is the gate's own
		// decision about whether it is narrowed. `Margins::narrowed` is false in both, so the two
		// documents have to be identical.
		//
		// What this catches that nothing else does: a gate arm that keys off "is the mode set?" rather
		// than "is the band narrower than the page?" on one of the paths the oracle sweeps cannot reach.
		let mut disagreed = Vec::new();
		for (name, stream) in act_alone_streams() {
			let plain = gated(stream);
			let behind_margins = gated(&[&full_width_margins(), stream].concat());
			if let Some(divergence) = difference(&plain, &behind_margins) {
				disagreed.push(format!(
					"  {name}: {divergence} (plain against full-width band)"
				));
			}
		}
		assert!(
			disagreed.is_empty(),
			"a full-width band changed {} of the paths cmote acts alone on:\n{}",
			disagreed.len(),
			disagreed.join("\n")
		);
	}

	#[test]
	fn with_no_margins_the_gate_is_the_engine() {
		// The property the whole margins design rests on: cmote's own arithmetic must not run at all until
		// a program asks for margins. Every stream is margin-free, so every gate arm it reaches is meant to
		// hand the bytes straight to the engine — and a single arm that forgot its `!narrowed()` guard shows
		// up as a different grid, a different cursor or a lost scrollback line.
		let disagreed = oracle_sweep(b"");
		assert!(
			disagreed.is_empty(),
			"the gate parted company with the engine on {} of {} margin-free streams:\n{}",
			disagreed.len(),
			margin_free_streams().len(),
			disagreed.join("\n")
		);
	}

	#[test]
	fn margins_as_wide_as_the_page_are_not_margins() {
		// The same corpus behind a full-width margin request. Both sides are fed the identical bytes,
		// request included: the engine's arm for the final `s` is `('s', [])` — an empty INTERMEDIATES
		// list, not an empty parameter list — so a bare engine takes `CSI 1;10s` for a save-cursor where
		// cmote takes it for a margin request, which is §57's collision. That difference stays invisible
		// because nothing in the corpus restores the cursor, and it is the only reason these streams can go
		// to both sides unchanged.
		let disagreed = oracle_sweep(&full_width_margins());
		assert!(
			disagreed.is_empty(),
			"a band as wide as the page moved {} of {} streams onto cmote's own arithmetic:\n{}",
			disagreed.len(),
			margin_free_streams().len(),
			disagreed.join("\n")
		);
	}

	#[test]
	fn a_band_operation_leaves_everything_outside_the_band_alone() {
		// xterm's rule, and the one `scroll_band` and `shift_cells` are written to obey: the columns
		// outside the margins are another column of the page and are not part of the operation. Checked by
		// photographing the page, running one operation, and photographing it again — so while the RULE is
		// a reading, the expected value for every cell outside the band is the cell that was already there.
		//
		// The history rows are photographed too, and a change in the DEPTH is reported here rather than
		// silently ending the row-by-row comparison early.
		let mut disturbed = Vec::new();
		for operation in band_operations() {
			let mut terminal = banded_page();
			let before = photograph(&terminal.term);
			terminal.process(operation.bytes);
			let after = photograph(&terminal.term);
			if before.history != after.history {
				disturbed.push(format!(
					"  {}: scrollback depth went from {} to {}",
					operation.name, before.history, after.history
				));
			}
			for (row, (was, now)) in before.rows.iter().zip(after.rows.iter()).enumerate() {
				for column in (0..COLS as usize).filter(|column| outside_the_band(*column)) {
					if was[column] != now[column] {
						disturbed.push(format!(
							"  {}: row {row}, column {column} outside the band changed from {:?} to {:?}",
							operation.name, was[column], now[column]
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
	fn a_band_operation_leaves_the_cursor_inside_the_band() {
		// The other half of "bounded by the margins": an operation that deposited the cursor outside them
		// would be one that had escaped them, and every operation here starts inside the band and carries
		// no cursor motion of its own beyond the row it names. The exact expected position is arithmetic on
		// this page rather than a measurement — see `band_operations` — and the wrapping row is the one
		// that pins a rule of cmote's own: a line breaks at the right margin and goes on at the LEFT one,
		// with the cursor left holding on the right margin and the next wrap owed.
		let mut wrong = Vec::new();
		for operation in band_operations() {
			let mut terminal = banded_page();
			terminal.process(operation.bytes);
			let (row, column) = photograph(&terminal.term).cursor;
			if (row, column) != operation.cursor {
				wrong.push(format!(
					"  {}: cursor at {:?}, expected {:?}",
					operation.name,
					(row, column),
					operation.cursor
				));
			}
			if outside_the_band(column) {
				wrong.push(format!(
					"  {}: cursor left the band, at column {column}",
					operation.name
				));
			}
		}
		assert!(
			wrong.is_empty(),
			"band operations put the cursor somewhere else in {} places:\n{}",
			wrong.len(),
			wrong.join("\n")
		);
	}

	#[test]
	fn a_band_scroll_pushes_nothing_to_the_scrollback() {
		// The decision `scroll_band` states in prose and nothing checked: a row leaving the band is
		// DISCARDED rather than pushed to the history, because the history holds whole lines and this row
		// is a slice of one — the columns outside the band are not leaving. xterm does the same, and it is
		// also the only answer that keeps the scrollback readable, since half-lines interleaved with whole
		// ones would make every search, selection and copy wrong (§35, §40).
		//
		// Both the depth AND the contents of the history are compared, which is why `banded_page` scrolls
		// three lines into it first: on a page with an empty history, "the history was not disturbed" is a
		// property with nothing to disturb.
		let mut filed = Vec::new();
		for operation in band_operations() {
			let mut terminal = banded_page();
			let before = photograph(&terminal.term);
			terminal.process(operation.bytes);
			let after = photograph(&terminal.term);
			if before.history != after.history {
				filed.push(format!(
					"  {}: scrollback went from {} to {} lines deep",
					operation.name, before.history, after.history
				));
				continue;
			}
			for row in 0..before.history {
				if before.rows[row] != after.rows[row] {
					filed.push(format!(
						"  {}: history line {row} was rewritten",
						operation.name
					));
				}
			}
		}
		assert!(
			filed.is_empty(),
			"band operations disturbed the scrollback:\n{}",
			filed.join("\n")
		);
	}

	#[test]
	fn an_operation_refused_outside_the_band_changes_nothing_at_all() {
		// A refusal is the one case where the whole document — cells, cursor, history, inside the band and
		// out — must come back identical, so this is the sweep that can see a guard disappear. `IL` and
		// `DL` are refused by `cursor_in_band`, `ICH` and `DCH` by `shift_cells`'s own test, and all four
		// for the same reason: there is no band to open, close or shift from out there, and guessing one
		// would move text the program had walled off.
		let mut acted = Vec::new();
		for operation in refused_operations() {
			let mut terminal = banded_page();
			terminal.process(operation.park);
			let before = photograph(&terminal.term);
			terminal.process(operation.bytes);
			let after = photograph(&terminal.term);
			if let Some(divergence) = difference(&before, &after) {
				acted.push(format!(
					"  {}: {divergence} (before against after)",
					operation.name
				));
			}
		}
		assert!(
			acted.is_empty(),
			"operations that should have been refused did something:\n{}",
			acted.join("\n")
		);
	}
}

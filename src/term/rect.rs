// term/rect.rs — the VT420 rectangular area operations (PLAN §58, §59, §60).
//
// A VT420 could act on a BOX of the screen rather than a run of it. Four sequences change what the
// cells HOLD, all sharing the `$` intermediate and all giving their corners as `Pt;Pl;Pb;Pr` — top,
// left, bottom, right, 1-based and inclusive (§58):
//
//   CSI Pt;Pl;Pb;Pr $ z                          DECERA  — erase the rectangle
//   CSI Pt;Pl;Pb;Pr $ {                          DECSERA — erase it, leaving protected cells (§56)
//   CSI Pch;Pt;Pl;Pb;Pr $ x                      DECFRA  — fill it with one character
//   CSI Pts;Pls;Pbs;Prs;Pps;Ptd;Pld;Ppd $ v      DECCRA  — copy it somewhere else
//
// Three more change what they LOOK like, leaving every character where it stands (§59):
//
//   CSI Pt;Pl;Pb;Pr;Ps… $ r                      DECCARA — turn attributes on and off across an area
//   CSI Pt;Pl;Pb;Pr;Ps… $ t                      DECRARA — flip them
//   CSI Ps * x                                   DECSACE — pick which SHAPE those two act on
//
// And one ASKS about a box instead of changing it — the only sequence in the family that writes
// bytes back down the pty (§60):
//
//   CSI Pid;Pp;Pt;Pl;Pb;Pr * y                   DECRQCRA — report a checksum of the rectangle
//
// TWO MORE JOINED IN §100, and they are not rectangles at all:
//
//   CSI Ps SP @                                  SL — shift the page left Ps columns
//   CSI Ps SP A                                  SR — shift it right Ps columns
//
// AND A THIRD IN §101, which is the only one here that moves the boundary between the page and the
// scrollback:
//
//   CSI Ps + T                                   UNSCROLL — scroll down, filling from the scrollback
//
// They are ECMA-48's rather than DEC's and they name no corners at all. They live here because what
// a module shares with its neighbours is not its grammar but its MECHANISM: SL and SR move whole
// cells across the page with no engine arm behind them, against the same background the erases
// write, under the same origin-mode refusal, clamped to the same visible page. A second module
// doing cell-moving by its own rules is the duplication worth avoiding; one more pair of arms in
// this scanner is not. The module's subject is therefore "operations that move cells the engine
// will not", and the file name is one section out of date.
//
// They were the block operations of a forms terminal: clear a field, rule a line of `-` across a box,
// scroll a sub-window by copying it up a row, underline a whole column of entry fields in one write.
// A modern full-screen program repaints instead, so almost nothing emits these — but they are cheap
// here, because everything they need already exists. §56 had to solve the hard half: writing cells
// directly into the engine's grid, and knowing which of them a program marked as protected.
//
// `vte` has no arm for any of the ten. Its CSI dispatch matches `$` only in `('p', [b'$'])` and
// `('p', [b'?', b'$'])` — the two DECRQM spellings — matches `*` in no CSI at all, and matches the
// SPACE intermediate in exactly two places, `('k', [b' '])` and `('q', [b' '])`, which are SCP and
// DECSCUSR. So every one of them falls through to the unhandled arm and is dropped whole. That makes
// them cmote's, the same way DECSCA and the `?` erases were.
//
// This module is the grammar and the arithmetic, and nothing else: it deals in plain row and column
// numbers, in a four-bit attribute mask of its own, and in the running total of a checksum, so every
// corner case of clamping, defaulting, overlap, folding and trimming is tested without building a
// terminal. `term/mod.rs` does the cell reading and writing, as it does for the selective erase, and
// owns the two translations from this module's numbers to the engine's flag names.
//
// Seven rules are worth stating where they are decided rather than where they are executed.
//
// PROTECTION. Only DECSERA respects it. DECERA, DECFRA and DECCRA go straight through a protected
// cell, exactly as the plain `CSI J` does and the `?` one does not (§56) — two verbs, the plain one
// stronger. A cell COPIED by DECCRA carries its protection with it, which comes for free from copying
// the whole cell, and is right: DECSCA marks a cell unerasable, not immovable. DECCARA and DECRARA
// ignore protection outright, and must: DECSCA marks a cell unerasable, and neither of these erases.
//
// DECFRA's CHARACTER is an allow-list. `Pch` is a character code, and only 32–126 and 160–255 are
// accepted — printable ASCII and printable Latin-1, the same two ranges xterm allows. Everything else
// drops the whole sequence. A remote must not be able to paint the page with C0 or C1 controls, with
// DEL, or with unassigned code points, and "fill four hundred cells with U+0000" is not a request
// worth honouring on the way to finding out what the renderer does with it (§12).
//
// DECCRA's SOURCE IS READ FIRST. The two rectangles may overlap — scrolling a sub-window by one row is
// the whole point of the sequence, and that is the maximally overlapping case — so the copy is defined
// as if it went through a buffer, and cmote does exactly that rather than choosing a clever direction
// to walk in. Its two page parameters (`Pps`, `Ppd`) are ignored: cmote has one page, which is what
// clamping a page number to the number of pages the terminal has amounts to.
//
// DECSACE IS A MODE, so it is absorbed here rather than reported. The scanner reads the stream in
// order, which is the only place the ordering between a DECSACE and the DECCARA after it is free, so
// each attribute request leaves this module carrying the extent that was in force when it arrived —
// and `term/mod.rs` never has to hold the mode or reason about when it changed. It resets on RIS,
// which resets everything; DECSTR does NOT reset it, because DEC's published DECSTR list does not
// name it, and inventing a reset is the same kind of guess as inventing a rectangle.
//
// THE ATTRIBUTE PAIR TOUCHES ATTRIBUTES ONLY. `Ps` here is a small DEC-defined subset of SGR — bold,
// underline, blink, reverse, and `0` for all of them — never colours, never a glyph, and never the
// flag word wholesale. That last one is not pedantry: cmote's DECSCA protection rides bit 15 of the
// engine's flag word (§56), so assigning the word would silently unprotect a form the moment a
// program underlined it. Only the named bits move, one at a time.
//
// THE CHECKSUM IS COPIED, NOT INVENTED (§60). A number nobody else computes the same way is worth
// less than no number at all: a conformance suite compares the four digits it got against the four a
// real terminal gave, so a checksum that is merely plausible fails exactly as loudly as a missing one
// and costs the work as well. cmote's is xterm's `xtermCheckRect` with no extension bits set — the
// `CSI 0 # y` default, which is the mode xterm tuned against screenshots from a real VT520, so it is
// DEC's answer arrived at by way of the one implementation everybody tests against:
//
//   * a cell contributes its character code, plus 0x04 if it is DECSCA protected, 0x08 if hidden,
//     0x10 underlined, 0x20 reverse, 0x40 blinking, 0x80 bold;
//   * a cell whose total comes out as exactly 0x20 — a plain space with nothing added on top — is
//     dropped, EXCEPT the first cell of the rectangle, which always counts;
//   * the running total is taken modulo 2^16, negated, and reported as four upper-case hex digits,
//     which is why a page of ordinary text reports a number just under 0x10000.
//
// Three parts of that cmote cannot match, and names rather than papers over. BLINK has no bit in the
// engine's flag word (§59), so 0x40 never lands — the same hole, in the same place, for the same
// reason. A cell written through a DEC character-set designation (`ESC ( 0`, then `q` for a
// box-drawing rule) reaches the grid already translated to Unicode, so cmote weighs U+2500 where
// xterm weighs the `q` it remembers seeing. And xterm knows which cells a program has actually
// WRITTEN, where the engine's grid starts out full of blanks that read identically — so a rectangle
// whose first cell has never been written reports 0xFFE0, one trimmed space, where xterm reports
// 0x0000. Every rectangle that begins on a written cell agrees to the digit, which is every
// rectangle a suite checksums after painting one.
//
// THE CHECKSUM IS A READ, AND THAT IS THE WHOLE OBJECTION TO IT. Ask about a one-cell rectangle and
// the reply is `-(character + attributes)`, which inverts in a single subtraction: a program can walk
// the page a cell at a time and recover every character on it. A hostile file `cat`ed into the
// terminal can read back what the commands before it left on screen. That is real, and it is why this
// gets weighed against the same line every other read-back has been (§12) — and comes down on the
// other side of it, for one reason. Every byte on that page arrived from the pty this reply goes back
// down: the remote wrote it, or the remote's own echo did. Contrast OSC 52's read form, refused
// outright, because the LOCAL clipboard holds what the user's other applications put there and the
// remote has never seen any of it. A screen readback crosses no boundary cmote is standing on; a
// clipboard readback crosses the only one that matters.
//
// Two properties keep it on that side, and both are enforced rather than assumed. The rectangle
// resolves against the VISIBLE PAGE, so the scrollback is out of reach — `from_corners` clamps to the page,
// and there is no spelling of a corner that reaches a retired line. And the answer is a function of
// grid cells and nothing else: not the window size, not the title, not the working directory, not the
// clock, nothing about cmote or the machine it runs on. It repeats what the remote already said, and
// only that.

use std::ops::RangeInclusive;

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The four corners as the sequence spelled them: 1-based and inclusive, with 0 — which is also what
/// an omitted parameter means — standing for "the edge of the page".
///
/// Kept unresolved on purpose. Turning these into cells needs the page size, which is the engine's to
/// know, so the scanner reports what the program asked for and `from_corners` below resolves it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Corners {
	pub top: u16,
	pub left: u16,
	pub bottom: u16,
	pub right: u16,
}

/// Which way SL and SR move the page (§100).
///
/// Named for what the CONTENT does, which is how both ECMA-48 and xterm write them: `SL` shifts the
/// data left, so the blanks arrive on the right. Reading the name as "the blanks come from the left"
/// gets it backwards, and a shift that goes the wrong way is a screen no program can recover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectDirection {
	/// `CSI Ps SP @` — content moves left, blanks fill the right edge.
	Left,
	/// `CSI Ps SP A` — content moves right, blanks fill the left edge.
	Right,
}

/// Which shape DECCARA and DECRARA act on — the whole content of DECSACE (§59).
///
/// The distinction only exists for those two. The four content operations of §58 are always the
/// rectangle, whatever this is set to, which is why `from_corners` below takes it as an argument rather than
/// reading it from anywhere: the call site says which family it belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RectExtent {
	/// `CSI 0 * x` and `CSI 1 * x`, and the state a terminal powers up in: the wrapped RUN of
	/// character positions from the top-left corner to the bottom-right one — out to the end of the
	/// first row, every whole row between, then in from the start of the last. The shape a mouse
	/// selection has, and the one that suits a paragraph.
	#[default]
	Stream,
	/// `CSI 2 * x`: the rectangle itself, the same box the erase and fill of §58 use.
	Rectangle,
}

/// The four attributes DECCARA and DECRARA can name, as a mask of cmote's own.
///
/// Deliberately not the engine's `Flags`. This module stays free of engine types so its folding is
/// testable without a terminal, and `term/mod.rs` holds the one table that maps these onto flag
/// names — which is also the honest place for BLINK to go missing, since the engine's flag word has
/// no bit for it.
pub const BOLD: u8 = 1 << 0;
/// Underline: DECCARA's `4` / `24`, DECRARA's `4`.
pub const UNDERLINE: u8 = 1 << 1;
/// Blink: DECCARA's `5` / `25`, DECRARA's `5`. Read, and then dropped by `term/mod.rs`.
pub const BLINK: u8 = 1 << 2;
/// Reverse video: DECCARA's `7` / `27`, DECRARA's `7`.
pub const REVERSE: u8 = 1 << 3;
/// All four at once — what the selector `0` names in both sequences.
pub const ALL_ATTRIBUTES: u8 = BOLD | UNDERLINE | BLINK | REVERSE;

/// What one DECCARA or DECRARA does to a cell's attributes, folded down from its selector list.
///
/// Three masks rather than a list, because the selectors are applied in order and a later one wins:
/// `1;22` is bold off, not both. Folding at parse time means the loop over the cells does the same
/// tiny amount of work per cell however long the list was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttributeChange {
	/// Attributes to turn on (DECCARA only).
	pub on: u8,
	/// Attributes to turn off (DECCARA only).
	pub off: u8,
	/// Attributes to flip (DECRARA only).
	pub flip: u8,
}

impl AttributeChange {
	/// The attributes a cell currently at `current` should end up with.
	///
	/// The three masks never overlap in practice — DECCARA fills only `on` and `off`, DECRARA only
	/// `flip` — so the order they are applied in is arbitrary, and stating one here is cheaper than
	/// proving it cannot matter.
	pub fn apply(self, current: u8) -> u8 {
		((current | self.on) & !self.off) ^ self.flip
	}

	/// Whether this asks for nothing at all — an empty selector list, or one naming only attributes
	/// cmote does not know. The caller may skip the walk entirely.
	pub fn is_empty(self) -> bool {
		self.on == 0 && self.off == 0 && self.flip == 0
	}
}

/// A resolved rectangle of cells: 0-based, inclusive on all four sides, and known to be inside the
/// page it was resolved against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
	pub top: usize,
	pub left: usize,
	pub bottom: usize,
	pub right: usize,
}

impl Rect {
	/// The rows this rectangle covers, top to bottom.
	pub fn rows(&self) -> RangeInclusive<usize> {
		self.top..=self.bottom
	}

	/// The columns this rectangle covers, left to right.
	pub fn columns(&self) -> RangeInclusive<usize> {
		self.left..=self.right
	}

	/// The columns covered on one particular row, which is where the two extents differ (§59).
	///
	/// Under `Rectangle` this is just `columns` on every row. Under `Stream` the area is a run
	/// rather than a box: the first row starts at the left corner and runs to the edge of the page,
	/// the last row starts at the edge and stops at the right corner, and everything between is
	/// whole. A one-row area is both at once and comes out as the plain span, which is what makes
	/// `CSI 3;10;3;20;4$r` underline ten cells of one line under either extent.
	pub fn columns_on(&self, row: usize, extent: RectExtent, cols: usize) -> RangeInclusive<usize> {
		match extent {
			RectExtent::Rectangle => self.columns(),
			RectExtent::Stream => {
				let first = if row == self.top { self.left } else { 0 };
				let last = if row == self.bottom {
					self.right
				} else {
					cols.saturating_sub(1)
				};
				first..=last
			}
		}
	}

	/// How many cells wide it is — the stride a copy reads its buffer back at.
	pub fn width(&self) -> usize {
		self.right - self.left + 1
	}

	/// How many cells tall it is.
	pub fn height(&self) -> usize {
		self.bottom - self.top + 1
	}
}

/// One rectangular operation the stream asked for, to be applied once the engine has been advanced
/// past the sequence that carried it (see `Rectangles::feed` on offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectRequest {
	/// DECERA — blank the rectangle. Protection does not stop it.
	Erase(Corners),
	/// DECSERA — blank the rectangle, leaving the cells DECSCA protected (§56).
	SelectiveErase(Corners),
	/// DECFRA — fill the rectangle with one character, in the pen's current attributes.
	Fill(char, Corners),
	/// DECCRA — copy the rectangle to a destination named by its top-left corner alone, since the
	/// extent comes from the source. The corner is 1-based like everything else here, with 0 meaning
	/// the first row or column.
	Copy {
		source: Corners,
		top: u16,
		left: u16,
	},
	/// DECCARA and DECRARA — change or flip attributes across an area, leaving every character
	/// where it stands (§59). The extent travels WITH the request rather than being looked up when
	/// it is applied, because DECSACE is a mode and only the scanner sees the two in stream order.
	Attributes {
		corners: Corners,
		extent: RectExtent,
		change: AttributeChange,
	},
	/// SL / SR — shift every row of the visible page sideways by `columns`, blanking the edge the
	/// content moved away from (§100).
	///
	/// `columns` is already defaulted here: an omitted or zero parameter is one column, which is what
	/// ECMA-48 and xterm both spell as the default. It is not yet clamped to the page — that needs a
	/// width, which is the applier's to know.
	Shift {
		direction: RectDirection,
		columns: u16,
	},
	/// UNSCROLL — scroll the page down `lines` lines and fill the top from the SCROLLBACK rather than
	/// with blanks (§101). kitty's, and the only operation in this module that changes how many lines
	/// the document has.
	///
	/// `lines` is defaulted here and clamped where it is applied, which needs a page height.
	Unscroll { lines: u16 },
	/// DECIC and DECDC — insert or delete whole COLUMNS at the cursor, across every row of the
	/// scrolling region, within the left and right margins (§102).
	///
	/// The vertical twins of IL and DL, and the only sequences in this module whose reach is decided
	/// by state rather than by their own parameters: they take the band the margins mark out, and the
	/// rows the scrolling region marks out, and touch nothing else.
	///
	/// `columns` is defaulted here to at least one and clamped where it is applied, which needs the
	/// band.
	Columns { columns: u16, insert: bool },
	/// DECRQCRA — report a checksum of the rectangle, and change nothing (§60). `id` is the label
	/// the program attached so it can match the answer to the question, echoed back untouched.
	///
	/// The only request here that OWES a reply. The others may resolve to nothing and simply not
	/// happen; a query that resolves to nothing still has to say so, or the program that asked waits
	/// on a terminal that has already moved on (§33).
	Checksum { id: u16, corners: Corners },
}

/// The rectangular-operations scanner (§58, §59, §60). Feed it every byte of shell output; it
/// reports the seven sequences the engine drops, in the order the stream put them, and holds the one
/// mode among them (DECSACE) itself.
#[derive(Debug, Default)]
pub struct Rectangles {
	/// The CSI grammar, shared with the other scanners (§111). For this family the intermediate is
	/// what matters most, and telling the seven apart on it is all that is left here.
	framer: super::csi::Framer,
	/// Whether the previous byte was an ESC, for RIS (`ESC c`) — not a CSI, so not the framer's.
	after_escape: bool,
	/// The extent DECSACE last selected, stamped onto each attribute request as it goes out (§59).
	extent: RectExtent,
}

impl Rectangles {
	/// Scan a chunk of shell output, returning what to do and where. Safe at any chunk boundary — the
	/// state machine carries over between calls, so a sequence may be split anywhere, even between the
	/// ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, as a selective erase's is (§56) and unlike a
	/// prompt mark's. These operations name their own coordinates and never read or move the cursor,
	/// so the offset is not about where the cursor will be — it is only about ordering against the
	/// text in the same chunk, and applying them on the far side of the sequence the engine is about
	/// to ignore keeps both halves in a defined state. The checksum needs that ordering for a second
	/// reason (§60): it READS the page, so it has to be answered from the page as it stood where the
	/// question sat, not as the rest of the chunk went on to leave it.
	///
	/// DECSACE comes out of here as nothing at all: it selects a mode, and the mode is stamped onto
	/// the attribute requests that follow it (§59). A chunk carrying only a DECSACE therefore still
	/// reports no request, and `process` still makes a single advance.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, RectRequest)> {
		// RIS resets DECSACE, and that mode is stamped onto every attribute request that FOLLOWS it,
		// so the reset has to land between the sequences either side of it. The other scanners that
		// read RIS collect it in a second pass and merge by offset (`scp`, `protect`, `sgrstack`);
		// that cannot work here, because a second pass would apply the reset to every request in the
		// chunk or to none of them. The chunk is cut into runs at each RIS instead, and each run fed
		// in turn — which is what "in the order the stream put them" has to mean when one of the two
		// families changes how the other reads (§59, §111).
		let mut requests = Vec::new();
		let mut start = 0;
		for (index, &byte) in bytes.iter().enumerate() {
			if self.after_escape && byte == b'c' {
				// Up to and INCLUDING the RIS, so the framer sees every byte exactly once and in
				// order — its own state carries across the cut like it carries across a chunk.
				self.scan(&bytes[start..=index], start, &mut requests);
				// RIS resets everything a terminal holds, and DECSACE is one of those things (§59).
				// Nothing else here has state to clear.
				self.extent = RectExtent::default();
				start = index + 1;
			}
			self.after_escape = byte == ESC;
		}
		self.scan(&bytes[start..], start, &mut requests);
		requests
	}

	/// Frame one run of the chunk and classify what it holds. `base` is where the run starts in the
	/// chunk, so the offsets reported are into the chunk the caller passed rather than into the run.
	fn scan(&mut self, bytes: &[u8], base: usize, requests: &mut Vec<(usize, RectRequest)>) {
		// Destructured so the closure can hold `extent` while `framer` is borrowed for the scan.
		let Self { framer, extent, .. } = self;
		framer.feed(bytes, |span, csi| {
			if let Some(request) = classify(csi, extent) {
				requests.push((base + span.past(), request));
			}
		});
	}
}

/// Decide what a finished sequence means. All three of final byte, marker and intermediates are
/// matched together, which is what keeps the neighbours out: `CSI Ps $ p` is DECRQM and belongs to
/// the engine, `CSI Ps t` with no intermediate is a window operation, and `CSI Pch;… $ x` is DECFRA
/// where `CSI Ps * x` is DECSACE — one intermediate byte apart.
///
/// Takes `extent` by mutable reference for one sequence only: DECSACE sets a mode instead of asking
/// for work, so it is recorded here and reported as nothing (§59).
fn classify(csi: &super::csi::Csi<'_>, extent: &mut RectExtent) -> Option<RectRequest> {
	// Every sequence below spells its parameters with `;`, so a sub-parameter means this was not one
	// of them (`Csi::sub_parameters`). Five of the ten scanners refuse a sub-parameter, and this is the
	// strictest of the five — the reason the rule was written down: a rectangle built from a misread
	// corner erases cells the program never named, so `CSI 2 : 3 ; 5 ; 7 $ z` must not become an erase
	// of rows 2 to 5.
	if csi.sub_parameters() {
		return None;
	}
	match (csi.final_byte(), csi.marker(), csi.intermediates()) {
		// DECERA / DECSERA — the same four corners, differing only in whether protection holds.
		(b'z', None, [b'$']) => Some(RectRequest::Erase(corners(csi, 0))),
		(b'{', None, [b'$']) => Some(RectRequest::SelectiveErase(corners(csi, 0))),
		// DECFRA — the character first, then the four corners.
		(b'x', None, [b'$']) => {
			let glyph = fill_char(number(csi, 0))?;
			Some(RectRequest::Fill(glyph, corners(csi, 1)))
		}
		// DECCRA — the source's four corners, its page (ignored), then the destination's top-left
		// corner. The eighth parameter is the destination page, ignored for the same reason.
		(b'v', None, [b'$']) => Some(RectRequest::Copy {
			source: corners(csi, 0),
			top: number(csi, 5),
			left: number(csi, 6),
		}),
		// DECCARA / DECRARA — the four corners, then a list of SGR-shaped selectors. Same
		// shape, different verb: one sets and clears, the other flips.
		(b'r', None, [b'$']) => Some(RectRequest::Attributes {
			corners: corners(csi, 0),
			extent: *extent,
			change: changes(selectors(csi)),
		}),
		(b't', None, [b'$']) => Some(RectRequest::Attributes {
			corners: corners(csi, 0),
			extent: *extent,
			change: reversals(selectors(csi)),
		}),
		// DECRQCRA — the id and the page come FIRST, so the corners start at parameter 2 (§60).
		// The page is ignored, as DECCRA's two are and for the same reason: cmote has one page,
		// which is what clamping a page number to the number of pages there are amounts to. That
		// also settles the `Pp = 0` case DEC defines as "all of page memory" — with one page, the
		// whole page is all of them, and omitted corners already mean the whole page.
		(b'y', None, [b'*']) => Some(RectRequest::Checksum {
			id: number(csi, 0),
			corners: corners(csi, 2),
		}),
		// SL / SR — one parameter, and the SPACE intermediate is the whole of what tells them
		// from their neighbours (§100). `CSI Ps @` with no intermediate is ICH, which inserts
		// blanks at the cursor, and `CSI Ps A` is CUU, which moves the cursor up: two sequences
		// the engine implements and every program uses, one byte away from these. Matching the
		// intermediate alongside the final byte is what keeps them apart, the same near-miss rule
		// §56 wrote down.
		//
		// An omitted or zero count is one column, as it is for ICH, CUU and every other
		// `Ps`-counted movement — `vte`'s own `next_param_or(1)` reads a literal `0` that way too,
		// so this agrees with how the engine would have read the same parameter.
		(b'@', None, [b' ']) => Some(RectRequest::Shift {
			direction: RectDirection::Left,
			columns: number(csi, 0).max(1),
		}),
		(b'A', None, [b' ']) => Some(RectRequest::Shift {
			direction: RectDirection::Right,
			columns: number(csi, 0).max(1),
		}),
		// UNSCROLL — SD's final byte under a `+` intermediate, which kitty chose because it is
		// "legal under ECMA 48 and previously unused" (§101). `CSI Ps T` with no intermediate is
		// SD itself and belongs to the engine; the intermediate is the whole difference, as it is
		// for SL and SR above.
		(b'T', None, [b'+']) => Some(RectRequest::Unscroll {
			lines: number(csi, 0).max(1),
		}),
		// DECIC / DECDC — insert or delete columns, under an apostrophe intermediate `vte` has no
		// arm for at all, so like the two above they reach nothing and are cmote's to read (§102).
		// The near-miss rule again: `CSI Ps }` and `CSI Ps ~` without the intermediate are other
		// sequences entirely, and the intermediate is the whole of what tells them apart.
		(b'}', None, [b'\'']) => Some(RectRequest::Columns {
			columns: number(csi, 0).max(1),
			insert: true,
		}),
		(b'~', None, [b'\'']) => Some(RectRequest::Columns {
			columns: number(csi, 0).max(1),
			insert: false,
		}),
		// DECSACE — a mode, absorbed rather than reported (§59). Only the three defined values
		// mean anything; a fourth leaves the extent where it was, rather than guessing at a
		// shape the program did not name (§54's rule, and §58's).
		(b'x', None, [b'*']) => {
			match number(csi, 0) {
				0 | 1 => *extent = RectExtent::Stream,
				2 => *extent = RectExtent::Rectangle,
				_ => {}
			}
			None
		}
		_ => None,
	}
}

/// One parameter by position, or 0 when the sequence stopped short of it or left it empty — the same
/// thing an omitted parameter means, so a program may leave any tail off.
///
/// There is no "unreadable" case left to report. Every sequence here used to be dropped whole if any
/// field would not parse, which was §54's rule (malformed remote input is a no-op, never a guess) and
/// still is — the framer just enforces it earlier and better: a parameter run is digits and separators
/// only, a private marker among them abandons the sequence, and a sub-parameter is refused above.
///
/// What DID change is a number too big for the type. `CSI 99999;1;1;1 $ z` used to be dropped, because
/// the fold used `checked_mul`; it saturates now, as the engine does. Nothing is erased either way —
/// `from_corners` refuses a top past the page — but the reading is the engine's rather than a
/// scanner giving up on a sequence that was never malformed to begin with (§106, §111).
fn number(csi: &super::csi::Csi<'_>, index: usize) -> u16 {
	csi.param(index).unwrap_or(0)
}

/// Four consecutive parameters read as corners, starting at `first`.
fn corners(csi: &super::csi::Csi<'_>, first: usize) -> Corners {
	Corners {
		top: number(csi, first),
		left: number(csi, first + 1),
		bottom: number(csi, first + 2),
		right: number(csi, first + 3),
	}
}

/// The attribute selectors of a DECCARA or DECRARA — everything after the four corners (§59).
///
/// Empty when the program named a rectangle and no attributes, which is a well-formed request to do
/// nothing and is treated as one. An iterator rather than a slice because there is no longer a `Vec`
/// of parsed numbers to take a tail of: the framer keeps the run, and each selector is read from it
/// where the fold needs it.
fn selectors(csi: &super::csi::Csi<'_>) -> impl Iterator<Item = u16> {
	(4..csi.param_count()).map(|index| number(csi, index))
}

/// Fold a DECCARA's selectors into one change.
///
/// The DEC-defined set and nothing else: `0` for all off, then the four attributes on and the same
/// four off. Applied strictly in order, each one clearing the other mask, so `1;22` is bold off and
/// `22;1` is bold on — a later selector wins, exactly as it does in an SGR.
///
/// A selector outside the set is IGNORED rather than fatal. That is the opposite of the rule for a
/// malformed number, and deliberately: a number that will not parse leaves cmote unable to say which
/// cells were meant, while `3` (italic, which DEC never gave DECCARA) is a perfectly clear request
/// for an attribute this sequence cannot name. Ignoring the one it cannot do and honouring the rest
/// is what an SGR does with an unknown attribute.
fn changes(selectors: impl Iterator<Item = u16>) -> AttributeChange {
	let mut change = AttributeChange::default();
	for selector in selectors {
		let (attribute, turn_on) = match selector {
			0 => (ALL_ATTRIBUTES, false),
			1 => (BOLD, true),
			4 => (UNDERLINE, true),
			5 => (BLINK, true),
			7 => (REVERSE, true),
			22 => (BOLD, false),
			24 => (UNDERLINE, false),
			25 => (BLINK, false),
			27 => (REVERSE, false),
			_ => continue,
		};
		if turn_on {
			change.on |= attribute;
			change.off &= !attribute;
		} else {
			change.off |= attribute;
			change.on &= !attribute;
		}
	}
	change
}

/// Fold a DECRARA's selectors into one change.
///
/// A shorter table than DECCARA's: DEC gives this one `0`, `1`, `4`, `5` and `7` only, because "off"
/// has no meaning for a verb that flips. The off-forms are therefore ignored here rather than
/// quietly treated as their on-forms, which would turn `CSI …;24 $ t` into an underline toggle the
/// program never asked for.
///
/// Repeats cancel — `1;1` flips bold twice and leaves it — because the selectors are applied in
/// order, the same rule DECCARA follows. Neither reading is written down by DEC, and "apply each in
/// turn" is the one that matches its sibling.
fn reversals(selectors: impl Iterator<Item = u16>) -> AttributeChange {
	let mut change = AttributeChange::default();
	for selector in selectors {
		let attribute = match selector {
			0 => ALL_ATTRIBUTES,
			1 => BOLD,
			4 => UNDERLINE,
			5 => BLINK,
			7 => REVERSE,
			_ => continue,
		};
		change.flip ^= attribute;
	}
	change
}

/// DECFRA's fill character, or `None` for a code cmote will not paint the page with.
///
/// Printable ASCII and printable Latin-1 only, the two ranges xterm allows. Latin-1 maps to the
/// Unicode code points of the same value, so the conversion is the identity and cannot fail — but it
/// is written as a conversion rather than a cast so the allow-list stays the only thing deciding.
fn fill_char(code: u16) -> Option<char> {
	match code {
		32..=126 | 160..=255 => char::from_u32(u32::from(code)),
		_ => None,
	}
}

/// Resolve four corners against a page of this size: 1-based and inclusive becomes 0-based and
/// inclusive, defaults become edges, and anything past an edge is clamped to it.
///
/// `None` when the area holds no cells — an empty page, corners the program crossed over, or a
/// top-left that starts off the page entirely. A DEC terminal does nothing for those, and so does
/// this.
///
/// The extent changes exactly one rule, which is why it is a parameter here rather than a mode read
/// from somewhere. A RECTANGLE with its right corner left of its left one is undrawable. A STREAM
/// with the same numbers is ordinary — `CSI 1;70;5;10;4$r` underlines from row 1 column 70, round
/// the wrap, to row 5 column 10 — so left and right are only compared when the run is confined to a
/// single row. Top below bottom is a crossing under either.
pub fn from_corners(
	corners: Corners,
	extent: RectExtent,
	rows: usize,
	cols: usize,
) -> Option<Rect> {
	if rows == 0 || cols == 0 {
		return None;
	}
	// An omitted or zero parameter means the default, and the default is the edge in that direction:
	// the first row or column for a start, the last for an end.
	let start = |value: u16| usize::from(value).max(1);
	let end = |value: u16, last: usize| match usize::from(value) {
		0 => last,
		given => given.min(last),
	};
	let top = start(corners.top);
	let left = start(corners.left);
	let bottom = end(corners.bottom, rows);
	let right = end(corners.right, cols);
	// An END past the page is clamped to it — a program sized for a bigger screen gets the part of
	// its rectangle that exists. A START past the page is not: clamping it back onto the last row
	// would act on a row the program never named, and doing nothing is the better answer (§57).
	if top > rows || left > cols || top > bottom {
		return None;
	}
	let columns_cross = match extent {
		RectExtent::Rectangle => left > right,
		RectExtent::Stream => top == bottom && left > right,
	};
	if columns_cross {
		return None;
	}
	Some(Rect {
		top: top - 1,
		left: left - 1,
		bottom: bottom - 1,
		right: right - 1,
	})
}

/// Where a copy actually lands: the part of the source that fits, and the destination's 0-based
/// top-left corner.
///
/// The destination is only a corner, so the extent comes from the source — and a source that would
/// run off the bottom or the right is trimmed rather than refused, which is what a DEC terminal does
/// and the only reading that makes `CSI 2;1;24;80;1;1;1;1 $ v` (scroll a page up one row) work at all.
/// `None` when the corner itself is off the page.
pub fn copy_extent(
	source: Rect,
	top: u16,
	left: u16,
	rows: usize,
	cols: usize,
) -> Option<(Rect, usize, usize)> {
	if rows == 0 || cols == 0 {
		return None;
	}
	let corner = |value: u16| usize::from(value).max(1) - 1;
	let to_row = corner(top);
	let to_col = corner(left);
	if to_row >= rows || to_col >= cols {
		return None;
	}
	let height = source.height().min(rows - to_row);
	let width = source.width().min(cols - to_col);
	let trimmed = Rect {
		top: source.top,
		left: source.left,
		bottom: source.top + height - 1,
		right: source.left + width - 1,
	};
	Some((trimmed, to_row, to_col))
}

/// What a cell has to weigh before the checksum will trim it away: a plain space, and nothing added
/// on top of it (§60).
///
/// The comparison is against the cell's FINISHED value, attributes included, which is what makes it
/// right rather than merely cheap: an underlined blank weighs 0x20 + 0x10 and so is kept. A cell you
/// can see is a cell that counts.
const BLANK: u32 = 0x20;

/// The running total behind a DECRQCRA report (§60) — xterm's `xtermCheckRect` with no extension
/// bits, which is the DEC-compatible default and the only version worth computing.
///
/// Kept here, away from the engine's types, so the two rules with any subtlety in them — the trimmed
/// blank and the exempt first cell — are tested against numbers rather than against a grid.
/// `term/mod.rs` weighs each cell and feeds it in; this decides what survives and what the four
/// digits come out as.
#[derive(Debug, Default)]
pub struct Checksum {
	total: u32,
	/// Whether a cell has been counted yet. xterm's `first`, inverted: its exemption is for the
	/// first cell of the WHOLE rectangle, not of each row, so it survives the row boundary.
	counted: bool,
}

impl Checksum {
	/// Weigh in one cell, given as its character code plus whatever its attributes added.
	///
	/// A plain space is dropped — that is the trim, and it is why a mostly-empty page checksums as
	/// though it held only its text. The very first cell is never dropped, so an all-blank rectangle
	/// still reports one space rather than nothing, which is xterm's behaviour and reads like a
	/// deliberate guard against a rectangle and an empty rectangle being indistinguishable.
	pub fn cell(&mut self, value: u32) {
		if self.counted && value == BLANK {
			return;
		}
		self.counted = true;
		self.total = self.total.wrapping_add(value);
	}

	/// The number to report: the total modulo 2^16, negated.
	///
	/// Negated because DEC's terminals did, which is why a real checksum of real text is a large hex
	/// number rather than a small one — the single detail most likely to be got wrong by an
	/// implementation working from the shape of the sequence rather than from a terminal.
	pub fn finish(self) -> u16 {
		let low = (self.total & 0xffff) as u16;
		low.wrapping_neg()
	}
}

/// Where an absolute document line ends up after an UNSCROLL (§101).
///
/// This is the whole reason unscrolling is more than a grid operation. Every position cmote
/// remembers about the session — a prompt mark, a bookmark, a command's output span, a picture's
/// anchor, a line's right-to-left flag — is an ABSOLUTE line index, `history_size + row` at the
/// moment it was taken (§34, §40, §76). Unscrolling moves the boundary between the scrollback and
/// the page, so it is the one operation in this terminal that can change what those numbers mean.
///
/// Three numbers describe the move, and the arithmetic behind them is worth writing out because the
/// happy case looks like nothing happens — which is a very easy thing to get wrong by not thinking
/// about it. Write the document as `[history 0..H] [page 0..R]`, absolute index being the position
/// in that list, and unscroll `lines` of which `N` could be filled from the scrollback:
///
///   before   [ history 0..H-N ][ history H-N..H ]                 [ page 0..R-lines ][ discarded ]
///   after    [ history 0..H-N ][ blanks × (lines-N) ][ the same H-N..H ][ the same 0..R-lines ]
///
/// So a line before the consumed history keeps its number; everything from there down moves by the
/// number of BLANKS, not by `lines`; and the page's bottom `lines` rows are gone. When the
/// scrollback held everything asked for — the ordinary case, since a program unscrolls what it just
/// scrolled — `N == lines`, the blank count is zero, and **not one number changes**. The document
/// simply gets shorter at the end.
///
/// The lines past `discard_from` must not merely be left alone: their content is gone, and the
/// document will grow back over those indices with new output, so an anchor left there would
/// reappear one day on text it never described.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unscrolled {
	/// The first absolute line the move touches — the oldest scrollback line pulled onto the page.
	pub cut: u64,
	/// How far everything from `cut` down moves: the number of blank lines inserted, which is zero
	/// whenever the scrollback could fill the request.
	pub shift: u64,
	/// The first absolute line whose content is discarded off the bottom of the page.
	pub discard_from: u64,
}

impl Unscrolled {
	/// Work the three numbers out from the page's own measurements: the history and page sizes as
	/// they stood BEFORE the move, the number of lines asked for, and how many of them the
	/// scrollback could supply.
	pub fn new(history: usize, rows: usize, lines: usize, from_history: usize) -> Self {
		let history = history as u64;
		Self {
			cut: history - from_history as u64,
			shift: (lines - from_history) as u64,
			discard_from: history + (rows - lines) as u64,
		}
	}

	/// Where `line` goes, or `None` when its content was pushed off the bottom and no longer exists.
	pub fn map(&self, line: u64) -> Option<u64> {
		if line >= self.discard_from {
			None
		} else if line >= self.cut {
			Some(line + self.shift)
		} else {
			Some(line)
		}
	}
}

/// The DECCKSR report a DECRQCRA earns: `DCS Pid ! ~ XXXX ST` (§60).
///
/// The id is echoed back exactly as the program spelled it, which is its whole purpose — a program
/// with several questions outstanding matches answers to questions by it. The checksum is four
/// upper-case hex digits, always four: a short one would be read as a short reply rather than a
/// small number.
pub fn checksum_reply(id: u16, checksum: u16) -> Vec<u8> {
	format!("\x1bP{id}!~{checksum:04X}\x1b\\").into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read what it asked for.
	fn scan(bytes: &[u8]) -> Vec<(usize, RectRequest)> {
		let mut rectangles = Rectangles::default();
		rectangles.feed(bytes)
	}

	/// The four corners, spelled out.
	fn box_of(top: u16, left: u16, bottom: u16, right: u16) -> Corners {
		Corners {
			top,
			left,
			bottom,
			right,
		}
	}

	/// Resolve corners as a rectangle — the extent all four §58 operations use, and DECSACE's
	/// second choice for the two of §59.
	fn boxed(corners: Corners, rows: usize, cols: usize) -> Option<Rect> {
		from_corners(corners, RectExtent::Rectangle, rows, cols)
	}

	/// Resolve corners as a wrapped run — DECSACE's default, and so the one an unarmed program gets.
	fn streamed(corners: Corners, rows: usize, cols: usize) -> Option<Rect> {
		from_corners(corners, RectExtent::Stream, rows, cols)
	}

	#[test]
	fn an_erase_reads_its_four_corners() {
		// `\x1b[2;3;5;7$z` is 11 bytes, so the offset is 11 — one PAST the final `z`.
		assert_eq!(
			scan(b"\x1b[2;3;5;7$z"),
			vec![(11, RectRequest::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn a_selective_erase_is_the_same_rectangle_by_another_verb() {
		assert_eq!(
			scan(b"\x1b[2;3;5;7${"),
			vec![(11, RectRequest::SelectiveErase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn an_erase_with_no_parameters_is_the_whole_page() {
		// Every corner defaults to its edge, which is how a program spells "all of it".
		assert_eq!(
			scan(b"\x1b[$z"),
			vec![(4, RectRequest::Erase(box_of(0, 0, 0, 0)))]
		);
	}

	#[test]
	fn a_fill_reads_the_character_before_the_corners() {
		// 45 is `-`: rule a line across a box.
		assert_eq!(
			scan(b"\x1b[45;2;1;2;80$x"),
			vec![(15, RectRequest::Fill('-', box_of(2, 1, 2, 80)))]
		);
	}

	#[test]
	fn a_fill_takes_printable_latin_one_too() {
		// 176 is `°`. The high range is the other half of what xterm allows.
		assert_eq!(
			scan(b"\x1b[176;1;1;1;1$x"),
			vec![(15, RectRequest::Fill('°', box_of(1, 1, 1, 1)))]
		);
	}

	#[test]
	fn a_fill_with_a_control_character_is_refused() {
		// The allow-list. A remote must not paint the page with C0, C1 or DEL — and refusing the
		// whole sequence is safer than filling with a substitute nobody asked for (§12).
		assert!(scan(b"\x1b[0;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[7;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[27;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[127;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[155;1;1;5;5$x").is_empty());
		assert!(scan(b"\x1b[300;1;1;5;5$x").is_empty());
	}

	#[test]
	fn a_copy_reads_a_source_and_a_corner() {
		// The shape that scrolls a sub-window up by one row: copy rows 2–10 to row 1.
		assert_eq!(
			scan(b"\x1b[2;1;10;40;1;1;1;1$v"),
			vec![(
				21,
				RectRequest::Copy {
					source: box_of(2, 1, 10, 40),
					top: 1,
					left: 1,
				}
			)]
		);
	}

	#[test]
	fn a_copy_may_leave_its_tail_off() {
		// Trailing parameters are omitted freely, and an omitted one reads as 0, which resolves to
		// the first row and column.
		assert_eq!(
			scan(b"\x1b[2;1;10;40$v"),
			vec![(
				13,
				RectRequest::Copy {
					source: box_of(2, 1, 10, 40),
					top: 0,
					left: 0,
				}
			)]
		);
	}

	#[test]
	fn a_change_reads_its_corners_then_its_selectors() {
		// `\x1b[1;1;5;5;1;4$r` is 16 bytes: bold and underline on, across rows 1–5.
		assert_eq!(
			scan(b"\x1b[1;1;5;5;1;4$r"),
			vec![(
				15,
				RectRequest::Attributes {
					corners: box_of(1, 1, 5, 5),
					extent: RectExtent::Stream,
					change: AttributeChange {
						on: BOLD | UNDERLINE,
						off: 0,
						flip: 0,
					},
				}
			)]
		);
	}

	#[test]
	fn a_reversal_flips_instead_of_setting() {
		assert_eq!(
			scan(b"\x1b[1;1;5;5;7$t"),
			vec![(
				13,
				RectRequest::Attributes {
					corners: box_of(1, 1, 5, 5),
					extent: RectExtent::Stream,
					change: AttributeChange {
						on: 0,
						off: 0,
						flip: REVERSE,
					},
				}
			)]
		);
	}

	#[test]
	fn a_later_selector_wins() {
		// The SGR rule: `1;22` is bold OFF, and `22;1` is bold on. Folded at parse time, so the walk
		// over the cells costs the same however long the list was.
		assert_eq!(
			changes([1, 22].into_iter()),
			AttributeChange {
				on: 0,
				off: BOLD,
				flip: 0
			}
		);
		assert_eq!(
			changes([22, 1].into_iter()),
			AttributeChange {
				on: BOLD,
				off: 0,
				flip: 0
			}
		);
	}

	#[test]
	fn selector_zero_names_all_four_attributes() {
		// And only those four — never a colour, and never the flag word wholesale, which would take
		// cmote's protection bit with it (§56).
		assert_eq!(
			changes([0].into_iter()),
			AttributeChange {
				on: 0,
				off: ALL_ATTRIBUTES,
				flip: 0
			}
		);
		assert_eq!(
			reversals([0].into_iter()),
			AttributeChange {
				on: 0,
				off: 0,
				flip: ALL_ATTRIBUTES
			}
		);
	}

	#[test]
	fn an_unknown_selector_is_ignored_not_fatal() {
		// `3` is italic, which DEC never gave DECCARA. The rest of the list still applies — the same
		// thing an SGR does with an attribute it does not know, and unlike a malformed NUMBER, which
		// drops the sequence because it leaves the cells themselves in doubt.
		assert_eq!(
			changes([3, 4, 38].into_iter()),
			AttributeChange {
				on: UNDERLINE,
				off: 0,
				flip: 0
			}
		);
		assert!(changes([3].into_iter()).is_empty());
	}

	#[test]
	fn a_reversal_has_no_off_forms() {
		// DEC gives DECRARA 0, 1, 4, 5 and 7 only. Reading `24` as an underline toggle would flip an
		// attribute on a request that plainly said "off".
		assert!(reversals([22, 24, 25, 27].into_iter()).is_empty());
	}

	#[test]
	fn repeated_reversals_cancel() {
		assert!(reversals([1, 1].into_iter()).is_empty());
		assert_eq!(
			reversals([0, 1].into_iter()),
			AttributeChange {
				on: 0,
				off: 0,
				flip: UNDERLINE | BLINK | REVERSE
			}
		);
	}

	#[test]
	fn a_change_with_no_selectors_asks_for_nothing() {
		// Well-formed, and a request to do nothing. `is_empty` lets the caller skip the walk.
		let requests = scan(b"\x1b[1;1;5;5$r");
		let [(_, RectRequest::Attributes { change, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert!(change.is_empty());
	}

	#[test]
	fn a_change_folds_to_the_new_attributes() {
		assert_eq!(
			changes([1, 4].into_iter()).apply(REVERSE),
			BOLD | UNDERLINE | REVERSE
		);
		assert_eq!(changes([0].into_iter()).apply(ALL_ATTRIBUTES), 0);
		assert_eq!(reversals([1].into_iter()).apply(BOLD | REVERSE), REVERSE);
		assert_eq!(reversals([1].into_iter()).apply(REVERSE), BOLD | REVERSE);
		// The empty change is the identity, which is what makes skipping it safe.
		assert_eq!(AttributeChange::default().apply(BOLD), BOLD);
	}

	#[test]
	fn the_extent_selects_the_shape_the_pair_acts_on() {
		// DECSACE reports nothing of its own — it stamps the requests after it (§59).
		let mut rectangles = Rectangles::default();
		assert!(rectangles.feed(b"\x1b[2*x").is_empty());
		let requests = rectangles.feed(b"\x1b[1;1;5;5;1$r");
		let [(_, RectRequest::Attributes { extent, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert_eq!(extent, RectExtent::Rectangle);
		// And back: 0 and 1 both mean the stream.
		assert!(rectangles.feed(b"\x1b[1*x").is_empty());
		let requests = rectangles.feed(b"\x1b[1;1;5;5;1$r");
		let [(_, RectRequest::Attributes { extent, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert_eq!(extent, RectExtent::Stream);
	}

	#[test]
	fn an_extent_nobody_defined_leaves_the_mode_alone() {
		let mut rectangles = Rectangles::default();
		assert!(rectangles.feed(b"\x1b[2*x\x1b[9*x").is_empty());
		let requests = rectangles.feed(b"\x1b[1;1;5;5;1$r");
		let [(_, RectRequest::Attributes { extent, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert_eq!(extent, RectExtent::Rectangle);
	}

	#[test]
	fn a_full_reset_puts_the_extent_back() {
		// RIS resets everything a terminal holds. DECSTR does not, because DEC's published list for
		// it does not name DECSACE — inventing a reset is the same guess as inventing a rectangle.
		let mut rectangles = Rectangles::default();
		assert!(rectangles.feed(b"\x1b[2*x\x1b[!p").is_empty());
		let requests = rectangles.feed(b"\x1b[1;1;5;5;1$r");
		let [(_, RectRequest::Attributes { extent, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert_eq!(extent, RectExtent::Rectangle);
		assert!(rectangles.feed(b"\x1bc").is_empty());
		let requests = rectangles.feed(b"\x1b[1;1;5;5;1$r");
		let [(_, RectRequest::Attributes { extent, .. })] = requests[..] else {
			panic!("expected one attribute request");
		};
		assert_eq!(extent, RectExtent::Stream);
	}

	/// A reset in the MIDDLE of a chunk resets the extent for the sequences after it and not for the
	/// ones before it — the reason `feed` cuts the chunk into runs at each RIS rather than collecting
	/// them in a second pass and merging by offset the way `scp` and `protect` do (§111).
	///
	/// A second pass would read every request in the chunk against the extent the chunk ENDED with,
	/// which for this input would stamp `Stream` on both — a mode the program had selected and not yet
	/// reset when it asked for the first one.
	#[test]
	fn a_reset_inside_a_chunk_divides_the_sequences_around_it() {
		let requests = Rectangles::default().feed(b"\x1b[2*x\x1b[1;1;5;5;1$r\x1bc\x1b[1;1;5;5;1$r");
		let [
			(_, RectRequest::Attributes { extent: before, .. }),
			(_, RectRequest::Attributes { extent: after, .. }),
		] = requests[..]
		else {
			panic!("expected two attribute requests, got {requests:?}");
		};
		assert_eq!(before, RectExtent::Rectangle, "the mode was still in force");
		assert_eq!(after, RectExtent::Stream, "and the reset put it back");
	}

	/// The offsets survive the cut: a sequence after a mid-chunk reset is still reported at its place
	/// in the WHOLE chunk, not at its place in the run the reset started.
	#[test]
	fn an_offset_after_a_reset_still_counts_from_the_start_of_the_chunk() {
		assert_eq!(
			Rectangles::default().feed(b"\x1bc\x1b[2;3;5;7$z"),
			vec![(13, RectRequest::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn a_fill_and_an_extent_are_one_intermediate_apart() {
		// `$ x` is DECFRA and `* x` is DECSACE. Matching the final byte alone would make a fill of
		// spaces out of every extent selection.
		assert_eq!(
			scan(b"\x1b[32;1;1;1;1$x"),
			vec![(14, RectRequest::Fill(' ', box_of(1, 1, 1, 1)))]
		);
		assert!(scan(b"\x1b[2*x").is_empty());
	}

	#[test]
	fn a_window_operation_is_not_mistaken_for_a_reversal() {
		// `CSI 18 t` reports the window size and belongs to the engine. Only `$ t` is DECRARA.
		assert!(scan(b"\x1b[18t\x1b[8;24;80t").is_empty());
	}

	#[test]
	fn a_request_mode_query_is_not_mistaken_for_one() {
		// DECRQM is the one `$` sequence the engine DOES answer, in both spellings. Claiming it here
		// would take an answer away from a program that asked a question.
		assert!(scan(b"\x1b[4$p\x1b[?69$p").is_empty());
	}

	#[test]
	fn the_intermediate_is_what_makes_the_family() {
		// Without the `$` these are ordinary sequences the engine handles: `CSI 5 x` is not a fill,
		// and `CSI 2 z` is nothing at all.
		assert!(scan(b"\x1b[45;2;1;2;80x\x1b[2;3;5;7z").is_empty());
	}

	#[test]
	fn a_private_marker_rules_the_sequence_out() {
		assert!(scan(b"\x1b[?2;3;5;7$z").is_empty());
	}

	/// A rectangle built from a misread corner erases the wrong cells, so a spelling none of these
	/// sequences defines is a no-op rather than a guess — and `:` opens a sub-parameter, which none of
	/// them takes (`Csi::sub_parameters`).
	///
	/// This is the strictest of the family's refusals of that byte, and the reason the rule was
	/// written down: `sgrstack` and `modkeys` had both started reading a `:` as another `;` on their
	/// way onto the framer, and following them here would have made this an erase of rows 2 to 5.
	#[test]
	fn a_sub_parameter_is_not_one_of_these_sequences() {
		assert!(scan(b"\x1b[2:3;5;7$z").is_empty());
		assert!(scan(b"\x1b[2;3;5;7:1$z").is_empty(), "anywhere in the run");
		assert!(scan(b"\x1b[1;1;5;5;1:4$r").is_empty(), "or among selectors");
		// The `;` spelling of the same four corners IS the erase, so this is about the separator.
		assert_eq!(
			scan(b"\x1b[2;3;5;7$z"),
			vec![(11, RectRequest::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		let mut rectangles = Rectangles::default();
		assert!(rectangles.feed(b"text\x1b").is_empty());
		assert!(rectangles.feed(b"[2;3;5").is_empty());
		// The offset counts from the start of the chunk that completed the sequence.
		assert_eq!(
			rectangles.feed(b";7$zmore"),
			vec![(4, RectRequest::Erase(box_of(2, 3, 5, 7)))]
		);
	}

	#[test]
	fn several_requests_in_one_chunk_come_out_in_order() {
		let requests = scan(b"\x1b[1;1;2;2$z\x1b[45;1;1;1;9$x");
		assert_eq!(
			requests,
			vec![
				(11, RectRequest::Erase(box_of(1, 1, 2, 2))),
				(25, RectRequest::Fill('-', box_of(1, 1, 1, 9))),
			]
		);
	}

	/// A hostile stream must not be able to make the scanner buffer without bound — and the two bounds
	/// answer differently on purpose, which is what this pins now that the grammar is shared (§111).
	#[test]
	fn the_two_parameter_bounds_answer_differently() {
		// More parameters than the engine's array holds: the engine ignores the whole sequence, so the
		// scanner abandons it too. Every field is a `1`, so if it were framed at all it would be an
		// erase of the single cell at row 1 column 1 — abandonment is the only thing that empties this.
		let list = |fields: usize| {
			let params = vec!["1"; fields].join(";");
			format!("\x1b[{params}$z").into_bytes()
		};
		let bound = super::super::csi::MAX_PARAMS;
		assert_eq!(
			scan(&list(bound)),
			vec![(bound * 2 + 3, RectRequest::Erase(box_of(1, 1, 1, 1)))],
			"thirty-two parameters still fit"
		);
		assert!(scan(&list(bound + 1)).is_empty(), "thirty-three do not");

		// A runaway DIGIT run is clamped instead, and the sequence LIVES — the engine saturates the
		// number rather than giving up on it. What comes out is a corner past any page, which
		// `from_corners` refuses, so nothing is erased by a different route.
		let mut digits = b"\x1b[".to_vec();
		digits.extend(std::iter::repeat_n(b'1', 500));
		digits.extend_from_slice(b";1;1;1$z");
		let requests = scan(&digits);
		let [(_, RectRequest::Erase(corners))] = requests[..] else {
			panic!("expected one erase");
		};
		assert_eq!(corners.top, 11111, "five significant digits, and no more");
		assert_eq!(from_corners(corners, RectExtent::Stream, 24, 80), None);
	}

	#[test]
	fn a_control_byte_inside_a_csi_does_not_abandon_it() {
		// The reverse of what this asserted before §106. The engine runs the line feed where it sits and
		// keeps reading the rectangle's corners around it, so a scanner that gave up would be describing a
		// different stream from the one the engine saw.
		assert!(!scan(b"\x1b[2;3\n;5;7$z").is_empty());
		// CAN and SUB are the only two bytes that really cancel a sequence in flight.
		assert!(scan(b"\x1b[2;3\x18;5;7$z").is_empty());
	}

	#[test]
	fn ordinary_output_asks_for_nothing() {
		// The common case has to cost nothing: no request means `process` never splits its advance.
		assert!(scan(b"\x1b[31mred\x1b[0m\x1b[2J\x1b[1;1Hhello").is_empty());
	}

	#[test]
	fn corners_resolve_to_zero_based_cells() {
		assert_eq!(
			boxed(box_of(2, 3, 5, 7), 24, 80),
			Some(Rect {
				top: 1,
				left: 2,
				bottom: 4,
				right: 6
			})
		);
	}

	#[test]
	fn omitted_corners_reach_the_edges_of_the_page() {
		assert_eq!(
			boxed(box_of(0, 0, 0, 0), 24, 80),
			Some(Rect {
				top: 0,
				left: 0,
				bottom: 23,
				right: 79
			})
		);
	}

	#[test]
	fn corners_past_the_page_clamp_to_it() {
		// A program sized for a bigger screen gets the part of its rectangle that exists.
		assert_eq!(
			boxed(box_of(1, 1, 999, 999), 24, 80),
			Some(Rect {
				top: 0,
				left: 0,
				bottom: 23,
				right: 79
			})
		);
	}

	#[test]
	fn a_rectangle_that_starts_off_the_page_holds_nothing() {
		assert_eq!(boxed(box_of(25, 1, 30, 5), 24, 80), None);
		assert_eq!(boxed(box_of(1, 81, 5, 90), 24, 80), None);
	}

	#[test]
	fn a_stream_covers_whole_rows_between_its_ends() {
		// Rows 2–4 of an 80-column page, from column 3 to column 7: out to the edge, all of the
		// middle, then in from the edge. The box of the same corners is five columns on each row.
		let run = streamed(box_of(2, 3, 4, 7), 24, 80).expect("inside the page");
		assert_eq!(run.columns_on(1, RectExtent::Stream, 80), 2..=79);
		assert_eq!(run.columns_on(2, RectExtent::Stream, 80), 0..=79);
		assert_eq!(run.columns_on(3, RectExtent::Stream, 80), 0..=6);
		assert_eq!(run.columns_on(2, RectExtent::Rectangle, 80), 2..=6);
	}

	#[test]
	fn a_one_row_stream_is_the_same_span_as_the_box() {
		let run = streamed(box_of(3, 10, 3, 20), 24, 80).expect("inside the page");
		assert_eq!(run.columns_on(2, RectExtent::Stream, 80), 9..=19);
		assert_eq!(run.columns_on(2, RectExtent::Rectangle, 80), 9..=19);
	}

	#[test]
	fn a_stream_may_end_left_of_where_it_started() {
		// Row 1 column 70 round to row 5 column 10 is an ordinary run and an undrawable rectangle.
		assert!(streamed(box_of(1, 70, 5, 10), 24, 80).is_some());
		assert!(boxed(box_of(1, 70, 5, 10), 24, 80).is_none());
		// Confined to one row, it is backwards under either.
		assert!(streamed(box_of(3, 70, 3, 10), 24, 80).is_none());
	}

	#[test]
	fn crossed_corners_hold_nothing() {
		// Bottom above top, or right left of left. DEC does nothing for these rather than swapping
		// them, and silently erasing a rectangle the program did not describe would be worse.
		assert_eq!(boxed(box_of(5, 1, 2, 9), 24, 80), None);
		assert_eq!(boxed(box_of(1, 9, 5, 2), 24, 80), None);
	}

	#[test]
	fn a_single_cell_is_a_rectangle() {
		assert_eq!(
			boxed(box_of(3, 4, 3, 4), 24, 80),
			Some(Rect {
				top: 2,
				left: 3,
				bottom: 2,
				right: 3
			})
		);
	}

	#[test]
	fn a_page_with_no_cells_holds_no_rectangle() {
		assert_eq!(boxed(box_of(1, 1, 1, 1), 0, 80), None);
		assert_eq!(boxed(box_of(1, 1, 1, 1), 24, 0), None);
	}

	#[test]
	fn a_copy_that_fits_keeps_its_whole_source() {
		let source = boxed(box_of(2, 1, 10, 40), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 1, 1, 24, 80), Some((source, 0, 0)));
	}

	#[test]
	fn a_copy_running_off_the_page_is_trimmed_not_refused() {
		// Nine rows of source, but only three rows of room at the destination: the top three land and
		// the rest is dropped, which is what makes a scroll-by-copy work at the bottom of a page.
		let source = boxed(box_of(2, 1, 10, 40), 24, 80).expect("inside the page");
		let (trimmed, row, col) = copy_extent(source, 22, 79, 24, 80).expect("the corner is on it");
		assert_eq!(row, 21);
		assert_eq!(col, 78);
		assert_eq!(trimmed.height(), 3);
		assert_eq!(trimmed.width(), 2);
		assert_eq!(trimmed.top, source.top);
		assert_eq!(trimmed.left, source.left);
	}

	#[test]
	fn a_copy_to_a_corner_off_the_page_does_nothing() {
		let source = boxed(box_of(1, 1, 2, 2), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 25, 1, 24, 80), None);
		assert_eq!(copy_extent(source, 1, 81, 24, 80), None);
	}

	#[test]
	fn a_copy_corner_defaults_to_the_first_cell() {
		// An omitted destination reads as 0, and both 0 and 1 mean the first row or column.
		let source = boxed(box_of(3, 3, 4, 4), 24, 80).expect("inside the page");
		assert_eq!(copy_extent(source, 0, 0, 24, 80), Some((source, 0, 0)));
	}

	#[test]
	fn a_checksum_request_reads_its_id_then_its_corners() {
		// `Pid;Pp;Pt;Pl;Pb;Pr` — the rectangle starts at parameter 2, two later than everywhere else
		// in this family, which is the one thing about DECRQCRA's grammar that is easy to get wrong.
		assert_eq!(
			scan(b"\x1b[42;1;2;3;4;5*y"),
			vec![(
				16,
				RectRequest::Checksum {
					id: 42,
					corners: box_of(2, 3, 4, 5),
				}
			)]
		);
	}

	#[test]
	fn a_checksum_may_name_no_rectangle_at_all() {
		// DEC defines an omitted rectangle as the whole page, and `Pp = 0` as all of page memory —
		// which on a one-page terminal is the same answer, reached by the same defaulting.
		assert_eq!(
			scan(b"\x1b[1*y"),
			vec![(
				5,
				RectRequest::Checksum {
					id: 1,
					corners: box_of(0, 0, 0, 0),
				}
			)]
		);
	}

	#[test]
	fn a_checksum_and_an_extent_are_one_final_byte_apart() {
		// `* x` selects the attribute extent and reports nothing; `* y` asks a question. Sharing an
		// intermediate, they are told apart by the final byte alone.
		assert!(scan(b"\x1b[2*x").is_empty());
		assert_eq!(scan(b"\x1b[2*y").len(), 1);
	}

	#[test]
	fn a_self_test_is_not_mistaken_for_a_checksum() {
		// `CSI 2;1 y` with no intermediate is DECTST, which orders a terminal to run its power-up
		// self-test. Claiming it here would answer a question nobody asked with a number.
		assert!(scan(b"\x1b[2;1y").is_empty());
		assert!(scan(b"\x1b[1;1;1;1;1;1$y").is_empty());
	}

	/// SL and SR, and the direction each of them names (§100).
	#[test]
	fn a_shift_reads_its_direction_and_its_column_count() {
		assert_eq!(
			scan(b"\x1b[3 @"),
			vec![(
				5,
				RectRequest::Shift {
					direction: RectDirection::Left,
					columns: 3
				}
			)]
		);
		assert_eq!(
			scan(b"\x1b[3 A"),
			vec![(
				5,
				RectRequest::Shift {
					direction: RectDirection::Right,
					columns: 3
				}
			)]
		);
	}

	/// One column is the default, and a literal `0` is the same request — the reading every
	/// `Ps`-counted movement gets, and the one the engine would have given these parameters itself.
	#[test]
	fn a_shift_with_no_count_moves_one_column() {
		for sequence in [&b"\x1b[ @"[..], &b"\x1b[0 @"[..]] {
			assert_eq!(
				scan(sequence),
				vec![(
					sequence.len(),
					RectRequest::Shift {
						direction: RectDirection::Left,
						columns: 1
					}
				)],
				"{sequence:?}"
			);
		}
	}

	/// DECIC and DECDC (§102), under an apostrophe intermediate `vte` has no arm for at all — so
	/// unlike SL, SR and UNSCROLL these have no engine behaviour to be mistaken for, only each other.
	#[test]
	fn a_column_insert_and_delete_read_their_counts_and_their_direction() {
		assert_eq!(
			scan(b"\x1b[3'}"),
			vec![(
				5,
				RectRequest::Columns {
					columns: 3,
					insert: true
				}
			)]
		);
		assert_eq!(
			scan(b"\x1b[2'~"),
			vec![(
				5,
				RectRequest::Columns {
					columns: 2,
					insert: false
				}
			)]
		);
		// Omitted and zero are both one column, as they are for every other `Ps`-counted operation.
		assert_eq!(
			scan(b"\x1b['}"),
			vec![(
				4,
				RectRequest::Columns {
					columns: 1,
					insert: true
				}
			)]
		);
		assert_eq!(
			scan(b"\x1b[0'~"),
			vec![(
				5,
				RectRequest::Columns {
					columns: 1,
					insert: false
				}
			)]
		);
		assert!(scan(b"\x1b[3}").is_empty(), "no intermediate, not ours");
		assert!(scan(b"\x1b[3 }").is_empty(), "a different intermediate");
		assert!(scan(b"\x1b[?3'}").is_empty(), "a marker rules it out");
	}

	/// UNSCROLL (§101), and the neighbour it must not be read as: `CSI Ps T` is SD, which the engine
	/// implements and which fills with BLANKS — the one behaviour this sequence exists to avoid.
	#[test]
	fn an_unscroll_reads_its_line_count_and_is_not_a_plain_scroll_down() {
		assert_eq!(
			scan(b"\x1b[3+T"),
			vec![(5, RectRequest::Unscroll { lines: 3 })]
		);
		assert_eq!(
			scan(b"\x1b[+T"),
			vec![(4, RectRequest::Unscroll { lines: 1 })]
		);
		assert_eq!(
			scan(b"\x1b[0+T"),
			vec![(5, RectRequest::Unscroll { lines: 1 })]
		);
		assert!(scan(b"\x1b[3T").is_empty(), "SD is the engine's");
		assert!(scan(b"\x1b[3 T").is_empty(), "a different intermediate");
		assert!(scan(b"\x1b[?3+T").is_empty(), "a marker rules it out");
	}

	/// The ordinary case, and the one worth pinning hardest: when the scrollback holds everything
	/// asked for, unscrolling moves **no line number at all**. The document only gets shorter at the
	/// end, so every mark, picture and path anchored above the seam stays exactly where it was.
	#[test]
	fn an_unscroll_the_scrollback_can_fill_moves_no_line_number() {
		// Five lines of history, a four-row page, two lines asked for and two available.
		let moved = Unscrolled::new(5, 4, 2, 2);
		assert_eq!(moved.shift, 0);
		for line in 0..7 {
			assert_eq!(moved.map(line), Some(line), "line {line} must not move");
		}
		// The page's bottom two rows are gone: absolute 7 and 8 name nothing now.
		assert_eq!(moved.map(7), None);
		assert_eq!(moved.map(8), None);
	}

	/// And the case that does move numbers: more lines were asked for than the scrollback holds, so
	/// the difference arrives as blanks — which INSERTS lines, pushing everything below them down.
	#[test]
	fn an_unscroll_the_scrollback_cannot_fill_pushes_the_rest_down() {
		// One line of history, a four-row page, three asked for: one restored, two blanks.
		let moved = Unscrolled::new(1, 4, 3, 1);
		assert_eq!((moved.cut, moved.shift, moved.discard_from), (0, 2, 2));
		// The restored history line and the page's first surviving row both move down by the two
		// blanks that were inserted above them.
		assert_eq!(moved.map(0), Some(2));
		assert_eq!(moved.map(1), Some(3));
		// The page's last three rows went off the bottom.
		assert_eq!(moved.map(2), None);
		assert_eq!(moved.map(4), None);
	}

	/// The near misses, and they are the most dangerous in this module: both neighbours are sequences
	/// the ENGINE implements and every program uses, one intermediate byte away.
	#[test]
	fn the_two_sequences_a_shift_sits_beside_are_left_to_the_engine() {
		assert!(scan(b"\x1b[3@").is_empty(), "ICH — insert blanks");
		assert!(scan(b"\x1b[3A").is_empty(), "CUU — move the cursor up");
		assert!(
			scan(b"\x1b[3$@").is_empty(),
			"another intermediate entirely"
		);
		assert!(scan(b"\x1b[?3 @").is_empty(), "a marker rules it out");
		assert!(
			scan(b"\x1b[3  @").is_empty(),
			"two intermediates is not one"
		);
	}

	#[test]
	fn the_checksum_is_the_negated_sum() {
		// `AB` is 0x41 + 0x42 = 0x83, and DEC reports the negative of it. Getting this backwards is
		// the single most likely way to ship a checksum that is wrong in every case at once.
		let mut checksum = Checksum::default();
		checksum.cell(0x41);
		checksum.cell(0x42);
		assert_eq!(checksum.finish(), 0xff7d);
	}

	#[test]
	fn a_plain_space_is_trimmed_and_the_first_cell_is_not() {
		// ` ` ` ` `A`: the leading space counts because it is first, the second is dropped, and `A`
		// counts because it is not a space. 0x20 + 0x41 = 0x61.
		let mut checksum = Checksum::default();
		checksum.cell(0x20);
		checksum.cell(0x20);
		checksum.cell(0x41);
		assert_eq!(checksum.finish(), 0xff9f);
		// The exemption is for the rectangle, not for each row — feeding a whole row of blanks
		// leaves the total at one space however many there were.
		let mut row = Checksum::default();
		for _ in 0..80 {
			row.cell(0x20);
		}
		assert_eq!(row.finish(), 0xffe0);
	}

	#[test]
	fn a_blank_that_can_be_seen_is_not_trimmed() {
		// An underlined space weighs 0x20 + 0x10, which is not 0x20, so it survives the trim. That
		// is the whole reason the comparison is against the finished value rather than the glyph.
		let mut checksum = Checksum::default();
		checksum.cell(0x41);
		checksum.cell(0x30);
		assert_eq!(checksum.finish(), 0xff8f);
	}

	#[test]
	fn a_rectangle_with_no_cells_checksums_as_zero() {
		assert_eq!(Checksum::default().finish(), 0);
	}

	#[test]
	fn the_total_wraps_at_sixteen_bits() {
		// A big enough rectangle overflows, and the report is the low sixteen bits of the sum,
		// negated. 0xff00 twice is 0x1fe00, so the sum is 0xfe00 and the report 0x0200.
		let mut checksum = Checksum::default();
		checksum.cell(0xff00);
		checksum.cell(0xff00);
		assert_eq!(checksum.finish(), 0x0200);
	}

	#[test]
	fn the_report_echoes_the_id_and_four_hex_digits() {
		// `DCS Pid ! ~ XXXX ST`. Four digits always, upper case always: a program reading a fixed
		// width would take a short number as a short reply.
		assert_eq!(checksum_reply(42, 0xff7d), b"\x1bP42!~FF7D\x1b\\".to_vec());
		assert_eq!(checksum_reply(0, 0), b"\x1bP0!~0000\x1b\\".to_vec());
		assert_eq!(checksum_reply(7, 0x000a), b"\x1bP7!~000A\x1b\\".to_vec());
	}

	#[test]
	fn an_area_reports_its_own_size() {
		let source = boxed(box_of(2, 3, 5, 7), 24, 80).expect("inside the page");
		assert_eq!(source.height(), 4);
		assert_eq!(source.width(), 5);
		assert_eq!(source.rows().count(), 4);
		assert_eq!(source.columns().count(), 5);
	}
}

// term/lineattr.rs — the double-height and double-width lines (PLAN §146).
//
//   ESC # 3    DECDHL — this line is the TOP half of a double-height, double-width line
//   ESC # 4    DECDHL — this line is the BOTTOM half of one
//   ESC # 5    DECSWL — this line is back to single width and single height
//   ESC # 6    DECDWL — this line is double width, single height
//
// A line attribute, in DEC's own words. It belongs to the LINE the cursor is on and it says nothing
// about the characters in it: the same bytes are on the grid either way, and what changes is how
// wide and how tall they are drawn. A double-height line is written TWICE by the program — once with
// `ESC # 3` on the upper row and once with `ESC # 4` on the lower — and the terminal draws the top
// half of the glyphs on one and the bottom half on the other.
//
// `vte` has one arm for the `#` intermediate, `('8', [b'#'])` for DECALN, and sends `3` through `6`
// to `unhandled!()`. So these are found beside the stream like everything else here.
//
// THIS IS A RENDERING RULE AND NOT A WRITE TO THE GRID, which is the whole reason it is buildable
// without becoming a second writer of engine state (§71, §73). §76 drew that line first, for the
// character path: "the grid stays in the order the host sent, and the mirroring is a rule the
// RENDERER applies when it derives a frame from it". The same holds here — the scrollback, the
// search, a copy and the selection's own text all go on reading the grid as the host wrote it, and
// the only thing that knows about a line's size is the code that turns cells into pixels.
//
// So the state is keyed by ABSOLUTE DOCUMENT LINE (§40), exactly as `scp::Paths` is and for the same
// reason: a line keeps its attribute as the viewport scrolls under it.
//
// WHAT IS EXACT AND WHAT IS NOT, measured rather than guessed.
//
// DECDHL is a DOUBLE-HEIGHT, DOUBLE-WIDTH line — both axes by the same factor — so it is a UNIFORM
// scale of 2, and iced can express exactly that: the text is drawn at twice the font size with twice
// the line height, positioned so the wanted half lands in the row, and clipped to the row. Nothing
// is approximated.
//
// DECDWL is double width and SINGLE height, which is an anisotropic scale, and iced 0.14 has no way
// to express one for text. `Transformation::scale` takes a single factor
// (`iced_core/src/transformation.rs`), and the wgpu text pipeline reduces whatever transformation is
// in force to `scale: transformation.scale_factor() * layer_transformation.scale_factor()` before
// handing it to glyphon (`iced_wgpu/src/text.rs:625-626`) — one number, both axes.
//
// So a DECDWL line is drawn with its CELLS twice as wide and its glyphs at the normal size, one cell
// per draw. The layout is right — half as many columns are visible, and a program that mixes a
// double-width title with single-width lines below it gets its columns lining up — and the glyphs
// are not fattened. That is the divergence, and it is chosen over the two alternatives rather than
// fallen into: drawing at 2× and clipping to one row cuts every glyph in half, and ignoring the
// sequence puts the title at the full width the program did not ask for, which breaks the layout
// that is the thing programs actually depend on here.

use std::collections::BTreeMap;

/// How many document lines may carry an attribute at once.
///
/// The same bound and the same reason as `scp::Paths`' (§76, §12): a remote can write one of these
/// per line for as long as it likes, and cmote must not grow a map without limit on its say-so. It is
/// comfortably deeper than the scrollback the viewport can reach, so dropping the oldest entry drops
/// history nobody can look at.
const MAX_LINES: usize = 20_000;

/// The size one line is drawn at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineAttribute {
	/// DECSWL — one cell per column, one row tall. Every line, until a program says otherwise.
	#[default]
	Single,
	/// DECDWL — two cells wide per column, one row tall.
	DoubleWidth,
	/// DECDHL, the upper half: two cells wide, and the TOP half of a double-height glyph.
	DoubleTop,
	/// DECDHL, the lower half: the same, showing the BOTTOM half.
	DoubleBottom,
}

impl LineAttribute {
	/// How many grid columns one of this line's cells covers on screen — 1 or 2.
	///
	/// All three double forms are double WIDTH; only the height differs between them. That is DEC's
	/// own arrangement and not a simplification: there is no double-height-single-width line.
	pub fn columns(self) -> u16 {
		if self == Self::Single { 1 } else { 2 }
	}

	/// Whether the glyphs are drawn at twice the font size — true for the two halves of a
	/// double-height line, false for a double-WIDTH one, which is where iced's uniform-only text
	/// scale stops cmote (see the module header).
	pub fn is_tall(self) -> bool {
		matches!(self, Self::DoubleTop | Self::DoubleBottom)
	}

	/// Whether this is the LOWER half of a double-height line, which is drawn from a row higher up so
	/// that the bottom of the glyph lands in it.
	pub fn is_lower_half(self) -> bool {
		self == Self::DoubleBottom
	}
}

/// Which document lines are drawn at what size (§146).
///
/// Only the lines that are NOT single are held, so an ordinary session's map stays empty and every
/// lookup is one miss in an empty tree.
#[derive(Debug, Default)]
pub struct LineSizes {
	attributes: BTreeMap<u64, LineAttribute>,
}

impl LineSizes {
	/// Give one document line an attribute. [`LineAttribute::Single`] forgets it rather than storing
	/// it, which is what keeps the map the size of what a program actually asked for.
	pub fn set(&mut self, line: u64, attribute: LineAttribute) {
		if attribute == LineAttribute::Single {
			self.attributes.remove(&line);
			return;
		}
		self.attributes.insert(line, attribute);
		// Bounded like the character paths' set (§76) and the prompt marks' ring (§34). The lowest
		// line number is the oldest, so this drops history the viewport can no longer reach before it
		// drops anything on screen.
		while self.attributes.len() > MAX_LINES {
			self.attributes.pop_first();
		}
	}

	/// The size this document line is drawn at.
	pub fn of(&self, line: u64) -> LineAttribute {
		self.attributes
			.get(&line)
			.copied()
			.unwrap_or(LineAttribute::Single)
	}

	/// Move every remembered line through a renumbering of the document (§101).
	///
	/// UNSCROLL is the one sequence that renumbers without clearing: it moves lines from the
	/// scrollback back onto the page, so a line's size has to travel with it. The map is rebuilt
	/// rather than edited in place, because the shift can move a line onto a number the map already
	/// holds and an in-place walk would then read its own output — `scp::Paths::renumber`'s reason,
	/// word for word.
	pub fn renumber(&mut self, remap: impl Fn(u64) -> Option<u64>) {
		self.attributes = self
			.attributes
			.iter()
			.filter_map(|(line, attribute)| remap(*line).map(|line| (line, *attribute)))
			.collect();
	}

	/// Forget every attribute. RIS renumbers the document by dropping the history, and a swap on or
	/// off the alternate screen restarts numbering against a page that keeps none — in both cases a
	/// remembered line number would now name different text.
	pub fn clear(&mut self) {
		self.attributes.clear();
	}
}

/// The line-attribute scanner (§146). Feed it every byte of shell output; it reports where each
/// sequence sat and which size it asked for.
///
/// The escape grammar is [`super::dcs::Framer`]'s (§111); what is left here is one question, which of
/// four final bytes under the `#` intermediate arrived.
///
/// The cap is zero: this scanner reads no control string, so no payload is buffered on its account.
#[derive(Debug, Default)]
pub struct LineAttributes {
	escapes: super::dcs::Framer<0>,
}

impl LineAttributes {
	/// Scan a chunk of shell output, returning each attribute and where it sat. Safe at any chunk
	/// boundary — the state machine carries over between calls, so a sequence may be split anywhere,
	/// including between the ESC and the `#`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the character path's (§76). What the
	/// offset is FOR is the same as SCP's too: the sequence names no line of its own, it acts on the
	/// one the cursor is on, so the engine has to be at the sequence before the cursor is read. Which
	/// side of the final byte does not matter — the sequence prints nothing and moves nothing — and
	/// one past is what the scanners around it use.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, LineAttribute)> {
		let mut requests = Vec::new();
		self.escapes.feed(bytes, |span, control| {
			if let super::dcs::Control::Escape(escape) = control
				&& escape.intermediates() == *b"#"
				&& let Some(attribute) = attribute(escape.final_byte())
			{
				requests.push((span.past(), attribute));
			}
		});
		requests
	}
}

/// Which attribute a final byte under the `#` intermediate asks for, or `None` for one of the
/// family's other members.
///
/// The near miss worth naming: `ESC # 8` is DECALN, the screen alignment test, which the ENGINE
/// implements — it fills the page with `E`. An intermediate-only match would read a conformance
/// suite's alignment test as a line attribute and leave the page double width. `ESC # 1` and
/// `ESC # 2` are DECDHL's and DECSWL's own neighbours on a VT52-era table and are not defined here,
/// so they leave the line as it was.
fn attribute(final_byte: u8) -> Option<LineAttribute> {
	match final_byte {
		b'3' => Some(LineAttribute::DoubleTop),
		b'4' => Some(LineAttribute::DoubleBottom),
		b'5' => Some(LineAttribute::Single),
		b'6' => Some(LineAttribute::DoubleWidth),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go.
	fn scan(bytes: &[u8]) -> Vec<(usize, LineAttribute)> {
		LineAttributes::default().feed(bytes)
	}

	#[test]
	fn each_attribute_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b#3"), vec![(3, LineAttribute::DoubleTop)]);
		assert_eq!(scan(b"ab\x1b#4"), vec![(5, LineAttribute::DoubleBottom)]);
		assert_eq!(scan(b"\x1b#5"), vec![(3, LineAttribute::Single)]);
		assert_eq!(scan(b"\x1b#6"), vec![(3, LineAttribute::DoubleWidth)]);
	}

	/// The near miss this scanner is built around: `ESC # 8` is DECALN, which the ENGINE performs.
	/// Reading it as a line attribute would leave a conformance suite's alignment page double width.
	#[test]
	fn the_alignment_test_is_not_a_line_attribute() {
		assert!(scan(b"\x1b#8").is_empty());
		assert!(scan(b"\x1b#1").is_empty(), "and nor is an undefined final");
		assert!(scan(b"\x1b#2").is_empty());
		assert!(scan(b"\x1b#7").is_empty());
	}

	/// The intermediate is what makes it this family — `ESC 6` and `ESC 3` without it are somebody
	/// else's, and `ESC 6` in particular is DECBI, which cmote performs (§112).
	#[test]
	fn the_hash_intermediate_is_required() {
		assert!(scan(b"\x1b6").is_empty(), "DECBI");
		assert!(scan(b"\x1b3").is_empty());
		assert!(scan(b"\x1b(6").is_empty(), "a charset slot is not a hash");
		assert!(scan(b"\x1b##6").is_empty(), "two hashes is not one");
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scanner = LineAttributes::default();
		assert!(scanner.feed(b"\x1b").is_empty());
		assert!(scanner.feed(b"#").is_empty());
		assert_eq!(scanner.feed(b"6"), vec![(1, LineAttribute::DoubleWidth)]);
	}

	/// `ESC` then a C0 stays in the escape state for the engine, so an attribute with a line feed in
	/// the middle of it still applies (§111).
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert_eq!(scan(b"\x1b\n#6").len(), 1, "LF is read through");
		assert!(scan(b"\x1b\x18#6").is_empty(), "CAN cancels");
	}

	/// All three double forms are double WIDTH; only the height differs. There is no
	/// double-height-single-width line, which is DEC's arrangement and not a simplification here.
	#[test]
	fn every_double_form_is_two_columns_wide() {
		assert_eq!(LineAttribute::Single.columns(), 1);
		for attribute in [
			LineAttribute::DoubleWidth,
			LineAttribute::DoubleTop,
			LineAttribute::DoubleBottom,
		] {
			assert_eq!(attribute.columns(), 2, "{attribute:?}");
		}
	}

	/// The measured split (see the module header): the two halves of a double-HEIGHT line are a
	/// uniform 2× scale, which iced can express, and a double-WIDTH line is not.
	#[test]
	fn only_the_double_height_halves_are_drawn_at_twice_the_font_size() {
		assert!(LineAttribute::DoubleTop.is_tall());
		assert!(LineAttribute::DoubleBottom.is_tall());
		assert!(!LineAttribute::DoubleWidth.is_tall());
		assert!(!LineAttribute::Single.is_tall());
		// And only one of the two halves is drawn from a row higher up.
		assert!(LineAttribute::DoubleBottom.is_lower_half());
		assert!(!LineAttribute::DoubleTop.is_lower_half());
	}

	#[test]
	fn a_line_keeps_its_attribute_and_single_forgets_it() {
		let mut sizes = LineSizes::default();
		assert_eq!(sizes.of(7), LineAttribute::Single, "nothing is stored");
		sizes.set(7, LineAttribute::DoubleTop);
		assert_eq!(sizes.of(7), LineAttribute::DoubleTop);
		assert_eq!(sizes.of(8), LineAttribute::Single, "and only that line");
		sizes.set(7, LineAttribute::Single);
		assert_eq!(sizes.of(7), LineAttribute::Single);
		assert!(
			sizes.attributes.is_empty(),
			"DECSWL forgets rather than storing"
		);
	}

	/// A remote can write one of these per line for as long as it likes, and cmote must not grow a map
	/// without limit on its say-so (§12). The oldest lines go first, which is history the viewport can
	/// no longer reach.
	#[test]
	fn the_map_is_bounded_and_drops_the_oldest_first() {
		let mut sizes = LineSizes::default();
		for line in 0..u64::try_from(MAX_LINES).expect("the bound fits a u64") + 100 {
			sizes.set(line, LineAttribute::DoubleWidth);
		}
		assert_eq!(sizes.attributes.len(), MAX_LINES);
		assert_eq!(sizes.of(0), LineAttribute::Single, "the oldest went");
		assert_eq!(
			sizes.of(u64::try_from(MAX_LINES).expect("fits") + 99),
			LineAttribute::DoubleWidth,
			"and the newest stayed"
		);
	}

	/// UNSCROLL moves lines from the scrollback back onto the page, so a line's size travels with it
	/// (§101). A line the remap drops is forgotten.
	#[test]
	fn a_renumbering_carries_every_attribute_with_its_line() {
		let mut sizes = LineSizes::default();
		sizes.set(10, LineAttribute::DoubleTop);
		sizes.set(11, LineAttribute::DoubleBottom);
		sizes.renumber(|line| (line != 11).then_some(line + 5));
		assert_eq!(sizes.of(15), LineAttribute::DoubleTop);
		assert_eq!(sizes.of(10), LineAttribute::Single, "it moved, not copied");
		assert_eq!(sizes.of(16), LineAttribute::Single, "and 11 was dropped");
	}
}

// palette.rs — the terminal's colour scheme, one source of truth for the whole app (§9, §23).
//
// Two places must agree on colour: the renderer (`ui::grid`) that paints each cell, and the
// query answerer (`term`) that tells a program "this is my foreground / background / palette
// slot N" when it asks (OSC 10 / 11 / 12 / 4). Resolving both here means the answer a program
// reads back is exactly what the grid draws — a terminal that lied about its own colours would
// break the very colour-scheme detection the query exists for. Colours are plain `(u8, u8, u8)`
// RGB with no dependency on iced or the engine; each caller converts to the type it needs.

/// The default foreground and background of the light-on-dark scheme: what a cell asking for
/// the terminal "default" colour resolves to, and what OSC 10 (foreground) / OSC 11
/// (background) report to a program that queries them.
pub const DEFAULT_FG: (u8, u8, u8) = (0xd0, 0xd0, 0xd0);
pub const DEFAULT_BG: (u8, u8, u8) = (0x1e, 0x1e, 0x1e);

/// The page's own two colours, swapped while DECSCNM is on (§149).
///
/// Here rather than in either module that needs it, and for this file's founding reason one turn
/// further round: the renderer and the RICH COPY both draw the page's own background, and a rule
/// about which colour that is has to be stated once or the two eventually disagree. It is three
/// lines; what it buys is that "reversed" cannot mean one thing on screen and another on the
/// clipboard.
///
/// The mode itself is `term/decmodes.rs`'s. This is only the consequence.
pub fn page_colors(reversed: bool) -> ((u8, u8, u8), (u8, u8, u8)) {
	if reversed {
		(DEFAULT_BG, DEFAULT_FG)
	} else {
		(DEFAULT_FG, DEFAULT_BG)
	}
}

/// The 16 base ANSI colours (indices 0-15): the 8 standard colours then their bright variants.
///
/// Chosen against `DEFAULT_BG` rather than inherited (§159). What was here before was the VGA
/// palette — `0x80` primaries — under a comment claiming it was xterm's, and on a `#1e1e1e` page
/// nine of its fifteen ink slots fell below the 4.5:1 that `every_ink_slot_is_legible_on_the_scheme_background`
/// now holds them to. Blue reached **1.04:1**, which against a ceiling of 1.0 is not dim, it is gone.
///
/// Blue is the slot that forces the shape of the whole table. Luminance weighs it at 0.0722, an
/// eleventh of green, so NO navy clears the floor — not xterm's `#0000ee` (1.77), not Campbell's
/// `#0037da` (2.03). Fixing the reported complaint at all means slot 4 stops being a navy, and once
/// it has moved a palette half in VGA and half not would read as an accident, so all of them moved
/// together.
///
/// Two properties the numbers alone would not keep, and each one costs a slot's freedom:
///
/// * **a bright is brighter than its normal.** cmote draws SGR 1 as a heavier FONT and never as a
///   colour, so 30-37 and 90-97 are the only way a program has of asking for the two, and a palette
///   that let them collide (several fashionable ones do) would silently merge them.
/// * **slot 7 is not `DEFAULT_FG`.** It stays at `0xc0` — it cleared the floor at 9.16 and needed no
///   lifting — so that `SGR 37` remains distinguishable from "the terminal's own foreground". A
///   white that equals the default reads identically and takes an expression away for nothing.
///
/// Slot 0 keeps `#000000` and is exempt from the floor: it is the colour a program picks to paint a
/// background, or to write ON one of the light slots, and it is darker than the page by design.
/// Lifting it would make black-on-default faintly legible at the price of black no longer being black.
///
/// Three slots — bright green, yellow and cyan — were changed with no contrast argument behind them
/// and LOST some (12.15 to 10.20, 15.52 to 11.77, 13.30 to 11.67). That is worth admitting rather
/// than dressing up: a pure `#00ff00` beside a `#5fb85f` reads as a colour from a different table,
/// not as the same green turned up, and the bright row is only useful to a program if it is legible
/// as a row. All three keep more than twice the floor, so the trade buys coherence at no risk.
const ANSI_16: [(u8, u8, u8); 16] = [
	(0x00, 0x00, 0x00), // 0 black       — exempt, see above
	(0xe2, 0x60, 0x6b), // 1 red         was 0x800000, 1.52:1
	(0x5f, 0xb8, 0x5f), // 2 green       was 0x008000, 3.25:1
	(0xd6, 0xa0, 0x2a), // 3 yellow      was 0x808000, 3.97:1
	(0x5a, 0xa9, 0xf0), // 4 blue        was 0x000080, 1.04:1 — the reported one
	(0xd0, 0x7f, 0xdd), // 5 magenta     was 0x800080, 1.77:1
	(0x3f, 0xc0, 0xc8), // 6 cyan        was 0x008080, 3.49:1
	(0xc0, 0xc0, 0xc0), // 7 white       unchanged, 9.16:1
	(0x8b, 0x8b, 0x8b), // 8 bright black (gray) was 0x808080, 4.22:1
	(0xff, 0x7d, 0x88), // 9 bright red  was 0xff0000, 4.17:1
	(0x7e, 0xe0, 0x7e), // 10 bright green   was 0x00ff00, 12.15:1 — softened, see below
	(0xf5, 0xd7, 0x6e), // 11 bright yellow  was 0xffff00, 15.52:1 — softened
	(0x8f, 0xc6, 0xff), // 12 bright blue    was 0x0000ff, 1.94:1
	(0xef, 0x9b, 0xf5), // 13 bright magenta was 0xff00ff, 5.32:1
	(0x6f, 0xea, 0xf0), // 14 bright cyan    was 0x00ffff, 13.30:1 — softened
	(0xff, 0xff, 0xff), // 15 bright white
];

/// The six intensity steps of the 6×6×6 colour cube (indices 16-231).
const CUBE_STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];

/// Resolve an xterm 256-colour index to RGB: 0-15 base ANSI, 16-231 a 6×6×6 cube, 232-255 a
/// 24-step grayscale ramp.
pub fn xterm_256(index: u8) -> (u8, u8, u8) {
	if index < 16 {
		return ANSI_16[index as usize];
	}
	if index < 232 {
		let value = index - 16;
		let r = CUBE_STEPS[(value / 36) as usize];
		let g = CUBE_STEPS[((value / 6) % 6) as usize];
		let b = CUBE_STEPS[(value % 6) as usize];
		return (r, g, b);
	}
	let level = 8 + (index - 232) * 10;
	(level, level, level)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// WCAG's floor for body text. The palette is chosen to clear it against `DEFAULT_BG` (§159).
	const CONTRAST_FLOOR: f64 = 4.5;

	/// One channel of sRGB, undone back to light. The transfer curve is not a plain power — it has a
	/// straight segment at the bottom — and the whole point of the exercise is the very dark end, so
	/// the piecewise form is the one to write.
	fn linear(channel: u8) -> f64 {
		let value = f64::from(channel) / 255.0;
		if value <= 0.04045 {
			value / 12.92
		} else {
			((value + 0.055) / 1.055).powf(2.4)
		}
	}

	/// How much light a colour puts out, weighted the way an eye receives it: green carries most of
	/// it and blue barely a fourteenth, which is the whole reason a navy cannot be read on a dark page.
	fn relative_luminance((red, green, blue): (u8, u8, u8)) -> f64 {
		0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
	}

	/// The WCAG contrast ratio of two colours: 1.0 when they are the same light, 21.0 for black on
	/// white. Order does not matter.
	fn contrast(first: (u8, u8, u8), second: (u8, u8, u8)) -> f64 {
		let (a, b) = (relative_luminance(first), relative_luminance(second));
		(a.max(b) + 0.05) / (a.min(b) + 0.05)
	}

	#[test]
	fn the_formula_agrees_with_the_two_ratios_everybody_knows() {
		// Anchoring the measuring stick before measuring anything with it. Black on white is the
		// definition's own maximum, and a colour against itself is its minimum — if the transfer
		// curve or the weights were wrong, one of these two would move.
		let ends = contrast((0x00, 0x00, 0x00), (0xff, 0xff, 0xff));
		assert!(
			(ends - 21.0).abs() < 0.01,
			"black on white is 21:1, got {ends}"
		);
		let same = contrast(DEFAULT_BG, DEFAULT_BG);
		assert!(
			(same - 1.0).abs() < 0.001,
			"a colour on itself is 1:1, got {same}"
		);
	}

	#[test]
	fn every_ink_slot_is_legible_on_the_scheme_background() {
		// The rule §159 settled on, kept as a rule rather than as sixteen pinned hex values — the
		// values are a consequence and anybody may retune them, but not below this. What made the
		// old palette a bug report was blue at 1.04:1 against a ceiling of 1.0, and no test could
		// have caught that because no test knew what the table was FOR.
		//
		// From 1: slot 0 is the colour a program paints a background with, and it is meant to be
		// darker than the page. Holding it to a floor would mean deleting black.
		for index in 1..16_u8 {
			let colour = xterm_256(index);
			let ratio = contrast(colour, DEFAULT_BG);
			assert!(
				ratio >= CONTRAST_FLOOR,
				"slot {index} {colour:?} is {ratio:.2}:1 on the page, under the {CONTRAST_FLOOR}:1 floor"
			);
		}
	}

	#[test]
	fn a_bright_is_brighter_than_its_normal_and_white_is_not_the_default_ink() {
		// The two properties the floor alone does not give (§159). cmote draws SGR 1 as a heavier
		// font and never as a colour change, so 30-37 and 90-97 are all a program has to ask for the
		// pair with; a palette whose bright equalled its normal would merge them with no way to tell.
		for index in 1..8_usize {
			let (normal, bright) = (ANSI_16[index], ANSI_16[index + 8]);
			assert!(
				relative_luminance(bright) > relative_luminance(normal),
				"slot {} {bright:?} must out-light slot {index} {normal:?}",
				index + 8
			);
		}
		// And SGR 37 has to stay distinguishable from "whatever the terminal's own ink is", or the
		// palette has spent a slot saying nothing.
		assert_ne!(ANSI_16[7], DEFAULT_FG);
	}

	#[test]
	fn the_cube_and_ramp_land_on_their_known_anchors() {
		// A few fixed points of the xterm-256 palette, so a refactor of the arithmetic cannot
		// quietly shift a colour: slot 0 is black, 15 bright white, 16 the cube's origin, 231
		// its far corner (pure white), and 232/255 the ends of the grey ramp.
		assert_eq!(xterm_256(0), (0x00, 0x00, 0x00));
		assert_eq!(xterm_256(15), (0xff, 0xff, 0xff));
		assert_eq!(xterm_256(16), (0x00, 0x00, 0x00));
		assert_eq!(xterm_256(231), (0xff, 0xff, 0xff));
		assert_eq!(xterm_256(232), (8, 8, 8));
		assert_eq!(xterm_256(255), (238, 238, 238));
	}
}

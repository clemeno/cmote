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

/// The 16 base ANSI colours (indices 0-15): the 8 standard colours then their bright variants.
/// Values follow the common xterm palette.
const ANSI_16: [(u8, u8, u8); 16] = [
	(0x00, 0x00, 0x00), // 0 black
	(0x80, 0x00, 0x00), // 1 red
	(0x00, 0x80, 0x00), // 2 green
	(0x80, 0x80, 0x00), // 3 yellow
	(0x00, 0x00, 0x80), // 4 blue
	(0x80, 0x00, 0x80), // 5 magenta
	(0x00, 0x80, 0x80), // 6 cyan
	(0xc0, 0xc0, 0xc0), // 7 white
	(0x80, 0x80, 0x80), // 8 bright black (gray)
	(0xff, 0x00, 0x00), // 9 bright red
	(0x00, 0xff, 0x00), // 10 bright green
	(0xff, 0xff, 0x00), // 11 bright yellow
	(0x00, 0x00, 0xff), // 12 bright blue
	(0xff, 0x00, 0xff), // 13 bright magenta
	(0x00, 0xff, 0xff), // 14 bright cyan
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

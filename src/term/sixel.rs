// term/sixel.rs — decode a sixel image into pixels (PLAN §41).
//
// Sixel is DEC's raster format for a text terminal: the image travels inside a DCS string
// (`DCS <params> q <payload> ST`) as printable ASCII, so it survives a pty and an SSH channel
// untouched. This module is the PURE half of cmote's image support — payload bytes in, RGBA
// pixels out. It knows nothing about the grid, the engine or the screen; `term::graphics` finds
// the payload in the stream and decides where the picture goes.
//
// The name is the unit: one character encodes SIX vertical pixels. Take a printable byte in
// `?`..`~`, subtract `?` (0x3F), and the low six bits say which of six stacked pixels are on —
// bit 0 the topmost. Characters march left to right across a BAND six pixels tall, and `-` drops
// to the next band. So a 100×60 picture is 10 bands of 100 characters, each character a column
// of six. Around that sit four commands:
//
//   " Pan;Pad;Ph;Pv   raster attributes — the pixel aspect ratio, then the image's size
//   # Pc              select colour register Pc for the sixels that follow
//   # Pc;Pu;Px;Py;Pz  define register Pc — Pu 1 = HLS, 2 = RGB
//   ! Pn <sixel>      repeat the next sixel character Pn times (the format's whole compression)
//   $                 carriage return — back to the left edge of the CURRENT band
//   -                 new line — down one band, back to the left edge
//
// Decoding runs the payload TWICE. The first walk measures (or reads the raster attributes) so
// the canvas can be allocated once, at a size we have already bounded; the second paints into it.
// The alternative — growing a row-major RGBA buffer sideways as the picture reveals its width —
// would restride every row on every growth, and would size an allocation from a number the remote
// chose before checking it. The grammar is written once (`walk`) and both passes are just
// different sinks for the commands it emits, so the two can never disagree about what the bytes
// mean.
//
// SECURITY (§12): every number here comes off the wire from a remote host, so nothing is
// allocated on its word alone. Parameters saturate instead of wrapping, an image past the caps
// below is refused outright rather than clipped (a refusal draws nothing; a clip would silently
// misreport what the host sent), and painting is bounds-checked per pixel so a raster attribute
// that disagrees with the payload's real extent can only lose pixels, never write past the canvas.

/// The widest and tallest image cmote will decode, and the most pixels it will hold whatever the
/// shape. 4 Mpx is a 16 MB RGBA canvas — comfortably past any terminal-sized picture (a 1920×1080
/// photo is 2.1 Mpx) while keeping a single hostile `"1;1;65535;65535` from asking for 17 GB.
/// Reported to a program that asks with XTSMGRAPHICS (§41), so a well-behaved sender knows the
/// limit before it sends.
pub const MAX_WIDTH: u16 = 4096;
pub const MAX_HEIGHT: u16 = 4096;
pub const MAX_PIXELS: usize = 4_194_304;

/// How many colour registers the palette holds. 256 is the VT340's count and what cmote reports to
/// XTSMGRAPHICS; a register index past it is clamped, so a sender using more collides two colours
/// rather than reaching past the palette.
pub const COLOR_REGISTERS: usize = 256;

/// How many parameters one command's number run keeps. The longest is a colour definition's five
/// (`#Pc;Pu;Px;Py;Pz`); a longer run is malformed, and the extra values are consumed but dropped.
const MAX_NUMBERS: usize = 5;

/// The first and last printable byte that carries pixels. Subtracting `SIXEL_FIRST` leaves the six
/// bits, one per pixel row of the band.
const SIXEL_FIRST: u8 = b'?';
const SIXEL_LAST: u8 = b'~';

/// How many pixel rows one sixel character stacks — the format's name, and the height of a band.
const BAND_HEIGHT: u32 = 6;

/// The VT340's sixteen default colour registers, as the RGB the percentages in DEC's table come
/// out at. A payload that paints without defining anything (`#1~`) draws in these, which is why
/// they are here rather than left black: several small emitters rely on them entirely. Registers
/// 16 and up default to black — nothing standard defines them, so a payload that selects one
/// without defining it gets an honest nothing-coloured pixel rather than a guess.
const DEFAULT_PALETTE: [[u8; 3]; 16] = [
	[0x00, 0x00, 0x00], // 0  black
	[0x33, 0x33, 0xcc], // 1  blue
	[0xcc, 0x24, 0x24], // 2  red
	[0x33, 0xcc, 0x33], // 3  green
	[0xcc, 0x33, 0xcc], // 4  magenta
	[0x33, 0xcc, 0xcc], // 5  cyan
	[0xcc, 0xcc, 0x33], // 6  yellow
	[0x87, 0x87, 0x87], // 7  grey 50%
	[0x42, 0x42, 0x42], // 8  grey 25%
	[0x54, 0x54, 0x8a], // 9  blue, dimmed
	[0x8a, 0x42, 0x42], // 10 red, dimmed
	[0x54, 0x8a, 0x54], // 11 green, dimmed
	[0x8a, 0x54, 0x8a], // 12 magenta, dimmed
	[0x54, 0x8a, 0x8a], // 13 cyan, dimmed
	[0x8a, 0x8a, 0x54], // 14 yellow, dimmed
	[0xcc, 0xcc, 0xcc], // 15 grey 75%
];

/// A decoded image: its size in pixels and its pixels, four bytes per pixel in RGBA order — the
/// layout a GPU texture wants, so `term::graphics` can hand it straight to the renderer without
/// touching it again. `rgba.len()` is always `width * height * 4`.
///
/// A pixel no sixel set is left fully TRANSPARENT (all four bytes zero) rather than filled with a
/// background colour. Sixel's second parameter nominally chooses between the two, but cmote draws
/// the picture over its own grid: transparent means the terminal's background shows through, which
/// is what "background" resolves to anyway — and it is the honest answer for the emitters that ask
/// for transparency, which is most of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
	pub width: u16,
	pub height: u16,
	pub rgba: Vec<u8>,
}

/// Decode a sixel payload — the bytes BETWEEN the `q` that opens the DCS and its terminator — into
/// pixels, or `None` when there is no picture in it: an empty payload, one that paints nothing, or
/// one whose size is past the caps this module will hold (`MAX_WIDTH`, `MAX_HEIGHT`, `MAX_PIXELS`).
///
/// The DCS's own parameters (before the `q`) are deliberately not taken: `P1` is a pixel aspect
/// ratio cmote does not apply (it draws square pixels, as every modern terminal does), `P2` is the
/// background-select this decoder answers with transparency either way (see `Image`), and `P3` is a
/// horizontal grid size no one sends. Nothing there changes the pixels, so nothing there is read.
pub fn decode(payload: &[u8]) -> Option<Image> {
	let (width, height) = canvas_size(payload)?;
	// Bounded above, so the multiplication cannot overflow a usize on either target.
	let mut rgba = vec![0u8; width as usize * height as usize * 4];
	paint(payload, &mut rgba, width, height);
	// `canvas_size` has already refused anything past `MAX_WIDTH` / `MAX_HEIGHT`, both `u16`, so these
	// fit — and answering `None` on the impossible path costs nothing, since a picture too big to hold
	// is exactly what this function already returns `None` for.
	Some(Image {
		width: u16::try_from(width).ok()?,
		height: u16::try_from(height).ok()?,
		rgba,
	})
}

/// One command from the payload. `Run` folds the plain sixel character and the `!Pn` repeat into
/// one shape — a repeat IS a run of one character — so both passes handle a single case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SixelCommand {
	/// Raster attributes: the image's own idea of its size in pixels.
	Raster { width: u16, height: u16 },
	/// Paint `count` columns of the six pixels `bits` selects, in the current register.
	Run { count: u16, bits: u8 },
	/// Select an existing colour register for the runs that follow.
	Select { register: u8 },
	/// Define a colour register: `coding` 1 is HLS, 2 is RGB, and `values` are that space's three.
	Define {
		register: u8,
		coding: u16,
		values: [u16; 3],
	},
	/// Back to the left edge of the current band, without changing band.
	CarriageReturn,
	/// Down one band (six pixel rows), back to the left edge.
	NextBand,
}

/// Walk the payload once, handing each command to `sink`. The one place the grammar is written
/// down: `canvas_size` and `paint` are both just sinks over this, so a measurement and a painting
/// of the same bytes cannot disagree.
///
/// Anything that is not a command and not a sixel character — the newlines emitters insert to keep
/// lines short, stray whitespace, a byte from a truncated command — is skipped. A tolerant reader
/// is the right call here: half a picture is worth more to the user than none, and there is nothing
/// to exploit in a byte we drop.
fn walk_payload(payload: &[u8], mut sink: impl FnMut(SixelCommand)) {
	let mut index = 0;
	while index < payload.len() {
		let byte = payload[index];
		index += 1;
		match byte {
			// `" Pan ; Pad ; Ph ; Pv` — only the size is used; the aspect ratio is not applied.
			b'"' => {
				let numbers = numbers(payload, &mut index);
				if let [_, _, width, height] = *numbers.as_slice() {
					sink(SixelCommand::Raster { width, height });
				}
			}
			// `#` is both "use this register" and "this register means this colour", told apart by
			// how many parameters follow.
			b'#' => {
				let numbers = numbers(payload, &mut index);
				match *numbers.as_slice() {
					[register] => sink(SixelCommand::Select {
						register: register_index(register),
					}),
					[register, coding, x, y, z] => sink(SixelCommand::Define {
						register: register_index(register),
						coding,
						values: [x, y, z],
					}),
					// No parameters at all, or a count nothing defines: not a command we can act on.
					_ => {}
				}
			}
			// `! Pn <sixel>` — the count, then the ONE character it repeats.
			b'!' => {
				let numbers = numbers(payload, &mut index);
				let [count] = *numbers.as_slice() else {
					continue;
				};
				// The repeated character is the next byte, so it is consumed here rather than left
				// for the loop — otherwise a `!3~` would paint the run AND a stray `~` after it.
				if let Some(&repeated) = payload.get(index) {
					index += 1;
					if let Some(bits) = sixel_bits(repeated) {
						sink(SixelCommand::Run { count, bits });
					}
				}
			}
			b'$' => sink(SixelCommand::CarriageReturn),
			b'-' => sink(SixelCommand::NextBand),
			_ => {
				if let Some(bits) = sixel_bits(byte) {
					sink(SixelCommand::Run { count: 1, bits });
				}
			}
		}
	}
}

/// The size to allocate for this payload, in pixels, or `None` when there is nothing to draw or it
/// is past the caps.
///
/// The raster attributes win when the payload states them and they are non-zero: they are the
/// sender's own crop, so a picture whose last band is only two pixels tall reports 62 rather than
/// the 66 its ten bands span, and a `$`-overprinted band does not inflate the width. Without them
/// the extent of the pixels actually PAINTED is used — measured from set bits only, so the trailing
/// blank columns emitters pad a band with do not widen the canvas.
fn canvas_size(payload: &[u8]) -> Option<(u32, u32)> {
	let mut raster = None;
	// Where the walk is, and how far it has reached: `x` counts columns from the left edge of the
	// current band, `band` counts bands from the top.
	let mut x: u32 = 0;
	let mut band: u32 = 0;
	let mut painted_width: u32 = 0;
	let mut painted_bands: u32 = 0;

	walk_payload(payload, |command| match command {
		SixelCommand::Raster { width, height } => {
			raster = Some((u32::from(width), u32::from(height)));
		}
		SixelCommand::Run { count, bits } => {
			x += u32::from(count);
			// Only a character with a bit set extends the picture; one that sets none is a step
			// across transparent space, which no more belongs to the image than the margin does.
			if bits != 0 {
				painted_width = painted_width.max(x);
				painted_bands = painted_bands.max(band + 1);
			}
		}
		SixelCommand::CarriageReturn => x = 0,
		SixelCommand::NextBand => {
			band += 1;
			x = 0;
		}
		SixelCommand::Select { .. } | SixelCommand::Define { .. } => {}
	});

	let (width, height) = match raster {
		Some((width, height)) if width > 0 && height > 0 => (width, height),
		_ => (painted_width, painted_bands * BAND_HEIGHT),
	};
	// Nothing painted and nothing declared: an empty or colour-definitions-only payload.
	if width == 0 || height == 0 {
		return None;
	}
	// Past what cmote will hold: refuse the whole picture. Clipping it would draw a lie — the user
	// would see a cropped image with nothing to say it had been cropped (§12).
	if width > u32::from(MAX_WIDTH) || height > u32::from(MAX_HEIGHT) {
		return None;
	}
	if (width as usize).saturating_mul(height as usize) > MAX_PIXELS {
		return None;
	}
	Some((width, height))
}

/// Paint the payload into an already-sized canvas. Every write is bounds-checked against
/// `width`/`height`, so a raster attribute smaller than the payload's real extent simply crops it
/// and one larger leaves the surplus transparent — either way nothing is written outside `rgba`.
fn paint(payload: &[u8], rgba: &mut [u8], width: u32, height: u32) {
	let mut palette = default_registers();
	let mut register = 0usize;
	let mut x: u32 = 0;
	let mut band: u32 = 0;

	walk_payload(payload, |command| match command {
		SixelCommand::Select { register: chosen } => register = usize::from(chosen),
		SixelCommand::Define {
			register: defined,
			coding,
			values,
		} => {
			palette[usize::from(defined)] = parse_color(coding, values);
			// A colour introducer SELECTS its register as well as defining it. That is not obvious
			// from the format's description — the two forms read like separate commands — but it is
			// what every emitter relies on: `#0;2;100;0;0~` defines red and expects the sixel right
			// after it to be red, with no `#0` in between. Without this the whole picture would paint
			// in whichever register was last selected outright, which for most payloads means all of
			// it in one colour.
			register = usize::from(defined);
		}
		SixelCommand::Run { count, bits } => {
			let ink = palette[register];
			for _ in 0..count {
				// A run can walk off the right edge (an overlong band, or a raster crop): keep
				// counting columns so a later `$` still lines up, but paint nothing out there.
				if x < width {
					for bit in 0..BAND_HEIGHT {
						if bits & (1 << bit) == 0 {
							continue;
						}
						let y = band * BAND_HEIGHT + bit;
						if y >= height {
							continue;
						}
						let offset = ((y * width + x) * 4) as usize;
						rgba[offset] = ink[0];
						rgba[offset + 1] = ink[1];
						rgba[offset + 2] = ink[2];
						// A painted pixel is fully opaque; everything the payload never touches
						// keeps the zero alpha the canvas was allocated with.
						rgba[offset + 3] = 0xff;
					}
				}
				x += 1;
			}
		}
		SixelCommand::CarriageReturn => x = 0,
		SixelCommand::NextBand => {
			band += 1;
			x = 0;
		}
		SixelCommand::Raster { .. } => {}
	});
}

/// A fresh palette: the VT340's sixteen defaults, then black for the rest.
fn default_registers() -> [[u8; 3]; COLOR_REGISTERS] {
	let mut palette = [[0u8; 3]; COLOR_REGISTERS];
	palette[..DEFAULT_PALETTE.len()].copy_from_slice(&DEFAULT_PALETTE);
	palette
}

/// A colour register index, clamped into the palette. A payload naming a register past cmote's 256
/// (xterm allows more) folds onto the last one rather than being dropped: two of its colours then
/// collide, which shows as a wrong shade — far less confusing than a hole where the pixels should be.
fn register_index(register: u16) -> u8 {
	// Clamped to the last register, and `COLOR_REGISTERS` is 256, so the result is a byte by
	// construction — the `min` is the bound and the `try_from` reads it back rather than assuming it.
	let last = u16::try_from(COLOR_REGISTERS - 1).unwrap_or(u16::from(u8::MAX));
	u8::try_from(register.min(last)).unwrap_or(u8::MAX)
}

/// The RGB a `#Pc;Pu;Px;Py;Pz` definition means. `Pu` 2 is RGB with each channel a PERCENTAGE
/// (0-100, not 0-255 — the classic sixel trap), and `Pu` 1 is DEC's HLS. Any other coding is
/// treated as RGB: it is the overwhelmingly common one, so guessing it keeps a mislabelled payload
/// looking right instead of painting it black.
fn parse_color(coding: u16, [x, y, z]: [u16; 3]) -> [u8; 3] {
	if coding == 1 {
		hls(x, y, z)
	} else {
		[percent(x), percent(y), percent(z)]
	}
}

/// One 0-100 percentage as a 0-255 channel, rounded rather than truncated so 50% is 128 and not
/// 127. A value past 100 is clamped — the scale has no room above full intensity.
fn percent(value: u16) -> u8 {
	// Clamped to 100 first, so the largest result is exactly 255 — the whole point of the clamp.
	u8::try_from((u32::from(value.min(100)) * 255 + 50) / 100).unwrap_or(u8::MAX)
}

/// DEC's HLS as RGB: `hue` in degrees, `lightness` and `saturation` as percentages.
///
/// The catch is the hue origin. DEC measures from BLUE — 0° blue, 120° red, 240° green — where
/// every modern HSL formula measures from red. Rotating the angle by 240° puts DEC's wheel onto
/// the standard one, and then this is the textbook conversion. HLS payloads are rare (nearly every
/// emitter writes RGB), but a picture drawn in the wrong primaries is unmistakable, so the rotation
/// is worth writing down rather than discovering.
fn hls(hue: u16, lightness: u16, saturation: u16) -> [u8; 3] {
	let lightness = f32::from(lightness.min(100)) / 100.0;
	let saturation = f32::from(saturation.min(100)) / 100.0;
	// Grey needs no hue at all, and this also keeps the channel maths off a zero-width range.
	if saturation == 0.0 {
		let grey = (lightness * 255.0).round() as u8;
		return [grey, grey, grey];
	}
	let max = if lightness < 0.5 {
		lightness * (1.0 + saturation)
	} else {
		lightness + saturation - lightness * saturation
	};
	let min = lightness * 2.0 - max;
	// Degrees to turns, with DEC's blue origin rotated onto the standard red one.
	let turn = f32::from((hue + 240) % 360) / 360.0;
	[
		channel(min, max, turn + 1.0 / 3.0),
		channel(min, max, turn),
		channel(min, max, turn - 1.0 / 3.0),
	]
}

/// One channel of an HLS colour: where `turn` (in turns, red at 0) falls between the colour's
/// darkest and lightest components. The three thresholds are the standard piecewise ramp — rising
/// over the first sixth of the wheel, flat at full over the next third, falling over the next
/// sixth, and dark for the remaining third.
fn channel(min: f32, max: f32, turn: f32) -> u8 {
	let turn = turn.rem_euclid(1.0);
	let value = if turn < 1.0 / 6.0 {
		min + (max - min) * 6.0 * turn
	} else if turn < 0.5 {
		max
	} else if turn < 2.0 / 3.0 {
		min + (max - min) * (2.0 / 3.0 - turn) * 6.0
	} else {
		min
	};
	(value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// The six pixel bits a printable byte carries, or `None` when it is not a sixel character at all.
fn sixel_bits(byte: u8) -> Option<u8> {
	(SIXEL_FIRST..=SIXEL_LAST)
		.contains(&byte)
		.then(|| byte - SIXEL_FIRST)
}

/// A command's parameter run: up to `MAX_NUMBERS` values, and how many the payload actually named.
/// A fixed array rather than a `Vec` because `#Pc` runs once per colour change — thousands of times
/// in a photograph — and an allocation per colour change would dominate the decode.
struct Numbers {
	values: [u16; MAX_NUMBERS],
	count: usize,
}

impl Numbers {
	/// Just the parameters the payload named, so a caller can match on the run's shape (`[register]`
	/// is a select, `[register, coding, x, y, z]` a definition).
	fn as_slice(&self) -> &[u16] {
		&self.values[..self.count]
	}
}

/// Read a `;`-separated run of decimal parameters, leaving `index` on the first byte that is not
/// part of it — which for `#0;2;100;0;0~` is the `~` the caller still has to paint.
///
/// An omitted parameter (`#;2;…`) counts as a zero, as the VT spec says. Digits accumulate with
/// saturation, so a remote's `99999999` clamps at `u16::MAX` instead of wrapping to a small number
/// that would look plausible; a run longer than `MAX_NUMBERS` is consumed to keep the walk in step
/// but its surplus is dropped.
fn numbers(payload: &[u8], index: &mut usize) -> Numbers {
	let mut values = [0u16; MAX_NUMBERS];
	let mut count = 0usize;
	// Whether the parameter being read has any digits yet — how an omitted one is told from the
	// separator that follows a named one.
	let mut open = false;
	while let Some(&byte) = payload.get(*index) {
		match byte {
			b'0'..=b'9' => {
				if !open {
					open = true;
					count += 1;
				}
				if let Some(slot) = values.get_mut(count - 1) {
					*slot = slot
						.saturating_mul(10)
						.saturating_add(u16::from(byte - b'0'));
				}
				*index += 1;
			}
			b';' => {
				if !open {
					count += 1;
				}
				open = false;
				*index += 1;
			}
			_ => break,
		}
	}
	Numbers {
		values,
		count: count.min(MAX_NUMBERS),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The pixel at `(x, y)` as RGBA, for asserting one dot at a time.
	fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
		let offset = ((y * u32::from(image.width) + x) * 4) as usize;
		image.rgba[offset..offset + 4]
			.try_into()
			.expect("four bytes per pixel")
	}

	#[test]
	fn one_sixel_character_paints_a_column_of_six_pixels() {
		// `#0;2;100;0;0` defines register 0 as full red (RGB percentages), and `~` (0x7E) sets all
		// six bits — so this is a 1×6 red column, which the raster attributes also state.
		let image = decode(b"\"1;1;1;6#0;2;100;0;0~").expect("a picture");
		assert_eq!((image.width, image.height), (1, 6));
		assert_eq!(pixel(&image, 0, 0), [255, 0, 0, 255]);
		assert_eq!(pixel(&image, 0, 5), [255, 0, 0, 255]);
	}

	#[test]
	fn a_pixel_no_sixel_set_stays_transparent() {
		// `@` is 0x40 — bit 0 only, the top pixel of the band. With no raster attributes the canvas
		// is the band it painted: one column, six rows. The five pixels under the dot were never
		// written, so they keep the zero alpha the grid's own background will show through.
		let image = decode(b"#0;2;0;100;0@").expect("a picture");
		assert_eq!((image.width, image.height), (1, 6));
		assert_eq!(pixel(&image, 0, 0), [0, 255, 0, 255]);
		assert_eq!(pixel(&image, 0, 1), [0, 0, 0, 0]);
	}

	#[test]
	fn the_repeat_introducer_fills_a_run_of_columns() {
		// `!5~` is five columns of six pixels — the format's compression, and the reason a wide flat
		// area costs a handful of bytes rather than one character per column.
		let image = decode(b"#0;2;100;100;100!5~").expect("a picture");
		assert_eq!((image.width, image.height), (5, 6));
		assert_eq!(pixel(&image, 4, 5), [255, 255, 255, 255]);
		// The `~` was consumed by the repeat, so nothing painted a sixth column.
		assert_eq!(image.rgba.len(), 5 * 6 * 4);
	}

	#[test]
	fn carriage_return_overprints_and_new_line_drops_a_band() {
		// `~` paints a column, `$` returns to the left edge of the SAME band and `~` paints over it
		// in the second colour, then `-` drops a band and paints there. So: one column wide, two
		// bands tall, and the top band shows the second colour.
		let image = decode(b"#0;2;100;0;0~$#1;2;0;0;100~-#0~").expect("a picture");
		assert_eq!((image.width, image.height), (1, 12));
		assert_eq!(pixel(&image, 0, 0), [0, 0, 255, 255]);
		assert_eq!(pixel(&image, 0, 6), [255, 0, 0, 255]);
	}

	#[test]
	fn a_payload_that_defines_nothing_draws_in_the_default_palette() {
		// `#1~` selects register 1 without defining it: the VT340 default, blue. Emitters that lean
		// on the built-in palette are the reason it is here rather than sixteen blacks.
		let image = decode(b"#1~").expect("a picture");
		assert_eq!(pixel(&image, 0, 0), [0x33, 0x33, 0xcc, 0xff]);
	}

	#[test]
	fn a_colour_is_a_percentage_not_a_byte() {
		// The classic sixel trap: `#0;2;50;50;50` is 50% grey (128), not RGB(50,50,50).
		let image = decode(b"#0;2;50;50;50~").expect("a picture");
		assert_eq!(pixel(&image, 0, 0), [128, 128, 128, 255]);
	}

	#[test]
	fn hls_hues_are_read_from_decs_blue_origin() {
		// DEC measures hue from blue, not red: 0° is blue, 120° red, 240° green. Lightness 50% and
		// full saturation put each at its pure primary, which is what pins the 240° rotation down.
		let blue = decode(b"#0;1;0;50;100~").expect("a picture");
		assert_eq!(pixel(&blue, 0, 0), [0, 0, 255, 255]);
		let red = decode(b"#0;1;120;50;100~").expect("a picture");
		assert_eq!(pixel(&red, 0, 0), [255, 0, 0, 255]);
		let green = decode(b"#0;1;240;50;100~").expect("a picture");
		assert_eq!(pixel(&green, 0, 0), [0, 255, 0, 255]);
		// No saturation is grey whatever the hue, at the lightness given.
		let grey = decode(b"#0;1;99;50;0~").expect("a picture");
		assert_eq!(pixel(&grey, 0, 0), [128, 128, 128, 255]);
	}

	#[test]
	fn the_raster_attributes_crop_the_canvas() {
		// Two bands are painted (12 pixel rows) but the sender says the picture is 8 tall: its crop
		// wins, and the rows past it are dropped rather than padding the image out.
		let image = decode(b"\"1;1;2;8#0;2;100;0;0!2~-!2~").expect("a picture");
		assert_eq!((image.width, image.height), (2, 8));
		assert_eq!(pixel(&image, 1, 7), [255, 0, 0, 255]);
	}

	#[test]
	fn an_image_past_the_caps_is_refused_whole() {
		// A raster attribute far past what cmote will hold draws NOTHING — not a crop, which would
		// show the user a silently truncated picture, and not an allocation on a remote's word (§12).
		assert_eq!(decode(b"\"1;1;9000;9000#0;2;100;0;0~"), None);
		// A shape inside both edge caps but past the pixel cap is refused the same way.
		assert_eq!(decode(b"\"1;1;4000;4000#0;2;100;0;0~"), None);
	}

	#[test]
	fn a_payload_with_no_pixels_is_no_picture() {
		// Empty, whitespace, and colour definitions with nothing painted: all nothing to draw.
		assert_eq!(decode(b""), None);
		assert_eq!(decode(b"\r\n  "), None);
		assert_eq!(decode(b"#0;2;100;0;0#1;2;0;100;0"), None);
	}

	#[test]
	fn a_run_past_the_right_edge_is_clipped_not_wrapped() {
		// The raster says two columns wide; the payload paints five. The surplus is dropped rather
		// than wrapping onto the next row, and the canvas stays exactly the size that was allocated.
		let image = decode(b"\"1;1;2;6#0;2;100;0;0!5~").expect("a picture");
		assert_eq!((image.width, image.height), (2, 6));
		assert_eq!(image.rgba.len(), 2 * 6 * 4);
		assert_eq!(pixel(&image, 1, 0), [255, 0, 0, 255]);
	}

	#[test]
	fn a_huge_parameter_saturates_instead_of_wrapping() {
		// `99999999` past `u16::MAX` must not fold back to a small plausible number — a wrapped
		// width would size a canvas the sender never asked for. Saturating puts it past the caps,
		// so the picture is refused.
		assert_eq!(decode(b"\"1;1;99999999;6#0;2;100;0;0~"), None);
	}

	#[test]
	fn newlines_inside_the_payload_are_ignored() {
		// Emitters wrap long sixel lines to keep them under a line-length limit; the embedded CR/LF
		// are not commands and must not be read as anything. Painted the same as the unwrapped form.
		let wrapped = decode(b"#0;2;100;0;0!3~\r\n-\r\n!3~").expect("a picture");
		let plain = decode(b"#0;2;100;0;0!3~-!3~").expect("a picture");
		assert_eq!(wrapped, plain);
	}
}

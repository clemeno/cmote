// term/graphics.rs — inline images in the terminal grid (PLAN §41).
//
// A program that wants to show a picture in a terminal writes it into the byte stream as a sixel
// DCS (`DCS <params> q <payload> ST`) — `img2sixel`, `chafa -f sixel`, gnuplot's sixel terminal,
// timg, matplotlib's sixel backend, `lsix`. `alacritty_terminal` carries no graphics at all: its
// VT parser's DCS hooks are no-op debug logs, so the payload is followed to its terminator and
// dropped. That is exactly the shape cmote has answered four times already — the cwd (§17),
// modifyOtherKeys (§9), the identity queries (§33) and the OSC 133 marks (§34) are all sequences
// the engine ignores and cmote scans out of the same bytes — so images need no engine fork either.
// They need three things this module owns:
//
//   1. FIND the picture. A byte-at-a-time scanner, because output arrives in arbitrary chunks and a
//      megabyte of sixel is certain to be split across several.
//   2. ANCHOR it. A placement is an ABSOLUTE document line plus a column (§40) — the same
//      scrollback-stable coordinate the prompt marks, the search hits and the selection use — so
//      scrolling moves the picture with its text and no image ever needs repositioning.
//   3. BOUND it. Decoded pixels are the only unbounded memory a remote can hand cmote, so the store
//      is capped in both count and bytes and evicts oldest-first (§12).
//
// The cells the picture covers are RESERVED in the engine by `term::mod`, which erases that box and
// feeds it the line feeds the image's height needs. So the grid underneath an image is ordinary
// blank cells: it scrolls, it reflows and it evicts exactly as text does, and the renderer's only
// job is to paint the pixels over the box the placement names (`ui::grid`).
//
// The store holds an iced image HANDLE rather than raw pixels, which is the one place this module
// looks up at the GUI. A handle carries the pixels plus an identity, and the renderer caches its
// GPU texture against that identity — so minting one per decoded image uploads it once, while
// minting one per frame (the only alternative, since the widget is rebuilt every frame and cannot
// cache anything itself) would re-upload every picture on screen sixty times a second.

use iced::advanced::image::Handle;

use super::sixel;

/// The escape byte that leads a CSI (`ESC [`) and a DCS (`ESC P`), and the bell some emitters use
/// as a string terminator in place of the canonical `ESC \`.
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The 8-bit string terminator, `ST` as a single C1 byte. No sixel payload can contain it — every
/// byte of a payload is printable ASCII — so accepting it costs nothing and reads the emitters that
/// send it.
const ST: u8 = 0x9c;

/// The longest DCS parameter run buffered before the final byte. A sixel's is `P1;P2;P3` at most; a
/// longer one is malformed, and refusing to grow keeps a hostile stream out of our memory (§12).
const MAX_PARAMS: usize = 16;

/// The most payload bytes buffered for one image. A screen-sized photograph is a megabyte or two of
/// sixel; 16 MiB leaves generous room for a big one while capping what a single DCS can make cmote
/// hold. Past it the image is abandoned — the scanner keeps following the DCS to its terminator, so
/// the rest of the payload still cannot be mistaken for commands, but nothing is decoded.
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// How many images the session keeps, and how many bytes of decoded pixels they may add up to.
/// Both are eviction thresholds, not refusals: the oldest picture goes so the newest can be shown,
/// which is the behaviour a user reading a scrollback of plots expects. 64 MiB is the RGBA cost of
/// about four full-screen photographs.
const MAX_IMAGES: usize = 64;
const MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// The cell size assumed until the GUI measures its own and calls `Terminal::set_cell_pixels` — the
/// metrics cmote's grid actually uses, so even the window between construction and the first
/// measurement reserves a sensible number of rows. The GUI owns the real numbers (§9); these exist
/// so a division by a zero-sized cell is impossible rather than to be authoritative.
const FALLBACK_CELL_WIDTH: u16 = 7;
const FALLBACK_CELL_HEIGHT: u16 = 14;

/// Something the scanner found that the image store has to act on, handed to `term::mod` with the
/// byte offset the engine should be advanced to before acting on it (§34's tactic).
///
/// The two kinds of event want that offset on OPPOSITE sides of their own bytes, which is worth being
/// explicit about because it is the difference between right and subtly wrong:
///
///   * An `Image` is reported PAST its DCS. A picture goes where the cursor is, and the cursor is
///     only in the right place once everything before the picture has been drawn.
///   * An erase is reported BEFORE its sequence. Which pictures an erase takes is decided by where
///     the screen ENDS and the scrollback begins — and the engine answers that differently the
///     instant it applies the erase (`CSI 3 J` drops its whole history, so asking afterwards is
///     asking a terminal that no longer remembers).
#[derive(Debug)]
pub enum Event {
	/// A complete, decoded sixel image, ready to be placed at wherever the cursor then is.
	Image(sixel::Image),
	/// `CSI 2 J` — the program erased the visible screen. The pictures ON that screen go with it;
	/// anything already scrolled into history is untouched, exactly as the text is.
	ClearScreen,
	/// `CSI 3 J` — the program erased the SCROLLBACK. The mirror of `ClearScreen`: the pictures in
	/// history go and the ones on screen stay. A shell's `clear` sends both, which clears the lot.
	ClearScrollback,
	/// `ESC c` (RIS) — a full reset. The session starts over, so every picture goes.
	Reset,
}

/// Where one image sits and what it is. The geometry is in two units on purpose: `line`/`col`/
/// `rows`/`cols` are the CELLS the picture reserved (the coordinates the grid and the engine share),
/// and `width`/`height` are its PIXELS, which is what it is actually drawn at.
///
/// `line` is absolute (§40): line 0 is the oldest line the session still retains, so the picture
/// keeps pointing at its own text as output pushes it up the screen and into the scrollback, without
/// anything having to move it.
#[derive(Debug, Clone)]
pub struct Placement {
	pub line: u64,
	pub col: u16,
	pub rows: u16,
	pub cols: u16,
	pub width: u16,
	pub height: u16,
	/// The decoded pixels, as the identity the renderer caches its texture against (see the module
	/// header). Cheap to clone — it is a reference-counted buffer plus that identity.
	pub handle: Handle,
}

impl Placement {
	/// How many bytes of pixels this placement holds, for the store's byte cap. Derived from the
	/// size rather than stored, because RGBA is four bytes per pixel by construction.
	fn bytes(&self) -> usize {
		usize::from(self.width) * usize::from(self.height) * 4
	}
}

/// Where the scanner sits in the byte stream. Only two shapes are tracked in detail — a DCS up to
/// its final byte (a `q` opens a sixel; anything else is some other DCS to be followed silently) and
/// a `CSI <digits> J` erase — and every other sequence resets straight back to `Text`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC: a CSI starts on `[`, a DCS on `P`, and a lone `c` is RIS.
	Esc,
	/// Inside `ESC [ …`, collecting digits in case this is an erase-display.
	Csi,
	/// Inside `ESC P <params>`, waiting for the final byte that says which DCS this is.
	Dcs,
	/// Inside a sixel payload, accumulating it.
	Payload,
	/// Saw ESC inside a payload; a `\` terminates the image.
	PayloadEsc,
	/// Following some OTHER DCS (a DECRQSS request, an XTGETTCAP reply) to its terminator,
	/// accumulating nothing — so its arbitrary data cannot be mistaken for a picture.
	Other,
	/// Saw ESC inside another DCS; a `\` ends it.
	OtherEsc,
}

/// The image scanner and the placements it has produced. Feed it every byte of shell output; it
/// returns the events that completed in the chunk (usually none) and keeps its state across calls,
/// so a picture split over any number of chunks is decoded on the chunk that finishes it.
#[derive(Debug)]
pub struct Images {
	state: Scan,
	params: Vec<u8>,
	payload: Vec<u8>,
	/// Whether the payload being read has already outgrown `MAX_PAYLOAD`. Kept as a flag rather than
	/// by abandoning the state, so the DCS is still followed to its terminator.
	overflowed: bool,
	/// Where in THIS chunk the escape sequence being read began, for the events that have to be acted
	/// on before their own bytes reach the engine (see `Event`). `None` once a sequence has run over a
	/// chunk boundary: its bytes then start at the very beginning of this chunk, which is offset 0.
	sequence_start: Option<usize>,
	placements: Vec<Placement>,
	/// Total decoded pixel bytes held, maintained alongside `placements` so the byte cap costs no
	/// walk of the list.
	bytes: usize,
	/// One cell in pixels, as the GUI measured it — what turns a picture's pixel size into the
	/// number of rows and columns it has to reserve.
	cell_width: u16,
	cell_height: u16,
}

impl Default for Images {
	fn default() -> Self {
		Self {
			state: Scan::default(),
			params: Vec::new(),
			payload: Vec::new(),
			overflowed: false,
			sequence_start: None,
			placements: Vec::new(),
			bytes: 0,
			cell_width: FALLBACK_CELL_WIDTH,
			cell_height: FALLBACK_CELL_HEIGHT,
		}
	}
}

impl Images {
	/// Scan a chunk of shell output. Each returned event carries the offset the engine should be
	/// advanced to before it is acted on — past its own bytes for a picture, before them for an erase,
	/// for the reasons on `Event` — so the caller advances, acts, and carries on: the split-advance
	/// `osc133` established (§34).
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Event)> {
		let mut found = Vec::new();
		// Any sequence still open from the last chunk began before this one did.
		self.sequence_start = None;
		for (index, &byte) in bytes.iter().enumerate() {
			// The two offsets an event can be reported at: past the byte that finished the sequence,
			// or at the byte the sequence started on (the beginning of the chunk when it started in an
			// earlier one).
			let past = index + 1;
			let began = self.sequence_start.unwrap_or(0);
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.begin(index);
					}
				}
				Scan::Esc => match byte {
					b'[' => {
						self.params.clear();
						self.state = Scan::Csi;
					}
					b'P' => {
						self.params.clear();
						self.state = Scan::Dcs;
					}
					// RIS: the terminal is reset, so nothing that was drawn survives it.
					b'c' => {
						found.push((began, Event::Reset));
						self.state = Scan::Text;
					}
					ESC => self.begin(index),
					_ => self.state = Scan::Text,
				},
				Scan::Csi => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						if self.params.len() > MAX_PARAMS {
							self.state = Scan::Text;
						}
					}
					b'J' => {
						// `CSI 2 J` erases the screen and `CSI 3 J` the scrollback; `CSI J` and
						// `CSI 0/1 J` erase only part of one line's worth, which no image spans, so
						// they are left alone.
						match self.params.as_slice() {
							b"2" => found.push((began, Event::ClearScreen)),
							b"3" => found.push((began, Event::ClearScrollback)),
							_ => {}
						}
						self.state = Scan::Text;
					}
					ESC => self.begin(index),
					// Any other CSI: a colour, a cursor move, a private mode. Not ours.
					_ => self.state = Scan::Text,
				},
				Scan::Dcs => match byte {
					// A sixel's parameters are `P1;P2;P3`; they change no pixel (see `sixel::decode`)
					// so they are collected only to be stepped over.
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						if self.params.len() > MAX_PARAMS {
							self.state = Scan::Other;
						}
					}
					// The final byte. `q` with no intermediate is sixel; a DECRQSS (`$q`) or an
					// XTGETTCAP (`+q`) reaches here with its intermediate already read as a
					// non-parameter, so it lands in `Other` below.
					b'q' => {
						self.payload.clear();
						self.overflowed = false;
						self.state = Scan::Payload;
					}
					ESC => self.begin(index),
					_ => self.state = Scan::Other,
				},
				Scan::Payload => match byte {
					ESC => self.state = Scan::PayloadEsc,
					BEL | ST => {
						self.complete(past, &mut found);
					}
					_ => {
						// Past the cap: stop accumulating but keep following the DCS, so the rest of
						// the payload still cannot be read as commands (§12).
						if self.payload.len() < MAX_PAYLOAD {
							self.payload.push(byte);
						} else {
							self.overflowed = true;
						}
					}
				},
				Scan::PayloadEsc => match byte {
					b'\\' => self.complete(past, &mut found),
					// `ESC ESC` inside a payload: still waiting for the terminator's `\`.
					ESC => self.state = Scan::PayloadEsc,
					// A stray ESC that formed no terminator: the picture is malformed, so it is
					// abandoned rather than guessed at.
					_ => self.state = Scan::Text,
				},
				Scan::Other => match byte {
					ESC => self.state = Scan::OtherEsc,
					BEL | ST => self.state = Scan::Text,
					_ => {}
				},
				Scan::OtherEsc => match byte {
					b'\\' => self.state = Scan::Text,
					ESC => self.state = Scan::OtherEsc,
					_ => self.state = Scan::Text,
				},
			}
		}
		found
	}

	/// Start reading an escape sequence at `index`, remembering where it began — that offset is what
	/// an erase event is reported at, so the engine can be advanced to just before its bytes.
	fn begin(&mut self, index: usize) {
		self.sequence_start = Some(index);
		self.state = Scan::Esc;
	}

	/// Finish the payload being read: decode it, and hand the picture on if there was one. An
	/// oversized, empty or undecodable payload produces no event at all — the caller then reserves
	/// no cells either, so a picture cmote cannot draw leaves the screen exactly as it was.
	fn complete(&mut self, past: usize, found: &mut Vec<(usize, Event)>) {
		self.state = Scan::Text;
		let payload = std::mem::take(&mut self.payload);
		if self.overflowed {
			return;
		}
		if let Some(image) = sixel::decode(&payload) {
			found.push((past, Event::Image(image)));
		}
	}

	/// Tell the store how big one cell is in pixels, so a picture's size can be turned into the rows
	/// and columns it reserves. Set by the GUI, which owns the metrics (§9); a zero is ignored — it
	/// could only be a bug on the way in, and dividing by it would be worse than keeping the last
	/// known good pair.
	pub fn set_cell_pixels(&mut self, width: u16, height: u16) {
		if width > 0 {
			self.cell_width = width;
		}
		if height > 0 {
			self.cell_height = height;
		}
	}

	/// Place a decoded image with its top-left corner at absolute `line`, column `col`, and return
	/// the `(rows, cols)` box it needs reserved from that corner.
	///
	/// Both counts round UP: a picture 30 pixels tall in a 14-pixel cell reserves three rows, so the
	/// cell box always covers every pixel and text can never be laid over the bottom of an image. The
	/// caller reserves exactly that box in the engine, which is what keeps the two in step.
	pub fn place(&mut self, image: sixel::Image, line: u64, col: u16) -> (u16, u16) {
		let rows = cells(image.height, self.cell_height);
		let cols = cells(image.width, self.cell_width);
		let placement = Placement {
			line,
			col,
			rows,
			cols,
			width: image.width,
			height: image.height,
			handle: Handle::from_rgba(u32::from(image.width), u32::from(image.height), image.rgba),
		};
		self.bytes += placement.bytes();
		self.placements.push(placement);
		self.evict();
		(rows, cols)
	}

	/// Every image the session is holding, oldest first — what the renderer walks each frame.
	pub fn placements(&self) -> &[Placement] {
		&self.placements
	}

	/// Drop the pictures on the visible screen, for `CSI 2 J`. `first_visible` is the absolute line
	/// the live screen starts at (the engine's `history_size`), so a picture anchored above it is in
	/// the scrollback and survives — the same split the erase itself makes in the text.
	pub fn clear_screen(&mut self, first_visible: u64) {
		self.retain(|placement| placement.line < first_visible);
	}

	/// Drop the pictures in the scrollback, for `CSI 3 J` — the mirror of `clear_screen`.
	pub fn clear_scrollback(&mut self, first_visible: u64) {
		self.retain(|placement| placement.line >= first_visible);
	}

	/// Drop every picture. Used for RIS, and for a resize — a reflow changes how many lines the
	/// history holds, so every absolute anchor stops meaning what it did (`ponytail:` the same
	/// trade-off the prompt marks make, §34: a picture that would land on the wrong line is better
	/// gone than wrong, and it is cleared even on a height-only resize that reflows nothing).
	pub fn clear(&mut self) {
		self.placements.clear();
		self.bytes = 0;
	}

	/// Keep the placements `keep` accepts, and re-total the bytes held.
	fn retain(&mut self, keep: impl Fn(&Placement) -> bool) {
		self.placements.retain(&keep);
		self.bytes = self.placements.iter().map(Placement::bytes).sum();
	}

	/// Enforce the store's caps by dropping the oldest pictures. A `Vec::remove(0)` is a shift of at
	/// most `MAX_IMAGES` entries — a handful of pointers — which is far cheaper than the ring buffer
	/// it would take to avoid it.
	fn evict(&mut self) {
		while self.placements.len() > MAX_IMAGES
			|| (self.bytes > MAX_TOTAL_BYTES && self.placements.len() > 1)
		{
			let dropped = self.placements.remove(0);
			self.bytes = self.bytes.saturating_sub(dropped.bytes());
		}
	}
}

/// How many cells `pixels` needs when one cell is `cell` pixels — rounding up, and never zero for a
/// picture with any pixels at all. Saturating, so even a pathological cell size cannot wrap the
/// count round to nothing.
fn cells(pixels: u16, cell: u16) -> u16 {
	let cell = u32::from(cell.max(1));
	let count = u32::from(pixels).div_ceil(cell);
	count.clamp(1, u32::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A minimal one-pixel-wide red sixel: `DCS q #0;2;100;0;0 ~ ST`, six pixels tall.
	const RED_COLUMN: &[u8] = b"\x1bPq#0;2;100;0;0~\x1b\\";

	/// Feed one slice to a fresh scanner and return what it found.
	fn scan(bytes: &[u8]) -> Vec<(usize, Event)> {
		Images::default().feed(bytes)
	}

	/// The single image in a scan result, or a panic naming what was found instead.
	fn only_image(found: Vec<(usize, Event)>) -> (usize, sixel::Image) {
		match found.into_iter().next() {
			Some((offset, Event::Image(image))) => (offset, image),
			other => panic!("expected one image, found {other:?}"),
		}
	}

	#[test]
	fn a_sixel_is_read_out_of_the_stream_with_its_end_offset() {
		// The picture is reported at the byte just past its terminator, so the caller can advance the
		// engine over the whole DCS before placing it.
		let found = scan(RED_COLUMN);
		let (offset, image) = only_image(found);
		assert_eq!(offset, RED_COLUMN.len());
		assert_eq!((image.width, image.height), (1, 6));
	}

	#[test]
	fn text_around_a_sixel_is_not_disturbed() {
		// A picture in the middle of ordinary output is found where it sits — the offset is inside
		// the chunk, not at its end.
		let mut stream = b"hello".to_vec();
		stream.extend_from_slice(RED_COLUMN);
		stream.extend_from_slice(b"world");
		let (offset, _) = only_image(scan(&stream));
		assert_eq!(offset, 5 + RED_COLUMN.len());
	}

	#[test]
	fn a_sixel_split_across_chunks_completes_on_the_terminator() {
		// Output arrives in arbitrary chunks and a real picture spans many; the scanner carries its
		// payload across, and the event lands on the chunk that finishes it, offset relative to THAT
		// chunk.
		let mut images = Images::default();
		assert!(images.feed(b"\x1bPq#0;2;100;0;0").is_empty());
		let found = images.feed(b"~\x1b\\rest");
		let (offset, image) = only_image(found);
		assert_eq!(offset, 3, "just past the terminator in this chunk");
		assert_eq!((image.width, image.height), (1, 6));
	}

	#[test]
	fn a_bell_terminates_a_sixel_too() {
		// Some emitters end the DCS with BEL rather than `ESC \`; both are accepted.
		let (_, image) = only_image(scan(b"\x1bPq#0;2;100;0;0~\x07"));
		assert_eq!((image.width, image.height), (1, 6));
	}

	#[test]
	fn another_dcs_is_not_mistaken_for_a_picture() {
		// A DECRQSS request and an XTGETTCAP reply are DCS strings whose data is arbitrary; they are
		// followed to their terminators and read as nothing, so neither can smuggle a picture — or
		// leave the scanner mid-payload for the next chunk.
		assert!(scan(b"\x1bP$qm\x1b\\").is_empty());
		assert!(scan(b"\x1bP1+r544E=787465726D\x1b\\").is_empty());
		// And after one, the scanner is back in text and still finds a real picture.
		let mut images = Images::default();
		assert!(images.feed(b"\x1bP$qm\x1b\\").is_empty());
		assert_eq!(images.feed(RED_COLUMN).len(), 1);
	}

	#[test]
	fn an_erase_sequence_asks_for_the_right_pictures_to_go() {
		// `CSI 2 J` is the screen, `CSI 3 J` the scrollback, `ESC c` the lot — each reported at the
		// offset its own bytes START at, so the caller decides which pictures it takes from a screen
		// the engine has not erased yet (see `Event`). A partial erase (`CSI 0 J`, `CSI K`) spans no
		// picture, so it says nothing.
		assert!(matches!(
			scan(b"text\x1b[2J").as_slice(),
			[(4, Event::ClearScreen)]
		));
		assert!(matches!(
			scan(b"text\x1b[3J").as_slice(),
			[(4, Event::ClearScrollback)]
		));
		assert!(matches!(scan(b"text\x1bc").as_slice(), [(4, Event::Reset)]));
		assert!(scan(b"\x1b[0J").is_empty());
		assert!(scan(b"\x1b[K").is_empty());
		// A shell's `clear` sends both erases, which between them clear everything.
		assert_eq!(scan(b"\x1b[3J\x1b[H\x1b[2J").len(), 2);
	}

	#[test]
	fn an_erase_split_across_chunks_is_reported_at_the_start_of_its_chunk() {
		// The sequence began in the previous chunk, so "before its bytes" is this chunk's offset 0 —
		// the engine is then exactly where it was when the erase started arriving.
		let mut images = Images::default();
		assert!(images.feed(b"text\x1b[").is_empty());
		assert!(matches!(
			images.feed(b"2J").as_slice(),
			[(0, Event::ClearScreen)]
		));
	}

	#[test]
	fn a_placement_reserves_the_cells_its_pixels_cover() {
		// A 20×30 picture in a 7×14 cell: three columns (20/7 rounds up) and three rows (30/14 rounds
		// up), so the reserved box always covers every pixel and text can never sit over the image.
		let mut images = Images::default();
		images.set_cell_pixels(7, 14);
		let image = sixel::Image {
			width: 20,
			height: 30,
			rgba: vec![0; 20 * 30 * 4],
		};
		assert_eq!(images.place(image, 12, 4), (3, 3), "the box reserved");
		let placement = &images.placements()[0];
		assert_eq!((placement.line, placement.col), (12, 4));
		assert_eq!((placement.rows, placement.cols), (3, 3));
		assert_eq!((placement.width, placement.height), (20, 30));
	}

	/// A picture smaller than one cell still reserves one row: the anchor line itself.
	#[test]
	fn a_tiny_picture_reserves_one_row() {
		let mut images = Images::default();
		images.set_cell_pixels(7, 14);
		let image = sixel::Image {
			width: 2,
			height: 3,
			rgba: vec![0; 2 * 3 * 4],
		};
		assert_eq!(images.place(image, 0, 0), (1, 1));
	}

	#[test]
	fn the_store_drops_its_oldest_picture_past_the_count_cap() {
		// Sixty-five one-pixel pictures: the first is evicted, so the list stays at the cap and the
		// newest is always the one on show (§12).
		let mut images = Images::default();
		for line in 0..=MAX_IMAGES as u64 {
			let image = sixel::Image {
				width: 1,
				height: 1,
				rgba: vec![0; 4],
			};
			images.place(image, line, 0);
		}
		assert_eq!(images.placements().len(), MAX_IMAGES);
		assert_eq!(
			images.placements()[0].line,
			1,
			"the oldest picture was the one dropped"
		);
	}

	#[test]
	fn erasing_the_screen_keeps_the_pictures_in_history() {
		// Two pictures, one in the scrollback (line 3) and one on the live screen (line 12) of a
		// session whose screen starts at line 10. Erasing the screen takes only the second; erasing
		// the scrollback takes only the first.
		let mut images = Images::default();
		for line in [3, 12] {
			let image = sixel::Image {
				width: 1,
				height: 1,
				rgba: vec![0; 4],
			};
			images.place(image, line, 0);
		}
		images.clear_screen(10);
		assert_eq!(images.placements().len(), 1);
		assert_eq!(images.placements()[0].line, 3);

		images.clear_scrollback(10);
		assert!(images.placements().is_empty());
	}

	#[test]
	fn a_reset_or_a_resize_drops_everything() {
		let mut images = Images::default();
		let image = sixel::Image {
			width: 1,
			height: 1,
			rgba: vec![0; 4],
		};
		images.place(image, 0, 0);
		images.clear();
		assert!(images.placements().is_empty());
	}

	#[test]
	fn an_oversized_payload_is_followed_but_not_decoded() {
		// A DCS whose payload runs past the cap yields no picture — and the scanner still ends up
		// back in text, so the bytes after the terminator are read as output rather than as more
		// payload (§12).
		let mut images = Images::default();
		let mut flood = b"\x1bPq#0;2;100;0;0".to_vec();
		flood.extend(std::iter::repeat_n(b'~', MAX_PAYLOAD + 1));
		assert!(images.feed(&flood).is_empty());
		assert!(images.feed(b"\x1b\\").is_empty());
		assert_eq!(images.feed(RED_COLUMN).len(), 1, "back in step afterwards");
	}
}

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
// There are TWO stores, because there are two pages. The alternate screen (`ranger`, `mpv --vo=sixel`,
// a preview pane in `fzf`) keeps no history at all, so its pictures cannot ride a document line that
// grows: they are anchored to a ROW on the page and thrown away whole when the program swaps back.
// That reads as a second coordinate space, which §40 spent its whole length collapsing to one — but
// it is the SAME space with the history at zero, since on the alternate screen `history_size` is 0
// and the document line of row `r` is exactly `r`. So the renderer needs no second path and the
// arithmetic is unchanged; what differs is the lifetime, which is what `Store` separates.
//
// The cells the picture covers are RESERVED in the engine by `term::mod`, which erases that box and
// steps the cursor down the rows the image's height needs. So the grid underneath an image is
// ordinary blank cells: it scrolls, it reflows and it evicts exactly as text does, and the renderer's
// only job is to paint the pixels over the box the placement names (`ui::grid`).
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

// The parameter run, bounded the way the engine bounds one (§106). This used to be a sixteen-BYTE cap
// that abandoned the sequence, which was wrong on the CSI side: a padded `CSI 0000000000000002 J` erases
// the screen as far as the engine is concerned, and giving up on it here left the pictures on a screen
// whose text had gone. The DCS side never parses its parameters at all, so the bound costs it nothing.
use super::csi::Params;

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
pub enum GraphicsEvent {
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
/// anything having to move it. On the ALTERNATE screen the session retains nothing, so line 0 is the
/// top of the page and `line` is simply the row — the same number, read against a document that
/// happens to be one screen tall (§41).
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

	/// Whether this placement's cell box overlaps `other`'s — the ordinary rectangle test, on the
	/// CELLS rather than the pixels, since the cells are what the engine reserved and what a later
	/// picture would be reserving over.
	///
	/// Only the alternate page asks (§41): a full-screen program redraws the same pane over and over,
	/// so the picture arriving is the replacement for the one already there rather than a second one
	/// beside it. On the primary screen the question never comes up — output only ever moves forward,
	/// so a new picture lands on lines no older one can be on.
	fn overlaps(&self, other: &Self) -> bool {
		let rows = self.line < other.line + u64::from(other.rows)
			&& other.line < self.line + u64::from(self.rows);
		let cols = u32::from(self.col) < u32::from(other.col) + u32::from(other.cols)
			&& u32::from(other.col) < u32::from(self.col) + u32::from(self.cols);
		rows && cols
	}
}

/// One page's pictures, and the pixel bytes they hold between them. There are two — the primary
/// screen's and the alternate screen's (§41) — so every rule about how a list grows and is trimmed is
/// written once here instead of twice in `Images`, and the caps are enforced on each page separately:
/// a program covering the alternate screen in pictures cannot evict the scrollback's.
#[derive(Debug, Default)]
struct Store {
	placements: Vec<Placement>,
	/// Total decoded pixel bytes held, maintained alongside `placements` so the byte cap costs no
	/// walk of the list.
	bytes: usize,
}

impl Store {
	/// Add a picture, then bring the page back inside its caps.
	fn push(&mut self, placement: Placement) {
		self.bytes += placement.bytes();
		self.placements.push(placement);
		self.evict();
	}

	/// Keep the placements `keep` accepts, and re-total the bytes held.
	fn retain(&mut self, keep: impl Fn(&Placement) -> bool) {
		self.placements.retain(&keep);
		self.bytes = self.placements.iter().map(Placement::bytes).sum();
	}

	/// Move each placement's anchor through `remap`, dropping the ones it answers `None` for (§101).
	/// The byte total is re-summed for the same reason `retain` does it: entries may have gone.
	fn renumber(&mut self, remap: impl Fn(u64) -> Option<u64>) {
		self.placements
			.retain_mut(|placement| match remap(placement.line) {
				Some(line) => {
					placement.line = line;
					true
				}
				None => false,
			});
		self.bytes = self.placements.iter().map(Placement::bytes).sum();
	}

	/// Drop every picture on this page.
	fn clear(&mut self) {
		self.placements.clear();
		self.bytes = 0;
	}

	/// Enforce the caps by dropping the oldest pictures. A `Vec::remove(0)` is a shift of at most
	/// `MAX_IMAGES` entries — a handful of pointers — which is far cheaper than the ring buffer it
	/// would take to avoid it.
	fn evict(&mut self) {
		while self.placements.len() > MAX_IMAGES
			|| (self.bytes > MAX_TOTAL_BYTES && self.placements.len() > 1)
		{
			let dropped = self.placements.remove(0);
			self.bytes = self.bytes.saturating_sub(dropped.bytes());
		}
	}
}

/// Where the scanner sits in the byte stream. Only two shapes are tracked in detail — a DCS up to
/// its final byte (a `q` opens a sixel; anything else is some other DCS to be followed silently) and
/// a `CSI <digits> J` erase — and every other sequence resets straight back to `Text`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum GraphicsScan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC: a CSI starts on `[`, a DCS on `P`, and a lone `c` is RIS.
	Esc,
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
	state: GraphicsScan,
	/// The CSI grammar, shared with the other scanners (§111) — the erases only. The DCS half stays in
	/// `state` beside it, which is the line `osc.rs` already drew when this module kept its own OSC
	/// framing: the framer reads CSI and nothing else.
	///
	/// Running the two over the same bytes is SAFER than the single fused machine it replaces, not
	/// riskier, and the reason is measured in `differential.rs`: an ESC ends a control string for the
	/// engine as well as opening the next sequence, so there is no payload the engine reads as data and
	/// a framer reads as a sequence. The framer's ESC handling is unconditional, and being conditional
	/// is exactly what the fused machine got wrong.
	framer: super::csi::Framer,
	params: Params,
	payload: Vec<u8>,
	/// Whether the payload being read has already outgrown `MAX_PAYLOAD`. Kept as a flag rather than
	/// by abandoning the state, so the DCS is still followed to its terminator.
	overflowed: bool,
	/// Where in THIS chunk the escape sequence being read began, for the events that have to be acted
	/// on before their own bytes reach the engine (see `GraphicsEvent`). `None` once a sequence has run over a
	/// chunk boundary: its bytes then start at the very beginning of this chunk, which is offset 0.
	sequence_start: Option<usize>,
	/// The PRIMARY screen's pictures, anchored to absolute document lines (§40) and living as long as
	/// the text they sit beside — which is to say until the scrollback evicts them or the session is
	/// reset.
	primary: Store,
	/// The ALTERNATE screen's, anchored to a row on the page and living only as long as the program
	/// that drew them (§41). Kept apart rather than mixed in with a flag, because the two differ in
	/// every way that matters: what the anchor means, what erases them, and when they all go.
	alternate: Store,
	/// One cell in pixels, as the GUI measured it — what turns a picture's pixel size into the
	/// number of rows and columns it has to reserve.
	cell_width: u16,
	cell_height: u16,
}

impl Default for Images {
	fn default() -> Self {
		Self {
			state: GraphicsScan::default(),
			framer: super::csi::Framer::default(),
			params: Params::default(),
			payload: Vec::new(),
			overflowed: false,
			sequence_start: None,
			primary: Store::default(),
			alternate: Store::default(),
			cell_width: FALLBACK_CELL_WIDTH,
			cell_height: FALLBACK_CELL_HEIGHT,
		}
	}
}

impl Images {
	/// Scan a chunk of shell output. Each returned event carries the offset the engine should be
	/// advanced to before it is acted on — past its own bytes for a picture, before them for an erase,
	/// for the reasons on `GraphicsEvent` — so the caller advances, acts, and carries on: the split-advance
	/// `osc133` established (§34).
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, GraphicsEvent)> {
		let mut found = Vec::new();
		// The DCS half FIRST, then the erases, and the order is not cosmetic. A picture is reported PAST
		// its terminator and an erase BEFORE its first byte, so `DCS … ST` immediately followed by
		// `CSI 2 J` — which is what `img2sixel; clear` sends — gives both events the SAME offset. The
		// sort below is stable, so whichever pass ran first wins the tie, and the stream order is draw
		// then erase: the picture has to go.
		self.scan_strings(bytes, &mut found);
		self.framer.feed(bytes, |span, csi| {
			if let Some(event) = erase(csi) {
				found.push((span.start(), event));
			}
		});
		found.sort_by_key(|&(offset, _)| offset);
		found
	}

	/// The control-string half: sixel payloads, and the RIS that throws every picture away.
	fn scan_strings(&mut self, bytes: &[u8], found: &mut Vec<(usize, GraphicsEvent)>) {
		// Any sequence still open from the last chunk began before this one did.
		self.sequence_start = None;
		for (index, &byte) in bytes.iter().enumerate() {
			// The two offsets an event can be reported at: past the byte that finished the sequence,
			// or at the byte the sequence started on (the beginning of the chunk when it started in an
			// earlier one).
			let past = index + 1;
			match self.state {
				GraphicsScan::Text => {
					if byte == ESC {
						self.begin(index);
					}
				}
				GraphicsScan::Esc => self.after_escape(byte, index, found),
				GraphicsScan::Dcs => match byte {
					// A sixel's parameters are `P1;P2;P3`; they change no pixel (see `sixel::decode`)
					// so they are collected only to be stepped over.
					b'0'..=b'9' | b';' => {
						if !self.params.push(byte) {
							self.state = GraphicsScan::Other;
						}
					}
					// The final byte. `q` with no intermediate is sixel; a DECRQSS (`$q`) or an
					// XTGETTCAP (`+q`) reaches here with its intermediate already read as a
					// non-parameter, so it lands in `Other` below.
					b'q' => {
						self.payload.clear();
						self.overflowed = false;
						self.state = GraphicsScan::Payload;
					}
					ESC => self.begin(index),
					_ => self.state = GraphicsScan::Other,
				},
				GraphicsScan::Payload => match byte {
					ESC => self.state = GraphicsScan::PayloadEsc,
					BEL | ST => {
						self.complete(past, found);
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
				GraphicsScan::PayloadEsc => match byte {
					b'\\' => self.complete(past, found),
					// `ESC ESC` inside a payload: still waiting for the terminator's `\`. The engine
					// keeps the payload it had across the pair too (§111, measured).
					ESC => self.state = GraphicsScan::PayloadEsc,
					// A stray ESC that formed no terminator: the picture is malformed, so it is
					// abandoned rather than guessed at — but the ESC still OPENED whatever follows it
					// (see `resume_after_escape`), and dropping to ordinary text here threw that away.
					// RIS is what made it matter: `ESC c` arriving mid-payload left every picture
					// standing on a screen the reset had just wiped.
					_ => self.resume_after_escape(byte, index, found),
				},
				GraphicsScan::Other => match byte {
					ESC => self.state = GraphicsScan::OtherEsc,
					BEL | ST => self.state = GraphicsScan::Text,
					_ => {}
				},
				GraphicsScan::OtherEsc => match byte {
					b'\\' => self.state = GraphicsScan::Text,
					ESC => self.state = GraphicsScan::OtherEsc,
					_ => self.resume_after_escape(byte, index, found),
				},
			}
		}
	}

	/// Start reading an escape sequence at `index`, remembering where it began — that offset is what
	/// an erase event is reported at, so the engine can be advanced to just before its bytes.
	fn begin(&mut self, index: usize) {
		self.sequence_start = Some(index);
		self.state = GraphicsScan::Esc;
	}

	/// Read `byte` as the one that FOLLOWS an ESC, whichever state that ESC arrived in.
	///
	/// `index` is where `byte` sits; `self.sequence_start` is where its ESC did, and is where an event
	/// gets reported from. Taking it from the field rather than as an argument is what keeps the two
	/// entry points from disagreeing about the offset.
	fn after_escape(&mut self, byte: u8, index: usize, found: &mut Vec<(usize, GraphicsEvent)>) {
		match byte {
			// A CSI, which this machine no longer reads: the framer beside it frames the erases (§111).
			// Dropping to ordinary text is safe because a CSI's own bytes hold no ESC, so the only thing
			// this half is waiting for — the next ESC — cannot arrive before the sequence has ended.
			b'[' => self.state = GraphicsScan::Text,
			b'P' => {
				self.params.clear();
				self.state = GraphicsScan::Dcs;
			}
			// RIS: the terminal is reset, so nothing that was drawn survives it.
			b'c' => {
				found.push((self.sequence_start.unwrap_or(0), GraphicsEvent::Reset));
				self.state = GraphicsScan::Text;
			}
			ESC => self.begin(index),
			_ => self.state = GraphicsScan::Text,
		}
	}

	/// The same, for an ESC that ENDED a control string rather than opening one out of ordinary text.
	///
	/// ESC does both jobs at once in the ANSI state machine, and the engine obeys that: a DCS
	/// interrupted by an ESC unhooks, and the sequence that ESC introduced is dispatched normally. So
	/// the string is abandoned — a malformed picture is never guessed at — and the reading carries on
	/// into the sequence the ESC opened instead of falling back to ordinary text.
	///
	/// The ESC sat one byte back, which is the whole reason this is a second entry point:
	/// `sequence_start` has to move off the abandoned string and onto it. `checked_sub` returning
	/// `None` means that byte was in the PREVIOUS chunk, which is exactly what a `None` records
	/// everywhere else here.
	fn resume_after_escape(
		&mut self,
		byte: u8,
		index: usize,
		found: &mut Vec<(usize, GraphicsEvent)>,
	) {
		self.sequence_start = index.checked_sub(1);
		self.after_escape(byte, index, found);
	}

	/// Finish the payload being read: decode it, and hand the picture on if there was one. An
	/// oversized, empty or undecodable payload produces no event at all — the caller then reserves
	/// no cells either, so a picture cmote cannot draw leaves the screen exactly as it was.
	fn complete(&mut self, past: usize, found: &mut Vec<(usize, GraphicsEvent)>) {
		self.state = GraphicsScan::Text;
		let payload = std::mem::take(&mut self.payload);
		if self.overflowed {
			return;
		}
		if let Some(image) = sixel::decode(&payload) {
			found.push((past, GraphicsEvent::Image(image)));
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
	/// The caller reserves exactly that box in the engine, which is what keeps the two in step.
	pub fn place(&mut self, image: sixel::Image, line: u64, col: u16) -> (u16, u16) {
		let placement = self.build(image, line, col);
		let reserved = (placement.rows, placement.cols);
		self.primary.push(placement);
		reserved
	}

	/// The same for the ALTERNATE page (§41), where the anchor is the `row` the cursor is on rather
	/// than a document line — the page keeps no history, so the two are the same number.
	///
	/// One rule differs, and it is the one that makes a preview pane work: a picture whose box the new
	/// one OVERLAPS is replaced rather than stacked under it. A full-screen program redraws the same
	/// pane every time the selection moves, so the picture arriving is the successor of the one
	/// already there — and without this the store would fill with the frames of a video, each hidden
	/// behind the next.
	pub fn place_alternate(&mut self, image: sixel::Image, row: u16, col: u16) -> (u16, u16) {
		let placement = self.build(image, u64::from(row), col);
		let reserved = (placement.rows, placement.cols);
		self.alternate
			.retain(|existing| !existing.overlaps(&placement));
		self.alternate.push(placement);
		reserved
	}

	/// Turn a decoded image into a placement at `line`/`col`, sizing its cell box from the measured
	/// cell. Both counts round UP: a picture 30 pixels tall in a 14-pixel cell reserves three rows, so
	/// the cell box always covers every pixel and text can never be laid over the bottom of an image.
	fn build(&self, image: sixel::Image, line: u64, col: u16) -> Placement {
		Placement {
			line,
			col,
			rows: cells(image.height, self.cell_height),
			cols: cells(image.width, self.cell_width),
			width: image.width,
			height: image.height,
			handle: Handle::from_rgba(u32::from(image.width), u32::from(image.height), image.rgba),
		}
	}

	/// Every image the PRIMARY screen is holding, oldest first — what the renderer walks each frame
	/// while that page is up.
	pub fn placements(&self) -> &[Placement] {
		&self.primary.placements
	}

	/// The same for the alternate page (§41). Empty until a full-screen program draws one, and empty
	/// again the moment it swaps back.
	pub fn alternate(&self) -> &[Placement] {
		&self.alternate.placements
	}

	/// Drop the pictures on the visible screen, for `CSI 2 J`. `first_visible` is the absolute line
	/// the live screen starts at (the engine's `history_size`), so a picture anchored above it is in
	/// the scrollback and survives — the same split the erase itself makes in the text.
	pub fn clear_screen(&mut self, first_visible: u64) {
		self.primary
			.retain(|placement| placement.line < first_visible);
	}

	/// Drop the pictures in the scrollback, for `CSI 3 J` — the mirror of `clear_screen`.
	pub fn clear_scrollback(&mut self, first_visible: u64) {
		self.primary
			.retain(|placement| placement.line >= first_visible);
	}

	/// Move every picture's anchor through a renumbering of the document (§101).
	///
	/// UNSCROLL pulls lines out of the scrollback and onto the page, which shifts absolute anchors
	/// below the seam and discards the page's bottom rows. A picture whose anchor line is discarded
	/// goes with it — the same call `clear_screen` makes and for the same reason, that a picture on a
	/// line that no longer exists would be drawn against text it never described.
	///
	/// The ALTERNATE page is untouched, and cannot be reached by this: its anchors are page rows
	/// rather than document lines (§41), and a page that keeps no scrollback has nothing to unscroll
	/// from.
	pub fn renumber(&mut self, remap: impl Fn(u64) -> Option<u64>) {
		self.primary.renumber(remap);
	}

	/// Drop the alternate page's pictures and nothing else (§41) — for the swap on or off that page,
	/// and for a `CSI 2 J` while it is up. They belong to the program that drew them: it owns the
	/// whole page, it repaints all of it, and it leaves nothing behind when it goes.
	pub fn clear_alternate(&mut self) {
		self.alternate.clear();
	}

	/// Retire the alternate page's pictures the program has since drawn text over (§41), `covered`
	/// answering whether a placement's cell box now holds any glyph.
	///
	/// This is the closest cmote gets to what a terminal with native graphics has for free: there the
	/// picture lives IN the cells, so writing a character erases the pixels under it. Here the picture
	/// is an object beside the grid, and the box it reserved was left blank — so a glyph appearing in
	/// it means the program has repainted over the picture and the picture is stale. The whole picture
	/// goes rather than the covered part of it: cmote does not cut pixels out of an image, and a
	/// half-erased plot would be a worse lie than no plot (§41's "not reflowed, dropped" trade).
	pub fn retire_covered_alternate(&mut self, covered: impl Fn(&Placement) -> bool) {
		self.alternate.retain(|placement| !covered(placement));
	}

	/// Drop every picture on both pages. Used for RIS, and for a resize — a reflow changes how many
	/// lines the history holds, so every absolute anchor stops meaning what it did (`ponytail:` the
	/// same trade-off the prompt marks make, §34: a picture that would land on the wrong line is
	/// better gone than wrong, and it is cleared even on a height-only resize that reflows nothing).
	pub fn clear(&mut self) {
		self.primary.clear();
		self.alternate.clear();
	}
}

/// How many cells `pixels` needs when one cell is `cell` pixels — rounding up, and never zero for a
/// picture with any pixels at all. Saturating, so even a pathological cell size cannot wrap the
/// count round to nothing.
fn cells(pixels: u16, cell: u16) -> u16 {
	let cell = u32::from(cell.max(1));
	let count = u32::from(pixels).div_ceil(cell);
	u16::try_from(count.clamp(1, u32::from(u16::MAX))).unwrap_or(u16::MAX)
}

/// Which pictures a finished CSI takes with it, or `None` when it takes none.
///
/// `CSI 2 J` erases the screen and `CSI 3 J` the scrollback; `CSI J` and `CSI 0/1 J` erase part of one
/// line's worth, which no image spans, so they are left alone. The marker and the intermediates are
/// tested too — `CSI ? 2 J` is the selective erase `term/protect.rs` reads, and protection is a
/// property of text rather than of pictures.
///
/// **A sub-parameter is READ here, not refused, and that is the opposite of what `term/rect.rs` does
/// with one.** The two are right for the same reason: `Csi::sub_parameters` reports the fact and the
/// policy belongs to the scanner. `rect` reads DECERA, which the engine has no arm for, so cmote is
/// the only actor and refusing a spelling DEC never defined costs nothing. ED is the other way round —
/// the engine DOES erase, `next_param_or(0)` reads the first sub-parameter of the first parameter, and
/// `differential.rs` measures it: `CSI 2:3 J` wipes the screen. A scanner that refused that spelling
/// would leave every picture standing on a screen whose text had gone, which is §106's defect shape
/// and was live here until §111.
fn erase(csi: &super::csi::Csi<'_>) -> Option<GraphicsEvent> {
	if csi.marker().is_some() || !csi.intermediates().is_empty() || csi.final_byte() != b'J' {
		return None;
	}
	// The engine's own `next_param_or(0)`, so an erase means the same thing on both sides of `process`.
	// Saturating, which `Csi::param` already is: a parameter past `u16` is one nobody can act on, and
	// clamping keeps a long digit run from wrapping round into a small plausible number like 2.
	match csi.param(0).unwrap_or(0) {
		2 => Some(GraphicsEvent::ClearScreen),
		3 => Some(GraphicsEvent::ClearScrollback),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A minimal one-pixel-wide red sixel: `DCS q #0;2;100;0;0 ~ ST`, six pixels tall.
	const RED_COLUMN: &[u8] = b"\x1bPq#0;2;100;0;0~\x1b\\";

	/// Feed one slice to a fresh scanner and return what it found.
	fn scan(bytes: &[u8]) -> Vec<(usize, GraphicsEvent)> {
		Images::default().feed(bytes)
	}

	/// The single image in a scan result, or a panic naming what was found instead.
	fn only_image(found: Vec<(usize, GraphicsEvent)>) -> (usize, sixel::Image) {
		match found.into_iter().next() {
			Some((offset, GraphicsEvent::Image(image))) => (offset, image),
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
		// the engine has not erased yet (see `GraphicsEvent`). A partial erase (`CSI 0 J`, `CSI K`) spans no
		// picture, so it says nothing.
		assert!(matches!(
			scan(b"text\x1b[2J").as_slice(),
			[(4, GraphicsEvent::ClearScreen)]
		));
		assert!(matches!(
			scan(b"text\x1b[3J").as_slice(),
			[(4, GraphicsEvent::ClearScrollback)]
		));
		assert!(matches!(
			scan(b"text\x1bc").as_slice(),
			[(4, GraphicsEvent::Reset)]
		));
		assert!(scan(b"\x1b[0J").is_empty());
		assert!(scan(b"\x1b[K").is_empty());
		// And the same three arriving INSIDE a control string, which is where they used to be lost
		// (§111). An ESC ends the string for the engine and opens the sequence that follows it, so a
		// reset written mid-picture really does reset — and a scanner that dropped to ordinary text
		// there left every picture standing on a screen that had just been wiped.
		//
		// The offsets are the ESC's own position, counted from the front of the chunk: the erases are
		// reported BEFORE their bytes, so an abandoned payload must not leave the offset pointing at
		// the picture it gave up on.
		let sixel = b"\x1bPq#0;2;0;0;0#0~~";
		assert!(matches!(
			scan(b"\x1bPq#0;2;0;0;0#0~~\x1bc").as_slice(),
			[(at, GraphicsEvent::Reset)] if *at == sixel.len()
		));
		assert!(matches!(
			scan(b"\x1bPq~~\x1b[2J").as_slice(),
			[(5, GraphicsEvent::ClearScreen)]
		));
		// An UNRECOGNISED control string too — a DECRQSS reply, say, which this module follows to its
		// terminator without reading a byte of it.
		assert!(matches!(
			scan(b"\x1bP$qm\x1bc").as_slice(),
			[(5, GraphicsEvent::Reset)]
		));
		// A SUB-PARAMETER is read rather than refused, which is the opposite of what `term/rect.rs`
		// does with one and right for the opposite reason: the engine has an arm for ED and none for
		// DECERA, so `CSI 2:3 J` really does wipe the screen (`differential.rs` measures it) and a
		// scanner that refused the spelling would leave every picture standing on it. Live here until
		// the shared grammar arrived (§111).
		assert!(matches!(
			scan(b"\x1b[2:3J").as_slice(),
			[(0, GraphicsEvent::ClearScreen)]
		));
		assert!(matches!(
			scan(b"\x1b[3:1J").as_slice(),
			[(0, GraphicsEvent::ClearScrollback)]
		));
		// A selective erase is `term/protect.rs`'s, and protection is a property of text rather than of
		// pictures — so the marker rules it out here.
		assert!(scan(b"\x1b[?2J").is_empty());
	}

	/// A picture is reported PAST its terminator and an erase BEFORE its first byte, so a `clear` right
	/// after an image gives both events the SAME offset — and the order they come out in decides
	/// whether the picture survives (§111).
	///
	/// This is what the two passes in `feed` are ordered for: the control strings are scanned first and
	/// the sort is stable, so at a tie the draw comes before the erase, which is the order the stream
	/// put them in. `img2sixel; clear` sends exactly these bytes.
	#[test]
	fn a_picture_and_the_erase_that_follows_it_come_out_in_stream_order() {
		let bytes = b"\x1bPq#0;2;100;0;0#0~-~\x1b\\\x1b[2J";
		let found = scan(bytes);
		let [
			(drawn, GraphicsEvent::Image(_)),
			(erased, GraphicsEvent::ClearScreen),
		] = found.as_slice()
		else {
			panic!("expected a picture then an erase, got {found:?}");
		};
		assert_eq!(
			drawn, erased,
			"the terminator's far side and the erase's near side are one byte"
		);
		// A shell's `clear` sends both erases, which between them clear everything.
		assert_eq!(scan(b"\x1b[3J\x1b[H\x1b[2J").len(), 2);
	}

	#[test]
	fn an_erase_is_read_as_a_number_the_way_the_engine_reads_it() {
		// The engine takes ED's parameter with `next_param_or(0)`: leading zeros are just zeros, and a
		// second parameter is ignored. So `CSI 002 J` and `CSI 2;5 J` both erase the screen there, and a
		// scanner that compared the parameter BYTES to `b"2"` said nothing about either — leaving the
		// pictures on a screen whose text had gone.
		assert!(matches!(
			scan(b"\x1b[002J").as_slice(),
			[(0, GraphicsEvent::ClearScreen)]
		));
		assert!(matches!(
			scan(b"\x1b[2;5J").as_slice(),
			[(0, GraphicsEvent::ClearScreen)]
		));
		assert!(matches!(
			scan(b"\x1b[003J").as_slice(),
			[(0, GraphicsEvent::ClearScrollback)]
		));
		// And the partial erases stay silent, however they are spelled.
		assert!(scan(b"\x1b[000J").is_empty());
		assert!(scan(b"\x1b[;2J").is_empty());
	}

	#[test]
	fn a_long_parameter_run_before_an_erase_is_still_an_erase() {
		// The other half of reading the parameter the engine's way: it counts PARAMETERS, not parameter
		// bytes, and saturates a long digit run rather than abandoning the sequence. So a padded ED
		// erases the screen there — and a scanner that gave up on the byte count left the pictures on it.
		assert!(matches!(
			scan(b"\x1b[000000000000000002J").as_slice(),
			[(0, GraphicsEvent::ClearScreen)]
		));
	}

	#[test]
	fn an_erase_split_across_chunks_is_reported_at_the_start_of_its_chunk() {
		// The sequence began in the previous chunk, so "before its bytes" is this chunk's offset 0 —
		// the engine is then exactly where it was when the erase started arriving.
		let mut images = Images::default();
		assert!(images.feed(b"text\x1b[").is_empty());
		assert!(matches!(
			images.feed(b"2J").as_slice(),
			[(0, GraphicsEvent::ClearScreen)]
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

	/// A one-pixel picture, for the tests that only care where a placement lands.
	fn dot() -> sixel::Image {
		sixel::Image {
			width: 1,
			height: 1,
			rgba: vec![0; 4],
		}
	}

	/// The two pages are separate stores (§41): a full-screen program's pictures never appear among
	/// the scrollback's, and the erases that split the primary screen's by line leave the alternate
	/// page's alone — it has no history for them to split against.
	#[test]
	fn the_two_pages_hold_their_pictures_apart() {
		let mut images = Images::default();
		images.place(dot(), 40, 0);
		images.place_alternate(dot(), 3, 0);
		assert_eq!(images.placements().len(), 1);
		assert_eq!(images.alternate().len(), 1);
		assert_eq!(
			images.alternate()[0].line,
			3,
			"the row, not a document line"
		);

		images.clear_screen(0);
		assert!(images.placements().is_empty());
		assert_eq!(images.alternate().len(), 1, "a different page, untouched");

		images.clear_alternate();
		assert!(images.alternate().is_empty());
	}

	/// A picture whose box the new one overlaps is REPLACED on the alternate page, because a
	/// full-screen program redraws the same pane over and over: each frame of a video is the successor
	/// of the last, not a second picture beside it. One that overlaps nothing is a second pane, and
	/// stays.
	#[test]
	fn a_new_alternate_picture_replaces_the_one_it_covers() {
		let mut images = Images::default();
		images.set_cell_pixels(7, 14);
		let wide = || sixel::Image {
			width: 21,
			height: 30,
			rgba: vec![0; 21 * 30 * 4],
		};
		// Three rows by three columns from row 2, column 0 — then the same box again.
		images.place_alternate(wide(), 2, 0);
		images.place_alternate(wide(), 2, 0);
		assert_eq!(images.alternate().len(), 1, "the redraw took the old frame");

		// Touching its bottom-right corner: rows 4-6 and columns 2-4 still overlap.
		images.place_alternate(wide(), 4, 2);
		assert_eq!(images.alternate().len(), 1);

		// Clear of it in both axes: another pane, so both are kept.
		images.place_alternate(wide(), 8, 6);
		assert_eq!(images.alternate().len(), 2);
	}

	/// The sweep retires the alternate page's pictures a program has drawn over, and only those — the
	/// primary screen's are not its business (§41).
	#[test]
	fn retiring_covered_pictures_touches_only_the_alternate_page() {
		let mut images = Images::default();
		images.place(dot(), 5, 0);
		images.place_alternate(dot(), 5, 0);
		images.place_alternate(dot(), 6, 0);

		images.retire_covered_alternate(|placement| placement.line == 5);
		assert_eq!(images.alternate().len(), 1);
		assert_eq!(images.alternate()[0].line, 6);
		assert_eq!(images.placements().len(), 1, "the other page is untouched");
	}

	#[test]
	fn a_reset_or_a_resize_drops_everything() {
		let mut images = Images::default();
		images.place(dot(), 0, 0);
		images.place_alternate(dot(), 0, 0);
		images.clear();
		assert!(images.placements().is_empty());
		assert!(
			images.alternate().is_empty(),
			"both pages, not just the one"
		);
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

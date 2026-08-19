// cursor.rs — the open and closed hand over everything cmote lets you pick up (PLAN §51).
//
// Two surfaces are grabbable, and they say so with the same pair of cursors: a **tab chip**, which
// drags along the strip to a new slot (§38), and a **dialog header**, which drags the card around
// the window (§10). Whatever is added next says it the same way by calling this module — the point
// of a shared affordance is that the user learns it once.
//
// CSS names those two cursors for "this can be picked up and moved": `grab`, an open hand, and
// `grabbing`, a closed one. Every browser shows them, so they are what a user expects over a
// draggable thing, and iced names them too — `mouse::Interaction::Grab` / `Grabbing`. Asking for
// them is not enough on Windows, and the reason is worth writing down because it is invisible from
// the Rust side:
//
//   * **Windows has no hand cursors.** The system set (`IDC_*`) has an arrow, an I-beam, a
//     four-arrow move, resize arrows, a wait ring and `IDC_HAND` — which is the POINTING finger
//     links use, not a hand that can hold something. There is no open palm and no fist.
//   * **winit therefore collapses both** into one: `CursorIcon::Grab | Grabbing | Move | AllScroll
//     => IDC_SIZEALL`. So the strip asked for two different hands and got the same four-arrow for
//     hover and for drag — the gesture said nothing about its own state.
//   * **iced exposes no custom cursor.** winit 0.30 can load one from pixels (`CustomCursor`), but
//     `iced_winit` only ever calls `window.set_cursor(CursorIcon)`, and iced hands out no winit
//     `Window`. There is no seam to pass an image through.
//
// Browsers solve this by shipping their own bitmaps — Firefox has `widget/windows/res/grab.cur`,
// Chromium its own resources — so cmote does the same: `assets/cursor-grab.png` and
// `assets/cursor-grabbing.png`, drawn for cmote, bundled into the binary and decoded at start-up.
// They are our own art under our own licence, so nothing third-party (Firefox's cursors are
// MPL-2.0) comes into the tree with them.
//
// Painting them takes one Win32 seam. winit answers `WM_SETCURSOR` itself — whenever the pointer
// is over the client area it calls `SetCursor` with the icon iced last asked for — so a cursor set
// from anywhere else is overwritten on the next mouse move. The window is subclassed and that one
// message is answered first: while a hand is wanted, this module sets it and returns TRUE, and
// winit's handler never runs. Every other message, and every moment no hand is wanted, is passed
// straight through, so nothing else about the window's behaviour changes.
//
// A handle therefore asks iced for NO interaction on Windows (`grab_interaction`) — if it asked for
// `Grab`, iced would set `IDC_SIZEALL` the moment the hover or the press changed the interaction,
// stomping the hand until the next mouse move. Off Windows nothing here does anything and the
// handle asks for the real thing, which those platforms draw as an actual hand.
//
// What this module is told, and by whom:
//   * `hover_entered` / `hover_exited` — a handle's own pointer events, raised as
//     `Message::GrabEntered` / `GrabExited` by whichever widget wears the hand, each naming itself;
//   * `control_entered` / `control_exited` — a button sitting ON a handle taking the pointer and
//     giving it back: a chip's "×", a dialog header's ✕. They win while they have it, because the
//     pointer is then over something to click rather than something to pick up;
//   * `drawn`, between `frame_begin` and `frame_end` — every handle says it is still on screen as
//     the frame is built, which is how a handle that VANISHED under the pointer lets go (below);
//   * `covered` — a modal's backdrop went over everything, so the handles beneath it cannot be
//     picked up while it is there;
//   * `hover_reset` — the pointer left a whole region of handles (the tab strip says this when it
//     is left), so none of them can still have it;
//   * `set_dragging` — a press picked something up, and the drop or the cancel put it down: a chip
//     grab and its drop (§38), a dialog header grab and its release (§10).
//
// The hand is a CLAIM held by one named handle, and the name is what makes it correct (§52). iced
// publishes a widget's `on_exit` from the widget itself, so a handle that is no longer in the tree
// never publishes anything: press a dialog's ✕ while the pointer is on its header, close a chip
// under the pointer, or send a tab to another region, and the exit that would have let go is never
// raised. An anonymous count could only be healed by wandering back over some other handle — which
// is why every claim now has to be RE-ASSERTED each frame by the handle drawing itself, and one that
// is not drawn has let go whether or not it managed to say so.
//
// Naming the claimant also settles the ordering trap the count was there for: leaving one handle and
// entering the next both fire on the same mouse move, and iced dispatches them in the widgets'
// layout order rather than the order the pointer crossed them. An exit only clears the claim if it
// is the claimant's own, so the chip being left cannot cancel the chip being entered.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Which hand the window should be wearing, if either.
///
/// **Compiled on Windows and under `cfg(test)` only (§80).** Off Windows nothing asks the question:
/// the toolkit has both hands of its own, `grab_interaction` asks it for them, and nothing here
/// paints anything — so a mac binary would carry an enum and a decision procedure no code consults.
/// `dead_code` says exactly that, and CI's clippy over the shipped `x86_64-apple-darwin` target runs
/// with `-D warnings`, so it says it as an error. The right answer is not to silence the lint but to
/// agree with it: this is Windows' half of the module.
///
/// `test` is in the predicate deliberately. The state machine below is plain atomics and no
/// platform, so it is worth checking wherever the suite runs — CI runs `cargo test` natively on the
/// mac runner too, and these tests should not quietly stop existing there just because the shipped
/// mac build has no use for what they cover.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
	/// Nothing to hold: leave the cursor to iced.
	None,
	/// Over something draggable — the open hand, CSS `grab`.
	Open,
	/// Holding it — the closed hand, CSS `grabbing`.
	Closed,
}

/// The name a dialog header goes by (§52). Handles are named by their tab's id, which is app-wide
/// and never reused (§26), and a header has no id of its own — but at most one dialog is ever open
/// per region, and no tab id can collide with this one.
pub const HEADER: u64 = u64::MAX;

/// Whether a handle currently has the pointer, and — in `CLAIM` — which.
///
/// Two flags rather than an `Option<u64>` because both are read from the `WM_SETCURSOR` handler,
/// which must not take a lock. They are only ever written from the window's own thread (iced's
/// `update` and `view` both run there), so the pair cannot be seen half-updated by anything that
/// matters.
static HELD: AtomicBool = AtomicBool::new(false);
static CLAIM: AtomicU64 = AtomicU64::new(0);

/// Whether the claimant drew itself during the frame being built. Cleared at the start of each
/// frame and set by the handle's own `drawn` call; a frame that ends without it means the handle is
/// gone from the tree and its claim goes with it.
static SEEN: AtomicBool = AtomicBool::new(false);

/// Whether a CONTROL sitting on a handle has the pointer, and — in `BLOCKER` — which handle it
/// belongs to (§52).
///
/// A handle is not uniform: a tab chip carries its own "×", a dialog header carries its ✕. Those are
/// buttons, they have their own cursor, and they win — the pointer is over something to CLICK, not
/// something to pick up, and offering to drag a close button is an invitation to lose work. The
/// handle's own `mouse_area` cannot tell: it reports the pointer being anywhere inside its bounds,
/// children included, so the control has to say so itself.
///
/// A separate claim rather than a flag on the handle's, because iced updates a CHILD before its
/// parent: entering a chip directly on its "×" raises the control's enter first, and folding the two
/// into one would let the handle's enter — arriving second — clear a block that is still true.
static BLOCKED: AtomicBool = AtomicBool::new(false);
static BLOCKER: AtomicU64 = AtomicU64::new(0);

/// The same "did it redraw" question for the control, and just as necessary: a chip's "×" that
/// closes its own tab takes itself off the screen with the pointer still on it, and would otherwise
/// leave the hand suppressed for good.
static SEEN_CONTROL: AtomicBool = AtomicBool::new(false);

/// Whether something is being dragged right now, which outranks hovering: the hand stays closed
/// while the button is down wherever the pointer has got to, which is what says the gesture is
/// still in flight. A dialog dragged clean off its header keeps the closed hand for the same reason
/// a chip dragged onto the terminal does — the thing is still held.
static DRAGGING: AtomicBool = AtomicBool::new(false);

/// The handle named `handle` took the pointer.
pub fn hover_entered(handle: u64) {
	CLAIM.store(handle, Ordering::Relaxed);
	HELD.store(true, Ordering::Relaxed);
	// It cannot have been entered without being on screen, and the frame it was entered during is
	// already built — so it counts as seen, or the claim would be dropped before its first redraw.
	SEEN.store(true, Ordering::Relaxed);
	apply();
}

/// The handle named `handle` lost the pointer. Only ITS OWN claim is cleared: on the mouse move
/// that crosses from one chip to the next, iced may raise the enter before the exit, and a handle
/// that has already handed the hand on must not take it away again.
pub fn hover_exited(handle: u64) {
	if HELD.load(Ordering::Relaxed) && CLAIM.load(Ordering::Relaxed) == handle {
		HELD.store(false, Ordering::Relaxed);
		apply();
	}
}

/// A control ON the handle named `handle` took the pointer — a chip's "×", a dialog header's ✕ —
/// so the hand gives way to whatever cursor that control asks for.
pub fn control_entered(handle: u64) {
	BLOCKER.store(handle, Ordering::Relaxed);
	BLOCKED.store(true, Ordering::Relaxed);
	SEEN_CONTROL.store(true, Ordering::Relaxed);
	apply();
}

/// That control lost the pointer — back onto the handle around it, or off both. Only its own block
/// is lifted, for the same reason an exit only clears its own claim.
pub fn control_exited(handle: u64) {
	if BLOCKED.load(Ordering::Relaxed) && BLOCKER.load(Ordering::Relaxed) == handle {
		BLOCKED.store(false, Ordering::Relaxed);
		apply();
	}
}

/// The pointer left a whole region of handles — the tab strip, say — so neither they nor the
/// controls on them can still have it, whoever holds the claim.
pub fn hover_reset() {
	HELD.store(false, Ordering::Relaxed);
	BLOCKED.store(false, Ordering::Relaxed);
	apply();
}

/// A frame is being built: nothing has redrawn itself yet.
pub fn frame_begin() {
	SEEN.store(false, Ordering::Relaxed);
	SEEN_CONTROL.store(false, Ordering::Relaxed);
}

/// The handle named `handle` drew itself into the frame being built, so it is still there to be
/// picked up. Called by the handle's own view code, which is the only place that knows.
///
/// It answers for the control on it too: the two are drawn by the same code, so one call says both
/// are still on screen and there is no second thing for a view to remember to do.
pub fn drawn(handle: u64) {
	if HELD.load(Ordering::Relaxed) && CLAIM.load(Ordering::Relaxed) == handle {
		SEEN.store(true, Ordering::Relaxed);
	}
	if BLOCKED.load(Ordering::Relaxed) && BLOCKER.load(Ordering::Relaxed) == handle {
		SEEN_CONTROL.store(true, Ordering::Relaxed);
	}
}

/// A modal's backdrop went over the window, so whatever is under it cannot be picked up (§52). The
/// dialog's own header is exempt — it is drawn ON the backdrop, and dragging the card is exactly
/// what it is for.
///
/// This under-claims rather than over-claims: when the dialog closes with the pointer still resting
/// on a chip, that chip believes it is hovered and will raise no fresh enter, so the hand comes back
/// only when the pointer leaves and returns. A missing hand over something draggable is a smaller
/// lie than a hand over something that cannot be dragged.
pub fn covered() {
	if HELD.load(Ordering::Relaxed) && CLAIM.load(Ordering::Relaxed) != HEADER {
		HELD.store(false, Ordering::Relaxed);
		apply();
	}
	if BLOCKED.load(Ordering::Relaxed) && BLOCKER.load(Ordering::Relaxed) != HEADER {
		BLOCKED.store(false, Ordering::Relaxed);
	}
}

/// The frame is built: a claimant that never drew itself is not on screen any more, so it has let
/// go whether or not it managed to say so (§52). The same for a control that blocked the hand — a
/// "×" that closed its own tab is not there to say it lost the pointer.
pub fn frame_end() {
	let mut changed = false;
	if HELD.load(Ordering::Relaxed) && !SEEN.load(Ordering::Relaxed) {
		HELD.store(false, Ordering::Relaxed);
		changed = true;
	}
	if BLOCKED.load(Ordering::Relaxed) && !SEEN_CONTROL.load(Ordering::Relaxed) {
		BLOCKED.store(false, Ordering::Relaxed);
		changed = true;
	}
	if changed {
		apply();
	}
}

/// A drag started or ended: a chip picked up or dropped (§38), a dialog header grabbed or released
/// (§10).
pub fn set_dragging(dragging: bool) {
	DRAGGING.store(dragging, Ordering::Relaxed);
	apply();
}

/// Test-only: the hand state is process-wide (one pointer, one window), so a test that touches it
/// takes this first. `cargo test` runs the suite on several threads at once, and two tests stepping
/// on the same two atomics would fail each other at random.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Test-only: back to "nothing held, nothing hovered", so a test starts from a known state and
/// leaves one behind.
#[cfg(test)]
pub(crate) fn forget() {
	HELD.store(false, Ordering::Relaxed);
	SEEN.store(false, Ordering::Relaxed);
	BLOCKED.store(false, Ordering::Relaxed);
	SEEN_CONTROL.store(false, Ordering::Relaxed);
	DRAGGING.store(false, Ordering::Relaxed);
}

/// Which hand the window should be wearing now.
///
/// A drag outranks everything: once something is held, the hand stays closed wherever the pointer
/// has got to — over a close button included, since the gesture is what is being reported. At rest,
/// a control on the handle outranks the handle: the pointer is over something to click.
///
/// Windows and the tests only, for the reason given on `Hand` (§80) — off Windows the only caller
/// this ever had, the `WM_SETCURSOR` subclass, is not compiled.
#[cfg(any(windows, test))]
pub fn hand() -> Hand {
	if DRAGGING.load(Ordering::Relaxed) {
		Hand::Closed
	} else if HELD.load(Ordering::Relaxed) && !BLOCKED.load(Ordering::Relaxed) {
		Hand::Open
	} else {
		Hand::None
	}
}

/// What a grab handle should ask ICED for — nothing on Windows, as long as there is a hand to paint.
///
/// `None` means "do not call `mouse_area::interaction` at all". iced only tells winit to change the
/// cursor when the requested interaction CHANGES — so leaving it alone over the handle means winit
/// never sets a cursor there, and the hand this module paints is never stomped mid-gesture. Asking
/// for `Grab` instead would set `IDC_SIZEALL` on every hover and every press, i.e. at exactly the
/// two moments the hand is supposed to change.
///
/// With **no drawing to paint** the answer flips to `Grab` / `Grabbing`, which winit collapses to
/// that same four-arrow move cursor. It says less than a hand — it cannot tell hovering from holding
/// — but it does say "this can be moved", which is the affordance, and it is the behaviour the strip
/// had before §51. That is the fallback whenever `assets/cursor-*.png` is empty or unreadable, and
/// it is also what shows for the moment between the window opening and the boot task installing the
/// cursors.
///
/// Off Windows the answer is always the real thing: those platforms have hand cursors and draw them.
#[cfg(windows)]
pub fn grab_interaction(dragging: bool) -> Option<iced::mouse::Interaction> {
	if platform::hands_ready() {
		return None;
	}
	Some(if dragging {
		iced::mouse::Interaction::Grabbing
	} else {
		iced::mouse::Interaction::Grab
	})
}

/// The same, on a platform whose toolkit already has the two hands.
#[cfg(not(windows))]
pub fn grab_interaction(dragging: bool) -> Option<iced::mouse::Interaction> {
	Some(if dragging {
		iced::mouse::Interaction::Grabbing
	} else {
		iced::mouse::Interaction::Grab
	})
}

// --- the art ---
//
// EVERYTHING FROM HERE TO THE WINDOWS SEAM IS WINDOWS' AND THE TESTS' (§80). The drawings exist
// because Windows has no hand cursors and iced offers no seam to pass a picture through; every
// other platform draws its own, so off Windows there is nobody to hand a decoded PNG to. The bundled
// bytes, the hotspot, the `Drawing` they decode into and the resampler that fits them are therefore
// compiled for `windows` and for `test`, and for nothing else — `dead_code` is right about them on a
// mac build, and CI's `-D warnings` over `x86_64-apple-darwin` is where that shows up.
//
// `test` keeps the drawings under test on EVERY platform, which is the half worth having: the
// hotspot check and the resampler's alpha weighting are facts about the pictures and the arithmetic,
// not about the OS, and CI runs `cargo test` on the mac runner as well as the Windows one.

/// The two hands, as they were drawn (§51).
///
/// They were `const` ASCII art to begin with — one character per pixel, reviewable in a diff, no
/// third-party asset or its licence in the tree. Two rounds of drawing them that way settled the
/// argument the other way: a cursor is a *drawing*, judged by eye at 32 pixels, and a drawing wants
/// a drawing tool. Editing a hand by moving characters in a grid is slow, and — the part that
/// actually decided it — the result looked hand-assembled rather than drawn.
///
/// So the shapes live in `assets/` as PNGs and are read back at start-up. They are cmote's own art
/// under cmote's own licence, so the reason the ASCII art existed — keeping someone else's cursor
/// file and its licence out of the repository (Firefox's are MPL-2.0) — does not apply to them.
///
/// **Bundled into the binary**, not read from disk beside it: the fonts already ride along this way
/// (§9, §19), and §11's promise is one portable executable. Changing a hand is therefore: overwrite
/// the file, rebuild.
///
/// Either file may be **empty**, which is what it means for a hand not to have been drawn yet. There
/// is then nothing to paint, the subclass is never fitted, and the handles fall back to asking iced
/// for `Grab` / `Grabbing` — the four-arrow move cursor Windows collapses those to, which is what
/// the strip showed before §51 (see `grab_interaction`). The file has to EXIST for `include_bytes!`
/// to compile, which is why an empty one is the placeholder rather than no file at all.
#[cfg(any(windows, test))]
const GRAB_PNG: &[u8] = include_bytes!("../assets/cursor-grab.png");
#[cfg(any(windows, test))]
const GRABBING_PNG: &[u8] = include_bytes!("../assets/cursor-grabbing.png");

/// Where the click lands, in the bundled drawing's OWN pixels (§51).
///
/// ONE hotspot for both shapes, and inside the part they share: press and the hand closes without
/// the pointer appearing to jump. A hand cursor is aimed with its middle rather than with a tip —
/// there is no tip to aim with — which is how the browsers place theirs too. This one is the middle
/// of the drawn hand: it sits between the two shapes' centres of area, on the palm of each.
///
/// In the DRAWING's pixels, not the cursor's: the art is 64×64 and Windows is handed whatever size
/// it asks for (see `install`), so this is scaled along with the image. A test pins it on an opaque
/// pixel of both drawings, so redrawing a hand that no longer covers it fails the build rather than
/// quietly clicking somewhere the user is not pointing.
#[cfg(any(windows, test))]
const HOTSPOT: (u32, u32) = (30, 34);

/// How much of the cursor BOX the hand is drawn into, as a fraction of the size Windows asks for
/// (§51).
///
/// `SM_CXCURSOR` is the size of the box a cursor is drawn in, not the size of the drawing in it —
/// and the standard arrow uses barely two thirds of its own: a 32×32 arrow bitmap carries a glyph
/// about twenty pixels tall with empty space around it. Artwork that fills its box edge to edge
/// therefore comes out visibly bigger than every other cursor on the screen, which is exactly how
/// the first fitted build looked.
///
/// So the hands are fitted to this fraction of the box instead, which puts them at about the arrow's
/// own footprint. It is the one number to turn if they still read large or start to read small: at
/// 100% scaling the box is 32 pixels, so the hands come out at 21.
///
/// `windows` alone rather than `any(windows, test)` like the rest of the art (§80), and the
/// difference is a fact rather than a nicety: no test reads this number, because the thing it feeds
/// — `scaled`, inside the Win32 seam — cannot run anywhere else. It is the one constant here whose
/// value is checked by eye on a Windows desktop and by nothing at all.
#[cfg(windows)]
const COVERAGE: f32 = 0.65;

/// One decoded cursor: its pixels as 32-bit BGRA, top row first — the order a Windows top-down DIB
/// wants — and the size they are that of.
#[cfg(any(windows, test))]
struct Drawing {
	bgra: Vec<u8>,
	width: u32,
	height: u32,
}

/// Decode one bundled PNG into `Drawing`, or `None` if it cannot be read.
///
/// `None` is not a defensive nicety: the images are `include_bytes!`d at compile time, so the only
/// way here is a file that was replaced with something that is not a PNG. The whole cursor feature
/// then does nothing and the handles keep the cursor they had, which is the same failure `install`
/// already has for a subclass that cannot be fitted — worth a broken drawing, not a panic on start.
///
/// The decoder is asked to normalise whatever it finds to 8-bit RGBA, so a hand exported as a
/// palette, as greyscale, or with a `tRNS` chunk instead of an alpha channel all arrive here the
/// same way. Alpha is passed through as drawn: Windows blends a 32-bit cursor with STRAIGHT alpha,
/// so an antialiased edge is drawn as an antialiased edge and there is no premultiplication to get
/// wrong.
///
/// `ponytail:` straight alpha is what 32-bit icons and cursors are documented to use, and what the
/// two-tone hands (opaque or clear, nothing between) cannot tell apart either way. If a redrawn hand
/// with soft edges ever comes out with a dark halo round it, that assumption is the thing to check:
/// the fix is one multiply of each channel by the alpha before the swizzle below.
#[cfg(any(windows, test))]
fn decode_png(bytes: &'static [u8]) -> Option<Drawing> {
	// `Cursor` only because the decoder wants to seek; the bytes are already in the binary.
	let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
	decoder.set_transformations(
		png::Transformations::normalize_to_color8() | png::Transformations::ALPHA,
	);
	let mut reader = decoder.read_info().ok()?;
	let mut buffer = vec![0; reader.output_buffer_size()?];
	let info = reader.next_frame(&mut buffer).ok()?;
	// Only the straight RGBA case is handled. The transformations above are what make that the case
	// for every ordinary export; anything left over (16-bit that did not normalise, say) is refused
	// rather than guessed at.
	if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
		return None;
	}
	let mut bgra = Vec::with_capacity(buffer.len());
	for pixel in buffer[..info.buffer_size()].chunks_exact(4) {
		// B, G, R, A — the byte order of a little-endian 0xAARRGGBB pixel, which is what the DIB
		// below is declared to hold. The PNG hands them over as R, G, B, A.
		bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
	}
	Some(Drawing {
		bgra,
		width: info.width,
		height: info.height,
	})
}

/// Resample a drawing to `width` × `height`, so a hand drawn at one size can be handed to Windows at
/// the size Windows actually wants (§51).
///
/// A box filter — each output pixel is the average of the source pixels it covers. Which is exact
/// for the halving this does in practice (64 drawn, 32 asked for at 100% DPI) and good enough for
/// the rest; a cursor is 32 pixels of artwork, and a sharper kernel would be measuring something
/// nobody can see.
///
/// The colour is averaged **weighted by alpha**, and that part is not optional: a transparent pixel
/// in a PNG still carries a colour, usually black, and averaging it in unweighted would ring every
/// soft edge with a dark halo. Weighting means fully transparent neighbours contribute nothing,
/// which is what "resize this shape" has to mean.
#[cfg(any(windows, test))]
fn resampled(drawing: &Drawing, width: u32, height: u32) -> Drawing {
	let mut bgra = Vec::with_capacity((width * height * 4) as usize);
	for y in 0..height {
		// The source band this output row covers, always at least one row tall.
		let y0 = y * drawing.height / height;
		let y1 = (((y + 1) * drawing.height).div_ceil(height))
			.max(y0 + 1)
			.min(drawing.height);
		for x in 0..width {
			let x0 = x * drawing.width / width;
			let x1 = (((x + 1) * drawing.width).div_ceil(width))
				.max(x0 + 1)
				.min(drawing.width);
			let (mut blue, mut green, mut red, mut alpha, mut count) =
				(0u64, 0u64, 0u64, 0u64, 0u64);
			for sy in y0..y1 {
				for sx in x0..x1 {
					let at = ((sy * drawing.width + sx) * 4) as usize;
					let a = u64::from(drawing.bgra[at + 3]);
					// Premultiplied on the way in, so the average is of the shape and not of the
					// colours hiding behind its transparent pixels.
					blue += u64::from(drawing.bgra[at]) * a;
					green += u64::from(drawing.bgra[at + 1]) * a;
					red += u64::from(drawing.bgra[at + 2]) * a;
					alpha += a;
					count += 1;
				}
			}
			// Undone on the way out: the cursor wants straight alpha, and a patch that was entirely
			// transparent has no colour to recover — it stays a clear pixel.
			// Each channel was premultiplied by its own alpha and is now divided by the alpha total,
			// so every result is back inside 0..=255 — `saturating` says that without a bound check
			// that would have to be kept in step with the arithmetic above it (§111).
			let channel = |value: u64| u8::try_from(value).unwrap_or(u8::MAX);
			let (b, g, r) = blue.checked_div(alpha).map_or((0, 0, 0), |b| {
				(channel(b), channel(green / alpha), channel(red / alpha))
			});
			bgra.extend_from_slice(&[b, g, r, channel(alpha / count.max(1))]);
		}
	}
	Drawing {
		bgra,
		width,
		height,
	}
}

// --- the Windows seam ---

#[cfg(windows)]
mod platform {
	use std::ffi::c_void;
	use std::ptr;
	use std::sync::OnceLock;

	use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
	use windows_sys::Win32::Graphics::Gdi::{
		BI_BITFIELDS, BITMAPINFO, BITMAPV5HEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
		DeleteObject, GetDC, ReleaseDC,
	};
	use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
	use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
	use windows_sys::Win32::UI::WindowsAndMessaging::{
		CreateIconIndirect, GetSystemMetrics, HTCLIENT, ICONINFO, SM_CXCURSOR, SM_CYCURSOR,
		SendMessageW, SetCursor, WM_SETCURSOR,
	};

	use super::{
		COVERAGE, Drawing, GRAB_PNG, GRABBING_PNG, HOTSPOT, Hand, decode_png, hand, resampled,
	};

	/// The two cursors, built once when the window is subclassed. Held as `isize` because a raw
	/// `HCURSOR` is a pointer and therefore not `Sync`; the handles are owned by the process for its
	/// whole life (they are never destroyed — there is exactly one of each, and they are wanted
	/// until the window closes), so passing them through an integer costs nothing but a cast.
	static HANDS: OnceLock<(isize, isize)> = OnceLock::new();

	/// Whether both hands were drawn, decoded and turned into cursors. `false` means there is nothing
	/// to paint — an empty or unreadable `assets/cursor-*.png` — and the handles fall back to asking
	/// iced for `Grab` / `Grabbing`, which Windows draws as the four-arrow move cursor.
	pub fn hands_ready() -> bool {
		HANDS.get().is_some()
	}

	/// The subclassed window, kept so `apply` can hand the cursor BACK when no hand is wanted any
	/// more (see there). Held as an `isize` for the same reason the cursors are.
	static WINDOW: OnceLock<isize> = OnceLock::new();

	/// Any id will do — it only has to be unique among subclasses of THIS window, and cmote
	/// installs exactly one.
	const SUBCLASS_ID: usize = 1;

	/// Build the two cursors and subclass the window so `WM_SETCURSOR` reaches us first.
	///
	/// Called once, from the boot task, with the window's raw handle. Everything after this is
	/// driven by the atomics above: the subclass reads them on each message, so no state has to
	/// travel through Win32.
	///
	/// Failure is silent and harmless. If the cursors cannot be created or the subclass cannot be
	/// installed, `hand()` still tracks the gesture and nothing else in cmote depends on it — the
	/// strip simply keeps the arrow it has today.
	pub fn install(hwnd: isize) {
		if hwnd == 0 {
			return;
		}
		// A drawing that cannot be read leaves the whole feature switched off, exactly as a subclass
		// that cannot be fitted does: the handles fall back to the move cursor (`grab_interaction`)
		// and nothing else notices.
		let (Some(open), Some(closed)) = (decode_png(GRAB_PNG), decode_png(GRABBING_PNG)) else {
			return;
		};
		// The size Windows wants a cursor to be. `SetCursor` does NOT scale, so handing over the
		// 64×64 the hands are drawn at would show a double-size cursor on an ordinary display. This
		// metric already accounts for the display's scaling AND for the user's own cursor-size
		// setting in Accessibility, so it is the one number to ask for. A machine that answers
		// nonsense leaves the drawings at the size they were drawn.
		//
		// Asked FOR THIS WINDOW's DPI, not the system's. iced runs per-monitor-DPI-aware, and in
		// such a process plain `GetSystemMetrics` answers for the DPI the session logged in at — so
		// on a 200% laptop panel beside a 100% primary it would hand back 32 and the hands would
		// come out half the size of every other cursor on that screen. `GetDpiForWindow` is where
		// the window actually is; a zero from it (a window not yet placed) falls back to the
		// system-wide answer, which is the old behaviour and never worse than nothing.
		//
		// `ponytail:` read once, at start-up. Dragging the window to a monitor at a different
		// scaling keeps the hands the size they were built for until cmote is restarted; the
		// upgrade is to rebuild them on `WM_DPICHANGED`, which the subclass already sees.
		// SAFETY: plain metric queries; `hwnd` is iced's own window, used on its own thread.
		let (box_x, box_y) = unsafe {
			match GetDpiForWindow(hwnd as HWND) {
				0 => (GetSystemMetrics(SM_CXCURSOR), GetSystemMetrics(SM_CYCURSOR)),
				dpi => (
					GetSystemMetricsForDpi(SM_CXCURSOR, dpi),
					GetSystemMetricsForDpi(SM_CYCURSOR, dpi),
				),
			}
		};
		// Fitted to a fraction of that box rather than filling it, so the hands sit at about the
		// same visual weight as the arrow they replace (`COVERAGE`). The cursor bitmap is simply
		// made that size — there is no rule that it must be `SM_CXCURSOR`, and padding the drawing
		// out to one would move the hotspot for nothing.
		let (want_x, want_y) = (scaled(box_x), scaled(box_y));
		// Read before fitting: the hotspot is in the DRAWING's pixels, so it moves by the same
		// ratio the image does.
		let (drawn_x, drawn_y) = (open.width.max(1), open.height.max(1));
		let open = fitted(open, want_x, want_y);
		let closed = fitted(closed, want_x, want_y);
		let hotspot = (
			HOTSPOT.0 * open.width / drawn_x,
			HOTSPOT.1 * open.height / drawn_y,
		);
		// SAFETY: the handles are built from our own bundled drawings and are never freed; `hwnd`
		// came from iced's own window and is used on the thread that owns it (the boot task runs on
		// the UI thread, which is the one that pumps this window's messages).
		unsafe {
			let open = cursor_from(&open, hotspot);
			let closed = cursor_from(&closed, hotspot);
			if open.is_null() || closed.is_null() {
				return;
			}
			let _ = HANDS.set((open as isize, closed as isize));
			let _ = WINDOW.set(hwnd);
			SetWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID, 0);
		}
	}

	/// One side of the cursor box, taken down to the share of it a hand is drawn into (`COVERAGE`).
	/// A metric that makes no sense is passed through untouched, and `fitted` then leaves the
	/// drawing alone.
	fn scaled(side: i32) -> i32 {
		if side <= 0 {
			return side;
		}
		#[expect(
			clippy::cast_possible_truncation,
			clippy::cast_precision_loss,
			reason = "a cursor is tens of pixels a side; the rounding is the point"
		)]
		let scaled = ((side as f32) * COVERAGE).round() as i32;
		// Never to nothing: a zero would leave `fitted` with an empty image to build a cursor from.
		scaled.max(8)
	}

	/// A drawing at the size asked for, or untouched when the answer is unusable or already right.
	/// Split out so `install` reads as the three steps it is: decode, fit, build.
	fn fitted(drawing: Drawing, width: i32, height: i32) -> Drawing {
		let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
			return drawing;
		};
		if width == 0 || height == 0 || (width, height) == (drawing.width, drawing.height) {
			return drawing;
		}
		resampled(&drawing, width, height)
	}

	/// The window procedure, ahead of winit's.
	///
	/// Only `WM_SETCURSOR` over the client area is answered, and only while a hand is wanted:
	/// setting the cursor and returning TRUE tells Windows the message is handled, so winit's own
	/// handler — which would put back whatever iced last asked for — never runs for it. Everything
	/// else goes straight on to the rest of the chain, so the window behaves exactly as it did.
	///
	/// The low word of `lparam` is the hit-test result for the position the cursor is at; anything
	/// but `HTCLIENT` is a frame, a border or a scrollbar, whose cursors belong to Windows.
	unsafe extern "system" fn subclass(
		hwnd: HWND,
		message: u32,
		wparam: WPARAM,
		lparam: LPARAM,
		_id: usize,
		_data: usize,
	) -> LRESULT {
		// The hit-test code is the LOW word of `lparam`, so the mask is the whole of the read and the
		// width of what it is masked out of does not matter — `cast_unsigned` keeps the bits and
		// `& 0xffff` discards everything the narrowing would have (§111).
		if message == WM_SETCURSOR
			&& u32::try_from(lparam.cast_unsigned() & 0xffff) == Ok(HTCLIENT)
			&& let Some(cursor) = wanted()
		{
			// SAFETY: a handle this module created and never freed.
			unsafe { SetCursor(cursor as *mut c_void) };
			return 1;
		}
		// SAFETY: the arguments are the ones Windows handed us, passed on unchanged.
		unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
	}

	/// The cursor handle for the hand wanted right now, or `None` to leave the cursor alone.
	fn wanted() -> Option<isize> {
		let (open, closed) = HANDS.get().copied()?;
		match hand() {
			Hand::None => None,
			Hand::Open => Some(open),
			Hand::Closed => Some(closed),
		}
	}

	/// Put the wanted hand on screen right now, without waiting for a mouse move — or give the
	/// cursor back when none is wanted any more.
	///
	/// `WM_SETCURSOR` arrives with mouse MOVEMENT, so between one message and the next there is
	/// nothing to answer, and both halves of that matter. A press that does not move the pointer
	/// still has to close the hand; and the pointer coming to rest on a close button ON a handle
	/// (§52) has to give the hand up there and then, rather than keep it until the user happens to
	/// move again — that arrival is one move, and the `WM_SETCURSOR` it came with was answered before
	/// the enter event reached iced.
	///
	/// Handing it back is done by asking the window the same question Windows would: the message
	/// re-enters the subclass, which now wants no hand and passes it to winit's own handler, and
	/// winit sets whatever iced last asked for — the button's pointing finger. It cannot recurse:
	/// this branch only runs when no hand is wanted, and the subclass never calls back into here.
	///
	/// Safe to call from `update`, which runs on the window's own thread.
	pub fn apply() {
		if let Some(cursor) = wanted() {
			// SAFETY: a handle this module created and never freed.
			unsafe { SetCursor(cursor as *mut c_void) };
		} else if let Some(hwnd) = WINDOW.get().copied() {
			// SAFETY: iced's own window handle, used on the thread that pumps its messages.
			unsafe {
				SendMessageW(
					hwnd as HWND,
					WM_SETCURSOR,
					// The window handle doubles as the WPARAM, which Win32 defines as pointer-wide
					// and unsigned: `cast_unsigned` is the same bits, said out loud. The hit-test
					// code is a small constant widened to a pointer-wide signed LPARAM.
					hwnd.cast_unsigned(),
					LPARAM::try_from(HTCLIENT).expect("HTCLIENT is a one-digit constant"),
				);
			}
		}
	}

	/// Build one cursor from a decoded drawing: a 32-bit top-down DIB for the colour, an empty 1-bit
	/// bitmap for the mask, and `CreateIconIndirect` with `fIcon = FALSE` to make it a CURSOR (an
	/// icon with a hotspot) rather than an icon.
	///
	/// The size comes from the image rather than from a constant, so a hand redrawn at another size
	/// needs no code change. 32×32 is what Windows asks for (`SM_CXCURSOR` at every normal DPI) and
	/// what a `.cur` file would carry; anything else is handed over as-is and scaled by the system.
	///
	/// The mask is all zeroes on purpose. A monochrome cursor uses it to decide which pixels show
	/// and which invert the screen; a 32-bit one carries its own alpha, and Windows uses that when
	/// the mask says "draw everything".
	unsafe fn cursor_from(drawing: &Drawing, hotspot: (u32, u32)) -> *mut c_void {
		let mut header: BITMAPV5HEADER = unsafe { std::mem::zeroed() };
		header.bV5Size = u32::try_from(size_of::<BITMAPV5HEADER>())
			.expect("a Win32 struct is a few dozen bytes");
		// `cast_signed` on the dimensions: a bundled 32x32 PNG, so the reinterpretation is exact, and
		// naming it says the sign bit is understood rather than assumed away (§111).
		header.bV5Width = drawing.width.cast_signed();
		// Negative height means top-down: the first row of bytes is the top row of pixels, which is
		// the order a PNG is read in. A positive height would draw every hand upside down.
		header.bV5Height = -drawing.height.cast_signed();
		header.bV5Planes = 1;
		header.bV5BitCount = 32;
		header.bV5Compression = BI_BITFIELDS;
		header.bV5RedMask = 0x00ff_0000;
		header.bV5GreenMask = 0x0000_ff00;
		header.bV5BlueMask = 0x0000_00ff;
		header.bV5AlphaMask = 0xff00_0000;

		// SAFETY: every call below is given a header we filled in full and buffers we own.
		unsafe {
			let dc = GetDC(ptr::null_mut());
			let mut bits: *mut c_void = ptr::null_mut();
			let color = CreateDIBSection(
				dc,
				ptr::from_ref(&header).cast::<BITMAPINFO>(),
				DIB_RGB_COLORS,
				&raw mut bits,
				ptr::null_mut(),
				0,
			);
			ReleaseDC(ptr::null_mut(), dc);
			if color.is_null() || bits.is_null() {
				return ptr::null_mut();
			}
			ptr::copy_nonoverlapping(drawing.bgra.as_ptr(), bits.cast::<u8>(), drawing.bgra.len());

			let mask = CreateBitmap(
				drawing.width.cast_signed(),
				drawing.height.cast_signed(),
				1,
				1,
				ptr::null(),
			);
			let info = ICONINFO {
				fIcon: 0, // a cursor, not an icon — which is what gives it a hotspot
				xHotspot: hotspot.0,
				yHotspot: hotspot.1,
				hbmMask: mask,
				hbmColor: color,
			};
			let cursor = CreateIconIndirect(&raw const info);
			// The bitmaps are copied into the cursor, so both are ours to drop right away.
			DeleteObject(mask);
			DeleteObject(color);
			cursor
		}
	}
}

/// Install the hands on the window (§51). A no-op off Windows, where the toolkit draws its own.
#[cfg(windows)]
pub fn install(hwnd: isize) {
	platform::install(hwnd);
}

/// The same, on a platform that needs no help.
#[cfg(not(windows))]
pub fn install(_hwnd: isize) {}

/// Put the wanted hand on screen at once, so a press that never moves the pointer still closes it.
#[cfg(windows)]
fn apply() {
	platform::apply();
}

/// The same, where nothing is painted by hand.
#[cfg(not(windows))]
fn apply() {}

#[cfg(test)]
mod tests {
	use super::*;

	// The atomics are process-wide and `cargo test` runs the suite on several threads at once, so a
	// state test takes the lock and starts from a known state. A poisoned lock is taken anyway: the
	// panic that poisoned it belongs to whichever test failed, and there is no invariant here for a
	// half-finished test to have broken — two integers, both of which this then overwrites.
	fn held() -> std::sync::MutexGuard<'static, ()> {
		let guard = TEST_LOCK
			.lock()
			.unwrap_or_else(std::sync::PoisonError::into_inner);
		forget();
		guard
	}

	/// An empty asset is not a broken one: it means that hand has not been drawn yet, and the window
	/// falls back to the move cursor (`grab_interaction`). The tests below check the drawings that
	/// ARE there, so they guard a redrawn hand without demanding one.
	fn bundled(bytes: &'static [u8]) -> Option<Drawing> {
		if bytes.is_empty() {
			return None;
		}
		Some(
			decode_png(bytes).expect("a bundled cursor that is not empty has to be a readable PNG"),
		)
	}

	#[test]
	fn the_two_hands_are_one_square_size() {
		let (Some(open), Some(closed)) = (bundled(GRAB_PNG), bundled(GRABBING_PNG)) else {
			return;
		};
		// Square, because Windows asks for a square cursor and anything else would be squashed into
		// one; and the SAME square, because the press swaps one for the other and a hand that
		// changed size on the way down would read as two pictures rather than one closing (§51).
		assert_eq!(open.width, open.height, "the open hand is square");
		assert_eq!(
			(open.width, open.height),
			(closed.width, closed.height),
			"both hands are drawn at one size"
		);
		assert_eq!(
			open.bgra.len(),
			(open.width * open.height * 4) as usize,
			"four bytes a pixel"
		);
	}

	#[test]
	fn the_hotspot_falls_on_something_drawn_in_both_hands() {
		// Not a detail: the hotspot is where the click lands, so one outside the shape means
		// pressing the chip under the drawn hand misses it — and the two must agree, or the pointer
		// appears to jump the moment the hand closes. This is the test a redrawn hand trips.
		for (name, bytes) in [("grab", GRAB_PNG), ("grabbing", GRABBING_PNG)] {
			let Some(drawing) = bundled(bytes) else {
				continue;
			};
			let at = ((HOTSPOT.1 * drawing.width + HOTSPOT.0) * 4) as usize;
			assert!(
				drawing.bgra[at + 3] > 0,
				"assets/cursor-{name}.png: the hotspot {HOTSPOT:?} is on a transparent pixel — move 				 the hotspot, or draw over it"
			);
		}
	}

	#[test]
	fn fitting_a_hand_to_the_size_windows_wants_halves_it_cleanly() {
		let Some(open) = bundled(GRAB_PNG) else {
			return;
		};
		// What actually happens on an ordinary display: the hands are drawn at 64 and Windows asks
		// for 32 (`SM_CXCURSOR`), which this halves exactly (§51).
		let half = resampled(&open, open.width / 2, open.height / 2);
		assert_eq!((half.width, half.height), (open.width / 2, open.height / 2));
		assert_eq!(half.bgra.len(), (half.width * half.height * 4) as usize);
		// The shape survives: a hand that came out blank or came out solid would both pass a size
		// check and fail on screen.
		let covered = half.bgra.chunks_exact(4).filter(|p| p[3] > 128).count();
		assert!(covered > 100, "there is still a hand in there");
		assert!(
			covered < (half.width * half.height) as usize,
			"and still something clear around it"
		);
	}

	#[test]
	fn resampling_weights_colour_by_alpha_so_a_soft_edge_keeps_its_colour() {
		// Two pixels: one opaque white, one fully transparent black — a soft edge, in miniature.
		// Averaged unweighted the result is grey, which is the dark halo this avoids; weighted by
		// alpha, the only colour that counts is the one that can be seen (§51).
		let drawing = Drawing {
			bgra: vec![0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00],
			width: 2,
			height: 1,
		};
		let squeezed = resampled(&drawing, 1, 1);
		assert_eq!(
			&squeezed.bgra[..3],
			&[0xff, 0xff, 0xff],
			"white, not grey — the transparent pixel brings no colour with it"
		);
		assert_eq!(squeezed.bgra[3], 0x7f, "and half the coverage it had");
	}

	#[test]
	fn a_drawing_arrives_as_bgra_with_its_alpha_intact() {
		let Some(drawing) = bundled(GRAB_PNG) else {
			return;
		};
		// Something is drawn and something is not: a hand that filled its whole square would have no
		// shape, and one that filled none of it would be invisible.
		let opaque = drawing.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
		assert!(opaque > 100, "the hand covers something");
		assert!(
			opaque < (drawing.width * drawing.height) as usize,
			"and leaves something clear around it"
		);
	}

	#[test]
	fn hovering_a_handle_opens_the_hand_and_dragging_closes_it() {
		let _held = held();
		assert_eq!(hand(), Hand::None, "nothing to hold");

		hover_entered(7);
		assert_eq!(hand(), Hand::Open);

		set_dragging(true);
		assert_eq!(
			hand(),
			Hand::Closed,
			"a drag outranks the hover it started from"
		);

		// The pointer wanders off the handle mid-drag: still closed, because the gesture is what the
		// hand is reporting, not what is under the pointer.
		hover_exited(7);
		assert_eq!(hand(), Hand::Closed);

		set_dragging(false);
		assert_eq!(hand(), Hand::None, "dropped, and off the handle");
	}

	#[test]
	fn two_handles_reporting_out_of_order_hand_the_claim_over_cleanly() {
		let _held = held();
		// Moving from one handle to the next: the one being ENTERED can be asked first, since iced
		// walks the widgets in layout order rather than in the order the pointer crossed them. The
		// exit that follows names the handle being LEFT, which no longer holds the claim, so it
		// takes nothing away (§52).
		hover_entered(1);
		hover_entered(2);
		hover_exited(1);
		assert_eq!(hand(), Hand::Open, "still over handle 2");

		hover_exited(2);
		assert_eq!(hand(), Hand::None);

		// An exit from a handle nothing is holding changes nothing either way.
		hover_exited(2);
		hover_entered(1);
		assert_eq!(hand(), Hand::Open);
	}

	#[test]
	fn leaving_a_region_of_handles_drops_the_claim_whoever_holds_it() {
		let _held = held();
		hover_entered(3);
		hover_reset();
		assert_eq!(hand(), Hand::None, "no handle can still have the pointer");
	}

	#[test]
	fn a_handle_that_stops_being_drawn_lets_go_of_the_hand() {
		let _held = held();
		hover_entered(9);

		// A frame it is still in: it says so, and keeps the hand.
		frame_begin();
		drawn(9);
		frame_end();
		assert_eq!(hand(), Hand::Open, "still on screen, still holding it");

		// A frame it is NOT in — the dialog closed under the pointer, the chip was sent to another
		// region. iced publishes no `on_exit` for a widget that has left the tree, so this is the
		// only way the hand hears about it (§52).
		frame_begin();
		drawn(4);
		frame_end();
		assert_eq!(hand(), Hand::None, "gone, so it cannot still be held");
	}

	#[test]
	fn a_control_on_a_handle_takes_the_cursor_off_it() {
		let _held = held();
		hover_entered(5);
		assert_eq!(hand(), Hand::Open);

		// Onto the chip's "×". iced updates a child before its parent, so this arrives while the
		// handle still holds the claim — and it wins: a press here closes the tab, it does not pick
		// it up (§52).
		control_entered(5);
		assert_eq!(hand(), Hand::None);

		// Back onto the chip around it: the handle never stopped holding the claim, so the hand
		// comes straight back with no fresh enter to help it.
		control_exited(5);
		assert_eq!(hand(), Hand::Open);

		// And a drag outranks both — the pointer crossing a close button mid-gesture must not open
		// the hand or drop the closed one.
		control_entered(5);
		set_dragging(true);
		assert_eq!(hand(), Hand::Closed);
		set_dragging(false);
		assert_eq!(hand(), Hand::None);
	}

	#[test]
	fn a_control_entered_before_its_handle_still_wins() {
		let _held = held();
		// The pointer arrives on a chip directly over its "×" — the child publishes first, then the
		// chip's own enter. Folding the block into the handle's claim would let that second message
		// clear a block that is still true, which is why they are two claims (§52).
		control_entered(6);
		hover_entered(6);
		assert_eq!(hand(), Hand::None);
	}

	#[test]
	fn a_control_that_closes_its_own_tab_stops_blocking_the_hand() {
		let _held = held();
		hover_entered(8);
		control_entered(8);
		assert_eq!(hand(), Hand::None);

		// The "×" was pressed: the tab, the chip and the button all go, so neither the handle nor
		// the control is there to say it lost the pointer. The frame says it for both (§52).
		frame_begin();
		frame_end();
		assert_eq!(hand(), Hand::None);

		// Another chip elsewhere now takes the pointer: the block from the button that no longer
		// exists must not still be suppressing the hand.
		hover_entered(9);
		frame_begin();
		drawn(9);
		frame_end();
		assert_eq!(hand(), Hand::Open);
	}

	#[test]
	fn a_modal_backdrop_takes_the_hand_off_what_it_covers_but_not_off_its_own_header() {
		let _held = held();
		// A chip behind a modal is still a live widget reporting the pointer, but it cannot be
		// picked up while the scrim is over it (§52).
		hover_entered(2);
		covered();
		assert_eq!(hand(), Hand::None, "nothing under a modal is grabbable");

		// The card's own header is drawn ON the backdrop, and dragging it is what it is for.
		hover_entered(HEADER);
		covered();
		assert_eq!(hand(), Hand::Open);
	}
}

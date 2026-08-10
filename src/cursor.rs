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
// Chromium its own resources — so cmote does the same, except that the two hands are DRAWN HERE,
// as text, rather than bundled as binary assets. That keeps the repository free of a third-party
// cursor file and its licence (cmote is MIT; Firefox's cursors are MPL-2.0), keeps the art
// reviewable in a diff, and costs about a hundred bytes of `const` per hand.
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
//     `Message::GrabEntered` / `GrabExited` by whichever widget wears the hand;
//   * `hover_reset` — the pointer leaving a whole region of handles (the tab strip does this when
//     it is left), which heals a count left standing by a handle that was closed or re-laid out
//     under the pointer;
//   * `set_dragging` — a press picked something up, and the drop or the cancel put it down: a chip
//     grab and its drop (§38), a dialog header grab and its release (§10).
// From those three it decides which hand, if any, the window should be wearing. It deliberately
// does NOT know which handle is which — the cursor question is "is the pointer on something
// grabbable, and is it held", and a count plus a flag answers it for every surface at once.

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

/// Which hand the window should be wearing, if either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
	/// Nothing to hold: leave the cursor to iced.
	None,
	/// Over something draggable — the open hand, CSS `grab`.
	Open,
	/// Holding it — the closed hand, CSS `grabbing`.
	Closed,
}

/// How many handles currently have the pointer, rather than *which* one.
///
/// A count, not a flag, because two handles report the same mouse move: leaving one and entering
/// the next both fire, and iced dispatches them in the widgets' layout order rather than in the
/// order the pointer crossed them. Moving right-to-left along the strip, the chip being ENTERED is
/// asked first, so a flag would be set and then immediately cleared by the chip being left, and the
/// hand would vanish exactly when the pointer arrived. A count cannot be put out of order by that —
/// and it is also what lets two DIFFERENT kinds of handle overlap (a dialog header over a strip)
/// without either having to know about the other.
static HOVERS: AtomicI32 = AtomicI32::new(0);

/// Whether something is being dragged right now, which outranks hovering: the hand stays closed
/// while the button is down wherever the pointer has got to, which is what says the gesture is
/// still in flight. A dialog dragged clean off its header keeps the closed hand for the same reason
/// a chip dragged onto the terminal does — the thing is still held.
static DRAGGING: AtomicBool = AtomicBool::new(false);

/// A handle took the pointer.
pub fn hover_entered() {
	HOVERS.fetch_add(1, Ordering::Relaxed);
	apply();
}

/// A handle lost the pointer. Clamped at zero: an unmatched exit — a chip closed under the pointer,
/// a dialog that closed while its header was hovered — must not push the count negative, or the
/// next real hover would have to climb back out of the hole before a hand appeared.
pub fn hover_exited() {
	let _ = HOVERS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |hovers| {
		Some((hovers - 1).max(0))
	});
	apply();
}

/// The pointer left a whole region of handles — the tab strip, say — so none of them can still
/// have it, whatever the count says. This is the one place the count is asserted rather than
/// adjusted, and it is why a missed exit heals by itself.
pub fn hover_reset() {
	HOVERS.store(0, Ordering::Relaxed);
	apply();
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
	HOVERS.store(0, Ordering::Relaxed);
	DRAGGING.store(false, Ordering::Relaxed);
}

/// Which hand the window should be wearing now.
pub fn hand() -> Hand {
	if DRAGGING.load(Ordering::Relaxed) {
		Hand::Closed
	} else if HOVERS.load(Ordering::Relaxed) > 0 {
		Hand::Open
	} else {
		Hand::None
	}
}

/// What a grab handle should ask ICED for, which is deliberately nothing on Windows.
///
/// `None` here means "do not call `mouse_area::interaction` at all". iced only tells winit to
/// change the cursor when the requested interaction CHANGES — so leaving it alone over the handle
/// means winit never sets a cursor there, and the hand this module paints is never stomped
/// mid-gesture. Asking for `Grab` instead would set `IDC_SIZEALL` on every hover and every press,
/// i.e. at exactly the two moments the hand is supposed to change.
///
/// Off Windows the answer is the real thing: those platforms have hand cursors and draw them.
#[cfg(windows)]
pub fn grab_interaction(_dragging: bool) -> Option<iced::mouse::Interaction> {
	None
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

/// Both cursors are square and this wide, which is what Windows asks for (`SM_CXCURSOR` is 32 on
/// every normal DPI setting) and what a `.cur` file would carry.
const SIZE: usize = 32;

/// One cursor, a row of characters per row of pixels: `#` outline, `.` fill, everything else
/// transparent. Written this way so the shapes can be read — and corrected — in a diff.
type Art = [&'static str; SIZE];

/// The open hand, CSS `grab`: four fingers up with staggered tips, a thumb out to the left, and a
/// palm below them. Black outline round white fill, the same two-tone the browsers use, so it
/// stays legible on a dark strip and on a light one.
#[rustfmt::skip]
const OPEN: Art = [
	"                                ",
	"                                ",
	"            ##                  ",
	"           #..###               ",
	"         ###..#..#              ",
	"        #..#..#..#              ",
	"        #..#..#..###            ",
	"        #..#..#..#..#           ",
	"        #..#..#..#..#           ",
	"        #..#..#..#..#           ",
	"     ## #..#..#..#..#           ",
	"    #..##..#..#..#..#           ",
	"    #...#..#..#..#..#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"     #.............#            ",
	"      #...........#             ",
	"       ###########              ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
];

/// The closed hand, CSS `grabbing`: the same hand with the fingers folded — four knuckles across
/// the top, a thumb bump on the left, and a shorter, rounder body. Deliberately the same width and
/// centred on the same column as the open hand, so the swap on press reads as one hand closing
/// rather than as two different pictures.
#[rustfmt::skip]
const CLOSED: Art = [
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"         ## ## ## ##            ",
	"        #..#..#..#..#           ",
	"       #............#           ",
	"      #.............#           ",
	"      #.............#           ",
	"     #..............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"    #...............#           ",
	"     #..............#           ",
	"     #..............#           ",
	"      #.............#           ",
	"      #.............#           ",
	"      #.............#           ",
	"       #...........#            ",
	"        #.........#             ",
	"         #########              ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
	"                                ",
];

/// Where the click lands, in pixels from the art's top-left.
///
/// ONE hotspot for both shapes, and inside the part they share: press and the hand closes without
/// the pointer appearing to jump. A hand cursor is aimed with its middle rather than with a tip —
/// there is no tip to aim with — which is how the browsers place theirs too.
const HOTSPOT: (u32, u32) = (12, 12);

/// Turn one art into 32-bit BGRA pixels, top row first — the order a Windows top-down DIB wants,
/// and a plain enough format that the conversion can be tested without a window.
///
/// Two colours and one hole: `#` opaque black, `.` opaque white, anything else fully transparent.
/// Alpha is 0 or 255 and never in between, so there is no premultiplication to get wrong; the
/// shapes carry their own outline instead of relying on antialiasing to separate them from
/// whatever is behind.
fn pixels(art: &Art) -> Vec<u8> {
	let mut out = Vec::with_capacity(SIZE * SIZE * 4);
	for row in art {
		let mut drawn = 0;
		for character in row.chars().take(SIZE) {
			// B, G, R, A — the byte order of a little-endian 0xAARRGGBB pixel.
			out.extend_from_slice(match character {
				'#' => &[0x00, 0x00, 0x00, 0xff],
				'.' => &[0xff, 0xff, 0xff, 0xff],
				_ => &[0x00, 0x00, 0x00, 0x00],
			});
			drawn += 1;
		}
		// A row written short is padded rather than rejected: the art is edited by hand, and a
		// missing trailing space should cost a transparent pixel, not a panic at start-up.
		out.extend(std::iter::repeat_n(0x00, (SIZE - drawn) * 4));
	}
	out
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
	use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
	use windows_sys::Win32::UI::WindowsAndMessaging::{
		CreateIconIndirect, HTCLIENT, ICONINFO, SetCursor, WM_SETCURSOR,
	};

	use super::{CLOSED, HOTSPOT, Hand, OPEN, SIZE, hand, pixels};

	/// The two cursors, built once when the window is subclassed. Held as `isize` because a raw
	/// `HCURSOR` is a pointer and therefore not `Sync`; the handles are owned by the process for its
	/// whole life (they are never destroyed — there is exactly one of each, and they are wanted
	/// until the window closes), so passing them through an integer costs nothing but a cast.
	static HANDS: OnceLock<(isize, isize)> = OnceLock::new();

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
		// SAFETY: the handles are built from our own const art and are never freed; `hwnd` came
		// from iced's own window and is used on the thread that owns it (the boot task runs on the
		// UI thread, which is the one that pumps this window's messages).
		unsafe {
			let open = cursor_from(&pixels(&OPEN));
			let closed = cursor_from(&pixels(&CLOSED));
			if open.is_null() || closed.is_null() {
				return;
			}
			let _ = HANDS.set((open as isize, closed as isize));
			SetWindowSubclass(hwnd as HWND, Some(subclass), SUBCLASS_ID, 0);
		}
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
		if message == WM_SETCURSOR
			&& (lparam as u32 & 0xffff) == HTCLIENT
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

	/// Put the wanted hand on screen right now, without waiting for a mouse move.
	///
	/// This is what makes a press that does not move the pointer close the hand: `WM_SETCURSOR`
	/// arrives with mouse movement, so between pressing and moving there would otherwise be no
	/// message to answer. Harmless when no hand is wanted — Windows re-asks on the next move
	/// regardless — and safe to call from `update`, which runs on the window's own thread.
	pub fn apply() {
		if let Some(cursor) = wanted() {
			// SAFETY: a handle this module created and never freed.
			unsafe { SetCursor(cursor as *mut c_void) };
		}
	}

	/// Build one cursor from BGRA pixels: a 32-bit top-down DIB for the colour, an empty 1-bit
	/// bitmap for the mask, and `CreateIconIndirect` with `fIcon = FALSE` to make it a CURSOR (an
	/// icon with a hotspot) rather than an icon.
	///
	/// The mask is all zeroes on purpose. A monochrome cursor uses it to decide which pixels show
	/// and which invert the screen; a 32-bit one carries its own alpha, and Windows uses that when
	/// the mask says "draw everything".
	unsafe fn cursor_from(bgra: &[u8]) -> *mut c_void {
		let mut header: BITMAPV5HEADER = unsafe { std::mem::zeroed() };
		header.bV5Size = size_of::<BITMAPV5HEADER>() as u32;
		header.bV5Width = SIZE as i32;
		// Negative height means top-down: the first row of bytes is the top row of pixels, which is
		// how the art reads. A positive height would draw every hand upside down.
		header.bV5Height = -(SIZE as i32);
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
				&mut bits,
				ptr::null_mut(),
				0,
			);
			ReleaseDC(ptr::null_mut(), dc);
			if color.is_null() || bits.is_null() {
				return ptr::null_mut();
			}
			ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), bgra.len());

			let mask = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, ptr::null());
			let info = ICONINFO {
				fIcon: 0, // a cursor, not an icon — which is what gives it a hotspot
				xHotspot: HOTSPOT.0,
				yHotspot: HOTSPOT.1,
				hbmMask: mask,
				hbmColor: color,
			};
			let cursor = CreateIconIndirect(&info);
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
			.unwrap_or_else(|poisoned| poisoned.into_inner());
		forget();
		guard
	}

	#[test]
	fn both_hands_are_square_and_use_only_the_three_symbols() {
		for art in [&OPEN, &CLOSED] {
			assert_eq!(art.len(), SIZE);
			for row in art {
				assert_eq!(
					row.chars().count(),
					SIZE,
					"every row is one pixel wide per column"
				);
				assert!(
					row.chars()
						.all(|character| matches!(character, '#' | '.' | ' ')),
					"outline, fill and hole are the whole alphabet"
				);
			}
		}
	}

	#[test]
	fn the_hotspot_falls_inside_both_hands() {
		// Not a detail: the hotspot is where the click lands, so a hotspot outside the shape means
		// pressing the chip under the drawn hand misses it — and the two must agree, or the
		// pointer appears to jump the moment the hand closes.
		for art in [&OPEN, &CLOSED] {
			let row = art[HOTSPOT.1 as usize];
			let at = row.chars().nth(HOTSPOT.0 as usize).expect("inside the art");
			assert!(at == '.' || at == '#', "the hotspot is on the hand itself");
		}
	}

	#[test]
	fn the_art_becomes_one_opaque_or_clear_pixel_per_character() {
		let bgra = pixels(&OPEN);
		assert_eq!(bgra.len(), SIZE * SIZE * 4);
		// The first row is all hole, so the alpha of every pixel in it is zero.
		assert!(bgra[..SIZE * 4].iter().all(|byte| *byte == 0));
		// Every pixel is either fully transparent or fully opaque: no half-alpha to premultiply.
		assert!(
			bgra.chunks_exact(4)
				.all(|pixel| pixel[3] == 0x00 || pixel[3] == 0xff)
		);
	}

	#[test]
	fn hovering_a_handle_opens_the_hand_and_dragging_closes_it() {
		let _held = held();
		assert_eq!(hand(), Hand::None, "nothing to hold");

		hover_entered();
		assert_eq!(hand(), Hand::Open);

		set_dragging(true);
		assert_eq!(
			hand(),
			Hand::Closed,
			"a drag outranks the hover it started from"
		);

		// The pointer wanders off the handle mid-drag: still closed, because the gesture is what the
		// hand is reporting, not what is under the pointer.
		hover_exited();
		assert_eq!(hand(), Hand::Closed);

		set_dragging(false);
		assert_eq!(hand(), Hand::None, "dropped, and off the handle");
	}

	#[test]
	fn the_hover_count_survives_two_handles_reporting_out_of_order() {
		let _held = held();
		// Moving from one handle to the next: the one being ENTERED can be asked first, since iced
		// walks the widgets in layout order rather than in the order the pointer crossed them. A
		// flag would end up cleared; the count ends up at one, which is the truth.
		hover_entered();
		hover_entered();
		hover_exited();
		assert_eq!(hand(), Hand::Open, "still over a handle");

		hover_exited();
		assert_eq!(hand(), Hand::None);

		// An exit with nothing to leave cannot push the count below zero, or the next real hover
		// would have to climb back out of the hole before a hand appeared.
		hover_exited();
		hover_entered();
		assert_eq!(hand(), Hand::Open);
	}

	#[test]
	fn leaving_a_region_of_handles_clears_a_count_a_closed_one_left_behind() {
		let _held = held();
		hover_entered();
		hover_entered();
		hover_reset();
		assert_eq!(hand(), Hand::None, "no handle can still have the pointer");
	}
}

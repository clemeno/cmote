// preview.rs — the picture a tab shows instead of a text buffer (PLAN §53).
//
// The pure half of the image preview: which files open as a picture rather than as text, and the
// decode — format sniffing, the caps that stand between a remote's bytes and this process's
// memory, and the refusal wording when a file is not something cmote can draw. The network read is
// `ssh/edit.rs`'s (a preview and an editor pull a remote file the same way, so they share it) and
// the drawing is `ui/preview.rs`'s, which is the same three-way split the editor and the panels use
// (§18, §19, §32) — so every rule here is testable with no server and no window.
//
// THE SECURITY SHAPE, since this module is the one that runs a parser over bytes a server chose.
// §41 refused kitty graphics and iTerm2's OSC 1337 precisely because they hand a PNG/JPEG decoder
// bytes a remote PUSHED into the terminal stream unasked. That refusal still stands. This is the
// other case — the user pointed at one file and asked for it — and it is fenced three ways: the
// decoder is picked by MAGIC BYTES so a remote-controlled file name cannot steer which parser
// runs; `Limits` caps the dimensions and the allocation so a header declaring 30000×30000 is
// refused before a buffer is reserved for it; and the read itself stops at `MAX_SIZE` bytes.

use iced::advanced::image::Handle;

/// The largest file the preview will pull off the server (§53). A picture is held whole in memory
/// twice over on the way in — the file's own bytes, then the decoded RGBA — so this is the ceiling
/// on the first of those, and `MAX_ALLOC` below is the ceiling on the second. 32 MiB is far past
/// any screenshot or camera JPEG and well short of trouble; it is deliberately larger than the
/// editor's 8 MiB, because 8 MiB is generous for text and mean for photographs.
pub const MAX_SIZE: u64 = 32 * 1024 * 1024;

/// The largest picture, per side, the decoder is allowed to produce (§53). Two reasons for one
/// number: it bounds what a crafted header can make this process allocate, and it keeps the result
/// inside the maximum texture a GPU will accept — 8192 is the smallest limit still found on
/// hardware cmote runs on, so a picture that decodes here is a picture that can actually be drawn.
const MAX_SIDE: u32 = 8192;

/// The most the decoder may allocate for one picture (§53). The dimension caps alone are not
/// enough: a format can ask for a big intermediate buffer without either side being over the line,
/// so this is the backstop. 128 MiB is ~32 megapixels of RGBA — every real photograph, no room for
/// a decompression bomb to sit in.
const MAX_ALLOC: u64 = 128 * 1024 * 1024;

/// Whether this path opens as a PICTURE rather than in the text editor (§53). Decided by the
/// extension, because it decides which tab to open and that has to happen before a byte is read.
///
/// It is `files`' own image table minus SVG, which is the one entry that is a picture by icon and
/// text by nature: an SVG is XML, the editor can genuinely edit it, and a preview could only refuse
/// it — drawing one is a layout engine, not a decoder. Everything else in that table opens here
/// even when cmote has no decoder for it (TIFF, HEIC), because "cmote cannot preview TIFF" is a
/// better answer than the editor's truthful but unhelpful "not text in a supported encoding".
pub fn opens_preview(path: &str) -> bool {
	match crate::editor::extension_key(path).as_str() {
		"svg" => false,
		extension => crate::files::IMAGE.contains(&extension),
	}
}

/// A decoded picture, before it becomes something the renderer can hold (§53). Pure data — width,
/// height, the format it turned out to be, and the pixels — so the whole decode, caps and refusals
/// included, is unit-testable without a GPU or a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
	pub width: u32,
	pub height: u32,
	/// What the bytes turned out to be, for the toolbar line. Read off the magic bytes, so it is
	/// what the file IS, not what it was named.
	pub format: &'static str,
	/// Straight RGBA8, four bytes a pixel — what `Handle::from_rgba` wants.
	pub rgba: Vec<u8>,
}

/// The caps every decode runs under (§53). Built here rather than inlined so a test can hand
/// `decode_within` a tighter set and drive the refusal path without forging a bomb.
fn limits() -> image::Limits {
	let mut limits = image::Limits::default();
	limits.max_image_width = Some(MAX_SIDE);
	limits.max_image_height = Some(MAX_SIDE);
	limits.max_alloc = Some(MAX_ALLOC);
	limits
}

/// Turn a file's bytes into a picture, or say why not (§53). The reason is written to be shown to
/// the user in place of the image, so it names the format when it can.
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
	decode_within(bytes, limits())
}

/// `decode` with the caps passed in, so the refusal path is reachable from a test with a tiny
/// picture instead of a real one (§53).
fn decode_within(bytes: &[u8], limits: image::Limits) -> Result<Decoded, String> {
	// The format comes from the LEADING BYTES, never from the file name. The name is a remote's to
	// choose, and letting it pick which parser runs would be handing an attacker the one decision
	// that matters here. It also means a mislabelled `.jpg` that is really a PNG simply opens.
	let format = image::guess_format(bytes)
		.map_err(|_| "This file is not a picture in a format cmote recognises.".to_owned())?;
	let name = format_name(format).ok_or_else(|| {
		format!(
			"cmote does not preview {} — it previews PNG, JPEG, GIF, BMP and WebP.",
			unsupported_name(format)
		)
	})?;

	// `with_format` rather than `with_guessed_format`: the format is already decided above, and
	// pinning it means the reader cannot be talked into a second opinion by the payload.
	let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
	reader.limits(limits);
	let decoded = reader
		.decode()
		.map_err(|error| format!("This picture could not be decoded: {error}."))?;

	// One conversion to RGBA8 whatever came out — a palette GIF, a grey JPEG and a 16-bit PNG all
	// end up as the four-bytes-a-pixel buffer the renderer uploads, so nothing downstream branches
	// on colour type.
	let rgba = decoded.to_rgba8();
	Ok(Decoded {
		width: rgba.width(),
		height: rgba.height(),
		format: name,
		rgba: rgba.into_raw(),
	})
}

/// The display name of a format cmote can actually decode, or `None` for one it only RECOGNISES
/// (§53). The distinction is the point: `image` sniffs every format it knows of, but only the five
/// codecs enabled in `Cargo.toml` are compiled in, and a decoder that is not compiled in is a
/// parser that cannot be reached. Matching here rather than letting the decode fail turns an
/// internal "unsupported" error into a sentence that names what the file is.
fn format_name(format: image::ImageFormat) -> Option<&'static str> {
	match format {
		image::ImageFormat::Png => Some("PNG"),
		image::ImageFormat::Jpeg => Some("JPEG"),
		image::ImageFormat::Gif => Some("GIF"),
		image::ImageFormat::Bmp => Some("BMP"),
		image::ImageFormat::WebP => Some("WebP"),
		_ => None,
	}
}

/// What to call a format cmote recognises but cannot draw (§53), for the refusal above.
/// `image`'s own extension is the shortest honest label; upper-cased, it reads as the format name.
fn unsupported_name(format: image::ImageFormat) -> String {
	format
		.extensions_str()
		.first()
		.map_or_else(|| "this format".to_owned(), |ext| ext.to_uppercase())
}

/// Where a preview tab is in its lifecycle (§53) — the same three states the editor has, for the
/// same reason: the view shows a spinner, a picture, or a sentence, and never a half of one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
	Loading,
	Ready,
	Failed(String),
}

/// A decoded picture as the renderer holds it (§53). The `Handle` is an iced type in a model
/// module, which is the same trade `term::graphics` makes for the inline sixel images (§41): the
/// alternative is keeping the pixels and rebuilding a handle every frame, which would re-upload the
/// texture on each paint. `Handle` is a cheap cloneable reference to the pixel data, so this is the
/// one place `preview` looks up at the renderer.
#[derive(Debug, Clone)]
pub struct Picture {
	pub width: u32,
	pub height: u32,
	pub format: &'static str,
	/// The size of the FILE on the server, not of the decoded pixels — it is what the user would see
	/// in the files pane, so it is what the toolbar repeats back.
	pub bytes: u64,
	pub handle: Handle,
}

/// One open preview — the state a `Tab` carries when it is showing a picture rather than running a
/// session (§53). Like the editor (§32) it has no connection of its own: `session` is the id of the
/// tab it was opened from, whose channel carried the read.
///
/// It is a much smaller thing than the editor, and deliberately so: there is nothing to save, so
/// there is no dirty flag, no close confirmation, no encoding to preserve and no account to fix for
/// a later write. A preview is read-only by nature, and that is most of what makes it simple.
#[derive(Debug)]
pub struct Preview {
	/// The parent session tab this picture was opened from — the channel its read rode (§53).
	pub session: u64,
	/// The remote path being shown. Never re-pointed: there is no Save As on a preview.
	pub path: String,
	/// Loading / Ready / Failed (§53).
	pub status: Status,
	/// The decoded picture, `Some` exactly while `status` is `Ready`.
	pub picture: Option<Picture>,
}

impl Preview {
	/// A fresh preview waiting on its bytes (§53), parented to `session`.
	pub fn loading(session: u64, path: String) -> Self {
		Self {
			session,
			path,
			status: Status::Loading,
			picture: None,
		}
	}

	/// Hand over a decoded picture and the size of the file it came from (§53). This is where the
	/// pixels become a renderer handle — once, on arrival, rather than on every frame.
	pub fn set_loaded(&mut self, decoded: Decoded, bytes: u64) {
		self.picture = Some(Picture {
			width: decoded.width,
			height: decoded.height,
			format: decoded.format,
			bytes,
			handle: Handle::from_rgba(decoded.width, decoded.height, decoded.rgba),
		});
		self.status = Status::Ready;
	}

	/// The read failed, or the bytes were not a picture cmote can draw (§53): show the reason in
	/// place of the image. Any picture already on screen is dropped with it, so the tab never shows
	/// a stale image under a fresh error.
	pub fn load_failed(&mut self, reason: String) {
		self.picture = None;
		self.status = Status::Failed(reason);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A real picture of the given size, encoded in `format` — so the decode tests run over bytes a
	/// decoder actually produced rather than a hand-forged header.
	fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
		let picture = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
		let mut bytes = Vec::new();
		image::DynamicImage::ImageRgba8(picture)
			.write_to(&mut std::io::Cursor::new(&mut bytes), format)
			.expect("the test's own encoder writes");
		bytes
	}

	#[test]
	fn a_png_decodes_to_its_own_size_and_says_what_it_was() {
		let decoded = decode(&encoded(3, 2, image::ImageFormat::Png)).expect("a PNG opens");
		assert_eq!((decoded.width, decoded.height), (3, 2));
		assert_eq!(decoded.format, "PNG");
		// Four bytes a pixel, whatever the file's own colour type was.
		assert_eq!(decoded.rgba.len(), 3 * 2 * 4);
	}

	#[test]
	fn the_format_is_read_off_the_bytes_not_the_file_name() {
		// `decode` takes no name at all, which is the point: these bytes would open identically
		// whether the server called them `.jpg`, `.txt` or nothing.
		let decoded = decode(&encoded(4, 4, image::ImageFormat::Jpeg)).expect("a JPEG opens");
		assert_eq!(decoded.format, "JPEG");
		assert_eq!((decoded.width, decoded.height), (4, 4));
	}

	/// `ponytail:` WebP is absent from this list because `image`'s WebP support is decode-only, so
	/// the test cannot build its own fixture the way it does for the other four. It is covered by
	/// the format table and by hand (README's walkthrough), not here.
	#[test]
	fn every_enabled_format_opens() {
		for (format, name) in [
			(image::ImageFormat::Png, "PNG"),
			(image::ImageFormat::Jpeg, "JPEG"),
			(image::ImageFormat::Gif, "GIF"),
			(image::ImageFormat::Bmp, "BMP"),
		] {
			let decoded = decode(&encoded(2, 2, format)).expect("an enabled format opens");
			assert_eq!(decoded.format, name, "{name} names itself");
		}
	}

	#[test]
	fn a_format_cmote_has_no_decoder_for_is_named_in_the_refusal() {
		// A TIFF magic number and nothing else: `image` recognises the format, but its codec is not
		// compiled in, so the refusal must say so rather than fall through to a decode error.
		let reason = decode(b"II*\0\0\0\0\0junk").expect_err("TIFF is refused");
		assert!(reason.contains("TIFF"), "names the format: {reason}");
		assert!(reason.contains("PNG"), "and says what it can do: {reason}");
	}

	#[test]
	fn bytes_that_are_not_a_picture_at_all_are_refused() {
		let reason = decode(b"#!/bin/sh\necho hello\n").expect_err("a script is not a picture");
		assert!(reason.contains("not a picture"), "{reason}");
	}

	#[test]
	fn an_empty_file_is_refused_rather_than_drawn_as_nothing() {
		assert!(decode(&[]).is_err());
	}

	#[test]
	fn a_picture_past_the_cap_is_refused_before_it_is_allocated() {
		// The real caps are far too big to reach with a test picture, so the tight ones stand in:
		// what is under test is that the caps are applied and their failure becomes our sentence.
		let mut tight = limits();
		tight.max_image_width = Some(2);
		let reason = decode_within(&encoded(3, 2, image::ImageFormat::Png), tight)
			.expect_err("a picture wider than the cap is refused");
		assert!(reason.contains("could not be decoded"), "{reason}");
	}

	#[test]
	fn the_shipped_caps_are_the_documented_ones() {
		let limits = limits();
		assert_eq!(limits.max_image_width, Some(MAX_SIDE));
		assert_eq!(limits.max_image_height, Some(MAX_SIDE));
		assert_eq!(limits.max_alloc, Some(MAX_ALLOC));
	}

	#[test]
	fn pictures_open_as_pictures_and_everything_else_does_not() {
		for path in ["/srv/a.png", "/srv/A.JPG", "photo.jpeg", "/x/y/scan.tif"] {
			assert!(opens_preview(path), "{path} is a picture");
		}
		for path in ["/etc/hosts", "/srv/main.rs", "notes.md", "/srv/archive.zip"] {
			assert!(!opens_preview(path), "{path} is not");
		}
	}

	#[test]
	fn an_svg_belongs_to_the_editor_because_it_is_text() {
		assert!(!opens_preview("/srv/logo.svg"));
	}

	#[test]
	fn a_dot_file_named_like_a_picture_is_not_one() {
		// `.png` is a hidden file whose whole name is `png`, not a file with a `png` extension —
		// the same reading the editor's theme key uses.
		assert!(!opens_preview("/home/user/.png"));
	}

	#[test]
	fn a_failure_after_a_picture_takes_the_picture_with_it() {
		let mut preview = Preview::loading(7, "/srv/a.png".to_owned());
		preview.set_loaded(
			Decoded {
				width: 1,
				height: 1,
				format: "PNG",
				rgba: vec![0, 0, 0, 255],
			},
			99,
		);
		assert_eq!(preview.status, Status::Ready);
		assert_eq!(preview.picture.as_ref().map(|p| p.bytes), Some(99));

		preview.load_failed("gone".to_owned());
		assert_eq!(preview.status, Status::Failed("gone".to_owned()));
		assert!(
			preview.picture.is_none(),
			"no stale picture under a fresh error"
		);
	}
}

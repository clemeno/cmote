// editor.rs — the in-tab text editor's model (PLAN §32).
//
// The pure half of the editor: the byte↔text encoding (BOM detection and the small UTF set we
// support), the changed-line diff that drives the gutter marks, and the `Editor` state a tab
// carries when it is editing a file rather than running a session. The network calls live in
// `ssh/edit.rs` and the drawing in `ui/editor.rs`, so everything here is testable with no server —
// the same three-way split the panels use (§18, §19).
//
// The encoding rule (the one the user set): keep a BOM if the file has one, let a BOM decide the
// UTF, assume UTF-8-without-BOM when there is none, refuse what cannot be decoded, and on save
// persist EXACTLY as opened — never convert behind the user's back.

use iced::widget::text_editor;

/// The largest edit band we will diff with an exact (quadratic) LCS before falling back to marking
/// the whole band changed (§32). A normal edit touches a handful of lines, so the band is tiny and
/// the LCS is cheap; this cap only guards a pathological "changed a thousand lines at once" so a
/// keystroke can never pay an O(n²) diff over the whole buffer.
const LCS_BAND_CAP: usize = 1000;

/// The BOMs we recognise. A leading byte-order mark decides the encoding; everything else is read
/// as UTF-8 without one (§32).
const BOM_UTF8: [u8; 3] = [0xEF, 0xBB, 0xBF];
const BOM_UTF16_LE: [u8; 2] = [0xFF, 0xFE];
const BOM_UTF16_BE: [u8; 2] = [0xFE, 0xFF];

/// The character set a file was opened as — the small, predictable set cmote decodes in-house
/// (§32). No statistical guessing, no Windows-1252 fallback: anything outside this set is the
/// "unsupported" case, refused rather than mangled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
	/// UTF-8, the default and the common case.
	Utf8,
	/// UTF-16 little-endian (the `FF FE` BOM).
	Utf16Le,
	/// UTF-16 big-endian (the `FE FF` BOM).
	Utf16Be,
}

/// How a file was opened, and therefore how it saves (§32): the charset plus whether it carried a
/// BOM we must re-emit. Held on the `Editor` from load to save so an edit round-trips the format
/// rather than silently converting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoding {
	pub charset: Charset,
	/// Whether the file began with a byte-order mark. Re-prepended on save iff set, so a
	/// BOM-less UTF-8 file stays BOM-less and a BOM'd one keeps its mark.
	pub bom: bool,
}

impl Encoding {
	/// The default for a file with no BOM: UTF-8, no mark (§32).
	pub const UTF8_NO_BOM: Self = Self {
		charset: Charset::Utf8,
		bom: false,
	};

	/// A short label for the toolbar ("UTF-8", "UTF-8 BOM", "UTF-16 LE").
	pub fn label(self) -> &'static str {
		match (self.charset, self.bom) {
			(Charset::Utf8, false) => "UTF-8",
			(Charset::Utf8, true) => "UTF-8 BOM",
			(Charset::Utf16Le, _) => "UTF-16 LE",
			(Charset::Utf16Be, _) => "UTF-16 BE",
		}
	}
}

/// Decode raw file bytes into editable text and the encoding to save it back with (§32), or `None`
/// when the bytes are not text in a supported encoding — a binary, a legacy charset, or a UTF-32
/// file. A `None` is the caller's cue to refuse the file rather than show mojibake.
///
/// Detection is BOM-first: a mark picks the UTF and is stripped from the text (it is metadata, not a
/// character); with no mark the bytes are read as UTF-8-without-BOM. A file whose bytes do not decode
/// cleanly under the chosen encoding is unsupported.
pub fn decode(bytes: &[u8]) -> Option<(String, Encoding)> {
	if bytes.starts_with(&BOM_UTF8) {
		let text = std::str::from_utf8(&bytes[BOM_UTF8.len()..]).ok()?;
		return Some((
			text.to_owned(),
			Encoding {
				charset: Charset::Utf8,
				bom: true,
			},
		));
	}
	if bytes.starts_with(&BOM_UTF16_LE) {
		// `FF FE 00 00` is a UTF-32 LE BOM, not UTF-16 — refuse it rather than read the trailing
		// nulls as an empty first character.
		if bytes.starts_with(&[0xFF, 0xFE, 0x00, 0x00]) {
			return None;
		}
		let text = decode_utf16(&bytes[BOM_UTF16_LE.len()..], true)?;
		return Some((
			text,
			Encoding {
				charset: Charset::Utf16Le,
				bom: true,
			},
		));
	}
	if bytes.starts_with(&BOM_UTF16_BE) {
		let text = decode_utf16(&bytes[BOM_UTF16_BE.len()..], false)?;
		return Some((
			text,
			Encoding {
				charset: Charset::Utf16Be,
				bom: true,
			},
		));
	}
	// No BOM: the default. Valid UTF-8 opens; anything else (including a UTF-32 BE BOM, whose
	// `00 00` bytes are not valid UTF-8) is unsupported.
	let text = std::str::from_utf8(bytes).ok()?;
	Some((text.to_owned(), Encoding::UTF8_NO_BOM))
}

/// Decode a UTF-16 byte run (BOM already stripped) in the given endianness, or `None` if it is an
/// odd length or contains an unpaired surrogate. `char::decode_utf16` does the surrogate pairing;
/// we only supply the 16-bit code units.
fn decode_utf16(bytes: &[u8], little_endian: bool) -> Option<String> {
	if !bytes.len().is_multiple_of(2) {
		return None;
	}
	let units = bytes.chunks_exact(2).map(|pair| {
		let two = [pair[0], pair[1]];
		if little_endian {
			u16::from_le_bytes(two)
		} else {
			u16::from_be_bytes(two)
		}
	});
	char::decode_utf16(units)
		.collect::<Result<String, _>>()
		.ok()
}

/// Encode editable text back to file bytes under the encoding it was opened as (§32), re-prepending
/// the BOM iff the file had one. The text already carries its line endings (iced's `Content::text`
/// reassembles them), so this is a straight charset transform, nothing more.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
	let mut out = Vec::with_capacity(text.len() + 3);
	match encoding.charset {
		Charset::Utf8 => {
			if encoding.bom {
				out.extend_from_slice(&BOM_UTF8);
			}
			out.extend_from_slice(text.as_bytes());
		}
		Charset::Utf16Le => {
			if encoding.bom {
				out.extend_from_slice(&BOM_UTF16_LE);
			}
			for unit in text.encode_utf16() {
				out.extend_from_slice(&unit.to_le_bytes());
			}
		}
		Charset::Utf16Be => {
			if encoding.bom {
				out.extend_from_slice(&BOM_UTF16_BE);
			}
			for unit in text.encode_utf16() {
				out.extend_from_slice(&unit.to_be_bytes());
			}
		}
	}
	out
}

/// One flag per CURRENT line: `true` where the line differs from what was loaded (§32) — the bars
/// the gutter draws. Computed by trimming the common prefix and suffix (which localises the change
/// to a band) then running an exact LCS inside that band, so an inserted or edited line marks only
/// itself. An over-`LCS_BAND_CAP` band skips the quadratic diff and marks the whole band, the cheap
/// safe fallback.
pub fn changed_flags(original: &[String], current: &[String]) -> Vec<bool> {
	let count = current.len();
	let mut flags = vec![false; count];

	// The common prefix: lines equal from the top in both.
	let mut lo = 0;
	while lo < count && lo < original.len() && current[lo] == original[lo] {
		lo += 1;
	}
	// The common suffix: lines equal from the bottom, not crossing the prefix.
	let mut current_end = count;
	let mut original_end = original.len();
	while current_end > lo
		&& original_end > lo
		&& current[current_end - 1] == original[original_end - 1]
	{
		current_end -= 1;
		original_end -= 1;
	}

	let current_band = &current[lo..current_end];
	let original_band = &original[lo..original_end];
	// Nothing left in the current band means the change was a pure deletion — no current line to
	// mark, so the gutter shows nothing (dirtiness is tracked separately).
	if current_band.is_empty() {
		return flags;
	}
	if current_band.len() > LCS_BAND_CAP || original_band.len() > LCS_BAND_CAP {
		for flag in &mut flags[lo..current_end] {
			*flag = true;
		}
		return flags;
	}

	// Exact within the band: a current line kept by the LCS is unchanged; the rest are marked.
	let kept = lcs_kept(original_band, current_band);
	for (offset, keep) in kept.iter().enumerate() {
		if !keep {
			flags[lo + offset] = true;
		}
	}
	flags
}

/// The longest-common-subsequence mask over `current`: `true` where a current line is part of an LCS
/// with `original` (so it is unchanged, only shifted). A textbook DP filled from the end, then a
/// forward walk that marks the matched current lines.
fn lcs_kept(original: &[String], current: &[String]) -> Vec<bool> {
	let rows = original.len();
	let cols = current.len();
	// table[i][j] = LCS length of original[i..] and current[j..].
	let mut table = vec![vec![0u32; cols + 1]; rows + 1];
	for i in (0..rows).rev() {
		for j in (0..cols).rev() {
			table[i][j] = if original[i] == current[j] {
				table[i + 1][j + 1] + 1
			} else {
				table[i + 1][j].max(table[i][j + 1])
			};
		}
	}

	let mut kept = vec![false; cols];
	let (mut i, mut j) = (0, 0);
	while i < rows && j < cols {
		if original[i] == current[j] {
			kept[j] = true;
			i += 1;
			j += 1;
		} else if table[i + 1][j] >= table[i][j + 1] {
			i += 1;
		} else {
			j += 1;
		}
	}
	kept
}

/// Where an editor tab is in its lifecycle (§32). `Loading` until the bytes arrive; `Ready` with a
/// live buffer; `Failed` when the file is too big, binary, or an unsupported encoding — the view
/// then shows the reason in place of the buffer, never mojibake.
#[derive(Debug, Clone)]
pub enum Status {
	Loading,
	Ready,
	Failed(String),
}

/// What the editor view can ask of its tab (§32). The tab applies the ones that only touch this
/// buffer (typing, the Save As prompt's own field); `Save` and `SaveAsConfirm`, which have to reach
/// the parent session's channel, are turned by the tab into an App-level flush.
#[derive(Debug, Clone)]
pub enum EditorMessage {
	/// An action from the text widget — an edit, a selection, a scroll (§32).
	Action(text_editor::Action),
	/// Save the buffer to its current path (the Save button, or Ctrl+S).
	Save,
	/// Open the Save As prompt (the Save As button, or Ctrl+Shift+S).
	SaveAsStart,
	/// The Save As path field changed.
	SaveAsChanged(String),
	/// Confirm Save As — save to the typed path and edit it from now on.
	SaveAsConfirm,
	/// Close the Save As prompt without saving.
	SaveAsCancel,
}

/// One open editor — the state a `Tab` carries when it is editing rather than running a session
/// (§32). It has no connection of its own: `session` is the id of the tab it was opened from, and
/// `App` sends every load and save on THAT tab's channel, routing the reply back here by the
/// editor's own id.
#[derive(Debug)]
pub struct Editor {
	/// The parent session tab this file was opened from — the channel its SFTP load/save ride (§32).
	pub session: u64,
	/// The remote path being edited. Save As re-points this at a new path.
	pub path: String,
	/// How the file was opened, and how it saves back (§32).
	pub encoding: Encoding,
	/// The editable buffer. iced owns the text, the cursor and the line endings.
	pub content: text_editor::Content,
	/// The lines as they were at load or the last successful save — the baseline the changed-line
	/// diff and the dirty flag compare against.
	original: Vec<String>,
	/// Loading / Ready / Failed (§32).
	pub status: Status,
	/// A save is in flight: the toolbar disables Save so a second cannot race the first.
	pub saving: bool,
	/// A transient message shown in the toolbar (a save failure). Distinct from `Status::Failed`,
	/// which replaces the whole buffer — a save error must not throw away the edits it failed to
	/// persist, so it rides here and leaves the buffer editable.
	pub notice: Option<String>,
	/// The in-progress Save As path while its prompt is open, or `None` when it is closed (§32).
	pub save_as: Option<String>,
	/// The parent session has closed, so there is no channel to save through (§32): the buffer stays
	/// readable but Save is disabled with a note.
	pub parent_gone: bool,
	/// One flag per current line, recomputed on each edit — the gutter's changed-line bars (§32).
	changed: Vec<bool>,
	/// Whether the buffer differs from `original`. Drives the dirty dot and gates Save / the
	/// close-with-unsaved prompt.
	dirty: bool,
	/// Set by "Save & close" (§32): once the save lands the tab drops itself; on a save FAILURE the
	/// flag is cleared and the tab stays, showing the error, so a failed save never silently closes.
	close_after_save: bool,
}

impl Editor {
	/// A fresh editor waiting on its bytes (§32): an empty buffer, `Loading`, parented to `session`.
	/// The encoding is a placeholder until `set_loaded` learns the real one.
	pub fn loading(session: u64, path: String) -> Self {
		Self {
			session,
			path,
			encoding: Encoding::UTF8_NO_BOM,
			content: text_editor::Content::new(),
			original: Vec::new(),
			status: Status::Loading,
			saving: false,
			notice: None,
			save_as: None,
			parent_gone: false,
			changed: Vec::new(),
			dirty: false,
			close_after_save: false,
		}
	}

	/// Fill the buffer once the decoded text and its encoding arrive (§32). The freshly loaded text
	/// is the baseline, so nothing is marked changed and the editor is clean.
	pub fn set_loaded(&mut self, text: String, encoding: Encoding) {
		self.content = text_editor::Content::with_text(&text);
		self.encoding = encoding;
		self.original = lines_of(&self.content);
		self.status = Status::Ready;
		self.notice = None;
		self.recompute();
	}

	/// The load failed (too big, unreadable, unsupported): show the reason in place of the buffer.
	pub fn load_failed(&mut self, reason: String) {
		self.status = Status::Failed(reason);
	}

	/// Apply one editor action to the buffer (§32). An editing action refreshes the changed-line
	/// marks and the dirty flag; a selection or scroll leaves them alone. Returns whether it edited,
	/// so the caller need not re-check.
	pub fn perform(&mut self, action: text_editor::Action) -> bool {
		let edited = action.is_edit();
		self.content.perform(action);
		if edited {
			self.recompute();
		}
		edited
	}

	/// The bytes to write for a save (§32): the current text encoded as the file was opened.
	pub fn save_bytes(&self) -> Vec<u8> {
		encode(&self.content.text(), self.encoding)
	}

	/// A save succeeded: the current text becomes the new baseline, so the marks and the dirty dot
	/// clear (§32).
	pub fn mark_saved(&mut self) {
		self.original = lines_of(&self.content);
		self.saving = false;
		self.notice = None;
		self.recompute();
	}

	/// A save failed: keep the buffer dirty and surface the reason without disturbing the edits.
	pub fn save_failed(&mut self, reason: String) {
		self.saving = false;
		self.notice = Some(reason);
	}

	/// The parent session closed (§32): saving is no longer possible.
	pub fn mark_parent_gone(&mut self) {
		self.parent_gone = true;
	}

	/// Begin a Save, if one is warranted (§32): dirty, not already saving, and a channel to save
	/// through. Returns whether the caller should flush the bytes to the network.
	pub fn begin_save(&mut self) -> bool {
		if !self.dirty || self.saving || self.parent_gone || !matches!(self.status, Status::Ready) {
			return false;
		}
		self.saving = true;
		true
	}

	/// Begin a Save that closes the tab once it lands (§32) — the unsaved-changes prompt's "Save &
	/// close". Returns whether the caller should flush; `false` (nothing to save, or no channel)
	/// means the caller should just close the tab.
	pub fn begin_save_and_close(&mut self) -> bool {
		if self.begin_save() {
			self.close_after_save = true;
			return true;
		}
		false
	}

	/// Read and clear the "close once saved" flag (§32). The tab calls this on a successful save to
	/// learn whether to drop itself; a failed save clears it too, so the error can be seen instead.
	pub fn take_close_after_save(&mut self) -> bool {
		std::mem::take(&mut self.close_after_save)
	}

	/// Open the Save As prompt, pre-filled with the current path (§32).
	pub fn begin_save_as(&mut self) {
		if self.parent_gone || !matches!(self.status, Status::Ready) {
			return;
		}
		self.save_as = Some(self.path.clone());
	}

	/// Update the Save As path as the user types.
	pub fn save_as_changed(&mut self, path: String) {
		if let Some(current) = &mut self.save_as {
			*current = path;
		}
	}

	/// Close the Save As prompt without saving.
	pub fn save_as_cancel(&mut self) {
		self.save_as = None;
	}

	/// Confirm Save As: re-point the editor at the typed path and begin the save (§32). Returns
	/// whether the caller should flush — `false` for a blank or whitespace-only path, which is a
	/// no-op that leaves the prompt open.
	pub fn save_as_confirm(&mut self) -> bool {
		let Some(path) = self.save_as.as_ref().map(|p| p.trim().to_owned()) else {
			return false;
		};
		if path.is_empty() {
			return false;
		}
		self.path = path;
		self.save_as = None;
		self.saving = true;
		true
	}

	/// Whether the buffer has unsaved edits (§32) — drives the dirty dot and the close prompt.
	pub fn is_dirty(&self) -> bool {
		self.dirty
	}

	/// The gutter's changed-line flags, one per current line (§32).
	pub fn changed(&self) -> &[bool] {
		&self.changed
	}

	/// The line ending the buffer uses, for the toolbar ("LF" / "CRLF" / "CR"). A buffer with no
	/// newline reads as LF, the default cmote writes (§32).
	pub fn line_ending_label(&self) -> &'static str {
		use text_editor::LineEnding;
		match self.content.line_ending().unwrap_or(LineEnding::Lf) {
			LineEnding::Lf | LineEnding::None => "LF",
			LineEnding::CrLf => "CRLF",
			LineEnding::Cr => "CR",
			LineEnding::LfCr => "LF-CR",
		}
	}

	/// Refresh the dirty flag and the changed-line marks against the baseline (§32). Called after
	/// every edit and every save, never on the render path.
	fn recompute(&mut self) {
		let current = lines_of(&self.content);
		self.dirty = current != self.original;
		self.changed = changed_flags(&self.original, &current);
	}
}

/// The buffer's lines as owned strings, endings dropped (§32). The diff and the dirty check compare
/// text, not endings — iced never changes an ending on its own, so an ending shift is not an edit.
fn lines_of(content: &text_editor::Content) -> Vec<String> {
	content.lines().map(|line| line.text.into_owned()).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn utf8_without_bom_is_the_default() {
		// Arrange
		let bytes = b"hello\nworld";
		// Act
		let (text, encoding) = decode(bytes).expect("valid UTF-8 opens");
		// Assert
		assert_eq!(text, "hello\nworld");
		assert_eq!(encoding, Encoding::UTF8_NO_BOM);
	}

	#[test]
	fn a_utf8_bom_is_detected_and_stripped_but_remembered() {
		let mut bytes = BOM_UTF8.to_vec();
		bytes.extend_from_slice(b"hi");
		let (text, encoding) = decode(&bytes).expect("UTF-8 BOM opens");
		// The BOM is metadata, not a character, so it is not in the text...
		assert_eq!(text, "hi");
		// ...but it is remembered so the save re-emits it.
		assert_eq!(
			encoding,
			Encoding {
				charset: Charset::Utf8,
				bom: true
			}
		);
	}

	#[test]
	fn utf16_le_and_be_boms_decode() {
		// "AB" in UTF-16 LE and BE, each with its BOM.
		let le = [0xFF, 0xFE, b'A', 0x00, b'B', 0x00];
		let be = [0xFE, 0xFF, 0x00, b'A', 0x00, b'B'];
		assert_eq!(decode(&le).unwrap().0, "AB");
		assert_eq!(decode(&le).unwrap().1.charset, Charset::Utf16Le);
		assert_eq!(decode(&be).unwrap().0, "AB");
		assert_eq!(decode(&be).unwrap().1.charset, Charset::Utf16Be);
	}

	#[test]
	fn binary_and_utf32_are_unsupported() {
		// An invalid UTF-8 byte with no BOM.
		assert!(decode(&[0xFF, 0x00, 0x9A]).is_none());
		// A UTF-32 LE BOM must not be mis-read as UTF-16.
		assert!(decode(&[0xFF, 0xFE, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00]).is_none());
		// A UTF-32 BE BOM is not valid UTF-8 either.
		assert!(decode(&[0x00, 0x00, 0xFE, 0xFF]).is_none());
	}

	#[test]
	fn encode_round_trips_every_supported_encoding() {
		let text = "café\nsecond line\n";
		for encoding in [
			Encoding::UTF8_NO_BOM,
			Encoding {
				charset: Charset::Utf8,
				bom: true,
			},
			Encoding {
				charset: Charset::Utf16Le,
				bom: true,
			},
			Encoding {
				charset: Charset::Utf16Be,
				bom: true,
			},
		] {
			let bytes = encode(text, encoding);
			let (back, detected) = decode(&bytes).expect("our own output decodes");
			assert_eq!(back, text, "text survives {}", encoding.label());
			assert_eq!(detected, encoding, "encoding survives {}", encoding.label());
		}
	}

	#[test]
	fn a_bomless_utf8_save_adds_no_bom() {
		// The stated default: persist a BOM-less file without one.
		let bytes = encode("plain", Encoding::UTF8_NO_BOM);
		assert_eq!(bytes, b"plain");
	}

	fn lines(text: &[&str]) -> Vec<String> {
		text.iter().map(|s| (*s).to_owned()).collect()
	}

	#[test]
	fn an_unchanged_buffer_marks_nothing() {
		let base = lines(&["a", "b", "c"]);
		assert_eq!(changed_flags(&base, &base), vec![false, false, false]);
	}

	#[test]
	fn an_edited_line_marks_only_itself() {
		let base = lines(&["a", "b", "c"]);
		let now = lines(&["a", "B!", "c"]);
		assert_eq!(changed_flags(&base, &now), vec![false, true, false]);
	}

	#[test]
	fn an_inserted_line_marks_only_the_insertion() {
		let base = lines(&["a", "b", "c"]);
		let now = lines(&["a", "x", "b", "c"]);
		// Prefix "a", suffix "b","c" trim away, leaving the inserted "x" as the only mark.
		assert_eq!(changed_flags(&base, &now), vec![false, true, false, false]);
	}

	#[test]
	fn a_deleted_line_marks_nothing_in_the_shorter_buffer() {
		let base = lines(&["a", "b", "c"]);
		let now = lines(&["a", "c"]);
		// The deletion leaves no current line to mark; dirtiness is tracked elsewhere.
		assert_eq!(changed_flags(&base, &now), vec![false, false]);
	}

	#[test]
	fn appended_lines_are_marked() {
		let base = lines(&["a"]);
		let now = lines(&["a", "b", "c"]);
		assert_eq!(changed_flags(&base, &now), vec![false, true, true]);
	}
}

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

/// One search hit (§32): the line it is on and the byte range within that line's text. Byte offsets,
/// not character offsets, because iced places the cursor and the selection by BYTE index within a
/// line (`Position::column` is a byte index) — so a match found here can be selected verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Match {
	line: usize,
	byte_start: usize,
	byte_end: usize,
}

/// Every occurrence of `query` across `lines`, in document order (§32). The search is ASCII
/// case-insensitive: both sides are lowered with `to_ascii_lowercase`, which touches only `A`–`Z`
/// and so preserves every byte offset — the offsets found in the lowered copy are valid in the
/// original. (A non-ASCII case pair like `é`/`É` therefore stays distinct; a narrow, predictable
/// rule, the same spirit as the encoding set.) An empty query matches nothing.
fn find_matches(lines: &[String], query: &str) -> Vec<Match> {
	let mut out = Vec::new();
	if query.is_empty() {
		return out;
	}
	let needle = query.to_ascii_lowercase();
	for (line, text) in lines.iter().enumerate() {
		let hay = text.to_ascii_lowercase();
		let mut from = 0;
		// `find` respects UTF-8 boundaries, and `from` only ever lands on a match end (a boundary), so
		// the slice below is always valid. Matches do not overlap — each search resumes past the last.
		while let Some(rel) = hay[from..].find(&needle) {
			let byte_start = from + rel;
			let byte_end = byte_start + needle.len();
			out.push(Match {
				line,
				byte_start,
				byte_end,
			});
			from = byte_end;
		}
	}
	out
}

/// Apply `replacement` to every `matches` span in `lines`, returning the new lines (§32). Each line
/// is spliced from its rightmost match leftward so the earlier byte offsets on that line stay valid
/// as later ones are replaced; iterating the (document-ordered) matches in reverse gives exactly that
/// order. Used by Replace All — the matches it is handed are the ones the bar found, so what is
/// replaced is exactly what was highlighted.
fn apply_replacements(lines: &[String], matches: &[Match], replacement: &str) -> Vec<String> {
	let mut out = lines.to_vec();
	for m in matches.iter().rev() {
		out[m.line].replace_range(m.byte_start..m.byte_end, replacement);
	}
	out
}

/// The editor's find/replace state (§32): the query, every match, which one is current, and the
/// replace companion. Held as `Editor::find = Some(..)` only while the bar is open; closing drops it.
/// The matches are recomputed whenever the query or the buffer changes, so the count the bar shows
/// and the span it highlights always reflect the live text.
#[derive(Debug, Default)]
pub struct Find {
	/// The text being searched for. Empty means no matches and an idle bar.
	pub query: String,
	/// The replacement text for Replace / Replace All.
	pub replace: String,
	/// Whether the replace row is shown (its toggle, or Ctrl+H).
	pub replace_open: bool,
	/// Every match in the buffer, in document order (§32).
	matches: Vec<Match>,
	/// Which match is current — the one highlighted and stepped from. Zero and meaningless when
	/// `matches` is empty.
	current: usize,
}

impl Find {
	/// How many matches the query has right now (§32) — the denominator the bar shows.
	pub fn count(&self) -> usize {
		self.matches.len()
	}

	/// The current match's 1-based position for display ("3 / 12"), or `0` when there are none (§32).
	pub fn ordinal(&self) -> usize {
		if self.matches.is_empty() {
			0
		} else {
			self.current + 1
		}
	}
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

/// Which colour scheme an editor tab paints with (§32). Only the choice lives here in the model —
/// the concrete colours are the view's (`ui::editor`), so the split stays clean. The choice is held
/// on the tab and remembered per file extension by `App`, so reopening a `.json` comes up in the
/// scheme last used for JSON, independent of what a `.rs` or `.php` tab is set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorTheme {
	/// cmote's own dark panel palette — the default, matching the files pane and dialogs.
	#[default]
	Default,
	/// "CME": the colours of the user's VS Code theme (Themer My Color Set Dark), ported so a file
	/// reads here much as it does in the editor it was authored in.
	Cme,
}

impl EditorTheme {
	/// The label the toolbar's theme select shows for this scheme.
	pub fn label(self) -> &'static str {
		match self {
			EditorTheme::Default => "Default",
			EditorTheme::Cme => "CME",
		}
	}
}

/// The lower-cased file extension a theme choice is remembered under (§32) — `json` for `Notes.JSON`,
/// empty for a file with no extension (and for a dot-file like `.bashrc`, whose leading dot is a
/// hidden-file marker, not an extension). Splitting on both slash kinds tolerates a stray backslash
/// in an otherwise POSIX remote path.
pub fn extension_key(path: &str) -> String {
	let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
	match name.rfind('.') {
		Some(dot) if dot > 0 => name[dot + 1..].to_ascii_lowercase(),
		_ => String::new(),
	}
}

/// What the editor view can ask of its tab (§32). The tab applies the ones that only touch this
/// buffer (typing, the Save As prompt's own field); `Save` and `SaveAsConfirm`, which have to reach
/// the parent session's channel, are turned by the tab into an App-level flush.
#[derive(Debug, Clone)]
pub enum EditorMessage {
	/// An action from the text widget — an edit, a selection, a scroll (§32).
	Action(text_editor::Action),
	/// The outer scrollable moved: its current vertical offset and visible height (§32). Reported on
	/// every scroll and on the first frame, so the cursor-follow can keep the cursor line on screen
	/// without tracking the widget's own hidden offset.
	Scrolled { offset: f32, view_height: f32 },
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
	/// Open the find bar, or refocus it if already open (Ctrl+F) (§32).
	FindOpen,
	/// Close the find bar (Esc) (§32).
	FindClose,
	/// The find query changed — re-search and jump to the first match.
	FindQueryChanged(String),
	/// Step to the next (`true`) or previous (`false`) match, wrapping at the ends (§32).
	FindStep(bool),
	/// Show or hide the replace row (§32).
	ReplaceToggle,
	/// The replacement text changed.
	ReplaceChanged(String),
	/// Replace the current match and advance to the next (§32).
	ReplaceOne,
	/// Replace every match in one pass (§32).
	ReplaceAll,
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
	/// The colour scheme this editor paints with (§32). Seeded from `App`'s per-extension memory when
	/// the tab opens, and changed by the toolbar's theme select.
	pub theme: EditorTheme,
	/// The outer scrollable's current vertical offset and its visible height, as reported by
	/// `on_scroll` (§32). iced's `text_editor` hides its own scroll offset, so cmote defeats the
	/// widget's internal scroll (the gutter trick) and drives one outer scrollable instead — these two
	/// numbers are all the cursor-follow needs to keep the cursor line on screen after a move. Both
	/// are `0.0` until the first frame reports them, and the follow skips while the height is zero so
	/// it never scrolls against an unmeasured viewport.
	scroll: f32,
	view_height: f32,
	/// The find/replace bar's state while it is open, or `None` when closed (§32). Recomputed against
	/// the buffer on every edit so its match count stays live, and it drives the selection the buffer
	/// highlights as the user steps through hits.
	pub find: Option<Find>,
}

impl Editor {
	/// A fresh editor waiting on its bytes (§32): an empty buffer, `Loading`, parented to `session`,
	/// painting with `theme` (the scheme `App` remembers for this file's extension). The encoding is a
	/// placeholder until `set_loaded` learns the real one.
	pub fn loading(session: u64, path: String, theme: EditorTheme) -> Self {
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
			theme,
			scroll: 0.0,
			view_height: 0.0,
			find: None,
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

	/// Switch the colour scheme this editor paints with (§32) — the toolbar's theme select.
	pub fn set_theme(&mut self, theme: EditorTheme) {
		self.theme = theme;
	}

	/// Note the outer scrollable's offset and visible height (§32), reported by `on_scroll` on every
	/// scroll and on the first frame — the two numbers the cursor-follow reads.
	pub fn set_viewport(&mut self, offset: f32, view_height: f32) {
		self.scroll = offset;
		self.view_height = view_height;
	}

	/// The outer scrollable's current vertical offset (§32).
	pub fn scroll(&self) -> f32 {
		self.scroll
	}

	/// The outer scrollable's visible height, `0.0` until the first frame reports it (§32). The
	/// cursor-follow skips while this is zero, so it never scrolls against an unmeasured viewport.
	pub fn view_height(&self) -> f32 {
		self.view_height
	}

	/// The line the cursor sits on (§32) — what the cursor-follow scrolls onto screen. iced hides the
	/// widget's own scroll offset but exposes the cursor, so this line index plus the fixed line
	/// height is enough to place it in the outer scrollable.
	pub fn cursor_line(&self) -> usize {
		self.content.cursor().position.line
	}

	/// Open the find bar (§32), keeping any query it already held, and select its current match so
	/// reopening lands on a hit. Returns whether a match is now selected, so the caller can scroll it
	/// into view.
	pub fn find_open(&mut self) -> bool {
		if self.find.is_none() {
			self.find = Some(Find::default());
		}
		self.refind()
	}

	/// Close the find bar and forget its matches (§32). The buffer's own selection is left as it is —
	/// closing search does not deselect what was found.
	pub fn find_close(&mut self) {
		self.find = None;
	}

	/// The find query changed (§32): re-search from the top and select the first match. Returns
	/// whether a match is selected, so the caller can follow it.
	pub fn find_query_changed(&mut self, query: String) -> bool {
		if let Some(find) = self.find.as_mut() {
			find.query = query;
			find.current = 0;
		}
		self.refind()
	}

	/// Step to the next or previous match, wrapping at the ends (§32), and select it. Returns whether
	/// a match is selected (false when the query has none), so the caller can follow it.
	pub fn find_step(&mut self, forward: bool) -> bool {
		let selected = {
			let Some(find) = self.find.as_mut() else {
				return false;
			};
			let len = find.matches.len();
			if len == 0 {
				return false;
			}
			find.current = if forward {
				(find.current + 1) % len
			} else {
				// `+ len - 1` rather than `- 1` so a wrap from the first match stays in range.
				(find.current + len - 1) % len
			};
			find.matches[find.current]
		};
		self.select_match(selected);
		true
	}

	/// Show or hide the replace row (§32).
	pub fn replace_toggle(&mut self) {
		if let Some(find) = self.find.as_mut() {
			find.replace_open = !find.replace_open;
		}
	}

	/// The replacement text changed (§32).
	pub fn replace_changed(&mut self, text: String) {
		if let Some(find) = self.find.as_mut() {
			find.replace = text;
		}
	}

	/// Replace the current match with the replacement and move to the next (§32). The match is already
	/// the buffer's selection, so pasting over it swaps just that span — which keeps the widget's own
	/// undo, unlike Replace All. The buffer is then re-searched (offsets have shifted) and the current
	/// index, unchanged, now points at the following hit. Returns whether the buffer changed.
	pub fn replace_one(&mut self) -> bool {
		let (selected, replacement) = {
			let Some(find) = self.find.as_ref() else {
				return false;
			};
			if find.matches.is_empty() {
				return false;
			}
			(find.matches[find.current], find.replace.clone())
		};
		// Make sure the span we are about to overwrite is exactly the selection, then paste over it.
		self.select_match(selected);
		self.content
			.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
				std::sync::Arc::new(replacement),
			)));
		self.recompute();
		self.refind();
		true
	}

	/// Replace every match in one pass (§32). Unlike a per-match walk, this rebuilds the whole buffer
	/// from the matches the bar already found — so what changes is exactly what was highlighted — and
	/// re-seats it as a fresh `Content`. That resets the widget's undo history, the accepted cost of a
	/// bulk edit (a single Replace keeps undo). Returns whether the buffer changed.
	pub fn replace_all(&mut self) -> bool {
		let (matches, replacement) = {
			let Some(find) = self.find.as_ref() else {
				return false;
			};
			if find.matches.is_empty() {
				return false;
			}
			(find.matches.clone(), find.replace.clone())
		};
		let lines = lines_of(&self.content);
		let ending = self
			.content
			.line_ending()
			.unwrap_or(text_editor::LineEnding::Lf);
		let joined = apply_replacements(&lines, &matches, &replacement).join(ending.as_str());
		self.content = text_editor::Content::with_text(&joined);
		self.recompute();
		self.refind();
		true
	}

	/// Recompute the matches for the current query and select the current one so the buffer highlights
	/// it (§32). Shared by open / query-change / replace — every path that should re-search and jump.
	/// Returns whether a match is selected. Does nothing when the bar is closed.
	fn refind(&mut self) -> bool {
		if self.find.is_none() {
			return false;
		}
		let lines = lines_of(&self.content);
		let selected = {
			let find = self.find.as_mut().expect("just checked it is Some");
			find.matches = find_matches(&lines, &find.query);
			if find.matches.is_empty() {
				find.current = 0;
				None
			} else {
				// A query change resets `current` to 0; a buffer edit may have shrunk the match list, so
				// clamp either way before reading.
				if find.current >= find.matches.len() {
					find.current = 0;
				}
				Some(find.matches[find.current])
			}
		};
		match selected {
			Some(m) => {
				self.select_match(m);
				true
			}
			None => false,
		}
	}

	/// Select a match's span in the buffer so it highlights, cursor at its end (§32). iced selects by
	/// byte position within a line, which is exactly what `Match` carries.
	fn select_match(&mut self, m: Match) {
		self.content.move_to(text_editor::Cursor {
			position: text_editor::Position {
				line: m.line,
				column: m.byte_end,
			},
			selection: Some(text_editor::Position {
				line: m.line,
				column: m.byte_start,
			}),
		});
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
		// Keep the find bar's match list current as the buffer changes (§32) — but do NOT re-select,
		// so typing never yanks the cursor onto a match. The count the bar shows stays honest; the
		// jump-to-match only happens on an explicit search step.
		if let Some(find) = self.find.as_mut() {
			find.matches = find_matches(&current, &find.query);
			if find.current >= find.matches.len() {
				find.current = 0;
			}
		}
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

	#[test]
	fn extension_key_is_the_lowercased_extension_or_empty() {
		// The extension, lower-cased, drives the per-file-type theme memory (§32).
		assert_eq!(extension_key("/etc/app/config.JSON"), "json");
		assert_eq!(extension_key("src/main.rs"), "rs");
		assert_eq!(extension_key("weird.name.Ts"), "ts");
		// No extension, and a dot-file whose leading dot is not one, both bucket together as "".
		assert_eq!(extension_key("/var/log/messages"), "");
		assert_eq!(extension_key("/home/me/.bashrc"), "");
	}

	#[test]
	fn find_matches_are_ascii_case_insensitive_and_in_document_order() {
		// Arrange — "to" appears twice on line 0 (once cased differently) and once on line 2.
		let lines = lines(&["To do or not to do", "nothing here", "auto"]);
		// Act
		let hits = find_matches(&lines, "to");
		// Assert — order is line then byte offset; "To", "to", and the "to" inside "auto" all match.
		assert_eq!(
			hits,
			vec![
				Match {
					line: 0,
					byte_start: 0,
					byte_end: 2
				},
				Match {
					line: 0,
					byte_start: 13,
					byte_end: 15
				},
				Match {
					line: 2,
					byte_start: 2,
					byte_end: 4
				},
			]
		);
	}

	#[test]
	fn find_matches_do_not_overlap_and_an_empty_query_finds_nothing() {
		// "aa" in "aaaa" is two non-overlapping hits (0..2, 2..4), not three.
		let hits = find_matches(&lines(&["aaaa"]), "aa");
		assert_eq!(
			hits.iter().map(|m| m.byte_start).collect::<Vec<_>>(),
			vec![0, 2]
		);
		// An empty query is idle, not "everything".
		assert!(find_matches(&lines(&["anything"]), "").is_empty());
	}

	#[test]
	fn apply_replacements_swaps_every_span_and_keeps_offsets_valid() {
		// Two matches on one line: replacing right-to-left keeps the left offset valid.
		let lines = lines(&["foo and foo", "no match", "foo"]);
		let matches = find_matches(&lines, "foo");
		let out = apply_replacements(&lines, &matches, "BAR");
		assert_eq!(
			out,
			vec![
				"BAR and BAR".to_owned(),
				"no match".to_owned(),
				"BAR".to_owned()
			]
		);
	}
}

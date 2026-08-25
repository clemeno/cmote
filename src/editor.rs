// editor.rs — the in-tab text editor's model (PLAN §32).
//
// The pure half of the editor: the byte↔text encoding (BOM detection and the small UTF set we
// support), the changed-line diff that drives the gutter marks, and the `Editor` state a tab
// carries when it is editing a file rather than running a session. The network calls live in
// `ssh/edit.rs` and the drawing in `ui/editor.rs`, so everything here is testable with no server —
// the same three-way split the panes use (§18, §19).
//
// The encoding rule (the one the user set): keep a BOM if the file has one, let a BOM decide the
// UTF, assume UTF-8-without-BOM when there is none, refuse what cannot be decoded, and on save
// persist EXACTLY as opened — never convert behind the user's back.

use std::ops::Range;

use iced::widget::text_editor;

/// The largest edit band we will diff with an exact (quadratic) LCS before falling back to marking
/// the whole band changed (§32). A normal edit touches a handful of lines, so the band is tiny and
/// the LCS is cheap; this cap only guards a pathological "changed a thousand lines at once" so a
/// keystroke can never pay an O(n²) diff over the whole buffer.
const LCS_BAND_CAP: usize = 1000;

/// A tab's width in display columns for the horizontal-extent estimate (§32) — the common 8, matching
/// how most tools expand a tab. It sizes the scroll extent and places the cursor for the horizontal
/// follow; it never renders (iced lays the glyphs out itself, so this need only be close, not exact).
const TAB_WIDTH: usize = 8;

/// How much text goes into one insert when a loaded file's buffer is built (§121).
///
/// This number is MEASURED, not chosen, and it is the whole of why opening a big file no longer
/// freezes the window. Two facts about iced 0.14 sit behind it:
///
/// * `Content::with_text` calls cosmic-text's `set_text` with `Shaping::Advanced`, which shapes
///   EVERY line up front — about 0.5 ms per KiB, so nearly four seconds at the 8 MiB ceiling, all of
///   it on the GUI thread. A paste defers shaping to layout, and layout only ever shapes what is on
///   screen.
/// * A paste is QUADRATIC in the length of the one insert. So the chunk is not a nicety; it is the
///   trick. Pasting a whole 8 MiB file in one go is far WORSE than what it replaces.
///
/// At the 8 MiB ceiling, release build:
///
/// ```text
///   with_text            3918 ms     chunks of  32 KiB     193 ms
///   one whole paste    130251 ms     chunks of   8 KiB      65 ms
///   chunks of 128 KiB     746 ms     chunks of   4 KiB      53 ms
///                                    chunks of   2 KiB      84 ms  <- call overhead takes over
/// ```
///
/// 8 KiB rather than the 12 ms faster 4 KiB: half the calls for a difference no one can perceive,
/// and 65 ms at the absolute ceiling is already inside a handful of frames. Note the shape of the
/// curve — being too LARGE is unbounded, being too small costs a few ms — so if this is ever
/// retuned, err downward.
const PASTE_CHUNK: usize = 8 * 1024;

/// The display-column width of a line (§32): a tab advances to the next `TAB_WIDTH` stop, every other
/// character counts as one column. `ponytail:` a double-width CJK glyph counts as one, so a CJK-heavy
/// line's width is under-estimated (the horizontal extent is a hair short there, never long) — ASCII
/// source, the common case, is exact.
pub fn display_columns(text: &str) -> usize {
	let mut cols = 0;
	for ch in text.chars() {
		cols += column_advance(ch, cols);
	}
	cols
}

/// How many display columns `ch` occupies when it starts at column `cols` (§32) — one for an ordinary
/// character, or the jump to the next tab stop for a tab. Factored out because two walks need the same
/// rule: the whole-line width above, and the per-offset walk below (§138).
fn column_advance(ch: char, cols: usize) -> usize {
	if ch == '\t' {
		TAB_WIDTH - (cols % TAB_WIDTH)
	} else {
		1
	}
}

/// The display column each byte offset in `offsets` sits at, in ONE pass over `text` (§138).
///
/// `offsets` must be ascending — they are, being match boundaries in document order — which is what
/// makes this one traversal rather than one per offset. A one-letter query on a long line has hundreds
/// of hits, and asking `display_columns` for each prefix separately would be quadratic in the line.
/// An offset at or past the line's end lands on the line's final column.
fn display_columns_at(text: &str, offsets: &[usize]) -> Vec<usize> {
	let mut out = Vec::with_capacity(offsets.len());
	let mut cols = 0;
	for (byte, ch) in text.char_indices() {
		while out.len() < offsets.len() && offsets[out.len()] <= byte {
			out.push(cols);
		}
		cols += column_advance(ch, cols);
	}
	while out.len() < offsets.len() {
		out.push(cols);
	}
	out
}

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
pub fn decode_text(bytes: &[u8]) -> Option<(String, Encoding)> {
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
	// `as_chunks::<2>` hands back `&[[u8; 2]]`, which is the exact type `from_?e_bytes` wants — so
	// each pair goes straight in, with no array rebuilt by hand from two indexes. `.0` drops the
	// remainder, which the length guard above has already established is empty.
	let units = bytes.as_chunks::<2>().0.iter().map(|pair| {
		if little_endian {
			u16::from_le_bytes(*pair)
		} else {
			u16::from_be_bytes(*pair)
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
///
/// `pub` because the buffer draws an inverted block behind every visible hit (§138), and the block's
/// left edge and width come from exactly these two offsets.
/// It also carries the hit's DISPLAY columns, computed here rather than in the view. The view needs
/// them to place the block — a column times the character advance is its left edge — and the only way
/// to get one from a byte offset is to walk the line expanding tabs. Doing that in the view would mean
/// re-walking every visible line on every frame; doing it here means once, when the search runs (§138).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorMatch {
	/// The zero-based line the hit is on.
	pub line: usize,
	/// The byte offset of the hit's first byte within that line's text.
	pub byte_start: usize,
	/// The byte offset one past the hit's last byte.
	pub byte_end: usize,
	/// Display columns before the hit, tabs expanded — the block's left edge.
	pub col_start: usize,
	/// Display columns before the hit's end — so `col_end - col_start` is the block's width.
	pub col_end: usize,
}

/// Every occurrence of `query` within ONE line's text, as byte ranges in document order (§32). The
/// search is ASCII case-insensitive: both sides are lowered with `to_ascii_lowercase`, which touches
/// only `A`–`Z` and so preserves every byte offset — the offsets found in the lowered copy are valid
/// in the original. (A non-ASCII case pair like `é`/`É` therefore stays distinct; a narrow,
/// predictable rule, the same spirit as the encoding set.) An empty query matches nothing.
///
/// Its own function, and `pub(crate)`, because THREE places must agree on what "a match" is (§138):
/// the find bar's count, the inverted block the buffer paints behind each hit, and the highlighter
/// that recolours the glyphs inside that block. A second implementation of this loop anywhere would
/// be a way for the count, the block and the ink to disagree — the block sitting where no hit is.
pub(crate) fn line_matches(text: &str, query: &str) -> Vec<Range<usize>> {
	let mut out = Vec::new();
	if query.is_empty() {
		return out;
	}
	let needle = query.to_ascii_lowercase();
	let hay = text.to_ascii_lowercase();
	let mut from = 0;
	// `find` respects UTF-8 boundaries, and `from` only ever lands on a match end (a boundary), so the
	// slice below is always valid. Matches do not overlap — each search resumes past the last.
	while let Some(rel) = hay[from..].find(&needle) {
		let start = from + rel;
		let end = start + needle.len();
		out.push(start..end);
		from = end;
	}
	out
}

/// Every occurrence of `query` across `lines`, in document order (§32) — `line_matches` run down the
/// buffer, so the bar's count is the same rule the buffer paints with.
fn find_matches(lines: &[String], query: &str) -> Vec<EditorMatch> {
	let mut out = Vec::new();
	for (line, text) in lines.iter().enumerate() {
		let spans = line_matches(text, query);
		if spans.is_empty() {
			continue;
		}
		// Every boundary of every hit on this line, flattened and ascending, so one walk of the line
		// resolves them all to display columns (§138).
		let bounds: Vec<usize> = spans
			.iter()
			.flat_map(|span| [span.start, span.end])
			.collect();
		let cols = display_columns_at(text, &bounds);
		out.extend(spans.iter().enumerate().map(|(i, span)| EditorMatch {
			line,
			byte_start: span.start,
			byte_end: span.end,
			col_start: cols[i * 2],
			col_end: cols[i * 2 + 1],
		}));
	}
	out
}

/// Apply `replacement` to every `matches` span in `lines`, returning the new lines (§32). Each line
/// is spliced from its rightmost match leftward so the earlier byte offsets on that line stay valid
/// as later ones are replaced; iterating the (document-ordered) matches in reverse gives exactly that
/// order. Used by Replace All — the matches it is handed are the ones the bar found, so what is
/// replaced is exactly what was highlighted.
fn apply_replacements(lines: &[String], matches: &[EditorMatch], replacement: &str) -> Vec<String> {
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
	matches: Vec<EditorMatch>,
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

	/// The line the current match sits on, or `None` when the query has no matches (§32) — the line the
	/// gutter number and the buffer band highlight.
	pub fn current_line(&self) -> Option<usize> {
		self.matches.get(self.current).map(|m| m.line)
	}

	/// The matches falling on lines `first..last` (§138) — the window the buffer paints an inverted
	/// block behind, one per hit.
	///
	/// Two binary searches, not a scan, and that is the point: `matches` is in document order, so it is
	/// sorted by line, and the view asks this question every frame. A one-letter query in a big file has
	/// tens of thousands of hits, and the view must never walk them all to draw the fifty on screen.
	pub fn spans_between(&self, first: usize, last: usize) -> &[EditorMatch] {
		let from = self.matches.partition_point(|m| m.line < first);
		let to = self.matches.partition_point(|m| m.line < last);
		// `partition_point` is monotone in its predicate, so `from <= to` whenever `first <= last`; the
		// `max` covers a caller that hands them the other way round rather than panicking on the slice.
		&self.matches[from..to.max(from)]
	}
}

/// Where an editor tab is in its lifecycle (§32). `Loading` until the bytes arrive; `Ready` with a
/// live buffer; `Failed` when the file is too big, binary, or an unsupported encoding — the view
/// then shows the reason in place of the buffer, never mojibake.
///
/// `Loading` carries how far the read has got (§121) rather than that living in a field of its own
/// beside the status. A field would be able to hold a share for a load that is not running, which is
/// the shape §111 went out of its way to remove from the save side: there is no such state here to
/// get into, and so none to remember to clear.
#[derive(Debug, Clone)]
pub enum EditorStatus {
	Loading(crate::viewer::LoadProgress),
	Ready,
	Failed(String),
}

/// Whether a save is in flight, and what should happen when it lands (§32).
///
/// One value rather than the `saving` / `close_after_save` pair it replaces (§111). The pair could
/// express a state that has no meaning — "close when the save lands" while no save is in flight — and
/// nothing stopped it: the close flag was set immediately after `saving`, and had to be cleared by
/// hand on the failure path purely so a failed "Save & close" would not close the tab later. Here
/// there is no such state to get into, and no such clearing to remember.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SaveFlight {
	/// Nothing in flight. Save is offered when the buffer is dirty and a channel exists.
	#[default]
	Idle,
	/// A save is in flight; the toolbar disables Save so a second cannot race the first.
	Saving,
	/// The same, from the unsaved-changes prompt's "Save & close": the tab drops itself once the
	/// write lands, and stays — showing the error — if it does not.
	SavingToClose,
}

/// Which colour scheme an editor tab paints with (§32). Only the choice lives here in the model —
/// the concrete colours are the view's (`ui::editor`), so the split stays clean. The choice is held
/// on the tab and remembered per file extension in `Settings`, so reopening a `.json` comes up in
/// the scheme last used for JSON, independent of what a `.rs` or `.php` tab is set to — and that
/// memory now survives a restart, written into `settings.json` with the rest of the layout (§31).
///
/// `serde` serializes the two variants lower-cased (`"default"` / `"cme"`), so the settings file
/// stays legible to a hand-editor; an unrecognised value there fails the whole parse back to
/// defaults, the same "a bad file never stops the app" rule the rest of `settings.rs` follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
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

/// The file name (basename) of a remote path (§32): the last path segment, splitting on both slash
/// kinds so a stray backslash in an otherwise POSIX path is still handled. Borrowed from `path`, so
/// the caller owns no copy. Used both for the theme key (via `extension_key`) and for grammar
/// resolution, which needs the whole name (`Makefile`), not just the extension.
pub fn file_name(path: &str) -> &str {
	path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The lower-cased file extension a theme choice is remembered under (§32) — `json` for `Notes.JSON`,
/// empty for a file with no extension (and for a dot-file like `.bashrc`, whose leading dot is a
/// hidden-file marker, not an extension).
pub fn extension_key(path: &str) -> String {
	let name = file_name(path);
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
	/// The buffer's scrollable moved: its current offset and visible size on BOTH axes (§32). Reported
	/// on every scroll and on the first frame, so the cursor-follow can keep the cursor line and column
	/// on screen without tracking the widget's own hidden offset. The buffer now scrolls horizontally
	/// too (a long line no longer only reachable by arrowing into it), so all four numbers ride here.
	Scrolled {
		offset_x: f32,
		offset_y: f32,
		view_width: f32,
		view_height: f32,
	},
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
	/// The ACCOUNT on that session the file was opened as (§45's identity, §46). Fixed for the
	/// editor's whole life rather than following the session's current selection: a file opened as
	/// root is a root-owned file, and its save has to reach it as root however the panes have moved
	/// on. Reading it as whoever happens to be on screen at save time would fail — or, worse,
	/// succeed against a different file of the same path in another account's chroot.
	pub identity: u64,
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
	pub status: EditorStatus,
	/// Whether a save is in flight, and what should happen when it lands (§32).
	flight: SaveFlight,
	/// A transient message shown in the toolbar (a save failure). Distinct from `EditorStatus::Failed`,
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
	/// The colour scheme this editor paints with (§32). Seeded from `App`'s per-extension memory when
	/// the tab opens, and changed by the toolbar's theme select.
	pub theme: EditorTheme,
	/// The buffer scrollable's current offset and visible size on both axes, as reported by `on_scroll`
	/// (§32). iced's `text_editor` hides its own scroll offset, so cmote defeats the widget's internal
	/// scroll (the gutter/horizontal trick) and drives one outer scrollable instead — these numbers are
	/// all the cursor-follow needs to keep the cursor line AND column on screen after a move. All are
	/// `0.0` until the first frame reports them, and each follow skips while its extent is zero so it
	/// never scrolls against an unmeasured viewport. `scroll_x` / `view_width` drive the horizontal
	/// follow, the mirror of `scroll` / `view_height` for the vertical one.
	scroll: f32,
	view_height: f32,
	scroll_x: f32,
	view_width: f32,
	/// The buffer's widest line in display columns (tabs expanded), recomputed on every edit (§32).
	/// The view multiplies it by the fixed character advance to size the horizontal scroll extent, so
	/// the `text_editor` is laid out exactly as wide as its content and never scrolls itself — the
	/// horizontal counterpart of laying its HEIGHT out to the whole buffer.
	content_cols: usize,
	/// The find/replace bar's state while it is open, or `None` when closed (§32). Recomputed against
	/// the buffer on every edit so its match count stays live, and it drives the selection the buffer
	/// highlights as the user steps through hits.
	pub find: Option<Find>,
}

impl Editor {
	/// A fresh editor waiting on its bytes (§32): an empty buffer, `Loading`, parented to `session`
	/// and opened as the account `identity` names on it (§46), painting with `theme` (the scheme
	/// `App` remembers for this file's extension). The encoding is a placeholder until `set_loaded`
	/// learns the real one.
	pub fn loading(session: u64, identity: u64, path: String, theme: EditorTheme) -> Self {
		Self {
			session,
			identity,
			path,
			encoding: Encoding::UTF8_NO_BOM,
			content: text_editor::Content::new(),
			original: Vec::new(),
			status: EditorStatus::Loading(crate::viewer::LoadProgress::NOTHING_YET),
			flight: SaveFlight::Idle,
			notice: None,
			save_as: None,
			parent_gone: false,
			changed: Vec::new(),
			dirty: false,
			theme,
			scroll: 0.0,
			view_height: 0.0,
			scroll_x: 0.0,
			view_width: 0.0,
			content_cols: 0,
			find: None,
		}
	}

	/// Fill the buffer once the decoded text and its encoding arrive (§32). The freshly loaded text
	/// is the baseline, so nothing is marked changed and the editor is clean.
	///
	/// The buffer is built by `content_of`, not `Content::with_text`: this runs on the GUI thread, and
	/// `with_text` shaped the whole file eagerly — 98% of a four-second freeze at the ceiling (§121).
	pub fn set_loaded(&mut self, text: &str, encoding: Encoding) {
		self.content = content_of(text);
		self.encoding = encoding;
		self.original = lines_of(&self.content);
		self.status = EditorStatus::Ready;
		self.notice = None;
		self.recompute();
	}

	/// The load failed (too big, unreadable, unsupported): show the reason in place of the buffer.
	pub fn load_failed(&mut self, reason: String) {
		self.status = EditorStatus::Failed(reason);
	}

	/// Note how far the read has got (§121), for the tab strip's bar and the body's byte count.
	///
	/// Ignored unless the tab is still loading. A progress event can outlive the read it describes —
	/// the reader sends one per chunk and the terminal reply follows the last of them — so this must
	/// not be able to drag a `Ready` editor back into `Loading` and blank the buffer that just arrived.
	pub fn set_progress(&mut self, progress: crate::viewer::LoadProgress) {
		if matches!(self.status, EditorStatus::Loading(_)) {
			self.status = EditorStatus::Loading(progress);
		}
	}

	/// How far the read has got, while one is running (§121). `None` once the file is open or has
	/// failed, which is what stops the tab strip drawing a bar for a tab that is merely being edited.
	pub fn load_progress(&self) -> Option<crate::viewer::LoadProgress> {
		match self.status {
			EditorStatus::Loading(progress) => Some(progress),
			EditorStatus::Ready | EditorStatus::Failed(_) => None,
		}
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
	///
	/// Returns whether the TAB should now close itself — a "Save & close" waits on exactly this. The
	/// answer rides out of here rather than sitting in a flag for the caller to collect, because the
	/// two-call version had an order the caller could get wrong: clearing the flight first would have
	/// thrown the intent away before anyone read it.
	pub fn mark_saved(&mut self) -> bool {
		self.original = lines_of(&self.content);
		let closing = self.flight == SaveFlight::SavingToClose;
		self.flight = SaveFlight::Idle;
		self.notice = None;
		self.recompute();
		closing
	}

	/// A save failed: keep the buffer dirty and surface the reason without disturbing the edits.
	///
	/// Any pending close goes with the flight, so a FAILED "Save & close" keeps the tab and shows the
	/// error rather than closing over it. That used to be a separate call the caller had to remember.
	pub fn save_failed(&mut self, reason: String) {
		self.flight = SaveFlight::Idle;
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

	/// Note the buffer scrollable's offset and visible size on both axes (§32), reported by `on_scroll`
	/// on every scroll and on the first frame — the numbers the cursor-follow reads.
	pub fn set_viewport(
		&mut self,
		offset_x: f32,
		offset_y: f32,
		view_width: f32,
		view_height: f32,
	) {
		self.scroll_x = offset_x;
		self.scroll = offset_y;
		self.view_width = view_width;
		self.view_height = view_height;
	}

	/// Pre-seat the vertical offset after a cursor-follow scroll (§32), so a second keystroke arriving
	/// before the scrollable reports back still measures against the value we just asked for.
	pub fn set_scroll_y(&mut self, offset: f32) {
		self.scroll = offset;
	}

	/// Pre-seat the horizontal offset after a cursor-follow scroll (§32) — the mirror of `set_scroll_y`.
	pub fn set_scroll_x(&mut self, offset: f32) {
		self.scroll_x = offset;
	}

	/// The buffer scrollable's current vertical offset (§32).
	pub fn scroll(&self) -> f32 {
		self.scroll
	}

	/// The buffer scrollable's current horizontal offset (§32).
	pub fn scroll_x(&self) -> f32 {
		self.scroll_x
	}

	/// The buffer scrollable's visible height, `0.0` until the first frame reports it (§32). The
	/// cursor-follow skips while this is zero, so it never scrolls against an unmeasured viewport.
	pub fn view_height(&self) -> f32 {
		self.view_height
	}

	/// The buffer scrollable's visible width, `0.0` until the first frame reports it (§32) — the
	/// horizontal counterpart of `view_height`.
	pub fn view_width(&self) -> f32 {
		self.view_width
	}

	/// The buffer's widest line in display columns (§32) — the view scales it by the character advance
	/// to size the horizontal scroll extent.
	pub fn content_columns(&self) -> usize {
		self.content_cols
	}

	/// The cursor's horizontal position in display columns, tabs expanded (§32) — the mirror of
	/// `cursor_line`, read by the horizontal cursor-follow. Zero when the cursor's line cannot be read.
	pub fn cursor_display_column(&self) -> usize {
		let cursor = self.content.cursor().position;
		let Some(line) = self.content.lines().nth(cursor.line) else {
			return 0;
		};
		// `cursor.column` is a byte index on a char boundary within the line's text; slice up to it and
		// count the columns before it. A defensive `unwrap_or` guards a stale column past a shrunk line.
		let prefix = line.text.get(..cursor.column).unwrap_or(&line.text);
		display_columns(prefix)
	}

	/// The line the cursor sits on (§32) — what the cursor-follow scrolls onto screen. iced hides the
	/// widget's own scroll offset but exposes the cursor, so this line index plus the fixed line
	/// height is enough to place it in the outer scrollable.
	pub fn cursor_line(&self) -> usize {
		self.content.cursor().position.line
	}

	/// The line the current find match is on, or `None` when the bar is closed or its query has no
	/// matches (§32). The gutter and the buffer highlight this line so a match is visible even while
	/// the find FIELD, not the buffer, holds focus — iced paints the buffer's own selection only when
	/// the buffer is focused, so the selection alone would be invisible during a search.
	pub fn find_match_line(&self) -> Option<usize> {
		self.find.as_ref().and_then(Find::current_line)
	}

	/// The find matches on lines `first..last`, or nothing when the bar is closed (§138) — the hits the
	/// buffer draws an inverted block behind. Line-windowed because the view only ever draws what is on
	/// screen, exactly as the gutter does.
	pub fn find_spans_between(&self, first: usize, last: usize) -> &[EditorMatch] {
		self.find
			.as_ref()
			.map_or(&[][..], |find| find.spans_between(first, last))
	}

	/// The find bar's query, or `""` when the bar is closed (§138). The buffer hands this to the
	/// highlighter, which re-finds it line by line to recolour the matched glyphs.
	pub fn find_query(&self) -> &str {
		self.find.as_ref().map_or("", |find| find.query.as_str())
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
	/// bulk edit (a single Replace keeps undo). The buffer is reassembled with each line's OWN ending
	/// (the way iced's `Content::text` does), so a mixed-ending file's untouched lines are not
	/// normalised. Returns whether the buffer changed.
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
		// Rebuild the buffer from the matches the bar found — so what changes is exactly what was
		// highlighted — carrying each line's OWN ending. iced varies the ending per line and only a
		// bare `None` falls back to the default; joining with one uniform ending instead (the first
		// line's) would silently normalise every line of a mixed-ending file, not just the edited ones.
		let (lines, endings): (Vec<String>, Vec<text_editor::LineEnding>) = self
			.content
			.lines()
			.map(|line| (line.text.into_owned(), line.ending))
			.unzip();
		let joined = join_with_endings(
			&apply_replacements(&lines, &matches, &replacement),
			&endings,
		);
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
	/// byte position within a line, which is exactly what `EditorMatch` carries.
	fn select_match(&mut self, m: EditorMatch) {
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
		if !self.dirty
			|| self.is_saving()
			|| self.parent_gone
			|| !matches!(self.status, EditorStatus::Ready)
		{
			return false;
		}
		self.flight = SaveFlight::Saving;
		true
	}

	/// Whether a save is in flight (§32) — the toolbar disables Save and Save As while one is.
	pub fn is_saving(&self) -> bool {
		self.flight != SaveFlight::Idle
	}

	/// Begin a Save that closes the tab once it lands (§32) — the unsaved-changes prompt's "Save &
	/// close". Returns whether the caller should flush; `false` (nothing to save, or no channel)
	/// means the caller should just close the tab.
	pub fn begin_save_and_close(&mut self) -> bool {
		if self.begin_save() {
			self.flight = SaveFlight::SavingToClose;
			return true;
		}
		false
	}

	/// Open the Save As prompt, pre-filled with the current path (§32).
	pub fn begin_save_as(&mut self) {
		if self.parent_gone || !matches!(self.status, EditorStatus::Ready) {
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
		self.flight = SaveFlight::Saving;
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

	/// The buffer's first line, for shebang / mode-line grammar detection under the CME theme (§32);
	/// empty when the buffer is empty. It is read live, but the view folds the grammar it resolves into
	/// a stable token, so a file that already resolves by name or extension never re-highlights when its
	/// first line is edited — only a truly extensionless script re-resolves when its shebang changes.
	pub fn first_line(&self) -> String {
		self.content
			.lines()
			.next()
			.map(|line| line.text.into_owned())
			.unwrap_or_default()
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
		// The widest line drives the horizontal scroll extent (§32) — recomputed here so the extent
		// tracks an edit that lengthens or shortens the longest line. O(total chars) like the diff above,
		// paid only on an edit, never on the render path.
		self.content_cols = current
			.iter()
			.map(|line| display_columns(line))
			.max()
			.unwrap_or(0);
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

/// Cut `text` into pieces of at most `chunk` bytes for `content_of` (§121).
///
/// Deliberately NOT line-aligned. Aligning to the next newline reads tidier and would be free on
/// ordinary source, but a minified script is one line of megabytes — the exact file that would then
/// arrive as a single paste, which is the pathological case `PASTE_CHUNK` exists to avoid. Cutting
/// mid-line costs nothing: an insert is an insert, and the buffer is identical either way (the tests
/// below pin that against `with_text` for CRLF, no trailing newline, and one long line).
///
/// The walk back to a character boundary moves three bytes at worst, so no piece can come out empty
/// and no multi-byte character can be split across two pastes.
fn paste_chunks(text: &str, chunk: usize) -> Vec<&str> {
	let mut pieces = Vec::new();
	let mut at = 0;
	while at < text.len() {
		let mut end = (at + chunk).min(text.len());
		while !text.is_char_boundary(end) {
			end -= 1;
		}
		pieces.push(&text[at..end]);
		at = end;
	}
	pieces
}

/// Build the buffer for a freshly loaded file (§121) — chunked pastes rather than
/// `Content::with_text`, for the reasons measured on `PASTE_CHUNK`.
///
/// The cursor is put back to the start afterwards. The pastes walk it to the end of the buffer, and
/// `with_text` left it at line 0, so without this a file would open scrolled to its last line — a
/// plain regression, and the kind that only shows on a file long enough to scroll.
fn content_of(text: &str) -> text_editor::Content {
	let mut content = text_editor::Content::new();
	for piece in paste_chunks(text, PASTE_CHUNK) {
		content.perform(text_editor::Action::Edit(text_editor::Edit::Paste(
			std::sync::Arc::new(piece.to_owned()),
		)));
	}
	content.move_to(text_editor::Cursor {
		position: text_editor::Position { line: 0, column: 0 },
		selection: None,
	});
	content
}

/// The buffer's lines as owned strings, endings dropped (§32). The diff and the dirty check compare
/// text, not endings — iced never changes an ending on its own, so an ending shift is not an edit.
fn lines_of(content: &text_editor::Content) -> Vec<String> {
	content.lines().map(|line| line.text.into_owned()).collect()
}

/// Rejoin edited lines with each line's own ending, exactly as iced's `Content::text` reassembles a
/// buffer (§32): a line's ending is emitted only when another line follows it (so a buffer with no
/// trailing newline stays without one), and a bare `None` ending falls back to the default rather
/// than gluing two lines together. Used by Replace All so rebuilding the buffer keeps a mixed-ending
/// file's untouched lines as they were, rather than normalising them to one ending. `endings` is the
/// per-line ending beside `lines`, the two always the same length (both come from `Content::lines`).
fn join_with_endings(lines: &[String], endings: &[text_editor::LineEnding]) -> String {
	use text_editor::LineEnding;
	let mut out = String::new();
	let last = lines.len().saturating_sub(1);
	for (index, line) in lines.iter().enumerate() {
		out.push_str(line);
		if index != last {
			// A `None` ending between two lines is not "no separator" — iced writes the default there,
			// so mirror that; `unwrap_or_default` also covers a (never-hit) length mismatch as LF.
			let ending = match endings.get(index).copied().unwrap_or_default() {
				LineEnding::None => LineEnding::default(),
				ending => ending,
			};
			out.push_str(ending.as_str());
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every line of a buffer with its ending, so two buffers can be compared on the thing that
	/// matters — `lines_of` drops endings, and an ending shifted by a chunk boundary is exactly the
	/// bug worth catching.
	fn lines_and_endings(content: &text_editor::Content) -> Vec<(String, String)> {
		content
			.lines()
			.map(|line| (line.text.into_owned(), line.ending.as_str().to_owned()))
			.collect()
	}

	/// The chunked build has to produce the SAME buffer `with_text` produced, whatever the chunk
	/// boundary lands in the middle of (§121). Every case here is one that a naive split gets wrong:
	/// a boundary inside a CRLF pair, a file with no trailing newline, one line longer than a chunk,
	/// and multi-byte characters straddling the cut.
	#[test]
	fn a_chunked_build_is_the_buffer_with_text_would_have_made() {
		for (what, text) in [
			("empty", String::new()),
			("one line, no ending", "no trailing newline".to_owned()),
			("lf", "alpha\nbeta\ngamma\n".to_owned()),
			("crlf", "alpha\r\nbeta\r\ngamma\r\n".to_owned()),
			("mixed endings", "alpha\r\nbeta\ngamma\rdelta".to_owned()),
			("bare cr", "alpha\rbeta\r".to_owned()),
			// Longer than one chunk, so the cut really does land inside the content.
			("many lines", "0123456789\n".repeat(4096)),
			// One line of 40 KiB: five chunks with no newline to align to.
			("one long line", "x".repeat(40 * 1024)),
			// Multi-byte characters at an odd stride, so cuts land inside them.
			("multi-byte", "\u{1F600}\u{00E9}\u{4E2D}".repeat(4096)),
			("crlf, no trailing", "alpha\r\nbeta".to_owned()),
		] {
			// Arrange / Act
			let chunked = content_of(&text);
			let whole = text_editor::Content::with_text(&text);
			// Assert
			assert_eq!(
				lines_and_endings(&chunked),
				lines_and_endings(&whole),
				"chunked build differs from with_text for {what}"
			);
		}
	}

	/// The pastes leave the cursor at the end of the buffer, so `content_of` puts it back — otherwise
	/// a long file opens scrolled to its last line (§121).
	#[test]
	fn a_freshly_built_buffer_has_its_cursor_at_the_start() {
		// Arrange
		let text = "0123456789\n".repeat(4096);
		// Act
		let content = content_of(&text);
		// Assert
		let cursor = content.cursor();
		assert_eq!(
			(
				cursor.position.line,
				cursor.position.column,
				cursor.selection
			),
			(0, 0, None),
			"a loaded file must open at its first line, not its last"
		);
	}

	/// A chunk can never split a character, and never come out empty (§121) — the two ways the walk
	/// back to a boundary could go wrong.
	#[test]
	fn chunks_rejoin_into_the_text_they_came_from() {
		// Arrange — 4-byte, 3-byte and 2-byte characters, so a fixed stride lands inside all of them.
		let text = "\u{1F600}a\u{4E2D}b\u{00E9}c".repeat(500);
		// Act
		let pieces = paste_chunks(&text, 7);
		// Assert
		assert!(
			pieces.iter().all(|piece| !piece.is_empty()),
			"a chunk walked back past its own start"
		);
		assert!(pieces.len() > 1, "the test text must actually be split");
		assert_eq!(pieces.concat(), text, "the chunks lost or moved bytes");
	}

	#[test]
	fn utf8_without_bom_is_the_default() {
		// Arrange
		let bytes = b"hello\nworld";
		// Act
		let (text, encoding) = decode_text(bytes).expect("valid UTF-8 opens");
		// Assert
		assert_eq!(text, "hello\nworld");
		assert_eq!(encoding, Encoding::UTF8_NO_BOM);
	}

	#[test]
	fn a_utf8_bom_is_detected_and_stripped_but_remembered() {
		let mut bytes = BOM_UTF8.to_vec();
		bytes.extend_from_slice(b"hi");
		let (text, encoding) = decode_text(&bytes).expect("UTF-8 BOM opens");
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
		assert_eq!(decode_text(&le).unwrap().0, "AB");
		assert_eq!(decode_text(&le).unwrap().1.charset, Charset::Utf16Le);
		assert_eq!(decode_text(&be).unwrap().0, "AB");
		assert_eq!(decode_text(&be).unwrap().1.charset, Charset::Utf16Be);
	}

	#[test]
	fn binary_and_utf32_are_unsupported() {
		// An invalid UTF-8 byte with no BOM.
		assert!(decode_text(&[0xFF, 0x00, 0x9A]).is_none());
		// A UTF-32 LE BOM must not be mis-read as UTF-16.
		assert!(decode_text(&[0xFF, 0xFE, 0x00, 0x00, 0x41, 0x00, 0x00, 0x00]).is_none());
		// A UTF-32 BE BOM is not valid UTF-8 either.
		assert!(decode_text(&[0x00, 0x00, 0xFE, 0xFF]).is_none());
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
			let (back, detected) = decode_text(&bytes).expect("our own output decodes");
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
	fn display_columns_expands_tabs_to_the_next_stop() {
		// Plain ASCII is one column per char.
		assert_eq!(display_columns("hello"), 5);
		// A leading tab jumps to the first stop (8), then two more chars.
		assert_eq!(display_columns("\tab"), 10);
		// A tab after four chars advances to the next multiple of 8 (4 → 8), not by a full 8.
		assert_eq!(display_columns("abcd\tx"), 9);
		// Two tabs from column 0: 8 then 16.
		assert_eq!(display_columns("\t\t"), 16);
		// An empty line is zero columns wide.
		assert_eq!(display_columns(""), 0);
	}

	#[test]
	fn file_name_is_the_last_path_segment() {
		assert_eq!(file_name("/etc/app/config.json"), "config.json");
		assert_eq!(file_name("Makefile"), "Makefile");
		assert_eq!(file_name("/home/me/.bashrc"), ".bashrc");
		// A stray backslash in an otherwise POSIX path still splits.
		assert_eq!(file_name("/a\\weird"), "weird");
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
		// These lines are pure ASCII with no tabs, so each hit's display columns equal its byte offsets.
		assert_eq!(
			hits,
			vec![
				EditorMatch {
					line: 0,
					byte_start: 0,
					byte_end: 2,
					col_start: 0,
					col_end: 2,
				},
				EditorMatch {
					line: 0,
					byte_start: 13,
					byte_end: 15,
					col_start: 13,
					col_end: 15,
				},
				EditorMatch {
					line: 2,
					byte_start: 2,
					byte_end: 4,
					col_start: 2,
					col_end: 4,
				},
			]
		);
	}

	#[test]
	fn a_matched_span_reports_the_display_columns_a_tab_pushes_it_to() {
		// A leading tab is 8 columns wide, so the hit's bytes and its columns part company — and it is
		// the COLUMNS the buffer multiplies by the character advance to place the inverted block (§138).
		let hits = find_matches(&lines(&["\tneedle"]), "needle");
		assert_eq!(hits.len(), 1);
		assert_eq!((hits[0].byte_start, hits[0].byte_end), (1, 7));
		assert_eq!((hits[0].col_start, hits[0].col_end), (8, 14));
		// Two hits on one line, the second past a mid-line tab: 'a' at 0, tab to 8, 'a' at 8.
		let hits = find_matches(&lines(&["a\ta"]), "a");
		assert_eq!(
			hits.iter()
				.map(|m| (m.col_start, m.col_end))
				.collect::<Vec<_>>(),
			vec![(0, 1), (8, 9)]
		);
	}

	#[test]
	fn display_columns_at_resolves_every_offset_in_one_walk() {
		// Ascending offsets, tabs expanded, and an offset AT the end of the line lands on its width.
		assert_eq!(
			display_columns_at("ab\tcd", &[0, 2, 3, 5]),
			vec![0, 2, 8, 10]
		);
		// An offset past the end is clamped to the line's width rather than panicking.
		assert_eq!(display_columns_at("ab", &[99]), vec![2]);
		// Nothing asked for, nothing walked.
		assert!(display_columns_at("ab\tcd", &[]).is_empty());
	}

	#[test]
	fn spans_between_windows_the_matches_to_the_visible_lines() {
		// Arrange — one hit on each of lines 0, 1, 3 and 4.
		let find = Find {
			matches: find_matches(&lines(&["x", "x", "", "x", "x"]), "x"),
			..Default::default()
		};
		// Act / Assert — the window is half-open, exactly as `visible_lines` reports it.
		assert_eq!(
			find.spans_between(1, 4)
				.iter()
				.map(|m| m.line)
				.collect::<Vec<_>>(),
			vec![1, 3]
		);
		// A window with no hits in it is empty, not the whole list.
		assert!(find.spans_between(2, 3).is_empty());
		// And an empty or inverted window cannot panic on the slice.
		assert!(find.spans_between(2, 2).is_empty());
		assert!(find.spans_between(4, 1).is_empty());
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
	fn join_with_endings_keeps_each_lines_own_ending() {
		use text_editor::LineEnding;
		// A CRLF line then an LF line, no trailing newline: each ending kept, none added past the last —
		// so Replace All over a mixed-ending file does not normalise the lines it did not touch.
		let out = join_with_endings(&lines(&["a", "b"]), &[LineEnding::CrLf, LineEnding::Lf]);
		assert_eq!(out, "a\r\nb");
		// A `None` ending BETWEEN two lines is written as the default (LF), not glued, matching iced.
		let out = join_with_endings(&lines(&["a", "b"]), &[LineEnding::None, LineEnding::None]);
		assert_eq!(out, "a\nb");
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

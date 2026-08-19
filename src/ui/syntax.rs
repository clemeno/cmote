// ui/syntax.rs — syntax highlighting for the editor's CME theme (PLAN §32).
//
// The editor highlights only under the CME scheme (§32): a file reads much as it does in the user's
// VS Code, colours and all. This is an iced `Highlighter` — the trait `text_editor::highlight_with`
// drives — backed by `syntect`, the Sublime-grammar highlighting engine, with the syntax set from
// `two-face` (a big grammar pack, so TypeScript / PHP / TOML and the rest are covered).
//
// The design is iced's own `iced_highlighter` ported almost verbatim, with ONE change that is the
// whole point of doing it by hand: the theme. iced's built-in highlighter can only pick from a fixed
// set of bundled `.tmTheme`s; we instead build a `syntect::Theme` from the CME theme's own
// `tokenColors` (`cme_theme` below), so the scope colours are exactly the user's — comment `#aaaaaa`,
// string `#ffffbb`, keyword `#00ddff`, and so on. A scope the CME theme does not colour yields no
// modifier, so that token keeps the buffer's plain `value` colour.
//
// Incremental re-highlight (the caching in `change_line` / `highlight_line`) is iced's: a snapshot of
// the parser + scope stack is kept every `LINES_PER_SNAPSHOT` lines, so editing deep in a file
// re-parses from the nearest snapshot rather than from the top.

use std::ops::Range;
use std::str::FromStr;
use std::sync::LazyLock;

use iced::Color;
use iced::advanced::text::highlighter::{self, Format};
use iced::font::{self, Font};

// `syntect` is re-exported by `two-face`, so we speak to the exact version the syntax pack was built
// against — no chance of two `syntect`s in the tree.
use two_face::re_exports::syntect;

use syntect::highlighting::{
	self, Color as SynColor, ScopeSelectors, StyleModifier, Theme, ThemeItem,
};
use syntect::parsing::{ParseState, ScopeStack, ScopeStackOp, SyntaxReference, SyntaxSet};

/// How many lines between cached parser snapshots (§32). A larger value caches less and re-parses
/// more per edit; a smaller one caches more. iced's own value, and a sensible middle.
const LINES_PER_SNAPSHOT: usize = 50;

/// The big grammar set, built once. `extra_no_newlines` is the `bat`/two-face pack minus trailing
/// newlines, which is what a line-at-a-time highlighter wants (§32).
static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_no_newlines);

/// The CME colour scheme as a `syntect::Theme`, built once from the VS Code theme's `tokenColors`
/// (§32). `'static`, so a `syntect::Highlighter` can borrow it for the program's life.
static CME_THEME: LazyLock<Theme> = LazyLock::new(cme_theme);

/// What identifies a highlighter run to iced (§32): the NAME of the resolved grammar (`"Rust"`,
/// `"Makefile"`, `"Git Ignore"`, `"Plain Text"`). The view resolves the file's grammar once — widening
/// past the bare extension to whole names and shebangs (`resolve_syntax`) — and passes its name here,
/// so two states compare equal (and iced keeps the parser rather than rebuilding) exactly when the
/// effective grammar is the same. That is what keeps a normal file from re-highlighting when its first
/// line is edited: its name resolves by extension, so the shebang never enters the identity. The theme
/// is always CME, so it is not part of the identity; a change of grammar — a Save As to a new type, or
/// an edited shebang on an extensionless file — is what makes iced rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxSettings {
	/// The resolved grammar's name (`SyntaxReference::name`), from `resolve_syntax`.
	pub grammar: String,
}

/// Resolve the grammar for a file, widening past the bare extension so a whole-name file (`Makefile`,
/// `Dockerfile`, `.bashrc`, `.gitignore`, `CMakeLists.txt`) or an extensionless shebang script gets
/// highlighted instead of dropping to plain text (§32). First hit wins, most specific first: the whole
/// file name as a token — Sublime grammars register bare names like `Makefile` and `.gitignore` among
/// their "extensions", matched case-insensitively — then the extension alone, then the buffer's first
/// line as a shebang / mode-line, then plain text. `name` is the file's basename; `first_line` is the
/// buffer's first line (empty when there is none).
pub fn resolve_syntax(name: &str, first_line: &str) -> &'static SyntaxReference {
	by_whole_name(name)
		.or_else(|| by_extension(name))
		.or_else(|| by_first_line(first_line))
		.unwrap_or_else(|| SYNTAXES.find_syntax_plain_text())
}

/// The grammar registering this exact file name among its extensions (§32) — how a name-only file like
/// `Makefile` or a dot-file like `.bashrc` resolves. An empty name matches nothing.
fn by_whole_name(name: &str) -> Option<&'static SyntaxReference> {
	if name.is_empty() {
		return None;
	}
	SYNTAXES.find_syntax_by_extension(name)
}

/// The grammar for the file's extension (§32) — the ordinary case (`main.rs` → Rust). The extension is
/// the text after the LAST dot, and only when that dot is not the name's first character, so a dot-file
/// like `.bashrc` has no extension here (the whole-name step catches it instead).
fn by_extension(name: &str) -> Option<&'static SyntaxReference> {
	match name.rfind('.') {
		Some(dot) if dot > 0 => SYNTAXES.find_syntax_by_token(&name[dot + 1..]),
		_ => None,
	}
}

/// The grammar a first line's shebang or mode-line names (§32) — `#!/bin/sh` → bash. An empty line
/// matches nothing.
fn by_first_line(first_line: &str) -> Option<&'static SyntaxReference> {
	if first_line.is_empty() {
		return None;
	}
	SYNTAXES.find_syntax_by_first_line(first_line)
}

/// One highlighted span's style (§32): a `syntect` style modifier (a foreground colour, maybe a font
/// style). Wrapping it lets `to_format` turn it into what iced paints, exactly as iced's own does.
#[derive(Debug)]
pub struct SyntaxHighlight(StyleModifier);

impl SyntaxHighlight {
	/// The span's colour, or `None` to keep the buffer's plain colour — the CME theme leaves many
	/// scopes uncoloured, and those must not be forced to a colour (§32).
	fn color(&self) -> Option<Color> {
		self.0
			.foreground
			.map(|color| Color::from_rgba8(color.r, color.g, color.b, color.a as f32 / 255.0))
	}

	/// The span's font, for a bold / italic scope (§32). The CME token set is colour-only, so this is
	/// almost always `None` — but ported faithfully so a themed bold/italic would take effect.
	fn font(&self) -> Option<Font> {
		self.0.font_style.and_then(|style| {
			let bold = style.contains(highlighting::FontStyle::BOLD);
			let italic = style.contains(highlighting::FontStyle::ITALIC);
			if bold || italic {
				Some(Font {
					weight: if bold {
						font::Weight::Bold
					} else {
						font::Weight::Normal
					},
					style: if italic {
						font::Style::Italic
					} else {
						font::Style::Normal
					},
					..Font::MONOSPACE
				})
			} else {
				None
			}
		})
	}

	/// The span's colour and font together — what the editor view hands `highlight_with` (§32).
	pub fn to_format(&self) -> Format<Font> {
		Format {
			color: self.color(),
			font: self.font(),
		}
	}
}

/// The highlighter iced drives over the buffer (§32). Holds the resolved syntax, a `syntect`
/// highlighter bound to the CME theme, and the per-snapshot cache of parser + scope-stack states.
pub struct Highlighter {
	/// The grammar for this file's token, resolved once (falls back to plain text if unknown).
	syntax: &'static SyntaxReference,
	/// The colour engine, bound to the CME theme for the run. Named for its job rather than its type
	/// (`syntect`'s own `Highlighter`), so it does not read as this struct holding one of itself.
	colours: highlighting::Highlighter<'static>,
	/// Parser + scope-stack snapshots, one per `LINES_PER_SNAPSHOT`, for incremental re-highlight.
	caches: Vec<(ParseState, ScopeStack)>,
	/// The next line `highlight_line` will process.
	current_line: usize,
}

impl highlighter::Highlighter for Highlighter {
	type Settings = SyntaxSettings;
	type Highlight = SyntaxHighlight;

	type Iterator<'a> = Box<dyn Iterator<Item = (Range<usize>, Self::Highlight)> + 'a>;

	fn new(settings: &Self::Settings) -> Self {
		// The view already widened the file to a grammar (`resolve_syntax`) and handed us its name, so
		// here it is a direct lookup — plain text if the name is somehow unknown, never a panic.
		let syntax = SYNTAXES
			.find_syntax_by_name(&settings.grammar)
			.unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
		let colours = highlighting::Highlighter::new(&CME_THEME);
		let parser = ParseState::new(syntax);
		let stack = ScopeStack::new();
		Highlighter {
			syntax,
			colours,
			caches: vec![(parser, stack)],
			current_line: 0,
		}
	}

	fn update(&mut self, new_settings: &Self::Settings) {
		self.syntax = SYNTAXES
			.find_syntax_by_name(&new_settings.grammar)
			.unwrap_or_else(|| SYNTAXES.find_syntax_plain_text());
		self.colours = highlighting::Highlighter::new(&CME_THEME);
		// Restart from the top with the new grammar.
		self.change_line(0);
	}

	fn change_line(&mut self, line: usize) {
		let snapshot = line / LINES_PER_SNAPSHOT;
		if snapshot <= self.caches.len() {
			self.caches.truncate(snapshot);
			self.current_line = snapshot * LINES_PER_SNAPSHOT;
		} else {
			self.caches.truncate(1);
			self.current_line = 0;
		}

		let (parser, stack) = self
			.caches
			.last()
			.cloned()
			.unwrap_or_else(|| (ParseState::new(self.syntax), ScopeStack::new()));
		self.caches.push((parser, stack));
	}

	fn highlight_line(&mut self, line: &str) -> Self::Iterator<'_> {
		// Open a fresh snapshot whenever the line count crosses a `LINES_PER_SNAPSHOT` boundary.
		if self.current_line / LINES_PER_SNAPSHOT >= self.caches.len() {
			let (parser, stack) = self.caches.last().expect("caches must not be empty");
			self.caches.push((parser.clone(), stack.clone()));
		}
		self.current_line += 1;

		let (parser, stack) = self.caches.last_mut().expect("caches must not be empty");
		let ops = parser.parse_line(line, &SYNTAXES).unwrap_or_default();
		Box::new(scope_iterator(ops, line, stack, &self.colours))
	}

	fn current_line(&self) -> usize {
		self.current_line
	}
}

/// Walk one line's scope-stack operations into coloured ranges (§32). Each op advances the scope
/// stack; the style for the stack at that point becomes the range's modifier. iced's helper, ported.
fn scope_iterator<'a>(
	ops: Vec<(usize, ScopeStackOp)>,
	line: &str,
	stack: &'a mut ScopeStack,
	highlighter: &'a highlighting::Highlighter<'static>,
) -> impl Iterator<Item = (Range<usize>, SyntaxHighlight)> + 'a {
	ScopeRangeIterator {
		ops,
		line_length: line.len(),
		index: 0,
		last_str_index: 0,
	}
	.filter_map(move |(range, scope)| {
		let _ = stack.apply(&scope);
		if range.is_empty() {
			None
		} else {
			Some((
				range,
				SyntaxHighlight(highlighter.style_mod_for_stack(&stack.scopes)),
			))
		}
	})
}

/// Splits a line into `(byte range, the scope op that opens it)` pairs (§32). The op BEFORE an index
/// governs the range up to the next index; index 0 opens with a no-op (the inherited stack). iced's
/// helper, ported.
struct ScopeRangeIterator {
	ops: Vec<(usize, ScopeStackOp)>,
	line_length: usize,
	index: usize,
	last_str_index: usize,
}

impl Iterator for ScopeRangeIterator {
	type Item = (Range<usize>, ScopeStackOp);

	fn next(&mut self) -> Option<Self::Item> {
		if self.index > self.ops.len() {
			return None;
		}

		// The last range runs to the end of the line; the others to the next op's byte index.
		let next_str_i = if self.index == self.ops.len() {
			self.line_length
		} else {
			self.ops[self.index].0
		};

		let range = self.last_str_index..next_str_i;
		self.last_str_index = next_str_i;

		let op = if self.index == 0 {
			ScopeStackOp::Noop
		} else {
			self.ops[self.index - 1].1.clone()
		};

		self.index += 1;
		Some((range, op))
	}
}

/// Build the CME colour scheme as a `syntect::Theme` (§32), from the VS Code theme's own
/// `tokenColors` (the exact hexes the user's *Themer My Color Set Dark* uses). The default foreground
/// / background match `editor.foreground` / `editor.background`; each entry maps a TextMate scope to
/// its foreground. A scope not listed here yields no modifier, so its token keeps the plain colour.
//
// `Theme` is `#[non_exhaustive]`, so it cannot be built with a struct literal — it is defaulted and
// the fields we own are set after. The reassign-after-default lint has no better shape here, hence the
// allow on the whole function.
#[allow(clippy::field_reassign_with_default)]
fn cme_theme() -> Theme {
	// (scope selector, RGB) straight from the theme's tokenColors. syntect scores these by
	// specificity, so a more specific scope wins over a broader one regardless of order.
	//
	// Each colour is grouped per BYTE (§111) — `0x00_dd_ff` is r=00 g=dd b=ff, readable against the
	// theme's own hex. Clippy's default grouping of four digits would put green across two groups.
	let scopes: &[(&str, u32)] = &[
		("comment", 0xaa_aa_aa),
		("keyword", 0x00_dd_ff),
		("storage", 0x00_ff_dd),
		("constant", 0x00_ff_ff),
		("number", 0x00_ff_ff),
		("variable", 0xff_ff_ff),
		("entity", 0x00_dd_ff),
		("entity.name", 0x99_ff_ff),
		("entity.name.class", 0x44_ff_aa),
		("entity.other.attribute-name", 0x00_dd_ff),
		("support", 0x00_ff_dd),
		("invalid", 0x00_ff_ff),
		("string", 0xff_ff_bb),
		("string.quoted.double", 0xee_ee_00),
		("string.quoted.single", 0x00_ee_55),
		("string.quoted.double.json", 0xaa_ff_ff),
		("string.template.ts", 0xff_ff_aa),
		("string.template.ts.html.tag", 0xff_ff_00),
		("meta.template.expression.ts", 0xff_66_00),
		("meta.objectliteral", 0xff_ff_00),
		("meta.object-literal", 0xff_ff_00),
		("meta.block", 0xff_ff_ff),
		("meta.parameters", 0xff_ff_ff),
		("meta.brace", 0xff_ff_ff),
		(
			"meta.structure.dictionary.value.json string.quoted.double.json",
			0xff_ff_ff,
		),
		("markup.heading", 0x00_dd_ff),
		("markup.inserted", 0x00_ff_66),
		("markup.deleted", 0x00_ff_ff),
		("markup.changed", 0xaa_dd_ff),
		("markup.list", 0xff_ff_ff),
		("markup.raw", 0x00_ee_ff),
		("markup.underline.link", 0x00_cc_ff),
	];

	let mut theme = Theme::default();
	theme.name = Some("CME".to_owned());
	theme.settings.foreground = Some(syn_rgb(0xff_ff_ff));
	theme.settings.background = Some(syn_rgb(0x1a_2a_30));
	theme.scopes = scopes
		.iter()
		.map(|(scope, rgb)| theme_item(scope, *rgb))
		.collect();
	theme
}

/// One theme rule: a scope selector coloured with an opaque RGB (§32). An unparsable selector falls
/// back to matching nothing, so a typo cannot panic — it just does not colour.
fn theme_item(scope: &str, rgb: u32) -> ThemeItem {
	ThemeItem {
		scope: ScopeSelectors::from_str(scope).unwrap_or_default(),
		style: StyleModifier {
			foreground: Some(syn_rgb(rgb)),
			background: None,
			font_style: None,
		},
	}
}

/// A `0xRRGGBB` literal as an opaque `syntect` colour (§32).
fn syn_rgb(rgb: u32) -> SynColor {
	// One byte per channel, masked before it is narrowed: the mask is what makes each `try_from`
	// exact, so the conversion states the width instead of relying on the truncation to do it (§111).
	let channel = |shift: u32| u8::try_from((rgb >> shift) & 0xff).expect("one masked byte");
	SynColor {
		r: channel(16),
		g: channel(8),
		b: channel(0),
		a: 0xff,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_cme_theme_builds_with_its_scopes_and_ground_colours() {
		// Arrange / Act
		let theme = cme_theme();
		// Assert — the ground colours are the editor.* values, and every tokenColors rule made it in.
		assert_eq!(theme.settings.background, Some(syn_rgb(0x1a_2a_30)));
		assert_eq!(theme.settings.foreground, Some(syn_rgb(0xff_ff_ff)));
		assert_eq!(theme.scopes.len(), 32);
	}

	#[test]
	fn a_known_extension_resolves_to_a_real_grammar() {
		// Rust is well past syntect's own defaults, so this also proves the two-face pack loaded.
		let syntax = SYNTAXES.find_syntax_by_token("rs");
		assert_eq!(syntax.map(|s| s.name.as_str()), Some("Rust"));
	}

	#[test]
	fn resolve_widens_past_the_bare_extension() {
		// The ordinary case: a real extension resolves as before.
		assert_eq!(resolve_syntax("main.rs", "").name, "Rust");
		assert_eq!(resolve_syntax("data.json", "").name, "JSON");
		// Whole-name files with no usable extension now resolve instead of dropping to plain text.
		assert_eq!(resolve_syntax("Makefile", "").name, "Makefile");
		assert_eq!(resolve_syntax("Dockerfile", "").name, "Dockerfile");
		// A whole name that ends in a misleading extension still resolves by its name first: the CMake
		// grammar owns `CMakeLists.txt`, so it wins over the `.txt` extension's Plain Text.
		assert_eq!(resolve_syntax("CMakeLists.txt", "").name, "CMake");
		// A dot-file (no extension in our sense) resolves by its whole name.
		assert_eq!(resolve_syntax(".gitignore", "").name, "Git Ignore");
		assert!(resolve_syntax(".bashrc", "").name.contains("bash"));
	}

	#[test]
	fn resolve_uses_the_shebang_only_when_name_and_extension_miss() {
		// An extensionless script resolves by its first-line shebang.
		assert!(resolve_syntax("deploy", "#!/bin/sh").name.contains("bash"));
		assert_eq!(
			resolve_syntax("run", "#!/usr/bin/env python3").name,
			"Python"
		);
		// A file that already resolves by extension IGNORES the shebang — this is what keeps a normal
		// file's grammar (and so its highlighter identity) stable when line 0 is edited.
		assert_eq!(
			resolve_syntax("main.rs", "#!/usr/bin/env python3").name,
			"Rust"
		);
		// Nothing matches: plain text, never a panic.
		assert_eq!(resolve_syntax("mystery", "").name, "Plain Text");
	}
}

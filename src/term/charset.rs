// term/charset.rs — the character sets, and the shifts that invoke them (PLAN §143).
//
// A VT terminal does not have one character set. It has FOUR SLOTS — G0, G1, G2 and G3 — each
// holding a set that has been DESIGNATED into it, and two halves of the code space that each name a
// slot: GL, the left half (0x20–0x7f), and GR, the right half (0x80–0xff). A printed byte is read
// through whichever slot its half names.
//
//   ESC ( F    ESC ) F    ESC * F    ESC + F     SCS — designate set F into G0 / G1 / G2 / G3
//   SI  (0x0f)                                   LS0 — lock G0 into GL
//   SO  (0x0e)                                   LS1 — lock G1 into GL
//   ESC n      ESC o                             LS2 / LS3 — lock G2 / G3 into GL
//   ESC ~      ESC }      ESC |                  LS1R / LS2R / LS3R — lock G1 / G2 / G3 into GR
//   ESC N      ESC O                             SS2 / SS3 — invoke G2 / G3 for ONE character
//
// Before §143 cmote had the first line's `B` and `0` and the second line, and nothing else: the
// audit's own summary was that "G2 and G3 can be designated and never invoked" (§65). That is the
// hole this module fills, and filling it means taking the whole mechanism over from the engine.
//
// WHY TAKE IT OVER RATHER THAN ADD TO IT. `vte` knows exactly two sets — `StandardCharset::Ascii`
// and `StandardCharset::SpecialCharacterAndLineDrawing` (`ansi.rs:1202-1206`) — and its
// `esc_dispatch` matches `B` and `0` against the four slot intermediates and sends everything else
// to `unhandled!()` (`:1800-1812`). The engine holds the designations on its grid cursor and the GL
// invocation on the `Term` itself, and maps each printed character in `Term::input`
// (`term/mod.rs:985`). There is no seam to extend: a set the engine cannot name cannot be put in its
// slots, and there is no arm to invoke G2 with.
//
// So the state moves here whole, and the gate stops forwarding the two calls that would write the
// engine's copy. That is one writer, not two (§71, §73) — the engine's own slots stay ASCII for the
// life of the session and its `map` is the identity, and every substitution is made in
// `Gate::input` before the character reaches it. The DEC line-drawing set is NOT re-implemented on
// the way past: [`Charset::map`] calls the engine's own table for it, so the one set that already
// worked keeps working from the same source it always did.
//
// TWO DOORS, ONE STATE, AND WHICH IS WHICH. The rule is the one every module in this directory
// follows: the gate takes what `vte` dispatches, a scanner takes what `vte` drops.
//
//   * `ESC ( B`, `ESC ( 0` and their three slot spellings, plus SI and SO, reach
//     `Handler::configure_charset` and `Handler::set_active_charset`. Those are the GATE's, and they
//     write [`Charsets`] instead of the engine. Keeping them there is not tidiness: the soft reset
//     (§72) is SYNTHESISED and fed through `Terminal::advance`, which runs the parser and the gate
//     and no scanner at all — so `\E(B\E)B\E*B\E+B\017` in `SOFT_RESET` would reset nothing if the
//     gate stopped listening.
//   * Every other final byte, and all five locking shifts and both single shifts, reach nothing.
//     Those are [`Designations`]', found beside the stream and applied at the offset they sat, which
//     is what keeps a designation in front of the text it was written to govern.
//
// GR IS STATE, NOT SUBSTITUTION, AND THAT IS THE HONEST HALF. cmote decodes UTF-8 always (§67), so
// what arrives at `Gate::input` is a `char` and not a byte in a half. A stream that meant to put a
// character in the right half writes a byte past 0x7f, and `vte` reads that as UTF-8 — as a lead
// byte, a continuation, or as the replacement character when it is neither. Nothing can land in GR,
// so LS1R, LS2R and LS3R change a number that no printed character is ever read through. They are
// implemented anyway and the reason is DECCIR (§143): the cursor information report names the slot
// in GR as one of its ten fields, and a terminal that answered that field from a constant while
// silently ignoring the three sequences that set it would be reporting a state it refused to keep.
// LS2 and LS3, by contrast, lock into GL and are fully live — which is the half the audit row was
// actually about.
//
// WHICH SETS ARE IMPLEMENTED, AND WHY THE REST ARE REFUSED. The twelve national replacement sets
// plus JIS-Roman are here, from the tables cited on [`NATIONAL`]. Every one of them is a handful of
// substitutions inside ASCII, so a wrong entry is one wrong glyph and a reader can check the table
// against the source in a minute.
//
// The big sets are NOT here, and the refusal is deliberate rather than pending: DEC Supplemental
// (`<`, `%5`), DEC Technical (`>`), DEC Greek / Hebrew / Turkish / Cyrillic (`"?`, `"4`, `%0`,
// `&4`), the ISO Latin variants and JIS-Katakana (`I`). Each is a full 94-glyph table, several of
// them of symbols with no obvious Unicode counterpart, and none was read from a primary source here.
// A table of ninety-four glyphs written from memory is not a character set, it is ninety-four
// chances to put the wrong glyph on somebody's screen with nothing to notice it by. An unrecognised
// final leaves the slot exactly as it was, which is DEC's own behaviour for a set the terminal does
// not have — so a program that designates DEC Technical goes on getting ASCII, which is what it got
// before this module existed.

use alacritty_terminal::vte::ansi::StandardCharset;

/// How many slots a designation can go into: G0, G1, G2, G3.
pub const SLOTS: usize = 4;

/// The intermediate byte that names each slot, in slot order — `ESC ( F` is G0 and `ESC + F` is G3.
const SLOT_INTERMEDIATES: [u8; SLOTS] = *b"()*+";

/// The slot GL names at power-up, after RIS and after a soft reset: G0.
const DEFAULT_GL: usize = 0;

/// The slot GR names at the same three moments: G1.
///
/// Not a free choice and not a copy of anything, because the source does not settle it: DEC's own
/// DECSTR page lists "G0, G1, G2, G3, GL, GR" as reset to "Default settings" and never says what
/// those are. What narrows it to one answer is that **no sequence can put G0 in GR** — the locking
/// shifts that reach the right half are LS1R, LS2R and LS3R, and there is no LS0R — so G1 is the
/// lowest slot GR can ever name, and it is the pairing a VT220 powers up in (ASCII in GL, the
/// supplemental set in GR). cmote designates ASCII into all four slots, so the choice is visible
/// only through DECCIR's `Pgr` (§143); it is written down here rather than left to fall out of a
/// `Default` derive precisely because nothing else in the program can reveal it.
const DEFAULT_GR: usize = 1;

/// One national replacement character set: what it is called on the wire, and what it replaces.
///
/// A 94-character set covers 0x21–0x7e, and an NRCS differs from ASCII in at most a dozen of those
/// positions — the rest of the set IS ASCII. So the table holds only the differences, and
/// [`Charset::map`] passes everything else through untouched.
#[derive(Debug, PartialEq, Eq)]
pub struct National {
	/// The designation as it is written after the slot intermediate: the set's own intermediates, if
	/// it has any, followed by its final byte. `"A"` for the United Kingdom, `"%6"` for Portugal.
	///
	/// This is also exactly what DECCIR's `Sdesig` field reports (§143), which is why a set with two
	/// spellings has two entries in [`NATIONAL`] sharing one table of replacements rather than one
	/// entry with a list of names: the report has to say which spelling was used, so the spelling has
	/// to survive the designation.
	designation: &'static str,
	/// The positions this set replaces and what it puts there, ascending by position. Every entry is
	/// inside 0x21–0x7e, because that is the whole of a 94-character set.
	replacements: &'static [(u8, char)],
}

/// A character set that can be designated into a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Charset {
	/// ASCII — `B`. What every slot holds at power-up, and the set for which mapping is the identity.
	#[default]
	Ascii,
	/// DEC Special Character and Line Drawing — `0`. The set that draws box corners and lines, and
	/// the one cmote has always had, because the ENGINE has it: [`Charset::map`] calls its table
	/// rather than carrying a second copy.
	LineDrawing,
	/// One of the national replacement sets in [`NATIONAL`].
	National(&'static National),
}

impl Charset {
	/// Read one character through this set.
	///
	/// A character outside 0x21–0x7e comes back unchanged from every set here. That is not a
	/// shortcut: a 94-character set is DEFINED over those columns, so a set has nothing to say about
	/// a character already outside them, and a stream that sends UTF-8 through a designated NRCS gets
	/// its own glyphs back rather than a substitution made on the strength of a coincidence.
	pub fn map(self, character: char) -> char {
		match self {
			Self::Ascii => character,
			// The engine's own table, called rather than copied. It is the one set that worked before
			// this module existed, and the way to keep it working is to keep asking the code that was
			// answering — a transcription here would be a second table to keep in step with a crate
			// bump nobody would think to check.
			Self::LineDrawing => StandardCharset::SpecialCharacterAndLineDrawing.map(character),
			Self::National(set) => {
				let Ok(byte) = u8::try_from(u32::from(character)) else {
					return character;
				};
				set.replacements
					.iter()
					.find(|(position, _)| *position == byte)
					.map_or(character, |(_, replacement)| *replacement)
			}
		}
	}

	/// How DECCIR spells this set in its `Sdesig` field (§143) — the intermediates and final of the
	/// SCS that would designate it.
	pub fn designation(self) -> &'static str {
		match self {
			Self::Ascii => "B",
			Self::LineDrawing => "0",
			Self::National(set) => set.designation,
		}
	}
}

/// The national replacement sets, one entry per SCS spelling.
///
/// Sources, both read for this table rather than remembered: xterm's `ctlseqs` for which final bytes
/// select which set, and the position-by-position substitution tables of DEC's own national
/// replacement sets. Where the two disagreed the substitutions were checked a second time against
/// xterm's `charsets.h` — which is what settled Norwegian/Danish, a set that is documented in two
/// versions, a ten-position one and a six-position one. xterm carries a single table and it is the
/// ten-position one, so that is what is here.
///
/// A set with more than one final byte appears more than once, sharing its table. `C` and `5` are
/// both Finland; `` ` ``, `E` and `6` are all Norway/Denmark. They are separate entries because
/// DECCIR reports the spelling that was used and not a canonical one.
static NATIONAL: &[National] = &[
	National {
		designation: "A",
		replacements: &[(0x23, '£')],
	},
	National {
		designation: "4",
		replacements: DUTCH,
	},
	National {
		designation: "C",
		replacements: FINNISH,
	},
	National {
		designation: "5",
		replacements: FINNISH,
	},
	National {
		designation: "R",
		replacements: FRENCH,
	},
	National {
		designation: "f",
		replacements: FRENCH,
	},
	National {
		designation: "Q",
		replacements: FRENCH_CANADIAN,
	},
	National {
		designation: "9",
		replacements: FRENCH_CANADIAN,
	},
	National {
		designation: "K",
		replacements: GERMAN,
	},
	National {
		designation: "Y",
		replacements: ITALIAN,
	},
	National {
		designation: "`",
		replacements: NORWEGIAN_DANISH,
	},
	National {
		designation: "E",
		replacements: NORWEGIAN_DANISH,
	},
	National {
		designation: "6",
		replacements: NORWEGIAN_DANISH,
	},
	National {
		designation: "%6",
		replacements: PORTUGUESE,
	},
	National {
		designation: "Z",
		replacements: SPANISH,
	},
	National {
		designation: "H",
		replacements: SWEDISH,
	},
	National {
		designation: "7",
		replacements: SWEDISH,
	},
	National {
		designation: "=",
		replacements: SWISS,
	},
	National {
		designation: "J",
		replacements: JIS_ROMAN,
	},
];

/// Dutch — `ESC ( 4`. The odd one of the twelve: it puts a plain `|` at 0x5d, so one of its
/// replacements is an ASCII character standing in a different column.
static DUTCH: &[(u8, char)] = &[
	(0x23, '£'),
	(0x40, '¾'),
	(0x5b, 'ĳ'),
	(0x5c, '½'),
	(0x5d, '|'),
	(0x7b, '¨'),
	(0x7c, 'ƒ'),
	(0x7d, '¼'),
	(0x7e, '´'),
];

/// Finnish — `ESC ( C` or `ESC ( 5`.
static FINNISH: &[(u8, char)] = &[
	(0x5b, 'Ä'),
	(0x5c, 'Ö'),
	(0x5d, 'Å'),
	(0x5e, 'Ü'),
	(0x60, 'é'),
	(0x7b, 'ä'),
	(0x7c, 'ö'),
	(0x7d, 'å'),
	(0x7e, 'ü'),
];

/// French — `ESC ( R` or `ESC ( f`.
static FRENCH: &[(u8, char)] = &[
	(0x23, '£'),
	(0x40, 'à'),
	(0x5b, '°'),
	(0x5c, 'ç'),
	(0x5d, '§'),
	(0x7b, 'é'),
	(0x7c, 'ù'),
	(0x7d, 'è'),
	(0x7e, '¨'),
];

/// French Canadian — `ESC ( Q` or `ESC ( 9`.
static FRENCH_CANADIAN: &[(u8, char)] = &[
	(0x40, 'à'),
	(0x5b, 'â'),
	(0x5c, 'ç'),
	(0x5d, 'ê'),
	(0x5e, 'î'),
	(0x60, 'ô'),
	(0x7b, 'é'),
	(0x7c, 'ù'),
	(0x7d, 'è'),
	(0x7e, 'û'),
];

/// German — `ESC ( K`.
static GERMAN: &[(u8, char)] = &[
	(0x40, '§'),
	(0x5b, 'Ä'),
	(0x5c, 'Ö'),
	(0x5d, 'Ü'),
	(0x7b, 'ä'),
	(0x7c, 'ö'),
	(0x7d, 'ü'),
	(0x7e, 'ß'),
];

/// Italian — `ESC ( Y`.
static ITALIAN: &[(u8, char)] = &[
	(0x23, '£'),
	(0x40, '§'),
	(0x5b, '°'),
	(0x5c, 'ç'),
	(0x5d, 'é'),
	(0x60, 'ù'),
	(0x7b, 'à'),
	(0x7c, 'ò'),
	(0x7d, 'è'),
	(0x7e, 'ì'),
];

/// Norwegian/Danish — `` ESC ( ` ``, `ESC ( E` or `ESC ( 6`. The ten-position table; see [`NATIONAL`]
/// for why this one and not the six-position variant that is also documented.
static NORWEGIAN_DANISH: &[(u8, char)] = &[
	(0x40, 'Ä'),
	(0x5b, 'Æ'),
	(0x5c, 'Ø'),
	(0x5d, 'Å'),
	(0x5e, 'Ü'),
	(0x60, 'ä'),
	(0x7b, 'æ'),
	(0x7c, 'ø'),
	(0x7d, 'å'),
	(0x7e, 'ü'),
];

/// Portuguese — `ESC ( % 6`. One of the two sets here with a multi-byte designation.
static PORTUGUESE: &[(u8, char)] = &[
	(0x5b, 'Ã'),
	(0x5c, 'Ç'),
	(0x5d, 'Õ'),
	(0x7b, 'ã'),
	(0x7c, 'ç'),
	(0x7d, 'õ'),
];

/// Spanish — `ESC ( Z`.
static SPANISH: &[(u8, char)] = &[
	(0x23, '£'),
	(0x40, '§'),
	(0x5b, '¡'),
	(0x5c, 'Ñ'),
	(0x5d, '¿'),
	(0x7b, '°'),
	(0x7c, 'ñ'),
	(0x7d, 'ç'),
];

/// Swedish — `ESC ( H` or `ESC ( 7`.
static SWEDISH: &[(u8, char)] = &[
	(0x40, 'É'),
	(0x5b, 'Ä'),
	(0x5c, 'Ö'),
	(0x5d, 'Å'),
	(0x5e, 'Ü'),
	(0x60, 'é'),
	(0x7b, 'ä'),
	(0x7c, 'ö'),
	(0x7d, 'å'),
	(0x7e, 'ü'),
];

/// Swiss — `ESC ( =`. The widest of the twelve, and the only one that replaces 0x5f.
static SWISS: &[(u8, char)] = &[
	(0x23, 'ù'),
	(0x40, 'à'),
	(0x5b, 'é'),
	(0x5c, 'ç'),
	(0x5d, 'ê'),
	(0x5e, 'î'),
	(0x5f, 'è'),
	(0x60, 'ô'),
	(0x7b, 'ä'),
	(0x7c, 'ö'),
	(0x7d, 'ü'),
	(0x7e, 'û'),
];

/// JIS-Roman — `ESC ( J`. Two positions, and it is here rather than with the refused sets for that
/// reason: it is an NRCS in everything but name, not a 94-glyph table written from memory.
static JIS_ROMAN: &[(u8, char)] = &[(0x5c, '¥'), (0x7e, '‾')];

/// One screen's worth of character-set state: what is in each slot, and which slot each half names.
///
/// Its own type because there are TWO of them — see [`Charsets`] for the alternate screen — and
/// because the whole of it is what DECSC saves and DECRC puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bank {
	/// What is designated into G0, G1, G2 and G3.
	designations: [Charset; SLOTS],
	/// The slot GL names, written by SI, SO, LS2 and LS3.
	gl: usize,
	/// The slot GR names, written by LS1R, LS2R and LS3R — and read only by DECCIR, for the reason
	/// the module header gives.
	gr: usize,
	/// The slot a single shift owes the next character, if one is pending. SS2 puts a 2 here and SS3
	/// a 3; the next character through [`Charsets::map`] takes it away again.
	single_shift: Option<usize>,
}

impl Default for Bank {
	fn default() -> Self {
		Self {
			designations: [Charset::Ascii; SLOTS],
			gl: DEFAULT_GL,
			gr: DEFAULT_GR,
			single_shift: None,
		}
	}
}

/// The character-set state, per screen, with what DECSC last saved on each (§143).
///
/// TWO BANKS, ONE PER SCREEN, because that is what the arrangement being replaced did. The engine
/// keeps its designations on the grid cursor (`term/mod.rs:2192`) and swaps whole grids on the
/// alternate-screen swap, so a full-screen program that designates line drawing has always got its
/// designations back to itself when it left. A single global table here would have been a
/// REGRESSION and a visible one: a program that left G1 on line drawing under SO would hand the
/// shell a screen of box corners.
///
/// The saved slot is per bank too, for the same reason: the engine's `saved_cursor` lives on the
/// grid and is swapped along with it, so a DECSC on one screen has never been restorable on the
/// other.
#[derive(Debug, Default)]
pub struct Charsets {
	banks: [Bank; 2],
	/// What DECSC last saved on each screen, or nothing if it has not been used there.
	saved: [Option<Bank>; 2],
	/// Whether the ALTERNATE screen is the one up, which is the index into both arrays above. Written
	/// by the gate from the engine's own mode flag rather than tracked from the sequences, so every
	/// spelling of the swap — 47, 1047, 1049, and whatever a later engine adds — moves it.
	alternate: bool,
}

impl Charsets {
	/// Read one character through the half and slot in force, taking any single shift with it.
	///
	/// The fast path is a pair of reads and no write, which matters because this is on the printing
	/// path: every glyph of every screenful goes through it. A stream that has designated nothing is
	/// reading ASCII through G0 with nothing pending, and comes straight back out.
	///
	/// The single shift is consumed by ANY character, including a zero-width combining one. DEC
	/// defines it as lasting for the next graphic character and a combining mark is one, so there is
	/// no case here where taking it would be wrong — and a shift that survived a character would be a
	/// shift that survived the wrong character.
	pub fn map(&mut self, character: char) -> char {
		let bank = &mut self.banks[usize::from(self.alternate)];
		if bank.single_shift.is_none() && bank.designations[bank.gl] == Charset::Ascii {
			return character;
		}
		let slot = bank.single_shift.take().unwrap_or(bank.gl);
		bank.designations[slot].map(character)
	}

	/// SCS — put `charset` in `slot`. Out-of-range slots cannot arrive: both callers derive theirs
	/// from a fixed four-wide table.
	pub fn designate(&mut self, slot: usize, charset: Charset) {
		self.bank_mut().designations[slot] = charset;
	}

	/// A locking shift — SI, SO, LS2 and LS3 into GL; LS1R, LS2R and LS3R into GR.
	pub fn lock(&mut self, slot: usize, right: bool) {
		let bank = self.bank_mut();
		if right {
			bank.gr = slot;
		} else {
			bank.gl = slot;
		}
	}

	/// SS2 or SS3 — owe the next character a read through `slot`.
	pub fn single_shift(&mut self, slot: usize) {
		self.bank_mut().single_shift = Some(slot);
	}

	/// DECSC. The whole bank travels with the cursor, which is DEC's own definition of the item: a
	/// saved cursor carries the character sets, and it is why `ESC 7` / `ESC 8` around a line-drawing
	/// run puts the sets back as well as the position.
	pub fn save(&mut self) {
		self.saved[usize::from(self.alternate)] = Some(self.banks[usize::from(self.alternate)]);
	}

	/// DECRC. A restore with nothing saved leaves the sets alone rather than guessing at a default —
	/// the same rule XTRESTORE keeps for a mode that was never saved (§141), and for the same reason:
	/// inventing "back to ASCII" would let an unpaired `ESC 8` undo a designation the program still
	/// wanted.
	pub fn restore(&mut self) {
		if let Some(bank) = self.saved[usize::from(self.alternate)] {
			self.banks[usize::from(self.alternate)] = bank;
		}
	}

	/// RIS and the soft reset: both banks, both saved slots, back to ASCII in every slot with GL on
	/// G0 and GR on G1.
	///
	/// BOTH banks, because RIS is the hard reset and leaves nothing behind — and because the engine
	/// does the same to its own copy (`Term::reset_state` rebuilds the grids). The soft reset shares
	/// this for the practical reason that DECSTR's published list names "G0, G1, G2, G3, GL, GR"
	/// without saying what the default is, so there is one default here and one place it is written.
	pub fn reset(&mut self) {
		*self = Self {
			alternate: self.alternate,
			..Self::default()
		};
	}

	/// DECSTR — the screen that is up, back to the same defaults, and the other one left alone.
	///
	/// Narrower than [`Charsets::reset`] on purpose, and the split is the difference between the two
	/// resets everywhere else in this program: RIS is the hard one and clears the terminal, DECSTR is
	/// the soft one and puts the CURRENT presentation back. DEC's own DECSTR list names "G0, G1, G2,
	/// G3, GL, GR" as reset to "Default settings" and says nothing about a second page, which is
	/// consistent with a terminal that has none.
	///
	/// The saved slot is untouched here because the soft-reset string re-saves it: it ends in `ESC 7`,
	/// which since §143 carries the character sets, so the save lands on the bank this call just put
	/// back.
	pub fn soft_reset(&mut self) {
		*self.bank_mut() = Bank::default();
	}

	/// Follow the engine onto or off the alternate screen, which selects the other bank.
	pub fn set_alternate(&mut self, alternate: bool) {
		self.alternate = alternate;
	}

	/// What is designated in `slot`, for DECCIR's `Sdesig` (§143).
	pub fn designated(&self, slot: usize) -> Charset {
		self.banks[usize::from(self.alternate)].designations[slot]
	}

	/// The slot GL names, for DECCIR's `Pgl`.
	pub fn gl(&self) -> usize {
		self.banks[usize::from(self.alternate)].gl
	}

	/// The slot GR names, for DECCIR's `Pgr`.
	pub fn gr(&self) -> usize {
		self.banks[usize::from(self.alternate)].gr
	}

	/// The slot a single shift is owed to, for DECCIR's `Sflag` — which has a bit for SS2 and one for
	/// SS3 and so has to be able to tell them apart.
	pub fn pending_single_shift(&self) -> Option<usize> {
		self.banks[usize::from(self.alternate)].single_shift
	}

	/// The bank of whichever screen is up.
	fn bank_mut(&mut self) -> &mut Bank {
		&mut self.banks[usize::from(self.alternate)]
	}
}

/// One thing a charset sequence asks for, found beside the stream and applied where it sat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharsetRequest {
	/// SCS — designate a set into a slot.
	Designate { slot: usize, charset: Charset },
	/// A locking shift. `right` is GR, which is LS1R / LS2R / LS3R.
	Lock { slot: usize, right: bool },
	/// SS2 or SS3 — the slot the next character alone is read through.
	SingleShift(usize),
}

/// The scanner for the charset sequences `vte` drops (§143).
///
/// The escape-sequence grammar is [`super::dcs::Framer`]'s (§111); what is left here is deciding
/// what a finished sequence MEANS — which slot, which set, which half.
///
/// The cap is zero: this scanner reads no control string, so no payload is buffered on its account.
#[derive(Debug, Default)]
pub struct Designations {
	escapes: super::dcs::Framer<0>,
}

impl Designations {
	/// Scan a chunk of shell output, returning what each sequence asks for and where it sat. Safe at
	/// any chunk boundary — the state machine carries over between calls, so a sequence may be split
	/// anywhere, including between the ESC and its intermediate.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the tab-stop reset (§74) and the
	/// rectangular operations (§58). What matters here is that a request is applied WHERE IT SAT and
	/// not at the end of the chunk: a designation says how the characters after it are to be read, so
	/// one applied a chunk late would map the very text it was written for through the set it
	/// replaced, and one applied a chunk early would map the text in front of it.
	///
	/// **Which SIDE of the sequence is arbitrary, and it is worth saying so rather than inventing a
	/// reason.** For every other scanner here the side is a real decision — `graphics` needs the ESC
	/// because an erase past it destroys what the question is about (§41), and eight others need one
	/// past because the engine is about to dispatch something. Neither applies to these bytes: the
	/// engine has no arm for any of them, and the sequences themselves print nothing, so `past` and
	/// `start` produce identical grids. It is one past because that is what the scanners around it do,
	/// and a lone exception would read as a decision somebody made.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, CharsetRequest)> {
		let mut requests = Vec::new();
		self.escapes.feed(bytes, |span, control| {
			if let super::dcs::Control::Escape(escape) = control
				&& let Some(request) = classify(escape.intermediates(), escape.final_byte())
			{
				requests.push((span.past(), request));
			}
		});
		requests
	}
}

/// What a finished escape sequence asks for, or `None` if it is not this module's.
///
/// The near misses this keeps out, in the order they are easy to get wrong:
///
///   * `ESC # 3` .. `ESC # 6` are the double-height and double-width lines and `ESC # 8` is DECALN.
///     They wear an intermediate that is not a slot, which is what the slot lookup tests.
///   * `ESC SP F` and `ESC SP G` are the 7- and 8-bit control output switches, and `ESC % G` selects
///     UTF-8 — three more sequences whose intermediate is not a slot.
///   * `ESC 6` and `ESC 9` are DECBI and DECFI (§112), and `ESC 7` is DECSC. All three carry NO
///     intermediate, where `ESC ( 6` is Norway/Denmark — so the empty-intermediate arm must not read
///     a designation's final byte, and it does not: it matches only the seven shift bytes.
///   * `ESC ( B` and `ESC ( 0` are refused here on purpose. `vte` dispatches those two to the gate,
///     and answering them in both places would be one instruction with two writers.
fn classify(intermediates: &[u8], final_byte: u8) -> Option<CharsetRequest> {
	match intermediates {
		[] => shift(final_byte),
		[slot, tail @ ..] => {
			let slot = SLOT_INTERMEDIATES.iter().position(|byte| byte == slot)?;
			Some(CharsetRequest::Designate {
				slot,
				charset: designate(tail, final_byte)?,
			})
		}
	}
}

/// The set a designation names, or `None` for one cmote does not have.
///
/// An unknown final leaves the slot alone, which is what a terminal without the set does — and it is
/// the same answer for a set that is refused (DEC Technical) as for a byte that designates nothing
/// at all, because from the far side of the wire those two are the same fact.
fn designate(tail: &[u8], final_byte: u8) -> Option<Charset> {
	// `B` and `0` with no intermediates of their own belong to the gate; see [`classify`].
	if tail.is_empty() && matches!(final_byte, b'B' | b'0') {
		return None;
	}
	NATIONAL
		.iter()
		.find(|set| {
			let bytes = set.designation.as_bytes();
			bytes.split_last() == Some((&final_byte, tail))
		})
		.map(Charset::National)
}

/// The shift a bare escape sequence asks for, or `None` if the final byte is somebody else's.
fn shift(final_byte: u8) -> Option<CharsetRequest> {
	match final_byte {
		// SS2 and SS3 — one character each, and the only two sequences here that do not last.
		b'N' => Some(CharsetRequest::SingleShift(2)),
		b'O' => Some(CharsetRequest::SingleShift(3)),
		// LS2 and LS3, into GL. SI and SO are the other two of this family and are the gate's, being
		// the two `vte` dispatches.
		b'n' => Some(CharsetRequest::Lock {
			slot: 2,
			right: false,
		}),
		b'o' => Some(CharsetRequest::Lock {
			slot: 3,
			right: false,
		}),
		// LS1R, LS2R and LS3R, into GR — where nothing can be read, for the reason the module header
		// gives. There is no LS0R, which is why GR can never name G0.
		b'~' => Some(CharsetRequest::Lock {
			slot: 1,
			right: true,
		}),
		b'}' => Some(CharsetRequest::Lock {
			slot: 2,
			right: true,
		}),
		b'|' => Some(CharsetRequest::Lock {
			slot: 3,
			right: true,
		}),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go.
	fn scan(bytes: &[u8]) -> Vec<(usize, CharsetRequest)> {
		Designations::default().feed(bytes)
	}

	/// Just what each sequence asked for, for the tests that are not about offsets.
	fn requests(bytes: &[u8]) -> Vec<CharsetRequest> {
		scan(bytes)
			.into_iter()
			.map(|(_, request)| request)
			.collect()
	}

	/// The set a designation lands on, or `None` where the scanner reported nothing.
	fn designated(bytes: &[u8]) -> Option<Charset> {
		match requests(bytes).first() {
			Some(CharsetRequest::Designate { charset, .. }) => Some(*charset),
			_ => None,
		}
	}

	#[test]
	fn a_designation_is_found_just_past_its_final_byte() {
		assert_eq!(
			scan(b"\x1b(A"),
			vec![(
				3,
				CharsetRequest::Designate {
					slot: 0,
					charset: designated(b"\x1b(A").unwrap(),
				}
			)]
		);
		// A two-byte final is three bytes past the ESC, and the offset counts the whole sequence.
		assert_eq!(scan(b"ab\x1b(%6cd")[0].0, 6);
	}

	#[test]
	fn each_intermediate_names_its_own_slot() {
		let slots: Vec<usize> = [&b"\x1b(A"[..], b"\x1b)A", b"\x1b*A", b"\x1b+A"]
			.iter()
			.map(|bytes| match requests(bytes)[0] {
				CharsetRequest::Designate { slot, .. } => slot,
				other => panic!("not a designation: {other:?}"),
			})
			.collect();
		assert_eq!(slots, vec![0, 1, 2, 3]);
	}

	/// The two sets `vte` itself dispatches are deliberately NOT claimed here: the gate answers them,
	/// and a second writer of one instruction is exactly what this split exists to avoid.
	#[test]
	fn ascii_and_line_drawing_are_left_to_the_gate() {
		assert!(scan(b"\x1b(B").is_empty());
		assert!(scan(b"\x1b(0").is_empty());
		assert!(scan(b"\x1b+B").is_empty());
		assert!(scan(b"\x1b+0").is_empty());
	}

	/// An unrecognised final leaves the slot alone. DEC Technical and DEC Supplemental are the two
	/// worth naming, because they are refusals rather than oversights (see the module header).
	#[test]
	fn a_set_cmote_does_not_have_designates_nothing() {
		for designation in [
			&b"\x1b(>"[..], // DEC Technical
			b"\x1b(<",      // DEC Supplemental
			b"\x1b(%5",     // DEC Supplemental Graphics
			b"\x1b(\"?",    // DEC Greek
			b"\x1b(&4",     // DEC Cyrillic
			b"\x1b(I",      // JIS-Katakana
			b"\x1b(\x7e",   // not a designation at all
		] {
			assert!(
				scan(designation).is_empty(),
				"{designation:?} must leave the slot as it was"
			);
		}
	}

	/// The near miss this module is built around: three families share the escape door and two of
	/// them wear an intermediate that is one byte away from a slot's.
	#[test]
	fn the_other_escape_families_are_not_designations() {
		for sequence in [
			&b"\x1b#3"[..], // DECDHL top
			b"\x1b#6",      // DECDWL
			b"\x1b#8",      // DECALN
			b"\x1b F",      // S7C1T
			b"\x1b G",      // S8C1T
			b"\x1b%G",      // select UTF-8
			b"\x1bc",       // RIS
			b"\x1b7",       // DECSC
			b"\x1b6",       // DECBI — and `ESC ( 6` IS Norway/Denmark, one intermediate apart
			b"\x1b9",       // DECFI
		] {
			assert!(
				scan(sequence).is_empty(),
				"{sequence:?} is not a charset sequence"
			);
		}
		// The same final byte WITH a slot intermediate is a designation, so the loop above is testing
		// the intermediate rather than rejecting everything.
		assert_eq!(requests(b"\x1b(6").len(), 1);
	}

	#[test]
	fn the_locking_shifts_name_their_slot_and_their_half() {
		assert_eq!(
			requests(b"\x1bn\x1bo\x1b~\x1b}\x1b|"),
			vec![
				CharsetRequest::Lock {
					slot: 2,
					right: false
				},
				CharsetRequest::Lock {
					slot: 3,
					right: false
				},
				CharsetRequest::Lock {
					slot: 1,
					right: true
				},
				CharsetRequest::Lock {
					slot: 2,
					right: true
				},
				CharsetRequest::Lock {
					slot: 3,
					right: true
				},
			]
		);
	}

	#[test]
	fn the_single_shifts_name_g2_and_g3() {
		assert_eq!(
			requests(b"\x1bN\x1bO"),
			vec![
				CharsetRequest::SingleShift(2),
				CharsetRequest::SingleShift(3)
			]
		);
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to
	/// carry across a boundary drawn anywhere — including inside a two-byte designation.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scanner = Designations::default();
		assert!(scanner.feed(b"\x1b").is_empty());
		assert!(scanner.feed(b"(").is_empty());
		assert!(scanner.feed(b"%").is_empty());
		// The offset is into THIS chunk, which is where the interruption advance uses it.
		assert_eq!(scanner.feed(b"6")[0].0, 1);
	}

	/// `ESC` then a C0 stays in the escape state for the engine, so a designation with a line feed in
	/// the middle of it still designates — the rule four hand-rolled watchers got wrong before the
	/// shared grammar existed (§111).
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert_eq!(requests(b"\x1b\n(A").len(), 1, "LF is read through");
		assert!(scan(b"\x1b\x18(A").is_empty(), "CAN cancels");
	}

	/// Every table is inside the 94 columns a 94-character set is defined over, and ascending. The
	/// second half is not decoration: [`Charset::map`] walks the table, and a duplicate position
	/// would make the first entry win silently.
	#[test]
	fn every_national_table_is_ordered_and_inside_the_ninety_four_columns() {
		for set in NATIONAL {
			let mut previous = 0x20;
			for (position, _) in set.replacements {
				assert!(
					(0x21..=0x7e).contains(position),
					"{} replaces {position:#04x}, outside a 94-character set",
					set.designation
				);
				assert!(
					*position > previous,
					"{} is out of order at {position:#04x}",
					set.designation
				);
				previous = *position;
			}
		}
	}

	/// The two sets with more than one spelling share one table, and each spelling reports itself.
	/// DECCIR names the designation that was used (§143), so the spellings cannot be canonicalised.
	#[test]
	fn a_set_with_two_spellings_reports_the_one_that_was_written() {
		let finnish = [
			designated(b"\x1b(C").unwrap(),
			designated(b"\x1b(5").unwrap(),
		];
		assert_eq!(finnish[0].designation(), "C");
		assert_eq!(finnish[1].designation(), "5");
		assert_eq!(
			finnish[0].map('['),
			finnish[1].map('['),
			"one set, two names"
		);
	}

	#[test]
	fn a_national_set_replaces_its_own_positions_and_nothing_else() {
		let german = designated(b"\x1b(K").unwrap();
		assert_eq!(german.map('['), 'Ä');
		assert_eq!(german.map('~'), 'ß');
		assert_eq!(german.map('a'), 'a', "outside the table");
		assert_eq!(german.map('é'), 'é', "outside the set entirely");
	}

	/// The one set that was already working goes on working from the engine's own table rather than
	/// a copy of it — which is what this asserts by comparing against that table directly.
	#[test]
	fn line_drawing_is_still_the_engines_own_table() {
		for character in ['q', 'x', 'l', 'j', '`', '_'] {
			assert_eq!(
				Charset::LineDrawing.map(character),
				StandardCharset::SpecialCharacterAndLineDrawing.map(character)
			);
		}
		assert_eq!(Charset::LineDrawing.map('q'), '─', "and it really maps");
	}

	#[test]
	fn ascii_maps_to_itself() {
		for character in ['a', '[', '#', '~', 'é', '漢'] {
			assert_eq!(Charset::Ascii.map(character), character);
		}
	}

	#[test]
	fn a_designation_is_read_through_the_half_that_names_it() {
		let mut charsets = Charsets::default();
		assert_eq!(charsets.map('['), '[', "ASCII everywhere at power-up");
		charsets.designate(1, designated(b"\x1b)K").unwrap());
		assert_eq!(charsets.map('['), '[', "G1 is designated, not invoked");
		charsets.lock(1, false);
		assert_eq!(charsets.map('['), 'Ä', "SO puts G1 in GL");
		charsets.lock(0, false);
		assert_eq!(charsets.map('['), '[', "SI puts G0 back");
	}

	/// The audit row this module answers: G2 and G3 could be designated and never invoked (§65).
	#[test]
	fn g2_and_g3_can_now_be_invoked() {
		let mut charsets = Charsets::default();
		charsets.designate(2, designated(b"\x1b*K").unwrap());
		charsets.designate(3, designated(b"\x1b+A").unwrap());
		charsets.lock(2, false);
		assert_eq!(charsets.map('['), 'Ä', "LS2");
		charsets.lock(3, false);
		assert_eq!(charsets.map('#'), '£', "LS3");
	}

	#[test]
	fn a_single_shift_lasts_exactly_one_character() {
		let mut charsets = Charsets::default();
		charsets.designate(2, designated(b"\x1b*K").unwrap());
		charsets.single_shift(2);
		assert_eq!(charsets.map('['), 'Ä');
		assert_eq!(charsets.map('['), '[', "and GL is back in force");
	}

	/// GR is written and never read, which is the module header's honest half — so what this pins is
	/// that a right-half lock does NOT change what a printed character maps to.
	#[test]
	fn a_right_half_lock_changes_no_glyph() {
		let mut charsets = Charsets::default();
		charsets.designate(1, designated(b"\x1b)K").unwrap());
		charsets.lock(1, true);
		assert_eq!(charsets.gr(), 1, "the state is kept, for DECCIR");
		assert_eq!(charsets.map('['), '[', "and nothing is read through it");
	}

	#[test]
	fn a_saved_bank_comes_back_whole() {
		let mut charsets = Charsets::default();
		charsets.designate(1, designated(b"\x1b)K").unwrap());
		charsets.lock(1, false);
		charsets.save();
		charsets.designate(1, Charset::Ascii);
		charsets.lock(0, false);
		assert_eq!(charsets.map('['), '[');
		charsets.restore();
		assert_eq!(charsets.map('['), 'Ä', "the designation and the lock both");
	}

	/// A restore with nothing saved leaves the sets where they are rather than guessing at a default
	/// — the same rule XTRESTORE keeps for a mode that was never saved (§141).
	#[test]
	fn a_restore_with_nothing_saved_changes_nothing() {
		let mut charsets = Charsets::default();
		charsets.designate(0, designated(b"\x1b(K").unwrap());
		charsets.restore();
		assert_eq!(charsets.map('['), 'Ä');
	}

	/// The regression the two banks exist to prevent: a full-screen program that leaves line drawing
	/// invoked must not hand the shell a screen of box corners.
	#[test]
	fn the_alternate_screen_keeps_its_designations_to_itself() {
		let mut charsets = Charsets::default();
		charsets.set_alternate(true);
		charsets.designate(1, Charset::LineDrawing);
		charsets.lock(1, false);
		assert_eq!(charsets.map('q'), '─');
		charsets.set_alternate(false);
		assert_eq!(charsets.map('q'), 'q', "the main screen never heard of it");
		charsets.set_alternate(true);
		assert_eq!(charsets.map('q'), '─', "and the program's own sets survive");
	}

	/// RIS clears BOTH banks and both saved slots, and leaves the halves where the constants say.
	#[test]
	fn a_reset_empties_both_banks() {
		let mut charsets = Charsets::default();
		charsets.designate(1, Charset::LineDrawing);
		charsets.lock(1, false);
		charsets.save();
		charsets.set_alternate(true);
		charsets.designate(2, Charset::LineDrawing);
		charsets.lock(2, false);
		charsets.reset();
		assert_eq!(charsets.map('q'), 'q', "the alternate bank");
		charsets.set_alternate(false);
		assert_eq!(charsets.map('q'), 'q', "and the main one");
		charsets.restore();
		assert_eq!(charsets.map('q'), 'q', "and what was saved on it");
		assert_eq!(charsets.gl(), DEFAULT_GL);
		assert_eq!(charsets.gr(), DEFAULT_GR);
	}

	/// GR can never name G0, because the three sequences that write it are LS1R, LS2R and LS3R and
	/// there is no LS0R. That is the whole argument for [`DEFAULT_GR`], so it is asserted rather than
	/// left in prose.
	#[test]
	fn no_locking_shift_can_put_g0_in_the_right_half() {
		for byte in *b"no~}|" {
			if let Some(CharsetRequest::Lock { slot, right }) = shift(byte) {
				assert!(
					!right || slot != 0,
					"ESC {} would put G0 in GR",
					char::from(byte)
				);
			}
		}
		assert_ne!(DEFAULT_GR, 0, "so the default cannot be G0 either");
	}
}

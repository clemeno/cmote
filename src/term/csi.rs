// term/csi.rs — the facts every CSI scanner has to agree with the engine about.
//
// TEN modules in this directory scan CSI sequences beside the stream, each for its own reason, and
// every one of them used to carry its own copy of the grammar. §106's architecture review put that
// duplication first, as "give the CSI family the floor OSC already has", and §111 did it: all ten
// read [`Framer`] now and keep only the part that is theirs — deciding what a sequence MEANS.
//
// The ten, in the order they migrated: `tabs`, `dsr`, `scp`, `protect`, `sgrstack`, `modkeys`, `rect`,
// `cancel`, `graphics`, `query`. The number is worth spelling out because it was got wrong twice in
// §111's own prose — nine while the migration ran, then "eleven" once it was over, which counted
// `differential` as a scanner when it is the test harness that drives them. Both counts were written
// from memory. The check that settles it is `framer: super::csi::Framer` — one per scanner, and
// nothing else in this directory holds one.
//
// What that bought was not the line count. Defects came out of the migrations, one at a time, because
// the shared grammar had to take the STRICTEST rule of its callers rather than the laxest — two rules
// only `scp` and `protect` had enforced by hand became everyone's, and then the rest fell out of
// asking what the engine really does:
//
//   * FOUR bounds named `MAX_PARAMS` that counted parameter BYTES rather than parameters, and
//     abandoned a sequence the engine saturates and acts on (`sgrstack` and `rect` at 64, `modkeys`
//     and `query` at 16);
//   * a `Params` that could not tell a written zero from an omitted parameter;
//   * `CSI 2 : 3 J`, which wipes the screen — and which `graphics` did not claim, so every picture
//     stayed on it;
//   * two scanners deaf to the sequence after a control string, one of them losing a RIS;
//   * two scanners with no state for an intermediate byte or for a byte the engine reads through.
//
// Every one is recorded where it was fixed and in PLAN §111.
//
// This module started with the part that could not wait for it. A scanner's LIMITS are not a private
// choice: cmote and the engine read the same bytes, and wherever the two disagree about whether a
// sequence is well formed, one of them acts and the other does not. Two of those disagreements were
// live defects, both fixed by the numbers below (§57 read a padded DECSLRM as a save-cursor; §56 lost
// protection across a long SGR), and the numbers were previously spelled eight times at three
// different values, none of them the engine's.
//
// So the engine's own numbers live here, once, with their source, and the scanners refer to them.
// Whatever else the framer takes over, it will not have to re-derive these.

/// The most parameters one CSI sequence may carry before the engine stops reading it.
///
/// `vte`'s own `MAX_PARAMS`, from its vendored source: `params.rs:5` declares it, `:16-19` fixes the
/// array at that width, and `:49-51` is the `is_full` test. Once full, every further parameter action
/// sets the parser's `ignoring` flag (`lib.rs:454-517`), and `ansi.rs:1545-1548` then drops the whole
/// sequence rather than dispatching it. Sub-parameters (the `:` form) share the same budget.
///
/// A scanner that exceeds this should abandon the sequence, because that is what the engine does with
/// it: both sides ignoring the same bytes is agreement, and it is the only bound here that can be
/// enforced by giving up.
pub const MAX_PARAMS: usize = 32;

/// The most digits worth keeping in one parameter.
///
/// This one is NOT a limit the engine has — `vte` folds every digit in with `saturating_mul`
/// (`lib.rs:514-515`) and never abandons a sequence over the length of a run. Copying that literally
/// would mean buffering unbounded remote input, which §12 refuses; abandoning instead is what caused
/// the two defects above, because the engine goes on to act on a sequence the scanner gave up on.
///
/// So the run is CLAMPED rather than capped: digits past this many are dropped and the sequence lives.
/// That is exactly equivalent to the engine's answer, and the test below is why — five digits already
/// reach past `u16::MAX`, so any value a sixth digit could produce is one the engine has saturated
/// too. The memory a scanner can be made to hold is then `MAX_PARAMS * MAX_DIGITS` plus its
/// separators, which is under 200 bytes.
pub const MAX_DIGITS: usize = 5;

/// Whether the engine would keep reading a sequence across `byte` — the bytes a CSI's own grammar does
/// not claim, but which do not end it either (§106).
///
/// `vte`'s CSI states run a C0 control where it sits and CARRY ON with the sequence around it
/// (`lib.rs:190`, `:219`, `:230`, `:241`), ignore DEL (`:222`, `:251`), and pass a byte past `0x7f` to
/// `anywhere`, which does nothing with it (`:438-449`). Only CAN (`0x18`) and SUB (`0x1a`) abandon the
/// sequence, which is the ANSI state machine's own definition of them, and ESC restarts it.
///
/// A scanner that gave up on one of these bytes would leave the engine to dispatch a sequence cmote never
/// judged, which is how §57's and §56's harm was reachable a second time over: `CSI 5;` LF `70 s` is a
/// margin request to the engine and was nothing at all to the scanner shadowing it.
pub fn passes_through(byte: u8) -> bool {
	matches!(byte, 0x00..=0x17 | 0x19 | 0x1c..=0x1f | 0x7f | 0x80..=0xff)
}

/// The parameter run of the CSI or DCS sequence a scanner is in the middle of reading.
///
/// [`Framer`] holds one, and so does `graphics` for the sixel parameters its own DCS machine reads —
/// the two uses that make this a seam rather than a hypothetical one. It was a seam before the framer
/// existed, too: the rule below was wrong in the same way in the two scanners that had hand-rolled it,
/// and had to be fixed in one place.
///
/// What it keeps is the run's BYTES, so a caller can still read the run the way it always did — as
/// `1`, `?2`, `38;5;196` — rather than being handed numbers it did not ask for. Parsing stays with the
/// caller, because the ten disagree about what an omitted parameter means (0, 1, "not ours",
/// "everything") and a shared parser would need every one of those as an option.
#[derive(Debug, Default)]
pub struct Params {
	bytes: Vec<u8>,
	/// Separators seen, so the parameter count is `fields + 1`.
	fields: usize,
	/// Significant digits kept in the parameter being written now. Leading zeros are not counted,
	/// because they are not significant — see [`Params::push`].
	digits: usize,
	/// Whether any parameter byte has arrived, kept or dropped. Distinct from `bytes.is_empty()`, which
	/// a dropped leading zero would leave true — and a caller that reads emptiness as "no parameters
	/// yet" would then take the next byte for a private marker on a sequence that already had one.
	started: bool,
	/// Whether the parameter being written now has had any byte at all, kept or dropped.
	///
	/// The same distinction as `started`, one field down, and it is what keeps `0` from reading as an
	/// OMITTED parameter. Dropping leading zeros leaves an all-zero field with nothing written, so
	/// `CSI # 1 ; 0 {` and `CSI # 1 ; {` would render identically — and a scanner that treats an
	/// empty field as malformed (§99's SGR stack does, deliberately) would then drop a sequence that
	/// named zero perfectly well. [`close_field`] writes the zero back.
	field_started: bool,
	/// Whether any separator in the run was a `:` rather than a `;` — see [`Csi::sub_parameters`] for
	/// what a scanner does about it.
	subs: bool,
}

impl Params {
	/// Start a fresh run, for the `[` or `P` that begins one.
	pub fn clear(&mut self) {
		self.bytes.clear();
		self.fields = 0;
		self.digits = 0;
		self.started = false;
		self.field_started = false;
		self.subs = false;
	}

	/// Fold one parameter byte in, and say whether the sequence is still one the engine would read.
	///
	/// `false` means it carries more parameters than the engine's array holds, so the engine ignores the
	/// whole sequence — and the caller should abandon it too, which is what makes the two agree.
	///
	/// A long DIGIT run never returns `false`. Digits past [`MAX_DIGITS`] SIGNIFICANT ones are dropped
	/// and the run lives, because the engine saturates the number instead of giving up on the sequence.
	/// Leading zeros are dropped and cost nothing at all, which is the correction that matters: they do
	/// not change the value, so a clamp that counted them would read `CSI 0000000000000002 J` as 0 and
	/// leave the engine erasing a screen cmote thought was untouched.
	pub fn push(&mut self, byte: u8) -> bool {
		self.started = true;
		if byte == b';' || byte == b':' {
			self.subs |= byte == b':';
			self.close_field();
			self.fields += 1;
			self.digits = 0;
			self.field_started = false;
			if self.fields >= MAX_PARAMS {
				return false;
			}
			self.bytes.push(byte);
		} else if byte == b'0' && self.digits == 0 {
			// A leading zero. Nothing to keep: the value is the same without it, and the engine's fold
			// over it is the identity. `close_field` puts one back if the whole field was zeros, so
			// that a written 0 does not end up indistinguishable from a parameter nobody wrote.
			self.field_started = true;
		} else if self.digits < MAX_DIGITS {
			self.digits += 1;
			self.field_started = true;
			self.bytes.push(byte);
		} else {
			// Past the clamp: the digit is dropped, but the field has still been written to.
			self.field_started = true;
		}
		true
	}

	/// Finish the run, so the LAST field gets the same treatment every earlier one got at its
	/// separator. The framer calls this before handing the sequence to a scanner; reading `bytes`
	/// without it would see `CSI 0 J`'s parameter as absent rather than as zero.
	pub fn finish(&mut self) {
		self.close_field();
	}

	/// Give the field being written a `0` if it was written to but kept no digits — an all-zero run.
	///
	/// Idempotent, because `finish` may follow a separator that already closed the field: the flag is
	/// cleared as the zero goes in.
	fn close_field(&mut self) {
		if self.field_started && self.digits == 0 {
			self.bytes.push(b'0');
			self.digits = 1;
		}
		self.field_started = false;
	}

	/// The run as the sequence wrote it, minus the bytes that could not change what it means — so a
	/// canonical decimal rendering of the same numbers, `;` and `:` separators intact.
	pub fn bytes(&self) -> &[u8] {
		&self.bytes
	}

	/// Whether any parameter byte has arrived yet — the test for "a private marker is still legal here",
	/// which is only true before the first one.
	pub fn started(&self) -> bool {
		self.started
	}

	/// How many parameters the run carries, counting an EMPTY one.
	///
	/// `6` is one, `6;1` is two, and `6;` is also two — the second is present and omitted, which is
	/// not the same as absent. That distinction is the whole reason this is not `param(1).is_none()`:
	/// a scanner that takes exactly one `Ps` (§82's DSR) must reject `CSI ? 6 ; n`, and reading the
	/// second field as "not there" would accept it.
	///
	/// Zero when nothing arrived at all, including when every byte was a dropped leading zero — so
	/// `started` rather than `bytes.is_empty()` decides it.
	pub fn count(&self) -> usize {
		if self.started { self.fields + 1 } else { 0 }
	}

	/// Whether any separator in the run was a `:`.
	pub fn has_subs(&self) -> bool {
		self.subs
	}
}

/// The most intermediate bytes a scanner will buffer.
///
/// A real CSI carries at most one or two (`"` for DECSCA, `!` for DECSTR), and refusing to grow past
/// this keeps a hostile stream out of our memory (§12). Deliberately LOOSER than the engine's own two
/// (`vte`'s `MAX_INTERMEDIATES`, which counts the private marker against it as well), and unlike
/// [`MAX_PARAMS`] this bound cannot be observed from outside: a scanner classifies on the
/// intermediates it knows, so a sequence carrying three of them goes unclassified here and is dropped
/// there — both sides ignore it, by different routes.
///
/// Six modules spelled this number for themselves before the framer arrived, all at 4, and
/// `protect.rs` said in writing that it belonged here (§106, §111).
pub const MAX_INTERMEDIATES: usize = 4;

/// The escape byte that opens every sequence the framer is looking for.
const ESC: u8 = 0x1b;

/// One finished CSI sequence, handed to a scanner to judge.
///
/// The four parts a CSI has, and no more: `ESC [`, an optional private marker, a parameter run,
/// intermediate bytes, one final byte. A scanner reads what it needs and ignores the rest — which is
/// the whole point of the split, because deciding what a sequence MEANS is the only part that differs
/// between the ten of them.
///
/// Borrowed rather than owned, and passed to a callback rather than collected: a scanner keeps at most
/// a byte offset out of each sequence, so allocating one of these per sequence would be a `Vec` built
/// and dropped on the hot path for every `CSI ? 1049 h` the engine owns.
///
/// The accessors arrive WITH the scanners that need them, one migration at a time (§111). A helper
/// with no caller is a build error here, which is the `[lints]` rule doing exactly what it is for —
/// and it has already paid: this note used to predict that `sgrstack` would need the raw parameter run
/// as bytes, because it walks every parameter rather than indexing one. It does not. `param_count`
/// with `param` covers the walk, and the run stayed private.
#[derive(Debug, Clone, Copy)]
pub struct Csi<'a> {
	marker: Option<u8>,
	params: &'a Params,
	intermediates: &'a [u8],
	final_byte: u8,
}

impl Csi<'_> {
	/// The private marker the sequence opened with — `<`, `=`, `>` or `?` — if it had one.
	pub fn marker(&self) -> Option<u8> {
		self.marker
	}

	/// The byte that ended the sequence, in `0x40..=0x7e`. What a scanner matches on first.
	pub fn final_byte(&self) -> u8 {
		self.final_byte
	}

	/// The intermediate bytes, in the order they arrived. Empty for most sequences.
	pub fn intermediates(&self) -> &[u8] {
		self.intermediates
	}

	/// How many parameters the sequence carries — see [`Params::count`] for why an empty one still
	/// counts. What a scanner uses to insist on exactly as many as its sequence defines.
	pub fn param_count(&self) -> usize {
		self.params.count()
	}

	/// Whether the parameter run carries a SUB-PARAMETER — at least one `:` where a `;` was expected.
	///
	/// A scanner here refuses a sequence that has one, unless the sequence it reads is defined to take
	/// them. **None of the ten is**, as of §111: DECERA takes four corners, XTPUSHSGR a list of
	/// codes, XTMODKEYS a resource and a value, and DEC and xterm spell every one of those with `;`.
	/// `protect` watches SGR — the one family that really does use `:`, for `38:2:r:g:b` — but it
	/// reports every SGR alike while the pen is armed and never reads a parameter, so a colour written
	/// with colons reaches it as an SGR like any other.
	///
	/// The framer does NOT abandon these, and the split is the point. Sub-parameters are legal in the
	/// engine's grammar and `vte` dispatches them, so refusing one there would be cmote putting its own
	/// policy in the module whose whole job is to agree with the engine — and it would throw the bytes
	/// away for whichever scanner one day wants them. Reporting the fact here leaves the choice at the
	/// site that knows what its own sequence defines, which is the near-miss rule §56 wrote down,
	/// applied to the separator.
	///
	/// **It is the fact, not the policy.** The three scanners this reaches are unanimous today, and
	/// pushing the refusal up here would look like a rule and read like an accident the moment a fourth
	/// one disagrees. That is not hypothetical: `sgrstack` and `modkeys` both crossed onto the framer
	/// reading a `:` as another `;` — a widening nobody asked for, and one that would have let
	/// `CSI 2 : 3 ; 5 ; 7 $ z` erase a rectangle the program never named once `rect` followed.
	pub fn sub_parameters(&self) -> bool {
		self.params.has_subs()
	}

	/// Parameter `index` as a number, or `None` when it is absent or unreadable.
	///
	/// **It does not supply a default, and that is deliberate.** `Params`' own note used to say a
	/// shared parser could not work because the ten scanners disagree about what an omitted parameter
	/// means — 0 for DECST8C, 1 for a cursor move, "not ours" for a sequence that requires the
	/// parameter. That objection is about a parser that BAKES IN a default; this one reports absence
	/// and leaves the choice where it was, so `param(0).unwrap_or(0)` and `param(0).unwrap_or(1)` are
	/// both still the caller's to write, and eight hand-rolled `split` walks over `b';'` go away —
	/// every scanner but `cancel`, which counted separators, and `dsr`, which matched whole runs
	/// against an allow-list (§111).
	///
	/// Saturating, because the engine saturates (`vte` folds with `saturating_mul`): a run of five
	/// nines is 65,535 to both sides. Every scanner that rolled this by hand used `checked_mul`
	/// instead and answered `None`, which is a scanner giving up on a sequence the engine acts on —
	/// the exact shape of §56's and §57's defects.
	pub fn param(&self, index: usize) -> Option<u16> {
		let field = self
			.params
			.bytes()
			.split(|&byte| byte == b';' || byte == b':')
			.nth(index)?;
		if field.is_empty() {
			return None;
		}
		let mut value: u16 = 0;
		for &byte in field {
			let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
			value = value.saturating_mul(10).saturating_add(u16::from(digit));
		}
		Some(value)
	}
}

/// Where one sequence sat in the chunk that completed it.
///
/// The scanners here want DIFFERENT points out of the same sequence, and each one's choice is a real
/// decision about correctness rather than a convention picked for tidiness. Before this type they all
/// took one bare `usize` and derived what they needed from it — `offset` here, `offset - 1` there —
/// with the reasoning spread across eight doc comments. The three points have names now, and each
/// scanner says which it means at the site that uses it.
///
/// Shared with [`super::dcs::Framer`] rather than spelled twice: the three conventions are the same
/// three whichever grammar found the sequence, and a second copy of this type would be a second place
/// to explain them (§111).
#[derive(Debug, Clone, Copy)]
pub struct Span {
	past: usize,
	start: Option<usize>,
}

impl Span {
	/// Build one. `past` is the byte after the sequence's last; `start` is where its ESC sat in this
	/// chunk, or `None` when that ESC arrived in an earlier one.
	pub(super) fn new(past: usize, start: Option<usize>) -> Self {
		Self { past, start }
	}

	/// The byte AFTER the final byte.
	///
	/// What a scanner wants when it has to feed the engine PAST the sequence before acting on it —
	/// eight of the ten, because the engine ignores the sequence and cmote answers it, and an advance
	/// that stopped short would leave the engine to parse the tail of a sequence cmote had already
	/// answered.
	pub fn past(self) -> usize {
		self.past
	}

	/// The final byte itself.
	///
	/// What a scanner wants when it REPLACES that byte rather than reading around it — `cancel` alone,
	/// which feeds the engine a CAN in place of a final byte it refuses to let it dispatch (§57). The
	/// engine is advanced up to this byte, fed the CAN instead of it, and resumed after it.
	pub fn final_byte_at(self) -> usize {
		// A CSI is `ESC [` and at least a final byte, so `past` is never 0 and this cannot wrap.
		self.past - 1
	}

	/// The ESC that OPENED the sequence — or 0 when it opened in an earlier chunk.
	///
	/// What a scanner wants when it has to act BEFORE the sequence reaches the engine at all, which is
	/// `graphics` alone and is the opposite of what the other nine need. Which pictures an erase takes
	/// is decided by where the screen ends and the scrollback begins, and the engine answers that
	/// differently the instant it applies the erase — `CSI 3 J` drops the very history the question is
	/// about, so asking afterwards is asking a terminal that no longer remembers (§41).
	///
	/// Zero for a sequence that began in an earlier chunk, because its bytes then start at the front of
	/// this one. That is not an approximation: everything before the sequence really has been fed
	/// already, which is exactly what the caller is about to advance past.
	pub fn start(self) -> usize {
		self.start.unwrap_or(0)
	}
}

/// Where the framer is in the byte stream.
#[derive(Debug, Default, PartialEq, Eq)]
enum CsiScan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC. A CSI starts if the next byte is `[`, and nothing else here is of interest.
	Escape,
	/// Inside `ESC [ …`, collecting the sequence until its final byte.
	Csi,
}

/// Cuts CSI sequences out of shell output, once, for every scanner that reads them.
///
/// This is `osc::Framer`'s counterpart, and it exists for the same reason: ten modules in this
/// directory each need to sniff a CSI the engine also reads, each cares about a DIFFERENT sequence,
/// and every one of them first had to solve the same problem — find where a CSI starts and ends in a
/// stream that arrives in arbitrary chunks. That was 62 to 162 lines apiece of identical grammar, and
/// the module-specific part of it is a handful of lines (§106, §111).
///
/// Safe at any chunk boundary: the state carries over between `feed` calls, so a sequence may be split
/// anywhere — between the ESC and the `[`, inside the parameters, or before the final byte.
///
/// **It frames CSI only.** The control strings are [`super::dcs::Framer`]'s and the OSC strings are
/// `osc::Framer`'s — three framers, one per family, because what a family's introducer and terminator
/// look like is the only thing each of them knows. They share what is genuinely shared: [`Params`],
/// [`MAX_INTERMEDIATES`], [`passes_through`] and [`Span`], and one differential harness holding all
/// three to the engine's own parser. What is still spelled three times is the ESC door itself (§111).
#[derive(Debug, Default)]
pub struct Framer {
	state: CsiScan,
	marker: Option<u8>,
	params: Params,
	intermediates: Vec<u8>,
	/// Where in THIS chunk the ESC that opened the sequence being read sat, for [`Span::start`].
	/// `None` once the sequence has run over a chunk boundary — its bytes then begin at the front of
	/// the chunk that finishes it, which is offset 0.
	///
	/// Written when the ESC arrives, not when the `[` does, because the two are not always adjacent: the
	/// engine reads a C0, a DEL or a high byte between them and keeps the sequence.
	start: Option<usize>,
}

impl Framer {
	/// Feed a chunk of shell output, calling `on_csi` once per CSI sequence that COMPLETES in it.
	///
	/// The [`Span`] says where in THIS `bytes` slice the sequence sat, and the caller picks which point
	/// of it it means — see that type for why there is more than one.
	pub fn feed(&mut self, bytes: &[u8], mut on_csi: impl FnMut(Span, &Csi<'_>)) {
		// Any sequence still open from the last chunk began before this one did.
		self.start = None;
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				CsiScan::Text => {
					if byte == ESC {
						// Where the sequence begins, recorded as the ESC arrives rather than worked back
						// from the `[`: the two are not always one byte apart (see the arm below).
						self.start = Some(index);
						self.state = CsiScan::Escape;
					}
				}
				CsiScan::Escape => match byte {
					b'[' => {
						self.marker = None;
						self.params.clear();
						self.intermediates.clear();
						self.state = CsiScan::Csi;
					}
					// ESC ESC: still waiting for the sequence's real first byte, and it is the LATEST ESC
					// that opens whatever follows.
					ESC => self.start = Some(index),
					// The read-through rule, one state earlier than the arm below — and the framer obeyed
					// it only there. `vte`'s escape state executes a C0 and STAYS in that state
					// (`lib.rs:341`), ignores DEL and every byte past `0x7f` (`:381-383`), so `ESC` LF
					// `[ 2 J` erases the screen. Dropping to ordinary text here read the `[` as a
					// printable character and lost the sequence for all ten scanners at once (§111).
					byte if passes_through(byte) => {}
					// CAN and SUB drop the escape back to GROUND, where a `[` starts nothing at all — so
					// unlike the arm below, this really is the end of any reading.
					_ => self.state = CsiScan::Text,
				},
				CsiScan::Csi => match byte {
					// Parameter bytes: digits and separators, plus the private markers (`< = > ?`,
					// 0x3c–0x3f) which are only legal as the very first one.
					0x30..=0x3f => {
						if !self.intermediates.is_empty() {
							// A parameter byte AFTER an intermediate. The engine refuses the whole
							// sequence for this — `vte`'s CSI-intermediate state goes straight to
							// `CsiIgnore` — so carrying on would mean acting alone on a spelling
							// nothing else in the world obeys (§106).
							//
							// Only `scp` enforced this before the framer arrived; the other eight would
							// have read `CSI 1 ! 2 k` as a sequence the engine had already thrown away.
							// Settling it here is the point of having one grammar (§111).
							self.state = CsiScan::Text;
						} else if byte >= 0x3c {
							// A private marker (`< = > ?`). Legal ONLY as the very first parameter byte;
							// after that `vte` drops the whole sequence (`lib.rs:249`), so the framer
							// does too rather than folding the byte into the digits.
							//
							// Which is what keeps a parameter run to digits and separators, and so keeps
							// `param` from ever having to report "unreadable". Letting the byte through
							// was a real defect for the two hops it existed: `scp` used to reject
							// `CSI 1 ? SP k` BECAUSE the `?` made its digits unparseable, and the
							// differential sweep caught the moment that accident stopped covering it.
							if self.params.started() || self.marker.is_some() {
								self.state = CsiScan::Text;
							} else {
								self.marker = Some(byte);
							}
						} else if !self.params.push(byte) {
							// More parameters than the engine's array holds, so the engine ignores the
							// whole sequence. Giving up here is what makes the two agree.
							self.state = CsiScan::Text;
						}
					}
					// Intermediate bytes.
					0x20..=0x2f => {
						self.intermediates.push(byte);
						if self.intermediates.len() > MAX_INTERMEDIATES {
							self.state = CsiScan::Text;
						}
					}
					// The final byte ends the sequence, so this is where the scanner is asked.
					0x40..=0x7e => {
						// Close the last parameter first: it never met a separator, so this is where an
						// all-zero one gets its digit back.
						self.params.finish();
						on_csi(
							Span::new(index + 1, self.start),
							&Csi {
								marker: self.marker,
								params: &self.params,
								intermediates: &self.intermediates,
								final_byte: byte,
							},
						);
						self.state = CsiScan::Text;
					}
					// A fresh ESC restarts the match.
					ESC => self.state = CsiScan::Escape,
					// A byte the grammar above does not claim, but which the engine reads STRAIGHT
					// THROUGH, keeping the sequence in every case (§106). Abandoning it here would mean
					// cmote and the engine disagreeing about what this byte stream even was, which is
					// how three defects reached a release.
					byte if passes_through(byte) => {}
					// CAN and SUB, the only two bytes that really cancel a sequence in flight.
					_ => self.state = CsiScan::Text,
				},
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed a whole parameter run and read back what was kept, plus whether it survived. Mid-run, so
	/// the last field is still open — which is what a scanner would see if it read the run early.
	fn run(bytes: &[u8]) -> (String, bool) {
		let mut params = Params::default();
		let alive = bytes.iter().all(|&byte| params.push(byte));
		(String::from_utf8_lossy(params.bytes()).into_owned(), alive)
	}

	/// The same, finished the way the framer finishes one before handing it to a scanner.
	fn run_closed(bytes: &[u8]) -> (String, bool) {
		let mut params = Params::default();
		let alive = bytes.iter().all(|&byte| params.push(byte));
		params.finish();
		(String::from_utf8_lossy(params.bytes()).into_owned(), alive)
	}

	#[test]
	fn leading_zeros_cost_nothing_and_change_nothing() {
		// The bug this module was written for: `CSI 0000000000000002 J` is an erase to the engine, and a
		// clamp that spent its budget on the zeros read it as 0 and said nothing.
		assert_eq!(run(b"000000000000000002"), ("2".to_owned(), true));
	}

	#[test]
	fn an_all_zero_field_keeps_one_zero_so_it_is_not_an_omitted_parameter() {
		// The distinction §111 restored. Dropping every leading zero left `0` rendering as nothing at
		// all, so a written zero and an omitted parameter came out identical — and a scanner that reads
		// an empty field as malformed (§99's SGR stack) would drop a sequence that named zero perfectly
		// well. The run is a canonical rendering of the same numbers now.
		assert_eq!(run_closed(b"0"), ("0".to_owned(), true));
		assert_eq!(run_closed(b"000"), ("0".to_owned(), true));
		assert_eq!(run_closed(b"0;0"), ("0;0".to_owned(), true));
		assert_eq!(run_closed(b"1;0;2"), ("1;0;2".to_owned(), true));
		// An omitted parameter is still omitted: nothing was written, so nothing is rendered.
		assert_eq!(run_closed(b";"), (";".to_owned(), true));
		assert_eq!(run_closed(b"1;"), ("1;".to_owned(), true));
	}

	#[test]
	fn a_runaway_digit_run_is_clamped_and_the_sequence_survives() {
		let (kept, alive) = run(&[b'9'; 500]);
		assert_eq!(kept, "99999", "five significant digits, and no more");
		assert!(
			alive,
			"the engine saturates rather than giving up, so we do too"
		);
	}

	#[test]
	fn too_many_parameters_ends_the_run() {
		// Thirty-two parameters is the most the engine reads; the thirty-second separator starts the
		// thirty-third, which it ignores the sequence over.
		let (_, alive) = run(&[b';'; MAX_PARAMS - 1]);
		assert!(alive, "thirty-two parameters still fit");
		let (_, alive) = run(&[b';'; MAX_PARAMS]);
		assert!(!alive, "thirty-three do not");
	}

	#[test]
	fn a_dropped_leading_zero_does_not_make_the_run_look_unstarted() {
		// Otherwise the caller takes the next byte for a private marker, and `CSI 0?J` — which the
		// engine drops outright — would classify here as a selective erase.
		let mut params = Params::default();
		assert!(!params.started());
		params.push(b'0');
		assert!(params.bytes().is_empty());
		assert!(params.started(), "a zero is still a parameter byte");
	}

	#[test]
	fn the_digit_clamp_reaches_the_saturation_point() {
		// The whole argument for clamping instead of capping: a parameter is at most a `u16` on both
		// sides of `process`, and a five-digit run can already express more than the largest one. So a
		// clamped run and the engine's saturating one land on the same number for every input that can
		// be told apart, and a scanner never has to abandon a sequence the engine will act on.
		assert!(
			u16::MAX.to_string().len() <= MAX_DIGITS,
			"a clamped run must be able to reach the value the engine saturates at"
		);
	}

	/// Every sequence a framer finds in `chunks`, as `(offset, marker, final byte, first parameter)`.
	/// Fed chunk by chunk through ONE framer, so a split sequence is scanned the way the stream
	/// delivers it.
	fn framed(chunks: &[&[u8]]) -> Vec<(usize, Option<u8>, u8, Option<u16>)> {
		let mut framer = Framer::default();
		let mut found = Vec::new();
		for chunk in chunks {
			framer.feed(chunk, |span, csi| {
				found.push((span.past(), csi.marker(), csi.final_byte(), csi.param(0)));
			});
		}
		found
	}

	#[test]
	fn a_sequence_is_reported_one_past_its_final_byte() {
		// The offset every scanner here relies on: the engine has to be advanced past the sequence
		// before cmote acts on it, so the offset names the byte AFTER the final one.
		assert_eq!(framed(&[b"\x1b[?5W"]), vec![(5, Some(b'?'), b'W', Some(5))]);
		assert_eq!(framed(&[b"ab\x1b[3J"]), vec![(6, None, b'J', Some(3))]);
	}

	#[test]
	fn a_sequence_split_anywhere_completes_on_the_chunk_that_ends_it() {
		// Split between the ESC and the `[`, mid-parameter, and just before the final byte. In every
		// case the offset is measured in the chunk that carried the terminator.
		assert_eq!(
			framed(&[b"\x1b", b"[?5W"]),
			vec![(4, Some(b'?'), b'W', Some(5))]
		);
		assert_eq!(
			framed(&[b"\x1b[?", b"5W"]),
			vec![(2, Some(b'?'), b'W', Some(5))]
		);
		assert_eq!(
			framed(&[b"\x1b[?5", b"W"]),
			vec![(1, Some(b'?'), b'W', Some(5))]
		);
	}

	#[test]
	fn two_sequences_in_one_chunk_are_both_reported_in_order() {
		let found = framed(&[b"\x1b[?5W\x1b[2J"]);
		assert_eq!(found.len(), 2);
		assert_eq!(found[0].0, 5);
		// Four bytes, not five: `ESC [ 2 J` carries no marker, so it ends at 9.
		assert_eq!(found[1], (9, None, b'J', Some(2)));
	}

	#[test]
	fn a_private_marker_after_the_parameters_abandons_the_sequence() {
		// `CSI ? 5 W` opens with one and frames. `CSI 5 ? W` is not "a marker read as a digit" — `vte`
		// DROPS the whole sequence (`lib.rs:249`), so the framer must as well.
		//
		// This test asserted the opposite when it was written, and the differential sweep is what said
		// otherwise: `scp` had been rejecting `CSI 1 ? SP k` only because the stray `?` made its digits
		// unparseable, so folding the byte into the run turned an accident into a sequence cmote acted
		// on and the engine had thrown away (§111).
		assert_eq!(framed(&[b"\x1b[?5W"])[0].1, Some(b'?'));
		assert!(framed(&[b"\x1b[5?W"]).is_empty());
		assert!(framed(&[b"\x1b[??W"]).is_empty(), "two markers is not one");
	}

	#[test]
	fn a_pass_through_byte_keeps_the_sequence_and_can_or_sub_ends_it() {
		// The rule `passes_through` exists for, at the framer this time: the engine reads a
		// mid-sequence control byte through and keeps the sequence, so the framer must too, or the two
		// disagree about what the same bytes were (§106).
		assert_eq!(framed(&[b"\x1b[?5\x07W"]).len(), 1, "BEL is read through");
		assert!(framed(&[b"\x1b[?5\x18W"]).is_empty(), "CAN cancels");
		assert!(framed(&[b"\x1b[?5\x1aW"]).is_empty(), "SUB cancels");
	}

	#[test]
	fn an_omitted_parameter_is_none_so_the_caller_picks_its_own_default() {
		// The whole reason `param` does not supply a default: DECST8C reads an absent parameter as 0
		// and a cursor move reads it as 1, and only the scanner knows which it is.
		assert_eq!(framed(&[b"\x1b[?W"])[0].3, None);
		assert_eq!(framed(&[b"\x1b[;5W"])[0].3, None, "an empty first field");
	}

	#[test]
	fn a_parameter_past_a_u16_saturates_rather_than_vanishing() {
		// Five digits can express more than a `u16` holds, and the engine folds with `saturating_mul`.
		// A scanner that answered `None` here would be giving up on a sequence the engine acts on,
		// which is the shape of §56's and §57's defects.
		assert_eq!(framed(&[b"\x1b[99999W"])[0].3, Some(u16::MAX));
	}

	#[test]
	fn a_parameter_after_an_intermediate_abandons_the_sequence() {
		// `vte`'s CSI-intermediate state goes straight to `CsiIgnore`, so the engine has already
		// thrown this sequence away; a scanner that read it would be acting alone (§106).
		assert!(framed(&[b"\x1b[1 2k"]).is_empty());
		// The legal order still frames: parameters, then intermediates, then the final byte.
		assert_eq!(framed(&[b"\x1b[1 k"]).len(), 1);
	}

	#[test]
	fn too_many_intermediates_abandon_the_sequence() {
		let mut bytes = b"\x1b[".to_vec();
		bytes.extend(std::iter::repeat_n(b'!', MAX_INTERMEDIATES + 1));
		bytes.push(b'W');
		assert!(framed(&[&bytes]).is_empty());
	}

	#[test]
	fn the_parameter_bound_is_the_engines_own() {
		// Written down as a test rather than only in prose, so a version bump that changes `vte`'s
		// width is a conversation rather than a silent drift. There is no way to read the constant out
		// of the crate — it is `pub(crate)` there — so this is the one place the number is asserted.
		assert_eq!(MAX_PARAMS, 32, "vte params.rs:5");
	}
}

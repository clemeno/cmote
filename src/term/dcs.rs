// term/dcs.rs — the facts every CONTROL-STRING scanner has to agree with the engine about.
//
// `csi::Framer` gave the CSI family one grammar (§111). This is the other door: `ESC`, and everything
// that comes through it which is not a CSI — a DCS control string (`ESC P … ST`) and the two-part
// escape sequences (`ESC c`, `ESC ( B`).
//
// It exists because that grammar was spelled SIX times over. Two modules read DCS strings and had a
// state machine each — `query` for DECRQSS and XTGETTCAP, `graphics` for sixel payloads — and four more
// watched for one escape sequence with three lines of hand-rolled machine apiece:
//
//     if self.after_escape && byte == b'c' { … }      // protect, scp, sgrstack, rect
//     self.after_escape = byte == ESC;
//
// §111 found three defects in the two DCS machines alone (two ESC, one ST), which is the whole argument
// for this module: the duplication was not costing line count, it was costing agreement with the engine.
//
// WHAT THE ENGINE DOES, read off `vte`'s state table rather than remembered (`lib.rs`, 0.15.0):
//
//   * The introducer (`advance_dcs_entry`/`_param`/`_intermediate`, `:256-313`) is the CSI grammar over
//     again: an optional private marker, a parameter run, intermediate bytes, one final byte — and the
//     same two refusals, a parameter byte after an intermediate and a marker after the parameters
//     started, both of which send the parser to `DcsIgnore`.
//   * Those three states READ THROUGH the bytes a sequence's own grammar does not claim (a C0, DEL,
//     anything past `0x7f`) and keep the sequence, exactly as the CSI states do — so
//     `csi::passes_through` is the predicate here too.
//   * In the payload (`advance_dcs_passthrough`, `:316-336`) the string ends three ways: `0x9c`, the
//     single-byte ST; CAN or SUB, which unhook it and drop to ground; and ESC, which unhooks it and
//     opens the next sequence. DEL and every byte past `0x7f` are DISCARDED rather than kept.
//   * BEL is NOT a terminator for the engine — it is a payload byte. cmote ends a string on it anyway,
//     which is leniency rather than agreement; see `BEL`.
//   * `ESC` then a C0, a DEL or a high byte stays in the escape state (`:341`, `:381-383`), so
//     `ESC` LF `c` is a hard reset. That rule cost `csi::Framer` a defect of its own in §111.
//
// WHY IT FRAMES ESCAPE SEQUENCES AND NOT ONLY STRINGS. ESC is the door to a DCS, so this machine has to
// own it; once it does, reporting the two-part sequences it necessarily walks past costs one enum
// variant and pays for the four hand-rolled watchers above. It is also the only way to get the rule
// right: a watcher that tests "was the previous byte an ESC" reads `ESC` LF `c` as ordinary text while
// the engine resets the terminal, and four modules were doing exactly that.
//
// WHAT IT DOES NOT FRAME. A CSI (`csi::Framer`), an OSC (`osc::Framer`), and the SOS / PM / APC strings
// nothing in cmote reads. All four are recognised and dropped back to ordinary text rather than
// followed to their ends, and that is SAFE for the reason §111 measured: a control string can only be
// interrupted by an ESC, and an ESC ends it for the engine too. So there is no payload the engine reads
// as data and a framer here reads as a sequence — which is also why an ABANDONED string needs no
// "follow it to the terminator" state. `query` and `graphics` each had one; they were doing nothing that
// hunting for the next ESC does not already do, and the same argument retires the cap-and-overflow
// policy `osc.rs` named as the reason `graphics` could not share ITS framer: whether an overlong payload
// is followed to its end or abandoned on the spot, the next thing either machine can react to is an ESC.

use super::csi::{MAX_INTERMEDIATES, Params, Span, passes_through};

/// The escape byte that opens every sequence this framer is looking for.
const ESC: u8 = 0x1b;

/// The bell, an alternate string terminator some programs use in place of `ESC \`.
///
/// Leniency rather than agreement, and it is worth being exact about which: `vte` does NOT end a
/// control string on BEL — it reads the byte into the payload and keeps going (`lib.rs:319`, measured in
/// `differential.rs`). cmote accepts it because real emitters send it, and what turns on it is only
/// whether cmote answers a question about ITSELF, or draws a picture, one byte earlier than the engine
/// would have stopped reading. Nothing the engine does depends on it.
const BEL: u8 = 0x07;

/// ST, the string terminator as a single byte — the C1 form of `ESC \`.
///
/// This one IS the engine's (`lib.rs:331`). `graphics` had it from the start and `query` never did,
/// which is duplicated grammar in its plainest form: one of a pair of control-string scanners learned a
/// rule and its twin never did, so a DECRQSS ended this way went unanswered while a picture ended this
/// way drew fine (§111).
const ST: u8 = 0x9c;

/// One finished control string, handed to a scanner to judge.
///
/// The introducer is a CSI's parts exactly — `ESC P`, an optional private marker, a parameter run,
/// intermediate bytes, one final byte — followed by the payload the terminator closed. A scanner reads
/// what it needs and ignores the rest, which is the same split [`super::csi::Csi`] draws: deciding what
/// a string MEANS is the only part that differs between the callers.
///
/// Borrowed rather than owned. A sixel payload is megabytes, and handing one over by value would copy
/// it once per picture for nothing.
#[derive(Debug, Clone, Copy)]
pub struct Dcs<'a> {
	marker: Option<u8>,
	params: &'a Params,
	intermediates: &'a [u8],
	final_byte: u8,
	payload: &'a [u8],
}

impl Dcs<'_> {
	/// The private marker (`< = > ?`) if the string carried one, which is only legal as its first
	/// parameter byte. A scanner that does not define one must refuse a string that has one.
	pub fn marker(&self) -> Option<u8> {
		self.marker
	}

	/// The intermediate bytes, in the order they arrived. `$` for DECRQSS, `+` for XTGETTCAP, none at
	/// all for a sixel — which is the whole of what tells the three apart, since all three end in `q`.
	pub fn intermediates(&self) -> &[u8] {
		self.intermediates
	}

	/// The final byte of the introducer — the one that started the payload.
	pub fn final_byte(&self) -> u8 {
		self.final_byte
	}

	/// The payload, terminator excluded, as the engine would have been given it: DEL and the high bytes
	/// the parser discards are not in here.
	pub fn payload(&self) -> &[u8] {
		self.payload
	}

	/// How many parameters the introducer carried — see [`Params::count`] for why an empty one still
	/// counts. What a scanner uses to insist a string defines no parameters at all, which is true of
	/// both DCS queries cmote answers.
	pub fn param_count(&self) -> usize {
		self.params.count()
	}
}

/// One finished escape sequence that opens no string: `ESC`, any intermediates, one final byte.
///
/// `ESC c` (RIS) is the only one anything in cmote reads today, and four modules were watching for it
/// with three lines of their own before this type existed (§111).
#[derive(Debug, Clone, Copy)]
pub struct Escape<'a> {
	intermediates: &'a [u8],
	final_byte: u8,
}

impl Escape<'_> {
	/// The intermediate bytes. Empty for RIS, and NOT empty for the charset designations that share its
	/// final byte — which is why a scanner has to test both parts. `ESC c` resets the terminal;
	/// `ESC ( c` designates a character set and resets nothing.
	pub fn intermediates(&self) -> &[u8] {
		self.intermediates
	}

	/// The final byte: `c` for RIS, `B` in `ESC ( B`, `\` for a stray ST.
	pub fn final_byte(&self) -> u8 {
		self.final_byte
	}
}

/// What the framer found. Two shapes, because ESC leads to both and a scanner may want either.
///
/// **A terminator is reported once, as the completion of the string it ended** — and that is the one
/// place this framer knowingly says less than the engine's parser, which fires `unhook` for the string
/// AND dispatches the `ESC \` as an escape sequence of its own (`lib.rs:326`, `:368`). Reporting both
/// would hand every caller a `\` to ignore after every picture and every query, for a sequence nothing
/// in a terminal acts on.
///
/// Where an `ESC \` DOES arrive as [`Control::Escape`] is when it terminated nothing this framer was
/// holding: after a payload past `CAP`, after a CAN or SUB, or in ordinary text. The first of those is
/// the engine's reading exactly (it had no cap, so its string was still open); the other two are stray
/// STs to both sides.
#[derive(Debug, Clone, Copy)]
pub enum Control<'a> {
	/// A control string that reached its terminator. Reported at [`Span::past`].
	String(Dcs<'a>),
	/// An escape sequence that opened no string.
	Escape(Escape<'a>),
}

/// Where the framer is in the byte stream.
#[derive(Debug, Default, PartialEq, Eq)]
enum DcsScan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC, and the sequence it opens is still one byte away.
	///
	/// `after_string` is what makes the terminator work: it is true when this ESC ENDED a control
	/// string whose payload is still buffered, so a `\` completes that string and anything else
	/// abandons it. The engine cannot tell those two apart — its `unhook` fires either way — and cmote
	/// deliberately can: a string that named no terminator is malformed and goes unanswered, which is
	/// §54's rule and §60's (an invented answer is worse than a missing one).
	Escape { after_string: bool },
	/// Saw `ESC` and at least one intermediate byte; a final byte now ends the escape sequence.
	EscapeIntermediate,
	/// Inside `ESC P …`, reading the introducer up to its final byte.
	Introducer,
	/// Inside a string's payload, collecting it until the terminator.
	Payload,
}

/// Cuts control strings and escape sequences out of shell output, once, for every scanner that reads
/// them.
///
/// `CAP` is the longest payload this framer will buffer; past it the string is abandoned, so a hostile
/// or broken stream cannot grow cmote's memory without bound (§12). A const parameter rather than a
/// field for the reasons `osc::Framer` gives: the scanners built on it keep deriving `Default`, and
/// each caller's limit stays a named constant in the module that chose it. They genuinely differ — a
/// DECRQSS selector is two bytes and a photograph is megabytes.
///
/// `CAP` of zero is a framer that keeps no payload at all, which is exactly what the four scanners
/// watching for RIS want. They read [`Control::Escape`] and nothing else, and a string's payload never
/// enters their memory.
///
/// Safe at any chunk boundary: the state carries over between `feed` calls, so a sequence may be split
/// anywhere — between the ESC and the `P`, inside the parameters, mid-payload, or between the ESC and
/// the `\` of the terminator.
#[derive(Debug, Default)]
pub struct Framer<const CAP: usize> {
	state: DcsScan,
	marker: Option<u8>,
	params: Params,
	intermediates: Vec<u8>,
	/// The introducer's final byte, kept because the string is not reported until its terminator
	/// arrives — which may be in a later chunk.
	final_byte: u8,
	payload: Vec<u8>,
	/// Where in THIS chunk the ESC being resolved sat, for the [`Span::start`] of an escape sequence.
	/// `None` when it arrived in an earlier chunk, which is what a `None` records everywhere here.
	escape_at: Option<usize>,
	/// The same for the ESC that opened the string being read. Kept apart from `escape_at` because a
	/// string is terminated by a SECOND escape sequence, whose ESC would otherwise overwrite the
	/// string's own start and hand a scanner an offset from the wrong end of the payload.
	string_at: Option<usize>,
}

impl<const CAP: usize> Framer<CAP> {
	/// Feed a chunk of shell output, calling `on_control` once per sequence that COMPLETES in it.
	///
	/// The [`Span`] says where in THIS `bytes` slice the sequence sat — `past` it for a string, and the
	/// ESC that opened it for an escape sequence, which is what a caller acting BEFORE the bytes reach
	/// the engine needs (`graphics` and its RIS).
	pub fn feed(&mut self, bytes: &[u8], mut on_control: impl FnMut(Span, &Control<'_>)) {
		// Anything still open from the last chunk began before this one did.
		self.escape_at = None;
		self.string_at = None;
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				DcsScan::Text => {
					if byte == ESC {
						self.open_escape(index, false);
					}
				}
				DcsScan::Escape { after_string } => {
					self.after_escape(index, byte, after_string, &mut on_control);
				}
				DcsScan::EscapeIntermediate => match byte {
					0x20..=0x2f => self.collect_intermediate(byte),
					0x30..=0x7e => {
						self.report_escape(index, byte, &mut on_control);
						self.state = DcsScan::Text;
					}
					// A fresh ESC restarts the match, and no string is open to complete: whatever was
					// being read here was an escape sequence, not a terminator.
					ESC => self.open_escape(index, false),
					// Read through, keeping the sequence — the engine's escape-intermediate state
					// executes a C0 and stays where it is (`lib.rs:392`).
					byte if passes_through(byte) => {}
					// CAN and SUB, the only two bytes that really cancel a sequence in flight.
					_ => self.state = DcsScan::Text,
				},
				DcsScan::Introducer => self.in_introducer(index, byte),
				DcsScan::Payload => match byte {
					// ESC ends the string — cleanly if a `\` follows it, and not otherwise.
					ESC => self.open_escape(index, true),
					// The single-byte ST, and the BEL cmote accepts beside it.
					ST | BEL => self.complete(index + 1, &mut on_control),
					// CAN and SUB: the engine unhooks the string and drops to ground, so the payload is
					// abandoned and nothing here is reported. Both of cmote's own DCS machines used to
					// read these two straight into the payload and go on waiting for a terminator (§111).
					0x18 | 0x1a => self.abandon(),
					// DEL and the high bytes are DISCARDED by the engine rather than put into the
					// payload (`lib.rs:330`, `:335`), so keeping them would hand a scanner a payload the
					// engine's own handler was never given. `0x9c` is the ST above, not one of these.
					0x7f..=0xff => {}
					_ => {
						self.payload.push(byte);
						if self.payload.len() > CAP {
							// Past the cap. Abandoning is enough: the only byte that can start anything
							// is an ESC, and hunting for one is what the text state does — see the
							// module header on why no "follow it to the end" state is needed.
							self.abandon();
						}
					}
				},
			}
		}
	}

	/// Read `byte` as the one that FOLLOWS an ESC. `after_string` says whether that ESC ended a string
	/// whose payload is still in hand.
	///
	/// **ESC does two jobs at once**, which is the whole reason this is one place rather than an arm of
	/// each state: it ENDS whatever control string is open AND it OPENS the next sequence. `vte` does
	/// both, and a machine that did only the first went deaf for exactly the sequence that followed —
	/// two of §111's defects, one in each of the modules this replaces, the worse of them losing a RIS
	/// that arrived inside a sixel payload.
	fn after_escape(
		&mut self,
		index: usize,
		byte: u8,
		after_string: bool,
		on_control: &mut impl FnMut(Span, &Control<'_>),
	) {
		match byte {
			// `ESC \` — ST. It completes the string this ESC ended, if there was one; on its own it is
			// an escape sequence like any other, which is what the engine dispatches it as.
			b'\\' if after_string => self.complete(index + 1, on_control),
			// A DCS. Whatever was open is abandoned, which is what the engine's `unhook` already did.
			b'P' => {
				self.marker = None;
				self.params.clear();
				self.intermediates.clear();
				self.payload.clear();
				self.string_at = self.escape_at;
				self.state = DcsScan::Introducer;
			}
			// The other families' doors: a CSI, an OSC, and the SOS / PM / APC strings. Each is left to
			// whoever reads it and this machine goes back to hunting for an ESC — safe because an ESC is
			// the only thing that can interrupt any of them (see the module header).
			b'[' | b']' | b'X' | b'^' | b'_' => self.state = DcsScan::Text,
			// ESC ESC: still waiting for the sequence's real first byte, and it is the LATEST ESC that
			// opens it. `after_string` survives the pair, because the engine keeps the payload it had
			// across one too (§111, measured).
			ESC => self.open_escape(index, after_string),
			// An intermediate: this is a longer escape sequence, so any string is abandoned here.
			0x20..=0x2f => {
				self.intermediates.clear();
				self.intermediates.push(byte);
				self.state = DcsScan::EscapeIntermediate;
			}
			// Every other final byte is an escape sequence of its own — RIS among them.
			0x30..=0x7e => {
				self.intermediates.clear();
				self.report_escape(index, byte, on_control);
				self.state = DcsScan::Text;
			}
			// Read through, staying in the escape state: the engine executes a C0 here and does not
			// leave (`lib.rs:341`), so `ESC` LF `c` really is a reset. A string stays open across it,
			// which means `ESC` NUL `\` still terminates one — the ST was dispatched, and adjacency was
			// never what made it a terminator.
			byte if passes_through(byte) => {}
			// CAN and SUB drop the escape back to GROUND, where nothing is being read at all.
			_ => self.abandon(),
		}
	}

	/// Read `byte` as part of a string's introducer — the CSI grammar over again, with the two refusals
	/// `csi::Framer` spells out at more length.
	fn in_introducer(&mut self, index: usize, byte: u8) {
		match byte {
			// Parameter bytes: digits and separators, plus the private markers (`< = > ?`, 0x3c–0x3f)
			// which are only legal as the very first one.
			0x30..=0x3f => {
				if !self.intermediates.is_empty() {
					// A parameter byte AFTER an intermediate. `vte`'s DCS-intermediate state goes
					// straight to `DcsIgnore` (`lib.rs:291`), so the whole string is thrown away there
					// and carrying on here would mean acting alone on a spelling nothing else obeys.
					self.state = DcsScan::Text;
				} else if byte >= 0x3c {
					// A private marker. Legal only before any parameter byte; after that the engine
					// drops the string (`lib.rs:309`).
					if self.params.started() || self.marker.is_some() {
						self.state = DcsScan::Text;
					} else {
						self.marker = Some(byte);
					}
				} else if !self.params.push(byte) {
					// More parameters than the engine's array holds, so it sets `ignoring` and its
					// handler never sees the string. Giving up here is what makes the two agree.
					self.state = DcsScan::Text;
				}
			}
			0x20..=0x2f => self.collect_intermediate(byte),
			// The final byte ends the introducer and opens the payload.
			0x40..=0x7e => {
				// Close the last parameter first: it never met a separator, so this is where an
				// all-zero one gets its digit back.
				self.params.finish();
				self.final_byte = byte;
				self.payload.clear();
				self.state = DcsScan::Payload;
			}
			// A fresh ESC restarts the match. Nothing is open to complete: the payload never started.
			ESC => self.open_escape(index, false),
			byte if passes_through(byte) => {}
			_ => self.state = DcsScan::Text,
		}
	}

	/// Start reading an escape sequence at `index`, remembering where its ESC sat.
	fn open_escape(&mut self, index: usize, after_string: bool) {
		self.escape_at = Some(index);
		self.state = DcsScan::Escape { after_string };
	}

	/// Collect one intermediate byte, abandoning the sequence if the run outgrows the bound.
	fn collect_intermediate(&mut self, byte: u8) {
		self.intermediates.push(byte);
		if self.intermediates.len() > MAX_INTERMEDIATES {
			self.state = DcsScan::Text;
		}
	}

	/// Hand a finished escape sequence to the caller, at the offset of the ESC that opened it.
	fn report_escape(
		&mut self,
		index: usize,
		final_byte: u8,
		on_control: &mut impl FnMut(Span, &Control<'_>),
	) {
		on_control(
			Span::new(index + 1, self.escape_at),
			&Control::Escape(Escape {
				intermediates: &self.intermediates,
				final_byte,
			}),
		);
	}

	/// Hand a finished control string to the caller and go back to hunting.
	fn complete(&mut self, past: usize, on_control: &mut impl FnMut(Span, &Control<'_>)) {
		on_control(
			Span::new(past, self.string_at),
			&Control::String(Dcs {
				marker: self.marker,
				params: &self.params,
				intermediates: &self.intermediates,
				final_byte: self.final_byte,
				payload: &self.payload,
			}),
		);
		self.abandon();
	}

	/// Drop whatever was being read and hunt for the next ESC.
	///
	/// The payload is REPLACED rather than cleared, which is the one place this differs from
	/// `osc::Framer`: a sixel's is megabytes, and keeping that capacity alive for the rest of the
	/// session to save one allocation per picture is the wrong trade (§12).
	fn abandon(&mut self) {
		self.state = DcsScan::Text;
		self.payload = Vec::new();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// What one chunk completed, rendered as something a test can compare: a string as
	/// `marker/params/intermediates/final:payload` with its `past` offset, an escape sequence as
	/// `ESC intermediates final` with the offset of its ESC.
	fn frame<const CAP: usize>(framer: &mut Framer<CAP>, bytes: &[u8]) -> Vec<(usize, String)> {
		let mut seen = Vec::new();
		framer.feed(bytes, |span, control| match control {
			Control::String(dcs) => {
				let marker = dcs.marker().map_or('-', char::from);
				seen.push((
					span.past(),
					format!(
						"{marker}{}{}{}:{}",
						String::from_utf8_lossy(dcs.params.bytes()),
						String::from_utf8_lossy(dcs.intermediates()),
						char::from(dcs.final_byte()),
						String::from_utf8_lossy(dcs.payload())
					),
				));
			}
			Control::Escape(escape) => seen.push((
				span.start(),
				format!(
					"ESC {}{}",
					String::from_utf8_lossy(escape.intermediates()),
					char::from(escape.final_byte())
				),
			)),
		});
		seen
	}

	/// Feed one slice to a fresh framer with a generous cap.
	fn all(bytes: &[u8]) -> Vec<(usize, String)> {
		frame(&mut Framer::<256>::default(), bytes)
	}

	/// Just the descriptions, for the cases where the offset is not what is under test.
	fn what(bytes: &[u8]) -> Vec<String> {
		all(bytes).into_iter().map(|(_, what)| what).collect()
	}

	/// Only the control STRINGS, for the tests whose subject is whether one was read at all. A stray
	/// `ESC \` is reported as an escape sequence — see [`Control`] for when that happens and why — and
	/// a test about strings should not have to spell it.
	fn strings(bytes: &[u8]) -> Vec<String> {
		what(bytes)
			.into_iter()
			.filter(|what| !what.starts_with("ESC "))
			.collect()
	}

	#[test]
	fn a_string_reports_its_introducer_and_payload() {
		// DECRQSS: one intermediate, no parameters, and the selector as the payload. The offset is past
		// the terminator's second byte — `ESC P $ q m` is 5 bytes, `ESC \` two more.
		assert_eq!(all(b"\x1bP$qm\x1b\\"), vec![(7, "-$q:m".to_owned())]);
		// A sixel: parameters, no intermediate, and a payload that is not text.
		assert_eq!(what(b"\x1bP0;1;0q#0~~\x1b\\"), vec!["-0;1;0q:#0~~"]);
		// XTGETTCAP, whose payload is a `;`-separated list of hex names.
		assert_eq!(what(b"\x1bP+q544E;436F\x1b\\"), vec!["-+q:544E;436F"]);
	}

	#[test]
	fn every_terminator_the_engine_takes_and_the_one_it_does_not() {
		// `ESC \`, and the single-byte ST that is its C1 form — both the engine's (`lib.rs:331`).
		assert_eq!(what(b"\x1bP$qm\x1b\\"), vec!["-$q:m"]);
		assert_eq!(what(b"\x1bP$qm\x9c"), vec!["-$q:m"]);
		// BEL, which the engine reads as a payload byte and cmote takes as a terminator anyway. The
		// leniency is deliberate and its whole cost is one byte of timing — see the constant.
		assert_eq!(what(b"\x1bP$qm\x07"), vec!["-$q:m"]);
		// An unterminated string is not reported at all, and neither is one that named no terminator.
		assert!(what(b"\x1bP$qm").is_empty());
		assert_eq!(
			what(b"\x1bP$qm\x1bc"),
			vec!["ESC c"],
			"the ESC ended the string unanswered and opened a reset"
		);
	}

	#[test]
	fn can_and_sub_end_a_string_the_way_the_engine_ends_it() {
		// The engine unhooks the string and drops to ground (`lib.rs:320-324`). Both of the machines
		// this framer replaces read these two into the payload and went on waiting for a terminator, so
		// a later ST would have completed a string the engine had thrown away hundreds of bytes back.
		for cancel in [0x18_u8, 0x1a] {
			let bytes = [ESC, b'P', b'$', b'q', b'm', cancel, ESC, b'\\'];
			assert!(
				strings(&bytes).is_empty(),
				"{cancel:#04x}: the string is abandoned, and the ST after it completes nothing"
			);
			// The engine's reading of those same bytes, stated so the two are pinned side by side: the
			// string went at the cancel, so what is left is a stray ST — which both sides read as an
			// escape sequence of its own.
			assert_eq!(what(&bytes), vec!["ESC \\"]);
		}
	}

	#[test]
	fn the_payload_is_what_the_engine_would_have_been_given() {
		// DEL and the high bytes are discarded by the parser rather than put (`lib.rs:330`, `:335`), so
		// a scanner comparing a payload against a known selector has to see them gone. `0x9c` is the
		// terminator, not a discarded byte, which is what ends this one.
		assert_eq!(what(b"\x1bP$qm\x7f\x80\xff\x1b\\"), vec!["-$q:m"]);
		// A C0 the parser DOES put, so it stays in the payload.
		assert_eq!(what(b"\x1bP$qm\x01\x1b\\"), vec!["-$q:m\u{1}"]);
	}

	#[test]
	fn an_escape_sequence_is_reported_with_its_intermediates() {
		// RIS, at the offset of its own ESC — what a caller acting before the bytes reach the engine
		// needs. `text` is four bytes, so the ESC sits at 4.
		assert_eq!(all(b"text\x1bc"), vec![(4, "ESC c".to_owned())]);
		// The charset designation that shares its final byte. A scanner testing the final byte alone
		// would read this as a reset, which is why `Escape` reports both parts.
		assert_eq!(what(b"\x1b(c"), vec!["ESC (c"]);
		// A stray ST with no string open is an escape sequence like any other — the engine dispatches
		// it as one (`lib.rs:368`).
		assert_eq!(what(b"\x1b\\"), vec!["ESC \\"]);
	}

	#[test]
	fn the_bytes_the_engine_reads_through_do_not_lose_the_sequence() {
		// The escape state executes a C0 and stays there (`lib.rs:341`), ignores DEL and every byte past
		// `0x7f` (`:381-383`). So each of these is still a reset, and this is the rule four hand-rolled
		// `after_escape` watchers were getting wrong (§111).
		for byte in [0x00_u8, b'\n', 0x1f, 0x7f, 0x80, 0x9c] {
			let bytes = [ESC, byte, b'c'];
			assert_eq!(what(&bytes), vec!["ESC c"], "{byte:#04x} between ESC and c");
		}
		// The same rule inside an introducer, where the engine also keeps the sequence (`lib.rs:258`).
		for byte in [0x00_u8, b'\n', 0x7f, 0x80] {
			let bytes = [ESC, b'P', b'$', byte, b'q', b'm', ESC, b'\\'];
			assert_eq!(
				what(&bytes),
				vec!["-$q:m"],
				"{byte:#04x} inside the introducer"
			);
		}
		// And CAN and SUB, which really do cancel: an escape they end leaves nothing being read.
		for cancel in [0x18_u8, 0x1a] {
			let bytes = [ESC, cancel, b'c'];
			assert!(what(&bytes).is_empty(), "{cancel:#04x} cancels the escape");
		}
	}

	#[test]
	fn an_escape_ends_a_string_and_opens_the_next_sequence() {
		// Both jobs at once, which is the defect §111 found in two modules at the same time. The reset
		// has to arrive even though it interrupted a payload — that is what makes it a reset.
		assert_eq!(
			what(b"\x1bP0q#0~~\x1bc"),
			vec!["ESC c"],
			"the picture is abandoned and the reset is reported"
		);
		// A second string opening inside the first one's payload.
		assert_eq!(what(b"\x1bP0q#0\x1bP$qm\x1b\\"), vec!["-$q:m"]);
		// ESC ESC inside a payload: still waiting for the terminator's `\`, and the payload survives the
		// pair — which is what the engine does with it (§111, measured).
		assert_eq!(what(b"\x1bP$qm\x1b\x1b\\"), vec!["-$q:m"]);
		// A byte the engine reads through between the ESC and the `\`. The ST was still dispatched, so
		// the string was still terminated: adjacency was never what made it one.
		assert_eq!(what(b"\x1bP$qm\x1b\x00\\"), vec!["-$q:m"]);
	}

	#[test]
	fn the_grammar_refuses_what_the_engine_refuses() {
		// A parameter byte after an intermediate, and a private marker after the parameters started.
		// Both send the engine's parser to `DcsIgnore` (`lib.rs:291`, `:309`), so neither string is
		// reported here either.
		assert!(strings(b"\x1bP$1qm\x1b\\").is_empty());
		assert!(strings(b"\x1bP1?qm\x1b\\").is_empty());
		// A marker BEFORE the parameters is legal, and reported as one rather than folded into them.
		assert_eq!(strings(b"\x1bP?1qm\x1b\\"), vec!["?1q:m"]);
		// More parameters than the engine's array holds: it sets `ignoring` and its handler never sees
		// the string, so cmote must not read one either.
		let mut bytes = vec![ESC, b'P'];
		bytes.extend(std::iter::repeat_n(b';', super::super::csi::MAX_PARAMS));
		bytes.extend_from_slice(b"qm\x1b\\");
		assert!(strings(&bytes).is_empty());
		// One more intermediate than this framer will hold.
		let mut bytes = vec![ESC, b'P'];
		bytes.extend(std::iter::repeat_n(b'$', MAX_INTERMEDIATES + 1));
		bytes.extend_from_slice(b"qm\x1b\\");
		assert!(strings(&bytes).is_empty());
	}

	#[test]
	fn a_string_says_how_many_parameters_it_carried() {
		// Both DCS queries cmote answers are defined with no parameters at all, so the count is what
		// tells `DCS $ q` from `DCS 1 $ q` — and an EMPTY parameter still counts as one, which is why
		// this is not "is the first one absent" (see `Params::count`).
		let counts = |bytes: &[u8]| {
			let mut seen = Vec::new();
			Framer::<64>::default().feed(bytes, |_, control| {
				if let Control::String(dcs) = control {
					seen.push(dcs.param_count());
				}
			});
			seen
		};
		assert_eq!(counts(b"\x1bP$qm\x1b\\"), vec![0]);
		assert_eq!(counts(b"\x1bP1$qm\x1b\\"), vec![1]);
		assert_eq!(counts(b"\x1bP;$qm\x1b\\"), vec![2]);
	}

	#[test]
	fn a_payload_past_the_cap_is_abandoned_and_the_next_sequence_is_not() {
		// The cap is the whole of §12 here: a remote can send a payload for ever, and a framer that
		// buffered it would grow cmote's memory until it died.
		let mut framer = Framer::<4>::default();
		let over = frame(&mut framer, b"\x1bP$qmmmmmm\x1b\\");
		assert_eq!(
			over,
			vec![(10, "ESC \\".to_owned())],
			"six payload bytes past a cap of four: no string, and the terminator reads as the stray ST \
			 it now is"
		);
		// And the framer is left hunting, not stuck: the next string is read normally.
		assert_eq!(
			frame(&mut framer, b"\x1bP$qm\x1b\\"),
			vec![(7, "-$q:m".to_owned())]
		);
		// Exactly at the cap is kept — the bound is what it says.
		assert_eq!(
			frame(&mut Framer::<4>::default(), b"\x1bP$qmmmm\x1b\\"),
			vec![(10, "-$q:mmmm".to_owned())]
		);
	}

	#[test]
	fn a_cap_of_zero_reads_escape_sequences_and_no_payload_at_all() {
		// What the four RIS watchers want: the escape sequences, and not one byte of any picture a
		// remote sends.
		let mut framer = Framer::<0>::default();
		assert_eq!(
			frame(&mut framer, b"\x1bP0q#0~~\x1b\\\x1bc"),
			vec![(8, "ESC \\".to_owned()), (10, "ESC c".to_owned())],
			"the sixel is dropped on its first payload byte, its terminator reads as a stray ST, and the \
			 reset still arrives"
		);
	}

	#[test]
	fn the_other_families_doors_are_left_to_whoever_reads_them() {
		// A CSI, an OSC and an APC string are not this framer's, and none of them is reported. Nor is
		// anything inside them mistaken for a sequence: an ESC is the only thing that can interrupt one,
		// and an ESC ends it for the engine too (§111).
		assert!(what(b"\x1b[2J").is_empty());
		assert!(what(b"\x1b]7;file:///tmp\x07").is_empty());
		assert_eq!(
			what(b"\x1b_Gf=100\x1b\\"),
			vec!["ESC \\"],
			"kitty graphics, APC: the payload is nothing to this framer, and its ST is a stray one"
		);
		// The sequence AFTER one of those is read normally, which is the part that matters.
		assert_eq!(what(b"\x1b]7;/tmp\x07\x1bc"), vec!["ESC c"]);
		assert_eq!(what(b"\x1b[2J\x1bP$qm\x1b\\"), vec!["-$q:m"]);
	}

	#[test]
	fn a_sequence_split_across_chunks_completes_on_the_chunk_that_finishes_it() {
		// Output arrives in arbitrary chunks, and every split has to be safe: between the ESC and the
		// `P`, inside the introducer, mid-payload, and between the ESC and the `\`.
		let mut framer = Framer::<256>::default();
		for chunk in [&b"\x1b"[..], b"P", b"$", b"q", b"m", b"\x1b"] {
			assert!(frame(&mut framer, chunk).is_empty());
		}
		assert_eq!(
			frame(&mut framer, b"\\"),
			vec![(1, "-$q:m".to_owned())],
			"the offset is measured in the chunk that completed it"
		);
	}

	#[test]
	fn an_offset_from_an_earlier_chunk_reads_as_the_front_of_this_one() {
		// A sequence that began in a previous chunk has no start offset in this one, and 0 is not an
		// approximation: everything before it really has been fed already.
		let mut framer = Framer::<256>::default();
		assert!(frame(&mut framer, b"text\x1b").is_empty());
		assert_eq!(frame(&mut framer, b"c"), vec![(0, "ESC c".to_owned())]);
	}
}

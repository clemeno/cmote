// term/differential.rs — feed the same bytes to cmote's scanners and to the engine's own parser, and
// compare what each of them did (§106).
//
// Every scanner in this directory reads the byte stream a second time, beside the engine. That tactic
// only works while the two READ THE SAME WAY: cmote acts on a sequence because it expects the engine to
// have acted (or to have ignored it), and wherever the two disagree about whether a sequence is even
// well formed, one of them acts alone. Three such disagreements shipped, and all three were found by
// hand — by reading `vte`'s source and noticing that a constant counted the wrong thing.
//
// A fourth was found by THIS module, on the first run of it: a control byte arriving mid-sequence, which
// the engine runs where it sits before carrying on with the sequence, and which every scanner here used
// to give up on. Nobody had thought to ask. That is the argument for the module in one line.
//
// Reading a crate by hand does not scale and does not survive a version bump. This module asks the
// question directly instead: `vte::Parser` is re-exported by `alacritty_terminal`, so a test can drive
// the ACTUAL parser the engine is built on, record what it dispatched, and compare that against what
// cmote's scanner made of the same bytes. No new dependency, and the answers come from the crate rather
// than from a note about the crate.
//
// The tests here are of two kinds, and the second kind is the point:
//
//   * AGREEMENTS. The three defects §106 fixed, pinned from both sides at once, so a regression shows up
//     as "the engine acted and cmote did not" rather than as a subtle behaviour change nobody notices.
//   * DIVERGENCES. Places where the two still read differently, asserted AS THEY BEHAVE TODAY, each with
//     what it costs. These are not approvals. They are the inventory §106's Not done list describes,
//     written as code so that the framer flips them deliberately and cannot land while pretending they
//     were never there.

use alacritty_terminal::vte;

/// One CSI the engine's parser handed on, in the shape its dispatch receives it.
#[derive(Debug, PartialEq, Eq)]
struct Csi {
	/// Each parameter with its sub-parameters, exactly as the parser grouped them.
	params: Vec<Vec<u16>>,
	/// The intermediates, which for this parser INCLUDES the private marker.
	intermediates: Vec<u8>,
	final_byte: char,
}

/// One control string the parser hooked, in the shape its `hook` receives it.
#[derive(Debug, PartialEq, Eq)]
struct Hook {
	/// The intermediates, which for this parser includes the private marker.
	intermediates: Vec<u8>,
	final_byte: char,
}

/// What the engine's parser made of a chunk: what it dispatched, what it refused, and which control
/// bytes it ran on the way.
#[derive(Debug, Default)]
struct EngineTrace {
	/// Every CSI dispatched with `ignore` clear — the ones the engine's handler actually sees.
	dispatched: Vec<Csi>,
	/// How many arrived with `ignore` set, which is the parser saying "too many parameters or
	/// intermediates, do not act on this". `ansi.rs` drops those.
	ignored: usize,
	/// Control bytes executed, including any that arrived INSIDE a sequence — which this parser runs
	/// without abandoning the sequence around them.
	executed: Vec<u8>,
	/// Every control string the parser hooked with `ignore` clear (§111).
	hooked: Vec<Hook>,
	/// The payload bytes it was GIVEN — which is not every byte between the final byte and the
	/// terminator: DEL and the high bytes are discarded on the way (`lib.rs:330`, `:335`).
	put: Vec<u8>,
	/// How many strings it unhooked. One per string that ENDED, however it ended — the parser cannot
	/// tell a clean terminator from an interrupted one, and cmote deliberately can (§54, §60).
	unhooked: usize,
	/// Every escape sequence dispatched with `ignore` clear: `ESC c`, `ESC ( B`, a stray ST.
	escapes: EscapeTrace,
	/// Every OSC string dispatched, as its parameters rejoined on `;` — which is the shape
	/// `osc::Framer` hands its callers, so the two can be compared directly (§111).
	oscs: Vec<Vec<u8>>,
}

impl vte::Perform for EngineTrace {
	fn csi_dispatch(
		&mut self,
		params: &vte::Params,
		intermediates: &[u8],
		ignore: bool,
		action: char,
	) {
		if ignore {
			self.ignored += 1;
			return;
		}
		self.dispatched.push(Csi {
			params: params.iter().map(<[u16]>::to_vec).collect(),
			intermediates: intermediates.to_vec(),
			final_byte: action,
		});
	}

	fn execute(&mut self, byte: u8) {
		self.executed.push(byte);
	}

	fn hook(&mut self, _params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
		if ignore {
			self.ignored += 1;
			return;
		}
		self.hooked.push(Hook {
			intermediates: intermediates.to_vec(),
			final_byte: action,
		});
	}

	fn put(&mut self, byte: u8) {
		self.put.push(byte);
	}

	fn unhook(&mut self) {
		self.unhooked += 1;
	}

	fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
		if ignore {
			self.ignored += 1;
			return;
		}
		self.escapes.push((intermediates.to_vec(), byte));
	}

	fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
		self.oscs.push(params.join(&b';'));
	}
}

/// Drive the real parser over `bytes` and report what it did.
fn engine(bytes: &[u8]) -> EngineTrace {
	let mut parser = vte::Parser::new();
	let mut seen = EngineTrace::default();
	parser.advance(&mut seen, bytes);
	seen
}

impl EngineTrace {
	/// Whether the parser dispatched a plain CSI — no marker, no intermediate — ending in `final_byte`,
	/// and with `first` as its first parameter. That is the shape every scanner here shadows.
	fn dispatched_plain(&self, final_byte: char, first: u16) -> bool {
		self.dispatched.iter().any(|csi| {
			csi.final_byte == final_byte
				&& csi.intermediates.is_empty()
				&& csi.params.first().and_then(|param| param.first()) == Some(&first)
		})
	}
}

/// One scanner's claim on one sequence: what to call it in a failure message, the bytes themselves, and
/// how to ask that scanner whether it claimed them.
///
/// Named types because the ten scanners answer with ten different verdicts, and the only question
/// common to all of them is "did you act on this at all".
#[cfg(test)]
type DifferentialClaim = (&'static str, &'static [u8], fn(&[u8]) -> bool);

/// The same, plus where in the sequence a stray byte is to be inserted.
#[cfg(test)]
type ClaimAt = (&'static str, &'static [u8], usize, fn(&[u8]) -> bool);

/// Every escape sequence one side saw, as its intermediates and its final byte (§111).
#[cfg(test)]
type EscapeTrace = Vec<(Vec<u8>, u8)>;

/// What one side made of the control strings in a chunk: the introducers it reported, the payload it
/// kept, and the escape sequences it saw beside them.
#[cfg(test)]
type StringTrace = (Vec<Hook>, Vec<u8>, EscapeTrace);

/// The bytes the engine reads through without ending the sequence, each with a name for the failure
/// message. Deliberately one of each KIND rather than all 30-odd values: NUL and LF are C0s it
/// executes, US is the top of that range, DEL is the one it ignores, and the two high bytes are the
/// `anywhere` fall-through — including `0x9c`, which is ST as a single byte and the most plausible one
/// to arrive by accident.
///
/// One list for the whole module, because the rule holds in more than one state: `vte` reads these
/// through inside a CSI, between the ESC and the byte that says which sequence it is, and inside a
/// DCS's introducer. Three states, one set of bytes (§111).
#[cfg(test)]
const READ_THROUGH: [(u8, &str); 6] = [
	(0x00, "NUL"),
	(b'\n', "LF"),
	(0x1f, "US"),
	(0x7f, "DEL"),
	(0x80, "high"),
	(0x9c, "ST8"),
];

/// Every spelling of one sequence that leaves the ENGINE's reading of it unchanged (§106).
///
/// The six hand-written tests below confirm the four defects are fixed. This is the part that goes
/// looking: it takes a sequence a scanner claims, rewrites its parameter region every way the engine is
/// indifferent to, and hands back each variant with a name to fail by.
///
/// Two kinds of rewrite, and both were a live defect before §106:
///
///   * **Padding.** Leading zeros do not change a number, so `CSI 2 J` and `CSI 002 J` are one sequence
///     as far as the engine is concerned.
///   * **Interruption.** A byte the CSI grammar does not claim but which the engine reads straight
///     through — a C0 it runs where it sits, DEL, anything past `0x7f` — inserted at every position in
///     the parameter region, one at a time.
///
/// The point of generating rather than listing is that neither of those defects was found by thinking of
/// a case. Both were found afterwards, by asking what else was in the same family.
fn variants(params: &[u8], final_byte: u8) -> Vec<(String, Vec<u8>)> {
	let sequence = |params: &[u8]| {
		let mut bytes = vec![0x1b, b'['];
		bytes.extend_from_slice(params);
		bytes.push(final_byte);
		bytes
	};

	let mut out = vec![("plain".to_owned(), sequence(params))];

	// Padding: each field zero-padded to four digits, then the whole run padded at its head.
	let padded: Vec<u8> = params
		.split(|&byte| byte == b';')
		.map(|field| {
			let mut field = field.to_vec();
			for _ in 0..3 {
				field.insert(0, b'0');
			}
			field
		})
		.collect::<Vec<_>>()
		.join(&b';');
	out.push(("every field padded".to_owned(), sequence(&padded)));

	// Interruption: one read-through byte at each position inside the parameter region, including both
	// ends of it — the boundary cases, since a byte right after the `[` or right before the final byte is
	// where an off-by-one in the state machine shows up.
	for (byte, name) in READ_THROUGH {
		for at in 0..=params.len() {
			let mut interrupted = params.to_vec();
			interrupted.insert(at, byte);
			out.push((format!("{name} at {at}"), sequence(&interrupted)));
		}
	}
	out
}

/// Every SHAPE a CSI can have, across the parts that decide which sequence it is (§106).
///
/// [`variants`] rewrites a sequence's spelling and keeps its shape. This does the opposite: it walks the
/// shape space — private marker × intermediates × parameter run × final byte — and leaves the spelling
/// alone. The two axes found different defects, which is the argument for having both: a padded parameter
/// was a spelling bug, and a parameter byte after an intermediate was a shape bug.
///
/// The final bytes are the ones cmote's own scanners claim, so the sweep is aimed at the sequences where
/// acting alone would matter rather than at the whole alphabet.
#[cfg(test)]
fn shapes() -> Vec<(String, Vec<u8>)> {
	const MARKERS: [Option<u8>; 5] = [None, Some(b'?'), Some(b'>'), Some(b'<'), Some(b'=')];
	/// Including runs of two and three, which is past the engine's `MAX_INTERMEDIATES` of 2 once the
	/// private marker is counted against it as the engine does — the case cmote's own limit of 4 lets
	/// through, and the one this sweep is here to keep honest.
	const INTERMEDIATES: [&[u8]; 9] = [b"", b" ", b"\"", b"#", b"$", b"!", b"'", b" \"", b"\"$#"];
	const PARAMS: [&[u8]; 4] = [b"", b"1", b"1;2", b"1:2"];
	/// Every final byte any scanner in this directory watches for. `c` and `S` joined when `query` did
	/// (§111): they are DA3 and XTSMGRAPHICS, two of the three private CSI forms it answers, and a
	/// sweep that never spelled them could not have caught it acting on one alone. `|` joined with
	/// `locator` (§140), and the `'` above with it — `z` and `{` were already here for `rect` and
	/// `sgrstack`, which is exactly why the intermediate had to come too: DECELR and DECERA differ only
	/// in it, and a sweep blind to `'` could not tell one scanner claiming another's sequence from a
	/// scanner minding its own.
	const FINALS: &[u8] = b"smqpJKWk{}zncS|";

	/// Where the parts are put relative to each other. The first is the only WELL-FORMED order; the other
	/// two are the ones the engine refuses, and they have to be generated deliberately because a generator
	/// that only emits well-formed sequences cannot catch cmote obeying a malformed one.
	///
	/// That is not hypothetical. This sweep was written with the well-formed order alone, passed, and was
	/// then run against §106's ordering fix reverted — where it passed again. A green test that cannot fail
	/// is worse than no test, so the two malformed orders are the reason the sweep exists at all.
	#[derive(Clone, Copy)]
	enum Order {
		/// `CSI <marker> <params> <intermediates> <final>` — the grammar as ECMA-48 defines it.
		WellFormed,
		/// A parameter byte after the intermediates. `vte` drops the whole sequence for this
		/// (`lib.rs:232`), and cmote obeyed it until §106.
		ParamAfterIntermediate,
		/// A private marker after the parameters. `vte` drops that too (`lib.rs:249`).
		MarkerAfterParams,
	}

	let mut out = Vec::new();
	for marker in MARKERS {
		for intermediates in INTERMEDIATES {
			for params in PARAMS {
				for &final_byte in FINALS {
					for order in [
						Order::WellFormed,
						Order::ParamAfterIntermediate,
						Order::MarkerAfterParams,
					] {
						let mut bytes = vec![0x1b, b'['];
						bytes.extend(marker);
						bytes.extend_from_slice(params);
						match order {
							Order::WellFormed => bytes.extend_from_slice(intermediates),
							Order::ParamAfterIntermediate => {
								bytes.extend_from_slice(intermediates);
								bytes.push(b'2');
							}
							Order::MarkerAfterParams => {
								bytes.push(b'?');
								bytes.extend_from_slice(intermediates);
							}
						}
						bytes.push(final_byte);
						let shape = format!(
							"CSI {}{}{}{} {}",
							marker.map_or(' ', char::from),
							String::from_utf8_lossy(params),
							String::from_utf8_lossy(intermediates),
							match order {
								Order::WellFormed => "",
								Order::ParamAfterIntermediate => " +param",
								Order::MarkerAfterParams => " +marker",
							},
							char::from(final_byte)
						);
						out.push((shape, bytes));
					}
				}
			}
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::{cancel::Cancel, graphics, protect};

	/// A DECSLRM padded with leading zeros, the sequence §57 was letting through.
	fn padded_margins() -> Vec<u8> {
		let mut bytes = vec![b'\x1b', b'['];
		bytes.extend(std::iter::repeat_n(b'0', 40));
		bytes.extend_from_slice(b"1;80s");
		bytes
	}

	#[test]
	fn a_padded_margin_request_is_read_by_both() {
		// The engine reads two parameters and dispatches to save-cursor, which is why cmote has to see
		// the sequence: with mode 69 set it cancels that byte. Before §106 only the engine saw it.
		let bytes = padded_margins();
		let engine = engine(&bytes);
		assert!(
			engine.dispatched_plain('s', 1),
			"the engine reads the padding as the number 1 and dispatches"
		);
		assert_eq!(
			engine.ignored, 0,
			"nothing about it made the parser give up"
		);

		let found = Cancel::default().feed(&bytes);
		assert_eq!(found.len(), 1, "and so does cmote, now");
		assert_eq!((found[0].left, found[0].right), (Some(1), Some(80)));
	}

	#[test]
	fn a_long_sgr_is_read_by_both() {
		// Thirty-three parameter bytes. The engine applies the SGR — `Attr::Reset` included, which takes
		// the borrowed protection bit — so cmote must report it to put the bit back.
		let bytes = b"\x1b[0;1;2;3;4;5;7;9;21;30;31;38;5;196m";
		let engine = engine(bytes);
		assert!(engine.dispatched_plain('m', 0), "the engine applies it");
		assert_eq!(engine.ignored, 0);

		let mut scanner = protect::Protect::default();
		scanner.feed(b"\x1b[1\"q");
		assert_eq!(
			scanner.feed(bytes),
			vec![(bytes.len(), protect::ProtectRequest::Reassert)],
			"and cmote reasserts protection across it"
		);
	}

	#[test]
	fn a_padded_erase_is_read_by_both() {
		// ED's parameter reaches the engine through `next_param_or(0)`, so the padding is just zeros and
		// the screen is erased. The pictures on that screen have to go with the text.
		let bytes = b"\x1b[000000000000000002J";
		assert!(engine(bytes).dispatched_plain('J', 2));

		assert!(matches!(
			graphics::Images::default().feed(bytes).as_slice(),
			[(0, graphics::GraphicsEvent::ClearScreen)]
		));
	}

	#[test]
	fn too_many_parameters_are_refused_by_both() {
		// The one bound where giving up is the agreeing answer: past thirty-two parameters the parser
		// sets `ignore`, `ansi.rs` drops the sequence, and cmote drops it too. Neither acts.
		let mut bytes = vec![b'\x1b', b'['];
		bytes.extend(std::iter::repeat_n(b'1', 40).flat_map(|digit| [digit, b';']));
		bytes.push(b's');
		let engine = engine(&bytes);
		assert!(
			engine.dispatched.is_empty(),
			"the engine dispatched nothing it would act on"
		);
		assert!(engine.ignored >= 1, "it refused the sequence explicitly");

		assert!(
			Cancel::default().feed(&bytes).is_empty(),
			"and so does cmote"
		);
	}

	#[test]
	fn a_control_byte_inside_a_csi_is_read_by_both() {
		// This harness found this one on its first run, and it was live: the parser runs a C0 that arrives
		// mid-sequence and CARRIES ON with the sequence around it (`lib.rs:190`, `:230`, `:241`), so
		// `CSI 5;` LF `70 s` executes a line feed and then dispatches the save-cursor — while all three
		// scanners abandoned it. §57's harm by a second route, and nobody had thought to ask.
		let bytes = b"\x1b[5;\n70s";
		let engine = engine(bytes);
		assert_eq!(
			engine.executed,
			vec![b'\n'],
			"the line feed runs where it sits"
		);
		assert!(
			engine.dispatched_plain('s', 5),
			"and the sequence still reaches the handler"
		);

		let found = Cancel::default().feed(bytes);
		assert_eq!(found.len(), 1, "and cmote judges it now");
		assert_eq!((found[0].left, found[0].right), (Some(5), Some(70)));
	}

	#[test]
	fn a_byte_the_engine_reads_through_between_the_escape_and_the_bracket() {
		// The same rule one state EARLIER, and the framer obeyed it only in the later one. `vte`'s escape
		// state executes a C0 and STAYS THERE (`lib.rs:341`), ignores DEL and every byte past 0x7f
		// (`:381-383`), and holds its ground across `ESC ESC` — so `ESC` LF `[ 2 J` erases the screen. The
		// framer dropped to ordinary text on that line feed and then read the `[` as a printable
		// character, which loses the whole sequence for all ten scanners at once.
		//
		// Found by reading the escape state while designing `dcs::Framer`, which has to obey the same rule
		// two doors down (§111).
		for (byte, name) in READ_THROUGH {
			let bytes = [0x1b, byte, b'[', b'2', b'J'];
			assert!(
				engine(&bytes).dispatched_plain('J', 2),
				"{name}: the engine dispatches the erase"
			);
			let found = graphics::Images::default().feed(&bytes);
			assert!(
				matches!(
					found.as_slice(),
					[(_, graphics::GraphicsEvent::ClearScreen)]
				),
				"{name}: and cmote claims it too, found {found:?}"
			);
		}

		// The other half of the rule, and the reason "read through" cannot be softened to "keep going
		// whatever arrives": CAN and SUB drop the escape back to GROUND, where a `[` is a printable
		// character and not the start of anything. Neither side reads an erase here.
		for cancel_byte in [0x18_u8, 0x1a] {
			let bytes = [0x1b, cancel_byte, b'[', b'2', b'J'];
			assert!(
				engine(&bytes).dispatched.is_empty(),
				"a cancelled escape leaves the bracket as text, so the engine dispatches nothing"
			);
			assert!(
				graphics::Images::default().feed(&bytes).is_empty(),
				"and cmote must not claim it either"
			);
		}
	}

	#[test]
	fn every_spelling_of_a_margin_request_is_read_by_both() {
		// The sweep, in the shape the four defects taught: not "does this case work" but "does cmote claim
		// this sequence exactly when the engine dispatches it". Both directions matter — cmote missing one
		// is a cursor overwritten, cmote claiming one the engine threw away is a save-cursor stolen from a
		// program that is entitled to it.
		let cases = variants(b"5;70", b's');
		assert_eq!(
			cases.len(),
			32,
			"1 plain + 1 padded + 6 bytes x 5 positions"
		);
		for (name, bytes) in cases {
			let dispatched = engine(&bytes).dispatched_plain('s', 5);
			let found = Cancel::default().feed(&bytes);
			assert_eq!(
				dispatched,
				found.len() == 1,
				"{name}: the engine dispatched {dispatched}, cmote found {}",
				found.len()
			);
			if dispatched {
				assert_eq!(
					(found[0].left, found[0].right),
					(Some(5), Some(70)),
					"{name}: and it read the same two numbers"
				);
			}
		}
	}

	#[test]
	fn every_spelling_of_an_erase_is_read_by_both() {
		// `CSI 2 J` takes the pictures with the text. Every way of writing it has to, or a picture outlives
		// the screen it was drawn on — and the padded spelling is exactly how that shipped.
		for (name, bytes) in variants(b"2", b'J') {
			let dispatched = engine(&bytes).dispatched_plain('J', 2);
			let found = graphics::Images::default().feed(&bytes);
			let cleared = matches!(
				found.as_slice(),
				[(_, graphics::GraphicsEvent::ClearScreen)]
			);
			assert_eq!(
				dispatched, cleared,
				"{name}: engine {dispatched}, cmote {cleared}"
			);
		}
	}

	#[test]
	fn every_spelling_of_an_sgr_is_reasserted_across() {
		// An SGR the engine applies while DECSCA is armed must be reported, whatever it is spelled like,
		// because `Attr::Reset` assigns the flag word whole and cmote's borrowed protection bit goes with
		// it. Over-reporting is free here (re-asserting a bit that is still set is a no-op); missing one
		// silently unprotects a run.
		for (name, bytes) in variants(b"0;1", b'm') {
			let dispatched = engine(&bytes).dispatched_plain('m', 0);
			let mut scanner = protect::Protect::default();
			scanner.feed(b"\x1b[1\"q");
			let found = scanner.feed(&bytes);
			let reasserted = matches!(found.as_slice(), [(_, protect::ProtectRequest::Reassert)]);
			assert_eq!(
				dispatched, reasserted,
				"{name}: engine {dispatched}, cmote {reasserted}"
			);
		}
	}

	#[test]
	fn no_scanner_changes_its_mind_when_the_bytes_arrive_one_at_a_time() {
		// Every scanner's doc claims it is safe at any chunk boundary, and the engine's parser is safe at one
		// by construction. This is the sweep for that claim, and it is here rather than in each module
		// because the claim is identical in all of them and a boundary bug has already cost this project a
		// release: §104's Ctrl+D rule settled on the first chunk of a two-read answer and shipped broken.
		//
		// Offsets are deliberately not compared — they are measured within the chunk that completed the
		// sequence, so a byte-at-a-time feed reports 0 and a whole-slice feed reports where it sat. What
		// must not change is the VERDICT.
		for (name, bytes) in variants(b"5;70", b's') {
			let whole = Cancel::default()
				.feed(&bytes)
				.iter()
				.map(|request| (request.left, request.right))
				.collect::<Vec<_>>();
			let mut scanner = Cancel::default();
			let split = bytes
				.iter()
				.flat_map(|&byte| scanner.feed(&[byte]))
				.map(|request| (request.left, request.right))
				.collect::<Vec<_>>();
			assert_eq!(whole, split, "margins, {name}");
		}

		for (name, bytes) in variants(b"2", b'J') {
			let cleared = |found: &[(usize, graphics::GraphicsEvent)]| {
				matches!(found, [(_, graphics::GraphicsEvent::ClearScreen)])
			};
			let whole = graphics::Images::default().feed(&bytes);
			let mut scanner = graphics::Images::default();
			let split: Vec<_> = bytes
				.iter()
				.flat_map(|&byte| scanner.feed(&[byte]))
				.collect();
			assert_eq!(
				cleared(&whole),
				cleared(&split),
				"erase, {name}: whole {}, split {}",
				cleared(&whole),
				cleared(&split)
			);
		}

		for (name, bytes) in variants(b"0;1", b'm') {
			let armed = || {
				let mut scanner = protect::Protect::default();
				scanner.feed(b"\x1b[1\"q");
				scanner
			};
			let whole: Vec<_> = armed().feed(&bytes).into_iter().map(|(_, it)| it).collect();
			let mut scanner = armed();
			let split: Vec<_> = bytes
				.iter()
				.flat_map(|&byte| scanner.feed(&[byte]))
				.map(|(_, it)| it)
				.collect();
			assert_eq!(whole, split, "sgr, {name}");
		}
	}

	#[test]
	fn the_scanners_with_no_engine_arm_read_through_a_stray_byte_too() {
		// The eight CSI scanners whose sequences the engine has no live arm for, and the reason this test is
		// here rather than in each of them: the rule they are being held to is the ENGINE's
		// (`csi::passes_through`), and the only thing that makes it their business is that they shadow the
		// same byte stream. (The other three — `cancel`, `protect`, `graphics` — watch sequences the engine
		// really acts on, so they are compared against it directly in the tests above.)
		//
		// The engine has no live arm behind any of these seven sequences, so there is nothing to compare a
		// verdict against — `vte` frames them and `ansi.rs` drops them. What can still be asserted is
		// SELF-CONSISTENCY, and it is worth exactly as much: a stray byte the engine reads through must not
		// change what cmote makes of a sequence, or the two will disagree the moment a version bump fills one
		// of those empty handler bodies. Five of the seven gave up on the line feed before this test existed.
		//
		// `modkeys` and `query` were the last two in, added as §111 moved each of them onto the shared
		// grammar: both were left out at first because neither obeyed the rule at all, and a test asserting
		// what a module does wrong is not a test. The framer is what made the claim true.
		let interrupted = |sequence: &[u8], at: usize| {
			let mut bytes = sequence.to_vec();
			bytes.insert(at, b'\n');
			bytes
		};

		let claims: [ClaimAt; 8] = [
			("dsr, DECXCPR", b"\x1b[?6n", 3, |bytes| {
				!super::super::dsr::Dsr::default().feed(bytes).is_empty()
			}),
			// The QUERY form, because it is the half of this module that answers with bytes — the
			// SET form's verdict is a level rather than a reply, which does not fit the one question
			// all of these can be asked in common.
			("modkeys, XTQMODKEYS", b"\x1b[?4m", 3, |bytes| {
				!super::super::modkeys::ModKeys::default()
					.feed(bytes)
					.is_empty()
			}),
			("tabs, DECST8C", b"\x1b[?5W", 3, |bytes| {
				!super::super::tabs::Tabs::default().feed(bytes).is_empty()
			}),
			("scp, SCP", b"\x1b[2 k", 3, |bytes| {
				!super::super::scp::Scp::default().feed(bytes).is_empty()
			}),
			("query, XTVERSION", b"\x1b[>0q", 3, |bytes| {
				!super::super::query::Queries::default()
					.feed(bytes)
					.is_empty()
			}),
			("sgrstack, XTPUSHSGR", b"\x1b[#{", 2, |bytes| {
				!super::super::sgrstack::SgrStack::default()
					.feed(bytes)
					.is_empty()
			}),
			("rect, DECERA", b"\x1b[2;3;5;7$z", 4, |bytes| {
				!super::super::rect::Rectangles::default()
					.feed(bytes)
					.is_empty()
			}),
			// DECRQLP, the DEC locator's one question (§140). The eighth, and it belongs to this list by
			// the same test as the seven above: `vte` frames the `'` family and `ansi.rs` has no arm for
			// any of it, so there is no engine verdict to compare against and self-consistency is what
			// can be asserted.
			("locator, DECRQLP", b"\x1b[0'|", 3, |bytes| {
				!super::super::locator::Locator::default()
					.feed(bytes)
					.is_empty()
			}),
		];

		for (what, sequence, at, claimed) in claims {
			assert!(claimed(sequence), "{what}: the plain spelling is claimed");
			assert!(
				claimed(&interrupted(sequence, at)),
				"{what}: and so is the one with a line feed in the middle, because the engine reads it \
				 through and goes on with the sequence"
			);
		}
	}

	#[test]
	fn can_and_sub_end_a_sequence_for_both() {
		// The other half of the same rule, so "keep reading" cannot quietly become "keep reading for ever".
		// CAN and SUB are the ANSI machine's cancels; the engine runs the byte and drops back to ground.
		for cancel_byte in [0x18_u8, 0x1a] {
			let bytes = [b'\x1b', b'[', b'5', b';', b'7', b'0', cancel_byte, b's'];
			let engine = engine(&bytes);
			assert!(
				engine.dispatched.is_empty(),
				"{cancel_byte:#04x} ended the sequence for the engine"
			);
			assert!(
				Cancel::default().feed(&bytes).is_empty(),
				"{cancel_byte:#04x} ends it for cmote too"
			);
		}
	}

	#[test]
	fn no_scanner_acts_on_a_sequence_the_parser_threw_away() {
		// The one-directional property, swept over the whole shape space. It is one-directional because the
		// converse is false by design: the parser frames a great deal that cmote deliberately ignores, and
		// that is the normal case rather than a divergence.
		//
		// What must never happen is the other way round — cmote acting on bytes the parser refused outright,
		// because then cmote is the only terminal in the world obeying that spelling. §106 shipped exactly
		// one of those (a parameter byte after an intermediate) and this is the net that would have caught it
		// without anyone thinking of the case.
		//
		// Every scanner in the directory, each asked only "did you act on this at all" — the single question
		// all eleven can answer in common. TWELVE entries for eleven scanners: `protect` is listed twice
		// because its verdict depends on whether the pen is armed, and both states have to be swept.
		let scanners: [DifferentialClaim; 12] = [
			("cancel", b"", |bytes| {
				!Cancel::default().feed(bytes).is_empty()
			}),
			("protect", b"", |bytes| {
				!protect::Protect::default().feed(bytes).is_empty()
			}),
			("protect, armed", b"", |bytes| {
				let mut scanner = protect::Protect::default();
				scanner.feed(b"\x1b[1\"q");
				!scanner.feed(bytes).is_empty()
			}),
			("graphics", b"", |bytes| {
				!graphics::Images::default().feed(bytes).is_empty()
			}),
			("dsr", b"", |bytes| {
				!super::super::dsr::Dsr::default().feed(bytes).is_empty()
			}),
			("tabs", b"", |bytes| {
				!super::super::tabs::Tabs::default().feed(bytes).is_empty()
			}),
			("scp", b"", |bytes| {
				!super::super::scp::Scp::default().feed(bytes).is_empty()
			}),
			("sgrstack", b"", |bytes| {
				!super::super::sgrstack::SgrStack::default()
					.feed(bytes)
					.is_empty()
			}),
			("rect", b"", |bytes| {
				!super::super::rect::Rectangles::default()
					.feed(bytes)
					.is_empty()
			}),
			("modkeys", b"", |bytes| {
				!super::super::modkeys::ModKeys::default()
					.feed(bytes)
					.is_empty()
			}),
			// The last to join this sweep — it could not before §111, because its own
			// machine had no state for an intermediate or for a byte the engine reads through, so it
			// disagreed with the parser over shapes this walks by the hundred.
			("query", b"", |bytes| {
				!super::super::query::Queries::default()
					.feed(bytes)
					.is_empty()
			}),
			// The newest (§140), and the one that widened the sweep to reach it: `|` and the `'`
			// intermediate went into `shapes` with this entry, so the shape space now walks the family
			// whose other three members cmote reads and deliberately declines to act on.
			("locator", b"", |bytes| {
				!super::super::locator::Locator::default()
					.feed(bytes)
					.is_empty()
			}),
		];

		let shapes = shapes();
		assert_eq!(
			shapes.len(),
			8100,
			"5 markers x 9 intermediates x 4 params x 15 finals x 3 orders"
		);
		// Collected rather than asserted case by case: the first failure is never the whole story, and an
		// inventory is what tells "one scanner has a bug" from "the rule is wrong everywhere".
		let mut alone = Vec::new();
		for (shape, bytes) in shapes {
			let framed = !engine(&bytes).dispatched.is_empty();
			for (who, _, claimed) in scanners {
				if claimed(&bytes) && !framed {
					alone.push(format!("{who} on {shape}"));
				}
			}
		}
		assert!(
			alone.is_empty(),
			"{} sequences cmote acts on and the parser threw away: {}",
			alone.len(),
			alone.join(", ")
		);
	}

	/// The bytes of a control string, in the four shapes that end one — used by both tests below, so
	/// the engine's rule and cmote's agreement with it are measured over the same inputs.
	///
	/// Each holds a complete XTVERSION (`CSI > 0 q`) that a program would expect answered, sitting
	/// either inside what looks like a payload or just after one.
	fn control_strings() -> [(&'static str, &'static [u8]); 5] {
		[
			// A recognised DCS (DECRQSS) whose payload opens with the query.
			("recognised, query inside", b"\x1bP$q\x1b[>0q\x1b\x5c"),
			// An unrecognised one, which this module's own doc calls the place a stream legitimately
			// carries arbitrary bytes.
			("unrecognised, query inside", b"\x1bPzzz\x1b[>0q\x1b\x5c"),
			// Cleanly terminated, then the query — the case that already worked.
			("terminated, query after", b"\x1bP$qm\x1b\x5c\x1b[>0q"),
			// Ended by ST as a single byte (0x9c) rather than `ESC \`.
			("single-byte ST, query after", b"\x1bP$qm\x9c\x1b[>0q"),
			// Never terminated at all.
			("unterminated, query inside", b"\x1bP$q\x1b[>0q"),
		]
	}

	#[test]
	fn an_escape_that_is_no_terminator_still_opens_the_next_sequence() {
		// ESC does two jobs at once in the ANSI state machine: it ENDS whatever control string is open
		// and it OPENS the next sequence. `vte` does both, so a query written after a control string —
		// or in place of its payload — reaches the dispatch either way.
		//
		// `query` did only the first, in two of its states, and this harness is what found it: every
		// case below had the engine dispatching an XTVERSION that cmote answered with nothing, so the
		// program that asked waited out its timeout. Not a screen divergence and not cmote acting
		// alone — the quietest kind there is, which is why nobody noticed: the scanner was written in
		// §33 and this was found in §111.
		for (what, bytes) in control_strings() {
			let dispatched = engine(bytes)
				.dispatched
				.iter()
				.any(|csi| csi.final_byte == 'q' && csi.intermediates == *b">");
			assert!(dispatched, "{what}: the engine dispatched the query");
			let answered = super::super::query::Queries::default()
				.feed(bytes)
				.contains(&super::super::query::Query::Version);
			assert!(answered, "{what}: and cmote answers it");
		}
	}

	#[test]
	fn a_control_string_ends_on_a_single_byte_st_for_both() {
		// ST's C1 form, 0x9c. The engine ends a control string on it and reads the payload it had;
		// `term/graphics.rs` has always known that and `term/query.rs` did not, so a DECRQSS spelled
		// this way went unanswered while a picture spelled this way drew (§111).
		//
		// The query AFTER the string was already reachable once a stray ESC stopped being a dead end —
		// what this pins is the string's own payload, which is the part 0x9c decides.
		let bytes = b"\x1bP$qm\x9c";
		let engine = engine(bytes);
		assert_eq!(engine.dispatched.len(), 0, "a DCS is not a CSI");
		assert!(
			super::super::query::Queries::default()
				.feed(bytes)
				.contains(&super::super::query::Query::Decrqss(
					super::super::query::Decrqss::Sgr
				)),
			"the SGR request is answered, so the payload ended at the ST"
		);
	}

	#[test]
	fn a_framer_cannot_be_fooled_by_a_control_string() {
		// The property that lets `query` and `graphics` keep their own DCS machine while their CSI half
		// moves onto the shared framer (§111) — and the correction of a worry that had it backwards.
		//
		// The worry was that a DCS payload could smuggle `ESC [ > c` past a framer that knows nothing
		// about DCS, and so answer a query out of bytes that were only ever data. It cannot, and the
		// reason is the rule above: the only way into a CSI is `ESC [`, and an ESC inside a control
		// string ends that string FOR THE ENGINE TOO. So there is no such thing as a payload the engine
		// reads as data and a framer reads as a sequence — wherever the framer claims one, the engine
		// dispatched it.
		//
		// Which makes the framer STRICTER than the fused machine it replaces, not laxer: its ESC
		// handling is unconditional, and being unconditional is exactly what `query` got wrong.
		for (what, bytes) in control_strings() {
			let mut framed = Vec::new();
			super::super::csi::Framer::default().feed(bytes, |_, csi| {
				framed.push(csi.final_byte());
			});
			let dispatched: Vec<u8> = engine(bytes)
				.dispatched
				.iter()
				.map(|csi| u8::try_from(csi.final_byte).expect("an ASCII final byte"))
				.collect();
			assert_eq!(framed, dispatched, "{what}");
		}
	}

	#[test]
	fn a_sub_parameter_is_read_or_refused_by_who_the_engine_leaves_it_to() {
		// The two right answers to the same byte, and why `Csi::sub_parameters` reports the FACT and
		// leaves the policy to each scanner (§111).
		//
		// ED is a sequence the engine implements. `next_param_or(0)` reads the first sub-parameter of
		// the first parameter, so `CSI 2:3 J` really does wipe the screen — and `graphics` has to take
		// the pictures with it or they stand on a screen whose text has gone. It did not, until the
		// shared grammar arrived.
		for (spelling, bytes) in [("screen", &b"\x1b[2:3J"[..]), ("scrollback", b"\x1b[3:1J")] {
			let first = if spelling == "screen" { 2 } else { 3 };
			assert!(
				engine(bytes).dispatched_plain('J', first),
				"{spelling}: the engine erases on the colon spelling"
			);
			assert!(
				!graphics::Images::default().feed(bytes).is_empty(),
				"{spelling}: so cmote takes the pictures with it"
			);
		}

		// DECERA is a sequence the engine has NO arm for — `vte` frames it and `ansi.rs` drops it — so
		// cmote is the only actor and refusing a spelling DEC never defined costs nothing. `rect` reads
		// four corners out of it, and a rectangle built from a misread corner erases cells the program
		// never named.
		let corners = b"\x1b[2:3;5;7$z";
		assert!(
			engine(corners)
				.dispatched
				.iter()
				.any(|csi| csi.final_byte == 'z'),
			"the engine frames it, and has nothing to do with it"
		);
		assert!(
			super::super::rect::Rectangles::default()
				.feed(corners)
				.is_empty(),
			"so cmote refuses the spelling rather than guessing at the corners"
		);
	}

	#[test]
	fn a_parameter_after_an_intermediate_is_refused_by_both() {
		// The last of the grammar divergences, and it leans the other way from the control bytes: the parser
		// refuses a parameter byte once an intermediate has arrived (`lib.rs:232`, straight to `CsiIgnore`,
		// and `:216-224` then swallows the rest and dispatches nothing), while cmote's scanners used to take
		// parameter bytes at any point and classify the sequence anyway. So cmote honoured a spelling the
		// engine calls malformed — acting alone, which is the direction that has no upside at all: there is
		// no engine behaviour to compensate for, only a sequence nobody else in the world would obey.
		//
		// Every scanner that buffers intermediates, each fed its own sequence with a parameter byte pushed in
		// after the intermediate.
		let cases: [DifferentialClaim; 4] = [
			("protect, DECSCA", b"\x1b[1\"2q", |bytes| {
				!protect::Protect::default().feed(bytes).is_empty()
			}),
			("scp, SCP", b"\x1b[2 1k", |bytes| {
				!super::super::scp::Scp::default().feed(bytes).is_empty()
			}),
			("sgrstack, XTPUSHSGR", b"\x1b[1#2{", |bytes| {
				!super::super::sgrstack::SgrStack::default()
					.feed(bytes)
					.is_empty()
			}),
			("rect, DECERA", b"\x1b[2;3;5;7$1z", |bytes| {
				!super::super::rect::Rectangles::default()
					.feed(bytes)
					.is_empty()
			}),
		];

		for (what, bytes, claimed) in cases {
			assert!(
				engine(bytes).dispatched.is_empty(),
				"{what}: the engine threw the whole sequence away"
			);
			assert!(
				!claimed(bytes),
				"{what}: and cmote no longer acts on it alone"
			);
		}
	}

	/// What `dcs::Framer` made of a chunk, in the same terms the trace above records: the introducers it
	/// reported, the payload it kept, and the escape sequences it saw (§111).
	fn framed_strings(bytes: &[u8]) -> StringTrace {
		let (mut hooked, mut put, mut escapes) = (Vec::new(), Vec::new(), Vec::new());
		super::super::dcs::Framer::<4096>::default().feed(bytes, |_, control| match control {
			super::super::dcs::Control::String(dcs) => {
				// The engine counts the private marker among the intermediates, so this side has to put
				// it back to compare like with like.
				let mut intermediates = Vec::new();
				intermediates.extend(dcs.marker());
				intermediates.extend_from_slice(dcs.intermediates());
				hooked.push(Hook {
					intermediates,
					final_byte: char::from(dcs.final_byte()),
				});
				put.extend_from_slice(dcs.payload());
			}
			super::super::dcs::Control::Escape(escape) => {
				escapes.push((escape.intermediates().to_vec(), escape.final_byte()));
			}
		});
		(hooked, put, escapes)
	}

	/// Every shape of DCS introducer worth walking, in the same spirit as [`shapes`] one door along.
	///
	/// The final bytes are `q` — every string cmote reads ends in one, and all three of them do — plus
	/// `|` and `r`, which are the finals of the REPLIES cmote sends (`DCS > | id ST`, `DCS 1 $ r … ST`),
	/// so a reply looping back through a scanner is covered too.
	#[cfg(test)]
	fn string_shapes() -> Vec<(String, Vec<u8>)> {
		const MARKERS: [Option<u8>; 3] = [None, Some(b'?'), Some(b'>')];
		const INTERMEDIATES: [&[u8]; 5] = [b"", b"$", b"+", b" ", b"$#"];
		const PARAMS: [&[u8]; 4] = [b"", b"1", b"1;2", b"1:2"];
		const FINALS: &[u8] = b"q|r";

		let mut out = Vec::new();
		for marker in MARKERS {
			for intermediates in INTERMEDIATES {
				for params in PARAMS {
					for &final_byte in FINALS {
						let mut bytes = vec![0x1b, b'P'];
						bytes.extend(marker);
						bytes.extend_from_slice(params);
						bytes.extend_from_slice(intermediates);
						bytes.push(final_byte);
						bytes.extend_from_slice(b"m\x1b\\");
						out.push((
							format!(
								"{}{}{}{}",
								marker.map_or(String::new(), |marker| (marker as char).to_string()),
								String::from_utf8_lossy(params),
								String::from_utf8_lossy(intermediates),
								char::from(final_byte)
							),
							bytes,
						));
					}
				}
			}
		}
		out
	}

	#[test]
	fn a_string_is_hooked_by_both_or_by_neither() {
		// The sweep for the second framer, and the same question as the CSI one: not "does this case
		// work" but "does cmote read this string exactly when the engine hooks it". Both refusals the
		// engine's DCS states have — a parameter byte after an intermediate, a private marker after the
		// parameters started — are in here, and so is the marker-BEFORE-parameters spelling that is
		// legal (§111).
		let shapes = string_shapes();
		assert_eq!(
			shapes.len(),
			180,
			"3 markers x 5 intermediates x 4 params x 3 finals"
		);
		for (shape, bytes) in shapes {
			let engine = engine(&bytes);
			let (hooked, _, _) = framed_strings(&bytes);
			if engine.hooked.is_empty() {
				// The engine threw the string away. What must hold then is not that the framer stayed
				// silent — it is LOOSER than the engine about one thing, `MAX_INTERMEDIATES`, which is 4
				// here against the 2 the engine counts the private marker into — but that no SCANNER
				// acts. Neither of the two claims a string with three intermediates: `query` insists on
				// exactly `$` or `+` and `graphics` on none at all, so both sides ignore it by different
				// routes, which is the same arrangement `csi::MAX_INTERMEDIATES` documents.
				assert!(
					super::super::query::Queries::default()
						.feed(&bytes)
						.is_empty(),
					"{shape}: the engine ignored it, so `query` must not answer"
				);
				assert!(
					graphics::Images::default().feed(&bytes).is_empty(),
					"{shape}: nor may `graphics` draw it"
				);
			} else {
				assert_eq!(
					engine.hooked, hooked,
					"{shape}: the engine hooked {:?}, cmote read {hooked:?}",
					engine.hooked
				);
			}
		}
	}

	#[test]
	fn the_payload_cmote_keeps_is_the_payload_the_engine_was_given() {
		// Not every byte between the final byte and the terminator reaches the engine's handler: DEL and
		// the high bytes are discarded on the way in (`lib.rs:330`, `:335`). A scanner comparing a
		// payload against a known selector — `query` does, for the DECRQSS `m` — has to see exactly what
		// the engine would have seen, or the two answer differently about the same string.
		//
		// BEL is left out of these payloads deliberately: the engine reads one as a payload byte and
		// cmote ends the string on it, which is the one leniency here and is stated on `dcs::BEL`.
		for payload in [
			&b"m"[..],
			b"m\x7f",
			b"m\x80\xff",
			b"\x01\x02m",
			b"#0;2;0;0;0~~",
			b"",
		] {
			let mut bytes = b"\x1bP$q".to_vec();
			bytes.extend_from_slice(payload);
			bytes.extend_from_slice(b"\x1b\\");
			let engine = engine(&bytes);
			let (_, put, _) = framed_strings(&bytes);
			assert_eq!(
				engine.put, put,
				"payload {payload:?}: the engine was given {:?}",
				engine.put
			);
			assert_eq!(engine.unhooked, 1, "payload {payload:?}: one string ended");
		}
	}

	#[test]
	fn a_cancel_ends_a_string_for_both() {
		// CAN and SUB unhook the string and drop the parser to ground (`lib.rs:320-324`), so the ST that
		// follows terminates nothing. Both of the machines `dcs::Framer` replaces read these two straight
		// into the payload and went on waiting — so a later terminator completed a string the engine had
		// thrown away, and `query` would have answered a question that was cancelled (§111).
		// The two sides report a string at DIFFERENT moments, which is worth stating because it decides
		// what this test can compare. The engine hooks at the introducer's final byte and unhooks
		// whenever the string ends, cleanly or not; cmote reports a string only once it is COMPLETE,
		// because only then does it know whether the string named a terminator (§54). So a cancelled
		// string is a hook and an unhook there, and nothing at all here — and nothing is what both sides
		// ACT on, which is the agreement that matters.
		for cancel in [0x18_u8, 0x1a] {
			let bytes = [0x1b, b'P', b'$', b'q', b'm', cancel, 0x1b, b'\\'];
			let engine = engine(&bytes);
			assert_eq!(
				engine.unhooked, 1,
				"{cancel:#04x}: the engine unhooked at the cancel"
			);
			assert_eq!(
				engine.escapes,
				vec![(Vec::new(), b'\\')],
				"{cancel:#04x}: and read the ST after it as an escape sequence of its own"
			);
			let (hooked, _, escapes) = framed_strings(&bytes);
			assert!(
				hooked.is_empty(),
				"{cancel:#04x}: cmote completed no string, so it answers nothing"
			);
			assert_eq!(
				escapes,
				vec![(Vec::new(), b'\\')],
				"{cancel:#04x}: and reads the same stray ST the engine did"
			);
			// The scanner that would have answered, asked directly: a cancelled DECRQSS gets no reply.
			assert!(
				super::super::query::Queries::default()
					.feed(&bytes)
					.is_empty(),
				"{cancel:#04x}: and `query` stays silent"
			);
		}
	}

	#[test]
	fn a_reset_the_engine_performs_is_one_cmote_sees() {
		// RIS is the one escape sequence anything in cmote acts on, and the engine really performs it —
		// so a scanner that misses one keeps state a reset threw away. Four scanners watched for this
		// with "was the previous byte an ESC", which is wrong for every byte the escape state reads
		// through (§111).
		// All five scanners that read a RIS, because until §111 four of them read it with a bool and one
		// with a state machine, and every one of the four missed these.
		let readers: [DifferentialClaim; 5] = [
			("graphics", b"", |bytes| {
				graphics::Images::default()
					.feed(bytes)
					.iter()
					.any(|(_, event)| matches!(event, graphics::GraphicsEvent::Reset))
			}),
			("protect", b"", |bytes| {
				!protect::Protect::default().feed(bytes).is_empty()
			}),
			("scp", b"", |bytes| {
				!super::super::scp::Scp::default().feed(bytes).is_empty()
			}),
			("sgrstack", b"", |bytes| {
				!super::super::sgrstack::SgrStack::default()
					.feed(bytes)
					.is_empty()
			}),
			// `rect` reports a RIS as nothing at all — it CUTS the chunk there, because DECSACE is reset
			// and that extent is stamped onto the requests that FOLLOW (§59). So the observable is the
			// extent a later DECCARA carries: `CSI 2 * x` selects rectangle-extent, the RIS puts it back
			// to stream-extent, and only a scanner that saw the reset stamps the request after it with
			// `Stream`.
			("rect", b"", |bytes| {
				let mut scanner = super::super::rect::Rectangles::default();
				scanner.feed(b"\x1b[2*x");
				let mut whole = bytes.to_vec();
				whole.extend_from_slice(b"\x1b[2;3;5;7;1$r");
				format!("{:?}", scanner.feed(&whole)).contains("Stream")
			}),
		];

		for (byte, name) in READ_THROUGH {
			let bytes = [0x1b, byte, b'c'];
			assert_eq!(
				engine(&bytes).escapes,
				vec![(Vec::new(), b'c')],
				"{name}: the engine dispatches the reset"
			);
			for (who, _, sees) in readers {
				assert!(sees(&bytes), "{who} misses the reset with {name} in it");
			}
		}

		// And the plain spelling still reads, so the test cannot pass by claiming everything.
		for (who, _, sees) in readers {
			assert!(sees(b"\x1bc"), "{who} misses a plain reset");
			assert!(
				!sees(b"\x1b(c"),
				"{who} reads a charset designation as a reset"
			);
		}

		// Inside a sixel payload, where §111 found the defect: the ESC ends the picture AND opens the
		// reset, and the picture must not survive it.
		let bytes = b"\x1bPq#0;2;0;0;0~~\x1bc";
		assert_eq!(engine(bytes).escapes, vec![(Vec::new(), b'c')]);
		let found = graphics::Images::default().feed(bytes);
		assert!(
			matches!(found.as_slice(), [(_, graphics::GraphicsEvent::Reset)]),
			"a reset interrupting a payload, found {found:?}"
		);
	}

	/// Every OSC payload `osc::Framer` completed in a chunk, in the same shape the trace records.
	fn framed_oscs(bytes: &[u8]) -> Vec<Vec<u8>> {
		let mut seen = Vec::new();
		super::super::osc::Framer::<4096>::default().feed(bytes, |_, payload| {
			seen.push(payload.to_vec());
		});
		seen
	}

	#[test]
	fn an_osc_is_read_by_both_and_carries_the_same_payload() {
		// The THIRD framer through the same door, and it had the same two ESC defects `dcs::Framer`
		// was built to end (§111). Both are here as agreements now, plus the payload rule that is the
		// engine's and was nobody's here: a C0 inside an OSC is DROPPED rather than passed to the
		// handler (`lib.rs:349`), so a cwd with one in it used to reach `cwd::parse` as a different
		// string on cmote's side than on the engine's.
		let cases: [(&str, &[u8]); 6] = [
			("plain", b"\x1b]7;file:///tmp\x07"),
			("ST-terminated", b"\x1b]7;file:///tmp\x1b\\"),
			// A byte the engine reads through between the ESC and the `]`.
			(
				"read-through before the bracket",
				b"\x1b\n]7;file:///tmp\x07",
			),
			("DEL before the bracket", b"\x1b\x7f]7;file:///tmp\x07"),
			// A C0 inside the payload, which the engine drops on the way in.
			(
				"a control byte in the payload",
				b"\x1b]7;file:\x01///tmp\x07",
			),
			// And one after the ESC of the terminator, which the engine reads through.
			(
				"a control byte before the ST",
				b"\x1b]7;file:///tmp\x1b\x00\\",
			),
		];

		for (what, bytes) in cases {
			let engine = engine(bytes);
			assert_eq!(
				engine.oscs,
				framed_oscs(bytes),
				"{what}: the engine dispatched {:?}",
				engine.oscs
			);
			// And the scanner on top of the framer really does act on it, which is what the agreement
			// is for.
			let mut cwd = super::super::cwd::Cwd::default();
			cwd.feed(bytes);
			assert_eq!(cwd.path(), Some("/tmp"), "{what}: and `cwd` reads it");
		}
	}

	#[test]
	fn a_cancelled_osc_is_dropped_here_and_dispatched_there() {
		// The one place this framer is deliberately STRICTER than the engine, asserted as it behaves so
		// that a change to it is a decision rather than a surprise. CAN and SUB make the engine's parser
		// dispatch what it had (`osc_end` then `execute`, `lib.rs:355-359`); cmote abandons the payload,
		// because a string that named no terminator is not answered (§54).
		//
		// Safe to be stricter here and nowhere else: the engine has no handler behind any OSC cmote
		// reads, so there is no second actor to fall out of step with. What it replaces was not stricter
		// but WRONG — the cancel went into the payload and the framer kept waiting, so a later BEL
		// handed `cwd` a cancelled path with everything after it glued on (§111).
		for cancel in [0x18_u8, 0x1a] {
			let bytes = [0x1b, b']', b'7', b';', b'/', b'a', cancel, b'/', b'b', 0x07];
			assert_eq!(
				engine(&bytes).oscs,
				vec![b"7;/a".to_vec()],
				"{cancel:#04x}: the engine dispatches the part before the cancel"
			);
			assert!(
				framed_oscs(&bytes).is_empty(),
				"{cancel:#04x}: cmote dispatches nothing at all"
			);
			// The part that matters either way: neither side reads `/a/b`, which is the string the old
			// framer would have handed on.
			let mut cwd = super::super::cwd::Cwd::default();
			cwd.feed(&bytes);
			assert_eq!(cwd.path(), None, "{cancel:#04x}: and no path is taken");
		}

		// The same strictness one door along, and the case that matters most: an ESC ends the first
		// string and OPENS the second. The engine dispatches BOTH — it cannot tell an interrupted
		// string from a finished one — and cmote drops the interrupted one. What must NOT happen, and
		// did before §111, is losing the string that followed: the framer went back to ordinary text
		// and read the second one's bytes as printable characters, so the cwd never arrived at all.
		let bytes = b"\x1b]0;title\x1b]7;file:///tmp\x07";
		assert_eq!(
			engine(bytes).oscs,
			vec![b"0;title".to_vec(), b"7;file:///tmp".to_vec()],
			"the engine dispatches the interrupted string too"
		);
		assert_eq!(
			framed_oscs(bytes),
			vec![b"7;file:///tmp".to_vec()],
			"cmote drops the interrupted one and reads the one that terminated"
		);
		let mut cwd = super::super::cwd::Cwd::default();
		cwd.feed(bytes);
		assert_eq!(cwd.path(), Some("/tmp"));
	}

	#[test]
	fn a_charset_designation_is_not_a_reset_for_either() {
		// `ESC ( c` shares RIS's final byte and resets nothing — the engine dispatches it with the `(`
		// among its intermediates, which is why `Escape` reports both parts and `graphics` tests both.
		let bytes = b"\x1b(c";
		assert_eq!(engine(bytes).escapes, vec![(vec![b'('], b'c')]);
		assert!(
			graphics::Images::default().feed(bytes).is_empty(),
			"and cmote does not read it as a reset"
		);
	}
}

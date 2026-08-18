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

/// What the engine's parser made of a chunk: what it dispatched, what it refused, and which control
/// bytes it ran on the way.
#[derive(Debug, Default)]
struct Engine {
	/// Every CSI dispatched with `ignore` clear — the ones the engine's handler actually sees.
	dispatched: Vec<Csi>,
	/// How many arrived with `ignore` set, which is the parser saying "too many parameters or
	/// intermediates, do not act on this". `ansi.rs` drops those.
	ignored: usize,
	/// Control bytes executed, including any that arrived INSIDE a sequence — which this parser runs
	/// without abandoning the sequence around them.
	executed: Vec<u8>,
}

impl vte::Perform for Engine {
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
}

/// Drive the real parser over `bytes` and report what it did.
fn engine(bytes: &[u8]) -> Engine {
	let mut parser = vte::Parser::new();
	let mut seen = Engine::default();
	parser.advance(&mut seen, bytes);
	seen
}

impl Engine {
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
			vec![(bytes.len(), protect::Request::Reassert)],
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
			[(0, graphics::Event::ClearScreen)]
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
	fn a_parameter_after_an_intermediate_is_where_the_two_still_disagree() {
		// NOT an approval either, and this one leans the other way: the parser refuses a parameter byte
		// once an intermediate has arrived (`lib.rs:232`, straight to `CsiIgnore`), so it dispatches
		// nothing at all — while cmote's scanners take parameter bytes at any point and classify the
		// sequence. So cmote honours a spelling the engine calls malformed.
		//
		// Harmless where it is today (DECSCA is cmote's own feature, so there is no engine action to
		// contradict), and it is still the two of us reading one grammar differently. The framer settles
		// it; until then it is written down here.
		let bytes = b"\x1b[1\"2q";
		let engine = engine(bytes);
		assert!(
			engine.dispatched.is_empty(),
			"the engine threw the whole sequence away"
		);

		let mut scanner = protect::Protect::default();
		assert_eq!(
			scanner.feed(bytes),
			vec![(bytes.len(), protect::Request::Protect(false))],
			"while cmote classified it as a DECSCA"
		);
	}
}

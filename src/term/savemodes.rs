// term/savemodes.rs — XTSAVE and XTRESTORE, the private modes a program puts back (PLAN §141).
//
//   CSI ? Pm s    XTSAVE    — remember what each named DEC-private mode is set to
//   CSI ? Pm r    XTRESTORE — put each named mode back to what it was
//
// xterm's pair, and the politeness protocol a full-screen program uses around a mode it is about to
// change: save `? 25`, hide the cursor, restore on the way out. The matrix named the harm of not
// having it precisely — "a program that saves `? 25`, hides the cursor and restores gets no restore,
// so the cursor stays hidden after it exits" — a stuck state of exactly the shape §72's soft reset
// exists to prevent.
//
// `vte` dispatches both and `ansi.rs` has no arm for either: the private marker counts as an
// intermediate to that parser, so the table's `('s', [])` is the save-cursor and `('r', [])` is
// DECSTBM, while `('s', [b'?'])` and `('r', [b'?'])` match nothing at all. `term/cancel.rs` has said
// so in a comment since §57 and tests it — XTSAVE is the near miss that makes DECSLRM's own scanner
// insist on there being no marker.
//
// WHY THIS WAS RECORDED AS BLOCKED, AND WHY THAT WAS TOO STRONG.
//
// The row and two Not done lists said the same thing: "restoring an arbitrary private mode means
// holding a copy of the engine's mode state, which makes cmote a second source for it (§71)". That
// reading conflates two things this project has kept apart everywhere else.
//
// A second WRITER is what §71 and §73 refuse — cmote reaching into the engine's mode word and setting
// bits, so that two pieces of code decide what a mode is. Nothing here does that. A restore FEEDS the
// engine `CSI ? Ps h` or `CSI ? Ps l`, sequences the engine already implements and the matrix already
// marks ✅, so the engine remains the only writer of its own modes. That is the route §72 took for the
// soft reset, §74 for the tab stops and §85 for the video-attribute stack.
//
// A saved COPY is not a second source, and §85 settled that one section at a time: `saved_pens` holds
// the engine's template cell as it stood at each push, and nothing calls it a second opinion about
// what the pen is NOW. A saved mode is the same object — a record of what a mode WAS, consulted only
// to be replayed through the engine, never consulted to answer "what is this mode set to". The one
// question that could make it a second source is DECRQM, and DECRQM is answered from the live mode
// (§60), not from here.
//
// So what was really blocking this was the smaller, factual half of the row's note: "which the
// engine's seam does not expose". The ENGINE exposes it — `Term::mode()` is public — and cmote's own
// seam simply had no reader. `Screen::private_mode` is that reader, and writing it is most of §141.
//
// WHAT CAN BE SAVED IS WHAT CAN BE READ, and the list is not this module's to hold. A mode is
// saveable exactly when `Screen::private_mode` answers `Some` for it, plus mode 69, which is cmote's
// own. Deriving the set from the reader rather than repeating it here is deliberate: two lists would
// be two things to keep in step, and the one that drifted would be the one that quietly saved a mode
// nothing could restore. See that function for why `3`, `12`, `2026` and every unknown mode answer
// `None`.
//
// A MODE NEVER SAVED IS NOT RESTORED. xterm keeps its saved values in an array that starts zeroed, so
// a restore of a mode nobody saved reads as "reset" there. cmote does nothing instead, and the
// difference is worth stating because it is a divergence chosen rather than inherited: ctlseqs says
// "the value of Ps previously saved is restored", and where no value was previously saved there is
// nothing to restore. Guessing "reset" would let two sequences no program paired turn the cursor off
// — the very stuck state this section exists to prevent, reached from the other direction.
//
// The store is bounded by construction. Only modes the reader answers for are ever kept, so a hostile
// stream cannot grow it past that handful however many parameters it sends (§12) — which is why this
// is a short vector with a linear scan rather than a map keyed by whatever arrives.

/// One XTSAVE or XTRESTORE, with the modes it named.
///
/// The modes are carried rather than acted on here, because which of them can be saved is a question
/// about the engine's seam and this module is the grammar. Bounded by the framer's own
/// [`csi::MAX_PARAMS`](super::csi::MAX_PARAMS), so the vector cannot be grown without bound by a
/// remote (§12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveModesRequest {
	/// `true` for XTRESTORE (`r`), `false` for XTSAVE (`s`).
	pub restore: bool,
	/// The modes named, in the order the sequence wrote them — which is the order a restore has to
	/// feed them back in, because the engine's mouse modes are mutually exclusive and the last one
	/// fed is the one that stands.
	pub modes: Vec<u16>,
}

/// The XTSAVE / XTRESTORE scanner (§141). Feed it every byte of shell output; it reports where each
/// of the pair sat and which modes it named.
///
/// The CSI grammar is [`csi::Framer`]'s (§111); what is left here is deciding which of the two
/// sequences a finished CSI is, and reading its parameter list.
#[derive(Debug, Default)]
pub struct SaveModes {
	framer: super::csi::Framer,
}

impl SaveModes {
	/// Scan a chunk of shell output, returning where each XTSAVE / XTRESTORE sat. Safe at any chunk
	/// boundary — the state machine carries over between calls, so a sequence may be split anywhere,
	/// even between the ESC and the `[`.
	///
	/// Each offset is ONE PAST the sequence's final byte, like the tab-stop reset (§74) and the
	/// video-attribute stack (§85). Both directions need it, for the two halves of one reason: a SAVE
	/// reads the modes as they stand where the sequence was written, and a RESTORE feeds the engine
	/// sequences that have to land at that same point rather than at the end of the chunk. A program
	/// that restores the cursor and then prints would otherwise print first.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, SaveModesRequest)> {
		let mut requests = Vec::new();
		self.framer.feed(bytes, |span, csi| {
			if let Some(request) = request(csi) {
				requests.push((span.past(), request));
			}
		});
		requests
	}
}

/// Which of the pair a finished sequence is, or `None` for everything else.
///
/// Both require the `?` marker and no intermediate. The near misses that makes matter are not
/// hypothetical, and they are the reason all three parts are matched together (§56):
///
///   * `CSI Pl ; Pr s` with NO marker is **DECSLRM**, the left and right margins — or a save-cursor,
///     depending on mode 69 (§102). `term/cancel.rs` is built around that ambiguity and has always
///     said in writing that the marker is what tells XTSAVE apart from it.
///   * `CSI Pt ; Pb r` with no marker is **DECSTBM**, the scrolling region, which the engine
///     implements. Reading a region change as a mode restore would be a scanner acting on a sequence
///     the engine is also acting on, in a different sense — the worst kind of disagreement.
///   * `CSI ... $ r` is **DECCARA** and `CSI ... $ p` DECRQM; an intermediate makes it somebody
///     else's sequence, which is why an empty intermediate list is required rather than ignored.
///
/// A sequence naming NO modes is not claimed. `CSI ? s` is well formed and asks for nothing to be
/// saved, so reporting it would put an empty request through the interruption loop for no effect —
/// and the loop's cost is the split advance, which the common chunk should not pay.
///
/// Sub-parameters rule it out. `Pm` is a list of modes and DEC and xterm spell every such list with
/// `;`, so `CSI ? 1 : 2 s` is a spelling neither defines — the rule [`Csi::sub_parameters`] exists to
/// leave at the site that knows its own sequence (§111).
///
/// An EMPTY field inside the list is skipped rather than read as a mode. `CSI ? 25 ; ; 1000 s` names
/// two modes and a gap, and `param` reports the gap as `None`; treating it as 0 would add a mode
/// nothing can save and that nobody wrote.
fn request(csi: &super::csi::Csi<'_>) -> Option<SaveModesRequest> {
	let restore = match (csi.final_byte(), csi.marker(), csi.intermediates()) {
		(b's', Some(b'?'), &[]) => false,
		(b'r', Some(b'?'), &[]) => true,
		_ => return None,
	};
	if csi.sub_parameters() {
		return None;
	}
	let modes: Vec<u16> = (0..csi.param_count())
		.filter_map(|index| csi.param(index))
		.collect();
	if modes.is_empty() {
		return None;
	}
	Some(SaveModesRequest { restore, modes })
}

/// What XTSAVE remembered, one slot per mode, newest value winning (§141).
///
/// A vector with a linear scan rather than a map, because the number of modes that can ever be in it
/// is the number `Terminal::private_mode` answers for — the engine's flags, mode 69, mode 2048, and
/// `term/decmodes.rs`'s own table. A remote can send a thousand parameters and still not make this
/// hold one entry more, which is the bound §12 asks for, stated as a property of the type rather than
/// as a limit checked somewhere. **No number is written here on purpose** (§161): the set grew four
/// times without this line noticing, and the readers above are the count.
#[derive(Debug, Default)]
pub struct Saved {
	entries: Vec<(u16, bool)>,
}

impl Saved {
	/// Remember that `mode` was `value`, replacing whatever was remembered before.
	///
	/// Overwriting is xterm's behaviour and the only one that makes sense for a protocol with no depth
	/// to it: XTSAVE has one slot per mode, not a stack, so a program that saves twice and restores
	/// once gets the second value. The video-attribute stack (§85) is the one in this codebase that is
	/// a stack, and it is a different sequence for that reason.
	pub fn save(&mut self, mode: u16, value: bool) {
		if let Some(entry) = self.entries.iter_mut().find(|(saved, _)| *saved == mode) {
			entry.1 = value;
		} else {
			self.entries.push((mode, value));
		}
	}

	/// What `mode` was when it was last saved, or `None` if it never was — see the module header for
	/// why that is not read as "reset".
	pub fn restore(&self, mode: u16) -> Option<bool> {
		self.entries
			.iter()
			.find(|(saved, _)| *saved == mode)
			.map(|(_, value)| *value)
	}

	/// How many modes are remembered. For the test that pins the bound above.
	#[cfg(test)]
	pub fn len(&self) -> usize {
		self.entries.len()
	}
}

/// `CSI ? Ps h` or `CSI ? Ps l` — the mode change a restore feeds the engine (§141).
///
/// This is the whole of how cmote puts a mode back, and it is why nothing here is a second writer of
/// engine state: the bytes go to the engine's own parser, through the same gate live output passes,
/// and the engine sets its own bit. Every sequence this can produce is one the matrix already marks
/// ✅.
pub fn mode_feed(mode: u16, set: bool) -> Vec<u8> {
	let final_byte = if set { 'h' } else { 'l' };
	format!("\x1b[?{mode}{final_byte}").into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go — the shape of every test below that is not about splitting.
	fn scan(bytes: &[u8]) -> Vec<(usize, SaveModesRequest)> {
		SaveModes::default().feed(bytes)
	}

	/// Just the requests, for the tests that are about WHAT was read rather than where.
	fn requests(bytes: &[u8]) -> Vec<SaveModesRequest> {
		scan(bytes)
			.into_iter()
			.map(|(_, request)| request)
			.collect()
	}

	/// The two sequences, told apart by their final byte, and the offset one past it.
	#[test]
	fn both_directions_are_read_with_their_modes() {
		assert_eq!(
			scan(b"\x1b[?25s"),
			vec![(
				6,
				SaveModesRequest {
					restore: false,
					modes: vec![25]
				}
			)]
		);
		assert_eq!(
			scan(b"\x1b[?25r"),
			vec![(
				6,
				SaveModesRequest {
					restore: true,
					modes: vec![25]
				}
			)]
		);
	}

	/// `Pm` is a LIST, and the order is kept because a restore has to feed it back in that order —
	/// the engine's mouse modes are mutually exclusive, so the last one fed is the one that stands.
	#[test]
	fn a_list_of_modes_keeps_the_order_it_was_written_in() {
		assert_eq!(
			requests(b"\x1b[?1000;1002;1006s"),
			vec![SaveModesRequest {
				restore: false,
				modes: vec![1000, 1002, 1006]
			}]
		);
	}

	/// The near miss the whole module is shaped around. Without the marker `s` is DECSLRM or a
	/// save-cursor and `r` is DECSTBM — sequences the ENGINE acts on, so claiming one here would be
	/// two pieces of code acting on the same bytes in different senses.
	#[test]
	fn without_the_private_marker_these_are_the_engines_own_sequences() {
		assert!(scan(b"\x1b[1;70s").is_empty(), "DECSLRM, or a save-cursor");
		assert!(scan(b"\x1b[1;24r").is_empty(), "DECSTBM");
		assert!(scan(b"\x1b[s").is_empty(), "the bare save-cursor");
		assert!(scan(b"\x1b[r").is_empty(), "DECSTBM reset to the full page");
		// And a marker that is not `?` is a third thing again.
		assert!(scan(b"\x1b[>25s").is_empty(), "XTSHIFTESCAPE's marker");
		assert!(scan(b"\x1b[<25r").is_empty());
	}

	/// An intermediate makes it somebody else's sequence — `$ r` is DECCARA, `$ p` DECRQM — so the
	/// match tests all three of final byte, marker and intermediates (§56).
	#[test]
	fn an_intermediate_rules_it_out() {
		assert!(scan(b"\x1b[?25$r").is_empty());
		assert!(scan(b"\x1b[?25 s").is_empty());
	}

	/// `Pm` is spelled with `;` by DEC and by xterm alike, so a `:` is a spelling neither defines.
	#[test]
	fn a_sub_parameter_rules_it_out() {
		assert!(scan(b"\x1b[?1000:1002s").is_empty());
	}

	/// A sequence naming no modes asks for nothing, so it is not put through the interruption loop.
	#[test]
	fn a_sequence_naming_no_modes_is_not_claimed() {
		assert!(scan(b"\x1b[?s").is_empty());
		assert!(scan(b"\x1b[?r").is_empty());
	}

	/// An empty field in the middle of the list is a gap, not a mode. Reading it as 0 would put a
	/// mode nobody wrote into the request.
	#[test]
	fn an_empty_field_in_the_list_is_skipped() {
		assert_eq!(
			requests(b"\x1b[?25;;1000s"),
			vec![SaveModesRequest {
				restore: false,
				modes: vec![25, 1000]
			}]
		);
	}

	/// Output arrives in chunks of whatever size the pty hands over, so the state machine has to
	/// carry across a boundary drawn anywhere — including between the ESC and the `[`.
	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scanner = SaveModes::default();
		assert!(scanner.feed(b"\x1b").is_empty());
		assert!(scanner.feed(b"[?2").is_empty());
		assert!(scanner.feed(b"5").is_empty());
		assert_eq!(scanner.feed(b"s").len(), 1);
	}

	/// A control byte inside a CSI is run where it sits and the sequence carries on around it, which
	/// is what the engine does — so giving up here would leave the engine reading a sequence cmote
	/// never judged (§106).
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert_eq!(scan(b"\x1b[?25\x07s").len(), 1, "BEL is read through");
		assert!(scan(b"\x1b[?25\x18s").is_empty(), "CAN cancels");
		assert!(scan(b"\x1b[?25\x1as").is_empty(), "SUB cancels");
	}

	/// One slot per mode, newest value winning — XTSAVE has no depth, unlike §85's stack.
	#[test]
	fn saving_a_mode_twice_keeps_the_second_value() {
		let mut saved = Saved::default();
		saved.save(25, true);
		assert_eq!(saved.restore(25), Some(true));
		saved.save(25, false);
		assert_eq!(saved.restore(25), Some(false));
		assert_eq!(saved.len(), 1, "one slot, not two");
	}

	/// A mode never saved has no value, and the caller is what turns that into "do nothing" —
	/// deliberately NOT xterm's zeroed array, which would read an unpaired restore as "reset".
	#[test]
	fn a_mode_never_saved_has_nothing_to_restore() {
		let mut saved = Saved::default();
		saved.save(25, true);
		assert_eq!(saved.restore(1000), None);
	}

	/// The feed is the engine's own spelling of a mode change, which is the whole reason cmote is not
	/// a second writer here.
	#[test]
	fn the_feed_is_the_engines_own_mode_sequence() {
		assert_eq!(mode_feed(25, true), b"\x1b[?25h".to_vec());
		assert_eq!(mode_feed(1006, false), b"\x1b[?1006l".to_vec());
	}
}

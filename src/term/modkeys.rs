// term/modkeys.rs — track the remote's xterm `modifyOtherKeys` mode, and answer when asked what it
// is (PLAN §9, §61).
//
// A few keys the main keyboard can produce have no room in the classic terminal input
// alphabet. Ctrl+letter collapses onto a C0 byte, so Ctrl+I is indistinguishable from Tab
// (both 0x09) and Ctrl+M from Enter (0x0d); Ctrl+digit and most Ctrl+symbol combos have no
// byte at all and are simply lost. Editors (vim, neovim, emacs, kakoune) want those combos
// back, so xterm offers a mode — `modifyOtherKeys`, resource 4 of XTMODKEYS — in which such
// keys are reported unambiguously as `CSI 27 ; <modifier> ; <codepoint> ~` instead. A program
// turns it on by writing to the OUTPUT stream:
//
//   CSI > 4 ; 1 m   — level 1: only the combos that have no ordinary encoding
//   CSI > 4 ; 2 m   — level 2: every Ctrl/Alt combo, including Ctrl+letter
//   CSI > 4 ; 0 m   — off (also `CSI > 4 m`, which restores the initial value)
//
// `alacritty_terminal` does not interpret this private-CSI (it is an input-encoding hint, not
// a screen operation), so — exactly like the cwd announcement (`cwd.rs`) — cmote scans the
// stream for it here and hands the level to the key encoder (`keymap::encode`). Finding the
// sequence in a stream that arrives in arbitrary chunks is `csi::Framer`'s job (§111); what is
// left here is the marker, the resource number and the level.
//
// THE QUESTION, AND WHY IT LIVES HERE (§61). A program may also ASK which level is in force:
//
//   CSI ? 4 m       XTQMODKEYS — "what is resource 4 set to?"
//   CSI > 4 ; Pv m  the answer, which is deliberately the SET form again, so a program can
//                   save the value and later write the reply back verbatim to restore it
//
// `vte` dispatches that to `report_modify_other_keys`, whose body in the `Handler` trait is
// empty and which `alacritty_terminal` never overrides — so until §61 the question fell on the
// floor and the program asking it waited out its timeout. It is answered here rather than in
// `term/query.rs` with the other identity answers for the reason DECSACE lives in `term/rect.rs`
// (§59) and the checksum with it (§60): the module that HOLDS the state is the one place that
// sees the sets and the questions in stream order, so the answer is the level as it stood where
// the question sat, not as the rest of the chunk left it.
//
// ONLY RESOURCE 4 IS ANSWERED, AND THE OTHER SIX ARE REFUSED RATHER THAN MISSING (§61, §163).
//
// XTMODKEYS carries seven resources and ctlseqs numbers them 0, 1, 2, 3, 4, 6 and 7 — there is no
// resource 5 — `modifyKeyboard`, `modifyCursorKeys`, `modifyFunctionKeys`, `modifyKeypadKeys`,
// `modifyOtherKeys`, `modifyModifierKeys` and `modifySpecialKeys`. cmote holds state for exactly one.
//
// The reply format IS an XTMODKEYS control, so there is no spelling of "I do not have that resource":
// an answer for resource 1 would be cmote asserting a level for a knob its key encoder does not have,
// and a program that then SET that resource would be ignored while the next query kept reporting the
// old number. So `report` below is an **allow-list one resource wide** and everything else draws
// silence — the same construction `term/dsr.rs` uses to refuse `CSI ? 26 n` (§36, §82),
// `term/iterm.rs` for OSC 1337 keys and `term/pointer.rs` for pointer shapes, and the same call §60
// made three times over: an invented number is worse than a missing one.
//
// **That makes this a refusal cmote performs, not a gap**, which is what §163 corrected: the matrix
// had read ❌ since §68 on the true observation that there is no way to say "not mine", and read past
// what cmote does about it. Silence here is chosen, reached through a parse, and pinned by a test that
// names all six. In practice the six are not asked; resource 4 is the one editors probe.

/// The XTMODKEYS resource number for `modifyOtherKeys` — the one resource cmote holds.
///
/// The same `CSI > Pp ; Pv m` shape carries six others (0, 1, 2, 3, 6, 7 — ctlseqs skips 5), which
/// cmote does not act on, so the resource is checked before the value is applied and again before a
/// question about it is answered.
const MODIFY_OTHER_KEYS: u16 = 4;

/// Every resource XTMODKEYS carries EXCEPT cmote's own, for the test that pins the refusal (§163).
///
/// Written out rather than derived from a range because the numbering has a hole in it: ctlseqs
/// defines 0, 1, 2, 3, 4, 6 and 7, and a `0..=7` loop would assert something about a resource number
/// that does not exist. `term/dsr.rs` names all nine of its refused reports for the same reason.
#[cfg(test)]
const OTHER_RESOURCES: [u16; 6] = [0, 1, 2, 3, 6, 7];

/// How aggressively the remote asked us to report modified "other" keys (§9). `Off` is the
/// default and the state a well-behaved program restores on exit; `Level1` fills only the gaps
/// (combos with no ordinary byte), `Level2` reports every Ctrl/Alt combo. `keymap::encode` reads
/// this to decide whether a Ctrl/Alt character key becomes the `CSI 27 ; mod ; code ~` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModifyOtherKeys {
	#[default]
	Off,
	Level1,
	Level2,
}

/// The `modifyOtherKeys` tracker (§9) and answerer (§61). Feed it every byte of shell output; it
/// keeps the level the remote last selected, reports the bytes owed to any question about it, and
/// ignores everything else in the stream.
#[derive(Debug, Default)]
pub struct ModKeys {
	/// The CSI grammar, shared with the other scanners (§111). What is left in this module is which
	/// marker opened the sequence — the whole difference between an order and a question (§61).
	framer: super::csi::Framer,
	level: ModifyOtherKeys,
}

impl ModKeys {
	/// Scan a chunk of shell output for a `modifyOtherKeys` change or a question about one, and
	/// return the reply bytes owed. Safe at any chunk boundary — the state machine carries over
	/// between calls.
	///
	/// The reply is built the moment the question's final byte is read, so it carries the level as
	/// it stood at that point in the stream. A chunk that sets the level and then asks reports the
	/// new one; a chunk that asks and then sets reports the old one. Both are what a terminal
	/// reading the stream in order would say, and neither costs a second pass.
	///
	/// Empty for the overwhelmingly common chunk that carries neither, so ordinary output allocates
	/// nothing.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
		// Destructured so the closure below can hold `level` while `framer` is borrowed for the
		// scan — two disjoint fields, which reads plainer than relying on the borrow checker to
		// see that.
		let Self { framer, level } = self;
		let mut replies = Vec::new();
		framer.feed(bytes, |_, csi| {
			// `m` is the final byte of both XTMODKEYS and XTQMODKEYS, and neither carries an
			// intermediate — `CSI > 4 SP m` is some other sequence on the same marker, which is the
			// near-miss rule §56 wrote down. Both spell their parameters with `;`, so a `:` is that
			// same rule applied to the separator (`Csi::sub_parameters`). The offset is not wanted at
			// all: nothing here is fed back to the engine, so there is no advance to line up against.
			if csi.final_byte() != b'm' || !csi.intermediates().is_empty() || csi.sub_parameters() {
				return;
			}
			match csi.marker() {
				// XTMODKEYS, which SETS a resource.
				Some(b'>') => apply(csi, level),
				// XTQMODKEYS, which asks what one is. `?` opens every DECSET and DECRST as well —
				// the common case by a wide margin — and those end on `h` or `l`, so they never
				// reach here.
				Some(b'?') => replies.extend_from_slice(&report(csi, *level)),
				// No marker at all is an SGR (`CSI 0 m`), and `<` or `=` is a private sequence that
				// is not this one.
				_ => {}
			}
		});
		replies
	}

	/// The level the remote last selected, or `Off` if it never asked (§9).
	pub fn level(&self) -> ModifyOtherKeys {
		self.level
	}
}

/// Read a `CSI > Pp ; Pv m` and set the level if it names resource 4.
///
/// `CSI > 4 m` (no value) and `CSI > 4 ; 0 m` both mean off; `1` and `2` select the levels; any
/// larger value is clamped to level 2, matching xterm. A sequence for another resource (0/1/2)
/// leaves the level untouched.
///
/// The resource is compared against `Some(4)`, so an OMITTED one does not match — `CSI > ; 2 m`
/// names no resource and changes nothing, which is the same answer the hand-rolled parse gave by
/// refusing an empty field (§111).
fn apply(csi: &super::csi::Csi<'_>, level: &mut ModifyOtherKeys) {
	if csi.param(0) != Some(MODIFY_OTHER_KEYS) {
		return;
	}
	*level = match csi.param(1) {
		Some(1) => ModifyOtherKeys::Level1,
		Some(value) if value >= 2 => ModifyOtherKeys::Level2,
		// 0, an omitted value, or no value at all: back to the default.
		_ => ModifyOtherKeys::Off,
	};
}

/// The bytes owed to a `CSI ? Pp m`, or nothing when the question was not about resource 4 (§61).
///
/// The answer is spelled as the SET form, `CSI > 4 ; Pv m`, which is xterm's own choice and a
/// good one: what comes back is exactly the sequence that would put the terminal into the
/// state it is in, so a program can pocket the reply and write it back on the way out without
/// understanding a byte of it.
///
/// A question naming any other resource, or naming none — the parameter defaults to 0, which is
/// `modifyKeyboard` — goes unanswered. cmote has one of these resources and inventing the other
/// six would be a number a program could act on and cmote would not honour. `CSI ? 4 ; 1 m` is
/// refused for a different reason: XTQMODKEYS takes one parameter, so a second one means the
/// sequence was not the one it looks like, and §54's rule is that malformed input is a no-op
/// rather than a guess. `param_count` is what asks that, because an empty second parameter is
/// still a second parameter — `CSI ? 4 ; m` is not this sequence either.
fn report(csi: &super::csi::Csi<'_>, level: ModifyOtherKeys) -> Vec<u8> {
	if csi.param(0) != Some(MODIFY_OTHER_KEYS) || csi.param_count() != 1 {
		return Vec::new();
	}
	let value = match level {
		ModifyOtherKeys::Off => 0,
		ModifyOtherKeys::Level1 => 1,
		ModifyOtherKeys::Level2 => 2,
	};
	format!("\x1b[>{MODIFY_OTHER_KEYS};{value}m").into_bytes()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh tracker and read the level.
	fn track(bytes: &[u8]) -> ModifyOtherKeys {
		let mut modkeys = ModKeys::default();
		modkeys.feed(bytes);
		modkeys.level()
	}

	#[test]
	fn the_default_level_is_off() {
		assert_eq!(ModKeys::default().level(), ModifyOtherKeys::Off);
	}

	#[test]
	fn level_two_is_recognised() {
		// The sequence vim/neovim/emacs send to ask for full key disambiguation.
		assert_eq!(track(b"\x1b[>4;2m"), ModifyOtherKeys::Level2);
	}

	#[test]
	fn level_one_is_recognised() {
		assert_eq!(track(b"\x1b[>4;1m"), ModifyOtherKeys::Level1);
	}

	#[test]
	fn the_mode_can_be_turned_back_off() {
		// A program resets on exit — both spellings mean off, and must clear a set level.
		let mut modkeys = ModKeys::default();
		modkeys.feed(b"\x1b[>4;2m");
		modkeys.feed(b"\x1b[>4;0m");
		assert_eq!(modkeys.level(), ModifyOtherKeys::Off);
		modkeys.feed(b"\x1b[>4;2m");
		modkeys.feed(b"\x1b[>4m");
		assert_eq!(modkeys.level(), ModifyOtherKeys::Off);
	}

	#[test]
	fn another_xtmodkeys_resource_is_ignored() {
		// `> 1 ; 2 m` is modifyCursorKeys, not modifyOtherKeys: it must not disturb our level.
		let mut modkeys = ModKeys::default();
		modkeys.feed(b"\x1b[>4;2m");
		modkeys.feed(b"\x1b[>1;2m");
		assert_eq!(modkeys.level(), ModifyOtherKeys::Level2);
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks, including a split mid-parameter.
		let mut modkeys = ModKeys::default();
		modkeys.feed(b"text\x1b[>4;");
		modkeys.feed(b"2mmore");
		assert_eq!(modkeys.level(), ModifyOtherKeys::Level2);
	}

	#[test]
	fn surrounding_output_does_not_confuse_the_scanner() {
		// The sequence embedded between ordinary text is still picked out.
		assert_eq!(track(b"hi\x1b[>4;2mbye"), ModifyOtherKeys::Level2);
	}

	#[test]
	fn an_ordinary_csi_does_not_trigger_the_mode() {
		// A plain SGR colour (`ESC [ 3 1 m`) has no `>` marker, so the level stays off.
		assert_eq!(track(b"\x1b[31mred\x1b[0m"), ModifyOtherKeys::Off);
	}

	#[test]
	fn a_value_above_two_clamps_to_level_two() {
		assert_eq!(track(b"\x1b[>4;9m"), ModifyOtherKeys::Level2);
	}

	/// Feed one byte slice to a fresh tracker and read the bytes it owes back (§61).
	fn ask(bytes: &[u8]) -> Vec<u8> {
		let mut modkeys = ModKeys::default();
		modkeys.feed(bytes)
	}

	#[test]
	fn a_question_is_answered_in_the_set_form() {
		// xterm's own choice, and the reason the reply is not a bespoke report: what comes back is
		// exactly the sequence that would restore this state, so a program can pocket it and write
		// it back on the way out.
		assert_eq!(ask(b"\x1b[?4m"), b"\x1b[>4;0m".to_vec());
		assert_eq!(ask(b"\x1b[>4;2m\x1b[?4m"), b"\x1b[>4;2m".to_vec());
		assert_eq!(ask(b"\x1b[>4;1m\x1b[?4m"), b"\x1b[>4;1m".to_vec());
	}

	#[test]
	fn the_answer_is_the_level_where_the_question_sat() {
		// Both orders in one chunk. Answering from the level the chunk ENDED at would report 2 for
		// the second of these, which is a level that was not in force when the question was asked.
		assert_eq!(ask(b"\x1b[>4;2m\x1b[?4m"), b"\x1b[>4;2m".to_vec());
		assert_eq!(ask(b"\x1b[?4m\x1b[>4;2m"), b"\x1b[>4;0m".to_vec());
	}

	/// The refusal, named resource by resource (§61, §163).
	///
	/// XTMODKEYS carries seven resources and cmote holds one. An answer for `modifyCursorKeys` would
	/// be a level asserted for a knob the key encoder does not have — the invented number §60 refused
	/// three times — and a program that then set it would be ignored while this kept reporting the
	/// number it first invented. So the answer is an allow-list one resource wide, and this test is
	/// what makes that a refusal rather than an oversight: all six are named, the way `term/dsr.rs`
	/// names all nine of its refused reports.
	#[test]
	fn a_question_about_another_resource_goes_unanswered() {
		for resource in OTHER_RESOURCES {
			let question = format!("\x1b[?{resource}m").into_bytes();
			assert!(
				ask(&question).is_empty(),
				"resource {resource} is not cmote's to report"
			);
		}
		// An omitted parameter defaults to 0, `modifyKeyboard`, likewise not ours.
		assert!(ask(b"\x1b[?m").is_empty());
		// And the one that IS ours still answers, so the allow-list is not simply silent.
		assert_eq!(ask(b"\x1b[?4m"), b"\x1b[>4;0m".to_vec());
	}

	/// Setting one of the other six leaves cmote's own level alone — the write half of the same
	/// allow-list (§163). `another_xtmodkeys_resource_is_ignored` above checks one resource; this
	/// checks that none of the six can reach the level, at either value that would change it.
	#[test]
	fn setting_another_resource_never_moves_our_level() {
		for resource in OTHER_RESOURCES {
			for value in [1, 2] {
				let mut modkeys = ModKeys::default();
				modkeys.feed(format!("\x1b[>{resource};{value}m").as_bytes());
				assert_eq!(
					modkeys.level(),
					ModifyOtherKeys::Off,
					"resource {resource} set to {value} moved modifyOtherKeys"
				);
			}
		}
	}

	#[test]
	fn a_second_parameter_makes_it_someone_elses_sequence() {
		// XTQMODKEYS takes one parameter. Two means this is not the sequence it looks like, and
		// §54's rule is that malformed input is a no-op rather than a guess.
		assert!(ask(b"\x1b[?4;1m").is_empty());
	}

	#[test]
	fn a_private_mode_is_not_mistaken_for_a_question() {
		// `?` opens every DECSET and DECRST there is — the common case by a wide margin. They are
		// collected as far as their own final byte and then dropped, and must never answer.
		assert!(ask(b"\x1b[?1049h\x1b[?2004h\x1b[?25l\x1b[?1000;1006h").is_empty());
		// Nor may the query marker leak into a set: `? 4 m` must not change the level.
		let mut modkeys = ModKeys::default();
		modkeys.feed(b"\x1b[>4;2m\x1b[?4m");
		assert_eq!(modkeys.level(), ModifyOtherKeys::Level2);
	}

	#[test]
	fn a_question_split_across_chunks_is_still_answered() {
		let mut modkeys = ModKeys::default();
		assert!(modkeys.feed(b"\x1b[>4;2mtext\x1b[?").is_empty());
		assert_eq!(modkeys.feed(b"4mmore"), b"\x1b[>4;2m".to_vec());
	}

	#[test]
	fn ordinary_output_owes_nothing() {
		// The common chunk must allocate nothing and say nothing.
		assert!(ask(b"\x1b[31mred\x1b[0m\x1b[2J\x1b[1;1Hhello").is_empty());
	}

	/// A byte the engine reads STRAIGHT THROUGH must not change what this module makes of a
	/// sequence — the §106 rule, which this scanner did not obey until the grammar was shared.
	///
	/// It is the last of the ten to get it. The engine has no live arm behind either of these
	/// sequences, so there is nothing acting alone today; what the rule buys is that a version bump
	/// filling that empty handler body cannot make the two disagree (§111).
	#[test]
	fn a_byte_the_engine_reads_through_does_not_abandon_the_sequence() {
		assert_eq!(track(b"\x1b[>4;\n2m"), ModifyOtherKeys::Level2);
		assert_eq!(ask(b"\x1b[?\x7f4m"), b"\x1b[>4;0m".to_vec());
		// CAN and SUB are the only two bytes that really cancel a sequence in flight.
		assert_eq!(track(b"\x1b[>4;\x182m"), ModifyOtherKeys::Off);
		assert!(ask(b"\x1b[?4\x1am").is_empty());
	}

	/// An intermediate byte makes it some other sequence on the same marker — the near-miss rule
	/// §56 wrote down, which this scanner used to get by accident: its old machine had no state for
	/// an intermediate and abandoned the sequence on one.
	#[test]
	fn an_intermediate_byte_rules_both_out() {
		assert_eq!(track(b"\x1b[>4;2 m"), ModifyOtherKeys::Off);
		assert!(ask(b"\x1b[?4 m").is_empty());
	}

	/// A hostile stream must not be able to make the scanner buffer without bound — and the two
	/// bounds answer differently on purpose (§111).
	///
	/// This module's own bound counted BYTES and abandoned the sequence over a long digit run, which
	/// is the §106 defect shape: the engine saturates the number and carries on. It clamps now.
	#[test]
	fn the_two_parameter_bounds_answer_differently() {
		// More parameters than the engine's array holds: the engine ignores the whole sequence, so
		// the scanner does too. The run is `4;2;2;…`, so if it were framed the level would move.
		let list = |fields: usize| {
			let params = std::iter::once("4")
				.chain(std::iter::repeat_n("2", fields - 1))
				.collect::<Vec<_>>()
				.join(";");
			format!("\x1b[>{params}m").into_bytes()
		};
		let bound = super::super::csi::MAX_PARAMS;
		assert_eq!(
			track(&list(bound)),
			ModifyOtherKeys::Level2,
			"thirty-two parameters still fit"
		);
		assert_eq!(
			track(&list(bound + 1)),
			ModifyOtherKeys::Off,
			"thirty-three do not"
		);

		// A runaway DIGIT run is clamped instead, and the sequence LIVES. The clamped resource is
		// not 4, so the level is left alone — but by not being ours rather than by being abandoned.
		let mut digits = b"\x1b[>".to_vec();
		digits.extend(std::iter::repeat_n(b'4', 500));
		digits.extend_from_slice(b";2m");
		assert_eq!(track(&digits), ModifyOtherKeys::Off);
	}

	/// XTMODKEYS spells `Pp ; Pv` with a semicolon, so a `:` means this was not that sequence — the
	/// near-miss rule §56 wrote down, applied to the separator (`Csi::sub_parameters`).
	#[test]
	fn a_sub_parameter_is_not_this_sequence() {
		assert_eq!(track(b"\x1b[>4:2m"), ModifyOtherKeys::Off);
		assert!(ask(b"\x1b[?4:1m").is_empty());
		// The `;` spelling of the same two numbers IS the sequence, so this is about the separator.
		assert_eq!(track(b"\x1b[>4;2m"), ModifyOtherKeys::Level2);
	}

	/// An empty second parameter is still a second parameter, so the question is not this one.
	#[test]
	fn a_trailing_separator_is_a_second_parameter() {
		assert!(ask(b"\x1b[?4;m").is_empty());
		assert_eq!(ask(b"\x1b[?4m"), b"\x1b[>4;0m".to_vec(), "one is ours");
	}

	/// Leading zeros do not change what a parameter means, which is what the engine's own saturating
	/// fold makes of them (§111).
	#[test]
	fn leading_zeros_still_name_resource_four() {
		assert_eq!(track(b"\x1b[>0004;0002m"), ModifyOtherKeys::Level2);
		assert_eq!(ask(b"\x1b[?004m"), b"\x1b[>4;0m".to_vec());
	}
}

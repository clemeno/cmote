// term/c1.rs — whether cmote's own replies are written with 7-bit or 8-bit C1 controls (PLAN §145).
//
//   ESC SP F    S7C1T — send C1 controls as their two-byte ESC form (the default)
//   ESC SP G    S8C1T — send C1 controls as single bytes in 0x80–0x9f
//
// Every C1 control has two spellings. `CSI` is `ESC [` or the single byte 0x9b; `DCS` is `ESC P` or
// 0x90; `ST` is `ESC \` or 0x9c. A terminal READS both, always — `vte` does, and so does every C1
// control cmote sends back — and this pair of sequences chooses which one it WRITES. From xterm's
// ctlseqs, the whole of what they govern is "its responses to queries".
//
// So this is not a parser setting and changes nothing about what cmote understands. It changes the
// bytes of an answer, which is why the whole implementation is one flag on the reply buffer and one
// rewrite as the buffer is sealed.
//
// `vte` sends `ESC SP F` and `ESC SP G` to `unhandled!()` — its `esc_dispatch` has no arm for the
// space intermediate at all — so like every other sequence in this directory they are found beside
// the stream.
//
// WHY IT IS IMPLEMENTED AT ALL, given that nothing in a modern stack asks for it. Because the answer
// to "will you?" is the sequence's own, and an unanswered switch is worse than a refused one: a
// program that sends S8C1T and then parses a reply looking for 0x9b gets an `ESC [` it does not
// match, and waits. Terminals that implement this have been the norm since the VT220; a terminal
// that reads the sequence and does nothing is one that lies by omission about which form it speaks.
//
// THE HAZARD, which is the requester's and is stated rather than guarded against. The single-byte C1
// controls are not valid UTF-8. cmote's own parser decodes UTF-8 always (§67), and so, usually, does
// whatever is reading on the other end of the pty. A program that asks for 8-bit controls and then
// reads its terminal's replies through a UTF-8 decoder will see replacement characters where the
// introducer was. That is a consequence of the request, not of this implementation — the sequence
// exists precisely to ask for those bytes — and the terminal's job is to answer what it was asked.
// It is one sequence to undo (`ESC SP F`) and cmote powers up in the 7-bit form, so nothing arrives
// in this state by accident.
//
// WHAT THE REWRITE TOUCHES, and the one assumption under it. [`encode`] replaces every `ESC`
// followed by a C1 door byte — the seven in [`DOORS`] — with the single byte that means the same
// thing, across the whole run of replies it is given. It does NOT parse the replies to find their
// introducers, and that is only safe because **no reply cmote sends carries an ESC inside a
// payload**: the only payload-bearing replies are DECRQSS (a sequence's parameters and final byte),
// XTGETTCAP (hex and `=`), and the OSC colour answers (`rgb:` and hex digits), and none of those
// alphabets contains one. The test below holds every reply builder in the crate to that.
//
// WHEN IT IS APPLIED, and why the reply buffer carries a watermark. A chunk can change the setting
// half way through, and the replies formed before the change belong to the old form. So the buffer
// is SEALED — everything not yet encoded is encoded, in the form in force — at the moment the
// setting changes, and again as the buffer is drained. The watermark is what keeps the first pass
// from being undone by the second: `encode` is idempotent (a converted introducer has no ESC left to
// find), but a 7-bit prefix followed by a switch to 8-bit would otherwise be converted whole by the
// final pass, retroactively rewriting answers the program had already been promised in the other
// form.
//
// ONE DIVERGENCE, stated because it is real and small: the replies `term/query.rs` builds are formed
// AFTER the chunk, so they are sealed in the setting as the chunk LEFT it, not as it stood where the
// question sat. `CSI c` then `ESC SP G` in one write therefore answers in 8-bit where xterm would
// answer in 7. That is the same trade §33 took for DECRQSS reading the pen as the chunk left it, and
// it needs a program that switches the form after asking rather than before.

/// The C1 controls that have a two-byte form, as `(the byte after ESC, the single byte it means)`.
///
/// The rule is mechanical — a C1 at `0x80 + n` is written `ESC` then `0x40 + n` — so this table is
/// derivable rather than remembered. It is written out anyway, because what belongs here is not
/// every value the arithmetic allows but the **doors**: the seven introducers that can begin or end
/// a reply. `ESC 7` (DECSC) is `0x37` and would map to `0xf7` under a blind subtraction, which is not
/// a C1 control at all and is a glyph in every encoding cmote might be talking to.
const DOORS: [(u8, u8); 7] = [
	(b'[', 0x9b),  // CSI
	(b']', 0x9d),  // OSC
	(b'P', 0x90),  // DCS
	(b'\\', 0x9c), // ST
	(b'^', 0x9e),  // PM
	(b'_', 0x9f),  // APC
	(b'X', 0x98),  // SOS
];

/// The escape byte, the first half of every two-byte C1.
const ESC: u8 = 0x1b;

/// The C1 form cmote powers up in, and returns to on a hard reset: 7-bit, the two-byte spelling.
///
/// DEC's published DECSTR list does not name this setting, so the SOFT reset leaves it alone — §72
/// was careful not to widen that list and this section does not widen it either. RIS does reset it,
/// which is what "power-on state" means.
pub const DEFAULT_EIGHT_BIT: bool = false;

/// The S7C1T / S8C1T scanner (§145). Feed it every byte of shell output; it reports where each
/// switch sat and which form it asked for — `true` for 8-bit.
///
/// The escape grammar is [`super::dcs::Framer`]'s (§111); what is left here is one question, which
/// of two final bytes under one intermediate arrived.
///
/// The cap is zero: this scanner reads no control string, so no payload is buffered on its account.
#[derive(Debug, Default)]
pub struct Controls {
	escapes: super::dcs::Framer<0>,
}

impl Controls {
	/// Scan a chunk of shell output, returning each switch and where it sat. Safe at any chunk
	/// boundary — the state machine carries over between calls, so a sequence may be split anywhere,
	/// including between the ESC and the space.
	///
	/// Each offset is ONE PAST the sequence's final byte. Here the side is not arbitrary the way it is
	/// for the character sets (§143): the switch seals the reply buffer, so it has to land after every
	/// reply the bytes in front of it produced and before every reply the bytes behind it will.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, bool)> {
		let mut switches = Vec::new();
		self.escapes.feed(bytes, |span, control| {
			if let super::dcs::Control::Escape(escape) = control
				&& escape.intermediates() == *b" "
				&& let Some(eight_bit) = form(escape.final_byte())
			{
				switches.push((span.past(), eight_bit));
			}
		});
		switches
	}
}

/// Which form a final byte under the space intermediate asks for, or `None` for one of the family's
/// other members.
///
/// The near miss worth naming: `ESC SP L`, `ESC SP M` and `ESC SP N` are the ANSI conformance levels,
/// which sit under the same intermediate and are a different question — they choose which level of
/// the standard the terminal claims, which is DECSCL's territory and refused for DECSCL's reasons
/// (part 6, §98). An intermediate-only match would answer all five.
fn form(final_byte: u8) -> Option<bool> {
	match final_byte {
		b'F' => Some(false),
		b'G' => Some(true),
		_ => None,
	}
}

/// Rewrite a run of replies into the C1 form asked for.
///
/// `eight_bit` false is the identity — the two-byte form is what every reply is built in, so the
/// default costs one comparison and no allocation.
///
/// Returns a fresh `Vec` rather than editing in place because the result is SHORTER than the input,
/// and a shrink in place would mean either a copy back or a `retain` with an index-carrying closure;
/// the caller splices it over the tail it handed in. Replies are rare and short, so the allocation is
/// not on any path that matters.
pub fn encode(bytes: &[u8], eight_bit: bool) -> Vec<u8> {
	if !eight_bit {
		return bytes.to_vec();
	}
	let mut out = Vec::with_capacity(bytes.len());
	let mut index = 0;
	while index < bytes.len() {
		// A trailing lone ESC cannot appear in a complete reply, but `get` rather than an index keeps
		// that a no-op instead of a panic if one ever does.
		if bytes[index] == ESC
			&& let Some(&next) = bytes.get(index + 1)
			&& let Some(&(_, single)) = DOORS.iter().find(|&&(door, _)| door == next)
		{
			out.push(single);
			index += 2;
		} else {
			out.push(bytes[index]);
			index += 1;
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Scan a whole chunk in one go.
	fn scan(bytes: &[u8]) -> Vec<(usize, bool)> {
		Controls::default().feed(bytes)
	}

	#[test]
	fn each_switch_is_found_just_past_its_final_byte() {
		assert_eq!(scan(b"\x1b G"), vec![(3, true)]);
		assert_eq!(scan(b"ab\x1b Fcd"), vec![(5, false)]);
		assert_eq!(scan(b"\x1b G\x1b F"), vec![(3, true), (6, false)]);
	}

	/// The near miss this scanner is built around: the ANSI conformance levels wear the same
	/// intermediate and are a different question entirely.
	#[test]
	fn the_conformance_levels_are_not_control_switches() {
		for final_byte in *b"LMN" {
			let sequence = [b"\x1b ".as_slice(), &[final_byte]].concat();
			assert!(
				scan(&sequence).is_empty(),
				"ESC SP {} is a conformance level",
				char::from(final_byte)
			);
		}
	}

	/// And the intermediate is what makes it this family at all — `ESC F` and `ESC G` without it are
	/// somebody else's two bytes.
	#[test]
	fn the_space_intermediate_is_required() {
		assert!(scan(b"\x1bG").is_empty());
		assert!(scan(b"\x1bF").is_empty());
		assert!(scan(b"\x1b(G").is_empty(), "a charset slot is not a space");
		assert!(scan(b"\x1b  G").is_empty(), "two spaces is not one");
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_found() {
		let mut scanner = Controls::default();
		assert!(scanner.feed(b"\x1b").is_empty());
		assert!(scanner.feed(b" ").is_empty());
		assert_eq!(scanner.feed(b"G"), vec![(1, true)]);
	}

	/// `ESC` then a C0 stays in the escape state for the engine, so a switch with a line feed in the
	/// middle of it still switches — the rule four hand-rolled watchers got wrong before the shared
	/// grammar existed (§111).
	#[test]
	fn a_control_byte_does_not_abandon_the_sequence() {
		assert_eq!(scan(b"\x1b\n G").len(), 1, "LF is read through");
		assert!(scan(b"\x1b\x18 G").is_empty(), "CAN cancels");
	}

	#[test]
	fn the_seven_bit_form_is_the_identity() {
		let reply = b"\x1b[?62;4c\x1bP1$r0m\x1b\\";
		assert_eq!(encode(reply, false), reply.to_vec());
	}

	/// Every door, and the arithmetic behind the table: a C1 at `0x80 + n` is written `ESC` then
	/// `0x40 + n`, so each single byte is its door plus 0x40.
	#[test]
	fn every_door_maps_to_its_own_single_byte() {
		for (door, single) in DOORS {
			assert_eq!(
				encode(&[ESC, door], true),
				vec![single],
				"ESC {} is one byte",
				char::from(door)
			);
			assert_eq!(
				u16::from(door) + 0x40,
				u16::from(single),
				"the C1 arithmetic, not a table typed twice"
			);
		}
	}

	/// The whole point of the doors being a LIST rather than a subtraction: `ESC 7` is DECSC and
	/// `0x37 + 0x40` is `0x77`, which is the letter `w`. A blind rewrite would turn a saved cursor
	/// into a glyph.
	#[test]
	fn an_escape_that_is_not_a_door_is_left_alone() {
		assert_eq!(encode(b"\x1b7", true), b"\x1b7".to_vec());
		assert_eq!(encode(b"\x1bc", true), b"\x1bc".to_vec());
		assert_eq!(encode(b"\x1b", true), b"\x1b".to_vec(), "a lone ESC");
	}

	/// A real reply, both ends of it: the DCS introducer and the ST that closes it.
	#[test]
	fn a_control_string_reply_loses_both_of_its_escapes() {
		assert_eq!(
			encode(b"\x1bP1$r0m\x1b\\", true),
			vec![0x90, b'1', b'$', b'r', b'0', b'm', 0x9c]
		);
	}

	/// Idempotent, which is what lets the buffer be sealed more than once without a converted prefix
	/// being converted again — there is no ESC left in it to find.
	#[test]
	fn encoding_twice_changes_nothing_the_second_time() {
		let once = encode(b"\x1b[?62c", true);
		assert_eq!(encode(&once, true), once);
	}

	/// The assumption the whole-run rewrite rests on, held to every reply the crate builds: **every
	/// ESC in a reply is followed by a C1 door**, so there is no ESC in a payload for the rewrite to
	/// touch. If a future reply ever carries one, this fails rather than quietly rewriting a payload
	/// byte into a control.
	///
	/// Stated as "every ESC is a door" rather than "nothing between the first and last is an ESC",
	/// which is where this test started and which is wrong: `gettcap_reply` given three names answers
	/// with three concatenated DCS strings, so a reply builder does not always return ONE sequence.
	/// That is also why [`encode`] walks a run rather than looking at its ends.
	#[test]
	fn no_reply_payload_carries_an_escape() {
		use super::super::{dsr, locator, presentation, query};
		let replies: Vec<Vec<u8>> = vec![
			query::version_reply("9.9.9"),
			query::da3_reply("00434D45"),
			query::decrqss_reply("0", "m"),
			query::decrqss_unsupported_reply(),
			query::gettcap_reply(&[b"544E".to_vec(), b"436F".to_vec(), b"525242".to_vec()]),
			query::displayed_extent_reply(24, 80),
			dsr::cursor_reply(3, 4),
			dsr::NO_LOCATOR.to_vec(),
			dsr::NO_LOCATOR_TYPE.to_vec(),
			dsr::DARK_SCHEME.to_vec(),
			locator::UNAVAILABLE.to_vec(),
			presentation::tab_stop_report([0, 8, 16].into_iter()),
		];
		for reply in replies {
			let mut index = 0;
			while index < reply.len() {
				if reply[index] == ESC {
					let next = reply.get(index + 1).copied();
					assert!(
						next.is_some_and(|byte| DOORS.iter().any(|&(door, _)| door == byte)),
						"an escape that is not a door, at {index} of {reply:?}"
					);
					index += 2;
				} else {
					index += 1;
				}
			}
		}
	}
}

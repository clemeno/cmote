// term/osc.rs — frame OSC strings out of the byte stream, once, for every scanner that reads them.
//
// Several features need to sniff an OSC sequence the engine ignores: the remote working directory
// (§17), the shell-integration prompt marks (§34), the progress a remote command reports (§54), and
// the icon name a remote gives its tab (§69).
// Each one cares about a DIFFERENT sequence, but every one of them first has to solve the same
// problem — find where an OSC string starts and ends in a stream that arrives in arbitrary chunks.
//
// Two of them then need the same SECOND thing, which is why `sanitize` is down there beside the
// framer: text a remote chose, about to be drawn in chrome cmote owns, has to be stripped and
// bounded first. That rule was written once in `iterm` and copied nowhere — until `icon` needed it,
// at which point one copy was the whole lesson this module already exists to teach.
//
// An OSC string is `ESC ] payload (BEL | ESC \)`. Finding it is a small state machine rather than a
// search over a buffer, because a sequence can be split anywhere: between the ESC and the `]`, in
// the middle of the payload, or between the ESC and the `\` of the terminator. The state carries
// over between `feed` calls, so any split is safe.
//
// That framing used to live three times over, copied into `cwd`, `osc133` and `graphics`, and it had
// already drifted between them. It lives here now, and the scanners above it are left with only the
// part that is actually theirs: deciding what a finished payload MEANS.
//
// This module used to say `graphics` could not share it, because that scanner reads a 16 MB binary
// payload and had to keep scanning past an overflow while this one abandons it — a cap-and-overflow
// policy parameter fitting neither caller. §111 measured that and it was not a difference at all: the
// only byte that can interrupt a control string is an ESC, and an ESC ends it for the engine too, so
// "keep following to the terminator" and "abandon and hunt for the next ESC" are the same machine. What
// remained was the cap itself, which is a const parameter here. `graphics` now reads `dcs::Framer`.
//
// ONE DOOR, THREE FRAMERS. This one, `csi::Framer` and `dcs::Framer` each open on the same ESC and each
// has to obey the same rules about it. §111 found the same two defects in all three — a byte the engine
// reads through between the ESC and the introducer, and an ESC that ends one string while opening the
// next sequence — and fixed them in all three, with `differential.rs` holding each one to the engine's
// own parser. The remaining duplication is the ESC door itself, which PLAN §111 names.

/// The escape and bell bytes that frame an OSC sequence.
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// Where the framer is in the byte stream.
#[derive(Debug, Default, PartialEq, Eq)]
enum OscScan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; an OSC starts if the next byte is `]`.
	Escape,
	/// Inside an OSC payload, collecting it until the terminator.
	Payload,
	/// Saw ESC inside a payload; the string ends if the next byte is `\` (ST).
	PayloadEscape,
}

/// Cuts OSC strings out of shell output. `CAP` is the longest payload this framer will buffer;
/// past it the payload is abandoned and the framer resumes hunting for the next sequence, so a
/// hostile or broken stream cannot grow our memory without bound (§12).
///
/// The cap is a const parameter rather than a field for two reasons: it lets the scanners built on
/// this one keep deriving `Default` (they are all created by `Terminal::new` with no arguments), and
/// it keeps each caller's limit visible as a named constant in the module that chose it — the caps
/// genuinely differ, because a cwd is a path and a prompt mark is a few bytes.
#[derive(Debug, Default)]
pub struct Framer<const CAP: usize> {
	state: OscScan,
	payload: Vec<u8>,
}

impl<const CAP: usize> Framer<CAP> {
	/// Feed a chunk of shell output, calling `on_payload` once per OSC string that COMPLETES in it.
	///
	/// The payload passed to the callback is the bytes between `]` and the terminator, terminator
	/// excluded. The `usize` is the byte offset in THIS `bytes` slice just past that terminator —
	/// which is what a caller needs when a sequence's meaning depends on where in the stream it sits
	/// (§34 lines a prompt mark up with the grid by advancing the engine exactly that far).
	///
	/// Safe at any chunk boundary: a sequence split across calls completes on the call that carries
	/// its terminator, and the offset is measured in that final chunk.
	pub fn feed(&mut self, bytes: &[u8], mut on_payload: impl FnMut(usize, &[u8])) {
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				OscScan::Text => {
					if byte == ESC {
						self.state = OscScan::Escape;
					}
				}
				OscScan::Escape => match byte {
					b']' => {
						self.payload.clear();
						self.state = OscScan::Payload;
					}
					// ESC ESC: still waiting for the sequence's real first byte.
					ESC => {}
					// A byte the engine reads through while it waits for that first byte: it executes a
					// C0 and STAYS in its escape state (`lib.rs:341`), and ignores DEL and everything
					// past `0x7f` (`:381-383`). So `ESC` LF `] 7 ; /tmp BEL` really is an OSC, and
					// dropping to text here read the `]` as a printable character and lost it — for all
					// four scanners built on this framer at once (§111).
					byte if super::csi::passes_through(byte) => {}
					// CAN and SUB drop the escape back to GROUND, where a `]` starts nothing.
					_ => self.state = OscScan::Text,
				},
				OscScan::Payload => match byte {
					// BEL ends the string; the offset is just past it.
					BEL => {
						on_payload(index + 1, &self.payload);
						self.abandon();
					}
					ESC => self.state = OscScan::PayloadEscape,
					// CAN and SUB. The engine ends the string here — and DISPATCHES what it had
					// (`osc_end` then `execute`, `lib.rs:355-359`) — where cmote abandons it. That is
					// deliberately the stricter side of the engine and it is safe to be: the engine has
					// no handler behind any OSC cmote reads, so nobody acts on one twice. What it fixes
					// is worse than what it refuses — this used to read the cancel INTO the payload and
					// go on waiting, so a later BEL dispatched a cwd with a cancelled path and
					// everything after it glued on (§111).
					0x18 | 0x1a => self.abandon(),
					// The C0s the engine DROPS rather than passing to its handler (`lib.rs:349`): a
					// payload it never sees is not part of the string. DEL and the high bytes are not in
					// this list, because an OSC keeps those — which is the opposite of what a DCS does
					// with them, and is why the two framers spell their payload rules separately.
					0x00..=0x06 | 0x08..=0x17 | 0x19 | 0x1c..=0x1f => {}
					_ => {
						self.payload.push(byte);
						if self.payload.len() > CAP {
							self.abandon();
						}
					}
				},
				OscScan::PayloadEscape => match byte {
					// ESC `\` is the string terminator (ST).
					b'\\' => {
						on_payload(index + 1, &self.payload);
						self.abandon();
					}
					// **ESC does two jobs at once**: it ends this string AND opens the next sequence
					// (§111). What is dropped is the interrupted payload — a string that named no
					// terminator is not answered, §54's rule — and what must NOT be dropped is the
					// sequence that follows, which is what this arm and the two below are for. Another
					// OSC starting here used to be lost entirely: the framer went back to ordinary text
					// and read its payload as printable characters.
					b']' => {
						self.payload.clear();
						self.state = OscScan::Payload;
					}
					// ESC ESC, and the bytes the engine reads through between an ESC and the `\`: the
					// terminator may still arrive, so the payload stays in hand.
					ESC => {}
					byte if super::csi::passes_through(byte) => {}
					_ => self.abandon(),
				},
			}
		}
	}

	/// Reset to hunting for the next sequence, discarding the payload in flight.
	fn abandon(&mut self) {
		self.state = OscScan::Text;
		self.payload.clear();
	}
}

/// Reduce remote-chosen text to something safe to draw in cmote's own chrome: printable characters
/// only, and no longer than `max_chars`.
///
/// The strip and the cap answer two different threats, which is why both are here and neither is
/// optional. Control characters — a newline, a tab, a carriage return — would let a remote disrupt
/// or SPOOF the surface its text is drawn on; the strip is the same rule, and for the same reason,
/// as the window title's (`term::mod::sanitize_title`). One control character never gets this far
/// on the scanner path above: an ESC inside a payload ends the OSC string or invalidates it, so
/// `Framer` has already settled it. The strip is what covers the rest, and what covers a caller
/// that got its text from somewhere other than a framed payload.
///
/// The cap stops a remote filling that surface with a name
/// nobody asked for, and is applied where the value is STORED rather than where it is drawn, so a
/// megabyte of it never sits in memory waiting for a widget to elide it.
///
/// Counted in `chars` rather than bytes so a multi-byte name is cut at a character boundary and
/// cannot panic. Each caller names its own limit, because the surfaces genuinely differ — a branch
/// pill and a tab label have different room.
pub fn sanitize(text: &str, max_chars: usize) -> String {
	text.chars()
		.filter(|character| !character.is_control())
		.take(max_chars)
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Collect every payload a chunk completes, as owned strings, with its end offset.
	fn frame<const CAP: usize>(framer: &mut Framer<CAP>, bytes: &[u8]) -> Vec<(usize, String)> {
		let mut seen = Vec::new();
		framer.feed(bytes, |offset, payload| {
			seen.push((offset, String::from_utf8_lossy(payload).into_owned()));
		});
		seen
	}

	#[test]
	fn a_bel_terminated_string_reports_its_payload_and_end() {
		// The offset is just past the BEL: `ab` (2) + `ESC ] 7 ; x` (5) + `BEL` (1) = 8.
		let mut framer = Framer::<64>::default();
		let seen = frame(&mut framer, b"ab\x1b]7;x\x07");
		assert_eq!(seen, vec![(8, "7;x".to_owned())]);
	}

	#[test]
	fn the_st_terminator_is_accepted_too() {
		// ESC \ instead of BEL, and the offset is past BOTH of its bytes.
		let mut framer = Framer::<64>::default();
		let seen = frame(&mut framer, b"\x1b]133;A\x1b\\");
		assert_eq!(seen, vec![(9, "133;A".to_owned())]);
	}

	#[test]
	fn a_string_split_across_chunks_completes_on_the_chunk_that_ends_it() {
		// The nastiest boundary: the chunk ends right after the ESC, before the `]`. The offset is
		// measured in the FINAL chunk, not in the stream as a whole.
		let mut framer = Framer::<64>::default();
		assert_eq!(frame(&mut framer, b"x\x1b"), vec![]);
		assert_eq!(frame(&mut framer, b"]7;fi"), vec![]);
		assert_eq!(
			frame(&mut framer, b"le\x07"),
			vec![(3, "7;file".to_owned())]
		);
	}

	#[test]
	fn several_strings_in_one_chunk_are_all_reported_in_order() {
		let mut framer = Framer::<64>::default();
		// `ESC ] 1 3 3 ; A BEL` is 8 bytes, so the first ends at 8 and the second at 16.
		let seen = frame(&mut framer, b"\x1b]133;A\x07\x1b]133;C\x07");
		assert_eq!(
			seen,
			vec![(8, "133;A".to_owned()), (16, "133;C".to_owned())]
		);
	}

	#[test]
	fn an_esc_inside_a_payload_that_is_not_st_abandons_the_string() {
		// ESC followed by anything but `\` is malformed; we drop it rather than guess, and the
		// scanner is left hunting text again — so the NEXT sequence still reads.
		let mut framer = Framer::<64>::default();
		assert_eq!(frame(&mut framer, b"\x1b]7;bad\x1bZ\x07"), vec![]);
		assert_eq!(
			frame(&mut framer, b"\x1b]7;ok\x07"),
			vec![(7, "7;ok".to_owned())]
		);
	}

	#[test]
	fn an_esc_esc_keeps_waiting_for_the_real_first_byte() {
		let mut framer = Framer::<64>::default();
		let seen = frame(&mut framer, b"\x1b\x1b]7;x\x07");
		assert_eq!(seen, vec![(7, "7;x".to_owned())]);
	}

	#[test]
	fn a_payload_past_the_cap_is_dropped_not_buffered() {
		// A payload of exactly CAP is still delivered; one byte more is abandoned. The framer keeps
		// working afterwards, so a flood costs us the flooded sequence and nothing else.
		let mut framer = Framer::<8>::default();
		assert_eq!(
			frame(&mut framer, b"\x1b]12345678\x07"),
			vec![(11, "12345678".to_owned())]
		);
		assert_eq!(frame(&mut framer, b"\x1b]123456789\x07"), vec![]);
		assert_eq!(
			frame(&mut framer, b"\x1b]7;x\x07"),
			vec![(6, "7;x".to_owned())]
		);
	}

	#[test]
	fn a_non_osc_escape_sequence_is_ignored() {
		// `ESC c` (full reset) and `ESC [ 0 m` (SGR) are not OSC strings and must not be framed.
		let mut framer = Framer::<64>::default();
		assert_eq!(frame(&mut framer, b"\x1bc\x1b[0mtext"), vec![]);
	}

	#[test]
	fn sanitizing_drops_control_characters_and_keeps_the_text_around_them() {
		// The spoofing case: a remote that wants its text to look like two lines, or to smuggle an
		// escape sequence into a widget that will draw it verbatim. Both bytes go; the rest stays.
		assert_eq!(sanitize("main\nfake prompt", 64), "mainfake prompt");
		assert_eq!(sanitize("a\x1b[31mb", 64), "a[31mb");
	}

	#[test]
	fn sanitizing_cuts_at_a_character_boundary_not_a_byte_one() {
		// Counted in `chars`: four accented letters are eight BYTES, and cutting by byte would
		// split one in half and panic. The cap is a limit on what is stored, so it is exact.
		assert_eq!(sanitize("éèêë", 3), "éèê");
		assert_eq!(sanitize("short", 64), "short");
	}
}

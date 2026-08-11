// term/iterm.rs — read the parts of iTerm2's OSC 1337 namespace cmote honours (PLAN §55).
//
// OSC 1337 is not one sequence. It is iTerm2's private namespace, a `key=value` grab-bag sharing one
// OSC number:
//
//   ESC ] 1337 ; SetMark                        BEL | ST
//   ESC ] 1337 ; CurrentDir=/home/user          BEL | ST
//   ESC ] 1337 ; SetUserVar=gitBranch=<base64>  BEL | ST
//   ESC ] 1337 ; SetProfile=Production          BEL | ST
//   … about twenty more
//
// Because they share a number, "support OSC 1337" is not a decision anyone can make in one go — and
// making it in one go would be actively dangerous, because two of those keys are decisions cmote has
// already taken, wearing a different costume:
//
//   Copy=<base64>              writes the system clipboard. That is OSC 52 write, which is REFUSED
//                              (TERMINAL_COMPATIBILITY_PLAN §6). A remote must not poison the local
//                              clipboard, and it must not get to do it through a second door.
//   SetProfile / SetColors /   repaint the theme. That is the fixed-scheme refusal (§6). The colour
//   SetBackgroundImageFile     scheme is chrome the USER chose. `SetBackgroundImageFile` is worse
//                              still: a remote naming a file for cmote to decode, which is §41's
//                              refusal as well.
//
// So this module is deliberately an ALLOW-LIST, not a parser with a policy bolted on. A key that is
// not named here produces nothing at all, which means a key added to iTerm2 tomorrow is refused by
// default rather than by our remembering to refuse it. The `refuses_*` tests below pin that down for
// the dangerous ones by name, so the guarantee is checked rather than merely intended.
//
// What is honoured, and why each is safe:
//
//   SetMark      A navigable bookmark on the current line. Tab-local, no side effect beyond a tick in
//                cmote's own gutter. Genuinely ADDITIVE over §34: OSC 133's marks are prompt-derived
//                (A/B/C/D bracket a command), so a script cannot mark a point mid-output — before
//                each test suite, each stage of a build. This can.
//
// Two more keys are handled elsewhere rather than here, because another module already owns the
// question they answer:
//
//   CurrentDir=  a third spelling of "the shell's working directory", so it lives beside OSC 7 and
//                OSC 9;9 in `term::cwd` — that module owns which sequences announce a directory.
//
// Framing (finding the sequence in a stream arriving in arbitrary chunks) is `term::osc`'s job.

/// The longest OSC 1337 payload we will buffer. Generous for the keys above — a `CurrentDir` is a
/// path — while bounding what a hostile stream can make us hold (§12). Deliberately far below what
/// an `iTerm2 File=` inline image would need: that key is refused, and refusing to BUFFER it is the
/// cheapest possible way to mean it.
const MAX_PAYLOAD: usize = 4096;

/// Something in the 1337 namespace that cmote acts on. One variant today; the enum is here because
/// the alternative — a scanner that returns bare offsets — would make adding the next honoured key a
/// change to the signature rather than a new arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Report {
	/// `SetMark` — put a navigable bookmark on the line the stream has reached (§55).
	Mark,
}

/// Reads the honoured parts of the 1337 namespace out of the shell's output. Feed it every byte; it
/// returns what completed in that chunk, each with the byte offset just past its terminator, because
/// a mark's meaning is WHERE it sits — the caller advances the engine that far and reads the cursor
/// there, exactly as it does for an OSC 133 mark (§34).
#[derive(Debug, Default)]
pub struct Iterm {
	framer: super::osc::Framer<MAX_PAYLOAD>,
}

impl Iterm {
	/// Scan a chunk, returning every honoured 1337 report that finished in it. Safe at any chunk
	/// boundary — the framer's state carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Report)> {
		let mut reports = Vec::new();
		self.framer.feed(bytes, |offset, payload| {
			if let Some(report) = parse(payload) {
				reports.push((offset, report));
			}
		});
		reports
	}
}

/// Read one OSC payload as a 1337 report, or `None` when it is not one we honour — which covers
/// every other OSC (a title, a cwd, a prompt mark) and every 1337 key not on the allow-list.
fn parse(payload: &[u8]) -> Option<Report> {
	let rest = payload.strip_prefix(b"1337;")?;
	// The allow-list. `SetMark` carries no value, so it is matched whole rather than by prefix: that
	// way `SetMarkAndSomethingElse` is not read as a mark.
	match rest {
		b"SetMark" => Some(Report::Mark),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Every honoured report a chunk completes, with its end offset.
	fn scan(bytes: &[u8]) -> Vec<(usize, Report)> {
		let mut iterm = Iterm::default();
		iterm.feed(bytes)
	}

	#[test]
	fn a_set_mark_is_read_with_the_offset_it_sat_at() {
		// `ESC ] 1 3 3 7 ; S e t M a r k BEL` is 15 bytes, so the offset just past it is 15 — the
		// point the engine is advanced to before the cursor is read for the mark's line.
		assert_eq!(scan(b"\x1b]1337;SetMark\x07"), vec![(15, Report::Mark)]);
	}

	#[test]
	fn the_st_terminator_works_as_well_as_bel() {
		assert_eq!(scan(b"\x1b]1337;SetMark\x1b\\"), vec![(16, Report::Mark)]);
	}

	#[test]
	fn a_mark_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks — including a split inside the key name.
		let mut iterm = Iterm::default();
		assert_eq!(iterm.feed(b"stage 2\r\n\x1b]1337;Set"), vec![]);
		assert_eq!(iterm.feed(b"Mark\x07more"), vec![(5, Report::Mark)]);
	}

	#[test]
	fn several_marks_in_one_chunk_come_back_in_order() {
		let scanned = scan(b"\x1b]1337;SetMark\x07a\x1b]1337;SetMark\x07");
		assert_eq!(scanned, vec![(15, Report::Mark), (31, Report::Mark)]);
	}

	#[test]
	fn a_key_that_merely_starts_with_the_honoured_one_is_not_it() {
		// Matched whole, not by prefix — otherwise a future or hostile key beginning `SetMark` would
		// be read as a mark.
		assert_eq!(scan(b"\x1b]1337;SetMarkNot\x07"), vec![]);
		assert_eq!(scan(b"\x1b]1337;SetMark=1\x07"), vec![]);
	}

	#[test]
	fn refuses_the_clipboard_key_which_is_osc_52_write_by_another_name() {
		// SECURITY (§6). `Copy=` puts base64 on the system clipboard. cmote refuses OSC 52 write
		// because a remote must not poison the local clipboard, and this key must not become a second
		// door to the same thing.
		assert_eq!(scan(b"\x1b]1337;Copy=aGVsbG8=\x07"), vec![]);
	}

	#[test]
	fn refuses_every_key_that_would_repaint_the_theme() {
		// SECURITY-adjacent policy (§6): the colour scheme is chrome the USER chose, so a remote does
		// not get to change it. `SetBackgroundImageFile` is doubly refused — it names a file for cmote
		// to decode, which is §41's refusal too.
		for key in [
			&b"\x1b]1337;SetProfile=Production\x07"[..],
			&b"\x1b]1337;SetColors=bg=ff0000\x07"[..],
			&b"\x1b]1337;SetBackgroundImageFile=L3RtcC9hLnBuZw==\x07"[..],
		] {
			assert_eq!(scan(key), vec![], "honoured {key:?}");
		}
	}

	#[test]
	fn refuses_every_key_whose_effect_would_escape_the_tab() {
		// The line §54 drew: a remote may change what its own tab looks like and nothing more.
		// `StealFocus` raises the window, `RequestAttention` flashes the taskbar button, and
		// `ClearScrollback` destroys the user's own record of the session.
		for key in [
			&b"\x1b]1337;StealFocus\x07"[..],
			&b"\x1b]1337;RequestAttention=yes\x07"[..],
			&b"\x1b]1337;RequestAttention=fireworks\x07"[..],
			&b"\x1b]1337;ClearScrollback\x07"[..],
		] {
			assert_eq!(scan(key), vec![], "honoured {key:?}");
		}
	}

	#[test]
	fn refuses_the_inline_image_key_without_even_buffering_it() {
		// §41's refusal: a remote must not get a PNG/JPEG parser run on bytes it PUSHED into the
		// stream unasked. The payload cap is the second half of meaning it — a real `File=` payload is
		// megabytes, so it overruns the cap, the framer abandons it, and cmote holds none of it.
		let mut iterm = Iterm::default();
		assert_eq!(iterm.feed(b"\x1b]1337;File=name=a.png;size=9:"), vec![]);
		assert_eq!(iterm.feed(&[b'A'; MAX_PAYLOAD + 1]), vec![]);
		assert_eq!(iterm.feed(b"\x07"), vec![]);
		// And the scanner is still working afterwards: a flood costs us the flood and nothing else.
		assert_eq!(
			iterm.feed(b"\x1b]1337;SetMark\x07"),
			vec![(15, Report::Mark)]
		);
	}

	#[test]
	fn an_unknown_key_is_refused_by_default_rather_than_by_being_listed() {
		// The allow-list's whole point: a key nobody here has heard of does nothing. That is what
		// makes a key iTerm2 adds tomorrow safe without a change in this file.
		assert_eq!(scan(b"\x1b]1337;SomethingInventedLater=1\x07"), vec![]);
		assert_eq!(scan(b"\x1b]1337;\x07"), vec![]);
	}

	#[test]
	fn the_other_osc_sequences_are_left_alone() {
		// Every OSC flows through the same framer. A prompt mark, a cwd and a title are all somebody
		// else's business.
		let scanned =
			scan(b"\x1b]133;A\x07\x1b]7;file://h/tmp\x07\x1b]0;title\x07\x1b]9;4;1;5\x07");
		assert_eq!(scanned, vec![]);
	}
}

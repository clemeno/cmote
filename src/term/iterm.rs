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
//   SetUserVar   A named string the shell sets, for cmote to show. iTerm2 lets any name be set and
//   =gitBranch   referenced from a title template; cmote has no template, so the ALLOW-LIST is applied
//                a second time, to the NAMES — only `gitBranch` is kept, the name iTerm2's own git
//                integration uses. That is not a shortcut but the security property: a remote cannot
//                make cmote hold a map it chose the keys of, so there is no unbounded store to bound.
//
//                Everything about this one is a spoofing surface, because the value is drawn in the
//                TAB STRIP — chrome cmote owns and the user reads to know what they are typing into.
//                So the value is base64-decoded (a bad decode is discarded whole, not partially),
//                UTF-8 checked, stripped of control characters, capped in length, and — the part that
//                matters most — shown BESIDE the host label in its own pill, never in place of it. A
//                remote can say what branch it is on. It cannot say what host it is.
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

/// The one user-variable name cmote keeps (§55) — the name iTerm2's own git integration sets. The
/// allow-list applied to names as well as keys: with no title template to reference an arbitrary
/// variable from, keeping others would be a remote-keyed map with no reader.
const HONOURED_VAR: &[u8] = b"gitBranch";

/// The longest variable value cmote keeps, in CHARACTERS. A branch name is short; this is generous for
/// a real one and stops a remote from filling the tab strip, which is the surface the value is drawn
/// on. Applied after decoding, so it bounds what is STORED and not merely what is shown.
const MAX_VALUE_CHARS: usize = 32;

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
	/// The branch the shell last announced (§55), already decoded and sanitised. `None` when nothing
	/// has announced one, and set back to `None` when the shell announces an EMPTY value — which is how
	/// leaving a repository is reported, and so has to clear the pill rather than leave a stale branch
	/// under a directory that has none.
	branch: Option<String>,
}

impl Iterm {
	/// Scan a chunk, returning every honoured 1337 report that finished in it. Safe at any chunk
	/// boundary — the framer's state carries over between calls.
	///
	/// Only the GRID-ANCHORED keys come back as reports; a user variable is a latest-value reading with
	/// no place in the stream, so it is folded in here and read through `branch` — the same division as
	/// between §34's marks and §17's cwd.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Report)> {
		let mut reports = Vec::new();
		let branch = &mut self.branch;
		self.framer.feed(bytes, |offset, payload| {
			if let Some(report) = parse(payload) {
				reports.push((offset, report));
				return;
			}
			// A variable assignment we honour. `Some(None)` is a real answer — the shell said the value
			// is empty — and is distinct from `None`, which means this payload was not an assignment at
			// all and must leave what we hold alone.
			if let Some(value) = parse_user_var(payload) {
				*branch = value;
			}
		});
		reports
	}

	/// The branch the shell last announced, if any (§55). Already safe to draw: decoded, UTF-8, free of
	/// control characters and capped in length.
	pub fn branch(&self) -> Option<&str> {
		self.branch.as_deref()
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

/// Read one OSC payload as an assignment to the variable cmote honours (§55).
///
/// Three-valued on purpose, and the distinction is load-bearing:
///
///   `None`        this payload is not an assignment we act on — a different key, a different variable
///                 name, or a value we could not trust. Whatever we already hold stays.
///   `Some(None)`  the shell announced an EMPTY value: it left the repository. Clear the pill.
///   `Some(Some)`  a value fit to draw.
///
/// A value that fails to decode lands in the first case, not the second. That matters: a remote must
/// not be able to CLEAR a real reading by sending rubbish, the same rule §54 applies to progress.
fn parse_user_var(payload: &[u8]) -> Option<Option<String>> {
	let rest = payload.strip_prefix(b"1337;SetUserVar=")?;
	// `name=<base64>`. The name is matched whole against the one name we keep, so a variable cmote has
	// no reader for is never stored — there is deliberately no map here for a remote to fill.
	let equals = rest.iter().position(|&byte| byte == b'=')?;
	if &rest[..equals] != HONOURED_VAR {
		return None;
	}
	let encoded = &rest[equals + 1..];
	// An empty payload is the shell saying "no branch" and needs no decode. iTerm2's own integration
	// sends this on leaving a repository.
	if encoded.is_empty() {
		return Some(None);
	}

	use base64::Engine as _;
	let decoded = base64::engine::general_purpose::STANDARD
		.decode(encoded)
		.ok()?;
	let text = String::from_utf8(decoded).ok()?;
	let clean = sanitize(&text);
	// A value that was nothing but control characters sanitises to empty. Treated as "no branch"
	// rather than as an empty pill, which would be a smudge on the strip with nothing in it.
	Some((!clean.is_empty()).then_some(clean))
}

/// Reduce a remote-set value to something safe to draw in the tab strip (§55): printable characters
/// only, and no longer than `MAX_VALUE_CHARS`.
///
/// The control-character strip is the same rule — and for the same reason — as the window title's
/// (`term::mod::sanitize_title`): the strip is chrome cmote owns, so a remote must not be able to
/// smuggle a newline or an escape into it. The length cap is counted in `chars` rather than bytes so a
/// multi-byte branch name is cut at a character boundary and cannot panic.
fn sanitize(text: &str) -> String {
	text.chars()
		.filter(|character| !character.is_control())
		.take(MAX_VALUE_CHARS)
		.collect()
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

	/// Feed a chunk to a fresh scanner and read the branch it holds.
	fn branch(bytes: &[u8]) -> Option<String> {
		let mut iterm = Iterm::default();
		iterm.feed(bytes);
		iterm.branch().map(str::to_owned)
	}

	/// The sequence a shell sends to announce `value` as the branch. `bWFpbg==` is `main`, and this
	/// builds the rest the same way so the tests read as the wire does.
	fn set_branch(value: &str) -> Vec<u8> {
		use base64::Engine as _;
		let encoded = base64::engine::general_purpose::STANDARD.encode(value);
		format!("\x1b]1337;SetUserVar=gitBranch={encoded}\x07").into_bytes()
	}

	#[test]
	fn a_branch_is_decoded_from_base64() {
		// The literal wire form, so the test does not merely agree with its own encoder.
		assert_eq!(
			branch(b"\x1b]1337;SetUserVar=gitBranch=bWFpbg==\x07").as_deref(),
			Some("main")
		);
		assert_eq!(
			branch(&set_branch("feature/osc-1337")).as_deref(),
			Some("feature/osc-1337")
		);
	}

	#[test]
	fn an_empty_value_clears_the_branch_because_that_is_leaving_a_repository() {
		// The `cd` out of a repo. A stale branch under a directory that has none would be a lie, so
		// this must clear rather than be ignored.
		let mut iterm = Iterm::default();
		iterm.feed(&set_branch("main"));
		assert_eq!(iterm.branch(), Some("main"));
		iterm.feed(b"\x1b]1337;SetUserVar=gitBranch=\x07");
		assert_eq!(iterm.branch(), None);
	}

	#[test]
	fn only_the_one_honoured_variable_name_is_kept() {
		// The allow-list applied to NAMES (§55). cmote has one reader, so it keeps one name — which is
		// what means there is no remote-keyed map to bound.
		assert_eq!(
			branch(b"\x1b]1337;SetUserVar=kubeContext=cHJvZA==\x07"),
			None
		);
		assert_eq!(
			branch(b"\x1b]1337;SetUserVar=gitBranchy=bWFpbg==\x07"),
			None
		);
		assert_eq!(branch(b"\x1b]1337;SetUserVar=bWFpbg==\x07"), None);
	}

	#[test]
	fn a_value_that_will_not_decode_leaves_the_branch_alone() {
		// SECURITY-shaped, the same rule §54 applies to progress: rubbish from a remote must not be a
		// way to WIPE a real reading. Bad base64, and valid base64 that is not UTF-8.
		let mut iterm = Iterm::default();
		iterm.feed(&set_branch("main"));
		for bad in [
			&b"\x1b]1337;SetUserVar=gitBranch=!!!not base64!!!\x07"[..],
			// `/w==` decodes to the single byte 0xFF, which is not valid UTF-8.
			&b"\x1b]1337;SetUserVar=gitBranch=/w==\x07"[..],
		] {
			iterm.feed(bad);
			assert_eq!(iterm.branch(), Some("main"), "wiped by {bad:?}");
		}
	}

	#[test]
	fn control_characters_are_stripped_from_a_value_drawn_in_our_own_chrome() {
		// SECURITY (§55). The strip is chrome cmote owns and the user reads to know what they are
		// typing into, so a remote must not smuggle a newline or an escape into it — the same rule the
		// window title has had since §23.
		assert_eq!(
			branch(&set_branch("ma\r\nin\x1b[31m")).as_deref(),
			Some("main[31m")
		);
	}

	#[test]
	fn a_value_that_is_nothing_but_control_characters_is_no_branch_at_all() {
		// Sanitising to empty must read as "no branch", not as an empty pill — a smudge on the strip
		// with nothing in it would be worse than nothing.
		assert_eq!(branch(&set_branch("\x07\x1b\r\n")), None);
	}

	#[test]
	fn an_overlong_value_is_capped_where_it_is_stored() {
		// A remote must not be able to fill the tab strip. Capped on the way IN, so the bound holds
		// however the value is later drawn.
		let long = "b".repeat(MAX_VALUE_CHARS * 3);
		let kept = branch(&set_branch(&long)).expect("a long branch is still a branch");
		assert_eq!(kept.chars().count(), MAX_VALUE_CHARS);
	}

	#[test]
	fn a_multibyte_value_is_cut_at_a_character_boundary() {
		// The cap counts characters, not bytes, so a name of multi-byte glyphs cannot panic the slice
		// or leave a broken one.
		let long = "é".repeat(MAX_VALUE_CHARS * 2);
		let kept = branch(&set_branch(&long)).expect("a long branch is still a branch");
		assert_eq!(kept.chars().count(), MAX_VALUE_CHARS);
		assert!(kept.chars().all(|character| character == 'é'));
	}

	#[test]
	fn a_branch_and_a_mark_in_one_chunk_are_both_read() {
		// A shell hook emits its prompt integration in one write: the mark is positional and comes
		// back as a report, the branch is latest-value and is folded in.
		let mut iterm = Iterm::default();
		let mut chunk = b"\x1b]1337;SetMark\x07".to_vec();
		chunk.extend_from_slice(&set_branch("main"));
		assert_eq!(iterm.feed(&chunk), vec![(15, Report::Mark)]);
		assert_eq!(iterm.branch(), Some("main"));
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

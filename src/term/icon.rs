// term/icon.rs — the icon name a remote sets, which cmote draws on the tab chip (PLAN §69, §90).
//
//   OSC 1      ESC ] 1 ; name          BEL | ST   the standard spelling
//   OSC 9;3    ESC ] 9 ; 3 ; "name"    BEL | ST   ConEmu's, quoted in its own documentation (§90)
//   OSC 30     ESC ] 30 ; name         BEL | ST   contour's `SETTABNAME` (§98)
//
// THREE SPELLINGS, ONE WRITER. ConEmu calls its sequence "set the tab text" and contour's is named
// "Set Session/Tab Name"; cmote's chip is the only tab text it has, so all three mean the same thing
// and land in the same field through this one module — which is what §71 asks of a second spelling. A
// second spelling is only refused when it would be a second SOURCE for a field somebody else owns
// (iTerm's `CursorShape`, which would write the engine's cursor from outside), and this is not that.
// The first is honoured for the reason `9;9` is (§17): cmote is a Windows client, and ConEmu's
// vocabulary is what a Windows-side shell reaches for. The name is capped, sanitised and appended to
// the endpoint exactly as OSC 1's is — a remote gets no more of the chip through a new spelling than
// it had through the old, which is the whole reason a third one costs nothing to accept.
//
// OSC 30 IS THE THINNEST-SOURCED THING IN THIS MODULE, and that is recorded rather than smoothed
// over. It is one line of contour's sequence index — mnemonic, number, six words — and the detail
// page behind it does not resolve, so nobody here has read a definition of its payload (§89's tiers
// would call it a vendor restatement with the vendor's own page missing). What makes it safe to act
// on anyway is that being WRONG about it costs a tab chip: the worst case is a remote's unrelated
// `OSC 30` payload appearing, sanitised and capped at 24 characters, after the endpoint on its own
// tab — and never in place of it. The same misreading on a colour, a font or a clipboard would not
// have been worth the risk, and none of those is what this module writes.
//
// The sequence is one of the oldest in the vocabulary and the one whose MEANING has moved furthest.
// It was the label X11 put under a window that had been iconified — a thing Windows does not have,
// and winit exposes no API for. What terminals settled on instead is the split iTerm2 made popular
// and most emulators now follow: OSC 2 names the WINDOW, OSC 1 names the TAB, and OSC 0 (the older
// spelling) means both at once. cmote has a tab strip, so it has somewhere to put it.
//
// It is worth having because the chip's label is the ENDPOINT (`App::Tab::strip_label`): open two
// shells on the same host and today the two chips read identically. A program that names itself —
// `vim`, a long build, a `tmux` window — is exactly the thing that tells them apart.
//
// The engine never sees this. `vte` routes OSC 0 and OSC 2 alike to `set_title` and has no arm for
// OSC 1 at all, so the sequence reaches its catch-all and is logged away. The framing is
// `term::osc`'s job, shared with the cwd, prompt-mark and progress scanners; what is left here is
// deciding which payload is an icon name and what is safe to keep of it.
//
// TWO DELIBERATE REFUSALS live in this file, and both are the same decision seen from either side:
//
//  * The ICON HALF OF OSC 0 is not honoured. OSC 0 sets the icon name and the window title to the
//    same string, so cmote already HAS those bytes — they are the title. Feeding them to the chip
//    as well would put them on every tab of every session forever, because Debian's stock `PS1`
//    carries `\[\e]0;\u@\h:\w\a\]` and fires it on every prompt. The chip would then repeat the
//    endpoint that is already on it and the title that is already in the title bar. So the icon
//    half of OSC 0 is refused HERE, by this module matching `1;` and nothing else — not dropped
//    upstream by `vte`, which is what makes it cmote's answer rather than an accident.
//
//  * The name is APPENDED to the chip's label and never replaces it (`strip_label`). This is the
//    §55 rule the branch pill already carries: the label is what says which machine this is, so
//    remote-chosen text must never be readable as the start of it. A remote that could rename its
//    own chip could make a staging box wear a production name.
//
// A full reset (RIS) does not clear the name — and neither does it clear the window title, because
// `alacritty_terminal::Term::reset_state` assigns `self.title = None` without raising the event
// cmote's listener watches. The two behave alike, which is the point: a user seeing one survive a
// reset and the other not would be looking at a bug, whichever way we had guessed.

/// The longest OSC payload this scanner will buffer. An icon name is a word or two; anything
/// longer is either not for us (a base64 clipboard write, a long title) or malformed, and buffering
/// it would let a hostile stream grow our memory without bound (§12).
const MAX_PAYLOAD: usize = 512;

/// The longest icon name cmote keeps, in CHARACTERS — the cap that decides how much of the chip a
/// remote may spend.
///
/// The arithmetic behind the number: a chip elides its whole label past `ui::tabs::MAX_LABEL_CHARS`
/// (48), and the label is already carrying an endpoint (`user@host`, call it fifteen) and the three
/// characters of the ` — ` that joins them. Twenty-four leaves the usual case whole and elides only
/// when the endpoint is itself long — which is the right way round, because eliding is the honest
/// outcome there and not a name being cut for no reason.
const MAX_NAME_CHARS: usize = 24;

/// The icon name the remote last set (§69). Feed it every byte of shell output; it keeps the most
/// recent name and ignores everything else.
#[derive(Debug, Default)]
pub struct Icon {
	framer: super::osc::Framer<MAX_PAYLOAD>,
	name: Option<String>,
}

impl Icon {
	/// Scan a chunk of shell output for an icon name. Safe at any chunk boundary — the framer's
	/// state carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) {
		// Every finished OSC arrives here — titles, colour queries, prompt marks and all — and
		// `parse` keeps only OSC 1. A payload that is not one leaves the current name alone, the
		// same way the cwd scanner refuses to forget a directory because a title went past.
		let name = &mut self.name;
		self.framer.feed(bytes, |_offset, payload| {
			if let Some(found) = parse(payload) {
				// An empty name is how a program says "I am done owning this chip" — the shell
				// does it when a command exits. It clears rather than drawing an empty suffix.
				*name = (!found.is_empty()).then_some(found);
			}
		});
	}

	/// The name the remote last set, or `None` if none was set (or the last one cleared it).
	pub fn name(&self) -> Option<&str> {
		self.name.as_deref()
	}
}

/// Pull the icon name out of an OSC payload, or `None` if this OSC is not one — which leaves the
/// current name alone rather than forgetting it.
///
/// Matched by the FULL `1;` prefix, so none of the OSC codes that merely start with a `1` can be
/// mistaken for it: `10;`/`11;`/`12;` (the dynamic colours), `104;`/`110;`/`112;` (their resets) and
/// `1337;` (iTerm2's namespace) all fail the match, and the last of those matters most because it
/// is the one cmote actually reads elsewhere. The same rule covers the third spelling twice over:
/// `30;` is not `3;` (xterm's X11 window property) and not `30001;` (kitty's colour stack), neither
/// of which cmote reads and both of which a prefix test would have swallowed.
fn parse(payload: &[u8]) -> Option<String> {
	// ConEmu's spelling arrives quoted in its own documentation, exactly as `9;9`'s path does, and
	// `term/cwd.rs` trims the quotes off that one for the same reason — they frame the value, they
	// are not part of it. Only that one: quote-trimming a spelling whose source does not quote would
	// eat a quotation mark a program meant to keep.
	let (rest, quoted) = if let Some(rest) = payload.strip_prefix(b"1;") {
		(rest, false)
	} else if let Some(rest) = payload.strip_prefix(b"9;3;") {
		(rest, true)
	} else {
		(payload.strip_prefix(b"30;")?, false)
	};
	// Lossy rather than strict: a name is decoration, and a remote with one bad byte in a UTF-8
	// sequence should get a replacement character on its chip, not silently keep the old name.
	let text = String::from_utf8_lossy(rest);
	let text = if quoted {
		text.trim().trim_matches('"')
	} else {
		text.trim()
	};
	Some(super::osc::sanitize(text, MAX_NAME_CHARS))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and read the result.
	fn track(bytes: &[u8]) -> Option<String> {
		let mut icon = Icon::default();
		icon.feed(bytes);
		icon.name().map(str::to_owned)
	}

	#[test]
	fn osc1_sets_the_icon_name() {
		// The usual announcement, BEL-terminated: what `vim` sends when it opens a file.
		assert_eq!(track(b"\x1b]1;vim\x07").as_deref(), Some("vim"));
	}

	/// ConEmu's spelling of the same thing (§90), quoted the way its own documentation writes it.
	#[test]
	fn the_conemu_spelling_sets_the_same_name() {
		assert_eq!(track(b"\x1b]9;3;\"build\"\x07").as_deref(), Some("build"));
		assert_eq!(
			track(b"\x1b]9;3;build\x07").as_deref(),
			Some("build"),
			"the quotes frame the value and are optional"
		);
	}

	/// contour's spelling of the same thing (§98), unquoted — its index writes the payload as a bare
	/// name, and a quote is only trimmed off the one spelling whose source puts it there.
	#[test]
	fn the_contour_spelling_sets_the_same_name() {
		assert_eq!(track(b"\x1b]30;build\x07").as_deref(), Some("build"));
		assert_eq!(
			track(b"\x1b]30;\x07"),
			None,
			"and clears on the empty name like the other two"
		);
		assert_eq!(
			track(b"\x1b]30;\"build\"\x07").as_deref(),
			Some("\"build\""),
			"a quote here is part of the name, not a frame around it"
		);
	}

	/// And it clears the chip the same way, so a shell can hand the name back on either spelling.
	#[test]
	fn the_conemu_spelling_clears_with_an_empty_name() {
		let mut icon = Icon::default();
		icon.feed(b"\x1b]9;3;\"build\"\x07");
		icon.feed(b"\x1b]9;3;\"\"\x07");
		assert_eq!(icon.name(), None);
	}

	/// The other four members of ConEmu's OSC 9 multiplex are not this one (§89, §90). `9;4` and
	/// `9;9` belong to other modules and `9;1` / `9;2` are refused outright — reading any of them
	/// as a tab name would put a millisecond count or a dialog's text on the chip.
	#[test]
	fn the_other_osc_nine_sub_codes_are_not_a_tab_name() {
		assert_eq!(track(b"\x1b]9;1;500\x07"), None, "sleep");
		assert_eq!(track(b"\x1b]9;2;\"a dialog\"\x07"), None, "message box");
		assert_eq!(track(b"\x1b]9;4;1;30\x07"), None, "progress");
		assert_eq!(track(b"\x1b]9;9;\"C:\\\\Users\\\\CLEm\"\x07"), None, "cwd");
		assert_eq!(track(b"\x1b]9;a notification\x07"), None, "the bare form");
	}

	#[test]
	fn the_st_terminator_is_accepted_too() {
		// ESC \ rather than BEL. Both frame an OSC string and a program may use either.
		assert_eq!(track(b"\x1b]1;build\x1b\\").as_deref(), Some("build"));
	}

	#[test]
	fn an_empty_name_clears_it() {
		// How a program hands the chip back when its command ends. The chip then reads as the
		// endpoint alone again, rather than keeping the name of something no longer running.
		let mut icon = Icon::default();
		icon.feed(b"\x1b]1;vim\x07");
		assert_eq!(icon.name(), Some("vim"));
		icon.feed(b"\x1b]1;\x07");
		assert_eq!(icon.name(), None);
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks — including a split between the ESC and the `]`.
		let mut icon = Icon::default();
		icon.feed(b"text \x1b");
		icon.feed(b"]1;ma");
		icon.feed(b"ke\x07more text");
		assert_eq!(icon.name(), Some("make"));
	}

	#[test]
	fn control_characters_are_stripped_from_the_name() {
		// The chip is chrome cmote owns: a remote must not be able to smuggle a newline, a tab or
		// a carriage return into it (§55, and the same rule the branch pill carries).
		assert_eq!(track(b"\x1b]1;a\nb\x07").as_deref(), Some("ab"));
		assert_eq!(track(b"\x1b]1;a\tb\rc\x07").as_deref(), Some("abc"));
	}

	#[test]
	fn an_escape_inside_the_name_kills_the_whole_sequence() {
		// The one control character that never reaches the strip above, because the framer settles
		// it first: an ESC inside an OSC payload is either the start of the ST terminator or a
		// malformed sequence, and `term::osc` abandons the payload on the second reading rather
		// than guessing. So an attempt to smuggle `ESC [ 31 m` into the chip yields no name at all
		// — a stricter answer than the strip would have given, and worth pinning as the reason
		// this case is not in the test above.
		assert_eq!(track(b"\x1b]1;a\x1b[31mb\x07"), None);
		// And the scanner is still working afterwards: the malformed one costs its own sequence
		// and nothing more.
		let mut icon = Icon::default();
		icon.feed(b"\x1b]1;a\x1b[31mb\x07\x1b]1;vim\x07");
		assert_eq!(icon.name(), Some("vim"));
	}

	#[test]
	fn a_name_that_was_only_control_characters_clears_it() {
		// Sanitising leaves nothing, and nothing is not a name — the chip shows no empty suffix.
		assert_eq!(track(b"\x1b]1;\x08\x08\x07"), None);
	}

	#[test]
	fn a_long_name_is_capped_where_it_is_stored() {
		// Bounded before it is kept, not merely before it is drawn: a remote cannot park a long
		// string in memory by pointing it at a widget that would have elided it anyway.
		let long = format!("\x1b]1;{}\x07", "n".repeat(200));
		let name = track(long.as_bytes()).expect("a long name is still a name");
		assert_eq!(name.chars().count(), MAX_NAME_CHARS);
	}

	#[test]
	fn the_icon_half_of_osc_0_is_not_honoured() {
		// The deliberate refusal (§69). OSC 0 sets icon name AND window title to the same string,
		// and cmote keeps that string as the title already. Honouring the icon half would put the
		// stock Debian prompt — `\[\e]0;\u@\h:\w\a\]`, sent on EVERY prompt — on every chip.
		assert_eq!(track(b"\x1b]0;user@host: ~\x07"), None);
	}

	#[test]
	fn no_other_osc_code_is_mistaken_for_an_icon_name() {
		// The prefix is matched whole, so the codes that merely begin with a `1` are not it. The
		// last is the one that matters most: OSC 1337 is a namespace cmote really does read.
		assert_eq!(track(b"\x1b]2;window title\x07"), None);
		assert_eq!(track(b"\x1b]10;?\x07"), None);
		assert_eq!(track(b"\x1b]11;rgb:00/00/00\x07"), None);
		assert_eq!(track(b"\x1b]104;3\x07"), None);
		assert_eq!(track(b"\x1b]112\x07"), None);
		assert_eq!(track(b"\x1b]1337;SetMark\x07"), None);
		// The two the third spelling sits between (§98). `OSC 3` sets an X11 window property and
		// `OSC 30001` is kitty's colour stack; a prefix test on `3` or `30` would have taken both.
		assert_eq!(track(b"\x1b]3;WM_NAME=x\x07"), None);
		assert_eq!(track(b"\x1b]30001;\x07"), None);
	}

	#[test]
	fn another_osc_does_not_forget_the_name_we_have() {
		// A shell sets the title and announces its cwd on every prompt. Neither is an icon name,
		// and neither may clear one — the same rule the cwd scanner keeps for the same reason.
		let mut icon = Icon::default();
		icon.feed(b"\x1b]1;vim\x07");
		icon.feed(b"\x1b]0;user@host: ~\x07\x1b]7;file://host/home\x07");
		assert_eq!(icon.name(), Some("vim"));
	}

	#[test]
	fn an_overlong_payload_is_dropped_not_buffered() {
		// A hostile or broken stream must not grow our memory: past the cap the payload is
		// abandoned and the scanner keeps hunting for the next sequence, so the flood costs us
		// the flooded sequence and nothing else.
		let mut icon = Icon::default();
		icon.feed(b"\x1b]1;");
		icon.feed(&vec![b'x'; MAX_PAYLOAD + 10]);
		icon.feed(b"\x07");
		assert_eq!(icon.name(), None);

		icon.feed(b"\x1b]1;vim\x07");
		assert_eq!(icon.name(), Some("vim"));
	}
}

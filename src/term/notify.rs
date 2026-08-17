// term/notify.rs — name the OSC payloads cmote refuses outright, so the refusal is stated
// (PLAN §79, §90).
//
// A remote asking the terminal to raise a desktop notification is ONE feature in three spellings,
// each an OSC payload and each from a different terminal's vocabulary:
//
//   ESC ] 9   ; <text>                      BEL | ST   ConEmu / Windows Terminal
//   ESC ] 777 ; notify ; <title> ; <body>   BEL | ST   urxvt
//   ESC ] 99  ; <metadata> ; <body>         BEL | ST   kitty, the "rich" one
//
// Neither of the last two is a citation. §89 went to look: urxvt's manual page documents no OSC 777
// at all, and ConEmu's page documents `9;1` through `9;4` and `9;9` but no bare `9;<text>` — that
// spelling may be Windows Terminal's alone. The sequences are real and widely emitted; the names
// beside them are folklore, and are left here as the names people use rather than as sources.
//
// OSC 9 IS MULTIPLEXED FIVE WAYS, which §89 found by reading ConEmu's own page and §90 acts on. Only
// two of the five were modelled here before, and the other three were being declined as
// notifications — the right outcome reached through the wrong description:
//
//   ESC ] 9 ; 1 ; <ms>     sleep the terminal          REFUSED here, by name (§90)
//   ESC ] 9 ; 2 ; "<txt>"  raise a GUI message box     REFUSED here, by name (§90)
//   ESC ] 9 ; 3 ; "<txt>"  set the tab's text          HONOURED — `term/icon.rs` (§90)
//   ESC ] 9 ; 4 ; st ; pr  progress                    HONOURED — `term/progress.rs` (§54)
//   ESC ] 9 ; 9 ; "<cwd>"  working directory           HONOURED — `term/cwd.rs` (§17)
//
// The two refused there are not notifications and are not refused for the notification's reason.
// `9;1` lets a remote STOP the terminal in front of the person using it, for as long as it likes —
// every other refusal in this file is about something escaping the tab, and this one is about the
// user's own time. `9;2` is the notification argument only more so: a notification leaves the
// window, and a modal dialog leaves the window and takes the focus with it.
//
// All three NOTIFICATION spellings are refused, and the reason is one line: a notification LEAVES
// the window. It lands on
// the user's desktop, outlives the tab that asked for it, and on Windows sits in the Action Center
// after the session is gone. That hands a remote a channel to the machine itself, and a compromised
// or merely chatty host would spam it. cmote's rule throughout is that a remote may change what its
// own tab looks like and nothing more — which is exactly why OSC 9;4 progress (§54) is implemented
// while OSC 9;<text> on the same OSC number is not: progress cannot leave the chip it belongs to.
//
// WHY THIS MODULE EXISTS AT ALL, given that nothing in cmote would raise a notification even if one
// were parsed. Because that is the difference §63 found in the OSC 52 row: the clipboard pair was
// refused only by a catch-all arm that happened to drop the event, and a refusal nobody states is
// one nobody can test and one an engine bump can quietly undo. Before this module, `kitty 99` and
// `OSC 777` were refused by NOBODY — no `vte` arm, no cmote scanner — and the bare `OSC 9;<text>`
// only by the accident that `term::progress` matches `9;4;` and `term::cwd` matches `9;9;`, so
// neither recognised it. The policy is the same as it was; what changes is that all three spellings
// are now named in one place, refused by name, and pinned by tests that fail if a later hand starts
// reading any of them.
//
// This is deliberately a plain function and NOT a scanner of its own. Every scanner in `term` keeps
// something the app reads or answers a query the engine dropped; one that kept nothing and answered
// nobody would be a no-op wearing a scanner's clothes, and it would have to be fed a copy of every
// chunk to do it. Instead the policy is called from `term::progress`, which is the
// module that already frames every OSC payload cmote sees and already owned the bare-OSC-9 refusal
// (§54) — so the check runs on the real stream rather than in a corner nothing feeds.
//
// The notification half changes no behaviour, and the rows in TERMINAL_COMPATIBILITY_PLAN say so.
// What it changes is that the refusal is performed by cmote's own code instead of being agreed with
// in principle — the whole distinction between that table's 🛑 and 🤷 columns. `9;1` and `9;2` are
// the same argument at one remove: nothing here would sleep or raise a dialog either, and until §90
// nothing here could say which payload it was declining or why.

/// What a refused payload turned out to be (§79, §90). Carried out of `refused` rather than a bare
/// `bool` so a test can say WHICH one it recognised, and so a later reader can see at a glance that
/// the three notification spellings are one decision and not three coincidences.
///
/// The last two are not notifications and are not spellings of one — they are the other two members
/// of ConEmu's `OSC 9` multiplex that cmote refuses, kept here because this is the module that
/// already owns "which `OSC 9` sub-code is this?" and splitting that question across two files is
/// how the two halves come to disagree (§90).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
	/// `OSC 9 ; <text>` — ConEmu's, adopted by Windows Terminal. The likeliest of the three to
	/// arrive here, because it is what a Windows-side script reaches for.
	ConEmu,
	/// `OSC 777 ; notify ; <title> ; <body>` — urxvt's, and the one `notify-send` wrappers emit.
	Urxvt,
	/// `OSC 99 ; <metadata> ; <body>` — kitty's, whose first field is a `:`-separated `key=value`
	/// list: `p` the payload type, `i` an identifier, `d` a done flag, `e` base64, `f` the
	/// application name, `u` the urgency and `n` an icon (§89). Rich, and refused for the same one
	/// reason as the two plain ones.
	Kitty,
	/// `OSC 9 ; 1 ; <ms>` — **sleep the terminal** for that many milliseconds (§90).
	///
	/// Not a notification and worse than one. Every other refusal in this module is about something
	/// escaping the tab; this one is a remote stopping the terminal in front of the person using it,
	/// for as long as it likes. A hostile or merely broken host could hold the UI still with a
	/// handful of bytes, and there is no reason a remote should be able to spend the user's time.
	Sleep,
	/// `OSC 9 ; 2 ; "<txt>"` — raise a **GUI message box** (§90).
	///
	/// The notification argument, only more so: a notification leaves the window, and a modal dialog
	/// leaves the window AND takes the focus with it. Text the remote chose, in a window wearing
	/// cmote's identity, interrupting whatever the user was doing.
	MessageBox,
	/// `OSC 50 ; <font>` — xterm's font operations (§88, §91) — and `OSC 60 ; <faces>`, contour's
	/// `SETFONTALL`, which sets every face, style and size at once (§98).
	///
	/// The font is chrome the **user** chose, which is the argument the fixed colour scheme stands on
	/// (§6): a remote may change what its own tab shows, not what the application looks like. xterm
	/// gates these behind an `allowFontOps` resource for the same reason, and its own default is off.
	///
	/// The one `OSC 50` payload cmote HONOURS is `CursorShape=`, another terminal's convention that
	/// `vte` parses on the same number (§88) — excluded here by name, so this refusal and that
	/// feature cannot come to disagree about which payload is which. `OSC 60` carries no such
	/// exception, being one vendor's sequence with one meaning.
	Font,
	/// `OSC 88 ; <op> [ ; <key>=<value> ]…` — the proposed Terminal Resume Protocol (§98).
	///
	/// A program declares how it should be **relaunched** if the terminal restarts: `cmd` (required)
	/// and `args`, base64-encoded, plus a `cwd`. `arm` stores the spec, `clear` withdraws it, `query`
	/// asks whether the terminal supports it.
	///
	/// This is the one refusal in this module that is not about something leaving the tab, or the
	/// user's time, or their chrome. It is a remote choosing **what the local machine executes** — at
	/// a moment nobody is watching, after a crash, from a command line that arrived over the wire.
	/// cmote is an SSH client: the remote end is the thing being defended against, and no payload it
	/// sends may ever become a local process. The `query` form is refused with the rest, and silence
	/// is the right answer to it — "supported" is exactly the advertisement (§71) that would make a
	/// program send the `arm` this refuses.
	Resume,
}

/// Read one OSC payload as something cmote refuses outright, or `None` when it is not one.
///
/// The awkward case is OSC 9, which is MULTIPLEXED five ways (§89): `9;3;…` is the tab name
/// `term::icon` reads, `9;4;…` the progress report `term::progress` reads and `9;9;…` the Windows
/// working directory `term::cwd` reads, while `9;1;…` and `9;2;…` are refused here by name and
/// anything else after `9;` is the notification. The three sub-codes cmote HONOURS are excluded with
/// their trailing `;` — the same prefixes those three modules strip — so this function and they can
/// never disagree about which payload belongs to whom.
///
/// The match is on the whole numeric field, never on a prefix of it: `99;` is kitty's but `990;`
/// and `999;` are somebody else's, and a prefix test would have swallowed both.
pub fn refused(payload: &[u8]) -> Option<Refused> {
	if let Some(rest) = payload.strip_prefix(b"9;") {
		// The THREE OSC 9 sub-codes cmote reads. None is a notification, and getting this wrong in
		// the permissive direction would break three shipped features rather than leak one.
		if rest.starts_with(b"4;") || rest.starts_with(b"9;") || rest.starts_with(b"3;") {
			return None;
		}
		// The two ConEmu defines that cmote refuses on their own terms. Named before the fall-through
		// below, because until §90 they arrived here and were declined as NOTIFICATIONS — the right
		// outcome reached through the wrong description, which is a refusal nobody can audit.
		if rest.starts_with(b"1;") || rest == b"1" {
			return Some(Refused::Sleep);
		}
		if rest.starts_with(b"2;") || rest == b"2" {
			return Some(Refused::MessageBox);
		}
		return Some(Refused::ConEmu);
	}
	// urxvt's OSC 777 is itself a dispatcher — `777;<module>;…` — and only the `notify` module is
	// this feature. A different module is not refused here because it is not this decision; it is
	// unimplemented, which is a different row and a different mark.
	if payload.starts_with(b"777;notify;") || payload == b"777;notify" {
		return Some(Refused::Urxvt);
	}
	// kitty's carries metadata in its first field, so everything after the number varies. The
	// number is the whole of what identifies it.
	if payload.starts_with(b"99;") || payload == b"99" {
		return Some(Refused::Kitty);
	}
	// xterm's OSC 50 is the FONT (§88). The cursor-shape payload cmote honours on the same number is
	// another terminal's, and is excluded first — the same shape of exclusion the OSC 9 sub-codes get
	// above, and for the same reason: one number, two meanings, and the two must not disagree.
	if let Some(rest) = payload.strip_prefix(b"50;")
		&& !rest.starts_with(b"CursorShape=")
	{
		return Some(Refused::Font);
	}
	// contour's `OSC 60` is the same decision at a larger size — every face at once (§98). The bare
	// form is its query, and a query is refused with the set it would lead to.
	if payload.starts_with(b"60;") || payload == b"60" {
		return Some(Refused::Font);
	}
	// The Terminal Resume Protocol (§98). `88;` and not `8;` — OSC 8 is the hyperlink cmote ships —
	// and not `888;`, which is contour's state dump and a different question with a different mark.
	if payload.starts_with(b"88;") || payload == b"88" {
		return Some(Refused::Resume);
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_three_spellings_are_one_refusal() {
		// The point of the module: one decision, recognised in whichever dialect it arrives in.
		assert_eq!(refused(b"9;Build finished"), Some(Refused::ConEmu));
		assert_eq!(
			refused(b"777;notify;Build;finished in 4s"),
			Some(Refused::Urxvt)
		);
		assert_eq!(
			refused(b"99;i=1:d=0:p=title;Build finished"),
			Some(Refused::Kitty)
		);
	}

	#[test]
	fn the_two_osc_nine_sub_codes_cmote_reads_are_not_notifications() {
		// The load-bearing exclusion. OSC 9 carries three unrelated features, and refusing too
		// eagerly here would silently break progress (§54) and the working directory (§17) —
		// two shipped features — in exchange for tightening a refusal that already holds.
		assert_eq!(refused(b"9;4;1;30"), None);
		assert_eq!(refused(b"9;4;0"), None);
		assert_eq!(refused(b"9;9;C:\\Users\\CLEm"), None);
	}

	/// The two ConEmu sub-codes that are refused on their own terms rather than as notifications
	/// (§90). Named, so a later hand cannot start honouring a remote's sleep or its dialog by
	/// widening some other arm.
	#[test]
	fn the_sleep_and_the_message_box_are_refused_as_themselves() {
		assert_eq!(refused(b"9;1;500"), Some(Refused::Sleep));
		assert_eq!(refused(b"9;2;\"are you sure?\""), Some(Refused::MessageBox));
		// Malformed, missing their argument — still theirs, and still refused.
		assert_eq!(refused(b"9;1"), Some(Refused::Sleep));
		assert_eq!(refused(b"9;2"), Some(Refused::MessageBox));
	}

	/// And they are matched on the whole sub-code: `9;10;…` is not `9;1`, and reading it as one
	/// would let a payload cmote does not understand wear a name it has a policy for.
	#[test]
	fn a_longer_sub_code_is_not_mistaken_for_a_shorter_one() {
		assert_eq!(refused(b"9;10;500"), Some(Refused::ConEmu));
		assert_eq!(refused(b"9;21;x"), Some(Refused::ConEmu));
		assert_eq!(
			refused(b"9;30;x"),
			Some(Refused::ConEmu),
			"not the tab text"
		);
	}

	/// The tab text is ConEmu's third sub-code and cmote HONOURS it (§90, `term/icon.rs`), so this
	/// function must not claim it — the exclusion that keeps the two modules from disagreeing.
	#[test]
	fn the_tab_text_is_not_a_notification() {
		assert_eq!(refused(b"9;3;\"build\""), None);
	}

	/// xterm's `OSC 50` is the font, and the font is the user's (§88, §91). The cursor shape shares
	/// the number and is honoured, so the exclusion is asserted from both sides — a refusal that
	/// swallowed `CursorShape=` would break a shipped feature to tighten a policy that already holds.
	#[test]
	fn the_font_is_refused_and_the_cursor_shape_on_the_same_number_is_not() {
		assert_eq!(refused(b"50;9x15bold"), Some(Refused::Font));
		assert_eq!(refused(b"50;#+1"), Some(Refused::Font), "a font-menu index");
		assert_eq!(
			refused(b"50;"),
			Some(Refused::Font),
			"an empty payload is still xterm's font namespace, and refusing it is the conservative \
			 reading — the permissive one would let a payload cmote does not understand through"
		);
		assert_eq!(refused(b"50;CursorShape=1"), None);
		assert_eq!(refused(b"500;something"), None, "not a prefix match");
	}

	/// contour's `OSC 60` is the whole font stack in one payload — every face, style and size (§98).
	/// One argument, two numbers, so it carries the same variant rather than a second one.
	#[test]
	fn the_other_font_sequence_is_the_same_refusal() {
		assert_eq!(refused(b"60;regular=IBM Plex Mono"), Some(Refused::Font));
		assert_eq!(refused(b"60"), Some(Refused::Font), "its bare query form");
		assert_eq!(refused(b"600;x"), None, "not a prefix match");
		assert_eq!(refused(b"6;x"), None, "nor read short");
	}

	/// The Terminal Resume Protocol (§98) — a remote handing the terminal a command line to run
	/// locally if it ever restarts. Refused by name, including the `query` that would advertise it.
	#[test]
	fn a_relaunch_specification_is_refused_in_every_operation() {
		assert_eq!(
			refused(b"88;arm;cmd=c3No;args=aG9zdA==").expect("an arm is refused"),
			Refused::Resume
		);
		assert_eq!(refused(b"88;clear"), Some(Refused::Resume));
		assert_eq!(
			refused(b"88;query"),
			Some(Refused::Resume),
			"answering 'supported' is what would bring the arm"
		);
		assert_eq!(refused(b"88"), Some(Refused::Resume));
		// The neighbours on either side, neither of which is this. OSC 8 is the hyperlink cmote
		// ships; OSC 888 is contour's state dump, unimplemented rather than refused.
		assert_eq!(refused(b"8;;https://example.invalid"), None);
		assert_eq!(refused(b"888;"), None);
	}

	#[test]
	fn a_longer_osc_number_is_not_mistaken_for_a_shorter_one() {
		// `99` must not be read out of `990` or `999`, and `9` must not be read out of `99`.
		// The separator is part of the match for exactly this reason.
		assert_eq!(refused(b"990;something"), None);
		assert_eq!(refused(b"999;something"), None);
		assert_eq!(refused(b"7770;notify;a"), None);
	}

	#[test]
	fn the_other_osc_payloads_cmote_sees_every_day_are_left_alone() {
		// Every scanner shares one framer, so this function is offered every OSC payload in the
		// stream. It must recognise nothing it was not asked to.
		assert_eq!(refused(b"0;a window title"), None);
		assert_eq!(refused(b"7;file:///C:/Users/CLEm"), None);
		assert_eq!(refused(b"133;A"), None);
		assert_eq!(refused(b"22;text"), None);
		assert_eq!(refused(b"1337;SetMark"), None);
		// The third tab-name spelling (§98). It sits two digits away from the font refusal above and
		// must not be caught by it, the same way `9;3;` is excluded from the notification arm.
		assert_eq!(refused(b"30;build"), None);
		assert_eq!(refused(b""), None);
	}

	#[test]
	fn only_the_urxvt_notify_module_is_this_decision() {
		// OSC 777 is a dispatcher. Refusing the whole number would be claiming a decision about
		// modules nobody here has looked at.
		assert_eq!(refused(b"777;notify;title;body"), Some(Refused::Urxvt));
		assert_eq!(refused(b"777;precmd"), None);
		assert_eq!(refused(b"777;"), None);
	}

	#[test]
	fn a_notification_with_no_text_is_still_a_notification() {
		// An empty body is a valid — and pointless — request in all three dialects. It is refused
		// on what it ASKS for, not on whether it would have shown anything.
		assert_eq!(refused(b"9;"), Some(Refused::ConEmu));
		assert_eq!(refused(b"99;"), Some(Refused::Kitty));
		assert_eq!(refused(b"99"), Some(Refused::Kitty));
		assert_eq!(refused(b"777;notify"), Some(Refused::Urxvt));
	}
}

// term/notify.rs — name the desktop-notification spellings, so the refusal is stated (PLAN §79).
//
// A remote asking the terminal to raise a desktop notification is ONE feature in three spellings,
// each an OSC payload and each from a different terminal's vocabulary:
//
//   ESC ] 9   ; <text>                      BEL | ST   ConEmu / Windows Terminal
//   ESC ] 777 ; notify ; <title> ; <body>   BEL | ST   urxvt
//   ESC ] 99  ; <metadata> ; <body>         BEL | ST   kitty, the "rich" one
//
// All three are REFUSED, and the reason is one line: a notification LEAVES the window. It lands on
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
// It changes no behaviour today, and the row in TERMINAL_COMPATIBILITY_PLAN says so. What it
// changes is that the refusal is performed by cmote's own code instead of being agreed with in
// principle — which is the whole distinction between that table's 🛑 and 🤷 columns.

/// Which spelling of the refused notification feature a payload turned out to be (§79). Carried
/// out of `refused` rather than a bare `bool` so a test can say WHICH one it recognised, and so a
/// later reader can see at a glance that the three are one decision and not three coincidences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spelling {
	/// `OSC 9 ; <text>` — ConEmu's, adopted by Windows Terminal. The likeliest of the three to
	/// arrive here, because it is what a Windows-side script reaches for.
	ConEmu,
	/// `OSC 777 ; notify ; <title> ; <body>` — urxvt's, and the one `notify-send` wrappers emit.
	Urxvt,
	/// `OSC 99 ; <metadata> ; <body>` — kitty's, with per-notification identity, urgency and
	/// icon metadata in the first field. Rich, and refused for exactly the same one reason.
	Kitty,
}

/// Read one OSC payload as a request for a desktop notification, or `None` when it is not one.
///
/// The awkward case is OSC 9, which is MULTIPLEXED: `9;4;…` is the progress report `term::progress`
/// reads and `9;9;…` is the Windows working directory `term::cwd` reads, while anything else after
/// `9;` is the notification. So the two sub-codes cmote actually honours are excluded by name here,
/// and they are excluded with their trailing `;` — the same prefixes those two modules strip — so
/// this function and they can never disagree about which payload belongs to whom.
///
/// The match is on the whole numeric field, never on a prefix of it: `99;` is kitty's but `990;`
/// and `999;` are somebody else's, and a prefix test would have swallowed both.
pub fn refused(payload: &[u8]) -> Option<Spelling> {
	if let Some(rest) = payload.strip_prefix(b"9;") {
		// The two OSC 9 sub-codes cmote reads. Neither is a notification, and getting this wrong
		// in the permissive direction would break two shipped features rather than leak one.
		if rest.starts_with(b"4;") || rest.starts_with(b"9;") {
			return None;
		}
		return Some(Spelling::ConEmu);
	}
	// urxvt's OSC 777 is itself a dispatcher — `777;<module>;…` — and only the `notify` module is
	// this feature. A different module is not refused here because it is not this decision; it is
	// unimplemented, which is a different row and a different mark.
	if payload.starts_with(b"777;notify;") || payload == b"777;notify" {
		return Some(Spelling::Urxvt);
	}
	// kitty's carries metadata in its first field, so everything after the number varies. The
	// number is the whole of what identifies it.
	if payload.starts_with(b"99;") || payload == b"99" {
		return Some(Spelling::Kitty);
	}
	None
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_three_spellings_are_one_refusal() {
		// The point of the module: one decision, recognised in whichever dialect it arrives in.
		assert_eq!(refused(b"9;Build finished"), Some(Spelling::ConEmu));
		assert_eq!(
			refused(b"777;notify;Build;finished in 4s"),
			Some(Spelling::Urxvt)
		);
		assert_eq!(
			refused(b"99;i=1:d=0:p=title;Build finished"),
			Some(Spelling::Kitty)
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
		assert_eq!(refused(b""), None);
	}

	#[test]
	fn only_the_urxvt_notify_module_is_this_decision() {
		// OSC 777 is a dispatcher. Refusing the whole number would be claiming a decision about
		// modules nobody here has looked at.
		assert_eq!(refused(b"777;notify;title;body"), Some(Spelling::Urxvt));
		assert_eq!(refused(b"777;precmd"), None);
		assert_eq!(refused(b"777;"), None);
	}

	#[test]
	fn a_notification_with_no_text_is_still_a_notification() {
		// An empty body is a valid — and pointless — request in all three dialects. It is refused
		// on what it ASKS for, not on whether it would have shown anything.
		assert_eq!(refused(b"9;"), Some(Spelling::ConEmu));
		assert_eq!(refused(b"99;"), Some(Spelling::Kitty));
		assert_eq!(refused(b"99"), Some(Spelling::Kitty));
		assert_eq!(refused(b"777;notify"), Some(Spelling::Urxvt));
	}
}

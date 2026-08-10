// glob.rs — the one text rule behind the home screen's filter box (PLAN §49).
//
// The filter box is not a search engine: it takes what the user has typed so far and answers,
// per saved target, "does this row stay on screen?". That answer lives here and nowhere else,
// so the view (which rows to draw) and `app` (whether the selected row is still one of them)
// can never disagree about what the box means.
//
// TWO rules in one function, because typing is the common case and globbing is the precise one:
//
//   * a pattern with NO wildcard in it is a FRAGMENT — it matches anywhere in the text, so
//     `prod` finds `web-production-01` without the user typing stars around it. This is what
//     makes the box usable one keystroke at a time: under whole-string matching the first
//     letter would match almost nothing and the list would blank out until the pattern was
//     finished, which is the opposite of a quick filter.
//   * a pattern that HAS a `*` or a `?` is a GLOB, and then it must match the WHOLE text, the
//     way a shell glob matches a whole filename. That is where anchoring comes from: `prod*` is
//     the targets whose text *begins* with prod, `*.db` the ones that end with `.db` — neither
//     of which the fragment rule can express, since a fragment is free to match in the middle.
//
// So the wildcard is the switch between the two, and the switch is visible in what was typed.
// The alternative — always matching a fragment, wildcards included — would make a trailing `*`
// mean nothing at all (a fragment already matches with anything after it), and then a user who
// typed the shell habit would get an answer that quietly ignored half of what they wrote.
//
// Matching is CASE-INSENSITIVE in both rules: a host list is typed in whatever case the naming
// scheme happened to use, and nobody filtering it is trying to make that distinction.

/// Does `text` survive the filter `pattern`? An empty pattern is "no filter" and keeps
/// everything — the box starts empty, and clearing it must put the whole list back.
///
/// See the module header for the two rules. The lowercasing allocates two `String`s per call,
/// which is fine for what this is used on: a screen of saved targets is a handful of rows, and
/// this runs once per row per keystroke. `ponytail:` if the list ever grew into thousands, the
/// pattern would be lowercased once by the caller instead of once per row.
pub fn matches(pattern: &str, text: &str) -> bool {
	if pattern.is_empty() {
		return true;
	}

	let pattern = pattern.to_lowercase();
	let text = text.to_lowercase();

	// The fragment rule: no wildcard typed, so this is a plain "contains", which is what a
	// half-typed pattern needs to be.
	if !pattern.contains(['*', '?']) {
		return text.contains(&pattern);
	}

	// The glob rule. Comparing by CHAR, not by byte: `?` means one character the user can see,
	// and a byte-wise `?` would match one third of an emoji and half of an accented letter.
	let pattern: Vec<char> = pattern.chars().collect();
	let text: Vec<char> = text.chars().collect();
	glob(&pattern, &text)
}

/// Match `text` against `pattern` in full, where `*` stands for any run of characters
/// (including none) and `?` for exactly one. Both are already lowercased by `matches`.
///
/// This is the classic two-pointer walk rather than a recursion or a regex build. A `*` is the
/// only place the match can go wrong in a way that is recoverable: everything else either
/// matches the character in front of it or fails outright. So the walk remembers the LAST `*`
/// it passed and how much text that star had swallowed at the time, and on a mismatch it feeds
/// the star one more character and carries on from there. That backtrack is what makes `a*b`
/// find its `b` at the end of `abab` instead of stopping at the first one — and it needs only
/// the last star, because an earlier star that has to give more ground will be reached again by
/// the same rule.
fn glob(pattern: &[char], text: &[char]) -> bool {
	let mut p = 0; // where we are in the pattern
	let mut t = 0; // where we are in the text
	// The last `*` seen, and how far into the text it has been stretched. `None` until one is
	// passed — before that a mismatch is simply a mismatch, with nothing to fall back to.
	let mut star: Option<usize> = None;
	let mut stretched = 0;

	while t < text.len() {
		match pattern.get(p).copied() {
			Some('*') => {
				// Note where the star is and let it match nothing for now — the mismatch arm
				// below is what grows it, one character at a time, only if it has to.
				star = Some(p);
				stretched = t;
				p += 1;
			}
			Some('?') => {
				p += 1;
				t += 1;
			}
			Some(character) if character == text[t] => {
				p += 1;
				t += 1;
			}
			// A mismatch, or a pattern that ran out with text left over. Either is fatal unless
			// a star can take the blame: give it the one more character it has not tried yet and
			// resume from just after it.
			_ => {
				let Some(star) = star else {
					return false;
				};
				p = star + 1;
				stretched += 1;
				t = stretched;
			}
		}
	}

	// The text is spent. What is left of the pattern must be able to match nothing at all, which
	// only a run of stars can — a leftover `?` or a literal still wants a character that is not
	// there.
	pattern[p..].iter().all(|character| *character == '*')
}

#[cfg(test)]
mod tests {
	use super::matches;

	/// The empty box is not a filter: every row stays. This is the state the screen opens in and
	/// the state clearing it returns to, so it has to keep the whole list rather than hide it.
	#[test]
	fn an_empty_pattern_keeps_everything() {
		assert!(matches("", "web-01"));
		assert!(matches("", ""));
	}

	/// Typed text with no wildcard is a fragment — it may match anywhere — so the list narrows
	/// from the first keystroke instead of waiting for a complete name.
	#[test]
	fn a_pattern_without_a_wildcard_matches_anywhere() {
		assert!(matches("prod", "web-production-01"));
		assert!(matches("web", "web-production-01"));
		assert!(matches("01", "web-production-01"));
		assert!(!matches("db", "web-production-01"));
	}

	/// A wildcard switches to whole-text matching, which is the only way to say "starts with"
	/// or "ends with" — a fragment is free to match in the middle, so it cannot anchor.
	#[test]
	fn a_wildcard_anchors_the_pattern_to_the_whole_text() {
		assert!(matches("prod*", "prod-01"));
		assert!(!matches("prod*", "web-prod-01"), "a glob is not a fragment");
		assert!(matches("*prod*", "web-prod-01"));
		assert!(matches("*.db", "roles.db"));
		assert!(!matches("*.db", "roles.db.old"));
	}

	/// `?` is exactly one character — no fewer, no more.
	#[test]
	fn a_question_mark_is_one_character() {
		assert!(matches("db?", "db1"));
		assert!(!matches("db?", "db"));
		assert!(!matches("db?", "db12"));
		assert!(matches("db??", "db12"));
	}

	/// The backtrack: a star must be willing to give up ground it already took, or `*b` would
	/// stop at the first `b` and call the rest a mismatch.
	#[test]
	fn a_star_gives_ground_until_the_rest_of_the_pattern_fits() {
		assert!(matches("*b", "abab"));
		assert!(matches("a*a*b", "aaab"));
		assert!(matches("*-01", "web-prod-01"));
		assert!(!matches("*-02", "web-prod-01"));
	}

	/// A trailing star may match nothing, and a pattern of nothing but stars matches anything —
	/// including the empty string, which is what `*` means everywhere else it is written.
	#[test]
	fn a_star_may_match_nothing_at_all() {
		assert!(matches("web*", "web"));
		assert!(matches("*", ""));
		assert!(matches("**", "anything"));
		assert!(matches("web*01", "web01"));
	}

	/// Case is not a distinction anyone filtering a host list is trying to make, under either
	/// rule.
	#[test]
	fn case_is_ignored_by_both_rules() {
		assert!(matches("PROD", "web-production-01"));
		assert!(matches("prod", "WEB-PRODUCTION-01"));
		assert!(matches("WEB-*", "web-prod-01"));
	}

	/// The endpoint half of a target is matched too, and it is full of punctuation — none of
	/// which is special here. Only `*` and `?` are.
	#[test]
	fn only_star_and_question_mark_are_special() {
		assert!(matches("root@10.", "root@10.0.0.7:22"));
		assert!(matches("*@10.0.0.7:22", "root@10.0.0.7:22"));
		// A dot is a literal dot, not "any character" — this is a glob, not a regex.
		assert!(!matches("root@1.", "root@10.0.0.7:22"));
	}
}

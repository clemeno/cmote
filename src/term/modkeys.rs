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
// stream for it here and hands the level to the key encoder (`keymap::encode`). The scanner is
// a small state machine rather than a match over a buffer, because output arrives in arbitrary
// chunks: the sequence can be split anywhere, even between the ESC and the `[`.
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
// ONLY RESOURCE 4 IS ANSWERED. XTMODKEYS carries seven resources (`modifyKeyboard`,
// `modifyCursorKeys`, `modifyFunctionKeys`, `modifyKeypadKeys`, `modifyOtherKeys`,
// `modifyModifierKeys`, `modifySpecialKeys`) and cmote holds state for exactly one of them. The
// reply format IS an XTMODKEYS control, so there is no spelling of "I do not have that resource"
// — an answer for resource 1 would be cmote asserting a level for a knob its key encoder does
// not have. Silence for the other six is the honest reading, and the same call §60 made three
// times over: an invented number is worse than a missing one. In practice the six are not asked;
// resource 4 is the one editors probe.

/// The escape byte that leads every CSI sequence.
const ESC: u8 = 0x1b;

/// The XTMODKEYS resource number for `modifyOtherKeys`. The same `CSI > Pp ; Pv m` shape also
/// carries resources 0/1/2 (modifyKeyboard / modifyCursorKeys / modifyFunctionKeys), which cmote
/// does not act on, so the resource is checked before the value is applied.
const MODIFY_OTHER_KEYS: u16 = 4;

/// The longest parameter run we will buffer inside one sequence. The real payload is tiny
/// (`4;2`); a longer one is malformed, and refusing to grow the buffer past this keeps a hostile
/// stream from ballooning our memory (§12).
const MAX_PARAMS: usize = 16;

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

/// Which of the two private markers opened the sequence being collected — the whole difference
/// between an order and a question (§61).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Marker {
	/// `CSI > …` — XTMODKEYS, which SETS a resource.
	#[default]
	Set,
	/// `CSI ? …` — XTQMODKEYS, which asks what one is.
	Query,
}

/// Where the scanner is in the byte stream. Only the two private-CSI shapes
/// (`ESC [ > … m` and `ESC [ ? … m`) are tracked; every other sequence resets straight back to
/// `Text`.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; a CSI starts if the next byte is `[`.
	Escape,
	/// Saw `ESC [`; the sequence is one we care about only if the next byte is a `>` or `?`.
	Bracket,
	/// Inside `ESC [ > …` or `ESC [ ? …`, collecting the parameter digits until the final byte.
	Params,
}

/// The `modifyOtherKeys` tracker (§9) and answerer (§61). Feed it every byte of shell output; it
/// keeps the level the remote last selected, reports the bytes owed to any question about it, and
/// ignores everything else in the stream.
#[derive(Debug, Default)]
pub struct ModKeys {
	state: Scan,
	/// Which marker opened the run being collected. Only meaningful inside `Scan::Params`.
	marker: Marker,
	params: Vec<u8>,
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
		let mut replies = Vec::new();
		for &byte in bytes {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Escape;
					}
				}
				Scan::Escape => {
					self.state = match byte {
						b'[' => Scan::Bracket,
						// ESC ESC: still waiting for the sequence's real first byte.
						ESC => Scan::Escape,
						_ => Scan::Text,
					};
				}
				Scan::Bracket => {
					self.state = match byte {
						// The two markers this module answers to. `?` opens every DECSET and
						// DECRST as well, which cost a few buffered digits and are then dropped
						// on their `h` / `l` final byte — the same toll `>` already paid.
						b'>' | b'?' => {
							self.marker = if byte == b'>' {
								Marker::Set
							} else {
								Marker::Query
							};
							self.params.clear();
							Scan::Params
						}
						// A fresh ESC restarts the match; any other byte is some other CSI
						// (an SGR colour, a cursor move) that we do not track.
						ESC => Scan::Escape,
						_ => Scan::Text,
					};
				}
				Scan::Params => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						// A run longer than any real payload is malformed; drop the sequence
						// rather than buffer it without bound.
						if self.params.len() > MAX_PARAMS {
							self.state = Scan::Text;
						}
					}
					// `m` is the final byte of both XTMODKEYS and XTQMODKEYS — the marker read
					// back at the start of the run is what says which of them this was.
					b'm' => {
						match self.marker {
							Marker::Set => self.apply(),
							Marker::Query => replies.extend_from_slice(&self.report()),
						}
						self.state = Scan::Text;
					}
					// A new ESC starts another sequence; any other final byte (`h`, `l`, `c`, …)
					// belongs to a private-CSI that is neither of ours, so we abandon this one.
					ESC => self.state = Scan::Escape,
					_ => self.state = Scan::Text,
				},
			}
		}
		replies
	}

	/// The level the remote last selected, or `Off` if it never asked (§9).
	pub fn level(&self) -> ModifyOtherKeys {
		self.level
	}

	/// Parse the collected `> Pp ; Pv` parameters and set the level if they name resource 4.
	/// `CSI > 4 m` (no value) and `CSI > 4 ; 0 m` both mean off; `1` and `2` select the levels;
	/// any larger value is clamped to level 2, matching xterm. A sequence for another resource
	/// (0/1/2) leaves the level untouched.
	fn apply(&mut self) {
		let mut parts = self.params.split(|&byte| byte == b';');
		if parts.next().and_then(parse_u16) != Some(MODIFY_OTHER_KEYS) {
			return;
		}
		self.level = match parts.next().and_then(parse_u16) {
			Some(1) => ModifyOtherKeys::Level1,
			Some(value) if value >= 2 => ModifyOtherKeys::Level2,
			// 0, an unparseable value, or no value at all: back to the default.
			_ => ModifyOtherKeys::Off,
		};
	}

	/// The bytes owed to a `CSI ? Pp m`, or nothing when the question was not about resource 4
	/// (§61).
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
	/// rather than a guess.
	fn report(&self) -> Vec<u8> {
		let mut parts = self.params.split(|&byte| byte == b';');
		if parts.next().and_then(parse_u16) != Some(MODIFY_OTHER_KEYS) || parts.next().is_some() {
			return Vec::new();
		}
		let value = match self.level {
			ModifyOtherKeys::Off => 0,
			ModifyOtherKeys::Level1 => 1,
			ModifyOtherKeys::Level2 => 2,
		};
		format!("\x1b[>{MODIFY_OTHER_KEYS};{value}m").into_bytes()
	}
}

/// A run of ASCII digits as a `u16`, or `None` when the run is empty or not all digits. Kept
/// small on purpose — a resource or level far past `u16` is meaningless here.
fn parse_u16(bytes: &[u8]) -> Option<u16> {
	if bytes.is_empty() {
		return None;
	}
	let mut value: u16 = 0;
	for &byte in bytes {
		let digit = byte.checked_sub(b'0').filter(|d| *d < 10)?;
		value = value.checked_mul(10)?.checked_add(u16::from(digit))?;
	}
	Some(value)
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

	#[test]
	fn a_question_about_another_resource_goes_unanswered() {
		// XTMODKEYS carries seven resources and cmote holds one. An answer for `modifyCursorKeys`
		// would be a level asserted for a knob the key encoder does not have — the invented number
		// §60 refused three times. An omitted parameter defaults to 0, `modifyKeyboard`, likewise
		// not ours.
		assert!(ask(b"\x1b[?0m").is_empty());
		assert!(ask(b"\x1b[?1m").is_empty());
		assert!(ask(b"\x1b[?2m").is_empty());
		assert!(ask(b"\x1b[?m").is_empty());
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
}

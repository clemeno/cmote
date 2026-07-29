// term/modkeys.rs — track the remote's xterm `modifyOtherKeys` mode (PLAN §9).
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

/// Where the scanner is in the byte stream. Only the one private-CSI shape
/// (`ESC [ > … m`) is tracked; every other sequence resets straight back to `Text`.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; a CSI starts if the next byte is `[`.
	Escape,
	/// Saw `ESC [`; the sequence is one we care about only if the next byte is the `>` marker.
	Bracket,
	/// Inside `ESC [ > …`, collecting the parameter digits until the `m` final byte.
	Params,
}

/// The `modifyOtherKeys` tracker (§9). Feed it every byte of shell output; it keeps the level
/// the remote last selected and ignores everything else in the stream.
#[derive(Debug, Default)]
pub struct ModKeys {
	state: Scan,
	params: Vec<u8>,
	level: ModifyOtherKeys,
}

impl ModKeys {
	/// Scan a chunk of shell output for a `modifyOtherKeys` change. Safe at any chunk
	/// boundary — the state machine carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) {
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
						b'>' => {
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
					// `m` is the final byte of XTMODKEYS: apply what we collected.
					b'm' => {
						self.apply();
						self.state = Scan::Text;
					}
					// A new ESC starts another sequence; any other final byte (`c`, `q`, …)
					// belongs to a private-CSI that is not XTMODKEYS, so we abandon this one.
					ESC => self.state = Scan::Escape,
					_ => self.state = Scan::Text,
				},
			}
		}
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
}

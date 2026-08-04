// term/osc133.rs — scan the shell-integration prompt marks out of the byte stream (PLAN §34).
//
// A shell that has "shell integration" turned on brackets each command it runs with OSC 133
// escape sequences, the FinalTerm/iTerm2 convention every modern terminal now reads:
//
//   OSC 133 ; A   ESC ] 133 ; A   BEL | ST   — a fresh prompt is about to be drawn
//   OSC 133 ; B   ESC ] 133 ; B   BEL | ST   — the prompt has ended; the user's input begins
//   OSC 133 ; C   ESC ] 133 ; C   BEL | ST   — input is done; the command's output begins
//   OSC 133 ; D [ ; exit ]        BEL | ST   — the command finished, with this exit code
//
// From those four marks a terminal knows where every prompt sits, whether a command is running,
// and how the last one ended — which is what powers "jump to the previous prompt" and a per-tab
// success/failure glyph (§34). Like the cwd (§17), modifyOtherKeys (§9) and the identity queries
// (§33), `alacritty_terminal` treats OSC 133 as an unknown OSC and ignores it, so cmote sniffs
// the same bytes out of the stream itself.
//
// This scanner does ONE job: turn the byte stream into a list of completed marks, each tagged
// with the byte offset just past its terminator. It deliberately does NOT decide where a prompt
// lands on the grid or track the command's state — that is `term::mod`'s job, because a mark's
// grid line is only known once the engine has been advanced up to it (`Terminal::process` splits
// the advance at each offset to read the cursor there). Keeping the scanner a pure bytes -> marks
// function is what lets it be unit-tested without an engine at all.
//
// The scanner is a small state machine rather than a search over a buffer, because output arrives
// in arbitrary chunks: a sequence can be split anywhere, including between the ESC and the `]` or
// in the middle of the payload. The state carries over between `feed` calls so any split is safe.

/// The escape and bell bytes that frame an OSC sequence.
const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;

/// The longest OSC 133 payload we will buffer. The marks themselves are tiny (`133;A`), but a
/// shell may append `key=value` fields (`133;A;aid=7`, `133;D;0;user`); this is generous for
/// those and still bounds the memory a hostile or broken stream can make us hold (§12). Past it
/// the payload is abandoned and the scanner resumes hunting for the next sequence.
const MAX_PAYLOAD: usize = 512;

/// One shell-integration mark, as read off the wire. The letter names the phase of the command
/// cycle; only `CommandEnd` carries data (the exit code, when the shell reported one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
	/// OSC 133;A — a new prompt is about to be drawn. Its grid line is the anchor a prompt jump
	/// lands on (§34).
	PromptStart,
	/// OSC 133;B — the prompt is written and the user's input starts. Carried for completeness;
	/// the state model treats the prompt as still active until output begins.
	PromptEnd,
	/// OSC 133;C — the user pressed Enter and the command's output begins: the command is running.
	OutputStart,
	/// OSC 133;D — the command finished. The exit code is `Some` when the shell reported one and
	/// `None` when it emitted a bare `133;D` (or a non-numeric field).
	CommandEnd(Option<i32>),
}

/// Where the scanner is in the byte stream. The same four-state shape as the cwd scanner (§17):
/// an OSC is `ESC ] payload (BEL | ESC \)`.
#[derive(Debug, Default, PartialEq, Eq)]
enum Scan {
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

/// Reads OSC 133 marks out of the shell's output. Feed it every byte; it returns the marks that
/// completed in that chunk, each with the offset just past its terminator so the caller can line
/// the mark up with the grid (§34). It holds only the in-flight sequence between calls.
#[derive(Debug, Default)]
pub struct Scanner {
	state: Scan,
	payload: Vec<u8>,
}

impl Scanner {
	/// Scan a chunk of shell output, returning every OSC 133 mark that finished in it. The
	/// `usize` in each pair is the byte offset in THIS `bytes` slice just past the mark's
	/// terminator — the point the engine has been advanced to when the mark is applied, so the
	/// cursor read there is exactly where the mark sits (`Terminal::process`). Safe at any chunk
	/// boundary: a sequence split across calls completes on the call that carries its terminator,
	/// and the offset is measured in that final chunk.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Mark)> {
		let mut marks = Vec::new();
		for (index, &byte) in bytes.iter().enumerate() {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Escape;
					}
				}
				Scan::Escape => {
					self.payload.clear();
					self.state = match byte {
						b']' => Scan::Payload,
						// ESC ESC: still waiting for the sequence's real first byte.
						ESC => Scan::Escape,
						_ => Scan::Text,
					};
				}
				Scan::Payload => match byte {
					// BEL ends the string; the offset is just past it.
					BEL => self.finish(index + 1, &mut marks),
					ESC => self.state = Scan::PayloadEscape,
					_ => {
						self.payload.push(byte);
						if self.payload.len() > MAX_PAYLOAD {
							self.abandon();
						}
					}
				},
				// ESC `\` is the string terminator (ST); an ESC followed by anything else is a
				// malformed sequence, so drop what we collected rather than guess.
				Scan::PayloadEscape => {
					if byte == b'\\' {
						self.finish(index + 1, &mut marks);
					} else {
						self.abandon();
					}
				}
			}
		}
		marks
	}

	/// A complete OSC payload: if it is an OSC 133 mark, push it with its end offset; otherwise
	/// drop it (a cwd announcement, a title, a clipboard write all arrive here too and are none
	/// of this scanner's business). Either way the scanner returns to hunting text.
	fn finish(&mut self, offset: usize, marks: &mut Vec<(usize, Mark)>) {
		if let Some(mark) = parse(&self.payload) {
			marks.push((offset, mark));
		}
		self.abandon();
	}

	/// Reset the scanner to hunt for the next sequence, discarding the current payload.
	fn abandon(&mut self) {
		self.state = Scan::Text;
		self.payload.clear();
	}
}

/// Pull an OSC 133 mark out of a payload, or `None` when the payload is some other OSC. The
/// payload is the bytes between `]` and the terminator, so an OSC 133 one reads `133;<letter>`
/// with optional trailing `;`-separated fields. Only the letter matters, plus the exit code that
/// follows `D`.
fn parse(payload: &[u8]) -> Option<Mark> {
	// Split on `;` so trailing key=value fields (`133;A;aid=7`) are ignored and `D`'s exit code
	// is just the next field. The first field identifies the sequence as OSC 133.
	let mut fields = payload.split(|&byte| byte == b';');
	if fields.next() != Some(b"133") {
		return None;
	}
	// The phase letter is the whole next field; a stray longer field (`133;AA`) is not a mark we
	// know, so it must not be mistaken for `A`.
	match fields.next() {
		Some(b"A") => Some(Mark::PromptStart),
		Some(b"B") => Some(Mark::PromptEnd),
		Some(b"C") => Some(Mark::OutputStart),
		Some(b"D") => {
			// `133;D` may carry an exit code as its next field; a bare `133;D` or a non-numeric
			// one reports `None` rather than a wrong number.
			let exit = fields
				.next()
				.and_then(|field| std::str::from_utf8(field).ok())
				.and_then(|text| text.parse::<i32>().ok());
			Some(Mark::CommandEnd(exit))
		}
		_ => None,
	}
}

/// Where the command cycle stands right now (§34), derived from the marks as they arrive. Drives
/// the per-tab status glyph: a running command shows a dot, and a finished one a ✓ or a ✗ with its
/// code (`last_exit`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CommandState {
	/// At rest — before the first prompt, or after a command finished and its result was read.
	#[default]
	Idle,
	/// A prompt is showing and the user may be editing the command line.
	Prompt,
	/// The user pressed Enter and the command's output is streaming.
	Running,
}

/// Which way a prompt jump moves through the marks (§34): to the prompt above the viewport, or
/// the one below it toward the live bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
	Previous,
	Next,
}

/// The most prompt marks we keep. A jump only ever needs the nearest one in a direction and the
/// ticks only the ones on screen, so a bounded ring of the most recent prompts is plenty; past
/// this the oldest is dropped, which caps the memory a very long session can hold.
const MAX_MARKS: usize = 4096;

/// The whole OSC 133 model for one terminal (§34): the byte scanner, the command-cycle state, the
/// last exit code, and the grid lines the prompts sit on. Positions are stored as ABSOLUTE line
/// indices — line 0 is the first line the session ever showed — so a mark keeps meaning as output
/// scrolls it up into history. A line's absolute index is `history_size + row` at the moment it is
/// recorded (`row` being its row on the active screen): the active screen's top line is always at
/// absolute `history_size`, since that is how many lines have scrolled off above it.
///
/// That identity is EXACT until the scrollback fills: once the engine hits its retention cap
/// (§23, `SCROLLBACK`) it evicts an old line for each new one, so `history_size` stops growing
/// while lines keep scrolling off — and absolute indices recorded on either side of that point no
/// longer share an origin. `ponytail:` past the cap the marks drift; a jump then lands near, not
/// on, an old prompt. The recent prompts a jump actually reaches are always fresh, so the common
/// case is exact; only history deeper than the cap is approximate.
#[derive(Debug, Default)]
pub struct Prompts {
	scanner: Scanner,
	marks: Vec<u64>,
	state: CommandState,
	last_exit: Option<i32>,
}

impl Prompts {
	/// Scan a chunk for OSC 133 marks, handing back each with its byte offset so the caller can
	/// advance the engine to it before applying it (`Terminal::process`). A thin pass-through to
	/// the scanner; `apply` is the half that needs the engine's cursor.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Mark)> {
		self.scanner.feed(bytes)
	}

	/// Fold one mark into the model, now that the engine has been advanced to it. `history_size`
	/// and `row` describe where the cursor sits at the mark — needed only by `PromptStart`, which
	/// anchors a prompt line; the others just move the command-cycle state.
	pub fn apply(&mut self, mark: Mark, history_size: usize, row: u16) {
		match mark {
			Mark::PromptStart => {
				self.state = CommandState::Prompt;
				self.record(history_size, row);
			}
			// The prompt is written and input begins; still the prompt phase as far as the glyph
			// is concerned, so nothing changes.
			Mark::PromptEnd => {}
			Mark::OutputStart => self.state = CommandState::Running,
			Mark::CommandEnd(exit) => {
				self.state = CommandState::Idle;
				self.last_exit = exit;
			}
		}
	}

	/// Record a prompt at absolute line `history_size + row`. A prompt the shell redraws in place
	/// (zsh redraws on each keystroke) fires `A` again at the same line, so a repeat of the last
	/// recorded line is ignored rather than stacked. The store is a bounded ring (`MAX_MARKS`).
	fn record(&mut self, history_size: usize, row: u16) {
		let absolute = history_size as u64 + row as u64;
		if self.marks.last() == Some(&absolute) {
			return;
		}
		self.marks.push(absolute);
		if self.marks.len() > MAX_MARKS {
			self.marks.remove(0);
		}
	}

	/// Forget every prompt (a full reset — `ESC c` — or the screen cleared them out from under us).
	pub fn clear(&mut self) {
		self.marks.clear();
	}

	/// Where the command cycle stands (§34), for the status glyph.
	pub fn state(&self) -> CommandState {
		self.state
	}

	/// The exit code of the last command that reported one, or `None` if none has yet.
	pub fn last_exit(&self) -> Option<i32> {
		self.last_exit
	}

	/// The viewport rows (0-based, top of the visible screen is 0) that hold a prompt mark, given
	/// where the scrollback is parked. Used to draw a tick beside each prompt on screen (§34). A
	/// viewport row for an absolute line is `absolute - history_size + display_offset`; a mark
	/// scrolled off the top or below the bottom falls outside `0..screen_lines` and is dropped.
	pub fn visible_rows(
		&self,
		history_size: usize,
		display_offset: usize,
		screen_lines: usize,
	) -> Vec<u16> {
		self.marks
			.iter()
			.filter_map(|&absolute| {
				let row = absolute as i64 - history_size as i64 + display_offset as i64;
				(0..screen_lines as i64)
					.contains(&row)
					.then_some(row as u16)
			})
			.collect()
	}

	/// The display offset that scrolls the nearest prompt above or below the current viewport top
	/// into view (§34), or `None` when there is no prompt in that direction. The viewport's top
	/// row shows absolute line `history_size - display_offset`; a jump lands the target prompt on
	/// that top row, clamped to the retained history so it never asks to scroll past either end.
	pub fn jump(
		&self,
		direction: Direction,
		history_size: usize,
		display_offset: usize,
	) -> Option<usize> {
		let top = history_size as i64 - display_offset as i64;
		let target = match direction {
			// Strictly above the top so a repeated press keeps climbing rather than sticking.
			Direction::Previous => self
				.marks
				.iter()
				.map(|&mark| mark as i64)
				.filter(|&mark| mark < top)
				.max()?,
			Direction::Next => self
				.marks
				.iter()
				.map(|&mark| mark as i64)
				.filter(|&mark| mark > top)
				.min()?,
		};
		let offset = (history_size as i64 - target).clamp(0, history_size as i64);
		Some(offset as usize)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and collect just the marks (dropping the offsets),
	/// for the tests that only care about what was recognised.
	fn marks(bytes: &[u8]) -> Vec<Mark> {
		Scanner::default()
			.feed(bytes)
			.into_iter()
			.map(|(_, mark)| mark)
			.collect()
	}

	#[test]
	fn each_phase_letter_maps_to_its_mark() {
		// The four marks that bracket one command, BEL-terminated.
		assert_eq!(marks(b"\x1b]133;A\x07"), vec![Mark::PromptStart]);
		assert_eq!(marks(b"\x1b]133;B\x07"), vec![Mark::PromptEnd]);
		assert_eq!(marks(b"\x1b]133;C\x07"), vec![Mark::OutputStart]);
		assert_eq!(marks(b"\x1b]133;D;0\x07"), vec![Mark::CommandEnd(Some(0))]);
	}

	#[test]
	fn a_failing_command_reports_its_exit_code() {
		// The exit code is the field after D — here 130, a Ctrl+C'd command.
		assert_eq!(
			marks(b"\x1b]133;D;130\x07"),
			vec![Mark::CommandEnd(Some(130))]
		);
	}

	#[test]
	fn a_bare_done_mark_has_no_exit_code() {
		// A shell that emits `133;D` with no code (it lost track of $?): the glyph shows "done",
		// not a wrong number.
		assert_eq!(marks(b"\x1b]133;D\x07"), vec![Mark::CommandEnd(None)]);
	}

	#[test]
	fn the_st_terminator_and_trailing_fields_are_accepted() {
		// ESC \ (ST) instead of BEL, and a `key=value` field after the letter that we ignore.
		assert_eq!(marks(b"\x1b]133;A;aid=7\x1b\\"), vec![Mark::PromptStart]);
	}

	#[test]
	fn a_whole_command_cycle_yields_its_marks_in_order() {
		// A/B, the command echoes, C, output, D — the shape of one prompt-to-result cycle.
		let stream =
			b"\x1b]133;A\x07user@host$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07file\r\n\x1b]133;D;0\x07";
		assert_eq!(
			marks(stream),
			vec![
				Mark::PromptStart,
				Mark::PromptEnd,
				Mark::OutputStart,
				Mark::CommandEnd(Some(0)),
			]
		);
	}

	#[test]
	fn the_offset_points_just_past_the_terminator() {
		// The offset a mark is tagged with is where the engine has been advanced to when the mark
		// is applied — the byte after the BEL, so the next segment starts there.
		let stream = b"xy\x1b]133;A\x07rest";
		let found = Scanner::default().feed(stream);
		assert_eq!(found.len(), 1);
		let (offset, mark) = found[0];
		assert_eq!(mark, Mark::PromptStart);
		// `xy` (2) + `ESC ] 1 3 3 ; A` (7) + `BEL` (1) = 10.
		assert_eq!(offset, 10);
		assert_eq!(&stream[offset..], b"rest");
	}

	#[test]
	fn a_sequence_split_across_chunks_completes_on_the_last_chunk() {
		// Output arrives in arbitrary chunks, including a split inside the payload; the mark
		// completes with the chunk that carries its terminator, and its offset is measured there.
		let mut scanner = Scanner::default();
		assert!(scanner.feed(b"prompt$ \x1b]133;").is_empty());
		let found = scanner.feed(b"A\x07after");
		assert_eq!(found, vec![(2, Mark::PromptStart)]);
	}

	#[test]
	fn a_split_between_the_esc_and_the_bracket_is_still_read() {
		// The nastiest boundary: the chunk ends right after the ESC, before the `]`.
		let mut scanner = Scanner::default();
		assert!(scanner.feed(b"\x1b").is_empty());
		assert_eq!(
			marks_of(scanner.feed(b"]133;C\x07")),
			vec![Mark::OutputStart]
		);
	}

	#[test]
	fn other_osc_sequences_are_ignored() {
		// A cwd announcement (OSC 7), a title (OSC 0) and a clipboard write (OSC 52) all pass
		// through this scanner: they are not OSC 133, so no mark comes out.
		assert!(marks(b"\x1b]7;file://host/home\x07").is_empty());
		assert!(marks(b"\x1b]0;a window title\x07").is_empty());
		assert!(marks(b"\x1b]52;c;Zm9v\x07").is_empty());
	}

	#[test]
	fn a_longer_number_is_not_mistaken_for_a_phase_letter() {
		// OSC 133 is our number; a different OSC that merely starts with those digits (there is
		// none standard, but a malformed `1330;A` must not match) is refused.
		assert!(marks(b"\x1b]1330;A\x07").is_empty());
	}

	#[test]
	fn an_overlong_payload_is_dropped_not_buffered() {
		// A hostile stream must not grow our memory: past the cap the payload is abandoned and
		// the scanner keeps hunting, so the next real mark is still read.
		let mut scanner = Scanner::default();
		scanner.feed(b"\x1b]133;A;");
		scanner.feed(&vec![b'x'; MAX_PAYLOAD + 10]);
		assert!(scanner.feed(b"\x07").is_empty());
		assert_eq!(
			marks_of(scanner.feed(b"\x1b]133;C\x07")),
			vec![Mark::OutputStart]
		);
	}

	/// Drop the offsets from a `feed` result, for the split-chunk tests above.
	fn marks_of(found: Vec<(usize, Mark)>) -> Vec<Mark> {
		found.into_iter().map(|(_, mark)| mark).collect()
	}

	#[test]
	fn the_command_state_follows_the_marks() {
		// The cycle: a prompt shows, the command runs, then it finishes with an exit code.
		let mut prompts = Prompts::default();
		assert_eq!(prompts.state(), CommandState::Idle);
		prompts.apply(Mark::PromptStart, 0, 0);
		assert_eq!(prompts.state(), CommandState::Prompt);
		prompts.apply(Mark::OutputStart, 0, 0);
		assert_eq!(prompts.state(), CommandState::Running);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 0);
		assert_eq!(prompts.state(), CommandState::Idle);
		assert_eq!(prompts.last_exit(), Some(0));
	}

	#[test]
	fn a_prompt_is_recorded_at_its_absolute_line() {
		// Two prompts recorded with 10 lines already scrolled off: one at active row 1 (absolute
		// 11) and one at active row 3 (absolute 13). At the live bottom the active screen spans
		// absolute 10..34, so they show at viewport rows 1 and 3.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 10, 1);
		prompts.apply(Mark::PromptStart, 10, 3);
		assert_eq!(prompts.visible_rows(10, 0, 24), vec![1, 3]);
	}

	#[test]
	fn a_redrawn_prompt_is_not_recorded_twice() {
		// A shell that redraws its prompt in place fires A again at the same line; the tick and
		// the jump target must not stack up.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 5, 2);
		prompts.apply(Mark::PromptStart, 5, 2);
		assert_eq!(prompts.visible_rows(5, 0, 24), vec![2]);
	}

	#[test]
	fn a_prompt_scrolled_off_the_top_leaves_the_visible_set() {
		// A prompt recorded at absolute line 2, viewed once 5 lines have scrolled off and the
		// view is at the live bottom, would sit at viewport row 2 - 5 = -3: off the top, dropped.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 2, 0);
		assert!(prompts.visible_rows(5, 0, 24).is_empty());
		// Scrolling back 3 lines (display_offset 3) brings it to viewport row 0 again.
		assert_eq!(prompts.visible_rows(5, 3, 24), vec![0]);
	}

	#[test]
	fn jumping_finds_the_nearest_prompt_in_each_direction() {
		// Prompts at absolute lines 2, 8 and 14, with 14 on the live screen and 20 lines of
		// history. From the live bottom (offset 0, top row is absolute 20) the previous prompt is
		// 14 -> its offset is 20 - 14 = 6.
		let mut prompts = Prompts::default();
		for line in [2u64, 8, 14] {
			prompts.apply(Mark::PromptStart, line as usize, 0);
		}
		assert_eq!(prompts.jump(Direction::Previous, 20, 0), Some(6));
		// From there (offset 6, top row is absolute 14) the next one up is 8 -> offset 12.
		assert_eq!(prompts.jump(Direction::Previous, 20, 6), Some(12));
		// And back down from offset 12 (top absolute 8) the next prompt below is 14 -> offset 6.
		assert_eq!(prompts.jump(Direction::Next, 20, 12), Some(6));
	}

	#[test]
	fn jumping_past_the_ends_finds_nothing() {
		// One prompt at absolute 5; from above it there is no earlier prompt, and from the live
		// bottom there is nothing further down.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 5, 0);
		// Viewing with the prompt already on the top row (offset = history - 5): nothing above it.
		assert_eq!(prompts.jump(Direction::Previous, 5, 0), None);
		assert_eq!(prompts.jump(Direction::Next, 5, 0), None);
	}
}

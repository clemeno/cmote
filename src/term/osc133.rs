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
// `D`'s exit code is optional in the grammar — vtdn writes the production as
// `"133", ";", "D", [ ";", exitcode ], ( 0x07 | 0x1b, "\\" )` and gives the bare form its own line,
// "Osc133Command finished (no exit code)". Two write-ups are reachable, Contour's
// (`contour-terminal.org/vt-extensions/osc-133-shell-integration/`, read for §95) and vtdn's
// (`vtdn.dev/docs/osc/osc133/`, read for §96), plus kitty's own shell integration
// (`sw.kovidgoyal.net/kitty/shell-integration/`, read for §97), which is not a write-up of the
// protocol but the shell code that emits it. Between them they name five optional `key=value`
// fields, and cmote reads exactly ONE:
//
//   133 ; A ; k=s                 — a SECONDARY prompt (zsh's PS2) — READ, and it suppresses the
//                                   mark, because a continuation prompt is not a new prompt (§97)
//   133 ; A ; click_events=1      — asks the terminal to report mouse clicks in the prompt area
//   133 ; A ; cl=m                — VS Code's hint that the prompt spans several lines
//   133 ; A ; special_key=1       — fish's own field on an ordinary prompt start; not a kind
//   133 ; C ; cmdline= | cmdline_url=
//                                 — the command line being run, shell-quoted (zsh) or
//                                   percent-encoded (fish)
//
// `click_events=1` is refused (§95): it turns on input reporting from a payload whose declared job
// is marking where the prompt sits, which is a side door around the mouse modes (§10) and would make
// a click inside the prompt behave unlike a click one line above it.
//
// `cl=m` and the command line are refused on one shared ground (§97): cmote can already SEE both.
// The prompt's extent is the grid between `A` and `B`; the command line is the grid between `B` and
// `C`. Taking the shell's word for either would put an assertion beside an observation cmote holds
// itself, and §71's rule is that the two can then disagree — with the remote winning.
//
// The four letters above are not the whole set, and until §164 this file said they could not be
// read because "the reachable accounts of them disagree". The accounts are write-ups; the thing
// they are write-ups OF is Per Bothner's semantic-prompts proposal, which §164 read, and it gives
// every letter ONE meaning. The three "disagreeing" reports turned out to be three partial views of
// it (§164).
//
// `N` is read since §164, as a prompt start. The proposal makes it "Same as `OSC "133;A"` but may
// first implicitly terminate a previous command: If the options specify an `aid` and there is an
// active (open) command with matching `aid`, finish the innermost such command ... If no `aid` is
// specified, treat as an `aid` whose value is the empty string." cmote tracks no `aid`, so every
// command it holds carries the empty one and the implicit termination is unconditional — which is
// exactly what `Prompts::apply` already does for `A`, superseding any half-built command. So `N` is
// `A` here, and the ARM says so rather than the model growing a variant that would behave
// identically (§71's rule about one answer, one mechanism).
//
// `P` is read since §164 too, and it is the letter the `k=` field really belongs to. The proposal:
// "Explicit start of prompt. Optional after an `A` or `N` command. The `k` (kind) option specifies
// the type of prompt: regular primary prompt (`k=i` or default), right-side prompts (`k=r`), or
// prompts for continuation lines (`k=c` or `k=s`)." So the three reports that looked like a
// disagreement are one rule: `133;P;k=i` for PS1 and `133;P;k=s` for PS2 is the taxonomy applied,
// and a fork using bare `133;P` for a REDRAW is using the one letter that starts a prompt without
// starting a command — which costs nothing here, because a redraw lands on the line the prompt is
// already anchored at and `record` drops a repeat of the last line.
//
// Only a CONTINUATION is not a prompt start. `k=r` keeps its mark: a right prompt is drawn on the
// same line as the prompt it decorates, so its anchor is the one already recorded there, and a
// stream that somehow sent only the right half would still have marked a prompt. `k=c` joins the
// `k=s` cmote already read, being the proposal's own second spelling of the same kind.
//
// `I` is read since §164 as a prompt end, `B`'s sibling: "End of prompt and start of user input,
// terminated by end-of-line." The two differ only in where the INPUT region ends — at the next `C`
// or prompt for `B`, at the line's end for `I` — and cmote reads no input region: `PromptEnd` moves
// nothing, the phase being the prompt's until output begins. So the distinction is real in the
// proposal and absent here, which is a divergence worth naming rather than a fold worth hiding. It
// would start to matter the day cmote showed the command line, and cmote refuses to (§71, §97).
//
// `L` is REFUSED (§164), and it is the only one of the four that is not a mark. The proposal: "Do a
// fresh-line: If the cursor is the initial column (left, assuming left-to-right writing), do
// nothing. Otherwise, it does the equivalent of `"\r\n"`." Every other letter tells cmote something
// ABOUT text the engine is already drawing. This one tells cmote to DRAW — and it arrives on a
// channel whose declared job is saying where the prompt sits, which is the ground `click_events=1`
// was refused on (§95), one level up: there a field, here the letter itself.
//
// The arrangement it would break is §34's own. This scanner observes and is a pure bytes -> marks
// function with no engine in it; `term::mod` places what it finds once the engine has been advanced
// to each offset; the engine draws. Honouring `L` makes the observer a writer — `Mark` gains a
// variant naming an action rather than a phase, `apply` grows the cursor COLUMN it does not take,
// and the OSC 133 path starts putting bytes on the grid beside the one that already does (§71, §73).
//
// The cost is stated rather than waved off: a stream that emits `133;L` and leans on it gets its
// prompt on the current line where another terminal would break the line first. cmote does not do
// the fresh-line on `A` either, which the proposal also specifies ("First do a fresh-line. Then
// start a new command"), so this is one decision consistently applied and not a special case made
// for `L`. Nothing reachable emits it: kitty's shell integration writes `A`, `C` and `D` only.
//
// The refusal is the letter list itself, which is an allow-list and drops what it does not name —
// there is no arm for `L`, because an arm returning `None` beside a wildcard returning `None` is a
// branch that never branches, and clippy's `match_same_arms` says so. What names `L` is the test,
// which fails the day the letter is wired to a mark. That is where a reader should look for the
// decision; this comment is where the argument for it lives.
//
// From those four marks a terminal knows where every prompt sits, whether a command is running,
// and how the last one ended — which is what powers "jump to the previous prompt" and a per-tab
// success/failure glyph (§34). Like the cwd (§17), modifyOtherKeys (§9) and the identity queries
// (§33), `alacritty_terminal` treats OSC 133 as an unknown OSC and ignores it, so cmote sniffs
// the same bytes out of the stream itself — and that reading of the crate is what vtdn's support
// table says too, listing Alacritty among the terminals that do not implement OSC 133 (§96).
//
// This scanner does ONE job: turn the byte stream into a list of completed marks, each tagged
// with the byte offset just past its terminator. It deliberately does NOT decide where a prompt
// lands on the grid or track the command's state — that is `term::mod`'s job, because a mark's
// grid line is only known once the engine has been advanced up to it (`Terminal::process` splits
// the advance at each offset to read the cursor there). Keeping the scanner a pure bytes -> marks
// function is what lets it be unit-tested without an engine at all.
//
// Finding where a sequence starts and ends — and how far into the chunk its terminator sat — is
// `term::osc`'s job, shared with the other scanners that read an OSC the engine ignores. What is
// left here is the part that is actually about shell integration: which payloads are marks, and how
// the four of them add up to a command's state.

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

/// Reads OSC 133 marks out of the shell's output. Feed it every byte; it returns the marks that
/// completed in that chunk, each with the offset just past its terminator so the caller can line
/// the mark up with the grid (§34). It holds only the in-flight sequence between calls.
#[derive(Debug, Default)]
pub struct Scanner {
	framer: super::osc::Framer<MAX_PAYLOAD>,
}

impl Scanner {
	/// Scan a chunk of shell output, returning every OSC 133 mark that finished in it. The
	/// `usize` in each pair is the byte offset in THIS `bytes` slice just past the mark's
	/// terminator — the point the engine has been advanced to when the mark is applied, so the
	/// cursor read there is exactly where the mark sits (`Terminal::process`). Safe at any chunk
	/// boundary: a sequence split across calls completes on the call that carries its terminator,
	/// and the offset is measured in that final chunk.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<(usize, Mark)> {
		// Every finished OSC arrives here; `parse` keeps only the marks, so a cwd announcement, a
		// title or a clipboard write passes through without producing anything.
		let mut marks = Vec::new();
		self.framer.feed(bytes, |offset, payload| {
			if let Some(mark) = parse(payload) {
				marks.push((offset, mark));
			}
		});
		marks
	}
}

/// Whether an OSC payload is the mark that ends a command (`133;D`). Exposed for §54, which has to
/// drop a command's progress bar the moment that command finishes — and has to do it in stream
/// order, because one chunk can carry a `D` and then the FIRST report of the next command. Reading
/// the payload itself is what makes that ordering possible; the grammar stays here, owned by the
/// module that defines it, rather than being copied into the one that needs the answer.
pub fn ends_command(payload: &[u8]) -> bool {
	matches!(parse(payload), Some(Mark::CommandEnd(_)))
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
		// `N` and `P` share this arm rather than getting one each (§164). `N` is the proposal's `A` plus
		// an implicit termination of the previous command, and `apply` already terminates
		// unconditionally because cmote holds no `aid` to match against. `P` is its "explicit start of
		// prompt", which is what this arm records. Sharing gives both the `k=` reading below, which is
		// where that field is actually defined — the proposal puts `k` on `P`, and kitty puts it on `A`.
		Some(b"A" | b"N" | b"P") => {
			// `k=s` marks a SECONDARY prompt — zsh's PS2, the one drawn for each continuation line
			// of a command still being typed (kitty's shell integration prepends this exact mark to
			// PS2; PS1 carries no `k=` at all). It is the one trailing field cmote cannot afford to
			// ignore, because a continuation prompt is not a new prompt and treating it as one costs
			// twice over (§97): every continuation line gets a gutter tick and a jump anchor, and —
			// worse — `Prompts::apply` starts a fresh `pending` span at each, so the finished command
			// is filed against its LAST continuation line instead of its prompt.
			//
			// Answered by dropping the mark rather than by carrying a new variant: cmote's model has
			// four phases and a continuation prompt is none of them. The stream is already in the
			// prompt phase when this arrives, so producing nothing leaves the state exactly right.
			//
			// Matched on the exact value. An unknown `k=` keeps the old behaviour deliberately —
			// mistaking a real prompt for a continuation would LOSE a jump anchor, where the reverse
			// only adds one, and between two guesses the recoverable one wins.
			//
			// `k=c` is the proposal's own second spelling of the same kind — "prompts for continuation
			// lines (`k=c` or `k=s`)" — and is suppressed with it since §164. The other two kinds are
			// prompts and keep their mark: `k=i` is the primary one, and `k=r` is the right-side prompt,
			// which shares its line with the prompt it decorates and so re-anchors a line `record`
			// already holds.
			if fields.any(|field| field == b"k=s" || field == b"k=c") {
				None
			} else {
				Some(Mark::PromptStart)
			}
		}
		// `I` shares `B`'s arm (§164): the proposal separates them by where the user's input ENDS, and
		// cmote holds no input region for the two to end differently in.
		Some(b"B" | b"I") => Some(Mark::PromptEnd),
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
pub enum Osc133Direction {
	Previous,
	Next,
}

/// The most prompt marks we keep. A jump only ever needs the nearest one in a direction and the
/// ticks only the ones on screen, so a bounded ring of the most recent prompts is plenty; past
/// this the oldest is dropped, which caps the memory a very long session can hold.
const MAX_MARKS: usize = 4096;

/// The most finished commands we keep the output span of — the same bound and reasoning as
/// `MAX_MARKS`: selecting an output only ever reaches a recent command, so a ring of the most
/// recent is plenty and caps the memory a long session holds.
const MAX_COMMANDS: usize = MAX_MARKS;

/// One finished command's line span, in ABSOLUTE line indices (§34) — the same scrollback-stable
/// coordinate the prompt marks use. `prompt` is the line the prompt sat on (the A mark, and so the
/// gutter tick a click lands on); `output` the line its output began on (the C mark); `end` the
/// line it finished on (the D mark, where the next prompt will draw). The output therefore occupies
/// the half-open line range `output .. end`, so a command that printed nothing has `output == end`.
#[derive(Debug, Clone, Copy)]
struct Osc133Command {
	prompt: u64,
	output: u64,
	end: u64,
}

impl Osc133Command {
	/// This command's output as an absolute half-open `(start, end)` line range, or `None` when it
	/// printed nothing (`output == end`, e.g. a bare Enter or a `cd`). The selection paths use this
	/// so an output-less command resolves to nothing to select rather than an empty highlight.
	fn range(self) -> Option<(u64, u64)> {
		(self.output < self.end).then_some((self.output, self.end))
	}
}

/// A command being stitched together as its marks arrive (§34): the prompt line is known at A, the
/// output line at C, and the whole span is finished and filed at D. Held between marks because the
/// three absolute lines are read at three different points in the stream.
#[derive(Debug, Clone, Copy)]
struct Pending {
	prompt: u64,
	output: Option<u64>,
}

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
	/// The lines a script bookmarked explicitly with iTerm2's `OSC 1337 ; SetMark` (§55), absolute
	/// like `marks` and bounded the same way.
	///
	/// Kept SEPARATE from `marks` rather than merged into it, because a bookmark is not a prompt.
	/// Nothing about it has a command state, an exit code or an output span, and `output_at_prompt`
	/// must not resolve one — a click on a bookmark's tick has no command to select. What the two do
	/// share is where they are shown and how they are reached, so `visible_rows` and `jump` treat
	/// them alike; only the tick's colour tells them apart, since a bookmark is somewhere the SCRIPT
	/// chose and a prompt is somewhere the shell was.
	user_marks: Vec<u64>,
	state: CommandState,
	last_exit: Option<i32>,
	/// The finished commands' output spans (§34), a bounded ring like `marks`. Built from the C and
	/// D marks so a command's output can be turned into a text selection; separate from `marks`,
	/// which holds only the prompt-start lines the ticks and jumps use.
	commands: Vec<Osc133Command>,
	/// The command currently being assembled from its marks, if a prompt has started but not yet
	/// finished (§34). `None` at rest and between commands.
	pending: Option<Pending>,
	/// How far back through `commands` the output selection has walked (§34): `None` before the
	/// first press, then an index INTO `commands` of the one selected last. Repeated Ctrl+Shift+O
	/// steps it further back through the session's history; a new command, a click on a prompt tick
	/// and any fresh grid press all put it back where it belongs.
	///
	/// An index into a ring that drops from the front is normally a hazard, and here it is safe for
	/// one reason worth stating: the only thing that drops a command is filing a new one, and filing
	/// a new one restarts the walk on the same call. So an index is never held across a shift.
	walk: Option<usize>,
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
		let absolute = super::as_document_line(history_size) + u64::from(row);
		match mark {
			Mark::PromptStart => {
				self.state = CommandState::Prompt;
				self.record(history_size, row);
				// Begin assembling this command's span (§34). A fresh prompt supersedes any
				// half-built one — a previous command that never reported its output start or end.
				self.pending = Some(Pending {
					prompt: absolute,
					output: None,
				});
			}
			// The prompt is written and input begins; still the prompt phase as far as the glyph
			// is concerned, so nothing changes.
			Mark::PromptEnd => {}
			Mark::OutputStart => {
				self.state = CommandState::Running;
				// The command's output begins on this line; record it for the output selection.
				if let Some(pending) = self.pending.as_mut() {
					pending.output = Some(absolute);
				}
			}
			Mark::CommandEnd(exit) => {
				self.state = CommandState::Idle;
				self.last_exit = exit;
				// The command finished on this line: file its span so its output can be selected.
				if let Some(pending) = self.pending.take() {
					self.file_command(pending, absolute);
				}
			}
		}
	}

	/// Record an explicit bookmark at absolute line `history_size + row` (§55) — iTerm2's
	/// `OSC 1337 ; SetMark`, applied once the engine has been advanced to it so the cursor names the
	/// line the script meant.
	///
	/// Deduped against the last bookmark exactly as prompts are, because a shell hook can emit one on
	/// every prompt redraw. Unlike `record`, this touches neither the command state nor the pending
	/// span: a bookmark says "here", and nothing about a command.
	pub fn record_user_mark(&mut self, history_size: usize, row: u16) {
		let absolute = super::as_document_line(history_size) + u64::from(row);
		if self.user_marks.last() == Some(&absolute) {
			return;
		}
		self.user_marks.push(absolute);
		if self.user_marks.len() > MAX_MARKS {
			self.user_marks.remove(0);
		}
	}

	/// Record a prompt at absolute line `history_size + row`. A prompt the shell redraws in place
	/// (zsh redraws on each keystroke) fires `A` again at the same line, so a repeat of the last
	/// recorded line is ignored rather than stacked. The store is a bounded ring (`MAX_MARKS`).
	fn record(&mut self, history_size: usize, row: u16) {
		let absolute = super::as_document_line(history_size) + u64::from(row);
		if self.marks.last() == Some(&absolute) {
			return;
		}
		self.marks.push(absolute);
		if self.marks.len() > MAX_MARKS {
			self.marks.remove(0);
		}
	}

	/// File a finished command's output span (§34), bounded like the prompt ring. `end` is the line
	/// the D mark sat on; a command with no output start (a bare Enter) takes `end` as its output
	/// line too, so its range comes out empty and it selects nothing — but it is still filed, so a
	/// click on its prompt tick resolves (to nothing) rather than falling through to a stray text
	/// selection.
	fn file_command(&mut self, pending: Pending, end: u64) {
		self.commands.push(Osc133Command {
			prompt: pending.prompt,
			output: pending.output.unwrap_or(end),
			end,
		});
		if self.commands.len() > MAX_COMMANDS {
			self.commands.remove(0);
		}
		// A command that has just finished is what the next Ctrl+Shift+O should reach for, whatever
		// the walk had wandered back to: running something new is the clearest possible statement
		// that the user is done reading old output (§34). It is also what makes the walk safe to hold
		// as an INDEX despite the ring above dropping from the front — every drop happens here, on
		// the same call that forgets the index, so a stored one can never be left pointing a place
		// along from where it was put.
		self.restart_walk();
	}

	/// The next output line-span going BACK through the session (§34), as absolute half-open
	/// `(start, end)` lines: the most recent finished command's on the first call, the one before it
	/// on the next, and so on. `None` when nothing has finished yet, or once the walk has reached
	/// the oldest command still held — at which point the walk stays there, so leaning on the key
	/// leaves the oldest output selected rather than wrapping round to the newest.
	///
	/// Commands that printed NOTHING are stepped over rather than stopping the walk. A `cd`, a bare
	/// Enter or a failed `cd` files a span with nothing in it (§34), and stopping on those would
	/// make the key look broken exactly when a session has a run of them.
	pub fn walk_output(&mut self) -> Option<(u64, u64)> {
		// Where to look from: one before the last one given, or the newest if the walk is fresh.
		let mut at = match self.walk {
			Some(at) => at.checked_sub(1)?,
			None => self.commands.len().checked_sub(1)?,
		};
		loop {
			if let Some(range) = self.commands[at].range() {
				self.walk = Some(at);
				return Some(range);
			}
			// Nothing printed here — keep going back. Running out means the walk stays where it was,
			// so the selection on screen is left alone rather than cleared.
			at = at.checked_sub(1)?;
		}
	}

	/// Start the walk over, so the next Ctrl+Shift+O takes the most recent command again (§34).
	/// Called when a command finishes and whenever the user does something else with the grid — the
	/// walk is a gesture, and any other gesture ends it.
	pub fn restart_walk(&mut self) {
		self.walk = None;
	}

	/// The output line-span of the finished command whose prompt sat on absolute line `prompt`
	/// (§34), or `None` when no finished command started there — the resolver behind clicking a
	/// prompt's gutter tick. Searched newest-first so a reused line resolves to its most recent
	/// command.
	///
	/// The click also PARKS THE WALK on that command, which is why this takes `&mut self`: a click
	/// picks a place in the history, so the Ctrl+Shift+O after it carries on back from there rather
	/// than jumping to the newest. The two ways of reaching a command's output are then one gesture
	/// — point at one, then keep stepping.
	pub fn output_at_prompt(&mut self, prompt: u64) -> Option<(u64, u64)> {
		let at = self
			.commands
			.iter()
			.rposition(|command| command.prompt == prompt)?;
		let range = self.commands[at].range()?;
		self.walk = Some(at);
		Some(range)
	}

	/// Forget every prompt (a full reset — `ESC c` — or the screen cleared them out from under us).
	/// The command spans and any half-built one go with them: their absolute lines no longer mean
	/// anything once the grid is reflowed or reset.
	pub fn clear(&mut self) {
		self.marks.clear();
		// The bookmarks are absolute lines too (§55), so a reflow invalidates them the same way.
		self.user_marks.clear();
		self.commands.clear();
		self.pending = None;
		// The walk indexed into the list that was just emptied.
		self.restart_walk();
	}

	/// Move every remembered line through a renumbering of the document (§101).
	///
	/// The one operation that needs this is UNSCROLL, which moves lines from the scrollback back onto
	/// the page and drops the page's bottom rows — so absolute indices below the seam shift, and the
	/// discarded ones stop naming anything. `remap` answers both questions at once: a new number, or
	/// `None` for a line whose content is gone.
	///
	/// Everything is renumbered together and a command is dropped whole if ANY of its three lines is,
	/// which is the conservative reading and the right one: a span with one end renumbered and the
	/// other not would select from a prompt to a line that no longer follows it. `clear` is the
	/// blunt version of this and stays for the cases where nothing can be salvaged — a reflow moves
	/// text between lines rather than moving lines, so there is no mapping to apply.
	pub fn renumber(&mut self, remap: impl Fn(u64) -> Option<u64>) {
		self.marks.retain_mut(|line| match remap(*line) {
			Some(moved) => {
				*line = moved;
				true
			}
			None => false,
		});
		self.user_marks.retain_mut(|line| match remap(*line) {
			Some(moved) => {
				*line = moved;
				true
			}
			None => false,
		});
		self.commands.retain_mut(|command| {
			match (
				remap(command.prompt),
				remap(command.output),
				remap(command.end),
			) {
				(Some(prompt), Some(output), Some(end)) => {
					command.prompt = prompt;
					command.output = output;
					command.end = end;
					true
				}
				_ => false,
			}
		});
		// A half-built command is dropped rather than repaired for the same reason: its remaining
		// marks will be recorded against the new numbering, and a `Pending` holding one of each would
		// file a span that spans the seam.
		if let Some(pending) = self.pending {
			let prompt = remap(pending.prompt);
			let output = pending.output.map(remap);
			self.pending = match (prompt, output) {
				(Some(prompt), None) => Some(Pending {
					prompt,
					output: None,
				}),
				(Some(prompt), Some(Some(output))) => Some(Pending {
					prompt,
					output: Some(output),
				}),
				_ => None,
			};
		}
		// The walk indexes into `commands`, which may just have lost entries under it.
		self.restart_walk();
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
		project(&self.marks, history_size, display_offset, screen_lines)
	}

	/// The same, for the explicit bookmarks a script dropped (§55). Kept a separate answer rather
	/// than folded into `visible_rows` because the two are drawn in different colours: a prompt is
	/// where the shell WAS, a bookmark is where the script said to look.
	pub fn visible_user_rows(
		&self,
		history_size: usize,
		display_offset: usize,
		screen_lines: usize,
	) -> Vec<u16> {
		project(&self.user_marks, history_size, display_offset, screen_lines)
	}

	/// The display offset that scrolls the nearest mark above or below the current viewport top
	/// into view (§34), or `None` when there is no mark in that direction. The viewport's top
	/// row shows absolute line `history_size - display_offset`; a jump lands the target mark on
	/// that top row, clamped to the retained history so it never asks to scroll past either end.
	///
	/// Prompts and the explicit bookmarks (§55) are considered TOGETHER here, unlike in the two
	/// `visible_*` answers. Being reachable is the whole point of a bookmark — a script that marks
	/// each stage of a build wants Ctrl+Shift+Up to visit those stages — and a user pressing the key
	/// is asking for "the last interesting line", not for a particular kind of interesting.
	pub fn jump(
		&self,
		direction: Osc133Direction,
		history_size: usize,
		display_offset: usize,
	) -> Option<usize> {
		// Signed throughout, because a mark above the viewport's top gives a negative difference and
		// that is the case the filters exist to tell apart (§111).
		let history = super::as_signed_line(super::as_document_line(history_size));
		let top = history - super::as_signed_line(super::as_document_line(display_offset));
		let anywhere = self
			.marks
			.iter()
			.chain(self.user_marks.iter())
			.map(|&mark| super::as_signed_line(mark));
		let target = match direction {
			// Strictly above the top so a repeated press keeps climbing rather than sticking.
			Osc133Direction::Previous => anywhere.filter(|&mark| mark < top).max()?,
			Osc133Direction::Next => anywhere.filter(|&mark| mark > top).min()?,
		};
		// Clamped into `0..=history_size` on the line above, so the conversion back cannot fail — and
		// this function already answers `None` when there is nowhere to jump, so it need not pretend.
		let offset = (history - target).clamp(0, history);
		usize::try_from(offset).ok()
	}
}

/// Absolute mark lines projected onto the viewport rows that are actually on screen (§34, §55). A
/// viewport row for an absolute line is `absolute - history_size + display_offset`; a mark scrolled
/// off the top or below the bottom falls outside `0..screen_lines` and is dropped.
///
/// Shared by the prompt ticks and the bookmark ticks: the two lists differ in what they mean and in
/// how they are drawn, but this arithmetic is the same for both and must stay so — a projection that
/// drifted between them would put one kind of tick a row off the line it marks.
fn project(
	marks: &[u64],
	history_size: usize,
	display_offset: usize,
	screen_lines: usize,
) -> Vec<u16> {
	let top = super::as_signed_line(super::as_document_line(history_size))
		- super::as_signed_line(super::as_document_line(display_offset));
	let rows = super::as_signed_line(super::as_document_line(screen_lines));
	marks
		.iter()
		.filter_map(|&absolute| {
			// Signed, because a mark scrolled off the TOP lands below zero — which is precisely what
			// the range check then drops. A `u16` row only exists once that check has passed, so the
			// conversion is the last step rather than an assumption made before it (§111).
			let row = super::as_signed_line(absolute) - top;
			(0..rows).contains(&row).then(|| u16::try_from(row).ok())?
		})
		.collect()
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
		// not a wrong number. Not a tolerated malformation but a documented spelling — vtdn gives
		// it its own line, "Osc133Command finished (no exit code)", and the grammar makes the field
		// optional (§96).
		assert_eq!(marks(b"\x1b]133;D\x07"), vec![Mark::CommandEnd(None)]);
	}

	#[test]
	fn the_st_terminator_and_trailing_fields_are_accepted() {
		// ESC \ (ST) instead of BEL, and a `key=value` field after the letter that we ignore.
		assert_eq!(marks(b"\x1b]133;A;aid=7\x1b\\"), vec![Mark::PromptStart]);
	}

	#[test]
	fn the_named_parameters_are_read_as_nothing_but_their_mark() {
		// Three optional fields are named between the two write-ups, and cmote answers each with
		// the bare mark and nothing else (§95, §96).
		//
		// `click_events=1` on A asks the terminal to "enable mouse click reporting for the prompt
		// area" — a remote turning on input reporting through a payload whose declared job is saying
		// where the prompt sits, and a side door around the mouse modes (§10). There is no path from
		// this scanner to a mouse mode at all: `Mark` has four variants and none carries a field, so
		// the refusal is structural. This test is what makes it deliberate rather than incidental.
		assert_eq!(
			marks(b"\x1b]133;A;click_events=1\x07"),
			vec![Mark::PromptStart]
		);
		// `cmdline_url=<percent-encoded>` on C carries the command line itself. Nothing in cmote
		// shows which command a range of output belongs to, so there is no reader for it and it is
		// dropped like any other trailing field rather than decoded and held.
		assert_eq!(
			marks(b"\x1b]133;C;cmdline_url=ls%20-la\x07"),
			vec![Mark::OutputStart]
		);
		// `cl=m` on A is VS Code's hint that the prompt spans several lines. Ignoring it costs
		// nothing: a prompt jump anchors on this mark's own line, which is the prompt's first line
		// with or without the hint (§96).
		assert_eq!(marks(b"\x1b]133;A;cl=m\x07"), vec![Mark::PromptStart]);
	}

	#[test]
	fn a_secondary_prompt_is_not_a_prompt_start() {
		// zsh's PS2 under kitty's shell integration, once per continuation line of a command still
		// being typed. It must not anchor a prompt: the ticks would multiply and the command would
		// be filed against its last continuation line (§97).
		assert!(marks(b"\x1b]133;A;k=s\x07").is_empty());
		// A real prompt carries no `k=` — and fish's own field on the same mark is not a kind, so
		// it stays a prompt start.
		assert_eq!(marks(b"\x1b]133;A\x07"), vec![Mark::PromptStart]);
		assert_eq!(
			marks(b"\x1b]133;A;special_key=1\x07"),
			vec![Mark::PromptStart]
		);
		// An unknown kind keeps the old behaviour on purpose: losing a jump anchor is worse than
		// gaining one.
		assert_eq!(marks(b"\x1b]133;A;k=x\x07"), vec![Mark::PromptStart]);
	}

	#[test]
	fn a_multi_line_entry_anchors_only_its_real_prompt() {
		// The shape the fix is for: PS1, the first line typed, two PS2 continuations, then the
		// command runs. One prompt start, not three.
		let stream = b"\x1b]133;A\x07$ \x1b]133;B\x07for i in 1 2\r\n\x1b]133;A;k=s\x07> do echo\r\n\x1b]133;A;k=s\x07> done\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
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

	/// The eight phase letters the proposal defines (§164). Used to pin the width of the allow-list
	/// `parse` is built on, by walking the rest of the alphabet and asserting none of them marks.
	///
	/// Derived-from rather than written out, which is the opposite choice from `term/modkeys.rs`'s
	/// `OTHER_RESOURCES` and for the opposite reason: XTMODKEYS' numbering has a hole at 5, so a range
	/// there would assert about a resource that does not exist, while here the refused set really is
	/// the alphabet minus a list.
	const SPECIFIED_LETTERS: &[u8] = b"ABCDILNP";

	#[test]
	fn every_letter_the_proposal_does_not_define_is_refused() {
		// The letter list in `parse` is an allow-list, and this is what says how wide (§164). The eight
		// specified letters have their own tests above — including `L`, which is specified and refused;
		// what is walked here is the rest of the alphabet, none of which any source gives a meaning.
		// A letter nobody defines cannot be a gap: there is nothing to build, and what cmote does when
		// one arrives is decide.
		for letter in b'A'..=b'Z' {
			if SPECIFIED_LETTERS.contains(&letter) {
				continue;
			}
			let name = char::from(letter);
			let stream = [b"\x1b]133;".as_slice(), &[letter], b"\x07"].concat();
			assert!(
				marks(&stream).is_empty(),
				"133;{name} produced a mark, so the letter list has stopped being an allow-list — \
				 see PLAN §164"
			);
		}
	}

	#[test]
	fn an_unrecognised_phase_letter_yields_no_mark() {
		// A letter the proposal does not define produces nothing rather than being guessed into the
		// nearest phase — a wrong mark would move a prompt jump or mis-bound a command's output, where
		// no mark just leaves both as they were (§96, §164).
		assert!(marks(b"\x1b]133;Z\x07").is_empty());
	}

	#[test]
	fn the_fresh_line_letter_is_refused_and_disturbs_nothing_around_it() {
		// `L` asks the terminal to WRITE — "if the cursor is the initial column, do nothing; otherwise
		// do the equivalent of `\r\n`" — and this scanner observes (§164). It produces no mark, and
		// this test is what stops it quietly becoming one the next time the letter list is widened.
		assert!(marks(b"\x1b]133;L\x07").is_empty());
		// And the refusal is inert rather than disruptive: a stream that sprinkles `L` between the real
		// marks reads exactly as the same stream without it. What the user loses is the line break the
		// shell asked for, which is the cost the row names — not a mark, and not an offset.
		let with_l = b"\x1b]133;L\x07\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
		let without = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
		assert_eq!(marks(with_l), marks(without));
	}

	#[test]
	fn the_explicit_prompt_start_reads_its_kind_and_only_a_continuation_loses_its_mark() {
		// `P` is the proposal's "explicit start of prompt", and the letter its `k` (kind) option is
		// actually defined on (§164). A bare one and the primary kind are prompt starts.
		assert_eq!(marks(b"\x1b]133;P\x07"), vec![Mark::PromptStart]);
		assert_eq!(marks(b"\x1b]133;P;k=i\x07"), vec![Mark::PromptStart]);
		// A right-side prompt keeps its mark: it is drawn on the line the prompt it decorates already
		// anchored, so the anchor is a repeat rather than a new place to jump to.
		assert_eq!(marks(b"\x1b]133;P;k=r\x07"), vec![Mark::PromptStart]);
		// Both spellings of the continuation kind are suppressed, on either letter — the field means
		// the same thing wherever it rides.
		assert!(marks(b"\x1b]133;P;k=c\x07").is_empty());
		assert!(marks(b"\x1b]133;P;k=s\x07").is_empty());
		assert!(marks(b"\x1b]133;A;k=c\x07").is_empty());
		// An unrecognised kind still keeps the mark, which is §97's rule and unchanged: losing a jump
		// anchor is worse than gaining one, so the recoverable guess wins.
		assert_eq!(marks(b"\x1b]133;P;k=v\x07"), vec![Mark::PromptStart]);
	}

	#[test]
	fn the_line_terminated_spelling_of_a_prompt_end_is_a_prompt_end() {
		// `I` is the proposal's "end of prompt and start of user input, terminated by end-of-line" —
		// `B` with a different end to the input region, and cmote holds no input region for the two to
		// differ in (§164). A whole cycle spelled with it reads exactly as one spelled with `B`.
		assert_eq!(marks(b"\x1b]133;I\x07"), vec![Mark::PromptEnd]);
		let with_i = b"\x1b]133;A\x07$ \x1b]133;I\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
		let with_b = b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\r\n\x1b]133;D;0\x07";
		assert_eq!(marks(with_i), marks(with_b));
	}

	#[test]
	fn a_redrawn_prompt_spelled_as_an_explicit_start_does_not_open_a_second_block() {
		// The Ghostty fork's use of `133;P` — a prompt REDRAW that must not open a new block — is the
		// report that made this letter look like it contradicted the others (§164). It costs nothing
		// here: a redraw lands on the line the prompt is already anchored at, so `record` drops it as a
		// repeat and the command filed afterwards still runs from the original prompt line.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 2);
		prompts.apply(Mark::PromptStart, 0, 2);
		assert_eq!(prompts.visible_rows(0, 0, 24), vec![2]);
		prompts.apply(Mark::OutputStart, 0, 3);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 5);
		assert_eq!(prompts.output_at_prompt(2), Some((3, 5)));
	}

	#[test]
	fn the_implicitly_terminating_spelling_of_a_prompt_start_is_a_prompt_start() {
		// `N` is the proposal's "same as A but may first implicitly terminate a previous command",
		// which is what Konsole emits and what this file used to call unreadable (§164). cmote holds no
		// `aid`, so the termination is unconditional — and superseding a half-built command is already
		// what a prompt start does here, which is why `N` needs no variant of its own.
		assert_eq!(marks(b"\x1b]133;N\x07"), vec![Mark::PromptStart]);
		// The fields are read the same way they are on `A`: a continuation prompt is still not a new
		// prompt when the letter is `N`.
		assert!(marks(b"\x1b]133;N;k=s\x07").is_empty());
		// And the implicit termination is the one `apply` already performs: a command left half-built by
		// a stream that never sent its `D` is superseded rather than filed. Lines 0 and 1 open a command
		// that never ends; the prompt on line 4 abandons it; lines 5 and 6 are the command that does.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::OutputStart, 0, 1);
		prompts.apply(Mark::PromptStart, 0, 4);
		prompts.apply(Mark::OutputStart, 0, 5);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 6);
		// The filed span is the SECOND command's, and it is the only one filed — the abandoned command
		// took its half-built span with it rather than leaving a span from line 1 to line 6.
		assert_eq!(prompts.walk_output(), Some((5, 6)));
		assert_eq!(prompts.walk_output(), None);
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
		for line in [2usize, 8, 14] {
			prompts.apply(Mark::PromptStart, line, 0);
		}
		assert_eq!(prompts.jump(Osc133Direction::Previous, 20, 0), Some(6));
		// From there (offset 6, top row is absolute 14) the next one up is 8 -> offset 12.
		assert_eq!(prompts.jump(Osc133Direction::Previous, 20, 6), Some(12));
		// And back down from offset 12 (top absolute 8) the next prompt below is 14 -> offset 6.
		assert_eq!(prompts.jump(Osc133Direction::Next, 20, 12), Some(6));
	}

	#[test]
	fn jumping_past_the_ends_finds_nothing() {
		// One prompt at absolute 5; from above it there is no earlier prompt, and from the live
		// bottom there is nothing further down.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 5, 0);
		// Viewing with the prompt already on the top row (offset = history - 5): nothing above it.
		assert_eq!(prompts.jump(Osc133Direction::Previous, 5, 0), None);
		assert_eq!(prompts.jump(Osc133Direction::Next, 5, 0), None);
	}

	#[test]
	fn a_bookmark_ticks_its_own_line_in_its_own_list() {
		// §55. A script marked absolute line 12 (row 2 with 10 lines scrolled off). It shows in the
		// bookmark list and NOT among the prompts, because the two are drawn in different colours.
		let mut prompts = Prompts::default();
		prompts.record_user_mark(10, 2);
		assert_eq!(prompts.visible_user_rows(10, 0, 24), vec![2]);
		assert!(prompts.visible_rows(10, 0, 24).is_empty());
	}

	#[test]
	fn a_bookmark_repeated_on_one_line_is_not_stacked() {
		// A shell hook that emits SetMark on every prompt redraw fires it repeatedly at the same
		// line, exactly as `A` does.
		let mut prompts = Prompts::default();
		prompts.record_user_mark(5, 2);
		prompts.record_user_mark(5, 2);
		assert_eq!(prompts.visible_user_rows(5, 0, 24), vec![2]);
	}

	#[test]
	fn a_jump_visits_bookmarks_and_prompts_alike() {
		// §55. A prompt at absolute 2 and a bookmark at absolute 14, with 20 lines of history. From
		// the live bottom the nearest thing above is the BOOKMARK (14 -> offset 6) — being reachable
		// is the whole point of dropping one, so the jump must not see only prompts.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 2, 0);
		prompts.record_user_mark(14, 0);
		assert_eq!(prompts.jump(Osc133Direction::Previous, 20, 0), Some(6));
		// Carrying on up from there reaches the prompt at 2 -> offset 18.
		assert_eq!(prompts.jump(Osc133Direction::Previous, 20, 6), Some(18));
		// And back down from above the prompt, the bookmark is the next one below.
		assert_eq!(prompts.jump(Osc133Direction::Next, 20, 18), Some(6));
	}

	#[test]
	fn a_bookmark_is_not_a_command_and_resolves_to_no_output() {
		// The reason bookmarks are stored apart from prompts: nothing about one has an output span,
		// so a click on its tick must find nothing rather than a stray selection from another
		// command that happened to sit on that line.
		let mut prompts = Prompts::default();
		prompts.record_user_mark(0, 4);
		assert_eq!(prompts.output_at_prompt(4), None);
		// And it moves neither the command state nor the exit code.
		assert_eq!(prompts.state(), CommandState::Idle);
		assert_eq!(prompts.last_exit(), None);
	}

	#[test]
	fn a_reflow_forgets_bookmarks_with_the_prompts() {
		// Both are absolute line indices, so a resize invalidates them together (§34, §55).
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 1);
		prompts.record_user_mark(0, 2);
		prompts.clear();
		assert!(prompts.visible_rows(0, 0, 24).is_empty());
		assert!(prompts.visible_user_rows(0, 0, 24).is_empty());
	}

	#[test]
	fn a_finished_command_records_its_output_span() {
		// A whole cycle with two lines of output: prompt on absolute line 0, output begins on line
		// 1 (the C mark), the command finishes on line 3 (the D mark). The output is the half-open
		// range [1, 3), so lines 1 and 2.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::OutputStart, 0, 1);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 3);
		assert_eq!(prompts.walk_output(), Some((1, 3)));
	}

	#[test]
	fn output_is_found_by_the_prompt_line_a_click_lands_on() {
		// Two commands, one after another; clicking a prompt tick resolves to that command's output,
		// not merely the latest. First: prompt line 0, output 1..2. Second: prompt line 5, output
		// 6..8.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::OutputStart, 0, 1);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 2);
		prompts.apply(Mark::PromptStart, 5, 0);
		prompts.apply(Mark::OutputStart, 5, 1);
		prompts.apply(Mark::CommandEnd(Some(0)), 5, 3);
		// The keybind with no walk under way takes the latest of the two …
		assert_eq!(prompts.walk_output(), Some((6, 8)));
		// … and a click reaches either, whichever the walk had got to.
		assert_eq!(prompts.output_at_prompt(0), Some((1, 2)));
		assert_eq!(prompts.output_at_prompt(5), Some((6, 8)));
		// A prompt line no command started at resolves to nothing.
		assert_eq!(prompts.output_at_prompt(99), None);
	}

	#[test]
	fn a_command_that_printed_nothing_selects_nothing() {
		// A bare Enter: the prompt starts and the command ends on the same line with no output start
		// between. Its span is empty, so it resolves to nothing to select — but it was still filed,
		// so its prompt line is a known command (returning `None`), not an unknown one.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 0);
		assert_eq!(prompts.walk_output(), None);
		assert_eq!(prompts.output_at_prompt(0), None);
	}

	#[test]
	fn an_unfinished_command_files_nothing_until_it_ends() {
		// A prompt has started and its output is streaming (C seen, no D yet): there is no finished
		// command, so nothing is selectable. The D mark is what files the span.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::OutputStart, 0, 1);
		assert_eq!(prompts.walk_output(), None);
		prompts.apply(Mark::CommandEnd(None), 0, 2);
		assert_eq!(prompts.walk_output(), Some((1, 2)));
	}

	/// Run one whole command cycle: a prompt on absolute line `at`, output from `at + 1`, finishing
	/// on `end`. Written out here because the walk tests need several commands each and the marks
	/// themselves are not what those tests are about.
	fn run_command(prompts: &mut Prompts, at: usize, end: usize) {
		prompts.apply(Mark::PromptStart, at, 0);
		prompts.apply(Mark::OutputStart, at + 1, 0);
		prompts.apply(Mark::CommandEnd(Some(0)), end, 0);
	}

	#[test]
	fn repeated_presses_walk_back_through_the_commands() {
		// Three commands, newest last. Ctrl+Shift+O takes the newest, then the one before it, then
		// the one before that — which is what makes the key a way of reading back through a session
		// rather than a way of grabbing the last thing only (§34).
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		run_command(&mut prompts, 10, 13);
		run_command(&mut prompts, 20, 23);
		assert_eq!(prompts.walk_output(), Some((21, 23)));
		assert_eq!(prompts.walk_output(), Some((11, 13)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
	}

	#[test]
	fn the_walk_stays_on_the_oldest_command_it_still_holds() {
		// Past the oldest there is nothing to select, and the answer is `None` rather than the newest
		// again: leaning on the key leaves the oldest output selected instead of silently starting
		// the session over from the top, which would look like the key had done something else.
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		run_command(&mut prompts, 10, 13);
		assert_eq!(prompts.walk_output(), Some((11, 13)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
		assert_eq!(prompts.walk_output(), None);
		assert_eq!(prompts.walk_output(), None);
	}

	#[test]
	fn the_walk_steps_over_commands_that_printed_nothing() {
		// A `cd`, a bare Enter and a failed `cd` all file a span with nothing in it. Stopping on one
		// would make the key look broken exactly when a session has a run of them, so the walk keeps
		// going back until it finds output (§34).
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		// Two commands that printed nothing: prompt and end on the same line, no output start.
		prompts.apply(Mark::PromptStart, 10, 0);
		prompts.apply(Mark::CommandEnd(Some(0)), 10, 0);
		prompts.apply(Mark::PromptStart, 11, 0);
		prompts.apply(Mark::CommandEnd(Some(1)), 11, 0);
		run_command(&mut prompts, 20, 23);
		assert_eq!(prompts.walk_output(), Some((21, 23)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
	}

	#[test]
	fn a_command_that_finishes_puts_the_walk_back_at_the_newest() {
		// Reading back through history, then running something new: the next press must take what
		// was just run. Running a command is the clearest statement there is that the user has
		// stopped reading old output (§34).
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		run_command(&mut prompts, 10, 13);
		assert_eq!(prompts.walk_output(), Some((11, 13)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
		run_command(&mut prompts, 20, 23);
		assert_eq!(prompts.walk_output(), Some((21, 23)));
	}

	#[test]
	fn restarting_the_walk_takes_the_newest_again() {
		// What a press on the grid does (§34): the walk is one gesture, and anything else the user
		// does with the grid ends it.
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		run_command(&mut prompts, 10, 13);
		assert_eq!(prompts.walk_output(), Some((11, 13)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
		prompts.restart_walk();
		assert_eq!(prompts.walk_output(), Some((11, 13)));
	}

	#[test]
	fn clicking_a_prompt_parks_the_walk_there_so_the_next_press_carries_on_back() {
		// The two ways to reach a command's output are one gesture (§34): point at a prompt tick,
		// then keep stepping back from it. Without the parking, the press after a click would jump
		// to the newest, which reads as the key having lost its place.
		let mut prompts = Prompts::default();
		run_command(&mut prompts, 0, 3);
		run_command(&mut prompts, 10, 13);
		run_command(&mut prompts, 20, 23);
		assert_eq!(prompts.output_at_prompt(10), Some((11, 13)));
		assert_eq!(prompts.walk_output(), Some((1, 3)));
	}

	#[test]
	fn clearing_forgets_the_command_spans() {
		// A reset drops the filed commands along with the prompt marks (§34): their absolute lines
		// no longer line up after the grid is reflowed.
		let mut prompts = Prompts::default();
		prompts.apply(Mark::PromptStart, 0, 0);
		prompts.apply(Mark::OutputStart, 0, 1);
		prompts.apply(Mark::CommandEnd(Some(0)), 0, 2);
		assert_eq!(prompts.walk_output(), Some((1, 2)));
		prompts.clear();
		assert_eq!(prompts.walk_output(), None);
	}
}

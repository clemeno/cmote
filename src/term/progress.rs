// term/progress.rs — read the progress a remote command reports (OSC 9;4, PLAN §54).
//
// A long command has no way to tell a terminal how far along it is, so the convention ConEmu
// introduced and Windows Terminal adopted is an OSC:
//
//   ESC ] 9 ; 4 ; st ; pr   BEL | ST
//
//     st = 0  remove progress          pr ignored
//     st = 1  a share of work is done  pr = 0..100
//     st = 2  the work failed          pr optional — stays where it was
//     st = 3  working, share unknown   pr ignored
//     st = 4  paused / needs attention pr optional — stays where it was
//
// Note which OSC number this is. OSC 9 is multiplexed: `9;9` is the Windows cwd announcement cmote
// already reads (`term::cwd`, §17), `9;4` is this, and a BARE `9;<text>` is a desktop notification.
// cmote reads the first two and refuses the third, on purpose — see the refusal recorded in
// TERMINAL_COMPATIBILITY_PLAN §6 and §8. The distinction is not fussiness: a notification LEAVES the
// window and lands on the user's desktop, so a remote that lied would be spamming the machine;
// progress cannot leave the tab it belongs to. That is the whole reason one is safe and the other is
// not, and it is why implementing 9;4 does not reopen 9.
//
// Since §79 that refusal is STATED rather than implied, and it covers all three spellings of the
// feature rather than only the one that shares this OSC number: `feed` asks `term::notify` about
// every payload before reading it, so `9;<text>`, `777;notify;…` and kitty's `99;…` are declined by
// name here. Nothing about the outcome changes — none of the three would have been read anyway —
// but a refusal that rests on nobody happening to match it is the shape §63 had to correct on the
// OSC 52 row, and this module is where every OSC payload already passes.
//
// Everything a remote sends here is untrusted, so the parse is defensive in a specific way: a
// malformed or unknown sequence changes NOTHING. It does not clear the bar, because "the remote sent
// us rubbish" must not be a way to wipe a real reading — and a percentage is clamped rather than
// believed, so a claimed 4 billion is 100.
//
// Framing (finding the sequence in a stream that arrives in arbitrary chunks) is `term::osc`'s job.
// What is left here is what a payload MEANS.

/// The longest payload we will buffer while looking for one of these. `9;4;2;100` is ten bytes, so
/// this is generous for the sequence and its variants while bounding what a hostile stream can make
/// us hold (§12). Every other OSC flows through the same framer and is simply not ours.
const MAX_PAYLOAD: usize = 128;

/// The largest share the bar can show. A remote that claims more is clamped, not believed.
const FULL: u32 = 100;

/// What a remote command last reported about its progress (§54). The percentages are 0..=100.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Progress {
	/// Nothing to show: no command has reported, or one explicitly cleared its report (`st = 0`).
	#[default]
	None,
	/// Work is under way, but the remote cannot say how far (`st = 3`).
	Indeterminate,
	/// This share of the work is done (`st = 1`).
	Working(u8),
	/// The work failed, at the share it had reached (`st = 2`).
	Failed(u8),
	/// The work is paused or wants the user's attention, at the share it had reached (`st = 4`).
	Paused(u8),
}

impl Progress {
	/// The share reported, when there is one. `None` and `Indeterminate` have no number to give —
	/// which is exactly the difference a progress bar has to render as "empty" and "pulsing".
	pub fn percent(self) -> Option<u8> {
		match self {
			Progress::None | Progress::Indeterminate => None,
			Progress::Working(share) | Progress::Failed(share) | Progress::Paused(share) => {
				Some(share)
			}
		}
	}
}

/// Tracks the progress a tab's remote command reports (§54). Feed it every byte of shell output; it
/// keeps the most recent report and ignores every other OSC.
#[derive(Debug, Default)]
pub struct Reports {
	framer: super::osc::Framer<MAX_PAYLOAD>,
	current: Progress,
}

impl Reports {
	/// Scan a chunk of shell output for a progress report. Safe at any chunk boundary — the
	/// framer's state carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) {
		let current = &mut self.current;
		self.framer.feed(bytes, |_offset, payload| {
			// The desktop notification cmote refuses, in whichever of its three spellings it
			// arrived (§79). Declining it here rather than letting it fall off the end of `parse`
			// is §63's move on the OSC 52 row: the payload was already going to be ignored, and a
			// refusal that rests on being ignored is one no test can see and one a later reader
			// can undo without noticing they have. This module performs it because it is the one
			// that already frames every OSC payload and already owned the bare `9;<text>` half.
			if super::notify::refused(payload).is_some() {
				return;
			}
			// A command ending takes its bar with it (§34's `D`). This is judged HERE, payload by
			// payload, rather than by the caller after the chunk, because the framer hands them over
			// in stream order and one chunk can easily carry a `D` and then the first report of the
			// next command — clearing afterwards would wipe that new report instead of the old one.
			if super::osc133::ends_command(payload) {
				*current = Progress::None;
				return;
			}
			// `parse` is given what we hold so `st = 2` and `st = 4` can report "failed/paused where
			// we already were" without the remote repeating the number. A payload that is not a
			// progress report — a title, a cwd, a prompt mark — yields `None` and leaves us alone.
			if let Some(next) = parse(payload, *current) {
				*current = next;
			}
		});
	}

	/// What this tab last reported.
	pub fn current(&self) -> Progress {
		self.current
	}
}

// There is deliberately no `clear` here for the caller to reach. The two endings that matter both
// arrive in the stream — the remote's own `st = 0`, and §34's `D` mark when the command finishes —
// and `feed` handles them in order. A resize is the one place a caller might reach for it, and it
// must NOT: `Terminal::resize` drops the prompt marks and the inline images because both are
// anchored to grid positions a reflow invalidates, whereas a progress reading has no place on the
// grid at all. Wiping a running command's bar because the window got wider would be a bug.

/// Read one OSC payload as a progress report, or `None` when it is not one — or is malformed.
/// `current` is what we already hold, which the states that carry an optional share fall back to.
///
/// Returning `None` rather than a default is the load-bearing choice here: it is what makes a
/// garbled sequence a no-op instead of a way for a remote to blank a real reading.
fn parse(payload: &[u8], current: Progress) -> Option<Progress> {
	// `9;4;` identifies the sequence. `9;9;` (a cwd) and a bare `9;` (a notification we refuse) both
	// fail this and are none of our business.
	let rest = payload.strip_prefix(b"9;4;")?;
	let mut fields = rest.split(|&byte| byte == b';');
	let state = number(fields.next()?)?;
	// The share, when the remote sent a readable one. Clamped, because this number is drawn.
	let share = fields
		.next()
		.and_then(number)
		.map(|value| value.min(FULL) as u8);
	// Where `st = 2` / `st = 4` land when the remote gives no share: at whatever we already showed,
	// which is what "it failed" means without a fresh number. A tab showing nothing yet stays at 0.
	let held = current.percent().unwrap_or(0);

	match state {
		0 => Some(Progress::None),
		// A share is the entire content of `st = 1`, so one that did not arrive makes the sequence
		// meaningless — and "working, amount unknown" already has its own state (3). Ignore it
		// rather than invent a number.
		1 => Some(Progress::Working(share?)),
		2 => Some(Progress::Failed(share.unwrap_or(held))),
		3 => Some(Progress::Indeterminate),
		4 => Some(Progress::Paused(share.unwrap_or(held))),
		// An `st` we do not know is a newer or broken sender; leave what we have on screen.
		_ => None,
	}
}

/// One decimal field as a number, or `None` when it is empty, non-numeric, or too big to hold. A
/// remote controls these bytes, so nothing here may panic or wrap.
fn number(field: &[u8]) -> Option<u32> {
	std::str::from_utf8(field).ok()?.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh tracker and read what it holds.
	fn track(bytes: &[u8]) -> Progress {
		let mut reports = Reports::default();
		reports.feed(bytes);
		reports.current()
	}

	#[test]
	fn a_share_of_work_is_read_and_shown() {
		assert_eq!(track(b"\x1b]9;4;1;42\x07"), Progress::Working(42));
		assert_eq!(track(b"\x1b]9;4;1;42\x07").percent(), Some(42));
	}

	#[test]
	fn state_zero_clears_the_bar() {
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;42\x07");
		reports.feed(b"\x1b]9;4;0\x07");
		assert_eq!(reports.current(), Progress::None);
	}

	#[test]
	fn indeterminate_work_has_no_number_to_show() {
		let progress = track(b"\x1b]9;4;3\x07");
		assert_eq!(progress, Progress::Indeterminate);
		// The difference that matters to the drawing: something IS shown, but not as a share — so a
		// bar appears, and it cannot be sized from a number.
		assert_ne!(progress, Progress::None);
		assert_eq!(progress.percent(), None);
	}

	#[test]
	fn a_failure_without_a_number_stays_where_the_work_had_reached() {
		// The documented shape of `st = 2`: the share is optional, and omitting it means "it failed
		// here", not "it failed at zero".
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;70\x07");
		reports.feed(b"\x1b]9;4;2\x07");
		assert_eq!(reports.current(), Progress::Failed(70));
	}

	#[test]
	fn a_pause_carries_its_own_number_when_it_gives_one() {
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;10\x07");
		reports.feed(b"\x1b]9;4;4;55\x07");
		assert_eq!(reports.current(), Progress::Paused(55));
	}

	#[test]
	fn a_share_over_one_hundred_is_clamped_not_believed() {
		// The number is drawn, so a remote must not be able to push the bar past its own width.
		assert_eq!(track(b"\x1b]9;4;1;4000000000\x07"), Progress::Working(100));
		assert_eq!(track(b"\x1b]9;4;1;101\x07"), Progress::Working(100));
	}

	#[test]
	fn a_number_too_big_for_the_field_is_not_a_reading() {
		// Past u32 the parse fails outright; that is a malformed sequence, so nothing changes rather
		// than the value wrapping to something small and plausible.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;30\x07");
		reports.feed(b"\x1b]9;4;1;99999999999999999999\x07");
		assert_eq!(reports.current(), Progress::Working(30));
	}

	#[test]
	fn a_garbled_report_never_wipes_a_real_one() {
		// The security-shaped rule: rubbish from a remote is a no-op. Every one of these is
		// malformed in a different way, and none of them may clear the 30% already on screen.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;30\x07");
		for bad in [
			&b"\x1b]9;4;1\x07"[..],     // `st = 1` with no share at all
			&b"\x1b]9;4;9;50\x07"[..],  // an `st` we do not know
			&b"\x1b]9;4;x;50\x07"[..],  // a non-numeric `st`
			&b"\x1b]9;4;1;abc\x07"[..], // a non-numeric share
			&b"\x1b]9;4;\x07"[..],      // no fields
			&b"\x1b]9;4;1;\x07"[..],    // an empty share
		] {
			reports.feed(bad);
			assert_eq!(reports.current(), Progress::Working(30), "wiped by {bad:?}");
		}
	}

	#[test]
	fn the_other_osc_nine_sequences_are_left_alone() {
		// OSC 9 is multiplexed. `9;9` is the cwd announcement (§17) and a bare `9;<text>` is the
		// desktop notification cmote refuses — neither is a progress report, and reading one as a
		// report would be how the refused feature leaked in through this door.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;30\x07");
		reports.feed(b"\x1b]9;9;C:\\Users\\CLEm\x07");
		reports.feed(b"\x1b]9;Build finished\x07");
		reports.feed(b"\x1b]0;a window title\x07");
		assert_eq!(reports.current(), Progress::Working(30));
	}

	#[test]
	fn the_other_two_notification_spellings_are_declined_here_too() {
		// §79. This module is where cmote performs the refusal for all three dialects, not just
		// the one that shares OSC 9 — so urxvt's and kitty's pass through `feed` and must leave a
		// running bar exactly as they found it. The order is the argument, as in §77's: a real
		// report is set FIRST, and the assertion is that it SURVIVED both.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;30\x07");
		reports.feed(b"\x1b]777;notify;Build;finished in 4s\x07");
		reports.feed(b"\x1b]99;i=1:d=0:p=title;Build finished\x07");
		assert_eq!(reports.current(), Progress::Working(30));
	}

	#[test]
	fn a_report_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks — including a split inside the number.
		let mut reports = Reports::default();
		reports.feed(b"working \x1b]9;4");
		reports.feed(b";1;6");
		reports.feed(b"5\x07 done");
		assert_eq!(reports.current(), Progress::Working(65));
	}

	#[test]
	fn the_st_terminator_works_as_well_as_bel() {
		assert_eq!(track(b"\x1b]9;4;1;5\x1b\\"), Progress::Working(5));
	}

	#[test]
	fn a_command_ending_takes_its_bar_with_it() {
		// The stale-bar case: the shell announces the command finished (§34's `D`), so the 60% it
		// was showing is no longer about anything.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;60\x07");
		reports.feed(b"\x1b]133;D;0\x07");
		assert_eq!(reports.current(), Progress::None);
	}

	#[test]
	fn a_new_command_reporting_in_the_same_chunk_survives_the_old_ones_ending() {
		// The ordering trap. One read off the wire can carry the end of one command, a fresh prompt,
		// and the first report of the next — all at once. Clearing after the chunk instead of in
		// stream order would wipe the NEW report and leave the tab looking idle while it works.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;60\x07");
		reports.feed(b"\x1b]133;D;0\x07\x1b]133;A\x07\x1b]9;4;3\x07");
		assert_eq!(reports.current(), Progress::Indeterminate);
	}

	#[test]
	fn a_resize_shaped_gap_between_reports_leaves_the_bar_alone() {
		// The counterpart to having no `clear` on the interface: nothing but a report in the stream
		// changes what is shown. Ordinary output between two reports — a resize's redraw, a wall of
		// build log — must not disturb the reading.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;60\x07");
		reports.feed(b"compiling thing v1.2.3\r\n\x1b[2J\x1b[H");
		assert_eq!(reports.current(), Progress::Working(60));
	}

	#[test]
	fn an_overlong_payload_is_dropped_not_buffered() {
		// A hostile stream must not grow our memory. Past the cap the payload is abandoned, and the
		// tracker keeps working for the next sequence.
		let mut reports = Reports::default();
		reports.feed(b"\x1b]9;4;1;");
		reports.feed(&[b'9'; MAX_PAYLOAD + 10]);
		reports.feed(b"\x07");
		assert_eq!(reports.current(), Progress::None);

		reports.feed(b"\x1b]9;4;1;20\x07");
		assert_eq!(reports.current(), Progress::Working(20));
	}
}

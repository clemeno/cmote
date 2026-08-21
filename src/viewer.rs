// viewer.rs — how far a viewer tab's file has been read (PLAN §121).
//
// The text editor (§32) and the picture preview (§53) are different models with different failure
// modes, but they wait on the SAME thing in the same way: one bounded read of one whole file, off
// the GUI thread, reported back by tab id. This is the sliver of state that wait needs, kept in one
// place so the two `Loading` statuses cannot drift apart on what "how far" means.
//
// It is deliberately tiny and deliberately NOT a status of its own. Each viewer keeps its own
// lifecycle enum, because what `Failed` and `Ready` mean differs between text and pictures; only the
// in-flight number is shared.

/// How much of a viewer's file has arrived (§121).
///
/// `total` is an `Option` because a server may decline to report a size, which the readers already
/// have to cope with — `ssh::edit::read_file` gates on the metadata when it can and re-checks the cap
/// as the buffer grows precisely because it sometimes cannot. That same "sometimes there is no size"
/// travels up here rather than being flattened into a fake 0 or 100.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LoadProgress {
	/// Bytes read so far.
	pub read: u64,
	/// The file's size, when the server or the filesystem gave one.
	pub total: Option<u64>,
}

impl LoadProgress {
	/// A read that has not reported yet: nothing read, no size known.
	pub const NOTHING_YET: Self = Self {
		read: 0,
		total: None,
	};

	/// The share read, 0..=100, or `None` when there is no size to be a share OF.
	///
	/// A `total` of 0 gives `None` rather than a division by zero: an empty file has no meaningful
	/// share, and it is also the one case that finishes before anything could be drawn.
	pub fn percent(self) -> Option<u8> {
		let total = self.total?;
		if total == 0 {
			return None;
		}
		// In `u128`, so the multiply is exact for every `u64` pair. `saturating_mul` was tried here
		// and is WRONG rather than merely conservative: it pins the numerator at `u64::MAX`, so half
		// of a very large file divides out to 1% instead of 50% — a plausible-looking number, which is
		// the worst kind. A test pins the case.
		//
		// The `min` is a real clamp, not defensive noise: a server that under-reports its own size,
		// or a file that grew between the stat and the read, would otherwise draw a bar past its
		// track. Done here, once, rather than at each drawing site.
		let share = u128::from(self.read) * 100 / u128::from(total);
		Some(u8::try_from(share.min(100)).expect("min(100) fits a u8 on the line above"))
	}

	/// What the tab strip draws for this read (§54, §121) — a real share when the size is known, the
	/// indeterminate pulse when it is not.
	///
	/// Reusing the OSC 9;4 progress type on purpose. A chip can already draw a bar along its bottom
	/// edge, the two states it needs are exactly the two this has, and a second bar-shaped thing on a
	/// 30-pixel chip would be a second set of colours and paddings to keep in step for no gain.
	pub fn as_progress(self) -> crate::term::progress::Progress {
		use crate::term::progress::Progress;
		match self.percent() {
			Some(share) => Progress::Working(share),
			None => Progress::Indeterminate,
		}
	}

	/// The bytes read out for a person, for the viewer's own body text — "3.1 MiB of 6.4 MiB", or
	/// just "3.1 MiB" when the size is unknown (§121).
	///
	/// `human::bytes` rather than a second spelling: §109 already paid for two copies of this
	/// formatter disagreeing about whether 1024 bytes are a `KB` or a `KiB`.
	pub fn label(self) -> String {
		let read = crate::human::bytes(self.read);
		match self.total {
			Some(total) => format!("{read} of {}", crate::human::bytes(total)),
			None => read,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::term::progress::Progress;

	/// The share is a share of a KNOWN size, and there are three ways for there not to be one: no
	/// metadata, an empty file, and a file that turned out bigger than its own stat said (§121).
	#[test]
	fn a_share_needs_a_size_to_be_a_share_of() {
		// Arrange / Act / Assert
		assert_eq!(
			LoadProgress {
				read: 512,
				total: Some(1024)
			}
			.percent(),
			Some(50),
			"half of a known size is 50%"
		);
		assert_eq!(
			LoadProgress {
				read: 512,
				total: None
			}
			.percent(),
			None,
			"a server that reports no size gives no share"
		);
		assert_eq!(
			LoadProgress {
				read: 0,
				total: Some(0)
			}
			.percent(),
			None,
			"an empty file has no share, and must not divide by zero"
		);
		assert_eq!(
			LoadProgress {
				read: 4096,
				total: Some(1024)
			}
			.percent(),
			Some(100),
			"a file bigger than its own stat is clamped, not drawn past the track"
		);
	}

	/// `read * 100` must not overflow, and the fix for that must not LIE (§121).
	///
	/// This test was written asserting `Some(100)` against a `saturating_mul`, and failed with
	/// `Some(1)` — the saturation pinned the numerator at `u64::MAX`, so half a file divided out to
	/// one percent. A too-low share that looks like a real number is worse than a panic, so the
	/// arithmetic moved to `u128` and this test pins the honest answer.
	#[test]
	fn a_size_near_the_top_of_a_u64_still_divides_honestly() {
		// Arrange — not real files; the arithmetic is the point. Both `read * 100` products are far
		// past `u64::MAX`, so both would have gone through the saturation and come out at 1%.
		let exact = LoadProgress {
			read: 1 << 62,
			total: Some(1 << 63),
		};
		let ragged = LoadProgress {
			read: u64::MAX / 2,
			total: Some(u64::MAX),
		};
		// Act / Assert
		assert_eq!(
			exact.percent(),
			Some(50),
			"half a file is half a file however big the numbers are"
		);
		// 49, not 50: `u64::MAX / 2` is 2^63 - 1, which is a hair UNDER half of 2^64 - 1, and the
		// division floors. Pinned deliberately — the first version of this test asserted the rounded
		// ideal and was simply wrong about what integer division does.
		assert_eq!(
			ragged.percent(),
			Some(49),
			"a share floors; it does not round to the nicer number"
		);
	}

	/// The two states the chip can draw, and which one an unknown size picks (§54, §121).
	#[test]
	fn an_unknown_size_draws_the_indeterminate_bar() {
		// Arrange / Act / Assert
		assert_eq!(
			LoadProgress {
				read: 300,
				total: Some(1200)
			}
			.as_progress(),
			Progress::Working(25)
		);
		assert_eq!(
			LoadProgress {
				read: 300,
				total: None
			}
			.as_progress(),
			Progress::Indeterminate,
			"no size means the pulse, not an empty bar"
		);
	}

	/// The body's label names both numbers when both are known, and does not invent the one it does
	/// not have (§121).
	#[test]
	fn the_label_omits_a_total_it_was_never_given() {
		// Arrange / Act / Assert
		assert_eq!(
			LoadProgress {
				read: 1536,
				total: Some(4096)
			}
			.label(),
			"1.5 KiB of 4.0 KiB"
		);
		assert_eq!(
			LoadProgress {
				read: 1536,
				total: None
			}
			.label(),
			"1.5 KiB",
			"an unknown total must not be spelled as anything"
		);
	}

	/// The starting value says "nothing yet" in both fields, so a tab that has just asked draws the
	/// pulse rather than a 0% bar (§121).
	#[test]
	fn a_read_that_has_not_reported_draws_the_pulse() {
		// Arrange / Act / Assert
		assert_eq!(LoadProgress::NOTHING_YET.percent(), None);
		assert_eq!(
			LoadProgress::NOTHING_YET.as_progress(),
			Progress::Indeterminate
		);
	}
}

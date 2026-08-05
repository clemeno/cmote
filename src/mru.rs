// mru.rs — which tab was on screen when, so closing one falls back to where the user just was
// (PLAN §37).
//
// The tab strip has an order of its own — left to right, the order the tabs were opened in (§26) —
// but that order says nothing about where the user has BEEN. Closing the active tab with strip
// arithmetic ("keep the index, or step back one") hands the window to whichever chip happens to sit
// next door, which is very often a tab the user has never even looked at. This module holds the
// other order: the ids of the open tabs, least recently activated first, the tab on screen last.
// Closing a tab pops it out of that order and names the one that should come forward.
//
// It is deliberately pure — ids, and nothing else. It knows no `Tab`, no index into the strip and no
// iced type, so the whole rule ("the tab you were on before is the tab you get back") is testable
// without a window, and `App` stays the only place that has to reconcile ids with strip positions.
//
// The list is a *stack of visits*, not a log of them: re-activating a tab moves it to the top rather
// than appending a second entry, so an id can never sit in the order twice and the length always
// matches the number of open tabs.

/// The activation order of the open tabs (§37): tab ids, least recently activated first, the tab on
/// screen last. Holds every open tab exactly once — no duplicates, and no tab that has been closed.
#[derive(Debug)]
pub struct Mru {
	/// The visits, oldest first. A plain `Vec` rather than a map or a deque, on purpose: a window
	/// holds a handful of tabs, so a linear scan beats any index for both speed and clarity, and the
	/// vector reads as the order it is when printed.
	order: Vec<u64>,
}

impl Mru {
	/// Start the order with the tab that is already on screen — at startup, cmote's first home tab.
	/// There is always an active tab, so the order is never constructed empty.
	pub fn new(active: u64) -> Self {
		Self {
			order: vec![active],
		}
	}

	/// Record that tab `id` has just become the active one: it goes to the top of the order. An id
	/// already present is MOVED rather than duplicated, so revisiting a tab re-dates its visit
	/// instead of leaving a stale entry further down that could later come forward out of turn.
	pub fn touch(&mut self, id: u64) {
		self.order.retain(|&open| open != id);
		self.order.push(id);
	}

	/// Drop tab `id` from the order — it has just been closed — and name the tab that should come
	/// forward: the most recently activated of those left, or `None` once the last tab has gone.
	///
	/// Returning the *top* rather than "the tab before the closed one" is what lets one rule cover
	/// both close cases. Closing the ACTIVE tab pops the top, so the answer is the tab the user was
	/// on before it — the point of the whole module. Closing a BACKGROUND tab leaves the top where it
	/// is, so the answer is the active tab itself, and the caller re-activating what is already on
	/// screen is a no-op. An id that is not in the order (nothing should reach here twice, but a
	/// caller cannot be made to prove it) leaves the order untouched and still gets a sound answer.
	pub fn forget(&mut self, id: u64) -> Option<u64> {
		self.order.retain(|&open| open != id);
		self.order.last().copied()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// The order is private, so these tests read it the way `App` does: by closing tabs one at a time
	// and checking which one is handed back. Draining the stack that way pins the whole order, not
	// just its top, without opening the field up to a getter no caller needs.

	#[test]
	fn the_tab_on_screen_at_startup_is_the_first_visit() {
		let mut mru = Mru::new(7);
		// Nothing else has been visited, so closing it leaves no tab to come forward.
		assert_eq!(mru.forget(7), None);
	}

	#[test]
	fn closing_the_active_tab_hands_back_the_previous_visit() {
		let mut mru = Mru::new(1);
		mru.touch(2);
		mru.touch(3);
		// 3 is on screen; the user was on 2 before it, and on 1 before that. Closing walks back along
		// that trail rather than along the strip.
		assert_eq!(mru.forget(3), Some(2));
		assert_eq!(mru.forget(2), Some(1));
		assert_eq!(mru.forget(1), None);
	}

	#[test]
	fn revisiting_a_tab_re_dates_it_instead_of_duplicating_it() {
		let mut mru = Mru::new(1);
		mru.touch(2);
		mru.touch(3);
		// Back to 1, so the trail is now 2, 3, 1 — the old entry for 1 must not still be sitting at
		// the bottom, or closing everything would ask for 1 twice and never reach 2.
		mru.touch(1);
		assert_eq!(mru.forget(1), Some(3));
		assert_eq!(mru.forget(3), Some(2));
		assert_eq!(mru.forget(2), None);
	}

	#[test]
	fn closing_a_background_tab_names_the_tab_already_on_screen() {
		let mut mru = Mru::new(1);
		mru.touch(2);
		// 2 is active; closing 1 from its own "×" must not move the window — the answer is 2, which
		// the caller is already showing.
		assert_eq!(mru.forget(1), Some(2));
	}

	#[test]
	fn a_closed_tab_never_comes_forward_later() {
		let mut mru = Mru::new(1);
		mru.touch(2);
		mru.touch(3);
		// Close the middle visit while 3 is on screen: 3 stays.
		assert_eq!(mru.forget(2), Some(3));
		// Now close the active tab. 2 is gone for good, so the fallback skips it to 1.
		assert_eq!(mru.forget(3), Some(1));
	}

	#[test]
	fn forgetting_an_unknown_id_leaves_the_order_alone() {
		let mut mru = Mru::new(1);
		mru.touch(2);
		// A close for a tab that was never in the order (or was already dropped) must not disturb it.
		assert_eq!(mru.forget(99), Some(2));
		assert_eq!(mru.forget(2), Some(1));
	}
}

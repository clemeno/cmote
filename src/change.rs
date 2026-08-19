// change.rs — a value that may be absent for two different reasons (PLAN §111).
//
// Two places in cmote had written this as `Option<Option<T>>` and then spent eight lines of doc
// comment explaining which nesting level meant what: `targets::SessionState`'s remembered sort (§22)
// and `term::iterm`'s user variable (§55). Both needed the SAME three answers, and the outer/inner
// spelling gets them backwards as easily as right — `Some(None)` and `None` sit one keystroke apart
// and mean opposite things, and nothing but the prose says which is which.
//
// So the three answers get names. A `match` on this is exhaustive, a missing case is a compile error,
// and the reader never has to count `Option`s.

/// What a source said about one value: nothing, that it is empty, or what it is.
///
/// The distinction between `Keep` and `Clear` is the whole point of the type. A source that says
/// nothing must leave what is already held alone; a source that says "empty" is making a positive
/// statement that the value is now unset. Collapsing the two loses a real answer — a pane whose sort
/// the user just cleared would be restored with the sort it had two runs ago, and a shell leaving a
/// git repository would leave its last branch on screen forever (§55).
///
/// `Keep` is the default, so a snapshot built field-by-field says nothing until it is told to — the
/// safe direction, since the alternative default would erase stored values nobody asked to erase.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Change<T> {
	/// Nothing was said. Leave whatever is held.
	#[default]
	Keep,
	/// Said to be empty. Unset whatever is held.
	Clear,
	/// Said to be this.
	Set(T),
}

impl<T> Change<T> {
	/// What a source that DEFINITELY knows is reporting: a value, or the absence of one.
	///
	/// This is the common case for cmote's own snapshots — a pane always knows its own sort, even
	/// when that sort is unset — so `Keep` is never what they mean. `Keep` exists for sources that
	/// may genuinely have nothing to say, like an OSC payload that turned out not to be an
	/// assignment at all.
	pub fn reported(value: Option<T>) -> Self {
		match value {
			Some(value) => Change::Set(value),
			None => Change::Clear,
		}
	}
}

impl<T: PartialEq> Change<T> {
	/// Apply this to a stored value, and say whether it actually changed anything.
	///
	/// The return value is what the callers need: `targets::Targets::set_session` folds a whole
	/// snapshot in and saves only if some part of it differed, so every field has to answer the same
	/// question. A `Set` that matches what is already there reports `false` — it is not a change.
	pub fn fold_into(self, stored: &mut Option<T>) -> bool {
		let wanted = match self {
			Change::Keep => return false,
			Change::Clear => None,
			Change::Set(value) => Some(value),
		};
		if *stored == wanted {
			return false;
		}
		*stored = wanted;
		true
	}
}

#[cfg(test)]
mod tests {
	use super::Change;

	#[test]
	fn keep_leaves_the_stored_value_and_reports_no_change() {
		let mut stored = Some(7);
		assert!(!Change::Keep.fold_into(&mut stored));
		assert_eq!(stored, Some(7), "silence must not overwrite");
	}

	#[test]
	fn clear_unsets_a_stored_value_and_is_not_the_same_as_keep() {
		// The distinction the type exists for: both leave the field empty-looking from the outside,
		// but only one of them is allowed to erase what an earlier run recorded.
		let mut cleared = Some(7);
		assert!(Change::<i32>::Clear.fold_into(&mut cleared));
		assert_eq!(cleared, None);

		let mut kept = Some(7);
		assert!(!Change::Keep.fold_into(&mut kept));
		assert_eq!(kept, Some(7));
	}

	#[test]
	fn clearing_what_is_already_empty_is_not_a_change() {
		let mut stored: Option<i32> = None;
		assert!(!Change::Clear.fold_into(&mut stored));
	}

	#[test]
	fn set_writes_the_value_and_only_reports_a_real_difference() {
		let mut stored = Some(7);
		assert!(
			!Change::Set(7).fold_into(&mut stored),
			"the same value again"
		);
		assert!(Change::Set(8).fold_into(&mut stored));
		assert_eq!(stored, Some(8));

		let mut empty = None;
		assert!(Change::Set(1).fold_into(&mut empty));
		assert_eq!(empty, Some(1));
	}

	#[test]
	fn a_source_that_knows_never_reports_keep() {
		// `reported` is for snapshots that always have an answer, so its absence means "unset",
		// never "no comment".
		assert_eq!(Change::reported(Some(3)), Change::Set(3));
		assert_eq!(Change::reported(None::<i32>), Change::Clear);
	}
}

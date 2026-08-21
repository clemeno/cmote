// ui/home.rs — the home screen: a list of saved connection targets (PLAN §14).
//
// This is the landing screen (§10 state machine). It lists the saved targets
// (`targets::Target`) sorted alphabetically by name, and lets the user:
//   * narrow the list — the filter box above it keeps only the rows matching what is typed
//     (§49): a fragment while there is no wildcard in it, a whole-row glob once `*` or `?`
//     appears. Everything below the filter works off the rows it left, not off the whole list;
//   * pick one — pre-fills the connect form with its host / port / user / auth so a
//     return visit is one click (the secret is still entered on the form, §12);
//   * rename one — F2 on the selected row, or right-click → Rename; the row becomes
//     an inline text field, and committing re-sorts the list by the new name;
//   * open / delete one — from the same right-click menu; deleting is not undoable, so
//     it goes through a confirmation dialog (Cancel, ✕, the backdrop and Esc all keep
//     the target) rather than removing the row on the click;
//   * start a fresh connection — the "New connection" button opens a blank form.
//
// The view is pure (state in, `Element` out); every action is a `Message` handled
// in `app::update`. The right-click menu reuses the terminal's overlay pattern (a
// floating panel plus a full-window dismiss layer), positioned by the selected
// row's index rather than the raw cursor — see `context_menu`.

use iced::alignment::Vertical;
use iced::widget::{
	button, column, container, mouse_area, row, scrollable, stack, text, text_editor, text_input,
};
use iced::{Border, Element, Length, Padding, Theme};

use crate::app::Message;
use crate::targets::Target;
use crate::ui::{dialog, menu};

/// The widget id of the inline rename field, so `app` can focus it the instant a
/// rename starts (the user types straight away, no click needed — like the passphrase
/// prompt, §7).
pub const RENAME_INPUT_ID: &str = "home-rename";

/// The widget id of the filter box (§49), so Ctrl+F can put the cursor in it without the user
/// reaching for the mouse — the same trick the rename field and the find bar use.
pub const FILTER_INPUT_ID: &str = "home-filter";

/// The body copy for the delete confirmation (§14). `app` appends the target being
/// deleted (its name and endpoint) and seeds the whole thing into the dialog buffer, so
/// the user confirms against the row they actually picked.
pub const DELETE_DIALOG_BODY: &str = "Removes this target from the saved list. Nothing on the server changes and its host key stays trusted — only the saved target is forgotten. This cannot be undone.";

// This screen has no hard-coded colours on purpose. The app sets no theme, so iced
// follows the system light/dark preference — a fixed light palette here put dark-mode's
// near-white text on a light background and made the list unreadable. Every colour comes
// from the active theme instead: `text::secondary` for muted text, `container::
// bordered_box` / `button::text` for the menu, and the palette's `primary.weak` pair for
// the selected row (its `text` is guaranteed readable on its `color`).

// The list geometry. Each row is a FIXED height so the right-click menu can be placed
// from the selected row's index without measuring the laid-out widgets (iced does not
// expose that). `LIST_TOP` is the approximate y where the first row starts, below the
// header, the hint and the filter box. `ponytail:` these are eyeballed to the current layout;
// the menu can sit a few pixels off if the header wraps, and assumes the list is scrolled to
// the top. The filter box (§49) is what pushed `LIST_TOP` down from 108: it is one text input
// (its own padding plus a line of text) and the column's spacing above it.
const ROW_HEIGHT: f32 = 34.0;
const LIST_TOP: f32 = 151.0;
const MENU_LEFT: f32 = 40.0;

/// How much further down the list starts when the Local bar is on screen (§103) — the bar's own
/// height (a button plus its padding, inside a bordered container) plus the column's spacing above
/// it. Added to `LIST_TOP` rather than folded into it, because the bar is not always there: a machine
/// where cmote found no shell to offer shows no bar, and the menu must still land beside the right
/// row there.
const LOCAL_BAR_HEIGHT: f32 = 54.0;

/// The current inline-rename edit: which target (by endpoint key) is being renamed and
/// the text typed so far. Held by `app`; `None` when no rename is in progress.
#[derive(Debug, Clone, Default)]
pub struct RenameState {
	pub key: String,
	pub text: String,
}

/// Everything the home screen shows besides the targets themselves, in one struct rather than
/// a row of positional arguments: `filter` is what is typed in the filter box (§49), `selected`
/// the endpoint key of the highlighted row, `rename` the in-progress inline edit, `menu_open`
/// the right-click menu anchored at the selected row, and `confirm_delete` the delete
/// confirmation over everything, with `dialog_body` as its selectable message and `card` its
/// floating position. Named fields also mean the call site says which flag is which — with seven of them
/// in a row, two `bool`s next to each other are a silent bug waiting for the day their order is
/// mistyped.
pub struct View<'a> {
	pub filter: &'a str,
	pub selected: Option<&'a str>,
	pub rename: Option<&'a RenameState>,
	pub menu_open: bool,
	pub confirm_delete: bool,
	pub dialog_body: &'a text_editor::Content,
	pub card: dialog::Card,
	/// The local shells this machine can open (§103), for the Local bar above the list. `'static`
	/// because the catalogue is searched once per run and kept (`local::shells::catalogue`) — a search
	/// per frame would put a dozen filesystem probes in the paint loop. Empty on a machine where none
	/// was found, and then no bar is drawn at all.
	pub shells: &'static [crate::local::shells::LocalShell],
	/// Why the saved store was not read, when it was not (§110). An empty list means one of three
	/// things now, and they must not read alike: nothing saved yet, nothing matching the filter, or
	/// a store this build is too old to open. Only the third is the user's data still being there.
	/// Owned rather than borrowed: the store is read through a short-lived `RefCell` borrow, and a
	/// `&str` from it would tie this view's return value to that temporary. It allocates only in the
	/// refusal case, which is the one frame where an allocation is not the interesting cost.
	pub refusal: Option<String>,
}

/// Render the home screen. `targets` are already in display order (`targets` keeps
/// them sorted); `state` is everything else on the screen (see `View`).
pub fn view<'a>(
	// `targets` has its OWN lifetime, not `'a`: every row clones the names it shows (see
	// `target_row`), so nothing in the returned element borrows the list. That lets `app` pass a
	// short-lived borrow of the shared, `RefCell`-guarded target list (§26).
	targets: &[Target],
	state: View<'a>,
) -> Element<'a, Message> {
	let header = row![
		text("cmote — targets").size(24).width(Length::Fill),
		button(text("New connection")).on_press(Message::HomeNewPressed),
	]
	.spacing(12)
	.align_y(Vertical::Center);

	// A one-line hint so the (deliberately terse) interactions are discoverable.
	let hint = text(
		"Click to select · click again or Enter to open · F2 or right-click to rename · Ctrl+F to filter",
	)
	.size(12)
	.style(text::secondary);

	// The rows the filter leaves on screen (§49). Everything below this line works off `shown`
	// and not off `targets` — the list, the count, and the row index the context menu is placed
	// by — so the menu lands beside the row the user is actually looking at rather than beside
	// wherever that target sits in the unfiltered list.
	let shown: Vec<&Target> = targets
		.iter()
		.filter(|target| target.matches(state.filter))
		.collect();

	// The Local bar (§103), above the filter rather than inside the list: it is not a saved target
	// and must not be filtered away by a pattern typed to find one.
	let mut page = column![header, hint].spacing(12);
	if !state.shells.is_empty() {
		page = page.push(local_bar(state.shells));
	}
	let base: Element<'a, Message> = page
		.push(filter_bar(state.filter, shown.len(), targets.len()))
		.push(target_list(
			&shown,
			!state.filter.is_empty(),
			state.selected,
			state.rename,
			state.refusal,
		))
		.padding(20)
		.into();

	// The menu only shows for a real selection; find that row's index to place it. If
	// the selection has somehow gone stale, fall back to just the base view.
	let menu_index = state
		.menu_open
		.then(|| state.selected.and_then(|key| index_of(&shown, key)))
		.flatten();

	let mut layers: Vec<Element<'a, Message>> = vec![base];
	if let Some(index) = menu_index {
		layers.push(menu::dismiss_layer(Message::HomeMenuDismissed));
		layers.push(context_menu(index, state.shells.is_empty()));
	}
	// Deleting a target cannot be undone, so it goes through the same confirmation
	// chrome as Disconnect (§10) rather than acting on the menu click. The list stays
	// visible (dimmed) behind the card, so the row being removed is still in view.
	if state.confirm_delete {
		layers.push(dialog::backdrop(Message::HomeDeleteCancelled));
		layers.push(confirm_delete_panel(state.dialog_body, state.card));
	}

	// One stack, always — even with nothing over the list. iced keys a widget's internal
	// state (here the target list's scroll offset) to its position in the widget tree,
	// and `Tree::diff` throws the whole subtree away when the root's type changes. So
	// swapping between the bare list and a `stack` reset the scroll every time a menu or
	// the delete prompt opened. Layers are only ever appended, so the list stays at
	// index 0 and keeps its state (§10, §14).
	stack(layers)
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
}

/// The delete confirmation modal (§14), in the shared dialog chrome: the question in
/// the header, what deleting does plus which target it hits in the body, Cancel /
/// Delete in the footer. Every dismissal route — Cancel, the header's ✕, a click on the
/// backdrop, Esc — emits `HomeDeleteCancelled`, so backing out always keeps the target;
/// only the Delete button removes it.
fn confirm_delete_panel(
	dialog_body: &text_editor::Content,
	card: dialog::Card,
) -> Element<'_, Message> {
	dialog::dialog(
		"Delete this target?".to_owned(),
		Message::HomeDeleteCancelled,
		dialog::selectable_body(dialog_body),
		vec![
			button("Cancel")
				.on_press(Message::HomeDeleteCancelled)
				.into(),
			button("Delete")
				.on_press(Message::HomeDeleteConfirmed)
				.into(),
		],
		card,
	)
}

/// The Local bar (§103): one button per shell cmote found on this machine, each opening a session on
/// THIS machine — a terminal running that shell, with the local filesystem in the folder tree and the
/// files pane beside it.
///
/// It sits above the filter and outside the list on purpose. A local shell is not a saved target: it
/// has no host, no account and nothing to remember, so it cannot be renamed, deleted or filtered, and
/// putting it among the rows would invite all four. A bar of its own says "these are the other kind of
/// thing you can open here", which is what it is.
///
/// The buttons are the catalogue in its own order, and the catalogue holds only what is really
/// installed — so a button on screen can always be pressed. Nothing is shown disabled: a greyed
/// "Git Bash" on a machine without Git teaches the user nothing they can act on.
fn local_bar(shells: &'static [crate::local::shells::LocalShell]) -> Element<'static, Message> {
	let mut bar = row![
		text("Local")
			.size(12)
			.style(text::secondary)
			.width(Length::Fixed(48.0)),
	]
	.spacing(8)
	.align_y(Vertical::Center);

	for shell in shells {
		// The `Shell` is cloned into the message rather than being looked up again on the press: it
		// carries the program path cmote resolved, so nothing re-searches the disk — and nothing the
		// user can type ever becomes a program to run (`local::shells`).
		bar = bar.push(
			button(text(shell.kind.label()).size(12))
				.on_press(Message::HomeLocalPressed(shell.clone())),
		);
	}

	container(bar)
		.width(Length::Fill)
		.padding(Padding::from([8.0, 10.0]))
		.style(container::bordered_box)
		.into()
}

/// The filter box above the list (§49), with a `shown of total` tally beside it once something
/// is typed — the tally is how the user knows the list is short because it was filtered and not
/// because the targets are gone.
///
/// The field deliberately has **no `on_submit`**. iced's text input only captures Enter when it
/// has a submit message, and the home screen's key handler only ever sees the keys no widget
/// captured — so leaving it off is what lets Enter fall through to the list and open the
/// selected target while the cursor is still in the box. Type, arrow nothing, press Enter,
/// connect. Backspace and Delete go the other way: the focused field captures those, so the
/// Delete key cannot reach the list and raise a delete prompt while a pattern is being edited.
fn filter_bar(filter: &str, shown: usize, total: usize) -> Element<'_, Message> {
	let field = text_input(
		"Filter targets — a fragment, or a glob with * and ?",
		filter,
	)
	.id(FILTER_INPUT_ID)
	.on_input(Message::HomeFilterEdited)
	.width(Length::Fill);

	let mut bar = row![field].spacing(10).align_y(Vertical::Center);
	if !filter.is_empty() {
		bar = bar.push(
			text(format!("{shown} of {total}"))
				.size(12)
				.style(text::secondary),
		);
	}
	bar.into()
}

/// The scrollable list of target rows, or an empty-state hint when there are none. `filtering`
/// says whether the box above holds a pattern, which is the difference between the two ways the
/// list can be empty: nothing saved yet, or nothing matching — the first is answered by
/// connecting somewhere, the second by editing the pattern, so they must not read alike.
fn target_list<'a>(
	// Fresh lifetime, like `view` — the rows clone what they show, so the list is not borrowed
	// into the returned element (§26).
	targets: &[&Target],
	filtering: bool,
	selected: Option<&str>,
	rename: Option<&'a RenameState>,
	refusal: Option<String>,
) -> Element<'a, Message> {
	if targets.is_empty() {
		// A refusal is checked FIRST and worded as itself: the store is on disk and intact, and
		// telling this user to add a connection would invite them to write a new file over it (§110).
		if let Some(reason) = refusal {
			return text(reason).style(text::danger).into();
		}
		let empty = if filtering {
			"No target matches this filter."
		} else {
			"No saved targets yet — “New connection” to add one."
		};
		return text(empty).style(text::secondary).into();
	}

	let rows = targets.iter().map(|target| {
		let key = target.endpoint();
		let is_selected = selected == Some(key.as_str());
		// A row being renamed shows its edit field instead of the label.
		match rename {
			Some(state) if state.key == key => rename_row(&state.text),
			_ => target_row(target, key, is_selected),
		}
	});

	let bar = scrollable(column(rows).spacing(2))
		// The terminal's own bar, not iced's default (§118) — one scrollbar look per window.
		.direction(scrollable::Direction::Vertical(crate::ui::scrollbar::bar()))
		.style(crate::ui::scrollbar::style)
		.height(Length::Fill);
	// And the terminal's own hand over it (§120) — the cursor half of one shared component.
	crate::ui::scrollbar::grabbable(
		bar,
		crate::ui::scrollbar::Axes::VERTICAL,
		crate::cursor::SCROLLBAR_HOME,
	)
}

/// One target row: the name (filling the width) with the endpoint in muted grey after
/// it. Wrapped in a `mouse_area` so a left click selects it and a right click opens the
/// context menu (both carry the endpoint key). Fixed height so the menu placement math
/// (see `context_menu`) lines up.
fn target_row(target: &Target, key: String, selected: bool) -> Element<'static, Message> {
	// The endpoint is muted grey — but ONLY on an unselected row. `text::secondary`
	// pins an absolute colour (`secondary.base.color`), which ignores the selected row's
	// tint and stays dark grey on it. On the selected row the style is left at its
	// default (`color: None`) so the text inherits the container's `text_color` — the
	// half of the palette pair guaranteed readable on that tint.
	let endpoint = move |theme: &Theme| {
		if selected {
			text::Style::default()
		} else {
			text::secondary(theme)
		}
	};

	let label = row![
		text(target.name.clone()).width(Length::Fill),
		text(key.clone()).size(12).style(endpoint),
	]
	.spacing(10)
	.align_y(Vertical::Center);

	let cell = container(label)
		.width(Length::Fill)
		.height(Length::Fixed(ROW_HEIGHT))
		.padding(Padding::from([0.0, 8.0]))
		.align_y(Vertical::Center)
		.style(move |theme: &Theme| {
			let mut style = container::Style {
				border: Border {
					radius: 4.0.into(),
					..Border::default()
				},
				..container::Style::default()
			};
			if selected {
				// The palette pair carries both halves, so the label stays readable on
				// the tint in light *and* dark themes.
				let pair = theme.extended_palette().primary.weak;
				style.background = Some(pair.color.into());
				style.text_color = Some(pair.text);
			}
			style
		});

	mouse_area(cell)
		.on_press(Message::HomeTargetClicked(key.clone()))
		.on_right_press(Message::HomeTargetRightClicked(key))
		.into()
}

/// A row in rename mode: an inline text field pre-filled with the current name. Enter
/// commits (via `on_submit`), Esc cancels (handled in `app` off the keyboard listener).
/// Focused by `app` the moment the rename starts.
fn rename_row(value: &str) -> Element<'_, Message> {
	container(
		text_input("Name", value)
			.id(RENAME_INPUT_ID)
			.on_input(Message::HomeRenameEdited)
			.on_submit(Message::HomeRenameCommitted),
	)
	.height(Length::Fixed(ROW_HEIGHT))
	.padding(Padding::from([0.0, 8.0]))
	.align_y(Vertical::Center)
	.into()
}

/// The right-click context menu (§14): Open / Rename / Delete for the selected target,
/// in the shared menu chrome (`ui::menu`, §10) so it matches the terminal's and the folder
/// tree's. It is anchored just below the selected row, whose y is derived from its `index`
/// and the fixed `ROW_HEIGHT` (iced does not expose the laid-out position, so we compute
/// it). `ponytail:` no scroll-offset or edge clamping — fine for the short lists this
/// screen holds.
///
/// `no_local_bar` is the one piece of layout it cannot assume: the Local bar (§103) pushes every row
/// down by its own height, and it is absent on a machine where cmote found no shell to offer. Passed
/// as a flag rather than read here, so the one place that decides whether the bar is drawn is also the
/// one place that accounts for it.
fn context_menu(index: usize, no_local_bar: bool) -> Element<'static, Message> {
	let panel = menu::panel(vec![
		menu::item("Open".to_owned(), Some(Message::HomeMenuOpen)),
		menu::item("Rename".to_owned(), Some(Message::HomeMenuRename)),
		menu::item("Delete".to_owned(), Some(Message::HomeMenuDelete)),
	]);

	let list_top = if no_local_bar {
		LIST_TOP
	} else {
		LIST_TOP + LOCAL_BAR_HEIGHT
	};
	let top = list_top + super::pixels(index, ROW_HEIGHT) + ROW_HEIGHT;
	container(panel)
		.width(Length::Fill)
		.height(Length::Fill)
		.padding(Padding {
			top,
			right: 0.0,
			bottom: 0.0,
			left: MENU_LEFT,
		})
		.into()
}

/// The index of the target with this endpoint key among the rows ON SCREEN, used to place the
/// context menu against the right row. A filtered-out selection is simply not found, and the
/// menu is not drawn — which is the right answer, since there is no row for it to point at.
fn index_of(targets: &[&Target], key: &str) -> Option<usize> {
	targets.iter().position(|target| target.endpoint() == key)
}

// ui/home.rs — the home screen: a list of saved connection targets (PLAN §14).
//
// This is the landing screen (§10 state machine). It lists the saved targets
// (`profiles::Target`) sorted alphabetically by name, and lets the user:
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
use crate::profiles::Target;
use crate::ui::dialog;

/// The widget id of the inline rename field, so `app` can focus it the instant a
/// rename starts (the user types straight away, no click needed — like the passphrase
/// prompt, §7).
pub const RENAME_INPUT_ID: &str = "home-rename";

/// The body copy for the delete confirmation (§14). `app` appends the target being
/// deleted (its name and endpoint) and seeds the whole thing into the dialog buffer, so
/// the user confirms against the row they actually picked.
pub const DELETE_DIALOG_BODY: &str = "Removes this target from the saved list. Nothing on the server changes and its host key stays trusted — only the saved profile is forgotten. This cannot be undone.";

// This screen has no hard-coded colours on purpose. The app sets no theme, so iced
// follows the system light/dark preference — a fixed light palette here put dark-mode's
// near-white text on a light background and made the list unreadable. Every colour comes
// from the active theme instead: `text::secondary` for muted text, `container::
// bordered_box` / `button::text` for the menu, and the palette's `primary.weak` pair for
// the selected row (its `text` is guaranteed readable on its `color`).

// The list geometry. Each row is a FIXED height so the right-click menu can be placed
// from the selected row's index without measuring the laid-out widgets (iced does not
// expose that). `LIST_TOP` is the approximate y where the first row starts, below the
// header. `ponytail:` these are eyeballed to the current layout; the menu can sit a few
// pixels off if the header wraps, and assumes the list is scrolled to the top.
const ROW_HEIGHT: f32 = 34.0;
const LIST_TOP: f32 = 108.0;
const MENU_LEFT: f32 = 40.0;

/// The current inline-rename edit: which target (by endpoint key) is being renamed and
/// the text typed so far. Held by `app`; `None` when no rename is in progress.
#[derive(Debug, Clone, Default)]
pub struct RenameState {
	pub key: String,
	pub text: String,
}

/// Render the home screen. `targets` are already in display order (`profiles` keeps
/// them sorted); `selected` is the endpoint key of the highlighted row, if any;
/// `rename` is the in-progress inline edit, if any; `menu_open` shows the right-click
/// menu anchored at the selected row; `confirm_delete` overlays the delete confirmation
/// on top of everything, with `dialog_body` as its selectable message and `drag` its
/// position.
pub fn view<'a>(
	targets: &'a [Target],
	selected: Option<&str>,
	rename: Option<&'a RenameState>,
	menu_open: bool,
	confirm_delete: bool,
	dialog_body: &'a text_editor::Content,
	drag: dialog::Drag,
) -> Element<'a, Message> {
	let header = row![
		text("cmote — targets").size(24).width(Length::Fill),
		button(text("New connection")).on_press(Message::HomeNewPressed),
	]
	.spacing(12)
	.align_y(Vertical::Center);

	// A one-line hint so the (deliberately terse) interactions are discoverable.
	let hint = text("Click to select · click again or Enter to open · F2 or right-click to rename")
		.size(12)
		.style(text::secondary);

	let base: Element<'a, Message> = column![header, hint, target_list(targets, selected, rename)]
		.spacing(12)
		.padding(20)
		.into();

	// The menu only shows for a real selection; find that row's index to place it. If
	// the selection has somehow gone stale, fall back to just the base view.
	let menu_index = menu_open
		.then(|| selected.and_then(|key| index_of(targets, key)))
		.flatten();

	let screen: Element<'a, Message> = match menu_index {
		Some(index) => stack![base, dismiss_layer(), context_menu(index)]
			.width(Length::Fill)
			.height(Length::Fill)
			.into(),
		None => base,
	};

	// Deleting a target cannot be undone, so it goes through the same confirmation
	// chrome as Disconnect (§10) rather than acting on the menu click. The list stays
	// visible (dimmed) behind the card, so the row being removed is still in view.
	if confirm_delete {
		stack![
			screen,
			dialog::backdrop(Message::HomeDeleteCancelled),
			confirm_delete_panel(dialog_body, drag),
		]
		.width(Length::Fill)
		.height(Length::Fill)
		.into()
	} else {
		screen
	}
}

/// The delete confirmation modal (§14), in the shared dialog chrome: the question in
/// the header, what deleting does plus which target it hits in the body, Cancel /
/// Delete in the footer. Every dismissal route — Cancel, the header's ✕, a click on the
/// backdrop, Esc — emits `HomeDeleteCancelled`, so backing out always keeps the target;
/// only the Delete button removes it.
fn confirm_delete_panel(
	dialog_body: &text_editor::Content,
	drag: dialog::Drag,
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
		drag,
	)
}

/// The scrollable list of target rows, or an empty-state hint when there are none.
fn target_list<'a>(
	targets: &'a [Target],
	selected: Option<&str>,
	rename: Option<&'a RenameState>,
) -> Element<'a, Message> {
	if targets.is_empty() {
		return text("No saved targets yet — “New connection” to add one.")
			.style(text::secondary)
			.into();
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

	scrollable(column(rows).spacing(2))
		.height(Length::Fill)
		.into()
}

/// One target row: the name (filling the width) with the endpoint in muted grey after
/// it. Wrapped in a `mouse_area` so a left click selects it and a right click opens the
/// context menu (both carry the endpoint key). Fixed height so the menu placement math
/// (see `context_menu`) lines up.
fn target_row(target: &Target, key: String, selected: bool) -> Element<'_, Message> {
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
/// as a small floating panel. It is anchored just below the selected row, whose y is
/// derived from its `index` and the fixed `ROW_HEIGHT` (iced does not expose the laid-out
/// position, so we compute it). `ponytail:` no scroll-offset or edge clamping — fine for
/// the short lists this screen holds.
fn context_menu(index: usize) -> Element<'static, Message> {
	let item = |label: &'static str, message: Message| {
		button(text(label).size(14))
			.width(Length::Fill)
			.on_press(message)
			.style(button::text)
	};

	let panel = container(
		column![
			item("Open", Message::HomeMenuOpen),
			item("Rename", Message::HomeMenuRename),
			item("Delete", Message::HomeMenuDelete),
		]
		.spacing(2),
	)
	.width(Length::Fixed(140.0))
	.padding(4)
	.style(container::bordered_box);

	let top = LIST_TOP + (index as f32) * ROW_HEIGHT + ROW_HEIGHT;
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

/// A full-window invisible layer under the menu: any click that misses the menu lands
/// here and dismisses it (mirrors the terminal's context menu, §10).
fn dismiss_layer() -> Element<'static, Message> {
	mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
		.on_press(Message::HomeMenuDismissed)
		.on_right_press(Message::HomeMenuDismissed)
		.into()
}

/// The index of the target with this endpoint key in display order, used to place the
/// context menu against the right row.
fn index_of(targets: &[Target], key: &str) -> Option<usize> {
	targets.iter().position(|target| target.endpoint() == key)
}

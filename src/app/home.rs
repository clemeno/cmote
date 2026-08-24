// app/home.rs — the `Tab` methods for the screen a session starts from (PLAN §14, §22, §49).
//
// The home screen is the saved-target list: the machines cmote remembers, filtered, renamed,
// deleted and opened from here (`CONTEXT.md`: **Target**). A tab begins on it, comes back to it when
// a session ends, and never holds a secret in it — a target is endpoint, auth kind, key path and
// where the last session left off, and nothing else (§14).
//
// The list itself lives in `crate::targets`, which is where the file format, the ordering and the
// filtering are, and it is tested with no session anywhere near it. This file is the tab's half: the
// screen changes, the keyboard, and the dialogs a rename or a delete opens — the parts that need a
// `Tab` and cannot be tested without one.
//
// Split out of `app/mod.rs` for size (§126). `pub(super)` marks the methods the rest of `app` calls;
// a plain `fn` is used only here. The connect FORM's own methods are still in `mod.rs`: they are the
// other half of this screen and a later slice's work.

use super::{AppScreen, Message, Tab, is_close_tab, ui};

impl Tab {
	/// Return to the connect form: reset the keyboard focus to the first field and
	/// focus it natively, so the form is ready for typing and its highlight ring is
	/// aligned (§10). Used by the paths that keep the user on the form to retry
	/// (error Back, passphrase cancel) — a full return to the list uses `go_home`.
	pub(super) fn go_to_form(&mut self) -> iced::Task<Message> {
		// A fresh `ConnectFlow` is both of the lines this used to carry (§132): nothing being asked
		// — which is what puts the form's own keyboard ring back on (§7), since the ring and the
		// prompt are never both live — and the ring back at the first field.
		self.screen = AppScreen::Connect(super::ConnectFlow::default());
		self.apply_form_focus()
	}

	/// Return to the home screen (§14). Closes any open menu / rename, drops a pending
	/// (unsaved) target, and clears the typed secrets out of the form so they do not
	/// linger once we leave it (§12). The saved-target selection is kept so the list
	/// re-opens on the last-used row.
	pub(super) fn go_home(&mut self) -> iced::Task<Message> {
		// A fresh `HomeScreen` IS the "closes any open menu / rename" above (§131). This used to be
		// three assignments under the screen change, which is the same thing said twice — once by
		// the screen and once by hand, with nothing keeping the two in step.
		self.screen = AppScreen::Home(super::HomeScreen::default());
		// Whatever the connect flow was asking is abandoned with the connect itself, and the
		// buffers it was holding go with it (§12) — which the line above now does, since §132 put
		// the prompt inside the screen this one replaces.
		// Leaving for the list abandons any connect in flight, so what it was carrying goes with
		// it — the unsaved target and, above all, the secret it captured (§12, §14, §16).
		self.abandon_attempt();
		self.form.password.clear();
		self.form.passphrase.clear();
		// Going back to the list abandons the connect a copy was opened for, so its carried
		// directory goes too (§52) — whatever is dialed from here is not that copy. The armed dial
		// goes with it, or a worker arriving a moment later would dial from the home screen.
		self.carry_cwd = None;
		self.pending_connect = false;
		iced::Task::none()
	}

	/// Open a blank connect form for a brand-new connection (§14): reset every field,
	/// focus the first, and switch to the form.
	pub(super) fn open_form_new(&mut self) -> iced::Task<Message> {
		// No menu to close: `go_to_form` leaves this screen, and the menu was part of it (§131).
		self.form = ui::connect::ConnectForm::default();
		self.go_to_form()
	}

	/// Open the connect form pre-filled from the selected target (§14): its host / port / user /
	/// auth / key path are copied in. The secret field starts empty UNLESS the target has a
	/// remembered secret (§16), in which case it is pre-filled from the vault — unlocking it via
	/// the master-passphrase prompt first if the vault is not yet open. A stale/missing
	/// selection is a no-op.
	pub(super) fn open_selected_target(&mut self) -> iced::Task<Message> {
		// Every path out of here leaves this screen — `seed_form` sets `Connect` before it opens the
		// vault prompt, and `go_to_form` is the other exit — so the menu goes with it (§131).
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		// A deferred task means the secret is behind the master passphrase and the form is not
		// finished being filled; otherwise it is ready as it stands and all that is left is to show
		// it (§16).
		self.seed_form(&key).unwrap_or_else(|| self.go_to_form())
	}

	/// A new pattern in the home screen's filter box (§49): keep it, and let go of the selection
	/// if the row it names is no longer on screen.
	///
	/// Dropping it is the whole point. Every shortcut this screen has acts on the selection and
	/// not on what the pointer is over — F2 renames it, Enter opens it, Delete asks to remove it
	/// — so a selection hidden behind a filter is one keystroke away from renaming or deleting a
	/// row the user cannot see, and the confirmation naming a target that is not in the list
	/// reads as a bug rather than as the warning it is. Re-selecting is a click, the same click
	/// that selected it in the first place, so nothing is lost by letting go.
	pub(super) fn on_home_filter(&mut self, pattern: String) {
		self.home_filter = pattern;
		let still_shown = self.home_selected.as_deref().is_some_and(|key| {
			self.targets
				.borrow()
				.find(key)
				.is_some_and(|target| target.matches(&self.home_filter))
		});
		if !still_shown {
			self.home_selected = None;
			// The context menu is anchored to the selected row, so it cannot outlive it.
			if let Some(home) = self.home_screen_mut() {
				home.menu_open = false;
			}
		}
	}

	/// Begin an inline rename of the selected target (§14): seed the edit with its
	/// current name and focus the field so the user types straight away. No selection
	/// (or a stale one) is a no-op.
	pub(super) fn start_rename(&mut self) -> iced::Task<Message> {
		let Some(key) = self.home_selected.clone() else {
			return iced::Task::none();
		};
		let Some(name) = self
			.targets
			.borrow()
			.find(&key)
			.map(|target| target.name.clone())
		else {
			return iced::Task::none();
		};
		// One `if let` for the menu and the rename together, because they are one screen's state
		// now (§131) — and it is also the guard that says a rename cannot be started from anywhere
		// but the list, which used to be true only because nothing tried.
		let Some(home) = self.home_screen_mut() else {
			return iced::Task::none();
		};
		home.menu_open = false;
		home.rename = Some(ui::home::RenameState { key, text: name });
		iced::widget::operation::focus(ui::home::RENAME_INPUT_ID)
	}

	/// Commit the in-progress rename (§14): apply it (which re-sorts the list) and save.
	/// A blank name is rejected by the store, so committing one just discards the edit.
	pub(super) fn commit_rename(&mut self) {
		if let Some(rename) = self.home_screen_mut().and_then(|home| home.rename.take()) {
			// Two borrows of the one shared cell must not overlap (a mut + a shared borrow is a
			// RefCell panic), so the rename's `borrow_mut` ends on its own line before `save`
			// takes a fresh shared borrow (§26).
			let renamed = self.targets.borrow_mut().rename(&rename.key, &rename.text);
			if renamed && let Err(error) = self.targets.borrow().save() {
				eprintln!("could not save targets: {error:#}");
			}
		}
	}

	/// Ask before deleting the selected target (§14). Seeds the dialog body with what
	/// deleting does *and* which target it hits — the list is only a click away from the
	/// wrong row — then opens the confirmation. No selection (or a stale one) is a no-op.
	pub(super) fn ask_delete_selected_target(&mut self) {
		let Some(key) = self.home_selected.clone() else {
			return;
		};
		let Some(name) = self
			.targets
			.borrow()
			.find(&key)
			.map(|target| target.name.clone())
		else {
			return;
		};
		let body = format!("{}\n\n{}  ({key})", ui::home::DELETE_DIALOG_BODY, name);
		// Before the borrow below: `set_dialog_body` takes `&mut self` too (§131).
		self.set_dialog_body(&body);
		if let Some(home) = self.home_screen_mut() {
			home.menu_open = false;
			home.confirm_delete = true;
		}
	}

	/// Delete the selected target (§14) and save — only reached from a confirmed prompt.
	/// Clears the selection so the menu and the shortcuts no longer point at a gone row. Also
	/// forgets any remembered secret for this endpoint (§16) when the vault is unlocked; if it
	/// is locked the encrypted entry is left orphaned in `secrets.age` — harmless (it is
	/// unreachable without its target and still encrypted) and pruned only when next unlocked.
	pub(super) fn delete_selected_target(&mut self) {
		if let Some(home) = self.home_screen_mut() {
			home.menu_open = false;
			home.confirm_delete = false;
		}
		if let Some(key) = self.home_selected.take() {
			if let Some(vault) = self.vault.borrow_mut().as_mut()
				&& let Err(error) = vault.forget(&key)
			{
				eprintln!("could not update the vault: {error:#}");
			}
			// Fresh, non-overlapping borrows of the shared target cell (see `commit_rename`).
			let removed = self.targets.borrow_mut().remove(&key);
			if removed && let Err(error) = self.targets.borrow().save() {
				eprintln!("could not save targets: {error:#}");
			}
		}
	}

	/// Handle a key on the home screen (§14). While something is holding the keyboard — the delete
	/// prompt, an inline rename — the list shortcuts are inert and only Esc is handled; a stray
	/// Enter must not open a connection behind the modal, and a rename's Enter belongs to the
	/// field's own `on_submit`. Otherwise F2 renames the selection, Enter opens it, Delete asks to
	/// remove it; all are no-ops without a selection. Other keys fall through.
	pub(super) fn on_home_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		let iced::keyboard::Event::KeyPressed {
			key,
			modifiers,
			repeat,
			..
		} = event
		else {
			return iced::Task::none();
		};

		if let Some(claim) = self.keyboard_claim() {
			if matches!(key, iced::keyboard::Key::Named(Named::Escape)) {
				self.dismiss(claim);
			}
			return iced::Task::none();
		}

		// Ctrl+D closes this tab — but only from the home screen, i.e. once logged off from any
		// remote (§30). On a live shell the same key is EOF to the remote (the way you log out),
		// so it is left to the terminal there; pressing it logs the shell out, which lands back
		// here, and a second Ctrl+D then closes the tab — mirroring a terminal's own Ctrl+D twice.
		// It routes through `TabCloseRequested`, so closing the last tab still asks to quit cmote.
		//
		// An AUTO-REPEAT is not the second press — see `is_close_tab`, which is where that fix lives (§104).
		if is_close_tab(&key, modifiers, repeat) {
			return iced::Task::done(Message::TabCloseRequested(self.id));
		}

		// Ctrl+F puts the cursor in the filter box (§49) — the browser's shortcut for the same
		// thing, and the one the terminal's find bar answers to a screen away (Ctrl+Shift+F,
		// §35; the shell has a claim on plain Ctrl+F, this screen does not). Pressing it while
		// already typing there simply focuses it again, which is a no-op rather than a surprise.
		if modifiers.control()
			&& !modifiers.alt()
			&& !modifiers.logo()
			&& matches!(&key, iced::keyboard::Key::Character(character) if character.as_str() == "f")
		{
			return iced::widget::operation::focus(ui::home::FILTER_INPUT_ID);
		}

		match key {
			iced::keyboard::Key::Named(Named::F2) => self.start_rename(),
			iced::keyboard::Key::Named(Named::Enter) => self.open_selected_target(),
			iced::keyboard::Key::Named(Named::Delete) => {
				self.ask_delete_selected_target();
				iced::Task::none()
			}
			// Esc empties the filter box and puts the whole list back (§49) — the way out of a
			// pattern that matches nothing, without going back to the box to erase it. From
			// INSIDE the box it takes two presses: iced's text input unfocuses on Esc and
			// captures the event, so the first press only hands the keyboard back and the second
			// one arrives here. That is the widget's behaviour, not a rule of this screen.
			iced::keyboard::Key::Named(Named::Escape) => {
				self.on_home_filter(String::new());
				iced::Task::none()
			}
			_ => iced::Task::none(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::*;
	use super::super::*;

	/// The home screen's claimants are the home screen's (§14). A terminal-screen holder is not
	/// consulted there and vice versa, which is what the screen match in `keyboard_claim` says.
	#[test]
	fn the_home_screen_has_claimants_of_its_own() {
		let mut app = Tab::default();
		assert!(matches!(app.screen, AppScreen::Home(_)));
		assert_eq!(app.keyboard_claim(), None);

		// Arranged through the screen (§131), which is the only place this state exists — a test
		// that had to reach a `Tab` field for it could arrange a rename on a terminal tab, and did
		// not have to mean anything by it.
		app.home_screen_mut().expect("the home screen").rename = Some(ui::home::RenameState {
			key: "one".to_owned(),
			text: "one".to_owned(),
		});
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::TargetRename));

		// The delete confirmation outranks the rename: a stray Enter must not open a connection
		// from behind the modal.
		app.home_screen_mut()
			.expect("the home screen")
			.confirm_delete = true;
		assert_eq!(app.keyboard_claim(), Some(KeyboardClaim::DeleteTarget));
	}

	/// Going off the list to the FORM ends the rename that was open on it (§131) — and this is the
	/// state that used to survive the trip. `open_form_new` closed the menu by hand and said nothing
	/// about the rename, so a `Tab` sat on the connect screen still holding one: harmless, because
	/// only `ui::home::view` ever drew it, and unrepresentable now rather than harmless.
	///
	/// Not asserted by reading a field back, because there is no field to read: `home_screen()`
	/// answering `None` IS the claim.
	#[test]
	fn a_rename_does_not_survive_the_trip_to_the_form() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());
		let _focus = tab.start_rename();
		assert!(
			tab.home_screen().expect("the home screen").rename.is_some(),
			"a rename is in progress"
		);

		// Off to the form, which is not a "cancel the rename" path anywhere in the code.
		let _focus = tab.open_form_new();

		assert!(
			tab.home_screen().is_none(),
			"there is nowhere for the rename to have survived"
		);
	}

	/// `go_home` puts a FRESH list up: whatever was open on the last one is gone, and the selection
	/// is not (§14, §131). Its doc has always promised both halves; since §131 the first half is the
	/// screen being rebuilt rather than three assignments underneath it.
	///
	/// Called from the home screen on purpose — that is the case where the rebuild is the only thing
	/// doing the clearing, since arriving from another screen would have dropped the state anyway.
	#[test]
	fn going_home_from_home_puts_a_fresh_list_up() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());
		let _focus = tab.start_rename();
		tab.home_screen_mut().expect("the home screen").menu_open = true;
		tab.home_screen_mut()
			.expect("the home screen")
			.confirm_delete = true;

		let _task = tab.go_home();

		let home = tab.home_screen().expect("still the home screen");
		assert!(home.rename.is_none(), "the rename is over");
		assert!(!home.menu_open, "and the menu is closed");
		assert!(!home.confirm_delete, "and the confirmation is gone");
		assert_eq!(
			tab.home_selected.as_deref(),
			Some("root@web-01:22"),
			"but the selection is kept, so the list re-opens on the last-used row"
		);
	}

	/// Typing a pattern the selected row still matches leaves the selection alone — a list that
	/// narrows under the pointer must not also move what the keyboard is aimed at (§49).
	#[test]
	fn a_filter_the_selection_survives_keeps_it_selected() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());

		tab.on_home_filter("web".to_owned());

		assert_eq!(tab.home_selected.as_deref(), Some("root@web-01:22"));
	}

	/// A pattern that hides the selected row lets go of it, and of the menu anchored to it (§49).
	/// Every shortcut on this screen acts on the selection — F2 renames it, Enter opens it,
	/// Delete asks to remove it — so a selection behind the filter is one keystroke away from
	/// acting on a row that is not on screen.
	#[test]
	fn a_filter_that_hides_the_selection_drops_it() {
		let mut tab = tab_with_targets();
		tab.home_selected = Some("root@web-01:22".to_owned());
		tab.home_screen_mut().expect("the home screen").menu_open = true;

		tab.on_home_filter("db".to_owned());

		assert_eq!(
			tab.home_selected, None,
			"the hidden row is no longer selected"
		);
		assert!(
			!tab.home_screen().expect("the home screen").menu_open,
			"and its context menu went with it"
		);
	}

	/// The pattern is matched against the endpoint as well as the name, so a target still called
	/// after its endpoint — which is how every target starts out — is findable by its host, its
	/// login or its port (§49).
	#[test]
	fn a_filter_matches_the_endpoint_as_well_as_the_name() {
		let tab = tab_with_targets();
		tab.targets.borrow_mut().rename("root@db-01:22", "ledger");
		let targets = tab.targets.borrow();

		let ledger = targets.find("root@db-01:22").expect("the renamed target");
		assert!(ledger.matches("ledger"), "by the name the user gave it");
		assert!(ledger.matches("db-01"), "and by where it actually is");
		assert!(ledger.matches("root@*"), "globs read the endpoint too");
		assert!(!ledger.matches("web"), "the other row is not this one");
	}

	/// Esc empties the filter box and puts the whole list back (§49) — the way out of a pattern
	/// that matches nothing without going back to the box to erase it.
	#[test]
	fn escape_empties_the_home_filter() {
		let mut tab = tab_with_targets();
		tab.home_filter = "prod".to_owned();

		let _ = tab.on_home_key(key_press(
			iced::keyboard::key::Named::Escape,
			iced::keyboard::key::Code::Escape,
			iced::keyboard::Modifiers::empty(),
		));

		assert!(tab.home_filter.is_empty());
	}
}

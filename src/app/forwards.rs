// app/forwards.rs — the `Tab` methods for the tunnels a session carries (PLAN §27).
//
// A forward is a tunnel over the live connection — `-L` local, `-R` remote, `-D` dynamic — managed
// from the Tunnels dialog on the status bar and remembered per target, so a reconnect re-establishes
// what the last session had (`CONTEXT.md`: **Forward**). This file is the tab's half of that: the
// dialog's form, adding and removing a row, following each row's status and its live connection
// counts, and writing the set back onto the target.
//
// The arithmetic and the parsing are NOT here. `crate::forward` owns what a spec means and whether
// two of them collide, and it is tested without a session; this file is what needs the channel and
// the shared target list, which is the same split `send_fetches` makes for the panes.
//
// Split out of `app/mod.rs` for size (§126). `pub(super)` marks the methods the rest of `app` calls;
// a plain `fn` is used only here.

use super::{Message, Modal, SshCommand, Tab, ui};

impl Tab {
	/// Open the port-forwards manager (§27): the dialog opens centred with a blank add form, and
	/// the listen field takes the keyboard so a forward can be typed straight away. The form goes
	/// with the dialog, so reopening it never shows what a previous visit left half typed.
	pub(super) fn open_forwards_dialog(&mut self) -> iced::Task<Message> {
		// The manager draws its own list; the shared body buffer has nothing to say for it, and is
		// seeded empty so no previous dialog's message lingers behind it.
		self.open_modal(Modal::Forwards(ui::forward::ForwardForm::default()), "");
		iced::widget::operation::focus(ui::forward::LISTEN_INPUT_ID)
	}

	/// The add form of the open tunnels dialog, or `None` when that is not what is open (§27).
	pub(super) fn forward_form_mut(&mut self) -> Option<&mut ui::forward::ForwardForm> {
		match &mut self.modal {
			Some(Modal::Forwards(form)) => Some(form),
			_ => None,
		}
	}

	/// Add the forward described by the add form (§27): parse the two fields, reject a duplicate
	/// bind, then hand it a fresh id, queue it as `Starting`, ask the worker to start it, and
	/// save the updated set to the target. A parse error is shown under the form and nothing is
	/// sent. The listen/target fields are cleared on success so the next forward starts blank;
	/// the kind is kept, since adding several of one kind is common.
	pub(super) fn add_forward(&mut self) {
		let Some(Modal::Forwards(form)) = &self.modal else {
			return;
		};
		let parsed = crate::forward::ForwardSpec::parse(form.kind, &form.listen, &form.to);
		let spec = match parsed {
			Ok(spec) => spec,
			Err(reason) => return self.refuse_forward(reason),
		};
		// Two forwards cannot bind the same local (or server) endpoint; refuse the duplicate
		// before it is sent, so the second one's inevitable bind failure never happens.
		if self
			.forwards
			.iter()
			.any(|entry| entry.spec.same_endpoint(&spec))
		{
			return self.refuse_forward("A forward already binds that address.".to_owned());
		}

		let id = self.next_forward_id;
		self.next_forward_id += 1;
		if self.send_command(SshCommand::AddForward {
			id,
			spec: spec.clone(),
		}) {
			self.forwards.push(crate::forward::ForwardEntry {
				id,
				spec,
				status: crate::forward::ForwardStatus::Starting,
				// Set only if this is a `-R 0` and the server later reports the port it chose.
				bound_port: None,
				// A fresh forward has carried nothing yet; the gauge fills as connections flow.
				open_count: 0,
				total_count: 0,
			});
			if let Some(form) = self.forward_form_mut() {
				form.listen.clear();
				form.to.clear();
				form.error = None;
			}
			self.persist_forwards();
		}
	}

	/// Show why an add was refused, under the form that asked for it (§27). Nothing is sent, and
	/// what was typed stays, so the reason names a field the user can still see.
	fn refuse_forward(&mut self, reason: String) {
		if let Some(form) = self.forward_form_mut() {
			form.error = Some(reason);
		}
	}

	/// Tear down the forward with this id (§27): drop it from the list, ask the worker to stop
	/// it, and save the shrunk set. An unknown id is a no-op.
	pub(super) fn remove_forward(&mut self, id: u64) {
		let Some(index) = self.forwards.iter().position(|entry| entry.id == id) else {
			return;
		};
		self.forwards.remove(index);
		self.send_command(SshCommand::RemoveForward(id));
		self.persist_forwards();
	}

	/// Start a set of forwards a reconnect restored (§27): each gets a fresh id, is queued as
	/// `Starting`, and is asked for down the channel. No persistence here — the set came FROM the
	/// stored target, so it is already saved.
	pub(super) fn establish_forwards(&mut self, specs: Vec<crate::forward::ForwardSpec>) {
		for spec in specs {
			let id = self.next_forward_id;
			self.next_forward_id += 1;
			if self.send_command(SshCommand::AddForward {
				id,
				spec: spec.clone(),
			}) {
				self.forwards.push(crate::forward::ForwardEntry {
					id,
					spec,
					status: crate::forward::ForwardStatus::Starting,
					// Set only if this is a `-R 0` and the server later reports the port it chose.
					bound_port: None,
					// A fresh forward has carried nothing yet; the gauge fills as connections flow.
					open_count: 0,
					total_count: 0,
				});
			}
		}
	}

	/// Mark a forward's row from a worker event (§27). An id with no matching entry — a late
	/// event for one already removed — is ignored.
	pub(super) fn set_forward_status(&mut self, id: u64, status: crate::forward::ForwardStatus) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			entry.status = status;
		}
	}

	/// A forward came up (§27): mark its row Active, and for a `-R 0` record the port the server
	/// assigned so the row shows where it is actually listening. The spec keeps its authored 0, so
	/// a reconnect asks for a fresh port rather than pinning this ephemeral one.
	pub(super) fn mark_forward_ready(&mut self, id: u64, assigned_port: Option<u16>) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			entry.status = crate::forward::ForwardStatus::Active;
			if assigned_port.is_some() {
				entry.bound_port = assigned_port;
			}
		}
	}

	/// A connection opened or closed on forward `id` (§27): move its live gauge. `opened` raises the
	/// open and total counts; a close lowers the open count (the total only ever grows). An id with
	/// no matching row — a late event for one already removed — is ignored.
	pub(super) fn bump_forward(&mut self, id: u64, opened: bool) {
		if let Some(entry) = self.forwards.iter_mut().find(|entry| entry.id == id) {
			if opened {
				entry.connection_opened();
			} else {
				entry.connection_closed();
			}
		}
	}

	/// Save the session's current forward set to its target (§27), so a reconnect re-establishes
	/// them. Only meaningful with a live connection (the forwards belong to that target); the
	/// specs are written whole, and `set_forwards` skips the disk write when nothing changed.
	fn persist_forwards(&mut self) {
		let Some(endpoint) = self.connection().map(str::to_owned) else {
			return;
		};
		let specs: Vec<crate::forward::ForwardSpec> = self
			.forwards
			.iter()
			.map(|entry| entry.spec.clone())
			.collect();
		// Non-overlapping borrows of the shared target cell (see `commit_rename`).
		let moved = self.targets.borrow_mut().set_forwards(&endpoint, specs);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::*;
	use super::super::*;

	// Open the tunnels dialog if it is not already up and type a forward into its add form (§27) —
	// the way the user does, since the form lives INSIDE the open modal and there is no field on
	// the tab to set. Driving it through `update` is also what pins the wiring: an edit reaching a
	// closed dialog would silently do nothing, and these tests would catch it.
	fn type_forward(app: &mut Tab, kind: crate::forward::ForwardKind, listen: &str, to: &str) {
		if !matches!(app.modal, Some(Modal::Forwards(_))) {
			let _ = app.open_forwards_dialog();
		}
		let _ = app.update(Message::ForwardKindSelected(kind));
		let _ = app.update(Message::ForwardListenChanged(listen.to_owned()));
		let _ = app.update(Message::ForwardToChanged(to.to_owned()));
	}

	// The open tunnels dialog's add form, for assertions about what it is left holding.
	fn forward_form(app: &Tab) -> &ui::forward::ForwardForm {
		match &app.modal {
			Some(Modal::Forwards(form)) => form,
			other => panic!("the tunnels dialog is not open: {other:?}"),
		}
	}

	/// Adding a forward from the dialog parses the two fields, queues the entry as `Starting`,
	/// sends the worker an `AddForward`, and clears the fields for the next one (§27).
	#[test]
	fn adding_a_forward_parses_queues_and_sends_it() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);

		app.add_forward();

		// Queued once, marked starting, and the input fields reset (the kind is kept).
		assert_eq!(app.forwards.len(), 1);
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Starting
		);
		assert!(forward_form(&app).listen.is_empty());
		assert!(forward_form(&app).to.is_empty());
		assert!(forward_form(&app).error.is_none());

		// The worker was asked to start exactly that spec.
		match next_command(&mut rx) {
			Some(SshCommand::AddForward { id, spec }) => {
				assert_eq!(id, app.forwards[0].id);
				assert_eq!(spec.listen_port, 8080);
				assert_eq!(spec.target_host, "db");
				assert_eq!(spec.target_port, 5432);
			}
			other => panic!("expected AddForward, got {other:?}"),
		}
	}

	/// A forward that does not parse sets the inline error and sends nothing (§27).
	#[test]
	fn a_bad_forward_shows_an_error_and_sends_nothing() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"not-a-port",
			"db:5432",
		);

		app.add_forward();

		assert!(app.forwards.is_empty());
		assert!(forward_form(&app).error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Two forwards cannot bind the same endpoint: the duplicate is refused before it is sent,
	/// so the second one's inevitable bind failure never happens (§27).
	#[test]
	fn a_duplicate_bind_is_refused() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(&mut app, crate::forward::ForwardKind::Local, "8080", "a:1");
		app.add_forward();
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::AddForward { .. })
		));

		// Same bind, different target: rejected, nothing added, nothing sent.
		type_forward(&mut app, crate::forward::ForwardKind::Local, "8080", "b:2");
		app.add_forward();
		assert_eq!(app.forwards.len(), 1);
		assert!(forward_form(&app).error.is_some());
		assert!(next_command(&mut rx).is_none());
	}

	/// Removing a forward drops its row and asks the worker to tear it down (§27).
	#[test]
	fn removing_a_forward_drops_it_and_sends_remove() {
		let (mut app, mut rx) = app_with_terminal(16);
		type_forward(&mut app, crate::forward::ForwardKind::Dynamic, "1080", "");
		app.add_forward();
		let id = app.forwards[0].id;
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::AddForward { .. })
		));

		app.remove_forward(id);
		assert!(app.forwards.is_empty());
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::RemoveForward(removed)) if removed == id
		));

		// Removing an unknown id is a no-op — no row change, no command.
		app.remove_forward(999);
		assert!(next_command(&mut rx).is_none());
	}

	/// A worker's readiness / failure event marks the matching row; an event for a forward
	/// already removed is ignored (§27).
	#[test]
	fn a_forward_event_marks_its_row() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);
		app.add_forward();
		let id = app.forwards[0].id;

		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id,
			assigned_port: None,
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Active
		);

		let _ = app.on_ssh_event(SshEvent::ForwardFailed {
			id,
			reason: "port in use".to_owned(),
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Failed("port in use".to_owned())
		);

		// A stale event for a removed forward touches nothing.
		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id: 999,
			assigned_port: None,
		});
		assert_eq!(app.forwards.len(), 1);
	}

	/// A `-R 0` forward's readiness carries the port the server chose; the row records it (so it
	/// shows where the server listens) while the spec keeps its authored 0 (§27).
	#[test]
	fn a_server_assigned_remote_port_is_recorded_on_the_row() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Remote,
			"0",
			"localhost:3000",
		);
		app.add_forward();
		let id = app.forwards[0].id;
		// Authored as 0, no assigned port yet.
		assert_eq!(app.forwards[0].spec.listen_port, 0);
		assert_eq!(app.forwards[0].bound_port, None);

		let _ = app.on_ssh_event(SshEvent::ForwardReady {
			id,
			assigned_port: Some(38217),
		});
		assert_eq!(
			app.forwards[0].status,
			crate::forward::ForwardStatus::Active
		);
		assert_eq!(app.forwards[0].bound_port, Some(38217));
		// The row shows the real port; the persisted spec still asks for a fresh one on reconnect.
		assert_eq!(
			app.forwards[0].label(),
			"R  127.0.0.1:38217 → localhost:3000"
		);
		assert_eq!(app.forwards[0].spec.listen_port, 0);
	}

	/// Connection open/close events move a forward's live gauge (§27): opens raise the live and
	/// total counts, a close lowers the live count while the total stands, and a stale event for a
	/// removed forward is ignored.
	#[test]
	fn a_forward_connection_event_moves_the_gauge() {
		let (mut app, _rx) = app_with_terminal(16);
		type_forward(
			&mut app,
			crate::forward::ForwardKind::Local,
			"8080",
			"db:5432",
		);
		app.add_forward();
		let id = app.forwards[0].id;
		// A fresh forward has carried nothing.
		assert_eq!(app.forwards[0].activity_gauge(), "0 open · 0 total");

		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id });
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id });
		assert_eq!(app.forwards[0].activity_gauge(), "2 open · 2 total");

		// A close drops the live count; the total, a record of traffic seen, stays.
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionClosed { id });
		assert_eq!(app.forwards[0].activity_gauge(), "1 open · 2 total");

		// A stale event for a forward that no longer exists changes nothing.
		let _ = app.on_ssh_event(SshEvent::ForwardConnectionOpened { id: 999 });
		assert_eq!(app.forwards[0].activity_gauge(), "1 open · 2 total");
	}
}

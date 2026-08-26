// app/accounts.rs — the `Tab` methods for having more than one account on one connection
// (PLAN §45, §46, §47).
//
// An SSH connection authenticates once, as one user. Becoming another account is a PROGRAM run on
// that same connection — `sudo -u root -i` — which gets a channel and a shell of its own, so a
// session is a set of identities rather than one (`CONTEXT.md`: **Identity**). This file is that
// set's behaviour: opening the dialog that lists them, running an elevation and answering whatever
// it asks, remembering its password in the vault, switching which one is on screen, and letting one
// go.
//
// It is split out of `app/mod.rs` for size and nothing else (§126): every method here was an
// `impl Tab` method there and still is one. What that costs is visibility — a method a child module
// declares is invisible to its parent, so the ones `mod.rs` calls are `pub(super)` where they used
// to be private. That is the honest price of the split and it is why the marker is worth reading:
// `pub(super)` here means "the rest of `app` calls this", and a plain `fn` means only this file
// does.
//
// The state itself stays in `mod.rs`, because `Tab` does: `identities`, `identity`,
// `next_identity`, `pending_elevation`, and the `Workspace` each parked identity holds.

use super::{
	Identity, Message, Modal, PendingElevation, Secret, SshCommand, Tab, Workspace, bridge,
	fit_terminal, new_emulator, ui,
};

impl Tab {
	/// Open the accounts dialog (§47) — the one way in.
	///
	/// The form opens from the target's SAVED elevation when it has one, so a return visit sees what
	/// the next connection will do and turning it off is one click rather than a re-type. With
	/// nothing saved it opens blank on `sudo`, which is what a sudoers-managed machine expects.
	pub(super) fn open_accounts_dialog(&mut self) -> iced::Task<Message> {
		// A question already outstanding is not to be thrown away by re-opening: pressing Account
		// while sudo is asking must show that question, not a blank form over an elevation that is
		// still waiting for an answer (§47).
		if self
			.elevate_form_mut()
			.is_some_and(|form| form.is_answering())
		{
			return iced::widget::operation::focus(ui::elevate::ANSWER_INPUT_ID);
		}
		let saved = self
			.connection()
			.and_then(|endpoint| self.targets.borrow().find(endpoint).cloned())
			.and_then(|target| target.elevate);
		let form = saved.as_ref().map_or_else(
			ui::elevate::ElevateForm::default,
			ui::elevate::ElevateForm::from_saved,
		);
		// The dialog draws its own list and form; the shared body buffer has nothing to say for it,
		// and is seeded empty so no previous dialog's message lingers behind it.
		self.open_modal(Modal::Elevate(form), "");
		iced::widget::operation::focus(ui::elevate::ACCOUNT_INPUT_ID)
	}

	/// The elevation form of the open accounts dialog, or `None` when that is not what is open (§47).
	pub(super) fn elevate_form_mut(&mut self) -> Option<&mut ui::elevate::ElevateForm> {
		match &mut self.modal {
			Some(Modal::Elevate(form)) => Some(form),
			_ => None,
		}
	}

	/// The rows the accounts dialog lists (§47): every identity this session has, named, with the
	/// one on screen marked and every elevated one closable.
	///
	/// The login identity's name comes from the session's endpoint rather than from the identity —
	/// see [`Identity`] for why it is not stored twice.
	pub(super) fn account_rows(&self) -> Vec<ui::elevate::AccountRow> {
		let login = self.login_account();
		self.identities()
			.iter()
			.map(|identity| ui::elevate::AccountRow {
				identity: identity.id,
				label: match &identity.account {
					Some(account) => account.clone(),
					None => login.clone(),
				},
				selected: identity.id == self.identity(),
				closable: identity.id != bridge::LOGIN_IDENTITY,
			})
			.collect()
	}

	/// The account the session authenticated as, read off its endpoint (§47). `user@host:port` up to
	/// the `@`, falling back to a plain word when there is no session — which the dialog cannot be
	/// open without, so the fallback is for the type rather than for the screen.
	fn login_account(&self) -> String {
		self.connection()
			.and_then(|endpoint| endpoint.split('@').next())
			.unwrap_or("login")
			.to_owned()
	}

	/// The account whose terminal is on screen, for the status bar's button (§47). `None` for the
	/// login identity, which the bar's centred endpoint already names.
	pub(super) fn showing_account(&self) -> Option<&str> {
		self.identities()
			.iter()
			.find(|identity| identity.id == self.identity())
			.and_then(|identity| identity.account.as_deref())
	}

	/// Send the elevation the dialog is asking for (§47).
	///
	/// The account is vetted here and nowhere later: `elevate::valid_user` is the rule that keeps
	/// anything but a plain login name out of the command line `ElevateKind::command` composes, and
	/// this is the boundary the user's own text crosses (§12). A refused name is reported under the
	/// form and nothing is sent.
	pub(super) fn submit_elevation(&mut self) -> iced::Task<Message> {
		let Some(form) = self.elevate_form_mut() else {
			return iced::Task::none();
		};
		// A conversation already running must not be restarted by a second press.
		if !matches!(form.stage, ui::elevate::Stage::Asking) {
			return iced::Task::none();
		}
		let account = form.account.trim().to_owned();
		if account.is_empty() {
			form.error = Some("Which account?".to_owned());
			return iced::Task::none();
		}
		if !crate::elevate::valid_user(&account) {
			form.error = Some(
				"An account is a plain login name — letters, digits, and `_ - .` (§12).".to_owned(),
			);
			return iced::Task::none();
		}
		let (kind, on_connect, remember) = (form.kind, form.on_connect, form.remember);
		form.error = None;
		self.start_elevation(&account, kind, remember, false);
		// The preference is stored on the way OUT, not on success: it says what the next connection
		// should try, and a refused attempt is still what the user asked for. The password is the
		// other way round — see `settle_elevation_secret` (§47).
		self.persist_elevation(&account, kind, on_connect);
		iced::Task::none()
	}

	/// Ask the session to become `account`, and record what has to be known when it resolves (§47).
	///
	/// `automatic` says whether this came from the target's stored preference rather than from the
	/// dialog, which is what decides how a FAILURE is reported: a hands-free attempt has no dialog
	/// behind it, so a refusal has to put one up.
	fn start_elevation(
		&mut self,
		account: &str,
		kind: crate::elevate::ElevateKind,
		remember: bool,
		automatic: bool,
	) {
		let identity = self.next_identity();
		if !self.send_command(SshCommand::Elevate {
			identity,
			kind,
			user: account.to_owned(),
		}) {
			return;
		}
		// One borrow for both, after the send (§134).
		if let Some(session) = self.session_mut() {
			session.next_identity += 1;
			// Listed straight away, and NOT ready: a shell still elevating cannot be switched to, but
			// it has to be in the list for a failure to be reported against (§45).
			session.identities.push(Identity {
				id: identity,
				account: Some(account.to_owned()),
				ready: false,
				work: Workspace::default(),
			});
		}
		self.pending_elevation = Some(PendingElevation {
			identity,
			account: account.to_owned(),
			remember,
			automatic,
			answer: None,
		});
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Waiting { identity };
		}
	}

	/// Write the answer the dialog is holding to the elevating shell (§47).
	///
	/// The typed text becomes a `Secret` here — the last point it is an ordinary `String` — and a
	/// COPY of it is kept in `pending_elevation` until the elevation resolves, which is the only way
	/// a password can be stored after being proved good rather than before (§12, §16).
	pub(super) fn send_elevate_answer(&mut self) -> iced::Task<Message> {
		let Some(form) = self.elevate_form_mut() else {
			return iced::Task::none();
		};
		let ui::elevate::Stage::Answering {
			identity, answer, ..
		} = &mut form.stage
		else {
			return iced::Task::none();
		};
		let identity = *identity;
		// Taken, not cloned: the field is cleared as the answer leaves it, so the plaintext is not
		// left sitting in a widget behind the dialog.
		let secret = Secret::new(std::mem::take(answer));
		form.stage = ui::elevate::Stage::Waiting { identity };
		if let Some(pending) = self.pending_elevation.as_mut()
			&& pending.identity == identity
		{
			pending.answer = Some(secret.clone());
		}
		self.send_command(SshCommand::ElevateAnswer { identity, secret });
		iced::Task::none()
	}

	/// A credential question arrived from an elevating shell (§45, §47).
	///
	/// Two things can be true when one lands. If the dialog is open, the question goes into it. If it
	/// is NOT — which is what an elevation started from the target's stored preference looks like —
	/// the dialog is opened to ask it, because a question nobody is shown is an elevation that hangs.
	///
	/// A password the vault holds is tried FIRST, and only for the first question: `refusal.is_some()`
	/// means the stored one was just rejected, and a question after the first may be a second factor,
	/// which a stored password must never be offered as (§45).
	pub(super) fn on_elevate_prompt(
		&mut self,
		identity: u64,
		label: String,
		refusal: Option<String>,
	) -> iced::Task<Message> {
		// A question for an elevation that is not the one in flight is stale — its shell has since
		// ended — and answering it would put a password on a channel nobody is watching.
		if self
			.pending_elevation
			.as_ref()
			.is_none_or(|pending| pending.identity != identity)
		{
			return iced::Task::none();
		}
		if refusal.is_none()
			&& let Some(secret) = self.stored_elevation_secret(identity)
		{
			if let Some(pending) = self.pending_elevation.as_mut() {
				pending.answer = Some(secret.clone());
			}
			self.send_command(SshCommand::ElevateAnswer { identity, secret });
			return iced::Task::none();
		}
		let mut task = iced::Task::none();
		if self.elevate_form_mut().is_none() {
			task = self.open_accounts_dialog();
		}
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Answering {
				identity,
				label,
				refusal,
				answer: String::new(),
			};
		}
		// The answer field, not the account field: the dialog is a prompt now, and nothing else on
		// it is worth typing into.
		iced::Task::batch([
			task,
			iced::widget::operation::focus(ui::elevate::ANSWER_INPUT_ID),
		])
	}

	/// The password the vault holds for the elevation in flight, if the user asked for one to be
	/// kept and the vault is open (§47).
	///
	/// Offered ONCE per elevation, because a refusal comes back as a question with a `refusal`
	/// attached and `on_elevate_prompt` will not answer one of those from the vault. A locked vault
	/// yields nothing rather than prompting for the master passphrase — an elevation is not the
	/// moment to interrupt with a second question.
	fn stored_elevation_secret(&self, identity: u64) -> Option<Secret> {
		let pending = self.pending_elevation.as_ref()?;
		if pending.identity != identity || !pending.remember {
			return None;
		}
		let endpoint = self.connection()?;
		let key = crate::vault::elevation_key(endpoint, &pending.account);
		self.vault.borrow().as_ref()?.get(&key).cloned()
	}

	/// Store or forget the password of an elevation that has just resolved (§47).
	///
	/// The rule is §45's, applied one layer up: `factors` is how many DISTINCT things were asked
	/// for, and only when it is 1 is the answer a PASSWORD. More than one means a second factor was
	/// involved, and a one-time code kept as a password would be replayed to a machine that has
	/// already spent it. A question re-put after a refusal is the same factor over again, so a
	/// corrected password still counts as one.
	///
	/// Unticking "Remember the password" is how a stored one is removed, which is why the `false`
	/// branch forgets rather than doing nothing.
	fn settle_elevation_secret(&mut self, factors: u32) {
		let Some(pending) = self.pending_elevation.take() else {
			return;
		};
		let Some(endpoint) = self.connection().map(str::to_owned) else {
			return;
		};
		let key = crate::vault::elevation_key(&endpoint, &pending.account);
		let mut stored = false;
		if let Some(vault) = self.vault.borrow_mut().as_mut() {
			if pending.remember && factors == 1 {
				if let Some(secret) = pending.answer {
					match vault.store(&key, secret) {
						Ok(()) => stored = true,
						Err(error) => eprintln!("could not save the vault: {error:#}"),
					}
				}
			} else if let Err(error) = vault.forget(&key) {
				eprintln!("could not update the vault: {error:#}");
			}
		}
		// The flag follows what the vault ACTUALLY holds, so the dialog never opens promising a
		// hands-free elevation that cannot happen — §16's own rule for the connect secret.
		// Non-overlapping borrows of the shared target cell (see `commit_rename`).
		let moved =
			self.targets
				.borrow_mut()
				.set_elevation_remembered(&endpoint, &pending.account, stored);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// Remember (or update) what this target's sessions should become (§47).
	///
	/// The password flag is not touched here: it follows what the vault actually holds, which is
	/// `settle_elevation_secret`'s business.
	fn persist_elevation(
		&mut self,
		account: &str,
		kind: crate::elevate::ElevateKind,
		on_connect: bool,
	) {
		let Some(endpoint) = self.connection().map(str::to_owned) else {
			return;
		};
		let moved = self
			.targets
			.borrow_mut()
			.set_elevation(&endpoint, account, kind, on_connect);
		if moved && let Err(error) = self.targets.borrow().save() {
			eprintln!("could not save targets: {error:#}");
		}
	}

	/// End one elevated shell (§45): EOF on its channel, which ends its login shell and with it the
	/// elevation. The list entry goes when the session says the shell has ended, not here — a shell
	/// that refuses to die must not vanish from the dialog while it is still running.
	pub(super) fn close_identity(&mut self, identity: u64) -> iced::Task<Message> {
		if identity != bridge::LOGIN_IDENTITY {
			self.send_command(SshCommand::CloseIdentity(identity));
		}
		iced::Task::none()
	}

	/// Start the elevation this target remembers, if it remembers one (§47).
	///
	/// Called once the login shell is live, which is the earliest moment a program can be run on the
	/// connection. Three things stop it: no stored elevation, one whose account this build declines
	/// to act on (`Elevation::usable` — `targets.json` is a file the user is invited to edit), and
	/// one that says only "remember this account" rather than "do it every time".
	pub(super) fn elevate_on_connect(&mut self) {
		let saved = self
			.connection()
			.and_then(|endpoint| self.targets.borrow().find(endpoint).cloned())
			.and_then(|target| target.elevate);
		let Some(saved) = saved else { return };
		if !saved.on_connect || !saved.usable() {
			return;
		}
		self.start_elevation(&saved.account, saved.kind, saved.remember_password, true);
	}

	/// Put another identity's terminal on screen (§45).
	///
	/// The swap is the whole mechanism: the live view moves into the identity being left, and the
	/// arriving one's parked view becomes live. The SSH task is told which shell typing belongs to
	/// now, ahead of any keystroke for it — both ride one ordered channel, so they cannot cross.
	///
	/// The returned task re-fits the grid, because the view arriving was laid out for the window as
	/// it was when it was parked: a resize while it was away reached its pty (every shell is
	/// reflowed, §45) but not its emulator, and this is what brings the two back into step.
	pub(super) fn switch_identity(&mut self, to: u64) -> iced::Task<Message> {
		// One borrow for the whole swap (§134): the identities and the live view are both the
		// session's now, so this reads as the list operation it always was. The borrow ends before
		// `send_command` below, which is the tab's.
		let Some(session) = self.session_mut() else {
			return iced::Task::none();
		};
		if to == session.identity {
			return iced::Task::none();
		}
		// An identity still elevating has no terminal to show; its shell does not exist yet.
		if !session
			.identities
			.iter()
			.any(|identity| identity.id == to && identity.ready)
		{
			return iced::Task::none();
		}
		// The identity being LEFT must have an entry to be parked into. It always does — the login
		// identity is listed the moment the shell opens — but checking before anything moves means a
		// list that somehow disagreed would leave the view alone rather than drop a whole terminal on
		// the floor.
		let leaving = session.identity;
		if !session
			.identities
			.iter()
			.any(|identity| identity.id == leaving)
		{
			return iced::Task::none();
		}
		// Taken out of the list first so the swap borrows nothing that is still inside it.
		let mut incoming = match session
			.identities
			.iter_mut()
			.find(|identity| identity.id == to)
		{
			Some(identity) => std::mem::take(&mut identity.work),
			None => return iced::Task::none(),
		};
		// The swap, in one line (§134). It used to be `exchange`, seven `mem::swap`s whose doc called
		// itself *"the one place that has to be COMPLETE: every field of `Workspace` is exchanged
		// here, and a field added there without a line here would leak one account's state into
		// another's pane."* The live view is a `Workspace` now, exactly like the parked ones, so
		// there is no list to keep complete and no way to leave a field out of it.
		std::mem::swap(&mut session.work, &mut incoming);
		if let Some(identity) = session
			.identities
			.iter_mut()
			.find(|identity| identity.id == leaving)
		{
			identity.work = incoming;
		}
		session.identity = to;
		self.send_command(SshCommand::SelectIdentity(to));
		// The file panes follow the same switch (§46) — and are announced AFTER `SelectIdentity`, on
		// the same ordered channel, so the listings cannot be answered by the account being left.
		self.reread_panes();
		// Nothing about the account switch belongs to the grid the user was on: a half-made
		// selection, a drag in flight, a click tally. They are all parked with it and the arriving
		// view brings its own.
		fit_terminal()
	}

	/// An elevated shell is through its conversation (§45): it now has a terminal of its own, so
	/// give it one, close the dialog and put it on screen.
	pub(super) fn on_identity_ready(&mut self, identity: u64, factors: u32) -> iced::Task<Message> {
		// The elevation resolved, so the answer it was holding is either stored or dropped (§47).
		// Before the early return below: an identity the list has lost is exactly the case where a
		// held credential must not be left in memory.
		if self
			.pending_elevation
			.as_ref()
			.is_some_and(|pending| pending.identity == identity)
		{
			self.settle_elevation_secret(factors);
			// The dialog was the thing asking; with the account up and running there is nothing left
			// on it to answer, so it closes rather than sitting over the new terminal.
			if matches!(self.modal, Some(Modal::Elevate(_))) {
				self.modal = None;
			}
		}
		let Some(entry) = self.session_mut().and_then(|session| {
			session
				.identities
				.iter_mut()
				.find(|entry| entry.id == identity)
		}) else {
			return iced::Task::none();
		};
		entry.ready = true;
		// Its own emulator, parked until the switch below brings it forward. Built exactly like the
		// login shell's, so an elevated terminal is in every way the same terminal (§9).
		//
		// `get_or_insert_with`, not an assignment: output for this identity may already have built one
		// and put bytes in it. The session sends this event before the flush that carries the account's
		// greeting and first prompt precisely so it does not have to (`ssh::shell`), but a plain
		// assignment here would silently discard anything that did arrive first — which is the bug that
		// left an elevated terminal blank. Two ways of not losing it are better than one.
		entry.work.terminal.get_or_insert_with(new_emulator);
		self.switch_identity(identity)
	}

	/// An elevated shell has gone (§45): it exited, or it never opened.
	///
	/// If it was on screen, the login identity comes forward — there is always one, and it is the
	/// one shell that cannot go while the session lives. A reason is the remote's own words about
	/// its own policy, so it is shown: a user who cannot tell "wrong password" from "not in the
	/// sudoers file" can fix neither.
	pub(super) fn on_identity_ended(
		&mut self,
		identity: u64,
		reason: Option<String>,
	) -> iced::Task<Message> {
		let mut task = iced::Task::none();
		if identity == self.identity() {
			task = self.switch_identity(bridge::LOGIN_IDENTITY);
		}
		if let Some(session) = self.session_mut() {
			session.identities.retain(|entry| entry.id != identity);
		}
		let Some(reason) = reason else {
			return task; // an ordinary `exit` at an elevated prompt
		};
		// The elevation that failed was holding an answer; it goes now, stored nowhere (§47). The
		// factor count is irrelevant — nothing is kept from an elevation that did not happen — and
		// `settle_elevation_secret` is called with a count that cannot store, so the "unticked means
		// forget" half still runs.
		let automatic = self
			.pending_elevation
			.as_ref()
			.filter(|pending| pending.identity == identity)
			.map(|pending| pending.automatic);
		if automatic.is_some() {
			self.settle_elevation_secret(u32::MAX);
		}
		// Where the reason goes depends on who asked. With the dialog open it goes under the form,
		// beside the account that was refused, and the form goes back to asking so the name can be
		// corrected. A hands-free attempt from the target's stored preference has no dialog behind
		// it, so one is opened to carry the news — otherwise the session simply stays at the login
		// account with nothing said (§47).
		if automatic == Some(true) && self.elevate_form_mut().is_none() {
			task = iced::Task::batch([task, self.open_accounts_dialog()]);
		}
		if let Some(form) = self.elevate_form_mut() {
			form.stage = ui::elevate::Stage::Asking;
			form.error = Some(reason);
			return task;
		}
		// Nothing open to say it in: a toast says why without stealing the keyboard (§10).
		self.toast(reason);
		task
	}
}

#[cfg(test)]
mod tests {
	use super::super::fixtures::*;
	use super::super::*;

	// --- becoming another account on the same connection (§45) ---

	// --- §47: the accounts dialog, the stored preference, and the remembered password ---

	/// The connect form's own elevation fields, end to end (§47): filled in on a target that does not
	/// exist yet, they are stored on the target the connect CREATES and acted on the moment its shell
	/// is live — one press of Connect, no dialog.
	///
	/// Every other test of the hands-free path calls `elevate_on_connect` directly, which is the half
	/// that was already known to work. This drives the other half: `ConnectPressed`, then the
	/// `Connected` the session answers with, so the three steps between them are under test —
	/// `upsert_on_connect` making the target, `adopt_target` writing the form's preference onto it,
	/// and `elevate_on_connect` reading that preference back out through the session's own endpoint.
	#[test]
	fn the_connect_forms_elevation_runs_itself_on_the_first_connection() {
		let (tx, mut rx) = tokio::sync::mpsc::channel(32);
		let mut app = Tab {
			command_tx: Some(tx),
			..Tab::default()
		};
		// Typed, not assigned: an edit that never reached the form would make every assertion below
		// pass against a form the user did not fill in.
		let _ = app.update(Message::HostChanged("rec".to_owned()));
		let _ = app.update(Message::PortChanged("22".to_owned()));
		let _ = app.update(Message::UserChanged("rocky".to_owned()));
		let _ = app.update(Message::PasswordChanged("pw".to_owned()));
		let _ = app.update(Message::FormElevateAccountChanged("root".to_owned()));
		let _ = app.update(Message::FormElevateKindChanged(
			crate::elevate::ElevateKind::Sudo,
		));
		let _ = app.update(Message::FormElevateOnConnectToggled);

		let _ = app.update(Message::ConnectPressed);
		assert!(
			matches!(next_command(&mut rx), Some(SshCommand::Connect(_))),
			"the connect goes out first"
		);

		// The session is up, which is the earliest a program can be run on it.
		let _task = app.on_ssh_event(SshEvent::Connected);
		match drain(&mut rx).into_iter().next() {
			Some(SshCommand::Elevate {
				identity,
				kind,
				user,
			}) => {
				assert_eq!(identity, 1, "the first identity after the login shell");
				assert_eq!(kind, crate::elevate::ElevateKind::Sudo);
				assert_eq!(user, "root");
			}
			other => panic!("expected an elevation, got {other:?}"),
		}
		// And the target the connect created remembers it, so the next connection needs no form.
		let saved = app
			.targets
			.borrow()
			.find("rocky@rec:22")
			.and_then(|target| target.elevate.clone())
			.expect("the new target remembers what the form asked for");
		assert_eq!(saved.account, "root");
		assert!(saved.on_connect);
	}

	/// The whole of the ordinary path (§47): ask to become root, answer the question sudo asks, and
	/// end up with root's terminal on screen — with what was asked for remembered on the target.
	#[test]
	fn becoming_another_account_asks_answers_and_lands() {
		let (mut app, mut rx) = app_with_saved_target();

		ask_to_become(&mut app, "root", true, false);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate {
				identity,
				kind,
				user,
			}) => {
				assert_eq!(kind, crate::elevate::ElevateKind::Sudo);
				assert_eq!(user, "root");
				identity
			}
			other => panic!("expected an elevation, got {other:?}"),
		};
		// Listed at once and not ready: a shell still elevating cannot be switched to, but a failure
		// has to have something to be reported against (§45).
		assert!(
			app.identities()
				.iter()
				.any(|entry| entry.id == identity && !entry.ready),
			"the elevating identity is listed"
		);
		// The preference is stored on the way out, not on success: it says what the NEXT connection
		// should try, and a refused attempt is still what the user asked for.
		let saved = saved_elevation(&app).expect("the target remembers the account");
		assert_eq!(saved.account, "root");
		assert!(saved.on_connect);
		assert!(!saved.remember_password, "nothing was asked to be kept");

		// sudo asks, in its own words, and the dialog puts exactly that question.
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		match &app.elevate_form_mut().expect("the dialog is open").stage {
			ui::elevate::Stage::Answering { label, refusal, .. } => {
				assert_eq!(label, crate::elevate::MARKER);
				assert!(refusal.is_none());
			}
			other => panic!("expected a question, got {other:?}"),
		}

		// The answer goes down the wire as a `Secret`, and the field it was typed into is cleared.
		let _ = app.update(Message::ElevateAnswerEdited("hunter2".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		match next_command(&mut rx) {
			Some(SshCommand::ElevateAnswer {
				identity: to,
				secret,
			}) => {
				assert_eq!(to, identity);
				assert_eq!(secret.expose(), "hunter2");
			}
			other => panic!("expected an answer, got {other:?}"),
		}

		// The shell comes up, root's terminal is put on screen, and the dialog closes — there is
		// nothing left on it to answer.
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 1,
		});
		assert_eq!(app.identity(), identity, "root's terminal is on screen");
		assert!(app.modal.is_none(), "the dialog is done asking");
		assert_eq!(app.showing_account(), Some("root"), "and the bar names it");
	}

	/// An account name is vetted at the field, not quoted and hoped for (§12, §47): the one place
	/// cmote composes a remote command line from something the user typed.
	#[test]
	fn an_account_that_is_not_a_login_name_is_refused_at_the_field() {
		let (mut app, mut rx) = app_with_saved_target();

		for attempt in ["root; rm -rf /", "-froot", "", "ro ot", "root$(id)"] {
			ask_to_become(&mut app, attempt, false, false);
			assert!(
				next_command(&mut rx).is_none(),
				"{attempt:?} must not reach the wire"
			);
			let form = app.elevate_form_mut().expect("the dialog stays open");
			assert!(
				form.error.is_some(),
				"{attempt:?} is reported under the form"
			);
			assert!(
				matches!(form.stage, ui::elevate::Stage::Asking),
				"{attempt:?} leaves the form asking"
			);
		}
		assert!(saved_elevation(&app).is_none(), "and nothing is remembered");
	}

	/// A password is stored only when the elevation SUCCEEDED and only when one factor was asked for
	/// (§45, §47). This is the ordinary case: one question, one answer, kept.
	#[test]
	fn a_password_that_worked_is_kept_when_it_was_asked_for() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		let _ = app.update(Message::ElevateAnswerEdited("hunter2".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		let _drain = next_command(&mut rx);
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 1,
		});

		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert_eq!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.map(|secret| secret.expose().to_owned()),
			Some("hunter2".to_owned()),
			"the password that worked is in the vault"
		);
		assert!(
			saved_elevation(&app).expect("remembered").remember_password,
			"and the target says so, so the dialog can promise it"
		);
	}

	/// SECURITY (§45, §47): an account that took TWO factors has nothing kept. The second question
	/// may have been a one-time code, and a code stored as a password would be replayed to a machine
	/// that has already spent it — which is the same rule that stops the FILE side following such an
	/// account (§46), read off the same number.
	#[test]
	fn an_account_that_took_two_factors_has_nothing_kept() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		// The password, then a second factor — both under cmote's own marker, which is exactly why
		// the wording cannot be what tells them apart.
		for _ in 0..2 {
			let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
				identity,
				label: crate::elevate::MARKER.to_owned(),
				refusal: None,
			});
			let _ = app.update(Message::ElevateAnswerEdited("123456".to_owned()));
			let _ = app.update(Message::ElevateAnswerSubmitted);
			let _drain = next_command(&mut rx);
		}
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity,
			factors: 2,
		});

		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.is_none(),
			"two factors, so nothing is kept"
		);
		assert!(
			!saved_elevation(&app).expect("remembered").remember_password,
			"and the flag says nothing is stored, so nothing promises otherwise"
		);
	}

	/// A refused elevation keeps nothing either (§47), and the reason goes under the form so the
	/// account can be corrected where it was typed.
	#[test]
	fn a_refused_elevation_reports_where_it_was_asked_and_keeps_nothing() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		*app.vault.borrow_mut() = Some(crate::vault::Vault::for_tests(dir.path()));

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		let _ = app.update(Message::ElevateAnswerEdited("wrong".to_owned()));
		let _ = app.update(Message::ElevateAnswerSubmitted);
		let _drain = next_command(&mut rx);
		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity,
			reason: Some("3 incorrect password attempts".to_owned()),
		});

		let form = app.elevate_form_mut().expect("the dialog is still open");
		assert_eq!(
			form.error.as_deref(),
			Some("3 incorrect password attempts"),
			"the remote's own words, under the form"
		);
		assert!(
			matches!(form.stage, ui::elevate::Stage::Asking),
			"and the form is asking again, so the account can be corrected"
		);
		let key = crate::vault::elevation_key("cme@rec:22", "root");
		assert!(
			app.vault
				.borrow()
				.as_ref()
				.and_then(|vault| vault.get(&key))
				.is_none(),
			"a password that was refused is never stored"
		);
	}

	/// A target that remembers an elevation acts on it as soon as the shell is live (§47), and the
	/// stored password answers the first question without a dialog.
	#[test]
	fn a_remembered_elevation_runs_itself_on_connect() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		let mut vault = crate::vault::Vault::for_tests(dir.path());
		vault
			.store(
				&crate::vault::elevation_key("cme@rec:22", "root"),
				Secret::new("hunter2".to_owned()),
			)
			.expect("the test vault stores");
		*app.vault.borrow_mut() = Some(vault);
		app.targets.borrow_mut().set_elevation(
			"cme@rec:22",
			"root",
			crate::elevate::ElevateKind::Sudo,
			true,
		);
		app.targets
			.borrow_mut()
			.set_elevation_remembered("cme@rec:22", "root", true);

		app.elevate_on_connect();
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, user, .. }) => {
				assert_eq!(user, "root");
				identity
			}
			other => panic!("expected an elevation, got {other:?}"),
		};
		// No dialog: nobody asked for one, and the stored password answers the question by itself.
		assert!(app.modal.is_none(), "nothing was put in the user's way");
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		match next_command(&mut rx) {
			Some(SshCommand::ElevateAnswer { secret, .. }) => {
				assert_eq!(secret.expose(), "hunter2", "answered from the vault");
			}
			other => panic!("expected an answer, got {other:?}"),
		}
		assert!(app.modal.is_none(), "and still nothing in the way");
	}

	/// A stored password that the remote REFUSES puts the question to the user rather than trying it
	/// again (§47): a refusal arrives as the same question with the program's words attached, and a
	/// stored password is offered once.
	#[test]
	fn a_refused_stored_password_puts_the_question_to_the_user() {
		let (mut app, mut rx) = app_with_saved_target();
		let dir = tempfile::tempdir().expect("a temp dir for the vault");
		let mut vault = crate::vault::Vault::for_tests(dir.path());
		vault
			.store(
				&crate::vault::elevation_key("cme@rec:22", "root"),
				Secret::new("stale".to_owned()),
			)
			.expect("the test vault stores");
		*app.vault.borrow_mut() = Some(vault);

		ask_to_become(&mut app, "root", false, true);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: None,
		});
		assert!(
			matches!(
				next_command(&mut rx),
				Some(SshCommand::ElevateAnswer { .. })
			),
			"the stored password is tried first"
		);
		// Refused: the same question comes back with the program's words about the last answer.
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: crate::elevate::MARKER.to_owned(),
			refusal: Some("Sorry, try again.".to_owned()),
		});
		assert!(
			next_command(&mut rx).is_none(),
			"the stored password is not tried twice"
		);
		match &app.elevate_form_mut().expect("the dialog is open").stage {
			ui::elevate::Stage::Answering { refusal, .. } => {
				assert_eq!(refusal.as_deref(), Some("Sorry, try again."));
			}
			other => panic!("expected the question to be put, got {other:?}"),
		}
	}

	/// A hand-edited `targets.json` is remote input as far as the account check is concerned (§12,
	/// §47): an elevation whose account is not a plain login name is a stored preference cmote
	/// declines to act on, not an error to report.
	#[test]
	fn a_stored_elevation_with_an_impossible_account_is_not_acted_on() {
		let (mut app, mut rx) = app_with_saved_target();
		// Written past the dialog's own check, which is what editing the file by hand does: the
		// setter stores what it is given, and the READ is where the account is vetted.
		app.targets.borrow_mut().set_elevation(
			"cme@rec:22",
			"root; id",
			crate::elevate::ElevateKind::Sudo,
			true,
		);

		app.elevate_on_connect();
		assert!(
			next_command(&mut rx).is_none(),
			"nothing composed from it reaches the wire"
		);
		assert!(app.modal.is_none(), "and nothing is put in the way");
	}

	/// Switching between the accounts a session has, and closing one, are where the dialog puts them
	/// — beside the account they act on (§45, §47). The login account has no ✕: ending it is what
	/// Disconnect does.
	#[test]
	fn the_dialog_lists_every_account_and_only_elevated_ones_close() {
		let (mut app, mut rx) = app_with_saved_target();
		let root = elevate_to(&mut app);

		let rows = app.account_rows();
		assert_eq!(rows.len(), 2, "the login account and root");
		let login = rows
			.iter()
			.find(|row| row.identity == bridge::LOGIN_IDENTITY)
			.expect("the login account is listed");
		assert_eq!(login.label, "cme", "named from the session's endpoint");
		assert!(!login.closable, "ending it is what Disconnect does");
		let elevated = rows
			.iter()
			.find(|row| row.identity == root)
			.expect("root is listed");
		assert_eq!(elevated.label, "root");
		assert!(elevated.closable);
		assert!(elevated.selected, "and it is the one on screen");

		// Switching back to the login account, by clicking its name.
		let _task = app.update(Message::IdentitySelected(bridge::LOGIN_IDENTITY));
		assert_eq!(app.identity(), bridge::LOGIN_IDENTITY);
		assert_eq!(app.showing_account(), None, "so the bar stops naming one");

		// And closing root: EOF on its channel. Drained rather than taken one at a time, because a
		// switch sends the file panes' re-listing ahead of it (§46).
		let _task = app.update(Message::IdentityClosed(root));
		let mut closed = Vec::new();
		while let Some(command) = next_command(&mut rx) {
			if let SshCommand::CloseIdentity(id) = command {
				closed.push(id);
			}
		}
		assert_eq!(closed, vec![root], "the close goes down the wire");
		// The list entry stays until the session says the shell has ended — a shell that refuses to
		// die must not vanish from the dialog.
		assert!(app.identities().iter().any(|entry| entry.id == root));
		// The login identity is not closable this way, whatever asks.
		let _task = app.update(Message::IdentityClosed(bridge::LOGIN_IDENTITY));
		let mut after = Vec::new();
		while let Some(command) = next_command(&mut rx) {
			if let SshCommand::CloseIdentity(id) = command {
				after.push(id);
			}
		}
		assert!(after.is_empty(), "the login shell is Disconnect's to end");
	}

	/// Pressing Account while sudo is asking shows the question, not a blank form over an elevation
	/// that is still waiting for an answer (§47).
	#[test]
	fn re_opening_the_dialog_does_not_throw_away_an_outstanding_question() {
		let (mut app, mut rx) = app_with_saved_target();
		ask_to_become(&mut app, "root", false, false);
		let identity = match next_command(&mut rx) {
			Some(SshCommand::Elevate { identity, .. }) => identity,
			other => panic!("expected an elevation, got {other:?}"),
		};
		let _focus = app.on_ssh_event(SshEvent::ElevateChallenge {
			identity,
			label: "Verification code:".to_owned(),
			refusal: None,
		});

		let _focus = app.update(Message::AccountPressed);
		match &app.elevate_form_mut().expect("still open").stage {
			ui::elevate::Stage::Answering { label, .. } => {
				assert_eq!(
					label, "Verification code:",
					"the question survives the press"
				);
			}
			other => panic!("expected the question, got {other:?}"),
		}
	}

	/// Switching accounts moves the FILE panes too (§46), and reads them again as the account now
	/// selected: the path stays — elevating because a folder would not open is the ordinary reason to
	/// do it — but nothing another account listed is left on screen while the new listing is awaited.
	#[test]
	fn switching_accounts_reads_the_file_panes_again_as_the_new_account() {
		let (mut app, mut rx) = app_with_login_identity();
		// A tree with a listed, open folder and a pane showing it — `cme`'s view of /etc.
		let _fetch = app.panes.tree.expand("/etc", false);
		app.panes.tree.listed("/etc", vec!["ssl".to_owned()]);
		if let Some(request) = app.panes.pane.show("/etc") {
			app.list_files(request);
		}
		// Becoming root puts root's shell on screen, and that same switch moves the panes.
		let root = elevate_to(&mut app);
		assert_eq!(app.identity(), root);

		let sent = drain(&mut rx);
		// The account is announced BEFORE the listings, on the one ordered channel, so a listing can
		// never be answered by the account being left.
		let select = sent
			.iter()
			.position(|command| matches!(command, SshCommand::SelectIdentity(id) if *id == root))
			.expect("the switch is announced");
		let listed = sent
			.iter()
			.position(|command| matches!(command, SshCommand::ListDir(path) if path == "/etc"))
			.expect("the open folder is read again");
		assert!(select < listed, "the account is named first");
		assert!(
			sent.iter().any(
				|command| matches!(command, SshCommand::ListFiles { path, .. } if path == "/etc")
			),
			"and so is the pane's own folder"
		);
		// Nothing `cme` listed is on screen in the meantime: the rows stand empty under the spinner
		// until root's own listing lands.
		assert!(
			app.panes
				.tree
				.rows()
				.iter()
				.all(|row| row.path != "/etc/ssl"),
			"another account's children must not survive the switch"
		);
		assert_eq!(app.panes.pane.count(), 0, "nor its files");

		// And it happens in both directions: going back to `cme` re-reads what root had listed.
		app.panes.tree.listed("/etc", vec!["shadow.d".to_owned()]);
		let _task = app.switch_identity(bridge::LOGIN_IDENTITY);
		let back = drain(&mut rx);
		assert!(
			back.iter()
				.any(|command| matches!(command, SshCommand::ListDir(path) if path == "/etc")),
			"the folder is read again as the login account too"
		);
		assert!(
			app.panes
				.tree
				.rows()
				.iter()
				.all(|row| row.path != "/etc/shadow.d"),
			"and root's children go with the switch"
		);
	}

	/// A file opened as root belongs to root for as long as the editor lives (§46): its save names
	/// that account, not whichever one the session happens to be showing when Save is pressed.
	#[test]
	fn a_file_opened_as_root_is_still_saved_as_root_after_switching_back() {
		let (mut session, rx) = app_with_login_identity();
		let root = elevate_to(&mut session);
		let mut app = tab_app();
		let id = session.id;
		let region = strip_mut(&mut app);
		region.tabs.clear();
		region.tabs.push(session);
		region.active = 0;
		app.next_id = id + 1;

		let _task = app.open_viewer(app.focus, id, "/root/.ssh/authorized_keys".to_owned());
		let editor = app
			.tabs()
			.find_map(Tab::editor)
			.expect("the editor tab is open");
		assert_eq!(editor.identity, root, "opened as the account on screen");

		// The session goes back to `cme` while the file is still open, and the save still names root.
		let viewer_id = app
			.tabs()
			.find(|tab| tab.editor().is_some())
			.map(|tab| tab.id)
			.expect("the editor tab has an id");
		if let Some(tab) = app.tab_mut(id) {
			let _task = tab.switch_identity(bridge::LOGIN_IDENTITY);
		}
		let mut rx = rx;
		let _drained = drain(&mut rx);
		let _task = app.flush_editor_save(viewer_id);

		let saved = drain(&mut rx)
			.into_iter()
			.find_map(|command| match command {
				SshCommand::EditSave { identity, .. } => Some(identity),
				_ => None,
			})
			.expect("the save was sent");
		assert_eq!(saved, root, "written back as the account that read it");
	}

	/// Switching accounts swaps a whole view, not just the grid (§45): the scrollback, the
	/// selection and the find bar all belong to the account, and all of them come back.
	#[test]
	fn switching_accounts_parks_one_whole_view_and_restores_the_other() {
		let (mut app, _rx) = app_with_login_identity();
		let _ = app.on_ssh_event(shell_output(b"i am cme\r\n"));
		let _focus = app.open_term_find();
		app.term_find_query("cme".to_owned());
		assert_eq!(app.search().unwrap().count(), 1);

		let root = elevate_to(&mut app);
		assert_eq!(app.identity(), root, "the new account comes forward");
		assert!(
			app.search().is_none(),
			"root's view has its own find bar, which is shut"
		);
		assert!(
			app.terminal().unwrap().find("i am cme").is_empty(),
			"and its own scrollback, which is empty"
		);

		// Back to the login account: everything that was parked is on screen again.
		let _task = app.switch_identity(bridge::LOGIN_IDENTITY);
		assert_eq!(app.identity(), bridge::LOGIN_IDENTITY);
		assert!(
			!app.terminal().unwrap().find("i am cme").is_empty(),
			"cme's scrollback survived the round trip"
		);
		assert_eq!(
			app.search().map(super::super::term::search::Search::count),
			Some(1),
			"and so did its find bar, query and all"
		);
	}

	/// Output for an account that is NOT on screen fills that account's own scrollback (§45) — a
	/// build left running as cme must not print into root's grid, and must not be lost either.
	#[test]
	fn output_for_a_parked_account_goes_to_its_own_scrollback() {
		let (mut app, mut rx) = app_with_login_identity();
		let root = elevate_to(&mut app);
		let _ = drain(&mut rx);

		// cme's shell keeps talking while root's is on screen.
		let _ = app.on_ssh_event(shell_output(b"still building\r\n"));
		assert!(
			app.terminal().unwrap().find("still building").is_empty(),
			"root's grid is not where cme's output belongs"
		);
		let parked = app
			.identities()
			.iter()
			.find(|identity| identity.id == bridge::LOGIN_IDENTITY)
			.and_then(|identity| identity.work.terminal.as_ref())
			.expect("cme's terminal is parked, not dropped");
		assert!(
			!parked.find("still building").is_empty(),
			"it went into cme's own scrollback"
		);
		assert_eq!(app.identity(), root, "and the view never moved");
	}

	/// A query from a parked account is still answered — its program is blocked until it is
	/// (§23) — and answered on that account's OWN channel, not down the typing path, which goes
	/// wherever the user is looking (§45).
	#[test]
	fn a_parked_accounts_query_is_answered_on_its_own_channel() {
		let (mut app, mut rx) = app_with_login_identity();
		let _root = elevate_to(&mut app);
		let _ = drain(&mut rx);

		// A cursor-position report request from the shell the user is NOT looking at.
		let _ = app.on_ssh_event(shell_output(b"\x1b[6n"));
		let sent = drain(&mut rx);
		let reply = sent
			.iter()
			.find_map(|command| match command {
				SshCommand::Reply { identity, bytes } => Some((*identity, bytes.clone())),
				_ => None,
			})
			.expect("the query was answered");
		assert_eq!(
			reply.0,
			bridge::LOGIN_IDENTITY,
			"to the shell that asked, not the one on screen"
		);
		assert!(!reply.1.is_empty());
		assert!(
			!sent
				.iter()
				.any(|command| matches!(command, SshCommand::Input(_))),
			"never as ordinary input, which would go to the wrong shell"
		);
	}

	/// The words an elevation ends with are the account's own greeting and its first prompt, flushed as
	/// the program hands the channel over (§45). They must survive arriving BEFORE the identity has an
	/// emulator — dropping them is what left a freshly elevated terminal blank but for its caret.
	#[test]
	fn the_greeting_an_elevation_ends_with_is_not_lost_to_the_order_it_arrives_in() {
		let (mut app, _rx) = app_with_login_identity();
		let root = app.next_identity();
		app.set_identity(app.identity(), app.next_identity() + 1);
		app.identities_mut().expect("a session").push(Identity {
			id: root,
			account: Some("root".to_owned()),
			ready: false,
			work: Workspace::default(),
		});

		// The flush, arriving while root is still off screen and has no terminal of its own.
		let _task = app.on_ssh_event(SshEvent::Output {
			identity: root,
			bytes: b"root@rec:~# ".to_vec(),
		});
		let _task = app.on_ssh_event(SshEvent::IdentityReady {
			identity: root,
			factors: 1,
		});
		assert_eq!(app.identity(), root, "and it is brought forward");
		assert!(
			!app.terminal()
				.expect("root has a terminal")
				.find("root@rec")
				.is_empty(),
			"the prompt it printed is on screen, not swallowed"
		);
	}

	/// An elevated shell exiting brings the login account forward with its view intact (§45), and
	/// the session carries on — only the login shell going down ends that.
	#[test]
	fn an_elevated_shell_exiting_falls_back_to_the_login_account() {
		let (mut app, _rx) = app_with_login_identity();
		let _ = app.on_ssh_event(shell_output(b"i am cme\r\n"));
		let root = elevate_to(&mut app);

		let _task = app.on_ssh_event(SshEvent::IdentityEnded {
			identity: root,
			reason: None,
		});
		assert_eq!(app.identity(), bridge::LOGIN_IDENTITY);
		assert!(
			!app.terminal().unwrap().find("i am cme").is_empty(),
			"back to cme's own scrollback, where it was left"
		);
		assert_eq!(app.identities().len(), 1);
		assert!(app.is_live(), "the session is still up");
	}

	/// The session ending takes every identity with it (§45): they were shells on that connection.
	#[test]
	fn disconnecting_forgets_every_account() {
		let (mut app, _rx) = app_with_login_identity();
		let _root = elevate_to(&mut app);

		let _task = app.on_ssh_event(SshEvent::Disconnected);
		assert!(app.identities().is_empty());
		assert_eq!(app.identity(), bridge::LOGIN_IDENTITY);
	}
}

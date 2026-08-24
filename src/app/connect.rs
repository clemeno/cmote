// app/connect.rs — the `Tab` methods for getting a session open (PLAN §7, §8, §10, §12, §16, §103).
//
// From the connect form to an authenticated shell: validating the form and sending the `Connect`,
// seeding the form from a saved target or from a session being copied, opening a session on THIS
// machine instead (§103), and every credential the handshake can stop and ask for — a host key to
// trust, a key passphrase, keyboard-interactive answers, and the master passphrase the secret vault
// needs before it will give any of them up.
//
// What a credential *is* is not here. `crate::ssh` owns the handshake, `crate::secret` owns the
// zeroized wrapper a typed passphrase lives in, `crate::vault` owns the sealed file, and
// `crate::targets` owns what a target remembers; all four are tested with no window anywhere near
// them. What is left here is the tab's side: which prompt is up, what a submit resumes, and what a
// failed attempt must NOT leave behind.
//
// Split out of `app/mod.rs` for size (§129), finishing the half of the home screen §126 left —
// `home.rs` holds the saved-target list, this holds the form it fills in. `pub(super)` marks the
// methods the rest of `app` calls; a plain `fn` is used only here.

use super::{
	AuthKind, Carry, Challenge, Element, HostKeyChoice, Message, Prompt, Secret, SshCommand, Tab,
	TabContent, VaultPending, bridge, explorer, extract_secret, ui,
};

impl Tab {
	/// Validate the form, then begin connecting (§10). Cheap validation fails fast to the error
	/// screen. When "Remember" is ticked and a non-empty secret is in play (§16), the secret is
	/// captured to store on success; if the vault is not yet unlocked the whole connect is
	/// deferred behind the master-passphrase prompt and resumed on unlock.
	pub(super) fn on_connect_pressed(&mut self) -> iced::Task<Message> {
		let params = match self.form.validate() {
			Ok(params) => params,
			Err(reason) => {
				self.show_error(&reason);
				return iced::Task::none();
			}
		};

		// Decide, before `params` moves into the dial, whether this connect should remember its
		// secret — and capture it now. Only a non-empty secret is worth storing (§16).
		//
		// Written UNCONDITIONALLY, `None` included. An earlier attempt may have captured a secret
		// and then failed, and skipping this line when Remember is off would leave that capture in
		// place: the next successful connect would store the OLD host's password under the OLD
		// endpoint, with nothing ticked and no connection to it (§12, §16).
		self.pending_remember = if self.form.remember {
			extract_secret(&params.auth).map(|secret| {
				let endpoint = crate::targets::endpoint_of(&params.user, &params.host, params.port);
				(endpoint, secret)
			})
		} else {
			None
		};
		// A secret is in play, so the vault must be unlocked to store it. If it is not yet, defer
		// the connect behind the master-passphrase prompt and resume it on unlock.
		if self.pending_remember.is_some() && self.vault.borrow().is_none() {
			return self.open_vault_modal(VaultPending::Connect(params));
		}

		self.dial(params)
	}

	/// Send a validated `Connect` to the SSH task and move to the connecting screen (§10). Split
	/// from `on_connect_pressed` so the deferred-vault path can resume straight here once the
	/// master passphrase is entered (§16). Records the target (no secret) to save if the
	/// session opens (§14).
	fn dial(&mut self, params: bridge::ConnectParams) -> iced::Task<Message> {
		// Fresh attempt: no passphrase has been tried yet, so any upcoming prompt is
		// a first ask (no "incorrect" hint) until the user submits one (§7).
		self.passphrase_failed = false;

		// Capture the target (no secret) to save if this connect succeeds (§14). The
		// key path and certificate are only meaningful for key auth; the name here is a
		// placeholder — `upsert_on_connect` keeps an existing target's custom name.
		let (key_path, cert_path) = if self.form.auth_kind == ui::connect::AuthKind::Key {
			(self.form.key_path.clone(), self.form.cert_path.clone())
		} else {
			(None, None)
		};
		self.pending_target = Some(crate::targets::Target {
			name: crate::targets::endpoint_of(&params.user, &params.host, params.port),
			host: params.host.clone(),
			port: params.port,
			user: params.user.clone(),
			auth_kind: self.form.auth_kind,
			key_path,
			cert_path,
			// Placeholder like `name`: the stored preference wins on connect, and a
			// brand-new target takes the default `upsert_on_connect` gives it (§14).
			show_hidden: self.panes.show_hidden(),
			// The pending target only carries auth into `upsert_on_connect`; the remembered
			// session (§22), the remember flag (§16) and the saved forwards (§27) live with the
			// *stored* target, which the upsert leaves untouched, so these placeholders are never read.
			terminal_path: None,
			files_path: None,
			explorer_width: None,
			files_height: None,
			// A pending target's sort is a placeholder too: the stored target's remembered sort
			// wins on connect, and a brand-new one starts unsorted (§19, §22).
			sort: None,
			sort_dir: None,
			remember_secret: false,
			forwards: Vec::new(),
			// And a placeholder elevation for the same reason: what the form asked for is applied to
			// the STORED target after the connect succeeds (§47).
			elevate: None,
		});

		let status = format!("connecting to {}:{}…", params.host, params.port);
		// The label the terminal status bar will show once the shell is open (§10);
		// capture it now, before `params` moves into the command.
		let endpoint = format!("{}@{}:{}", params.user, params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			// The session begins HERE (§134), which is the one thing `dial` and `dial_local` both do
			// and neither used to say: they set `connection`, and then a screen, and the two agreeing
			// was a convention. One value now.
			self.content = TabContent::Session(super::Session::dialing(endpoint, None, status));
		} else {
			// The command never left, so there is no attempt for either capture to belong to.
			self.abandon_attempt();
		}
		iced::Task::none()
	}

	/// Where the file panes open when this session remembers nowhere (§22, §103).
	///
	/// A REMOTE session opens at `/`, because that is the top of the server and there is nothing better
	/// to say about a machine cmote has just met. A LOCAL one can do better: the shell is standing in
	/// the user's own folder from its first prompt, and opening the panes at the drive list would put
	/// two clicks between the session and the first folder anyone wants. So the two panes start where
	/// the shell already is.
	pub(super) fn default_files_root(&self) -> String {
		if self.local().is_some() {
			crate::local::path::home()
		} else {
			explorer::ROOT.to_owned()
		}
	}

	/// Open a session on THIS machine (§103) — the home screen's Local bar.
	///
	/// The twin of [`dial`], and shorter for everything it does not have to do. There is no target to
	/// capture (a local shell is not a target: no host, no account, nothing to remember), no secret to
	/// store, and no passphrase state to reset — so `abandon_attempt` runs to drop anything a previous,
	/// abandoned connect attempt left behind rather than to prepare for this one.
	///
	/// What it does share with `dial` is the shape: send the command, and move to `Connecting` only if
	/// it left. The status is a full sentence rather than "connecting to …:22" because nothing is being
	/// connected to; it is on screen for a frame or two, until `Connected` arrives.
	pub(super) fn dial_local(
		&mut self,
		shell: crate::local::shells::LocalShell,
	) -> iced::Task<Message> {
		self.abandon_attempt();
		let status = format!("starting {}…", shell.kind.label());
		let endpoint = shell.endpoint();
		let kind = shell.kind;
		if self.send_command(SshCommand::ConnectLocal(shell)) {
			self.content =
				TabContent::Session(super::Session::dialing(endpoint, Some(kind), status));
		}
		iced::Task::none()
	}

	/// This connection attempt is over without opening a session (§14, §16): drop the two things
	/// it was carrying on the promise that it would.
	///
	/// The target is only a target, but the SECRET matters. It is captured when Connect is
	/// pressed with Remember ticked and stored only when the session opens, so anything that ends
	/// the attempt in between has to drop it — otherwise a later successful connect finds it still
	/// there and stores it, under the endpoint it was captured for rather than the one that just
	/// succeeded (§12). One method, so a new way for an attempt to die cannot forget half of it.
	pub(super) fn abandon_attempt(&mut self) {
		self.pending_target = None;
		self.pending_remember = None;
	}

	/// Open the master-passphrase prompt for the secret vault (§16), recording what to resume
	/// once it unlocks. The prompt is in CREATE mode (two fields) when no vault file exists yet,
	/// UNLOCK mode (one field) when it does — fixed here so the view need not re-check the disk.
	/// It shows over the connect form, so the caller has already put the form on screen.
	fn open_vault_modal(&mut self, pending: VaultPending) -> iced::Task<Message> {
		let creating = !crate::vault::Vault::exists();
		let body = if creating {
			ui::VAULT_CREATE_BODY
		} else {
			ui::VAULT_UNLOCK_BODY
		};
		self.open_prompt(
			Prompt::Vault {
				input: String::new(),
				confirm: String::new(),
				creating,
				failed: false,
				pending,
			},
			body,
		);
		iced::widget::operation::focus(ui::VAULT_INPUT_ID)
	}

	/// Handle the vault prompt's submit (§16). Creating: the passphrase must be non-empty and
	/// match its confirmation, else re-ask with the mismatch hint. Unlocking: a wrong passphrase
	/// (or an unreadable file) re-asks with the "not correct" hint — no oracle beyond that
	/// (§12). On success the unlocked vault is kept for the session and the pending action
	/// resumes. The typed values are taken (not copied) out of the fields so nothing lingers.
	pub(super) fn on_vault_submitted(&mut self) -> iced::Task<Message> {
		// Taking the prompt takes the typed values with it: whatever happens next, the passphrase
		// is not left sitting in app state (§12). A re-ask below builds a fresh prompt.
		let Some(Prompt::Vault {
			input,
			confirm,
			creating,
			pending,
			..
		}) = self.take_prompt()
		else {
			return iced::Task::none();
		};

		let opened = if creating {
			// A new master passphrase must be non-empty and typed identically twice, so the one
			// value that protects everything can never be a typo the user cannot reproduce.
			if input.is_empty() || input != confirm {
				return self.reask_vault(creating, pending);
			}
			crate::vault::Vault::create(input)
		} else {
			crate::vault::Vault::unlock(input)
		};

		match opened {
			Ok(vault) => {
				*self.vault.borrow_mut() = Some(vault);
				self.resume_vault_pending(pending)
			}
			Err(error) => {
				// Wrong passphrase, or a damaged / unresolvable file: re-ask. The detail is
				// logged, never shown (§12).
				eprintln!("could not open the vault: {error:#}");
				self.reask_vault(creating, pending)
			}
		}
	}

	/// Ask again, with the "wrong / do not match" hint and empty fields (§16). The prompt is
	/// rebuilt rather than edited in place, so the rejected passphrase is dropped rather than
	/// left in the buffer the next attempt types over.
	fn reask_vault(&mut self, creating: bool, pending: VaultPending) -> iced::Task<Message> {
		// Through `open_prompt`, which is what puts it on the connect screen (§132) — this used to
		// assign the field and rely on being there already. The body is the one already showing,
		// re-seeded, since a re-ask says the same thing plus a hint the view adds from `failed`.
		let body = if creating {
			ui::VAULT_CREATE_BODY
		} else {
			ui::VAULT_UNLOCK_BODY
		};
		self.open_prompt(
			Prompt::Vault {
				input: String::new(),
				confirm: String::new(),
				creating,
				failed: true,
				pending,
			},
			body,
		);
		iced::widget::operation::focus(ui::VAULT_INPUT_ID)
	}

	/// Resume whatever the vault unlock was blocking (§16): continue the deferred connect, or
	/// pre-fill the form's masked field from the now-readable secret. A `Prefill` whose entry is
	/// missing (the flag out of step with the vault) simply leaves the field blank.
	fn resume_vault_pending(&mut self, pending: VaultPending) -> iced::Task<Message> {
		match pending {
			VaultPending::Connect(params) => self.dial(params),
			VaultPending::Prefill(endpoint) => {
				// Read the secret in a short borrow that ends before the `&mut self` call: a held
				// `Ref` on the shared vault cell would clash with `fill_secret_field` (§26).
				let secret = self
					.vault
					.borrow()
					.as_ref()
					.and_then(|vault| vault.get(&endpoint).cloned());
				if let Some(secret) = secret {
					self.fill_secret_field(&secret);
				}
				self.go_to_form()
			}
		}
	}

	/// Dismiss the vault prompt (§16): the prompt goes, and the typed values and the deferred
	/// action go with it, leaving the connect form (populated behind the prompt in both flows).
	/// Cancelling never stores anything — the deferred connect and the pre-fill are simply
	/// abandoned; the user can still type the secret by hand.
	pub(super) fn on_vault_cancelled(&mut self) -> iced::Task<Message> {
		// The prompt goes and the form it was over stays, keeping the ring where it was — the flow
		// is not ending, a question over it is (§132). This used to clear a field and then set the
		// screen it was already on.
		self.clear_prompt();
		// The connect this prompt was blocking is abandoned with it, and the secret it captured
		// goes too (§12, §16).
		self.abandon_attempt();
		iced::Task::none()
	}

	/// Put a decrypted secret into the masked form field its auth method uses (§16): the
	/// password under password auth, the key passphrase under key auth. One endpoint has one
	/// stored secret and one auth kind, so the destination is unambiguous.
	fn fill_secret_field(&mut self, secret: &Secret) {
		match self.form.auth_kind {
			AuthKind::Password => secret.expose().clone_into(&mut self.form.password),
			AuthKind::Key => secret.expose().clone_into(&mut self.form.passphrase),
			// The promptless methods have no stored secret to fill — interactive types every
			// factor live and agent auth signs with a key the agent already holds (§7). A
			// remembered target is never one of these, so these arms are not reached in practice.
			AuthKind::Interactive | AuthKind::Agent => {}
		}
	}

	/// An answer went back and the handshake carries on (§7, §8): the question is closed and the
	/// status line says what is happening now. Said in one place because it is one fact — three
	/// prompts reach it, and a copy that forgot to close the prompt would leave the dialog on screen
	/// over a connection that had already moved on.
	fn authenticating(&mut self) {
		// The session says what it is doing and stops asking, in one call (§134). It used to move the
		// SCREEN, from `Connect` back to `Connecting` — which is what made §133 impossible, because
		// the session had to survive that round trip and could not if it lived in the screen. Nothing
		// moves now: the challenge was the session's all along.
		if let Some(session) = self.session_mut() {
			session.proceeding("authenticating…");
		}
	}

	/// Relay the user's host-key choice to the SSH task (§8): reject, trust once, or pin. Any
	/// choice but reject means the handshake proceeds, so we go back to a connecting status; on
	/// reject the refused handshake surfaces its own error and moves the screen.
	pub(super) fn on_host_key_decision(&mut self, choice: HostKeyChoice) {
		let proceeding = choice != HostKeyChoice::Reject;
		if self.send_command(SshCommand::HostKeyResponse(choice)) && proceeding {
			self.authenticating();
		}
	}

	/// Send the typed passphrase to the SSH task (§7) and return to a connecting
	/// status. The text is moved straight into a `Secret` and the input field
	/// cleared, so no plain copy of the passphrase lingers in app state (§12).
	pub(super) fn on_passphrase_submitted(&mut self) {
		// Taking the prompt takes the typed text with it and moves it straight into a `Secret`, so
		// no plain copy is left behind whether the send succeeds or not (§12).
		let Some(Challenge::Passphrase(input)) = self.take_asked() else {
			return;
		};
		if self.send_command(SshCommand::Passphrase(Secret::new(input))) {
			// An attempt is now in flight. If the key does not unlock, the SSH task
			// re-asks and this flag makes the next prompt show its "incorrect" hint (§7).
			self.passphrase_failed = true;
			self.authenticating();
		}
	}

	/// Dismiss a credential prompt mid-handshake — the key passphrase (§7) or the server's
	/// keyboard-interactive challenge (§7). Both mean the same thing and did the same three lines
	/// twice: the prompt goes first, so the discarded text does not linger (§12); the half-done
	/// handshake is torn down, because there is no way to answer it later; and what the attempt
	/// captured is abandoned rather than left for a future connect to store (§16).
	///
	/// The vault prompt is NOT one of these: it is asked BEFORE anything is dialed, so there is no
	/// handshake to tear down — see `on_vault_cancelled`.
	pub(super) fn on_credential_cancelled(&mut self) -> iced::Task<Message> {
		// `go_to_form` below leaves the session screen, so the challenge and its typed text go with
		// the session that was asking (§134).
		self.send_command(SshCommand::Disconnect);
		self.abandon_attempt();
		self.go_to_form()
	}

	/// Send the typed keyboard-interactive answers to the SSH task (§7) and return to a
	/// connecting status. Each answer is moved straight into a `Secret` and the buffers cleared,
	/// so no plain copy of an OTP or password lingers in app state (§12). The server drives what
	/// happens next: another prompt (the dialog reappears), success, or a generic failure.
	pub(super) fn on_interactive_submitted(&mut self) -> iced::Task<Message> {
		// Taking the prompt takes the answers with it and moves each straight into a `Secret`, so
		// no plain copy of an OTP or password is left behind (§12).
		let Some(Challenge::Interactive { answers, .. }) = self.take_asked() else {
			return iced::Task::none();
		};
		let answers: Vec<Secret> = answers.into_iter().map(Secret::new).collect();
		if self.send_command(SshCommand::Interactive(answers)) {
			self.authenticating();
		}
		iced::Task::none()
	}

	/// Fill the connect form from the stored target `key`, secret and all (§14, §16). Shared by the
	/// home list's Open and by a chip menu's Duplicate (§52), which needs the same form filled the
	/// same way before it can dial.
	///
	/// Returns `Some(task)` when the fill could NOT be finished on the spot: the target remembers a
	/// secret and the vault holding it is locked, so the task is the master-passphrase prompt and
	/// the fill resumes on unlock. `None` means the form is ready as it stands — which includes a
	/// target that remembers nothing, and one whose secret was already to hand.
	///
	/// A key naming no stored target answers `None` with the form untouched, since there is nothing
	/// to fill it from.
	pub(super) fn seed_form(&mut self, key: &str) -> Option<iced::Task<Message>> {
		// Copy out the fields before touching `self.form`, so the borrow of `self.targets` ends
		// first (assigning the form mutably borrows `self`).
		let (host, port, user, auth_kind, key_path, cert_path, remember, elevate) =
			self.targets.borrow().find(key).map(|target| {
				(
					target.host.clone(),
					target.port,
					target.user.clone(),
					target.auth_kind,
					target.key_path.clone(),
					target.cert_path.clone(),
					target.remember_secret,
					target.elevate.clone(),
				)
			})?;
		self.form = ui::connect::ConnectForm {
			host,
			port: port.to_string(),
			user,
			auth_kind,
			password: String::new(),
			key_path,
			cert_path,
			passphrase: String::new(),
			// A remembered target opens with the box already ticked (§16); untick to stop
			// remembering it, which forgets the stored secret on the next connect.
			remember,
			// And with whatever it remembers about becoming another account (§47), so a return
			// visit sees what the next session will do and can change it before connecting.
			elevate_account: elevate
				.as_ref()
				.map(|saved| saved.account.clone())
				.unwrap_or_default(),
			elevate_kind: elevate
				.as_ref()
				.map_or(crate::elevate::ElevateKind::default(), |saved| saved.kind),
			elevate_on_connect: elevate.is_some_and(|saved| saved.on_connect),
		};

		if remember {
			// Read the vault's state in short borrows and drop them before any `&mut self` call
			// (`fill_secret_field` / `open_vault_modal`), so the shared cell is never held across
			// a mutation of the tab (§26).
			if self.vault.borrow().is_some() {
				// Vault already open this session: pull the secret straight into the field.
				let secret = self
					.vault
					.borrow()
					.as_ref()
					.and_then(|vault| vault.get(key).cloned());
				if let Some(secret) = secret {
					self.fill_secret_field(&secret);
				}
			} else {
				// Vault locked: show the (now populated) form as the backdrop and prompt to
				// unlock; the pre-fill resumes on success. `open_prompt` is what moves the screen
				// now (§132) — this used to set it on the line before.
				return Some(self.open_vault_modal(VaultPending::Prefill(key.to_owned())));
			}
		}
		None
	}

	/// Open this fresh tab as a copy of a session on `endpoint`, standing in `cwd` (§52).
	///
	/// The tab is brand new and on the home screen; this fills its form from the same stored target
	/// the endpoint was connected through and dials it, so a duplicate is one menu click rather than
	/// a form to fill in again. The carried directory is set first and spent when the shell opens.
	///
	/// It does NOT reach into the source tab for the secret. The credential comes from the vault, by
	/// exactly the route the home list's Open takes (§16), so a duplicate can do no more than the
	/// user could do by hand — and a password that was typed once and never stored still has to be
	/// typed again, which is the promise "remember" is the opt-in to (§12).
	///
	/// Three ways this can end, and the form is filled in all of them:
	///   * the vault is locked — the master-passphrase prompt opens over the form, and the user
	///     presses Connect once it is filled;
	///   * something is still needed from the user (a password that was never remembered) — the
	///     form opens with the rest already in it;
	///   * nothing is — it dials at once, which is the common case and the point of the feature.
	pub(super) fn open_copy_of(
		&mut self,
		endpoint: &str,
		cwd: Option<String>,
	) -> iced::Task<Message> {
		self.carry_cwd = cwd.map(|cwd| Carry {
			endpoint: endpoint.to_owned(),
			cwd,
		});
		if let Some(deferred) = self.seed_form(endpoint) {
			return deferred;
		}
		if self.ready_to_dial() {
			// The worker is normally not there yet — this tab was made a moment ago — so the dial is
			// armed and fired by the `Ready` that follows. A tab that somehow already has one is
			// dialed on the spot rather than made to wait for an event that has been and gone.
			if self.command_tx.is_some() {
				return self.on_connect_pressed();
			}
			self.pending_connect = true;
		}
		self.go_to_form()
	}

	/// Whether a connect could be sent with the form exactly as it stands (§52) — nothing left for
	/// the user to type.
	///
	/// Validation on its own is not the test: it accepts an EMPTY password, deliberately, because
	/// some servers do (§7). Dialing on an empty password field would spend an authentication
	/// attempt to arrive back at the same form with a failure notice on it, so a password that is
	/// not there is treated as something still to type. Every other method needs no field — a key's
	/// passphrase, a keyboard-interactive challenge and an agent's confirmation are all asked for
	/// during the connect, exactly as they would be from the form's own button.
	fn ready_to_dial(&self) -> bool {
		if self.form.auth_kind == ui::connect::AuthKind::Password && self.form.password.is_empty() {
			return false;
		}
		self.form.validate().is_ok()
	}

	/// Move native focus to match the current form stop: focus the stop's text input,
	/// or — for a radio/button stop — focus a non-existent id, which unfocuses every
	/// input so no field keeps the caret behind the highlight ring (§10).
	pub(super) fn apply_form_focus(&self) -> iced::Task<Message> {
		let id = self
			.form_focus()
			.input_id(self.form.shape())
			.unwrap_or(ui::connect::NO_FOCUS_ID);
		iced::widget::operation::focus(id)
	}

	/// Handle a key on the connect form (§10): Tab / Shift+Tab move the focus ring
	/// (skipping stops that do not apply to the current auth method, §14), Enter / Space
	/// activate the current stop, and Esc returns to the home list. What "activate" means
	/// depends on the stop: a radio/button runs its own callback (switch auth, Browse, or —
	/// on the Connect stop — submit); a TEXT stop has no callback of its own, so Enter there
	/// submits the whole form while Space is left to type a space in the field. Anything else
	/// is ignored here; the focused input still receives it through the widget tree.
	pub(super) fn on_form_key(&mut self, event: iced::keyboard::Event) -> iced::Task<Message> {
		use iced::keyboard::key::Named;

		// Not while something is being asked over the form (§7, §8, §16). `subscription` already
		// switches this listener off, but iced rebuilds the subscription list only AFTER the update
		// that opened the prompt has returned — so a key pressed in the same frame the dialog
		// appeared still arrives here. Without this, Enter could press the Connect button under a
		// host-key dialog.
		if self.prompt().is_some() {
			return iced::Task::none();
		}

		let iced::keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
			return iced::Task::none();
		};

		match key {
			iced::keyboard::Key::Named(Named::Tab) => {
				let shape = self.form.shape();
				// The ring moves inside the flow (§132). The shape is read first, because it comes
				// off `self.form` and the flow is borrowed mutably to step the stop.
				if let Some(flow) = self.connect_flow_mut() {
					flow.focus = if modifiers.shift() {
						flow.focus.previous(shape)
					} else {
						flow.focus.next(shape)
					};
				}
				self.apply_form_focus()
			}
			iced::keyboard::Key::Named(named @ (Named::Enter | Named::Space)) => {
				if self.form_focus().input_id(self.form.shape()).is_some() {
					// A text stop: Enter submits the form (the field has no submit of its
					// own), Space types a space and is left to the field.
					if named == Named::Enter {
						iced::Task::done(Message::ConnectPressed)
					} else {
						iced::Task::none()
					}
				} else if let Some(message) = self.form_focus().activation(self.form.shape()) {
					// A radio/button stop turns the key into its own activation message.
					iced::Task::done(message)
				} else {
					iced::Task::none()
				}
			}
			// Esc backs out of the form to the home list (matches the "← Targets" button).
			iced::keyboard::Key::Named(Named::Escape) => self.go_home(),
			_ => iced::Task::none(),
		}
	}

	/// Remembered-secret bookkeeping for a connect that has just succeeded (§16).
	///
	/// A successful connect is the ONLY place a secret is persisted, and that is the rule this
	/// function exists to keep in one piece: the credentials are now known good, so a wrong password
	/// was never stored. With "Remember" on, store what dial captured; with it off, forget whatever
	/// the vault held for this endpoint. The target's flag is then synced to what the vault ACTUALLY
	/// holds, so the home list never promises a pre-fill that is not there.
	///
	/// All of it needs the vault unlocked, which the dial / open flow already ensured whenever a
	/// secret was in play. If it is locked — the user never engaged it — the flag is left as stored
	/// rather than being cleared on the strength of a vault nobody has opened.
	pub(super) fn settle_remembered_secret(&mut self, key: &str) {
		if let Some(vault) = self.vault.borrow_mut().as_mut() {
			if let Some((endpoint, secret)) = self.pending_remember.take() {
				if let Err(error) = vault.store(&endpoint, secret) {
					eprintln!("could not save the vault: {error:#}");
				}
			} else if !self.form.remember
				&& let Err(error) = vault.forget(key)
			{
				eprintln!("could not update the vault: {error:#}");
			}
			self.targets
				.borrow_mut()
				.set_remembered(key, vault.get(key).is_some());
		}
		self.pending_remember = None;
	}

	/// Overlay a connect-flow dialog on the (dimmed) connect form (§10): the form as the
	/// base, a dimming backdrop that dismisses with `on_dismiss` on a click-away, then the
	/// dialog card on top. The form stays visible behind the dialog rather than being
	/// replaced, so the prompt reads as a modal over the page.
	pub(super) fn form_with_dialog<'a>(
		&'a self,
		dialog: Element<'a, Message>,
		on_dismiss: Message,
	) -> Element<'a, Message> {
		iced::widget::stack![
			ui::connect::view(&self.form, self.form_focus()),
			ui::dialog::backdrop(on_dismiss),
			dialog,
		]
		.width(iced::Length::Fill)
		.height(iced::Length::Fill)
		.into()
	}
}

/// What the tab does with a connect attempt and with the credentials one asks for (§7, §8, §12, §16).
/// The handshake itself is tested in `crate::ssh` and the sealed store in `crate::vault`; these are
/// the rules that only exist once a window is in the way — which prompt is up, what a submit resumes,
/// and what a failed attempt is forbidden to leave behind.
#[cfg(test)]
mod tests {
	use super::super::fixtures::*;
	use super::super::*;

	/// SECURITY (§16, §12): a secret captured for ONE attempt must never be stored on the back of a
	/// later one. The capture happens when Connect is pressed with Remember ticked; the store happens
	/// only on a successful connect. A failed attempt in between has to drop it, or the two ends stop
	/// describing the same connection.
	#[test]
	fn a_failed_attempt_leaves_no_secret_for_a_later_connect_to_store() {
		let (mut app, _rx) = app_with_terminal(16);
		app.pending_remember = Some(("u@a:22".to_owned(), Secret::new("hunter2".to_owned())));
		// A dial's own capture: the target it would save if the session opened (§14).
		app.form.host = "a".to_owned();
		app.form.port = "22".to_owned();
		app.form.user = "u".to_owned();
		app.form.auth_kind = AuthKind::Password;
		app.form.password = "hunter2".to_owned();
		let _ = app.dial(app.form.validate().expect("a valid form"));
		assert!(app.pending_target.is_some(), "the dial captured a target");

		let _ = app.on_ssh_event(SshEvent::Error("authentication failed".to_owned()));

		assert!(
			app.pending_remember.is_none(),
			"the secret belonged to the attempt that just failed"
		);
		assert!(app.pending_target.is_none());
	}

	/// SECURITY (§16): pressing Connect with Remember OFF captures nothing — and clears anything a
	/// previous press captured. Otherwise a secret from an earlier attempt would still be sitting
	/// there when this connection succeeds, and would be stored under the EARLIER endpoint: a
	/// password persisted for a host the user is not even connecting to, without ticking anything.
	#[test]
	fn a_connect_with_remember_off_captures_nothing_and_clears_what_came_before() {
		let (mut app, _rx) = app_with_terminal(16);
		app.pending_remember = Some(("u@a:22".to_owned(), Secret::new("hunter2".to_owned())));

		app.form.host = "b".to_owned();
		app.form.port = "22".to_owned();
		app.form.user = "u".to_owned();
		app.form.auth_kind = AuthKind::Password;
		app.form.password = "different".to_owned();
		app.form.remember = false;
		let _ = app.on_connect_pressed();

		assert!(
			app.pending_remember.is_none(),
			"nothing was ticked, so there is nothing to store — for this host or any other"
		);
	}

	/// SECURITY (§8): an unknown host key is trusted ONLY by an explicit choice. The prompt itself
	/// sends nothing — the handshake is parked on the far side — and Reject sends a refusal without
	/// moving on to "authenticating", so a rejected server never looks like a connecting one.
	#[test]
	fn an_unknown_host_key_is_trusted_only_by_an_explicit_choice() {
		let (mut app, mut rx) = dialing_tab("cme@rec:22", 16);
		let _ = app.on_ssh_event(SshEvent::HostKey("SHA256:aaaa".to_owned()));
		// One pattern for both halves since §132: a host-key question exists only as part of the
		// connect screen, so "the prompt is up" and "the screen shows it" are one claim. The four
		// `on_ssh_event` arms that open a prompt used to set the screen on the next line by hand.
		assert!(matches!(app.asking(), Some(Challenge::HostKey)));
		assert!(
			matches!(app.content, TabContent::Session(_)),
			"and the session it is being asked about is still the screen (§134) — this is the 			 round trip that made §133's shape impossible"
		);
		assert!(
			next_command(&mut rx).is_none(),
			"asking is not answering: nothing goes back until the user chooses"
		);

		let _ = app.update(Message::RejectHostKey);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::HostKeyResponse(HostKeyChoice::Reject))
		));
		// The handshake did NOT move on: the question still stands and the status never became
		// "authenticating". This used to be `!matches!(screen, AppScreen::Connecting { .. })`,
		// which said the same thing when a rejection left the screen on `Connect` — and cannot,
		// now that the session is the screen either way (§134). What it was checking is that
		// `on_host_key_decision` does not call `authenticating` on a refusal, so that is what it
		// checks.
		assert!(
			matches!(app.asking(), Some(Challenge::HostKey)),
			"a refusal leaves the question standing until the far side says the handshake is over"
		);
		assert!(
			!matches!(
				app.session().map(|session| &session.phase),
				Some(SessionPhase::Dialing { status, .. }) if status == "authenticating…"
			),
			"a refusal does not read as a connection in progress"
		);
	}

	/// SECURITY (§8): a CHANGED host key is the man-in-the-middle case, so rejecting is the default
	/// and trusting is the deliberate act. Every dismissal route on that dialog carries
	/// `RejectHostKey` — the ✕, the backdrop, and now Esc — and only the two explicit buttons pin.
	#[test]
	fn a_changed_host_key_rejects_unless_the_user_says_otherwise() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let (mut app, mut rx) = dialing_tab("cme@rec:22", 16);
		let _ = app.on_ssh_event(SshEvent::HostKeyChanged {
			stored: "SHA256:old".to_owned(),
			presented: "SHA256:new".to_owned(),
		});
		assert!(matches!(app.asking(), Some(Challenge::HostKeyChanged)));
		assert!(next_command(&mut rx).is_none());

		// Both fingerprints are in the copyable body, so the user can compare them out of band.
		let body = app.dialog_body.text();
		assert!(body.contains("SHA256:old") && body.contains("SHA256:new"));

		// A key pressed in the frame the dialog appeared must not reach the form's ring underneath —
		// Enter there would press Connect (§10).
		let _ = app.on_form_key(key_press(Named::Enter, Code::Enter, Modifiers::empty()));
		assert!(next_command(&mut rx).is_none());
		assert!(matches!(app.asking(), Some(Challenge::HostKeyChanged)));

		// Replacing the pinned key is the deliberate act, and only then does the handshake go on.
		let _ = app.update(Message::ReplaceHostKey);
		assert!(matches!(
			next_command(&mut rx),
			Some(SshCommand::HostKeyResponse(HostKeyChoice::Pin))
		));
		// Two assertions where one would now do, kept because they say different things about the
		// same fact: the handshake moved on, and no dialog is left over it. Since §132 the second
		// FOLLOWS from the first — `authenticating` closes the question by leaving the screen that
		// holds it, rather than by clearing a field and hoping the two stay in step.
		assert!(matches!(app.content, TabContent::Session(_)));
		assert!(app.prompt().is_none(), "the question is answered and gone");
	}

	/// A question asked OVER the form leaves the form's focus ring where it was (§10, §132): the
	/// flow is not ending, a dialog is opening on top of it. So dismissing the dialog puts the user
	/// back on the field they were on, not at the top of the form.
	///
	/// This is the one behaviour §132 had to choose rather than preserve. `form_focus` used to be a
	/// `Tab` field, so it survived everything by default — including the transitions where surviving
	/// was an accident. Inside `ConnectFlow` it survives exactly where the flow does, and this pins
	/// which of those cases is which.
	#[test]
	fn a_dialog_over_the_form_leaves_the_focus_ring_alone() {
		use iced::keyboard::Modifiers;
		use iced::keyboard::key::{Code, Named};

		let mut app = Tab::default();
		let _focus = app.go_to_form();
		app.form.host = "web-01".to_owned();
		app.form.user = "root".to_owned();
		// Two Tabs along the ring, so the stop is not the default one and the test can tell.
		for _ in 0..2 {
			let _focus = app.on_form_key(key_press(Named::Tab, Code::Tab, Modifiers::empty()));
		}
		let before = app.form_focus();
		assert_ne!(
			before,
			ui::connect::FormStop::default(),
			"the ring has moved off the first field"
		);

		// A vault prompt over the form, then dismissed.
		let _focus = app.open_vault_modal(VaultPending::Prefill("root@web-01:22".to_owned()));
		assert!(matches!(app.prompt(), Some(Prompt::Vault { .. })));
		assert_eq!(
			app.form_focus(),
			before,
			"the ring is where it was while the question is up"
		);

		let _task = app.on_vault_cancelled();
		assert!(app.prompt().is_none(), "the question is gone");
		assert_eq!(
			app.form_focus(),
			before,
			"and the ring is still where the user left it"
		);
	}

	/// A wrong master passphrase re-asks with the hint and EMPTY fields (§16, §12): the rejected
	/// value is dropped rather than left in the buffer the next attempt types over. The deferred
	/// action it was blocking survives the re-ask, or the retry would unlock into nothing.
	#[test]
	fn a_refused_vault_passphrase_re_asks_empty_and_keeps_what_it_was_blocking() {
		// Create mode with a mismatched confirmation — no vault file is touched, so this needs no disk.
		// Arranged as a SCREEN since §132: a prompt with no connect screen under it is not a state
		// this test could set up any more, which is the point of the section.
		let mut app = Tab {
			content: TabContent::Connect(ConnectFlow {
				prompt: Some(Prompt::Vault {
					input: "one".to_owned(),
					confirm: "two".to_owned(),
					creating: true,
					failed: false,
					pending: VaultPending::Prefill("u@h:22".to_owned()),
				}),
				..ConnectFlow::default()
			}),
			..Tab::default()
		};
		let _ = app.on_vault_submitted();
		match app.prompt() {
			Some(Prompt::Vault {
				input,
				confirm,
				failed,
				pending,
				..
			}) => {
				assert!(input.is_empty() && confirm.is_empty());
				assert!(*failed, "the hint says the two did not match");
				assert!(matches!(pending, VaultPending::Prefill(_)));
			}
			other => panic!("expected the prompt to re-ask, got {other:?}"),
		}
	}

	/// A pending target as `dial` builds one (§14): auth and endpoint only. Everything else is a
	/// placeholder there too — the STORED target's remembered session, forwards and flag are what
	/// `adopt_target` reads back, and `upsert_on_connect` leaves those alone.
	fn pending_target(host: &str, user: &str) -> crate::targets::Target {
		crate::targets::Target {
			name: crate::targets::endpoint_of(user, host, 22),
			host: host.to_owned(),
			port: 22,
			user: user.to_owned(),
			auth_kind: AuthKind::Password,
			key_path: None,
			cert_path: None,
			show_hidden: true,
			terminal_path: None,
			files_path: None,
			explorer_width: None,
			files_height: None,
			sort: None,
			sort_dir: None,
			remember_secret: false,
			forwards: Vec::new(),
			elevate: None,
		}
	}

	/// A connection arriving reads everything the target remembers in ONE go (§14, §22, §27).
	///
	/// The read used to be three separate borrows of the shared target list, interleaved with the
	/// `&mut self` calls that act on what they found — so the order was load-bearing and two
	/// comments existed to say so. Asked as one question, it can be asserted as one answer.
	#[test]
	fn a_connect_reads_the_targets_layout_and_forwards_in_one_go() {
		let mut tab = tab_with_targets();
		let endpoint = "root@web-01:22";

		// The endpoint has been connected to before, so it carries a layout and a forward.
		{
			let mut targets = tab.targets.borrow_mut();
			targets.set_session(
				endpoint,
				crate::targets::LeftOff {
					files_path: Some("/srv/data".to_owned()),
					show_hidden: Some(true),
					..crate::targets::LeftOff::default()
				},
			);
			targets.set_forwards(
				endpoint,
				vec![crate::forward::ForwardSpec {
					kind: crate::forward::ForwardKind::Local,
					listen_host: "127.0.0.1".to_owned(),
					listen_port: 8080,
					target_host: "localhost".to_owned(),
					target_port: 80,
				}],
			);
		}

		let arrival = tab.adopt_target(pending_target("web-01", "root"));

		assert_eq!(arrival.key, endpoint, "and it re-uses the saved row");
		let session = arrival.session.expect("the remembered layout came back");
		assert_eq!(session.files_path.as_deref(), Some("/srv/data"));
		assert_eq!(session.show_hidden, Some(true));
		assert_eq!(arrival.forwards.len(), 1, "and so did the saved forward");
		assert_eq!(arrival.forwards[0].listen_port, 8080);
	}

	/// A target never connected to before remembers nothing, and says so rather than being absent
	/// (§14) — the first-connection path, which then falls back to the root and a login directory.
	#[test]
	fn a_first_connection_brings_back_nothing_to_restore() {
		let mut tab = Tab::default();
		let arrival = tab.adopt_target(pending_target("new-host", "cme"));

		assert_eq!(arrival.key, "cme@new-host:22");
		// It IS saved now — a real connect persists the target (§14) — it just has no history.
		assert!(arrival.forwards.is_empty());
		assert!(
			arrival
				.session
				.is_none_or(|session| session.files_path.is_none()),
			"nothing to resume to"
		);
	}
}

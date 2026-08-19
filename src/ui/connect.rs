// ui/connect.rs — the connection form (PLAN §10) and its input validation (§6.0).
//
// The form's field contents live in `ConnectForm`, owned by `App`. The `view`
// function renders those fields as iced widgets; `validate` turns the raw
// strings into a typed `ConnectParams` (or a clear error) before anything is
// sent to the network — "validate at the boundary" (§12).

use std::path::{Path, PathBuf};

use iced::widget::text::Wrapping;
use iced::widget::{button, checkbox, column, container, radio, row, text, text_input};
use iced::{Border, Color, Element, Length};
use serde::{Deserialize, Serialize};

use crate::app::Message;
use crate::bridge::{AuthMethod, ConnectParams};
use crate::secret::Secret;

/// The default SSH port, used when the user leaves the port field blank.
const DEFAULT_SSH_PORT: u16 = 22;

/// Widget ids for the form's text inputs, so `app` can move native focus to the input
/// matching the current keyboard stop (§10). iced can only focus text inputs, so these
/// cover exactly the focusable widgets.
pub const HOST_INPUT_ID: &str = "connect-host";
pub const PORT_INPUT_ID: &str = "connect-port";
pub const USER_INPUT_ID: &str = "connect-user";
pub const PASSWORD_INPUT_ID: &str = "connect-password";
/// The optional key-passphrase field shown under key auth (§14). Distinct from
/// `ui::PASSPHRASE_INPUT_ID`, which is the *interactive* prompt's field — this one
/// lives on the form and only pre-seeds the passphrase.
pub const KEY_PASSPHRASE_INPUT_ID: &str = "connect-key-passphrase";

/// An id that matches no widget on purpose: focusing it unfocuses every input (iced's
/// focus operation unfocuses all non-matching focusables). `app` uses it when the
/// keyboard stop is a radio or button, so no field keeps the caret while the highlight
/// ring marks the active control (§10).
pub const NO_FOCUS_ID: &str = "connect-none";

/// The highlight-ring colour drawn around the focused radio or button (§10) — iced
/// cannot give those widgets a native focus outline, so the form draws its own.
const FOCUS_RING: Color = Color::from_rgb8(0x5a, 0x9c, 0xff);

/// Sizing the chosen key file's middle-ellipsis (§14, §22). The path sits between the row's
/// "Key file" label and the Browse button inside the form's ~420px column, so a long path is
/// trimmed with `…` to two lines' worth and wraps within `KEY_PATH_WIDTH` rather than shoving
/// Browse off the row. `KEY_PATH_CHAR` is a glyph advance; `KEY_PATH_LINES` is the same two
/// the file grid's names wrap to.
///
/// `KEY_PATH_CHAR` is deliberately PESSIMISTIC — fatter than the face truly is — so the char
/// budget lands under what two lines of `KEY_PATH_WIDTH` really hold and the `…` trims the
/// path with margin rather than letting it spill onto a third line, the same discipline the
/// pane headers use.
const KEY_PATH_WIDTH: f32 = 210.0;
const KEY_PATH_CHAR: f32 = 8.0;
const KEY_PATH_LINES: usize = 2;

/// The connect form's keyboard-focus stops, in Tab order (§10). iced can only focus
/// text inputs, so the radios and Connect button are navigated by this bespoke ring:
/// `app` tracks the current stop, Tab / Shift+Tab move it (`next`/`previous`), Enter or
/// Space activate it (`activation`), and `view` draws a highlight on the active
/// radio/button. Text stops instead take native focus via `input_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormStop {
	#[default]
	Host,
	Port,
	User,
	AuthPassword,
	AuthKey,
	/// The keyboard-interactive radio (§7). Like the other auth radios it is a focus ring
	/// stop, activated by Enter/Space to switch the method.
	AuthInteractive,
	/// The SSH agent / Pageant radio (§7). Another focus-ring stop, activated by Enter/Space
	/// to switch the method; like interactive auth it shows no credential fields.
	AuthAgent,
	/// The credential control: the password field under password auth, the Browse
	/// button under key auth (§7). Absent under interactive auth, which shows no credential.
	Credential,
	/// The optional certificate Browse button (§7, §14). Only exists under key auth, between the
	/// key file and the passphrase; a focus-ring stop like the key Browse, activated by
	/// Enter/Space to open the certificate picker. Tab skips it under every other method.
	Certificate,
	/// The optional key-passphrase field. Only exists under key auth (§14); Tab skips
	/// it entirely under password auth (see `is_applicable`).
	KeyPassphrase,
	/// The "Remember" checkbox (§16). A toggle, not a text field, so — like the radios — it
	/// wears the focus ring and is flipped by Enter/Space rather than taking native focus.
	Remember,
	Connect,
}

impl FormStop {
	/// The stops in Tab order; `next`/`previous` cycle through it, skipping any that
	/// do not apply to the current auth method.
	const ORDER: [FormStop; 12] = [
		FormStop::Host,
		FormStop::Port,
		FormStop::User,
		FormStop::AuthPassword,
		FormStop::AuthKey,
		FormStop::AuthInteractive,
		FormStop::AuthAgent,
		FormStop::Credential,
		FormStop::Certificate,
		FormStop::KeyPassphrase,
		FormStop::Remember,
		FormStop::Connect,
	];

	/// Whether this stop is reachable under `auth`. This is what lets Tab skip stops for
	/// controls that are not on screen: the key-passphrase field exists only under key auth,
	/// and under INTERACTIVE auth there is no credential control and no "remember" toggle (the
	/// server drives every prompt, §7), so both are skipped too.
	fn is_applicable(self, auth: AuthKind) -> bool {
		match self {
			// Grouped by the CONDITION rather than kept in Tab order, so each rule is written once
			// and the two stops that share it are visibly the same case.
			FormStop::Credential | FormStop::Remember => !auth.is_promptless(),
			FormStop::Certificate | FormStop::KeyPassphrase => auth == AuthKind::Key,
			_ => true,
		}
	}

	/// The next applicable stop in Tab order, wrapping around at the end.
	pub fn next(self, auth: AuthKind) -> Self {
		let mut stop = self;
		loop {
			stop = Self::ORDER[(stop.index() + 1) % Self::ORDER.len()];
			if stop.is_applicable(auth) {
				return stop;
			}
		}
	}

	/// The previous applicable stop in Tab order, wrapping around at the start.
	pub fn previous(self, auth: AuthKind) -> Self {
		let mut stop = self;
		loop {
			stop = Self::ORDER[(stop.index() + Self::ORDER.len() - 1) % Self::ORDER.len()];
			if stop.is_applicable(auth) {
				return stop;
			}
		}
	}

	/// This stop's position in `ORDER`.
	fn index(self) -> usize {
		Self::ORDER
			.iter()
			.position(|stop| *stop == self)
			.expect("every stop is in ORDER")
	}

	/// The text-input id to focus natively at this stop, or `None` when the stop is a
	/// radio or button (which iced cannot focus). Under key auth the `Credential` stop is
	/// the Browse button, so it has no input id; the `KeyPassphrase` stop is a text field.
	pub fn input_id(self, auth: AuthKind) -> Option<&'static str> {
		match self {
			FormStop::Host => Some(HOST_INPUT_ID),
			FormStop::Port => Some(PORT_INPUT_ID),
			FormStop::User => Some(USER_INPUT_ID),
			FormStop::Credential if auth == AuthKind::Password => Some(PASSWORD_INPUT_ID),
			FormStop::KeyPassphrase if auth == AuthKind::Key => Some(KEY_PASSPHRASE_INPUT_ID),
			_ => None,
		}
	}

	/// The message Enter/Space should dispatch when this stop is a radio or button.
	/// Text-input stops return `None` — there those keys type or submit in the field.
	pub fn activation(self, auth: AuthKind) -> Option<Message> {
		match self {
			FormStop::AuthPassword => Some(Message::AuthKindChanged(AuthKind::Password)),
			FormStop::AuthKey => Some(Message::AuthKindChanged(AuthKind::Key)),
			FormStop::AuthInteractive => Some(Message::AuthKindChanged(AuthKind::Interactive)),
			FormStop::AuthAgent => Some(Message::AuthKindChanged(AuthKind::Agent)),
			FormStop::Credential if auth == AuthKind::Key => Some(Message::BrowseKeyPressed),
			FormStop::Certificate => Some(Message::BrowseCertPressed),
			FormStop::Remember => Some(Message::RememberToggled),
			FormStop::Connect => Some(Message::ConnectPressed),
			_ => None,
		}
	}
}

/// Which authentication method the form is set to. A tiny `Copy` enum so the
/// radio buttons can compare it by value and select the current one; `Password`
/// is the default. This is the UI-side mirror of `bridge::AuthMethod` — the form
/// holds a choice, `validate` turns it into the real method with its secrets.
///
/// It is also what a saved target records (§14), hence the serde derives; the
/// `lowercase` rename keeps the JSON tidy (`"password"` / `"key"`) and stable if the
/// Rust variant names ever change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
	#[default]
	Password,
	Key,
	/// Keyboard-interactive: the server drives the prompts — 2FA / OTP and challenge-response
	/// (§7). Nothing is entered on the form; every factor is answered live on connect, so this
	/// choice carries no secret to validate or persist.
	Interactive,
	/// SSH agent / Pageant: a running agent holds the keys and signs the challenge (§7). Like
	/// interactive auth nothing is entered on the form — there is no key file to pick and no
	/// passphrase, since the agent already unlocked the key — so this choice carries no secret
	/// to validate or persist either.
	Agent,
}

impl AuthKind {
	/// Whether this method shows no credential fields and stores no secret, so the form has
	/// nothing to capture for it (§7). Interactive (the server drives every prompt) and Agent
	/// (a running agent holds and signs with the key) are both promptless; the credential
	/// control, the key-passphrase field and the "Remember" toggle are all hidden for them.
	fn is_promptless(self) -> bool {
		matches!(self, AuthKind::Interactive | AuthKind::Agent)
	}
}

/// The connect form's editable fields. Plain owned values that mirror the
/// widgets: text inputs work with `String`s, the file picker yields a `PathBuf`,
/// and validation converts them to typed values on submit.
#[derive(Debug, Default)]
pub struct ConnectForm {
	pub host: String,
	pub port: String,
	pub user: String,
	/// Which method is selected; decides which credential fields are read.
	pub auth_kind: AuthKind,
	/// Password for `Password` auth.
	pub password: String,
	/// Chosen private-key file for `Key` auth (set by the file picker).
	pub key_path: Option<PathBuf>,
	/// Optional OpenSSH certificate for `Key` auth (§7, §14). Auto-filled with the
	/// `<key>-cert.pub` sibling when a key is picked and that file exists, cleared or replaced
	/// from the form. `None` means plain public-key auth. A path, not a secret, so it is
	/// remembered with the saved target — unlike the passphrase below.
	pub cert_path: Option<PathBuf>,
	/// Optional passphrase for an encrypted key (§14). Left empty, the connect flow
	/// keeps its original behavior — an encrypted key prompts interactively (§7).
	/// Filled, it is tried first so the key unlocks without a prompt. Session-only:
	/// it is moved into a `Secret` on submit and never saved with the target (§12).
	pub passphrase: String,
	/// Whether to remember the credential (password, or the key passphrase) for this target
	/// in the portable encrypted vault (§16). Off by default — the safe, no-secret-at-rest
	/// behaviour (§12) — and honoured only on a SUCCESSFUL connect, so a wrong password is
	/// never stored. When a saved-secret target is opened, this starts ticked and the masked
	/// field is pre-filled from the vault.
	pub remember: bool,
}

impl ConnectForm {
	/// Validate the raw fields and produce typed connection parameters, or a
	/// human-readable reason it is not ready. Cheap checks first, fail fast.
	pub fn validate(&self) -> Result<ConnectParams, String> {
		let host = self.host.trim();
		if host.is_empty() {
			return Err("Host is required.".to_string());
		}

		// Empty port means "use the default". Otherwise it must parse as a u16 —
		// we never trust the port as a raw string past this point.
		let port = if self.port.trim().is_empty() {
			DEFAULT_SSH_PORT
		} else {
			self.port
				.trim()
				.parse::<u16>()
				.map_err(|_| "Port must be a number between 1 and 65535.".to_string())?
		};

		let user = self.user.trim();
		if user.is_empty() {
			return Err("User is required.".to_string());
		}

		let auth = self.validate_auth()?;

		Ok(ConnectParams {
			host: host.to_string(),
			port,
			user: user.to_string(),
			auth,
		})
	}

	/// Turn the selected auth kind and its fields into a typed `AuthMethod`. The
	/// password is wrapped in `Secret` so it is redacted in logs and wiped on drop
	/// (§12); an empty password is allowed here — the server decides. A key needs
	/// a chosen file; an entered passphrase is pre-seeded (§14), an empty one is
	/// `None` so an encrypted key still prompts interactively (§7). Both secrets ride
	/// in `Secret` and are never persisted.
	fn validate_auth(&self) -> Result<AuthMethod, String> {
		match self.auth_kind {
			AuthKind::Password => Ok(AuthMethod::Password(Secret::new(self.password.clone()))),
			AuthKind::Key => {
				let path = self
					.key_path
					.clone()
					.ok_or_else(|| "Choose a private-key file.".to_string())?;
				// An empty passphrase field means "prompt me if needed" — send `None`
				// rather than an empty `Secret`, so the interactive path is unchanged.
				let passphrase = if self.passphrase.is_empty() {
					None
				} else {
					Some(Secret::new(self.passphrase.clone()))
				};
				// The optional certificate rides along as-is: `None` is plain key auth, `Some`
				// asks the connect flow to present the key-and-certificate pair (§7).
				Ok(AuthMethod::Key {
					path,
					passphrase,
					certificate: self.cert_path.clone(),
				})
			}
			// Interactive auth reads no field — the server prompts for every factor on connect
			// (§7), so there is nothing to validate or carry beyond the choice itself.
			AuthKind::Interactive => Ok(AuthMethod::Interactive),
			// Agent auth reads no field either — a running agent holds the keys and signs the
			// challenge (§7), so there is no file, passphrase or secret to capture.
			AuthKind::Agent => Ok(AuthMethod::Agent),
		}
	}
}

/// Render the connect form. Borrows the form so the text inputs can display the
/// current field values; returns an `Element` tied to that borrow. `focus` is the
/// current keyboard stop, used to draw the highlight ring on the active radio/button
/// (text inputs show iced's own focus outline instead) (§10).
pub fn view(form: &ConnectForm, focus: FormStop) -> Element<'_, Message> {
	let mut content = column![
		// A back affordance to the home list (§14). Not part of the Tab ring — it is a
		// navigation escape, also reachable with Esc (see `app::on_form_key`).
		row![
			button(text("← Targets")).on_press(Message::HomePressed),
			text("cmote — SSH connect").size(24),
		]
		.spacing(12)
		.align_y(iced::alignment::Vertical::Center),
		labeled_input(
			"Host",
			"example.com",
			&form.host,
			HOST_INPUT_ID,
			Message::HostChanged
		),
		labeled_input(
			"Port",
			"22",
			&form.port,
			PORT_INPUT_ID,
			Message::PortChanged
		),
		labeled_input(
			"User",
			"root",
			&form.user,
			USER_INPUT_ID,
			Message::UserChanged
		),
		auth_selector(form.auth_kind, focus),
		// The credential fields depend on the selected method — only the relevant
		// ones are shown, so the form stays uncluttered.
		auth_fields(form, focus),
	]
	.spacing(12)
	.padding(20)
	.max_width(420);

	// The opt-in "remember this secret" toggle (§16), below the credential it applies to —
	// omitted under the promptless methods (interactive, agent), which have no secret to store (§7).
	if !form.auth_kind.is_promptless() {
		content = content.push(remember_toggle(
			form.auth_kind,
			form.remember,
			focus == FormStop::Remember,
		));
	}

	content
		.push(focus_ring(
			button("Connect").on_press(Message::ConnectPressed),
			focus == FormStop::Connect,
		))
		.into()
}

/// Wrap `content` in a highlight ring when `focused` — a bordered container that marks
/// the active radio or button during keyboard navigation, since iced gives those
/// widgets no native focus outline (§10). When not focused the border is invisible, so
/// the layout does not shift as focus moves (the 2px padding is always reserved).
fn focus_ring<'a>(content: impl Into<Element<'a, Message>>, focused: bool) -> Element<'a, Message> {
	container(content)
		.padding(2)
		.style(move |_theme| container::Style {
			border: if focused {
				Border {
					color: FOCUS_RING,
					width: 2.0,
					radius: 4.0.into(),
				}
			} else {
				Border::default()
			},
			..container::Style::default()
		})
		.into()
}

/// The four radio buttons that choose the authentication method. `radio` needs a
/// `Copy + Eq` value; passing `Some(selected)` marks the current one as chosen. Each
/// radio wears the focus ring when it is the active keyboard stop.
///
/// The radios are laid out two-by-two beside the "Auth" label: four in a single row would
/// overrun the form's ~420px column, so they wrap onto two lines (Password / Key, then
/// Interactive / Agent) while the label stays aligned with the other rows.
fn auth_selector(selected: AuthKind, focus: FormStop) -> Element<'static, Message> {
	// A small helper so each radio is built the same way, differing only by label, value and
	// which focus stop rings it.
	let radio_stop = |label: &'static str, value: AuthKind, stop: FormStop| {
		focus_ring(
			radio(label, value, Some(selected), Message::AuthKindChanged),
			focus == stop,
		)
	};
	row![
		text("Auth").width(90),
		column![
			row![
				radio_stop("Password", AuthKind::Password, FormStop::AuthPassword),
				radio_stop("Key", AuthKind::Key, FormStop::AuthKey),
			]
			.spacing(10),
			row![
				radio_stop(
					"Interactive",
					AuthKind::Interactive,
					FormStop::AuthInteractive
				),
				radio_stop("Agent", AuthKind::Agent, FormStop::AuthAgent),
			]
			.spacing(10),
		]
		.spacing(8),
	]
	.spacing(10)
	.align_y(iced::alignment::Vertical::Center)
	.into()
}

/// The credential fields for the selected method: a password box, or a key-file
/// chooser plus an optional passphrase (§14). `focus` marks the Browse button when key
/// auth's Credential stop is active.
fn auth_fields(form: &ConnectForm, focus: FormStop) -> Element<'_, Message> {
	match form.auth_kind {
		AuthKind::Password => secure_input(
			"Password",
			&form.password,
			PASSWORD_INPUT_ID,
			Message::PasswordChanged,
		),
		// The key file, an optional certificate, and an optional passphrase. Leaving the
		// passphrase empty keeps the interactive-prompt behavior (§7); filling it pre-seeds the
		// unlock (§14). A certificate is optional and auto-filled from the key's sibling (§7).
		AuthKind::Key => column![
			key_file_row(form.key_path.as_deref(), focus == FormStop::Credential),
			cert_file_row(form.cert_path.as_deref(), focus == FormStop::Certificate),
			passphrase_field(&form.passphrase),
		]
		.spacing(12)
		.into(),
		// Interactive auth has no fields to fill: the server prompts for each factor (a
		// password, then a one-time code, …) once you connect (§7). A short note stands in
		// for the credential inputs so the form does not look unfinished.
		AuthKind::Interactive => text(
			"The server will prompt for each factor (for example a password, then a one-time \
			 code) when you connect.",
		)
		.size(14)
		.into(),
		// Agent auth has no fields either: a running SSH agent supplies and signs with the key.
		// The note names the agents cmote looks for so the user knows what must be running (§7).
		AuthKind::Agent => text(
			"cmote will use a key held by your running SSH agent (the Windows OpenSSH agent or \
			 Pageant on Windows, ssh-agent on macOS). Nothing to enter here.",
		)
		.size(14)
		.into(),
	}
}

/// The optional key-passphrase field (§14). Masked like any secret; its placeholder
/// spells out that leaving it empty falls back to the interactive prompt. It takes a
/// Tab stop (`FormStop::KeyPassphrase`) so keyboard navigation reaches it under key auth.
fn passphrase_field(value: &str) -> Element<'_, Message> {
	row![
		text("Passphrase").width(90),
		text_input("optional — leave empty to be prompted", value)
			.id(KEY_PASSPHRASE_INPUT_ID)
			.secure(true)
			.on_input(Message::KeyPassphraseChanged),
	]
	.spacing(10)
	.into()
}

/// The opt-in "Remember" toggle (§16): a checkbox that stores this target's secret in the
/// portable encrypted vault on a successful connect. The label names the exact credential it
/// applies to — the password, or the key passphrase — so it reads unambiguously under either
/// auth method. Like the radios, it wears the focus ring when it is the active keyboard stop and
/// is flipped by Enter/Space (`FormStop::Remember`); a click toggles it directly. It always
/// raises the same `RememberToggled` message (the new state is derived in `update`), so mouse
/// and keyboard agree.
fn remember_toggle(auth: AuthKind, remembered: bool, focused: bool) -> Element<'static, Message> {
	let label = match auth {
		AuthKind::Password => "Remember password",
		AuthKind::Key => "Remember passphrase",
		// Not reached — `view` omits this toggle under the promptless methods (interactive,
		// agent), which have no secret to store (§7) — but the match must be total, so give a
		// neutral fallback.
		AuthKind::Interactive | AuthKind::Agent => "Remember secret",
	};
	focus_ring(
		checkbox(remembered)
			.label(label)
			.on_toggle(|_checked| Message::RememberToggled),
		focused,
	)
}

/// The key-file chooser: the chosen path (or a prompt) and a Browse button that
/// opens the native file picker. Returns an owned (`'static`) element — the path
/// is copied into a label, so nothing is borrowed from the form. `focused` rings the
/// Browse button when the Credential stop is active under key auth.
fn key_file_row(path: Option<&Path>, focused: bool) -> Element<'static, Message> {
	// A chosen path can be long, so it is middle-ellipsised to the row's two lines (§22) — the
	// whole path is still what `validate` reads, this only keeps the Browse button on the row.
	let label = match path {
		Some(path) => {
			let per_line = crate::ui::cells(KEY_PATH_WIDTH, KEY_PATH_CHAR).max(1);
			crate::ui::elide_middle(&path.display().to_string(), per_line * KEY_PATH_LINES)
		}
		None => "No key file selected".to_string(),
	};
	row![
		text("Key file").width(90),
		text(label)
			.width(Length::Fixed(KEY_PATH_WIDTH))
			.wrapping(Wrapping::WordOrGlyph),
		focus_ring(
			button("Browse…").on_press(Message::BrowseKeyPressed),
			focused,
		),
	]
	.spacing(10)
	.into()
}

/// The optional certificate-file chooser under key auth (§7, §14). An SSH certificate is an
/// add-on to the key — the private key still signs, the certificate is the CA-signed public
/// blob sent with it — so this row sits between the key file and the passphrase. `Browse…` (the
/// Tab stop, ring-highlighted when `focused`) opens the picker; a `Clear` button appears only
/// once a certificate is set, dropping back to plain key auth — which matters because picking a
/// key auto-fills a `<key>-cert.pub` sibling when one exists (§7). The buttons sit beside the
/// label and the chosen path shows elided on its own line beneath, so two buttons fit the
/// narrow form column where the inline key-file row has room for only one. Returns an owned
/// (`'static`) element — the path is copied into a label, nothing is borrowed.
fn cert_file_row(path: Option<&Path>, focused: bool) -> Element<'static, Message> {
	// The chosen path is middle-ellipsised to the row's two lines, like the key file (§22); the
	// whole path is still what `validate` reads. A missing certificate reads as the plain default.
	let label = match path {
		Some(path) => {
			let per_line = crate::ui::cells(KEY_PATH_WIDTH, KEY_PATH_CHAR).max(1);
			crate::ui::elide_middle(&path.display().to_string(), per_line * KEY_PATH_LINES)
		}
		None => "No certificate (optional)".to_string(),
	};
	// Browse is always present and is the keyboard stop; Clear rides beside it only when a
	// certificate is set, and is mouse-only (a secondary affordance, not in the Tab ring).
	let mut buttons = row![focus_ring(
		button("Browse…").on_press(Message::BrowseCertPressed),
		focused,
	)]
	.spacing(10)
	.align_y(iced::alignment::Vertical::Center);
	if path.is_some() {
		buttons = buttons.push(button("Clear").on_press(Message::ClearCertPressed));
	}
	column![
		row![text("Certificate").width(90), buttons]
			.spacing(10)
			.align_y(iced::alignment::Vertical::Center),
		text(label)
			.size(13)
			.width(Length::Fill)
			.wrapping(Wrapping::WordOrGlyph),
	]
	.spacing(6)
	.into()
}

/// A small helper: a label beside a text input, wired to a message constructor.
/// `on_input` takes `fn(String) -> Message`, so we pass the enum variant itself. The
/// `id` lets `app` move native focus to this field for keyboard navigation (§10).
fn labeled_input<'a>(
	label: &'a str,
	placeholder: &'a str,
	value: &'a str,
	id: &'static str,
	on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
	row![
		text(label).width(90),
		text_input(placeholder, value).id(id).on_input(on_input),
	]
	.spacing(10)
	.into()
}

/// A masked (password-style) input with a label. `.secure(true)` hides the
/// characters. The `id` lets `app` focus it during keyboard navigation (§10).
fn secure_input<'a>(
	label: &'a str,
	value: &'a str,
	id: &'static str,
	on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
	row![
		text(label).width(90),
		text_input("", value).id(id).secure(true).on_input(on_input),
	]
	.spacing(10)
	.into()
}

#[cfg(test)]
mod tests {
	use super::*;

	// A form with the required non-auth fields filled, so tests vary only auth.
	fn base_form() -> ConnectForm {
		ConnectForm {
			host: "example.com".to_string(),
			user: "root".to_string(),
			..ConnectForm::default()
		}
	}

	#[test]
	fn password_auth_wraps_the_password() {
		let form = ConnectForm {
			auth_kind: AuthKind::Password,
			password: "hunter2".to_string(),
			..base_form()
		};
		let params = form.validate().expect("valid password form");
		match params.auth {
			AuthMethod::Password(secret) => assert_eq!(secret.expose(), "hunter2"),
			other => panic!("expected password auth, got {other:?}"),
		}
	}

	#[test]
	fn interactive_auth_carries_no_secret() {
		// Interactive auth validates to the fieldless method — the server drives the prompts,
		// so there is nothing on the form to capture (§7).
		let form = ConnectForm {
			auth_kind: AuthKind::Interactive,
			..base_form()
		};
		let params = form.validate().expect("valid interactive form");
		assert!(matches!(params.auth, AuthMethod::Interactive));
	}

	#[test]
	fn agent_auth_carries_no_secret() {
		// Agent auth validates to the fieldless method too — a running agent holds the key and
		// signs the challenge, so like interactive auth there is nothing on the form to capture,
		// no file to pick and no passphrase (§7).
		let form = ConnectForm {
			auth_kind: AuthKind::Agent,
			..base_form()
		};
		let params = form.validate().expect("valid agent form");
		assert!(matches!(params.auth, AuthMethod::Agent));
	}

	#[test]
	fn a_promptless_method_hides_the_credential_and_remember_stops() {
		// Both promptless methods (interactive, agent) drop the credential control, the optional
		// key-passphrase field and the "Remember" toggle from the Tab ring, since none of those
		// apply when the form captures no secret (§7).
		for auth in [AuthKind::Interactive, AuthKind::Agent] {
			assert!(!FormStop::Credential.is_applicable(auth));
			assert!(!FormStop::KeyPassphrase.is_applicable(auth));
			assert!(!FormStop::Remember.is_applicable(auth));
		}
	}

	#[test]
	fn key_auth_without_a_file_is_rejected() {
		let form = ConnectForm {
			auth_kind: AuthKind::Key,
			..base_form()
		};
		assert!(form.validate().is_err());
	}

	#[test]
	fn key_auth_carries_the_chosen_file() {
		let form = ConnectForm {
			auth_kind: AuthKind::Key,
			key_path: Some(PathBuf::from("/keys/id_ed25519")),
			..base_form()
		};
		let params = form.validate().expect("valid key form");
		match params.auth {
			AuthMethod::Key {
				path,
				passphrase,
				certificate,
			} => {
				assert_eq!(path, PathBuf::from("/keys/id_ed25519"));
				// An empty passphrase field must not become an empty secret — it stays
				// `None` so the interactive prompt still fires for an encrypted key (§7).
				assert!(passphrase.is_none());
				// No certificate chosen means plain public-key auth, exactly as before (§7).
				assert!(certificate.is_none());
			}
			other => panic!("expected key auth, got {other:?}"),
		}
	}

	#[test]
	fn key_auth_carries_a_chosen_certificate() {
		// A certificate set on the form rides through validation as `Some`, so the connect flow
		// presents the key-and-certificate pair (§7). It is orthogonal to the passphrase.
		let form = ConnectForm {
			auth_kind: AuthKind::Key,
			key_path: Some(PathBuf::from("/keys/id_ed25519")),
			cert_path: Some(PathBuf::from("/keys/id_ed25519-cert.pub")),
			..base_form()
		};
		let params = form.validate().expect("valid key form");
		match params.auth {
			AuthMethod::Key { certificate, .. } => {
				assert_eq!(
					certificate,
					Some(PathBuf::from("/keys/id_ed25519-cert.pub"))
				);
			}
			other => panic!("expected key auth, got {other:?}"),
		}
	}

	#[test]
	fn the_certificate_stop_is_reachable_only_under_key_auth() {
		// The certificate Browse button is a Tab stop only when key auth is selected; every
		// other method (which shows no key file) skips it, like the passphrase field.
		assert!(FormStop::Certificate.is_applicable(AuthKind::Key));
		for auth in [AuthKind::Password, AuthKind::Interactive, AuthKind::Agent] {
			assert!(!FormStop::Certificate.is_applicable(auth));
		}
	}

	#[test]
	fn key_auth_pre_seeds_a_typed_passphrase() {
		let form = ConnectForm {
			auth_kind: AuthKind::Key,
			key_path: Some(PathBuf::from("/keys/id_ed25519")),
			passphrase: "hunter2".to_string(),
			..base_form()
		};
		let params = form.validate().expect("valid key form");
		match params.auth {
			AuthMethod::Key { passphrase, .. } => {
				let secret = passphrase.expect("passphrase pre-seeded");
				assert_eq!(secret.expose(), "hunter2");
			}
			other => panic!("expected key auth, got {other:?}"),
		}
	}
}

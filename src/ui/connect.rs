// ui/connect.rs — the connection form (PLAN §10) and its input validation (§6.0).
//
// The form's field contents live in `ConnectForm`, owned by `App`. The `view`
// function renders those fields as iced widgets; `validate` turns the raw
// strings into a typed `ConnectParams` (or a clear error) before anything is
// sent to the network — "validate at the boundary" (§12).

use std::path::{Path, PathBuf};

use iced::Element;
use iced::widget::{button, column, radio, row, text, text_input};

use crate::app::Message;
use crate::bridge::{AuthMethod, ConnectParams};
use crate::secret::Secret;

/// The default SSH port, used when the user leaves the port field blank.
const DEFAULT_SSH_PORT: u16 = 22;

/// Which authentication method the form is set to. A tiny `Copy` enum so the
/// radio buttons can compare it by value and select the current one; `Password`
/// is the default. This is the UI-side mirror of `bridge::AuthMethod` — the form
/// holds a choice, `validate` turns it into the real method with its secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthKind {
	#[default]
	Password,
	Key,
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
	/// Chosen private-key file for `Key` auth (set by the file picker). Any
	/// passphrase for an encrypted key is asked for interactively at connect time
	/// (§7), so it is not kept in the form.
	pub key_path: Option<PathBuf>,
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
	/// a chosen file; its passphrase (if any) is collected interactively later.
	fn validate_auth(&self) -> Result<AuthMethod, String> {
		match self.auth_kind {
			AuthKind::Password => Ok(AuthMethod::Password(Secret::new(self.password.clone()))),
			AuthKind::Key => {
				let path = self
					.key_path
					.clone()
					.ok_or_else(|| "Choose a private-key file.".to_string())?;
				Ok(AuthMethod::Key { path })
			}
		}
	}
}

/// Render the connect form. Borrows the form so the text inputs can display the
/// current field values; returns an `Element` tied to that borrow.
pub fn view(form: &ConnectForm) -> Element<'_, Message> {
	column![
		text("cmote — SSH connect").size(24),
		labeled_input("Host", "example.com", &form.host, Message::HostChanged),
		labeled_input("Port", "22", &form.port, Message::PortChanged),
		labeled_input("User", "root", &form.user, Message::UserChanged),
		auth_selector(form.auth_kind),
		// The credential fields depend on the selected method — only the relevant
		// ones are shown, so the form stays uncluttered.
		auth_fields(form),
		button("Connect").on_press(Message::ConnectPressed),
	]
	.spacing(12)
	.padding(20)
	.max_width(420)
	.into()
}

/// The two radio buttons that choose the authentication method. `radio` needs a
/// `Copy + Eq` value; passing `Some(selected)` marks the current one as chosen.
fn auth_selector(selected: AuthKind) -> Element<'static, Message> {
	row![
		text("Auth").width(90),
		radio(
			"Password",
			AuthKind::Password,
			Some(selected),
			Message::AuthKindChanged
		),
		radio(
			"Key",
			AuthKind::Key,
			Some(selected),
			Message::AuthKindChanged
		),
	]
	.spacing(10)
	.into()
}

/// The credential fields for the selected method: a password box, or a key-file
/// chooser plus an optional passphrase box.
fn auth_fields(form: &ConnectForm) -> Element<'_, Message> {
	match form.auth_kind {
		AuthKind::Password => secure_input("Password", &form.password, Message::PasswordChanged),
		// No passphrase box here — an encrypted key prompts for it at connect time.
		AuthKind::Key => key_file_row(form.key_path.as_deref()),
	}
}

/// The key-file chooser: the chosen path (or a prompt) and a Browse button that
/// opens the native file picker. Returns an owned (`'static`) element — the path
/// is copied into a label, so nothing is borrowed from the form.
fn key_file_row(path: Option<&Path>) -> Element<'static, Message> {
	let label = match path {
		Some(path) => path.display().to_string(),
		None => "No key file selected".to_string(),
	};
	row![
		text("Key file").width(90),
		text(label),
		button("Browse…").on_press(Message::BrowseKeyPressed),
	]
	.spacing(10)
	.into()
}

/// A small helper: a label beside a text input, wired to a message constructor.
/// `on_input` takes `fn(String) -> Message`, so we pass the enum variant itself.
fn labeled_input<'a>(
	label: &'a str,
	placeholder: &'a str,
	value: &'a str,
	on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
	row![
		text(label).width(90),
		text_input(placeholder, value).on_input(on_input),
	]
	.spacing(10)
	.into()
}

/// A masked (password-style) input with a label. `.secure(true)` hides the
/// characters — used for both the password and the key passphrase.
fn secure_input<'a>(
	label: &'a str,
	value: &'a str,
	on_input: impl Fn(String) -> Message + 'a,
) -> Element<'a, Message> {
	row![
		text(label).width(90),
		text_input("", value).secure(true).on_input(on_input),
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
			AuthMethod::Key { path } => assert_eq!(path, PathBuf::from("/keys/id_ed25519")),
			other => panic!("expected key auth, got {other:?}"),
		}
	}
}

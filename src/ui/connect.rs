// ui/connect.rs — the connection form (PLAN §10) and its input validation (§6.0).
//
// The form's field contents live in `ConnectForm`, owned by `App`. The `view`
// function renders those fields as iced widgets; `validate` turns the raw
// strings into a typed `ConnectParams` (or a clear error) before anything is
// sent to the network — "validate at the boundary" (§12).

use iced::Element;
use iced::widget::{button, column, row, text, text_input};

use crate::app::Message;
use crate::bridge::ConnectParams;
use crate::secret::Secret;

/// The default SSH port, used when the user leaves the port field blank.
const DEFAULT_SSH_PORT: u16 = 22;

/// The connect form's editable fields. Plain owned `String`s: text inputs work
/// with strings, and validation converts them to typed values on submit.
#[derive(Debug, Default)]
pub struct ConnectForm {
	pub host: String,
	pub port: String,
	pub user: String,
	pub password: String,
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

		Ok(ConnectParams {
			host: host.to_string(),
			port,
			user: user.to_string(),
			// The password is wrapped so it is redacted in logs and wiped on
			// drop (§12). An empty password is allowed here — the server decides
			// whether it is acceptable.
			password: Secret::new(self.password.clone()),
		})
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
		// `.secure(true)` masks the characters — a password field, not plain text.
		row![
			text("Password").width(90),
			text_input("", &form.password)
				.secure(true)
				.on_input(Message::PasswordChanged),
		]
		.spacing(10),
		button("Connect").on_press(Message::ConnectPressed),
	]
	.spacing(12)
	.padding(20)
	.max_width(420)
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

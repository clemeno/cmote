// ui/mod.rs — view helpers (PLAN §10).
//
// Views are pure functions from state to an iced `Element`. Keeping them out of
// app.rs stops that file from growing without bound and groups the widget code
// by screen. Each submodule owns one screen's layout.

pub mod connect; // the connection form
pub mod dialog; // shared modal-dialog chrome (header / body / footer)
pub mod selection; // mouse text selection over the grid
pub mod terminal; // the live shell grid

use iced::widget::{button, column, text, text_editor, text_input};
use iced::{Color, Element};

use crate::app::Message;

/// The body copy for the host-key dialog (§8). The fingerprint is appended (on its own
/// line) when the dialog opens, so the whole message — fingerprint included — is one
/// selectable block. Public so `app` can seed it into the dialog buffer.
pub const HOST_KEY_DIALOG_BODY: &str = "This is the first connection to this server. Verify the fingerprint below matches the server you expect before trusting it.";

/// The body copy for the passphrase dialog (§7). Public so `app` can seed it into the
/// dialog buffer when the encrypted-key prompt opens.
pub const PASSPHRASE_DIALOG_BODY: &str =
	"This private key is protected by a passphrase. Enter it to unlock the key and continue.";

/// The widget id of the passphrase field. It is stable and shared: `passphrase_view`
/// tags the field with it, and `app` hands the same id to `text_input`'s focus
/// operation so the field is focused the instant the prompt appears — the user can
/// type immediately without first clicking it (§7). A plain `&'static str` is enough
/// because iced's widget `Id` is `From<&'static str>`.
pub const PASSPHRASE_INPUT_ID: &str = "passphrase-input";

/// The colour of the "wrong passphrase" hint (§7). A muted red that reads clearly on
/// the default light theme. This is about a *local* key-file passphrase, not remote
/// auth, so it is not a credential oracle (§12) — the key is decrypted and MAC-checked
/// on this machine, and telling the user their local passphrase was wrong is expected.
const PASSPHRASE_ERROR: Color = Color::from_rgb8(0xb0, 0x00, 0x00);

/// The error notice (§10): a generic message in the shared dialog chrome, with a
/// single Back button to the form. Its detail is logged, not shown, so nothing
/// sensitive leaks to the UI (§12). The message is a selectable body so it can be
/// copied; the close (✕) does the same as Back. `body` is `App::dialog_body`, seeded
/// with the error text when the screen opens.
pub fn error_view(body: &text_editor::Content) -> Element<'_, Message> {
	dialog::dialog(
		"Connection failed".to_owned(),
		Message::BackPressed,
		dialog::selectable_body(body),
		vec![button("Back").on_press(Message::BackPressed).into()],
	)
}

/// The first-contact host-key prompt (§8), in the shared dialog chrome: show the
/// fingerprint and make the user explicitly accept or reject. There is intentionally
/// no "always trust" shortcut — accepting pins this exact key, and any later change
/// is refused. Closing (✕) rejects, the safe default: an unverified host is not
/// trusted just because the dialog was dismissed. `body` (`App::dialog_body`) holds the
/// explanation plus the fingerprint as one selectable block, so the fingerprint can be
/// copied for out-of-band comparison.
pub fn host_key_view(body: &text_editor::Content) -> Element<'_, Message> {
	dialog::dialog(
		"Trust this host key?".to_owned(),
		Message::RejectHostKey,
		dialog::selectable_body(body),
		vec![
			button("Reject").on_press(Message::RejectHostKey).into(),
			button("Accept").on_press(Message::AcceptHostKey).into(),
		],
	)
}

/// The key-passphrase prompt (§7), shown only when the chosen private key turns
/// out to be encrypted. A masked field plus Unlock / Cancel; pressing Enter in
/// the field submits too. The typed value is owned by `App` and passed in for
/// display — this view stays pure. A wrong passphrase simply brings the prompt
/// back (the session re-asks), so no separate error state is needed here.
pub fn passphrase_view<'a>(
	value: &'a str,
	failed: bool,
	body: &'a text_editor::Content,
) -> Element<'a, Message> {
	// Only the message (`body`) is selectable; the field and the "incorrect" hint are
	// their own widgets. The hint is added only on a re-ask (`failed`), so the first
	// prompt stays clean. Building the column with `push` inserts it conditionally
	// without duplicating the layout; the shared chrome then wraps it as a dialog.
	let mut content = column![dialog::selectable_body(body)].spacing(12);

	if failed {
		content = content.push(
			text("That passphrase was not correct. Please try again.")
				.size(14)
				.color(PASSPHRASE_ERROR),
		);
	}

	content = content.push(
		text_input("Passphrase", value)
			.id(PASSPHRASE_INPUT_ID)
			.secure(true)
			.on_input(Message::PassphraseChanged)
			.on_submit(Message::PassphraseSubmitted),
	);

	dialog::dialog(
		"Unlock encrypted key?".to_owned(),
		Message::PassphraseCancelled,
		content.into(),
		vec![
			button("Unlock")
				.on_press(Message::PassphraseSubmitted)
				.into(),
			button("Cancel")
				.on_press(Message::PassphraseCancelled)
				.into(),
		],
	)
}

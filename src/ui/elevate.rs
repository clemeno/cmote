// ui/elevate.rs — the "Log in as…" dialog (PLAN §47).
//
// §45 gave a session a SET of shells, one per account, and the machinery to open another one. Its UX
// was then withdrawn: there was no way to start an elevation from the app at all, and no way back to
// the login account once one was running. This is the replacement, and it is one dialog rather than
// the four controls the withdrawn version spread the job over (a status-bar button, a context-menu
// item, an elevate dialog and an account switcher).
//
// It does three things, in the order a user meets them:
//
//   1. LISTS the accounts this session already has, so switching between them and closing one are
//      where switching and closing belong — beside each other, next to the account they act on.
//   2. ASKS who to become, and how: `sudo` or `su`, an account name, and the two questions that
//      decide what is remembered afterwards.
//   3. ANSWERS the credential conversation. sudo's questions arrive one at a time through
//      `SshEvent::ElevatePrompt`, and the dialog puts the remote's own wording to the user — so a
//      second factor asks for a code in the machine's words rather than in cmote's guess at them.
//
// The two checkboxes are the whole of §47's configuration, and they are not the same question:
//
//   "Do this on every connection" is a PREFERENCE, stored in `targets.json` as metadata (§14). It
//   says the next session should start this elevation by itself.
//
//   "Remember the password" is a SECRET, stored only in the sealed vault (§16), and it is a
//   deliberate relaxation of the rule §12 and §45 both wrote down — that a sudo password lives in
//   RAM and nowhere else. It is off by default, it is only ever honoured for an elevation that
//   SUCCEEDED, and it is refused outright for an account that needed more than one factor: a
//   one-time code is not a password and must never be kept as one (§45, `elevate::Handshake`).
//
// This module is the shape on screen. What the fields MEAN — the command composed from them, what
// counts as a question, which answer may be kept — is `crate::elevate`, which holds no iced type.

use iced::alignment::Vertical;
use iced::widget::{button, checkbox, column, container, row, text, text_input};
use iced::{Border, Color, Element, Length};

use crate::app::Message;
use crate::elevate::ElevateKind;
use crate::ui::dialog::{self, Card};

/// The account field's widget id, so `app` can focus it as the dialog opens — the first thing to
/// type when there is no saved account to accept.
pub const ACCOUNT_INPUT_ID: &str = "elevate-account";

/// The answer field's id. Focused when a question arrives, because the dialog is then a prompt and
/// nothing else on it is worth typing into.
pub const ANSWER_INPUT_ID: &str = "elevate-answer";

// The dialog's colours, the same palette the tunnels dialog uses (§27): a live account's green dot,
// a refusal's red, the muted grey for secondary lines, and the tint on the selected program.
const ACTIVE_FG: Color = Color::from_rgb8(0x6a, 0xbf, 0x6a);
const FAILED_FG: Color = Color::from_rgb8(0xd0, 0x6a, 0x6a);
const MUTED_FG: Color = Color::from_rgb8(0x90, 0x90, 0x90);
const FG: Color = Color::from_rgb8(0xe0, 0xe0, 0xe0);
const SELECTED_BG: Color = Color::from_rgb8(0x3d, 0x55, 0x77);

const BODY_SIZE: f32 = 14.0;

/// One account the dialog lists: which identity it is, what to call it, and whether it is the one on
/// screen (§45, §47).
///
/// A view of the tab's identity list rather than state of its own — the dialog owns nothing about the
/// accounts, so opening and closing it cannot lose one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
	pub identity: u64,
	/// What to call it: the login account's own name for the identity the session authenticated as,
	/// and the account elevated to for every other.
	pub label: String,
	/// Whether this identity's terminal is the one on screen.
	pub selected: bool,
	/// Whether this identity can be closed. The login identity cannot: ending it is what Disconnect
	/// does, and closing it here would be a second way to end the session (§45).
	pub closable: bool,
}

/// Where the dialog is in its conversation (§47).
///
/// Three stages, and the middle one is the reason this is a state rather than two booleans: while a
/// question is outstanding the dialog must show that question and nothing else, or the user is
/// offered a "Log in" button for an elevation that is already running.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum Stage {
	/// Asking who to become. The form is live and nothing has been sent.
	#[default]
	Asking,
	/// An elevation is out and the program has asked something. `label` is the remote's own wording
	/// (stripped and capped by `crate::elevate`), `refusal` its words about the previous answer when
	/// there was one, and `answer` what is being typed now.
	Answering {
		identity: u64,
		label: String,
		refusal: Option<String>,
		answer: String,
	},
	/// The last answer is in and the program has not spoken again yet. Distinct from `Asking` so the
	/// form cannot be re-submitted while a conversation is still running.
	Waiting { identity: u64 },
}

/// The dialog's own state (§47): what is being asked for, what has been answered, and the two
/// preferences the outcome is remembered under.
///
/// OWNED STATE, like `ui::forward::ForwardForm`, and it lives INSIDE the open modal for the same
/// reason: dismissing the dialog is what throws a half-typed elevation away. What must outlive it —
/// which accounts are open, what the target remembers — is on the tab and in `targets.json`.
#[derive(Debug, Default)]
pub struct ElevateForm {
	/// The account to become, as typed. Vetted by `elevate::valid_user` on submit, not on each
	/// keystroke: a name is half-typed for most of its life and complaining about it then would put
	/// an error under every field the moment it was touched.
	pub account: String,
	/// Which program does it. `sudo` by default, which is what a sudoers-managed machine expects.
	pub kind: ElevateKind,
	/// "Do this on every connection to this target" — the stored preference (§14).
	pub on_connect: bool,
	/// "Remember the password" — the vault opt-in (§16). See the module header for why these two are
	/// different questions.
	pub remember: bool,
	pub stage: Stage,
	/// The last thing that went wrong, shown under the form: a refused account name, a vault that
	/// could not be opened, or the remote's reason for ending an elevation.
	pub error: Option<String>,
}

impl ElevateForm {
	/// The form as a target's saved elevation opens it (§47): the account and program it remembers,
	/// with both checkboxes reflecting what is already stored, so the dialog opens saying what the
	/// next connection will do rather than blank.
	pub fn from_saved(saved: &crate::targets::Elevation) -> Self {
		Self {
			account: saved.account.clone(),
			kind: saved.kind,
			on_connect: saved.on_connect,
			remember: saved.remember_password,
			stage: Stage::Asking,
			error: None,
		}
	}

	/// Whether a question is outstanding — the one thing every caller asks this state.
	pub fn is_answering(&self) -> bool {
		matches!(self.stage, Stage::Answering { .. })
	}
}

/// Build the dialog card (§47): the accounts this session has, then either the form or the question.
pub fn panel(accounts: Vec<AccountRow>, form: &ElevateForm, card: Card) -> Element<'_, Message> {
	let mut body = column![].spacing(12);

	// The accounts always come first, even when there is only the login one. A dialog that showed
	// the list only once a second account existed would put the switcher somewhere the user had to
	// discover twice.
	let mut list = column![].spacing(6);
	for account in accounts {
		list = list.push(account_row(&account));
	}
	body = body.push(list);

	body = match &form.stage {
		Stage::Asking => body.push(ask_form(form)),
		Stage::Answering {
			label,
			refusal,
			answer,
			..
		} => body.push(question(label, refusal.as_deref(), answer)),
		Stage::Waiting { .. } => body.push(text("Working…").size(BODY_SIZE).color(MUTED_FG)),
	};

	if let Some(error) = &form.error {
		body = body.push(text(error).size(BODY_SIZE).color(FAILED_FG));
	}

	dialog::dialog(
		"Accounts".to_owned(),
		Message::ElevateClosed,
		body.into(),
		vec![button("Close").on_press(Message::ElevateClosed).into()],
		card,
	)
}

/// One account's row: a dot for whether it is on screen, its name, and — for an elevated one — a ✕
/// that ends it (§45, §47).
///
/// The name is a BUTTON for every account but the one showing, so switching is a click on the thing
/// switched to. The one showing is plain text: a button that does nothing when pressed is a worse
/// answer than no button.
fn account_row(account: &AccountRow) -> Element<'static, Message> {
	let dot = text(if account.selected { "●" } else { "○" })
		.size(BODY_SIZE)
		.color(if account.selected {
			ACTIVE_FG
		} else {
			MUTED_FG
		});

	let name: Element<'static, Message> = if account.selected {
		text(account.label.clone()).size(BODY_SIZE).color(FG).into()
	} else {
		button(text(account.label.clone()).size(BODY_SIZE))
			.padding([2, 6])
			.on_press(Message::IdentitySelected(account.identity))
			.style(|_theme, _status| button::Style {
				background: None,
				text_color: FG,
				..button::Style::default()
			})
			.into()
	};

	let mut cells = row![dot, name]
		.spacing(8)
		.align_y(Vertical::Center)
		.width(Length::Fill);
	if account.closable {
		cells = cells.push(
			button(text("✕").size(BODY_SIZE))
				.padding(2)
				.on_press(Message::IdentityClosed(account.identity))
				.style(|_theme, _status| button::Style {
					background: None,
					text_color: MUTED_FG,
					..button::Style::default()
				}),
		);
	}
	cells.into()
}

/// The "become another account" form: the program, the account, and the two preferences (§47).
fn ask_form(form: &ElevateForm) -> Element<'_, Message> {
	let selector = row![
		kind_button("sudo", ElevateKind::Sudo, form.kind),
		kind_button("su", ElevateKind::Su, form.kind),
	]
	.spacing(6);

	let account = text_input("account (root, deploy, …)", &form.account)
		.id(ACCOUNT_INPUT_ID)
		.on_input(Message::ElevateAccountEdited)
		.on_submit(Message::ElevateSubmitted)
		.size(BODY_SIZE);

	// The two checkboxes carry their own explanation, because one of them relaxes a promise the rest
	// of cmote keeps and a tick-box label is too small a place to say so.
	let on_connect = checkbox(form.on_connect)
		.label("Do this on every connection to this target")
		.on_toggle(Message::ElevateOnConnectToggled)
		.size(BODY_SIZE)
		.text_size(BODY_SIZE);
	let remember = checkbox(form.remember)
		.label("Remember the password (encrypted vault)")
		.on_toggle(Message::ElevateRememberToggled)
		.size(BODY_SIZE)
		.text_size(BODY_SIZE);
	let caveat = text(
		"A remembered password is kept in secrets.age, not in memory only — and never for an \
		 account that asked for a second factor.",
	)
	.size(BODY_SIZE - 2.0)
	.color(MUTED_FG);

	let go = button(text("Log in as…").size(BODY_SIZE)).on_press(Message::ElevateSubmitted);

	// A faint rule above the form separates it from the account list, the same divider the tunnels
	// dialog draws between its list and its add form.
	container(column![selector, account, on_connect, remember, caveat, go].spacing(8))
		.padding(iced::Padding {
			top: 12.0,
			right: 0.0,
			bottom: 0.0,
			left: 0.0,
		})
		.into()
}

/// The credential question, in the remote's own words (§45, §47).
///
/// `refusal` is shown ABOVE the question when the program said something about the previous answer,
/// which is the only thing that tells "wrong password, try again" from "now the second factor" — the
/// wording cannot, because sudo dresses every prompt in its stack in cmote's own `-p` text.
fn question<'a>(label: &'a str, refusal: Option<&'a str>, answer: &'a str) -> Element<'a, Message> {
	let mut body = column![].spacing(8);
	if let Some(refusal) = refusal {
		body = body.push(text(refusal).size(BODY_SIZE).color(FAILED_FG));
	}
	body = body.push(text(label).size(BODY_SIZE).color(FG));
	body = body.push(
		text_input("", answer)
			.id(ANSWER_INPUT_ID)
			.secure(true)
			.on_input(Message::ElevateAnswerEdited)
			.on_submit(Message::ElevateAnswerSubmitted)
			.size(BODY_SIZE),
	);
	body =
		body.push(button(text("Send").size(BODY_SIZE)).on_press(Message::ElevateAnswerSubmitted));
	container(body)
		.padding(iced::Padding {
			top: 12.0,
			right: 0.0,
			bottom: 0.0,
			left: 0.0,
		})
		.into()
}

/// One program choice, drawn as a toggle: the selected one is tinted (§27's own kind selector, so
/// the two dialogs pick things the same way).
fn kind_button(label: &str, kind: ElevateKind, selected: ElevateKind) -> Element<'_, Message> {
	let is_selected = kind == selected;
	button(text(label).size(BODY_SIZE))
		.padding([4, 10])
		.on_press(Message::ElevateKindPicked(kind))
		.style(move |_theme, _status| button::Style {
			background: is_selected.then(|| SELECTED_BG.into()),
			text_color: FG,
			border: Border {
				color: MUTED_FG,
				width: 1.0,
				radius: 4.0.into(),
			},
			..button::Style::default()
		})
		.into()
}

/// What the status bar's account button says (§47).
///
/// The login account alone reads "Account", because there is nothing to distinguish. Once a session
/// has more than one, the button names the one on screen — which is information the bar does not
/// otherwise carry: the centred endpoint is the account the session AUTHENTICATED as, and after an
/// elevation that is no longer who is typing. §45's read-only label was removed for duplicating the
/// endpoint; this earns its place by not duplicating it.
pub fn account_label(showing: Option<&str>) -> String {
	match showing {
		Some(account) => format!("Account: {account}"),
		None => "Account".to_owned(),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_button_names_the_account_only_when_there_is_one_to_name() {
		// The login account alone: nothing to say that the centred endpoint does not already say.
		assert_eq!(account_label(None), "Account");
		// An elevated one: the bar has no other place that says who is typing.
		assert_eq!(account_label(Some("root")), "Account: root");
	}

	#[test]
	fn a_saved_elevation_opens_the_form_it_was_saved_from() {
		// The dialog opens saying what the next connection will do, both checkboxes included, so
		// turning one off is one click rather than a re-type (§47).
		let saved = crate::targets::Elevation {
			kind: ElevateKind::Su,
			account: "deploy".to_owned(),
			on_connect: true,
			remember_password: true,
		};
		let form = ElevateForm::from_saved(&saved);
		assert_eq!(form.account, "deploy");
		assert_eq!(form.kind, ElevateKind::Su);
		assert!(form.on_connect);
		assert!(form.remember);
		assert_eq!(form.stage, Stage::Asking);
	}

	#[test]
	fn a_form_with_a_question_outstanding_says_so() {
		// The one thing every caller asks this state: a question outstanding means the form must not
		// be submitted again and the answer field is what has focus.
		let mut form = ElevateForm::default();
		assert!(!form.is_answering());
		form.stage = Stage::Answering {
			identity: 1,
			label: "cmote-password:".to_owned(),
			refusal: None,
			answer: String::new(),
		};
		assert!(form.is_answering());
		form.stage = Stage::Waiting { identity: 1 };
		assert!(!form.is_answering());
	}
}

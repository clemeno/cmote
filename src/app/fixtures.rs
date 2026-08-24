// app/fixtures.rs — the test fixtures more than one of `app`'s modules needs (PLAN §126).
//
// `app` was one 14 800-line file, so every test and every fixture lived beside every other one and
// "shared" meant nothing. Splitting the file (§126) made the question real: a fixture used by the
// accounts tests AND by the held-frame ones cannot live in either module, because a child module's
// items are invisible to its siblings.
//
// So they live here, `#[cfg(test)]` and `pub(super)`, which reads exactly right: visible to every
// module under `app` and to nothing outside it. A fixture used by ONE module stays in that module's
// own test block — this file is for the ones that cross.
//
// They are fixtures and not helpers: each returns a `Tab` (and usually its command channel) in a
// state a test can start from, which is the thing that was being copied by hand before there was
// anywhere to put it.

use std::cell::RefCell;
use std::rc::Rc;

use iced::widget::pane_grid;
use tokio::sync::mpsc;

use super::{
	App, AppScreen, AuthKind, Focus, Identity, Message, Region, Session, SessionState, SshCommand,
	SshEvent, Tab, Workspace, bridge, term, ui,
};

// An `App` with a live emulator and an open command channel, so `send_command` succeeds
// and its bytes can be read back off `rx`. The window starts focused with the ring on the
// shell — the baseline a program assumes — so a focus change is measured against it.
pub(super) fn app_with_terminal(rx_cap: usize) -> (Tab, mpsc::Receiver<SshCommand>) {
	let (tx, rx) = mpsc::channel(rx_cap);
	// A tab with a live terminal is a tab with a live SESSION (§134) — one value, so the fixture can
	// no longer set a terminal and forget the screen. It used to be able to, and its own comment
	// recorded what that cost: the fixture sat at its `Home` default for a long time and nothing
	// minded, because `on_key` only ever ran on the terminal screen in production and so never had
	// to ask. `keyboard_claim` does ask.
	let mut session = live_session("cme@rec:22", None);
	session.work.terminal = Some(term::Terminal::new(24, 80));
	let app = Tab {
		command_tx: Some(tx),
		screen: AppScreen::Session(session),
		window_focused: true,
		focus: Focus::Terminal,
		..Tab::default()
	};
	(app, rx)
}

// A tab mid-DIAL: a session exists and there is no shell yet, which is the state `Connected`, a
// host-key question and a passphrase ask all actually arrive in (§134).
//
// Before it, the tests that needed this wrote `connection = Some(…)` (and sometimes a terminal) on a
// `Tab` still sitting on the home screen — an endpoint with no session, which is precisely the shape
// §134 made unwritable. Nine of them, and every one passed, because nothing ever checked that the
// screen agreed with the fields.
pub(super) fn dialing_tab(endpoint: &str, cap: usize) -> (Tab, mpsc::Receiver<SshCommand>) {
	let (tx, rx) = mpsc::channel(cap);
	let app = Tab {
		command_tx: Some(tx),
		screen: AppScreen::Session(Session::dialing(
			endpoint.to_owned(),
			None,
			"connecting…".to_owned(),
		)),
		window_focused: true,
		..Tab::default()
	};
	(app, rx)
}

// A session that is already LIVE, for the fixtures that need one to hang state off (§134). Built
// through `Session::dialing` and then flipped, rather than by naming every field: a fixture that
// listed them would be a second constructor to keep in step with the real one.
pub(super) fn live_session(
	endpoint: &str,
	local: Option<crate::local::shells::ShellKind>,
) -> Session {
	let mut session = Session::dialing(endpoint.to_owned(), local, String::new());
	session.state = SessionState::Live;
	session
}

// One chunk of output from the LOGIN shell (§45) — the identity every test's terminal is,
// since none of them elevate unless they say so.
pub(super) fn shell_output(bytes: &[u8]) -> SshEvent {
	SshEvent::Output {
		identity: bridge::LOGIN_IDENTITY,
		bytes: bytes.to_vec(),
	}
}

// The next input queued for the shell, or `None` if nothing was sent.
pub(super) fn next_input(rx: &mut mpsc::Receiver<SshCommand>) -> Option<Vec<u8>> {
	match rx.try_recv() {
		Ok(SshCommand::Input(bytes)) => Some(bytes),
		_ => None,
	}
}

// A tab whose login shell is up and listed as its first identity, exactly as `Connected`
// leaves it. Every elevation test starts from here, since an identity to park INTO is what
// makes a switch possible at all.
pub(super) fn app_with_login_identity() -> (Tab, mpsc::Receiver<SshCommand>) {
	let (mut app, rx) = app_with_terminal(32);
	if let Some(session) = app.session_mut() {
		session.identities = vec![Identity {
			id: bridge::LOGIN_IDENTITY,
			account: None,
			ready: true,
			work: Workspace::default(),
		}];
		session.identity = bridge::LOGIN_IDENTITY;
		session.next_identity = 1;
	}
	(app, rx)
}

// Put a second account's shell on screen the way the SSH side reports one. It no longer goes
// through a dialog — that UX was withdrawn — so the identity is listed here as `elevate_submit`
// used to list it, and then announced live. Returns the new identity's number, which is also on
// screen when this returns.
pub(super) fn elevate_to(app: &mut Tab) -> u64 {
	let id = app.next_identity();
	if let Some(session) = app.session_mut() {
		session.next_identity += 1;
		session.identities.push(Identity {
			id,
			account: Some("root".to_owned()),
			ready: false,
			work: Workspace::default(),
		});
	}
	let _task = app.on_ssh_event(SshEvent::IdentityEnded {
		identity: u64::MAX, // a stray event for nothing, to prove it disturbs nothing
		reason: None,
	});
	let _task = app.on_ssh_event(SshEvent::IdentityReady {
		identity: id,
		factors: 1,
	});
	id
}

// A tab with a login shell up and a saved target behind it, which is what every §47 test needs:
// the preference and the password flag live on the TARGET, so a session with no target to write
// to would exercise half of each path.
pub(super) fn app_with_saved_target() -> (Tab, mpsc::Receiver<SshCommand>) {
	let (app, rx) = app_with_login_identity();
	// The endpoint is the session's now (§134), and `app_with_terminal` already dialed this one.
	app.targets
		.borrow_mut()
		.upsert_on_connect("rec", 22, "cme", AuthKind::Password, None, None);
	(app, rx)
}

// What the target remembers about becoming another account, or `None`.
pub(super) fn saved_elevation(app: &Tab) -> Option<crate::targets::Elevation> {
	app.targets
		.borrow()
		.find("cme@rec:22")
		.and_then(|target| target.elevate.clone())
}

// Ask for an elevation the way the user does: open the dialog, type the account, tick what is
// wanted, submit. Driving it through `update` is what pins the wiring — an edit reaching a closed
// dialog would silently do nothing and these tests would catch it.
pub(super) fn ask_to_become(app: &mut Tab, account: &str, on_connect: bool, remember: bool) {
	if app.elevate_form_mut().is_none() {
		let _focus = app.update(Message::AccountPressed);
	}
	let _ = app.update(Message::ElevateAccountEdited(account.to_owned()));
	if on_connect {
		let _ = app.update(Message::ElevateOnConnectToggled(true));
	}
	if remember {
		let _ = app.update(Message::ElevateRememberToggled(true));
	}
	let _ = app.update(Message::ElevateSubmitted);
}

// The commands queued for the SSH task, drained in order.
pub(super) fn drain(rx: &mut mpsc::Receiver<SshCommand>) -> Vec<SshCommand> {
	let mut out = Vec::new();
	while let Ok(command) = rx.try_recv() {
		out.push(command);
	}
	out
}

// The next command queued for the SSH worker, or `None` if nothing was sent. Broader than
// `next_input`, since the forward flow sends `AddForward` / `RemoveForward`, not `Input`.
pub(super) fn next_command(rx: &mut mpsc::Receiver<SshCommand>) -> Option<SshCommand> {
	rx.try_recv().ok()
}

// A bare app with one undivided region holding one home tab, and empty shared state, so the
// tab-strip bookkeeping (§26) is exercised without an iced runtime or the disk. The `Task`s these
// calls return are dropped — only the tab list and active index are under test.
pub(super) fn tab_app() -> App {
	let targets = Rc::new(RefCell::new(crate::targets::Targets::default()));
	let vault = Rc::new(RefCell::new(None));
	let first = Tab::home(targets.clone(), vault.clone(), 0, iced::Size::default());
	// The order starts on the tab already on screen — id 0, the home tab built above (§37).
	let (regions, focus) = pane_grid::State::new(Region::new(first));
	App {
		regions,
		focus,
		// A plausible window, so the split tests have room to divide and the overlay tests have
		// bounds to be clamped into (§48).
		window: iced::Size::new(1200.0, 800.0),
		next_id: 1,
		targets,
		vault,
		pending_close: None,
		pending_editor_close: None,
		quit: None,
		overlay: ui::dialog::Card::default(),
		// Default (nothing remembered): `save` is a no-op on default, so a quit test never
		// touches the disk (§31).
		settings: crate::settings::Settings::default(),
		// The pointer has not moved and no seam has been pressed — the state a divider
		// double-click starts from (§48).
		pointer: iced::Point::ORIGIN,
		seam_clicks: ui::selection::Clicks::default(),
		strip_menu: None,
	}
}

/// That region, mutably, for the tests that arrange a strip rather than assert on one (§48).
pub(super) fn strip_mut(app: &mut App) -> &mut Region {
	let pane = app.focused_pane();
	app.regions.get_mut(pane).expect("the focused region")
}

/// Two saved targets, `root@web-01:22` and `root@db-01:22`, on a tab sitting at the home
/// list — enough to have one row the filter keeps and one it hides (§49).
pub(super) fn tab_with_targets() -> Tab {
	let tab = Tab::default();
	{
		let mut targets = tab.targets.borrow_mut();
		targets.upsert_on_connect("web-01", 22, "root", AuthKind::Password, None, None);
		targets.upsert_on_connect("db-01", 22, "root", AuthKind::Password, None, None);
	}
	tab
}

// A key-press event for the terminal handler. `text: None` is fine for the named keys these
// tests use (Enter / PageUp encode from the key itself), and the physical code is non-numpad
// so it never trips the NumLock special-case in `keymap`.
pub(super) fn key_press(
	named: iced::keyboard::key::Named,
	code: iced::keyboard::key::Code,
	modifiers: iced::keyboard::Modifiers,
) -> iced::keyboard::Event {
	iced::keyboard::Event::KeyPressed {
		key: iced::keyboard::Key::Named(named),
		modified_key: iced::keyboard::Key::Named(named),
		physical_key: iced::keyboard::key::Physical::Code(code),
		location: iced::keyboard::Location::Standard,
		modifiers,
		text: None,
		repeat: false,
	}
}

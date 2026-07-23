// app.rs — the iced application, written in the Elm architecture (PLAN §10).
//
// Three pieces make up an iced app and they are all pure/explicit:
//   * State   — one struct (`App`) owns EVERYTHING the UI can show.
//   * Message — one enum listing every event that can happen.
//   * update  — `fn(&mut State, Message)`: the ONLY place state changes.
//   * view    — `fn(&State) -> Element`: a pure render of the current state.
//
// There is no hidden widget tree and no global mutable state. Every change
// flows through `update`, and the compiler forces us to handle each `Message`.

use iced::Element;
use iced::widget::text;
use tokio::sync::mpsc;

use crate::bridge::{self, SshCommand, SshEvent};
use crate::ui;

/// Build and start the iced runtime. Called from `main`.
pub fn run() -> iced::Result {
	// The functional builder (iced 0.14): the first argument is the "boot"
	// function that produces the initial `(State, Task)` — here `App::new`. Then
	// the update and view functions. `.title` / `.subscription` are builder
	// methods, and `.run()` starts the event loop.
	iced::application(App::new, App::update, App::view)
		.title("cmote")
		.subscription(App::subscription)
		.run()
}

/// Which screen the single window is currently showing. This is the small state
/// machine from PLAN §10 — every transition happens in `update`.
#[derive(Debug, Default)]
pub enum Screen {
	/// The connection form (host / port / user / auth). This is where we start.
	#[default]
	Connect,
	/// Handshake and authentication in progress; `status` is a human-readable
	/// step for the UI ("connecting", "verifying host key", "authenticating").
	Connecting { status: String },
	/// First contact with an unknown host: the server's key fingerprint is shown
	/// and the user must accept or reject before the handshake continues (§8).
	ConfirmHostKey { fingerprint: String },
	/// A live shell: the vt100 grid fills the window.
	Terminal,
	/// A terminal failure. The message is generic and never leaks secrets (§12).
	Error(String),
}

/// The whole application state. Owned in one place; nothing else mutates it.
#[derive(Debug, Default)]
pub struct App {
	/// Which screen is visible.
	pub screen: Screen,
	/// The connect form's field contents. Lives here so it survives navigating
	/// to an error screen and back without losing what the user typed.
	pub form: ui::connect::ConnectForm,
	/// Channel to the SSH task. `None` until the worker starts and delivers it
	/// via `SshEvent::Ready`; `update` sends `SshCommand`s through it.
	command_tx: Option<mpsc::Sender<SshCommand>>,
}

/// Every event the app can react to. UI events come from widgets; `Ssh` events
/// are surfaced from the background tokio task via a subscription (§4).
#[derive(Debug, Clone)]
pub enum Message {
	// --- connect form field edits ---
	HostChanged(String),
	PortChanged(String),
	UserChanged(String),
	PasswordChanged(String),
	// --- form actions ---
	ConnectPressed,
	BackPressed,
	// --- host-key confirmation (§8) ---
	AcceptHostKey,
	RejectHostKey,
	// --- events bubbled up from the SSH task via the subscription (§4) ---
	Ssh(SshEvent),
}

impl App {
	/// Construct the initial state and the first `Task`. iced calls this once at
	/// startup. We start on the Connect screen with no work to do, so the task
	/// is empty.
	fn new() -> (Self, iced::Task<Message>) {
		(Self::default(), iced::Task::none())
	}

	/// The heart of the Elm loop: apply one `Message` to the state. Returns a
	/// `Task` for any async follow-up work (none yet in the skeleton).
	fn update(&mut self, message: Message) -> iced::Task<Message> {
		match message {
			Message::HostChanged(value) => self.form.host = value,
			Message::PortChanged(value) => self.form.port = value,
			Message::UserChanged(value) => self.form.user = value,
			Message::PasswordChanged(value) => self.form.password = value,
			Message::ConnectPressed => self.on_connect_pressed(),
			Message::BackPressed => self.screen = Screen::Connect,
			Message::AcceptHostKey => self.on_host_key_decision(true),
			Message::RejectHostKey => self.on_host_key_decision(false),
			Message::Ssh(event) => self.on_ssh_event(event),
		}
		iced::Task::none()
	}

	/// Validate the form, then send a `Connect` command to the SSH task. Cheap
	/// validation fails fast to the error screen.
	fn on_connect_pressed(&mut self) {
		let params = match self.form.validate() {
			Ok(params) => params,
			Err(reason) => {
				self.screen = Screen::Error(reason);
				return;
			}
		};

		let status = format!("connecting to {}:{}…", params.host, params.port);
		if self.send_command(SshCommand::Connect(params)) {
			self.screen = Screen::Connecting { status };
		}
	}

	/// Relay the user's host-key accept/reject to the SSH task (§8). On accept we
	/// go back to a connecting status; on reject the refused handshake will
	/// surface its own error.
	fn on_host_key_decision(&mut self, accept: bool) {
		if self.send_command(SshCommand::HostKeyResponse(accept)) && accept {
			self.screen = Screen::Connecting {
				status: "authenticating…".to_string(),
			};
		}
	}

	/// Send one command to the SSH task. Returns whether it was sent; a
	/// missing/closed channel becomes a visible error rather than a silent drop.
	/// `try_send` is non-blocking, so it is safe on the synchronous GUI thread.
	fn send_command(&mut self, command: SshCommand) -> bool {
		match &self.command_tx {
			Some(sender) => match sender.try_send(command) {
				Ok(()) => true,
				Err(error) => {
					self.screen = Screen::Error(format!("Could not reach the SSH worker: {error}"));
					false
				}
			},
			None => {
				self.screen = Screen::Error("SSH worker is not ready yet.".to_string());
				false
			}
		}
	}

	/// React to an event from the SSH task. Only the shape is here for now; the
	/// task that produces these events is added in a later step.
	fn on_ssh_event(&mut self, event: SshEvent) {
		match event {
			SshEvent::Ready(sender) => self.command_tx = Some(sender),
			SshEvent::Connecting => {
				self.screen = Screen::Connecting {
					status: "connecting…".to_string(),
				}
			}
			SshEvent::HostKey(fingerprint) => self.screen = Screen::ConfirmHostKey { fingerprint },
			SshEvent::NeedPassphrase => {
				self.screen = Screen::Connecting {
					status: "key needs a passphrase".to_string(),
				}
			}
			SshEvent::Connected => self.screen = Screen::Terminal,
			SshEvent::Output(_bytes) => { /* fed to the vt100 parser in §9 */ }
			SshEvent::Disconnected => self.screen = Screen::Connect,
			SshEvent::Error(message) => self.screen = Screen::Error(message),
		}
	}

	/// Render the current screen. Pure: it only reads state and returns widgets.
	fn view(&self) -> Element<'_, Message> {
		match &self.screen {
			Screen::Connect => ui::connect::view(&self.form),
			Screen::Connecting { status } => text(status).into(),
			Screen::ConfirmHostKey { fingerprint } => ui::host_key_view(fingerprint),
			Screen::Terminal => ui::terminal::view(),
			Screen::Error(message) => ui::error_view(message),
		}
	}

	/// Streams the app listens to. The SSH worker's outbound events (§4) are
	/// mapped into `Message::Ssh(..)` so `update` handles them like any other.
	fn subscription(&self) -> iced::Subscription<Message> {
		bridge::subscription().map(Message::Ssh)
	}
}

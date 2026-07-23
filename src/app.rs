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
	// --- events bubbled up from the SSH task (wired in a later step) ---
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
			Message::Ssh(event) => self.on_ssh_event(event),
		}
		iced::Task::none()
	}

	/// Validate the form, then send a `Connect` command to the SSH task. Cheap
	/// validation fails fast to the error screen; a missing/closed channel is
	/// also surfaced rather than silently dropped.
	fn on_connect_pressed(&mut self) {
		let params = match self.form.validate() {
			Ok(params) => params,
			Err(reason) => {
				self.screen = Screen::Error(reason);
				return;
			}
		};

		let status = format!("connecting to {}:{}…", params.host, params.port);
		match &self.command_tx {
			Some(sender) => {
				// `try_send` is non-blocking — safe to call from the synchronous
				// GUI thread (no tokio runtime needed here). A full or closed
				// channel becomes a visible error, never a silent no-op.
				if let Err(error) = sender.try_send(SshCommand::Connect(params)) {
					self.screen = Screen::Error(format!("Could not start connection: {error}"));
					return;
				}
				self.screen = Screen::Connecting { status };
			}
			None => self.screen = Screen::Error("SSH worker is not ready yet.".to_string()),
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
			SshEvent::HostKey(fingerprint) => {
				self.screen = Screen::Connecting {
					status: format!("verify host key: {fingerprint}"),
				}
			}
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

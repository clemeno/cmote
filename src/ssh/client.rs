// ssh/client.rs — the russh client Handler and the tokio task loop (PLAN §6).
//
// `run` is the task loop: it drains `SshCommand`s from the GUI and emits
// `SshEvent`s back. The real body (build a russh client, handshake, host-key
// gate §8, authenticate §7, open a shell with a pty, pump bytes) drops in here
// in the next slice. For now it proves the bridge round-trips end to end.

use tokio::sync::mpsc::{Receiver, Sender};

use crate::bridge::{SshCommand, SshEvent};

/// The SSH task loop. Owns the connection (once implemented) and both channel
/// ends. Runs on the dedicated tokio runtime thread (§4); returns when the
/// command channel closes (i.e. the GUI dropped its sender on exit).
pub async fn run(mut commands: Receiver<SshCommand>, events: Sender<SshEvent>) {
	while let Some(command) = commands.recv().await {
		match command {
			SshCommand::Connect(params) => {
				// ponytail: real russh connect/auth/shell lands in the next
				// slice. For now, prove the round-trip: acknowledge, then report
				// a clear "not wired yet" error back to the GUI.
				let _ = events.send(SshEvent::Connecting).await;
				let message = format!(
					"SSH client not wired yet — would connect to {}@{}:{}",
					params.user, params.host, params.port
				);
				let _ = events.send(SshEvent::Error(message)).await;
			}
			// No live session yet, so input and resize have nowhere to go.
			SshCommand::Input(_bytes) => {}
			SshCommand::Resize { .. } => {}
			SshCommand::Disconnect => {
				let _ = events.send(SshEvent::Disconnected).await;
			}
		}
	}
}

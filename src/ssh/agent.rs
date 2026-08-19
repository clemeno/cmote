// ssh/agent.rs — public-key authentication delegated to a running SSH agent (PLAN §7).
//
// An SSH agent holds the user's private keys already unlocked and signs auth challenges on
// request, so the private key never leaves the agent and cmote never touches key material.
// This is what the "Agent" method on the connect form uses. Two agent worlds are covered:
//
//   * Windows — the OpenSSH agent (a named pipe, `\\.\pipe\openssh-ssh-agent`, also reachable
//     through `SSH_AUTH_SOCK` when that variable points at a pipe) and PuTTY's Pageant. We try
//     them in that order and use the first that answers with at least one usable key.
//   * macOS / Unix — the classic `ssh-agent`, found via the `SSH_AUTH_SOCK` Unix socket.
//
// russh already wires the hard part: `AgentClient<S>` implements russh's `Signer`, so handing
// `&mut agent` to `authenticate_publickey_with` makes the agent sign each challenge. The
// different transports (pipe / Pageant / socket) all collapse to one type via `.dynamic()`,
// which boxes the stream — tokio implements `AsyncRead`/`AsyncWrite` for `Box<dyn _>`, so the
// boxed client is still a valid `Signer`.
//
// Failure is kept honest about the no-credential-oracle rule (§12): once we have actually
// offered a key and the server rejected it, the caller turns that into the same single generic
// "authentication failed". The messages we DO raise here — "no SSH agent found", "the agent has
// no keys" — are about the LOCAL agent's availability, not about which server credential was
// wrong, so they leak nothing and help the user fix their own setup.

use anyhow::{Result, bail};
use russh::MethodSet;
use russh::client::{self, AuthResult};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};

use crate::ssh::auth::Outcome;
use crate::ssh::client::Handler;

/// One agent transport erased to a single type. `.dynamic()` boxes whichever concrete stream a
/// source uses (named pipe, Pageant, or Unix socket) so the connect-and-try loop is uniform.
type BoxedAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// The Windows OpenSSH agent's fixed named-pipe path. Only referenced on Windows.
#[cfg(windows)]
const OPENSSH_AGENT_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";

/// Authenticate `session` for `user` with keys held by a running SSH agent (§7). Every agent
/// source available on this platform is asked, in priority order, for its identities; each
/// public key is offered to the server until one is accepted.
///
/// Returns `Outcome::Success` when a key was accepted (the caller may still chain into a second
/// factor), or `Outcome::Failure` when keys were offered but the server took none (the caller
/// collapses that into one generic error). It errors out only when there is nothing to try at
/// all: no agent answered, or an agent answered but held no usable key.
pub(crate) async fn try_agent(
	session: &mut client::Handle<Handler>,
	user: &str,
) -> Result<Outcome> {
	let sources = connect_sources().await;
	if sources.is_empty() {
		// No agent answered on any transport — a local setup problem, not a server secret (§12).
		bail!("no SSH agent found (start the OpenSSH agent or Pageant, or set SSH_AUTH_SOCK)");
	}

	// Whether we ever got as far as offering a key. If not, the agent(s) held nothing we could
	// use, which is again a local matter rather than a rejected credential.
	let mut offered_a_key = false;
	// The server's remaining methods after the last rejected key, carried forward so the caller
	// can still chain into keyboard-interactive if the server offers it as a second factor (§7).
	let mut last_remaining: Option<MethodSet> = None;

	for mut agent in sources {
		// A source that cannot list its identities is skipped rather than fatal — the next
		// source (say Pageant after the OpenSSH pipe) may still have keys.
		let Ok(identities) = agent.request_identities().await else {
			continue;
		};

		for identity in identities {
			// Only plain public keys are offered here. A certificate identity needs the
			// certificate-specific auth path, which is a separate deferred feature (§7), so it is
			// left alone rather than silently downgraded to its bare key.
			let AgentIdentity::PublicKey { key, .. } = &identity else {
				continue;
			};
			offered_a_key = true;

			// RSA keys must pick a signature hash; ask the server which it accepts and use the
			// strongest, exactly as the key-file path does. Other key types ignore this.
			let hash_alg = if key.algorithm().is_rsa() {
				session.best_supported_rsa_hash().await?.flatten()
			} else {
				None
			};

			// The agent (`&mut agent`) is the signer: russh sends it the challenge and it replies
			// with a signature made by the private key it holds. A `Failure` here is the ordinary
			// "server does not know this key" — try the next identity. An `Err` is a broken agent
			// or transport, not a rejected credential, so it aborts with a generic message.
			match session
				.authenticate_publickey_with(user, key.clone(), hash_alg, &mut agent)
				.await
			{
				Ok(AuthResult::Success) => return Ok(Outcome::Success),
				Ok(AuthResult::Failure {
					remaining_methods, ..
				}) => last_remaining = Some(remaining_methods),
				Err(_) => bail!("authentication request failed"),
			}
		}
	}

	if !offered_a_key {
		// An agent answered but held no usable key (empty, or certificates only) — local setup.
		bail!("the SSH agent has no keys to offer");
	}

	// Keys were offered and all were rejected. Hand the server's remaining methods back so the
	// caller's chaining loop can follow into keyboard-interactive if that is still on offer (§7);
	// otherwise the caller collapses this into one generic "authentication failed" (§12).
	Ok(Outcome::Failure {
		remaining: last_remaining.unwrap_or_else(MethodSet::empty),
	})
}

/// Connect to every SSH-agent source available on this platform, in priority order, returning a
/// client for each that answered. An unreachable source is simply absent from the list — a
/// missing Pageant or a stopped agent is normal, not an error. Order matters: the first source
/// holding a usable key wins in `authenticate`, so the platform's native agent comes first.
async fn connect_sources() -> Vec<BoxedAgent> {
	let mut sources = Vec::new();

	#[cfg(windows)]
	{
		// 1. `SSH_AUTH_SOCK`, when it names a pipe — some Windows setups point it at the OpenSSH
		// agent (or a proxy) this way. A value that is not a real pipe just fails to connect.
		if let Ok(sock) = std::env::var("SSH_AUTH_SOCK")
			&& let Ok(agent) = AgentClient::connect_named_pipe(&sock).await
		{
			sources.push(agent.dynamic());
		}
		// 2. The OpenSSH agent's fixed pipe.
		if let Ok(agent) = AgentClient::connect_named_pipe(OPENSSH_AGENT_PIPE).await {
			sources.push(agent.dynamic());
		}
		// 3. PuTTY's Pageant.
		if let Ok(agent) = AgentClient::connect_pageant().await {
			sources.push(agent.dynamic());
		}
	}

	#[cfg(unix)]
	{
		// The classic `ssh-agent`, located by `SSH_AUTH_SOCK`.
		if let Ok(agent) = AgentClient::connect_env().await {
			sources.push(agent.dynamic());
		}
	}

	sources
}

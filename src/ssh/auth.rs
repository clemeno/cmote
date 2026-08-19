// ssh/auth.rs — authentication method selection, attempts, and 2FA chaining (PLAN §7).
//
// The user picks ONE method on the form — password, key, or keyboard-interactive — and that
// goes first. After it, we follow the server: if it still offers `keyboard-interactive` we run
// that prompt loop too, which is what makes two-factor auth work without the user hand-picking
// the exact mechanism. Two shapes are covered by the same code path:
//   * a fallback   — our method was not offered, but the server does challenge-response;
//   * a second factor — a key or password PARTIALLY succeeds, then a one-time code is asked.
//
// keyboard-interactive itself (RFC 4256) is a conversation: the server sends a batch of
// prompts (each with an `echo` hint — masked when false), the client answers all of them at
// once, and this repeats until the server accepts or rejects. A message-only batch (no
// prompts) is answered with an empty response set without troubling the user.
//
// Auth failure is deliberately a single generic error — we never reveal which field, factor,
// or user was wrong (no credential oracle, §12).

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg};
use russh::{MethodKind, MethodSet};
use tokio::sync::mpsc;

use crate::bridge::{AuthMethod, ConnectParams, InteractivePrompt, SshEvent};
use crate::secret::Secret;
use crate::ssh::agent;
use crate::ssh::client::{Handler, SessionMsg};
use crate::ssh::keyfile::{self, Loaded};

/// How many times to re-prompt for a private-key passphrase before giving up (§7).
const MAX_PASSPHRASE_ATTEMPTS: u32 = 3;

/// How many keyboard-interactive conversations to run before giving up (§7). Bounded like
/// OpenSSH's `MaxAuthTries` so a server that keeps re-offering the method cannot loop the
/// prompt forever; the user can also cancel any prompt to abort sooner.
const MAX_AUTH_ATTEMPTS: u32 = 3;

/// The result of one authentication attempt, normalised across the two russh return types
/// (`AuthResult` for password/publickey, `KeyboardInteractiveAuthResponse` for the interactive
/// loop) so the chaining loop can reason about both the same way. We keep only what the loop
/// needs: whether it is done, and — on failure — which methods the server will still consider.
pub(crate) enum Outcome {
	Success,
	Failure { remaining: MethodSet },
}

impl From<client::AuthResult> for Outcome {
	fn from(result: client::AuthResult) -> Self {
		match result {
			client::AuthResult::Success => Outcome::Success,
			// `partial_success` is not tracked: we retry keyboard-interactive whenever the
			// server still lists it, whether this was a genuine first factor or a plain reject.
			client::AuthResult::Failure {
				remaining_methods, ..
			} => Outcome::Failure {
				remaining: remaining_methods,
			},
		}
	}
}

/// Authenticate the freshly handshaken `session` for `params`, running the chosen method and
/// then chaining into keyboard-interactive as the server directs (§7). Returns `Ok(())` once
/// the server has fully accepted us, or a generic error otherwise.
pub(crate) async fn authenticate(
	session: &mut client::Handle<Handler>,
	params: &ConnectParams,
	events: &mpsc::Sender<SshEvent>,
	to_session_rx: &mut mpsc::Receiver<SessionMsg>,
) -> Result<()> {
	// The method the user chose goes first.
	let mut outcome = match &params.auth {
		AuthMethod::Password(password) => {
			let result = session
				.authenticate_password(params.user.as_str(), password.expose())
				.await
				.context("authentication request failed")?;
			Outcome::from(result)
		}

		AuthMethod::Key {
			path,
			passphrase,
			certificate,
		} => {
			// Load the key. A passphrase pre-seeded from the form (§14) is tried first;
			// otherwise an encrypted key prompts interactively (§7). `clone` because the
			// passphrase is borrowed from `params` and `resolve_key` needs to own it.
			let key = resolve_key(path, passphrase.clone(), events, to_session_rx).await?;
			let result = match certificate {
				// No certificate: plain public-key auth, exactly as before.
				None => {
					// RSA keys must pick a signature hash: OpenSSH offers rsa-sha2-512,
					// rsa-sha2-256, or the legacy ssh-rsa (SHA-1). Ask the server which it
					// accepts and use the strongest; other key types ignore this.
					let hash_alg = if key.algorithm().is_rsa() {
						session.best_supported_rsa_hash().await?.flatten()
					} else {
						None
					};
					let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg);
					session
						.authenticate_publickey(params.user.as_str(), key)
						.await
						.context("authentication request failed")?
				}
				// A certificate: present the private key AND its CA-signed certificate (§7). The
				// key still signs the challenge; the certificate is the extra blob that lets the
				// server trust the signature via the CA rather than a per-key `authorized_keys`
				// entry. russh derives the signature algorithm from the certificate itself (so an
				// RSA certificate carries its own `rsa-sha2-*` choice), hence no separate hash
				// negotiation on this path. A certificate that will not load is a hard error
				// surfaced to the user, not a silent fall-back to bare-key auth.
				Some(cert_path) => {
					let cert = keyfile::load_certificate(cert_path)?;
					session
						.authenticate_openssh_cert(params.user.as_str(), Arc::new(key), cert)
						.await
						.context("authentication request failed")?
				}
			};
			Outcome::from(result)
		}

		// Explicit keyboard-interactive: straight into the prompt loop, no primary secret.
		AuthMethod::Interactive => {
			keyboard_interactive(session, params.user.as_str(), events, to_session_rx).await?
		}

		// Public-key auth via a running SSH agent / Pageant (§7). The agent holds the keys and
		// signs each challenge itself, so there is no secret to prompt for and no key file to
		// load — cmote never sees the private key. A success may still chain into a second
		// factor below, exactly like the key path.
		AuthMethod::Agent => agent::try_agent(session, params.user.as_str()).await?,
	};

	// Then follow the server into keyboard-interactive for as long as it keeps offering the
	// method and attempts remain (§7). An interactive primary counts as the first attempt, so
	// a wrong OTP re-asks up to the same bound rather than an extra time.
	let mut attempts = u32::from(matches!(params.auth, AuthMethod::Interactive));
	loop {
		match outcome {
			Outcome::Success => return Ok(()),
			Outcome::Failure { remaining } => {
				if should_try_interactive(&remaining, attempts) {
					attempts += 1;
					outcome =
						keyboard_interactive(session, params.user.as_str(), events, to_session_rx)
							.await?;
				} else {
					// Nothing left to try, or we have retried enough: one generic failure, with
					// no hint about which factor was wrong (no credential oracle, §12).
					bail!("authentication failed");
				}
			}
		}
	}
}

/// Whether to (re)try keyboard-interactive next: the server still lists it and we have not used
/// up our attempts. Pulled out so the bounded-retry rule is unit-testable without a live server.
fn should_try_interactive(remaining: &MethodSet, attempts: u32) -> bool {
	// `MethodSet` derefs to `[MethodKind]`, so a plain slice `contains` answers the membership.
	remaining.contains(&MethodKind::KeyboardInteractive) && attempts < MAX_AUTH_ATTEMPTS
}

/// Run one whole keyboard-interactive conversation (§7): open it, answer each batch of prompts
/// the server sends, and return the terminal outcome. The GUI is asked once per non-empty batch
/// and its answers ride back as `SessionMsg::Interactive`.
async fn keyboard_interactive(
	session: &mut client::Handle<Handler>,
	user: &str,
	events: &mpsc::Sender<SshEvent>,
	to_session_rx: &mut mpsc::Receiver<SessionMsg>,
) -> Result<Outcome> {
	// Open the exchange. `None` submethods lets the server choose the scheme.
	let mut response = session
		.authenticate_keyboard_interactive_start(user, None::<String>)
		.await
		.context("keyboard-interactive request failed")?;

	loop {
		match response {
			client::KeyboardInteractiveAuthResponse::Success => return Ok(Outcome::Success),
			client::KeyboardInteractiveAuthResponse::Failure {
				remaining_methods, ..
			} => {
				return Ok(Outcome::Failure {
					remaining: remaining_methods,
				});
			}
			client::KeyboardInteractiveAuthResponse::InfoRequest {
				name,
				instructions,
				prompts,
			} => {
				// A request with no prompts is the server merely showing a message (RFC 4256):
				// answer it with an empty response set and move on, rather than popping an empty
				// dialog. Otherwise surface the prompts and wait for one answer each.
				let answers: Vec<String> = if prompts.is_empty() {
					Vec::new()
				} else {
					// Mirror russh's prompts into the type the GUI owns, then ask.
					let shown = prompts
						.iter()
						.map(|prompt| InteractivePrompt {
							label: prompt.prompt.clone(),
							echo: prompt.echo,
						})
						.collect();
					let _ = events
						.send(SshEvent::Interactive {
							name,
							instructions,
							prompts: shown,
						})
						.await;
					let secrets = recv_interactive(to_session_rx, prompts.len()).await?;
					// russh takes the answers as plain `String`s; expose each only at this last
					// moment. The `Secret`s are wiped when they drop at the end of this branch.
					secrets.iter().map(|s| s.expose().to_owned()).collect()
				};
				response = session
					.authenticate_keyboard_interactive_respond(answers)
					.await
					.context("keyboard-interactive response failed")?;
			}
		}
	}
}

/// Await the user's keyboard-interactive answers from the GUI (§7), ignoring any stray
/// input/resize that could arrive first. A disconnect or a dropped channel means the user gave
/// up on the prompt. The count is checked here so a mismatch is a clean error, not a russh panic.
async fn recv_interactive(
	to_session_rx: &mut mpsc::Receiver<SessionMsg>,
	expected: usize,
) -> Result<Vec<Secret>> {
	loop {
		match to_session_rx.recv().await {
			Some(SessionMsg::Interactive(answers)) => {
				if answers.len() != expected {
					bail!("keyboard-interactive answer count did not match the prompts");
				}
				return Ok(answers);
			}
			Some(SessionMsg::Disconnect) | None => {
				bail!("cancelled before the keyboard-interactive prompt was answered")
			}
			Some(_) => {} // ignore keystrokes / resize / a stray passphrase until auth completes
		}
	}
}

/// Load the chosen private key (§7, §14), prompting for a passphrase only when the key is
/// actually encrypted. `initial` is the optional passphrase pre-seeded from the form: `Some` is
/// tried before any prompt (so a known passphrase unlocks the key silently), `None` keeps the
/// original interactive-only behavior.
///
/// The load happens in two stages. First we probe with NO passphrase, which cleanly classifies
/// the file: an unencrypted key loads here (any typed passphrase is meaningless for it and is
/// correctly ignored); an unencrypted but malformed key is a hard error; an encrypted key
/// reports `NeedsPassphrase` and drops to the retry loop. There we try the pre-seed (if any),
/// then ask the GUI and retry — up to `MAX_PASSPHRASE_ATTEMPTS` prompts. A wrong passphrase
/// (pre-seeded or typed) just asks again.
async fn resolve_key(
	path: &Path,
	initial: Option<Secret>,
	events: &mpsc::Sender<SshEvent>,
	to_session_rx: &mut mpsc::Receiver<SessionMsg>,
) -> Result<PrivateKey> {
	// Stage one: classify the file with no passphrase.
	match keyfile::load_private_key(path, None)? {
		Loaded::Key(key) => return Ok(*key),
		// Encrypted: fall through to the passphrase loop below.
		Loaded::NeedsPassphrase => {}
	}

	// Stage two: the key is encrypted. Try the pre-seed first, then prompt and retry.
	let mut passphrase = initial;
	let mut attempts = 0u32;

	loop {
		// A passphrase in hand (pre-seed or typed) that unlocks the key wins immediately.
		if let Some(secret) = passphrase.as_ref()
			&& let Ok(Loaded::Key(key)) = keyfile::load_private_key(path, Some(secret))
		{
			return Ok(*key);
		}

		if attempts >= MAX_PASSPHRASE_ATTEMPTS {
			bail!("too many incorrect passphrase attempts");
		}
		attempts += 1;

		let _ = events.send(SshEvent::NeedPassphrase).await;
		passphrase = Some(recv_passphrase(to_session_rx).await?);
	}
}

/// Await the user's passphrase from the GUI, ignoring any stray input/resize that could arrive
/// before the shell is open. A disconnect or a dropped channel means the user gave up (§7).
async fn recv_passphrase(to_session_rx: &mut mpsc::Receiver<SessionMsg>) -> Result<Secret> {
	loop {
		match to_session_rx.recv().await {
			Some(SessionMsg::Passphrase(secret)) => return Ok(secret),
			Some(SessionMsg::Disconnect) | None => {
				bail!("cancelled before a passphrase was entered")
			}
			Some(_) => {} // ignore keystrokes/resize until the shell exists
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// A `MethodSet` listing exactly the given methods, for the retry-rule tests.
	fn methods(kinds: &[MethodKind]) -> MethodSet {
		MethodSet::from(kinds)
	}

	#[test]
	fn interactive_is_retried_while_offered_and_within_the_attempt_bound() {
		// Arrange: the server still lists keyboard-interactive, first attempt.
		let remaining = methods(&[MethodKind::PublicKey, MethodKind::KeyboardInteractive]);

		// Act / Assert
		assert!(should_try_interactive(&remaining, 0));
		assert!(should_try_interactive(&remaining, MAX_AUTH_ATTEMPTS - 1));
	}

	#[test]
	fn interactive_stops_once_the_attempt_bound_is_reached() {
		// Arrange: still offered, but we have used up our attempts.
		let remaining = methods(&[MethodKind::KeyboardInteractive]);

		// Act / Assert: bounded like MaxAuthTries, so a re-offering server cannot loop forever.
		assert!(!should_try_interactive(&remaining, MAX_AUTH_ATTEMPTS));
	}

	#[test]
	fn interactive_is_skipped_when_the_server_does_not_offer_it() {
		// Arrange: the server offers other methods but not keyboard-interactive.
		let remaining = methods(&[MethodKind::Password, MethodKind::PublicKey]);

		// Act / Assert
		assert!(!should_try_interactive(&remaining, 0));
	}

	#[test]
	fn an_auth_result_maps_onto_the_normalised_outcome() {
		// Success maps to Success; a failure carries the server's remaining methods forward.
		assert!(matches!(
			Outcome::from(client::AuthResult::Success),
			Outcome::Success
		));

		let failure = client::AuthResult::Failure {
			remaining_methods: methods(&[MethodKind::KeyboardInteractive]),
			partial_success: true,
		};
		match Outcome::from(failure) {
			Outcome::Failure { remaining } => {
				assert!(remaining.contains(&MethodKind::KeyboardInteractive));
			}
			Outcome::Success => panic!("a failure must not map to success"),
		}
	}
}

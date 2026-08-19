// vault.rs — the opt-in encrypted credential vault (PLAN §16).
//
// §12 makes a strong promise: the safest secret is the one never persisted, and by
// default cmote keeps it — `targets.rs` stores connection *metadata* only, never a
// password or a key passphrase. This module is the deliberate, opt-in relaxation of
// that promise the user asked for: when — and only when — the user ticks "Remember" on
// the connect form, the secret for that endpoint is kept here so a return visit does not
// re-type it.
//
// PORTABLE BY DESIGN. The obvious way to persist a secret is an OS-bound store (Windows
// Credential Manager / macOS Keychain, both DPAPI/Keychain-encrypted to this user on this
// machine). That is secure but it does NOT travel: copy `cmote-data/` to another machine
// and the secret is gone. cmote's whole identity is a portable single-folder client (§11),
// so instead the vault is one file — `secrets.age` beside `targets.json` — encrypted with
// the `age` format under a MASTER PASSPHRASE the user chooses. That file unlocks on any OS
// or machine with the passphrase, so the secrets ride along on the USB stick like everything
// else.
//
// THE INESCAPABLE TRADE. To decrypt you need a key, and a portable key must live outside the
// machine: here it lives in the user's head (the master passphrase). So:
//   * the master passphrase is never stored anywhere — lose it and the secrets are gone,
//     by design (there is no recovery, and that is the point of encryption);
//   * `secrets.age` on its own is useless — it is scrypt-KDF'd and XChaCha20-Poly1305 sealed,
//     so possession of the stick is not possession of the secrets, unlike a plaintext file;
//   * in memory the decrypted secrets are `Secret` (zeroized on drop, redacted in `Debug`,
//     §12) and the plaintext JSON that carries them between serde and the cipher is held in
//     `Zeroizing` for the brief moment it exists.
//
// The plaintext inside the blob is a flat JSON map `{ "user@host:port": "<secret>" }` — one
// entry per saved target, keyed by the same endpoint string `targets.rs` uses as a target's
// identity, so the two line up without a second key scheme. `targets.json` records only a
// `remember_secret` flag (metadata, never ciphertext); the secret itself lives here alone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use age::secrecy::SecretString;
use anyhow::{Context, Result};
use zeroize::Zeroizing;

use crate::secret::Secret;

/// The vault file name, beside `targets.json` in the data directory (§11). The `.age`
/// extension is the format's convention and a hint to anyone browsing the folder that this
/// file is encrypted, not editable.
const VAULT_FILE: &str = "secrets.age";

/// The unlocked vault: the master passphrase (held for the session so repeated stores need no
/// re-prompt) and the decrypted endpoint→secret map. Created empty (`create`) the first time a
/// user opts in, or decrypted from disk (`unlock`) on a later launch. Every mutation re-seals
/// the whole file, so the on-disk blob is always the current set — there is no partial state.
///
/// `Debug` is derived on the fields that are safe: `SecretString` and `Secret` both redact
/// themselves, so a stray `{:?}` cannot leak the passphrase or any stored secret.
#[derive(Debug)]
pub struct Vault {
	/// The master passphrase, kept in `age`'s own redacted secret type for the session.
	passphrase: SecretString,
	/// endpoint (`user@host:port`) → the secret for it. `BTreeMap` so the serialized JSON has
	/// a deterministic key order (stable diffs, reproducible tests), not for lookup speed.
	entries: BTreeMap<String, Secret>,
	/// Where the sealed blob is written. Resolved once (the data dir) so every `persist` hits
	/// the same file; injectable in tests via `create_at` / `unlock_at`.
	path: PathBuf,
}

impl Vault {
	/// Whether a vault file already exists. The UI uses this to decide between prompting to
	/// CREATE a master passphrase (first time, typed twice to confirm) and prompting to UNLOCK
	/// an existing one (typed once). A path that cannot be resolved is treated as "no vault".
	pub fn exists() -> bool {
		vault_path().map(|path| path.exists()).unwrap_or(false)
	}

	/// Create a fresh, empty vault held only in memory. Nothing is written until the first
	/// secret is stored, so opting in and then backing out leaves no file behind.
	pub fn create(passphrase: String) -> Result<Self> {
		Ok(Self::create_at(vault_path()?, passphrase))
	}

	/// Unlock the existing vault, decrypting every entry into memory. A wrong master
	/// passphrase — or an unreadable / corrupt file — is an error, which the UI turns into a
	/// "that passphrase was not correct" re-ask. There is deliberately no oracle beyond that
	/// (§12): the caller cannot tell a wrong passphrase from a damaged file, and does not need
	/// to.
	pub fn unlock(passphrase: String) -> Result<Self> {
		Self::unlock_at(&vault_path()?, passphrase)
	}

	/// The secret stored for `endpoint`, if any. Borrowed — the caller clones it into the
	/// connect form to pre-fill the masked field (§16), so the vault keeps ownership.
	pub fn get(&self, endpoint: &str) -> Option<&Secret> {
		self.entries.get(endpoint)
	}

	/// Store (or replace) the secret for `endpoint` and re-seal the file. Called on a
	/// successful connect when "Remember" is ticked, so the secret is only ever persisted once
	/// the credentials are known good.
	pub fn store(&mut self, endpoint: &str, secret: Secret) -> Result<()> {
		self.entries.insert(endpoint.to_owned(), secret);
		self.persist()
	}

	/// Forget the secret for `endpoint`, re-sealing the file if one was actually stored. A
	/// no-op (and no write) when there was nothing there, so deleting a target that never
	/// saved a secret costs nothing. Used when a target is deleted or the user unticks
	/// "Remember".
	pub fn forget(&mut self, endpoint: &str) -> Result<()> {
		if self.entries.remove(endpoint).is_some() {
			self.persist()?;
		}
		Ok(())
	}

	// --- path-injected cores, so tests need not touch the real data directory ---

	/// The in-memory constructor `create` delegates to, with the file path supplied.
	fn create_at(path: PathBuf, passphrase: String) -> Self {
		Self {
			passphrase: SecretString::from(passphrase),
			entries: BTreeMap::new(),
			path,
		}
	}

	/// The decrypting constructor `unlock` delegates to, with the file path supplied.
	fn unlock_at(path: &Path, passphrase: String) -> Result<Self> {
		let passphrase = SecretString::from(passphrase);
		let ciphertext = std::fs::read(path).context("failed to read the vault file")?;
		let entries = unseal(&passphrase, &ciphertext)?;
		Ok(Self {
			passphrase,
			entries,
			path: path.to_owned(),
		})
	}

	/// Seal the current entries and write them, replacing the file atomically (`store`): a crash
	/// mid-write can never truncate the vault and lose every stored secret at once. This file's
	/// pattern is now every store's — see §110 for why the other two needed it as badly.
	fn persist(&self) -> Result<()> {
		let sealed = seal(&self.passphrase, &self.entries, None)?;
		crate::store::write_atomically(&self.path, &sealed)
	}
}

/// The resolved vault path: `secrets.age` in the shared data directory (§11), the same
/// directory `targets.rs` and `hostkey.rs` use.
fn vault_path() -> Result<PathBuf> {
	Ok(crate::paths::data_dir()?.join(VAULT_FILE))
}

/// Encrypt the entries under the master passphrase, returning the `age` blob. The secrets are
/// serialized to JSON in a `Zeroizing` buffer (wiped the moment sealing is done) and never
/// touch disk unencrypted. `log_n` overrides scrypt's work factor: `None` uses `age`'s default,
/// which auto-targets about a second on this machine — strong, and fine for a once-per-session
/// unlock; tests pass a small value to stay fast.
fn seal(
	passphrase: &SecretString,
	entries: &BTreeMap<String, Secret>,
	log_n: Option<u8>,
) -> Result<Vec<u8>> {
	// Borrow each secret's plaintext just long enough to serialize it. `expose` is the single
	// audited access point (§12); the resulting JSON lives only in the `Zeroizing` buffer.
	let plain: BTreeMap<&str, &str> = entries
		.iter()
		.map(|(endpoint, secret)| (endpoint.as_str(), secret.expose()))
		.collect();
	let json = Zeroizing::new(serde_json::to_vec(&plain).context("failed to encode the vault")?);

	let mut recipient = age::scrypt::Recipient::new(passphrase.clone());
	if let Some(log_n) = log_n {
		recipient.set_work_factor(log_n);
	}
	age::encrypt(&recipient, &json).context("failed to encrypt the vault")
}

/// Decrypt an `age` blob under the master passphrase and rebuild the endpoint→secret map. The
/// decrypted JSON is held in `Zeroizing` and each value is MOVED into a `Secret` (not copied),
/// so no plaintext copy of a secret outlives this function. Any failure — wrong passphrase, a
/// truncated or corrupt blob, malformed JSON — comes back as an error; the caller re-asks.
fn unseal(passphrase: &SecretString, ciphertext: &[u8]) -> Result<BTreeMap<String, Secret>> {
	let identity = age::scrypt::Identity::new(passphrase.clone());
	let plaintext =
		Zeroizing::new(age::decrypt(&identity, ciphertext).context("failed to decrypt the vault")?);
	let plain: BTreeMap<String, String> =
		serde_json::from_slice(&plaintext).context("failed to parse the vault")?;
	// Move each String into a Secret so it is zeroized on drop; the source map is consumed.
	Ok(plain
		.into_iter()
		.map(|(endpoint, secret)| (endpoint, Secret::new(secret)))
		.collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	// A low scrypt work factor so the round-trip tests run in milliseconds, not the ~1s each
	// the auto-targeted default would cost. Correctness is identical — only the KDF cost
	// differs — so testing the fast factor still exercises the whole seal/open path.
	const TEST_WORK_FACTOR: Option<u8> = Some(10);

	fn entries_of(pairs: &[(&str, &str)]) -> BTreeMap<String, Secret> {
		pairs
			.iter()
			.map(|(endpoint, secret)| (endpoint.to_string(), Secret::new(secret.to_string())))
			.collect()
	}

	#[test]
	fn seal_then_open_round_trips_the_secrets() {
		// Arrange
		let pass = SecretString::from("correct horse battery staple".to_string());
		let entries = entries_of(&[("root@host:22", "hunter2"), ("me@box:2222", "s3cr3t")]);

		// Act
		let blob = seal(&pass, &entries, TEST_WORK_FACTOR).expect("seal");
		let back = unseal(&pass, &blob).expect("open");

		// Assert: same endpoints, same secrets.
		assert_eq!(back.len(), 2);
		assert_eq!(back["root@host:22"].expose(), "hunter2");
		assert_eq!(back["me@box:2222"].expose(), "s3cr3t");
	}

	#[test]
	fn a_wrong_master_passphrase_fails_to_open() {
		// Arrange: seal under one passphrase.
		let right = SecretString::from("open sesame".to_string());
		let wrong = SecretString::from("open barley".to_string());
		let blob = seal(&right, &entries_of(&[("u@h:22", "pw")]), TEST_WORK_FACTOR).expect("seal");

		// Act / Assert: the wrong passphrase cannot decrypt — no oracle, just an error (§12).
		assert!(unseal(&wrong, &blob).is_err());
	}

	#[test]
	fn a_corrupt_blob_is_an_error_not_a_panic() {
		let pass = SecretString::from("whatever".to_string());
		assert!(unseal(&pass, b"this is not an age file").is_err());
	}

	#[test]
	fn the_sealed_blob_holds_no_secret_in_the_clear() {
		// The whole point: the secret must not be readable in the ciphertext.
		let pass = SecretString::from("master".to_string());
		let blob = seal(
			&pass,
			&entries_of(&[("u@h:22", "TOPSECRET")]),
			TEST_WORK_FACTOR,
		)
		.expect("seal");
		assert!(
			!blob
				.windows(b"TOPSECRET".len())
				.any(|window| window == b"TOPSECRET"),
			"plaintext secret leaked into the ciphertext"
		);
	}

	#[test]
	fn store_get_and_forget_persist_through_a_file() {
		// Arrange: an empty vault at a temp path, low work factor via a direct seal is not
		// reachable here (store uses the default), so this test accepts the ~1s cost to prove
		// the file round-trips. Kept minimal for that reason.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("secrets.age");
		let mut vault = Vault::create_at(path.clone(), "master-pass".to_string());

		// Act: store two, forget one, reload from disk.
		vault
			.store("root@host:22", Secret::new("pw1".to_string()))
			.expect("store 1");
		vault
			.store("me@box:2222", Secret::new("pw2".to_string()))
			.expect("store 2");
		assert!(vault.forget("root@host:22").is_ok());

		let reopened = Vault::unlock_at(&path, "master-pass".to_string()).expect("unlock");

		// Assert: the forgotten one is gone, the other survives.
		assert!(reopened.get("root@host:22").is_none());
		assert_eq!(reopened.get("me@box:2222").map(Secret::expose), Some("pw2"));

		// A wrong master passphrase cannot reopen it.
		assert!(Vault::unlock_at(&path, "wrong".to_string()).is_err());
	}

	#[test]
	fn forgetting_a_missing_entry_writes_nothing() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("secrets.age");
		let mut vault = Vault::create_at(path.clone(), "m".to_string());

		// Nothing stored yet: forget is a no-op and must not create the file.
		assert!(vault.forget("nobody@nowhere:22").is_ok());
		assert!(
			!path.exists(),
			"an empty forget must not write a vault file"
		);
	}
}

// ssh/keyfile.rs — load a private key from disk and hand back a russh
// `PrivateKey` (PLAN §7).
//
// Two on-disk families, one entry point:
//   * OpenSSH / PEM — native to the SSH ecosystem. russh decodes these directly
//     (Ed25519, ECDSA, RSA), encrypted or not, via `decode_secret_key`.
//   * PuTTY `.ppk`  — PuTTY's own container (v2/v3): base64 blobs wrapped in a
//     small text header, MAC-protected, optionally Argon2/AES-256-CBC encrypted.
//     russh's key crate (`ssh-key`, built with its `ppk` feature — already on in
//     our tree) parses AND verifies these for us: `PrivateKey::from_ppk`. It
//     checks the MAC in constant time before trusting a byte, derives the key
//     (Argon2 for v3, a SHA-1 construction for v2) and AES-256-CBC-decrypts the
//     private blob. We only sniff the format and route to it.
//
// Encrypted keys are handled interactively (§7): rather than demand a passphrase
// up front, `load_private_key` reports `Loaded::NeedsPassphrase` when a file is
// an encrypted key it could not unlock, and the session prompts the user and
// retries. So an *unencrypted* key never triggers a passphrase prompt.
//
// Why reuse the crate's parser rather than hand-roll one (as an earlier plan
// intended): MAC verification and key decryption are a security-sensitive path,
// and PLAN §12 puts "security outranks purity" — leaning on the audited
// RustCrypto implementation already compiled into our binary beats shipping our
// own crypto glue. See the PLAN §7 note for the reversed decision.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use russh::keys::{PrivateKey, decode_secret_key};

use crate::secret::Secret;

/// The first line of every PuTTY key file begins with this, regardless of
/// version (`PuTTY-User-Key-File-2:` or `-3:`). We sniff on the stable prefix.
const PPK_MAGIC: &str = "PuTTY-User-Key-File-";

/// The result of one load attempt (§7): either the decoded key, or a signal
/// that the file is an *encrypted* key we could not unlock with the passphrase
/// we had (none, or a wrong one). Separating these lets the caller prompt only
/// when it truly must, and retry without re-reading the file's role.
pub enum Loaded {
	/// A fully decoded private key, ready to authenticate with. Boxed because a
	/// `PrivateKey` is large (hundreds of bytes) while the other variant is empty
	/// — boxing keeps the enum small and satisfies `clippy::large_enum_variant`.
	Key(Box<PrivateKey>),
	/// The file is an encrypted key; ask the user for a passphrase and retry.
	NeedsPassphrase,
}

/// Load and decode a private key from disk (§7). The file is read once, its
/// format is detected by *content* (not the extension, which can lie), and the
/// bytes are routed to the matching decoder. `passphrase` decrypts an encrypted
/// key; `None` means "try as an unencrypted key". A genuinely malformed key is
/// an `Err`; an encrypted key we could not unlock is `Ok(Loaded::NeedsPassphrase)`.
///
/// `ponytail:` the read is blocking file I/O on the async runtime — fine for a
/// one-shot read of a small key file, a `spawn_blocking` candidate if it grows.
pub fn load_private_key(path: &Path, passphrase: Option<&Secret>) -> Result<Loaded> {
	let text = fs::read_to_string(path).context("could not read the key file")?;
	let passphrase = passphrase.map(Secret::expose);

	if is_ppk(&text) {
		load_ppk(&text, passphrase)
	} else {
		load_openssh(&text, passphrase)
	}
}

/// Sniff whether the text is a PuTTY `.ppk`: its first line always starts with
/// `PuTTY-User-Key-File-<version>:`. Content sniffing beats trusting the file's
/// extension, which a user can rename freely.
fn is_ppk(text: &str) -> bool {
	text.trim_start().starts_with(PPK_MAGIC)
}

/// Does the `.ppk` header declare encryption? PuTTY writes an `Encryption:` line;
/// any value other than `none` means a passphrase is required. Reading the plain
/// header ourselves lets us classify "needs a passphrase" without depending on
/// the crate's (private) error variants.
fn ppk_is_encrypted(text: &str) -> bool {
	text.lines()
		.find_map(|line| line.trim().strip_prefix("Encryption:"))
		.map(str::trim)
		.is_some_and(|value| value != "none")
}

/// Decode a PuTTY `.ppk` via `ssh-key`'s parser (which verifies the MAC and
/// decrypts as needed). A failure on an *encrypted* file means the passphrase
/// was missing or wrong (the MAC won't verify) → recoverable, ask again. A
/// failure on an unencrypted file is a real, hard error.
///
/// `ponytail:` `from_ppk` takes the passphrase as an owned `String` by value, so
/// we must copy it out of `Secret` for the call. That copy is a plain `String`
/// dropped inside the crate — not zeroized — a small secret-hygiene gap the
/// crate's API forces on us (§12). Acceptable for now; noted honestly.
fn load_ppk(text: &str, passphrase: Option<&str>) -> Result<Loaded> {
	match PrivateKey::from_ppk(text, passphrase.map(str::to_owned)) {
		Ok(key) => Ok(Loaded::Key(Box::new(key))),
		Err(error) => {
			if ppk_is_encrypted(text) {
				Ok(Loaded::NeedsPassphrase)
			} else {
				Err(anyhow::Error::new(error).context("could not load the PuTTY key"))
			}
		}
	}
}

/// Decode an OpenSSH/PEM key. russh reports an encrypted key it could not unlock
/// (no passphrase given) as `KeyIsEncrypted`; we turn that into a request for a
/// passphrase. A wrong passphrase surfaces as a different decode error, which the
/// caller — already knowing the key is encrypted — treats as "ask again".
fn load_openssh(text: &str, passphrase: Option<&str>) -> Result<Loaded> {
	match decode_secret_key(text, passphrase) {
		Ok(key) => Ok(Loaded::Key(Box::new(key))),
		Err(russh::keys::Error::KeyIsEncrypted) => Ok(Loaded::NeedsPassphrase),
		Err(other) => Err(anyhow::Error::new(other).context("could not load the private key")),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::io::Write;

	use russh::keys::Algorithm;
	use tempfile::NamedTempFile;

	// Real PuTTYgen fixtures (v3, Ed25519) — an unencrypted key and the same key
	// encrypted with the passphrase below. Vendored from `ssh-key`'s test suite.
	const PPK_ED25519: &str = include_str!("fixtures/id_ed25519.ppk");
	const PPK_ED25519_ENC: &str = include_str!("fixtures/id_ed25519_enc.ppk");
	const ENC_PASSPHRASE: &str = "123";

	// Write key text to a temp file so we exercise the real read-from-path path.
	fn temp_key(content: &str) -> NamedTempFile {
		let mut file = NamedTempFile::new().expect("create temp key file");
		file.write_all(content.as_bytes()).expect("write temp key");
		file.flush().expect("flush temp key");
		file
	}

	// Assert we got a decoded key of the expected algorithm.
	fn assert_key(loaded: Loaded, algorithm: Algorithm) {
		match loaded {
			Loaded::Key(key) => assert_eq!(key.algorithm(), algorithm),
			Loaded::NeedsPassphrase => panic!("expected a decoded key, got NeedsPassphrase"),
		}
	}

	#[test]
	fn detects_ppk_by_header() {
		assert!(is_ppk(PPK_ED25519));
		assert!(is_ppk("  \nPuTTY-User-Key-File-2: ssh-rsa")); // leading blanks tolerated
		assert!(!is_ppk("-----BEGIN OPENSSH PRIVATE KEY-----"));
		assert!(!is_ppk("not a key at all"));
	}

	#[test]
	fn reads_the_encryption_header() {
		assert!(!ppk_is_encrypted(PPK_ED25519));
		assert!(ppk_is_encrypted(PPK_ED25519_ENC));
	}

	#[test]
	fn loads_unencrypted_ed25519_ppk_without_prompting() {
		let file = temp_key(PPK_ED25519);
		let loaded = load_private_key(file.path(), None).expect("valid unencrypted ppk");
		assert_key(loaded, Algorithm::Ed25519);
	}

	#[test]
	fn loads_encrypted_ed25519_ppk_with_the_right_passphrase() {
		let file = temp_key(PPK_ED25519_ENC);
		let secret = Secret::new(ENC_PASSPHRASE.to_string());
		let loaded = load_private_key(file.path(), Some(&secret)).expect("valid encrypted ppk");
		assert_key(loaded, Algorithm::Ed25519);
	}

	#[test]
	fn encrypted_ppk_without_a_passphrase_asks_for_one() {
		let file = temp_key(PPK_ED25519_ENC);
		let loaded = load_private_key(file.path(), None).expect("classified, not errored");
		assert!(matches!(loaded, Loaded::NeedsPassphrase));
	}

	#[test]
	fn encrypted_ppk_with_a_wrong_passphrase_asks_again() {
		let file = temp_key(PPK_ED25519_ENC);
		let secret = Secret::new("wrong".to_string());
		let loaded = load_private_key(file.path(), Some(&secret)).expect("classified, not errored");
		assert!(matches!(loaded, Loaded::NeedsPassphrase));
	}

	#[test]
	fn non_ppk_garbage_routes_to_openssh_and_fails() {
		// Not a ppk header → the OpenSSH decoder is tried, which rejects garbage.
		let file = temp_key("this is not a key");
		assert!(load_private_key(file.path(), None).is_err());
	}
}

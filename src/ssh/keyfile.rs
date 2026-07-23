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
// Why reuse it rather than hand-roll the parser (as an earlier plan intended):
// this is a security-sensitive path — MAC verification and key decryption — and
// PLAN §12 puts "security outranks purity": leaning on the audited RustCrypto
// implementation that is *already compiled into our binary* beats shipping our
// own crypto glue. See the PLAN §7 note for the reversed decision.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use russh::keys::{PrivateKey, decode_secret_key};

use crate::secret::Secret;

/// The first line of every PuTTY key file begins with this, regardless of
/// version (`PuTTY-User-Key-File-2:` or `-3:`). We sniff on the stable prefix.
const PPK_MAGIC: &str = "PuTTY-User-Key-File-";

/// Load and decode a private key from disk (§7). The file is read once, its
/// format is detected by *content* (not the extension, which can lie), and the
/// bytes are routed to the matching decoder. `passphrase` decrypts an encrypted
/// key; `None` means "the key is stored unencrypted".
///
/// `ponytail:` the read is blocking file I/O on the async runtime — fine for a
/// one-shot read of a small key file, a `spawn_blocking` candidate if it grows.
pub fn load_private_key(path: &Path, passphrase: Option<&Secret>) -> Result<PrivateKey> {
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

/// Decode a PuTTY `.ppk` via `ssh-key`'s parser. It verifies the MAC and
/// decrypts (if needed) before returning the key.
///
/// `ponytail:` `from_ppk` takes the passphrase as an owned `String` by value, so
/// we must copy it out of `Secret` for the call. That copy is a plain `String`
/// dropped inside the crate — not zeroized — a small secret-hygiene gap the
/// crate's API forces on us (§12). Acceptable for now; noted honestly.
fn load_ppk(text: &str, passphrase: Option<&str>) -> Result<PrivateKey> {
	PrivateKey::from_ppk(text, passphrase.map(str::to_owned))
		.map_err(|error| anyhow::Error::new(error).context("could not load the PuTTY key"))
}

/// Decode an OpenSSH/PEM key. The "encrypted but no passphrase given" case gets
/// a clear message so the reason is obvious in the logs (the user still sees the
/// generic session error — no credential oracle, §12).
fn load_openssh(text: &str, passphrase: Option<&str>) -> Result<PrivateKey> {
	decode_secret_key(text, passphrase).map_err(|error| match error {
		russh::keys::Error::KeyIsEncrypted => {
			anyhow::anyhow!("the key is encrypted; a passphrase is required")
		}
		other => anyhow::Error::new(other).context("could not load the private key"),
	})
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

	#[test]
	fn detects_ppk_by_header() {
		assert!(is_ppk(PPK_ED25519));
		assert!(is_ppk("  \nPuTTY-User-Key-File-2: ssh-rsa")); // leading blanks tolerated
		assert!(!is_ppk("-----BEGIN OPENSSH PRIVATE KEY-----"));
		assert!(!is_ppk("not a key at all"));
	}

	#[test]
	fn loads_unencrypted_ed25519_ppk() {
		let file = temp_key(PPK_ED25519);
		let key = load_private_key(file.path(), None).expect("valid unencrypted ppk");
		assert_eq!(key.algorithm(), Algorithm::Ed25519);
	}

	#[test]
	fn loads_encrypted_ed25519_ppk_with_passphrase() {
		let file = temp_key(PPK_ED25519_ENC);
		let secret = Secret::new(ENC_PASSPHRASE.to_string());
		let key = load_private_key(file.path(), Some(&secret)).expect("valid encrypted ppk");
		assert_eq!(key.algorithm(), Algorithm::Ed25519);
	}

	#[test]
	fn encrypted_ppk_without_passphrase_is_rejected() {
		let file = temp_key(PPK_ED25519_ENC);
		assert!(load_private_key(file.path(), None).is_err());
	}

	#[test]
	fn wrong_passphrase_is_rejected() {
		let file = temp_key(PPK_ED25519_ENC);
		let secret = Secret::new("wrong".to_string());
		assert!(load_private_key(file.path(), Some(&secret)).is_err());
	}

	#[test]
	fn non_ppk_garbage_routes_to_openssh_and_fails() {
		// Not a ppk header → the OpenSSH decoder is tried, which rejects garbage.
		let file = temp_key("this is not a key");
		assert!(load_private_key(file.path(), None).is_err());
	}
}

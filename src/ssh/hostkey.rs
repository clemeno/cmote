// ssh/hostkey.rs — TOFU host-key verification (PLAN §8), the core MITM defense.
//
// This is pure, network-free logic: given the server's public key, the host,
// the port, and a known_hosts file, decide whether to proceed, prompt, or
// refuse. The policy (accept unknown after explicit user consent, refuse a
// changed key) is expressed by `HostKeyVerdict`; the russh `Handler` (next
// slice) turns a verdict into an action.
//
// We reuse russh's own known_hosts reader/writer rather than reimplement the
// format (which includes hashed hostnames): `check_known_hosts_path` and
// `known_hosts::learn_known_hosts_path`.

// ponytail: these items are exercised by the unit tests below and wired into the
// russh Handler in the next slice, so the non-test build sees them as unused for
// now. Temporary scaffolding allow — remove once client.rs calls them.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use russh::keys::{HashAlg, PublicKey};

/// The outcome of checking a server key against the known_hosts store. This is
/// the whole TOFU decision surface (§8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
	/// The key is pinned and matches — proceed silently.
	Known,
	/// We have never seen this host. Show the fingerprint, get explicit user
	/// consent, then `learn` it (trust on first use).
	Unknown,
	/// A key is pinned for this host but it is different. Treat as hostile
	/// (rotation *or* MITM) and refuse. `line` is the offending known_hosts line.
	Changed { line: usize },
}

/// The SHA-256 fingerprint of a public key, in the `SHA256:<base64>` form users
/// recognize from OpenSSH. This is what we show for first-contact confirmation.
pub fn fingerprint(pubkey: &PublicKey) -> String {
	pubkey.fingerprint(HashAlg::Sha256).to_string()
}

/// Check a server key against the known_hosts file at `path`. Never mutates the
/// file. A missing file is not an error — it simply means every host is unknown.
pub fn verify(host: &str, port: u16, pubkey: &PublicKey, path: &Path) -> Result<HostKeyVerdict> {
	match russh::keys::check_known_hosts_path(host, port, pubkey, path) {
		Ok(true) => Ok(HostKeyVerdict::Known),
		Ok(false) => Ok(HostKeyVerdict::Unknown),
		// A recorded key of the same type that no longer matches: the security
		// event we care about. russh signals it as this specific error.
		Err(russh::keys::Error::KeyChanged { line }) => Ok(HostKeyVerdict::Changed { line }),
		Err(error) => Err(anyhow::Error::new(error).context("failed to read known_hosts")),
	}
}

/// Pin a newly-accepted host key by appending it to the known_hosts file at
/// `path`. Only ever called after the user has explicitly accepted the
/// fingerprint (§8) — never automatically. Creates the file/parent if needed.
pub fn learn(host: &str, port: u16, pubkey: &PublicKey, path: &Path) -> Result<()> {
	russh::keys::known_hosts::learn_known_hosts_path(host, port, pubkey, path)
		.context("failed to record host key in known_hosts")
}

/// Resolve the portable known_hosts path (§11): prefer `cmote-data/` beside the
/// executable (true portable mode), else fall back to the per-user data directory
/// (`%LOCALAPPDATA%\cmote\` on Windows, `~/Library/Application Support/cmote/` on
/// macOS) — used only when the exe sits somewhere read-only (`Program Files`,
/// `/Applications`).
pub fn known_hosts_path() -> Result<PathBuf> {
	if let Some(dir) = writable_portable_dir() {
		return Ok(dir.join("known_hosts"));
	}

	let dir = user_data_dir()?;
	std::fs::create_dir_all(&dir).context("failed to create the fallback data directory")?;
	Ok(dir.join("known_hosts"))
}

// The per-user data directory is resolved with plain `std` per OS — no `dirs`
// crate (`ponytail:` §11). Only the two supported targets have a branch; any other
// target fails to compile with the message below rather than silently misbehaving.

/// Windows fallback: `%LOCALAPPDATA%\cmote` (e.g. `C:\Users\<user>\AppData\Local\cmote`).
#[cfg(windows)]
fn user_data_dir() -> Result<PathBuf> {
	let base = std::env::var_os("LOCALAPPDATA")
		.map(PathBuf::from)
		.context("no writable data directory (LOCALAPPDATA is not set)")?;
	Ok(base.join("cmote"))
}

/// macOS fallback: `~/Library/Application Support/cmote` — Apple's convention for
/// app-managed data, resolved from `$HOME`.
#[cfg(target_os = "macos")]
fn user_data_dir() -> Result<PathBuf> {
	let home = std::env::var_os("HOME")
		.map(PathBuf::from)
		.context("no writable data directory (HOME is not set)")?;
	Ok(home.join("Library/Application Support/cmote"))
}

#[cfg(not(any(windows, target_os = "macos")))]
compile_error!(
	"cmote supports only Windows and macOS (PLAN §2); no known_hosts fallback is defined for this target"
);

/// Return `cmote-data/` beside the exe if we can actually write there, else
/// `None`. `ponytail:` a create-dir + write-probe is enough to tell portable
/// (USB stick, any folder) from a read-only location like `Program Files`.
fn writable_portable_dir() -> Option<PathBuf> {
	let exe = std::env::current_exe().ok()?;
	let dir = exe.parent()?.join("cmote-data");
	std::fs::create_dir_all(&dir).ok()?;

	let probe = dir.join(".write-probe");
	std::fs::File::create(&probe).ok()?;
	let _ = std::fs::remove_file(&probe);
	Some(dir)
}

#[cfg(test)]
mod tests {
	use super::*;
	use russh::keys::parse_public_key_base64;

	// Two distinct, valid Ed25519 public keys (raw base64 blobs, no prefix).
	const KEY_A: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIJdD7y3aLq454yWBdwLWbieU1ebz9/cu7/QEXn9OIeZJ";
	const KEY_B: &str = "AAAAC3NzaC1lZDI1NTE5AAAAILIG2T/B0l0gaqj3puu510tu9N1OkQ4znY3LYuEm5zCF";

	fn key(blob: &str) -> PublicKey {
		parse_public_key_base64(blob).expect("fixture key should parse")
	}

	#[test]
	fn fingerprint_is_sha256_and_deterministic() {
		// Arrange
		let pubkey = key(KEY_A);

		// Act
		let first = fingerprint(&pubkey);
		let second = fingerprint(&pubkey);

		// Assert
		assert!(first.starts_with("SHA256:"), "unexpected format: {first}");
		assert_eq!(first, second, "fingerprint must be deterministic");
		assert_ne!(first, fingerprint(&key(KEY_B)), "different keys differ");
	}

	#[test]
	fn unknown_host_when_file_is_absent() {
		// Arrange
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts"); // never created

		// Act
		let verdict = verify("example.com", 22, &key(KEY_A), &path).unwrap();

		// Assert
		assert_eq!(verdict, HostKeyVerdict::Unknown);
	}

	#[test]
	fn known_host_when_key_matches() {
		// Arrange
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		std::fs::write(&path, format!("example.com ssh-ed25519 {KEY_A}\n")).unwrap();

		// Act
		let verdict = verify("example.com", 22, &key(KEY_A), &path).unwrap();

		// Assert
		assert_eq!(verdict, HostKeyVerdict::Known);
	}

	#[test]
	fn changed_host_when_key_differs() {
		// Arrange: host pinned to KEY_A, server now presents KEY_B.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		std::fs::write(&path, format!("example.com ssh-ed25519 {KEY_A}\n")).unwrap();

		// Act
		let verdict = verify("example.com", 22, &key(KEY_B), &path).unwrap();

		// Assert
		assert_eq!(verdict, HostKeyVerdict::Changed { line: 1 });
	}

	#[test]
	fn learn_then_verify_is_known() {
		// Arrange
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		let pubkey = key(KEY_A);

		// Act
		learn("host.example", 2222, &pubkey, &path).unwrap();
		let verdict = verify("host.example", 2222, &pubkey, &path).unwrap();

		// Assert
		assert_eq!(verdict, HostKeyVerdict::Known);
	}
}

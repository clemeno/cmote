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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use russh::keys::{HashAlg, PublicKey};

use crate::{paths, store};

/// The outcome of checking a server key against the `known_hosts` store. This is
/// the whole TOFU decision surface (§8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyVerdict {
	/// The key is pinned and matches — proceed silently.
	Known,
	/// We have never seen this host. Show the fingerprint, get explicit user
	/// consent, then `learn` it (trust on first use).
	Unknown,
	/// A key is pinned for this host but it is different. Treat as hostile
	/// (rotation *or* MITM) and refuse. `line` is the offending `known_hosts` line.
	Changed { line: usize },
}

/// The SHA-256 fingerprint of a public key, in the `SHA256:<base64>` form users
/// recognize from OpenSSH. This is what we show for first-contact confirmation.
pub fn fingerprint(pubkey: &PublicKey) -> String {
	pubkey.fingerprint(HashAlg::Sha256).to_string()
}

/// Check a server key against the `known_hosts` file at `path`. Never mutates the
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

/// Pin a newly-accepted host key by appending it to the `known_hosts` file at
/// `path`. Only ever called after the user has explicitly accepted the
/// fingerprint (§8) — never automatically. Creates the file/parent if needed.
pub fn learn(host: &str, port: u16, pubkey: &PublicKey, path: &Path) -> Result<()> {
	russh::keys::known_hosts::learn_known_hosts_path(host, port, pubkey, path)
		.context("failed to record host key in known_hosts")
}

/// The SHA-256 fingerprint of the key CURRENTLY pinned for a host, read from the `known_hosts` line
/// `verify` flagged as changed (§8). The mismatch dialog shows this beside the presented key's
/// fingerprint, so the user compares what was trusted before against what the server sends now —
/// the whole point of an override being a judgement, not a reflex. `line` is 1-indexed, exactly as
/// `HostKeyVerdict::Changed` reports it. Computed through the same `fingerprint` as the presented
/// key, so the two strings are directly comparable.
pub fn stored_fingerprint(path: &Path, line: usize) -> Result<String> {
	let text = std::fs::read_to_string(path).context("failed to read known_hosts")?;
	let index = line
		.checked_sub(1)
		.context("known_hosts line is 1-indexed")?;
	let entry = text
		.lines()
		.nth(index)
		.context("known_hosts line is out of range")?;
	// A known_hosts entry is `host[,host2…] keytype base64 [comment]`: the key blob is the third
	// whitespace field. Parse it back to a key rather than trust the raw text, so a malformed line
	// is an error, not a bogus fingerprint.
	let blob = entry
		.split_whitespace()
		.nth(2)
		.context("known_hosts line has no key blob")?;
	let pubkey = russh::keys::parse_public_key_base64(blob)
		.context("failed to parse the stored host key")?;
	Ok(fingerprint(&pubkey))
}

/// Replace the stale key pinned for a host (§8): drop the offending `known_hosts` line, then pin the
/// newly-accepted key in its place. Only ever reached after the user explicitly chose "Replace
/// key" in the mismatch dialog — never automatically. `line` is 1-indexed, as `verify` reports.
/// After this, future connections verify silently against the new key.
pub fn replace(host: &str, port: u16, pubkey: &PublicKey, path: &Path, line: usize) -> Result<()> {
	remove_line(path, line)?;
	learn(host, port, pubkey, path)
}

/// Remove the 1-indexed `line` from a `known_hosts` file, rewriting the rest verbatim. The helper
/// behind `replace`: drop the stale entry before the new key is learned. `lines()` strips the
/// terminator, so the kept lines are re-joined with `\n` and the file is newline-ended — the
/// OpenSSH format is one entry per line, each newline-terminated.
///
/// The rewrite goes through `store::write_atomically` (§110), and this file is the one where that
/// matters most. A plain `write` truncates before it fills, so a crash in that window leaves a
/// SHORT `known_hosts` — and a `known_hosts` missing entries does not fail loudly: every host it
/// forgot verifies as `Unknown`, which is the first-contact prompt. The attacked host whose key
/// this call was in the middle of replacing would be exactly the one to lose its pin, turning a
/// refusal into a "trust this new key?" the user is already primed to accept.
fn remove_line(path: &Path, line: usize) -> Result<()> {
	let text = std::fs::read_to_string(path).context("failed to read known_hosts")?;
	let index = line
		.checked_sub(1)
		.context("known_hosts line is 1-indexed")?;
	let kept: Vec<&str> = text
		.lines()
		.enumerate()
		.filter(|(number, _)| *number != index)
		.map(|(_, entry)| entry)
		.collect();
	let mut rebuilt = kept.join("\n");
	if !rebuilt.is_empty() {
		rebuilt.push('\n');
	}
	store::write_atomically(path, rebuilt.as_bytes()).context("failed to rewrite known_hosts")
}

/// Resolve the portable `known_hosts` path (§11): the shared data directory
/// (`cmote-data/` beside the exe, or the per-user fallback — see `paths::data_dir`)
/// with the `known_hosts` file name joined on.
pub fn known_hosts_path() -> Result<PathBuf> {
	Ok(paths::data_dir()?.join("known_hosts"))
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
	fn stored_fingerprint_reads_the_pinned_key_at_its_line() {
		// Arrange: two hosts, so the reported line is not trivially 1. example.com is pinned to
		// KEY_A on line 2; the server now presents KEY_B.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		std::fs::write(
			&path,
			format!("other.example ssh-ed25519 {KEY_B}\nexample.com ssh-ed25519 {KEY_A}\n"),
		)
		.unwrap();

		// Act: the mismatch verdict names the offending line; read the fingerprint stored there.
		let verdict = verify("example.com", 22, &key(KEY_B), &path).unwrap();
		let HostKeyVerdict::Changed { line } = verdict else {
			panic!("expected a changed verdict, got {verdict:?}");
		};
		let stored = stored_fingerprint(&path, line).unwrap();

		// Assert: it is KEY_A's fingerprint (what was trusted), not KEY_B's (what was presented).
		assert_eq!(stored, fingerprint(&key(KEY_A)));
		assert_ne!(stored, fingerprint(&key(KEY_B)));
	}

	#[test]
	fn replace_swaps_the_pinned_key_and_leaves_other_hosts() {
		// Arrange: other.example on line 1, example.com pinned to KEY_A on line 2.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		std::fs::write(
			&path,
			format!("other.example ssh-ed25519 {KEY_B}\nexample.com ssh-ed25519 {KEY_A}\n"),
		)
		.unwrap();
		let HostKeyVerdict::Changed { line } =
			verify("example.com", 22, &key(KEY_B), &path).unwrap()
		else {
			panic!("expected a changed verdict");
		};

		// Act: replace example.com's stale key with the presented one.
		replace("example.com", 22, &key(KEY_B), &path, line).unwrap();

		// Assert: example.com now verifies against KEY_B, and other.example's line is untouched.
		assert_eq!(
			verify("example.com", 22, &key(KEY_B), &path).unwrap(),
			HostKeyVerdict::Known
		);
		assert_eq!(
			verify("other.example", 22, &key(KEY_B), &path).unwrap(),
			HostKeyVerdict::Known
		);
	}

	#[test]
	fn the_rewrite_leaves_no_temp_file_beside_known_hosts() {
		// Arrange: the replace path rewrites the file through `store::write_atomically`, so the
		// bytes land in a sibling temp first. That sibling must be renamed away — a `known_hosts`
		// with a stray `known_hosts.tmp` beside it holding one host's key is a confusing artefact
		// in the directory that holds the whole trust store.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("known_hosts");
		std::fs::write(
			&path,
			format!("other.example ssh-ed25519 {KEY_B}\nexample.com ssh-ed25519 {KEY_A}\n"),
		)
		.unwrap();

		// Act
		replace("example.com", 22, &key(KEY_B), &path, 2).unwrap();

		// Assert: exactly one file in the directory, and it is the store itself.
		let left: Vec<String> = std::fs::read_dir(dir.path())
			.unwrap()
			.map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
			.collect();
		assert_eq!(
			left,
			vec!["known_hosts".to_owned()],
			"left behind: {left:?}"
		);
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

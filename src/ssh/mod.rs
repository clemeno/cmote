// ssh/mod.rs — the SSH layer (PLAN §6-§8), all running on the tokio task.
//
// Split by responsibility so no single file owns the whole protocol:
//   client  — the russh Handler + the connect→auth→shell→stream task loop (§6)
//   auth    — method selection and attempts: publickey then password (§7)
//   hostkey — TOFU host-key verification against a portable known_hosts (§8)
//   keyfile — load PEM/OpenSSH keys and PuTTY .ppk, handle passphrases (§7)
//
// These are stubs for the walking skeleton; each is filled in its own step.

pub mod auth;
pub mod client;
pub mod hostkey;
pub mod keyfile;

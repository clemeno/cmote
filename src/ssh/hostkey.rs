// ssh/hostkey.rs — TOFU host-key verification (PLAN §8), the core MITM defense.
//
// Checks the server key against a portable known_hosts file:
//   known + match    -> proceed silently
//   unknown          -> show the SHA-256 fingerprint, require explicit accept,
//                       then append (trust on first use)
//   known + mismatch -> refuse the connection (possible MITM); no override in v1
//
// Stub for the walking skeleton — implemented in a later step.

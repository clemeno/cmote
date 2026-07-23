// ssh/keyfile.rs — private key loading (PLAN §7).
//
// OpenSSH/PEM keys load natively via `russh::keys`. PuTTY `.ppk` keys use a
// different container: this module detects them and hands them to the in-house
// parser in `ppk.rs` (decided — no usable crate exists; see PLAN §7), which
// returns an OpenSSH key string we then decode. Decrypted key material and
// passphrases are wrapped in `Zeroizing` and wiped after use (§12).
//
// Stub for the walking skeleton — implemented in a later step.

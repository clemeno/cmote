// ssh/ppk.rs — in-house PuTTY .ppk parser (PLAN §7).
//
// No usable pure-Rust crate exists to read .ppk, so we parse it ourselves:
//   1. read the text container (header, algorithm, Public-Lines, Private-Lines,
//      Private-MAC);
//   2. verify the MAC (HMAC-SHA-256 for v3, HMAC-SHA-1 for v2) before trusting
//      any bytes;
//   3. if encrypted, derive the key (Argon2 for v3, SHA-1 for v2) from a
//      Zeroizing passphrase and AES-256-CBC decrypt the private blob;
//   4. re-encode the inner RSA/Ed25519 key as an OpenSSH key string for russh.
//
// Stub for the walking skeleton — implemented in a later step.

// ssh/auth.rs — authentication method selection and attempts (PLAN §7).
//
// Offers `publickey` first when a key is supplied, then `password`, respecting
// the server's advertised methods and stopping on first success. Auth failure
// returns a generic error — no oracle about which field was wrong (§12).
//
// Stub for the walking skeleton — implemented in a later step.

// secret.rs — a small wrapper for in-memory secrets (PLAN §12).
//
// Passwords and passphrases must not leak: not into logs, not into `Debug`
// output, and not left lingering in freed memory. `Secret` gives us all three:
//   * the inner `String` is wrapped in `zeroize::Zeroizing`, so its bytes are
//     overwritten with zeros when the value is dropped;
//   * `Debug` is implemented by hand to print `<redacted>`, so a stray
//     `{:?}` (e.g. debugging a `Message`) can never expose the value;
//   * the raw string is only reachable through the explicit `expose` method, so
//     every access is greppable and intentional.

use std::fmt;

use zeroize::Zeroizing;

/// A secret string held only in memory and wiped on drop. Session-only: never
/// serialized, never written to disk (§12).
#[derive(Clone, Default)]
pub struct Secret(Zeroizing<String>);

impl Secret {
	/// Wrap a plain string as a secret.
	pub fn new(value: String) -> Self {
		Self(Zeroizing::new(value))
	}

	/// Borrow the raw secret. Named `expose` on purpose: every real use of the
	/// secret is visible at the call site and easy to audit.
	pub fn expose(&self) -> &str {
		&self.0
	}
}

impl From<String> for Secret {
	fn from(value: String) -> Self {
		Self::new(value)
	}
}

// Redacting Debug: the whole point of the type. Never print the contents.
impl fmt::Debug for Secret {
	fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
		formatter.write_str("Secret(<redacted>)")
	}
}

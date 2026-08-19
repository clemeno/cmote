// link.rs — following an OSC 8 hyperlink the remote attached to a cell (PLAN §24).
//
// A modern program can wrap text in a clickable link with the OSC 8 escape
// (`ESC ] 8 ; params ; URI ST`); the engine records that URI on every cell the link
// covers, and `term::screen::Cell::hyperlink` hands it back to the UI. This module is the
// other half — turning a click on such a cell into an opened browser tab.
//
// The URI is REMOTE-CONTROLLED, which makes opening it a security boundary. Windows opens a
// URI with whatever program is registered for its scheme, so an arbitrary scheme
// (`file:`, `vscode:`, a custom handler, …) is a way for the remote to start a local
// program from a link. cmote therefore opens only web and mail links and refuses the rest,
// and it hands the URI to the `open` crate's default launcher, which passes it as data
// rather than building a shell command line — so a query string full of shell
// metacharacters cannot inject a command.

/// The URI schemes cmote is willing to open: the web and mail. Everything else is refused,
/// since a link's scheme decides which local program the OS launches and the URI comes from
/// the remote. Compared case-insensitively — RFC 3986 schemes are not case-sensitive.
const ALLOWED_SCHEMES: [&str; 3] = ["http", "https", "mailto"];

/// Whether `uri`'s scheme is one cmote will open. The scheme is the text before the first
/// `:` (RFC 3986); a URI with no `:` at all has no scheme and is refused, as is any scheme
/// outside `ALLOWED_SCHEMES`. This is the whole security decision, split out from the
/// side-effecting `open` below so it can be tested on its own.
pub fn is_allowed(uri: &str) -> bool {
	match uri.split_once(':') {
		Some((scheme, _)) => ALLOWED_SCHEMES
			.iter()
			.any(|allowed| scheme.eq_ignore_ascii_case(allowed)),
		None => false,
	}
}

/// Open `uri` in the OS's default handler when its scheme is allowed, and report whether it
/// was. `false` means the scheme was refused — the caller turns that into a note to the
/// user, so a blocked click is never silent. The launch itself is fire-and-forget
/// (`that_detached`): it returns as soon as the launcher process is spawned rather than
/// waiting for the browser, so the UI thread never stalls, and a rare launch failure (no
/// browser registered, say) is swallowed — there is nothing useful to do about it and it is
/// not worth a modal.
pub fn open_uri(uri: &str) -> bool {
	if !is_allowed(uri) {
		return false;
	}
	let _ = open::that_detached(uri);
	true
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn web_and_mail_schemes_are_allowed() {
		assert!(is_allowed("http://example.com/"));
		assert!(is_allowed("https://example.com/path?q=1"));
		assert!(is_allowed("mailto:someone@example.com"));
	}

	#[test]
	fn the_scheme_match_ignores_case() {
		// RFC 3986 makes the scheme case-insensitive, so a shouted scheme still opens.
		assert!(is_allowed("HTTPS://example.com/"));
		assert!(is_allowed("MailTo:someone@example.com"));
	}

	#[test]
	fn other_schemes_are_refused() {
		// The dangerous ones: a local file, and a scheme that would hand the click to a
		// registered protocol handler — the remote does not get to pick those.
		assert!(!is_allowed("file:///c:/windows/system32/"));
		assert!(!is_allowed("vscode://file/etc/passwd"));
		assert!(!is_allowed("javascript:alert(1)"));
	}

	#[test]
	fn a_uri_without_a_scheme_is_refused() {
		// No `:` means no scheme to vet, so there is nothing safe to open.
		assert!(!is_allowed("example.com/just/a/path"));
		assert!(!is_allowed(""));
	}

	#[test]
	fn a_query_string_does_not_widen_the_scheme() {
		// The scheme is only what precedes the FIRST colon; a later `:` in the path or query
		// cannot smuggle an allowed scheme past the check.
		assert!(!is_allowed("file:http://decoy"));
	}
}

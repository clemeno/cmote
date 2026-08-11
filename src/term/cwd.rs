// term/cwd.rs — track the remote shell's working directory (PLAN §17).
//
// SSH tells a client nothing about where the remote shell *is*: the cwd belongs to a
// process on the other side, and the protocol carries only bytes. Every terminal that
// shows the remote directory (VS Code, iTerm2, WezTerm, Windows Terminal) solves it the
// same way — the shell announces its cwd in an OSC escape sequence on each prompt, and
// the terminal picks it out of the output stream. cmote does exactly that:
//
//   OSC 7    ESC ] 7 ; file://host/path         BEL | ST   — the POSIX convention
//   OSC 9;9  ESC ] 9 ; 9 ; C:\path              BEL | ST   — the Windows convention
//   OSC 1337 ESC ] 1337 ; CurrentDir=/path      BEL | ST   — iTerm2's spelling (§55)
//
// All three are scanned here, so a remote of any family works. The third is the reason this
// module owns the question rather than `term::iterm`: what varies between them is only the
// spelling, and the thing that knows how to turn an announcement into a path should be one
// place. A dotfile written for iTerm2 then gets cwd tracking with nothing else configured.
//
// The sequences are
// invisible to the user (vt100 ignores OSC codes it does not know). cmote reads whatever
// the shell offers and injects nothing: a shell that announces its cwd (fish, a Windows
// OSC 9;9 shell) is followed; a silent bash/zsh — which emits neither unless the user has
// configured it to — leaves the cwd unknown, and the upload dialog then asks for the path.
//
// Finding where a sequence starts and ends is `term::osc`'s job, shared with the other
// scanners that read an OSC the engine ignores; a cwd announcement can be split anywhere
// in the stream and the framer carries that state between calls. What is left here is the
// part that is actually about directories: deciding which OSC codes announce one, and
// turning the payload into a path.

/// The longest OSC payload we will buffer. A cwd is a path; anything longer is either
/// not for us (a long window title, a base64 OSC 52 clipboard write) or malformed, and
/// buffering it would let a hostile stream grow our memory without bound (§12).
const MAX_PAYLOAD: usize = 4096;

/// The remote working directory as last announced by the shell (§17). Feed it every
/// byte of shell output; it keeps the most recent path and ignores everything else.
#[derive(Debug, Default)]
pub struct Cwd {
	framer: super::osc::Framer<MAX_PAYLOAD>,
	path: Option<String>,
}

impl Cwd {
	/// Scan a chunk of shell output for a cwd announcement. Safe at any chunk
	/// boundary — the framer's state carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) {
		// Every finished OSC arrives here — titles, clipboard writes, prompt marks and all — and
		// `parse` keeps only the two that announce a directory. A payload that is not one leaves the
		// last known path alone: shells set the title on every prompt, and forgetting the cwd each
		// time they did would make the feature useless.
		let path = &mut self.path;
		self.framer.feed(bytes, |_offset, payload| {
			if let Some(found) = parse(payload) {
				*path = Some(found);
			}
		});
	}

	/// The last announced remote directory, or `None` if the shell never said.
	pub fn path(&self) -> Option<&str> {
		self.path.as_deref()
	}
}

/// Pull the directory out of an OSC payload, or `None` if this OSC is not a cwd
/// announcement. OSC 7 carries a `file://` URI (percent-encoded); OSC 9;9 and iTerm2's
/// OSC 1337 `CurrentDir=` carry a bare path, sometimes quoted.
///
/// A payload that matches none of the three is somebody else's business — a title, a
/// clipboard write, a prompt mark — and reports `None`, which leaves the last known path
/// alone rather than forgetting it.
fn parse(payload: &[u8]) -> Option<String> {
	let text = if let Some(rest) = payload.strip_prefix(b"7;") {
		String::from_utf8(percent_decode(rest)).ok()?
	} else {
		// The two bare-path spellings. `CurrentDir=` is iTerm2's; it is read here rather than
		// in `term::iterm` because the only thing that differs is the prefix, and the path
		// arithmetic below should not exist twice.
		let rest = bare_path(payload)?;
		std::str::from_utf8(rest).ok()?.trim_matches('"').to_owned()
	};

	let path = strip_file_url(text.trim());
	(!path.is_empty()).then(|| path.to_owned())
}

/// The payload's path bytes, for the two announcements that carry one plainly. `None` when the
/// payload is neither.
fn bare_path(payload: &[u8]) -> Option<&[u8]> {
	payload
		.strip_prefix(b"9;9;")
		.or_else(|| payload.strip_prefix(b"1337;CurrentDir="))
}

/// Reduce a `file://host/path` URI to its path. The authority (the host) is dropped:
/// it names the machine the shell runs on, which is the one we are connected to. A
/// Windows URI resolves to `/C:/dir`, so the leading slash is trimmed back off. Input
/// that is already a plain path passes through untouched.
fn strip_file_url(text: &str) -> &str {
	let path = match text.strip_prefix("file://") {
		// Everything from the first `/` after the authority; a URI with no path at all
		// (`file://host`) leaves nothing, which the caller rejects as empty.
		Some(rest) => match rest.find('/') {
			Some(index) => &rest[index..],
			None => "",
		},
		None => text,
	};

	// `/C:/Users/...` -> `C:/Users/...`
	let windows_drive = path.as_bytes().get(2) == Some(&b':') && path.starts_with('/');
	if windows_drive { &path[1..] } else { path }
}

/// Decode `%XX` escapes in a URI path. A stray `%` that is not followed by two hex
/// digits is kept as-is rather than dropped — shells do escape reliably, but mangling
/// a path is worse than leaving one odd character in it.
fn percent_decode(input: &[u8]) -> Vec<u8> {
	let mut out = Vec::with_capacity(input.len());
	let mut index = 0;
	while index < input.len() {
		if input[index] == b'%'
			&& index + 2 < input.len()
			&& let (Some(high), Some(low)) = (hex(input[index + 1]), hex(input[index + 2]))
		{
			out.push(high * 16 + low);
			index += 3;
		} else {
			out.push(input[index]);
			index += 1;
		}
	}
	out
}

/// One hex digit's value, or `None` if the byte is not a hex digit.
fn hex(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		b'A'..=b'F' => Some(byte - b'A' + 10),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh tracker and read the result.
	fn track(bytes: &[u8]) -> Option<String> {
		let mut cwd = Cwd::default();
		cwd.feed(bytes);
		cwd.path().map(str::to_owned)
	}

	#[test]
	fn osc7_uri_becomes_a_plain_path() {
		// The usual POSIX announcement: OSC 7 with a file:// URI, BEL-terminated.
		let path = track(b"\x1b]7;file://myhost/home/user/work\x07");
		assert_eq!(path.as_deref(), Some("/home/user/work"));
	}

	#[test]
	fn osc7_accepts_the_st_terminator_and_percent_escapes() {
		// ESC \ (ST) instead of BEL, and a percent-escaped space in the path.
		let path = track(b"\x1b]7;file://host/home/user/my%20docs\x1b\\");
		assert_eq!(path.as_deref(), Some("/home/user/my docs"));
	}

	#[test]
	fn a_sequence_split_across_chunks_is_still_read() {
		// Output arrives in arbitrary chunks — including a split between ESC and `]`.
		let mut cwd = Cwd::default();
		cwd.feed(b"prompt$ \x1b");
		cwd.feed(b"]7;file://host/sr");
		cwd.feed(b"c/app\x07rest of the line");
		assert_eq!(cwd.path(), Some("/src/app"));
	}

	#[test]
	fn osc9_9_carries_a_bare_windows_path() {
		// The Windows convention: OSC 9;9 with a quoted native path.
		let path = track(b"\x1b]9;9;\"C:\\Users\\CLEm\"\x07");
		assert_eq!(path.as_deref(), Some("C:\\Users\\CLEm"));
	}

	#[test]
	fn a_windows_file_url_loses_its_leading_slash() {
		// OSC 7 on Windows yields file:///C:/... — the URI's leading slash is not part
		// of the native path.
		let path = track(b"\x1b]7;file:///C:/Users/CLEm\x07");
		assert_eq!(path.as_deref(), Some("C:/Users/CLEm"));
	}

	#[test]
	fn iterm2s_current_dir_is_read_as_a_third_spelling() {
		// A dotfile written for iTerm2 announces the cwd this way and nothing else; cmote follows it
		// with no extra configuration (§55).
		let path = track(b"\x1b]1337;CurrentDir=/home/user/work\x07");
		assert_eq!(path.as_deref(), Some("/home/user/work"));
	}

	#[test]
	fn a_current_dir_that_is_not_the_whole_prefix_is_not_a_path() {
		// Matched by its full prefix, so a different 1337 key cannot be mistaken for a directory —
		// most of that namespace is refused outright (`term::iterm`), and none of it lands here.
		assert_eq!(track(b"\x1b]1337;SetProfile=Production\x07"), None);
		assert_eq!(track(b"\x1b]1337;CurrentDirIsh=/tmp\x07"), None);
	}

	#[test]
	fn other_osc_sequences_leave_the_path_alone() {
		// A window title (OSC 0) is not a cwd announcement, and must not clear the one
		// we already have — shells set the title on every prompt too.
		let mut cwd = Cwd::default();
		cwd.feed(b"\x1b]7;file://host/home/user\x07");
		cwd.feed(b"\x1b]0;user@host: ~\x07");
		assert_eq!(cwd.path(), Some("/home/user"));
	}

	#[test]
	fn an_overlong_payload_is_dropped_not_buffered() {
		// A hostile or broken stream must not grow our memory: past the cap the payload
		// is abandoned, and the tracker keeps scanning for the next sequence.
		let mut cwd = Cwd::default();
		cwd.feed(b"\x1b]7;file://host/");
		cwd.feed(&vec![b'x'; MAX_PAYLOAD + 10]);
		cwd.feed(b"\x07");
		assert_eq!(cwd.path(), None);

		cwd.feed(b"\x1b]7;file://host/tmp\x07");
		assert_eq!(cwd.path(), Some("/tmp"));
	}
}

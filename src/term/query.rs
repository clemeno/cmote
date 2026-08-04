// term/query.rs — answer the terminal-identity queries the engine drops (PLAN §9, §33).
//
// `alacritty_terminal` answers the queries that touch the grid itself — DSR, DA, DECRQM,
// cursor-position and text-area reports — and cmote drains those replies straight through
// (`term::mod`). Three queries it does NOT answer: its VT parser treats every DCS string as a
// no-op (its `hook`/`put`/`unhook` just log at debug level), and it has no CSI arm for the
// version request, so all three fall on the floor:
//
//   CSI > q            XTVERSION  — "what terminal are you, and which version?"
//   DCS $ q <sel> ST   DECRQSS    — "what is setting <sel> right now?" (Request Status String)
//   DCS + q <hex> ST   XTGETTCAP  — "what is your value for terminfo capability <hex>?"
//
// A program that sends one waits for a reply; unanswered, it stalls until a timeout, and some
// paste the unanswered bytes as literal garbage. So cmote sniffs these out of the stream itself —
// exactly the tactic `cwd` and `modkeys` use for the sequences the engine ignores — and formats a
// reply. The scanner only PARSES here (it holds no engine state); `term::mod` fills a DECRQSS SGR
// reply from the live pen, because only that one needs to read the grid's current attributes.
//
// The scanner is a byte-at-a-time state machine, not a match over a buffer, because output arrives
// in arbitrary chunks: any sequence can be split anywhere, even between the ESC and the `[`/`P`.

/// The escape byte that leads every CSI (`ESC [`) and DCS (`ESC P`) sequence.
const ESC: u8 = 0x1b;

/// The bell, an alternate string terminator some programs use in place of the canonical `ESC \`.
const BEL: u8 = 0x07;

/// The longest parameter run we buffer inside a `CSI >` sequence. XTVERSION carries none (or a lone
/// `0`); a longer run is some other private query, and refusing to grow past this keeps a hostile
/// stream from ballooning our memory (§12).
const MAX_PARAMS: usize = 16;

/// The longest data string we buffer inside a recognised DCS. A DECRQSS selector is one or two
/// bytes and an XTGETTCAP name list is short; anything longer is malformed or a different DCS
/// (a sixel image), so the scanner abandons it rather than accumulate without bound (§12).
const MAX_DATA: usize = 256;

/// Which DCS query a `DCS … q` introduces, decided by the intermediate byte before the `q`
/// (`$` for DECRQSS, `+` for XTGETTCAP). Carried in the scanner state so the data string that
/// follows is dispatched to the right reader on the terminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DcsKind {
	Decrqss,
	GetTcap,
}

/// A DECRQSS request, reduced to what cmote can answer (§33). `Sgr` is the one setting cmote reads
/// back truthfully — the pen the grid actually paints with; every other setting (cursor shape,
/// scroll margins, conformance level) is `Unsupported`, because cmote either renders it fixed
/// (a block cursor drawn by inverting the cell) or the engine does not expose it. `Unsupported`
/// still earns a reply — an honest `ps=0` that says "I do not report that", which stops the
/// program waiting far more cheaply than a lie about state would cost in wrong behaviour.
#[derive(Debug, PartialEq, Eq)]
pub enum Decrqss {
	Sgr,
	Unsupported,
}

/// A completed query the scanner found in the stream (§33). Usually none arrive in a chunk; when
/// one does, `term::mod` turns it into reply bytes — `Version` and `Capabilities` from static
/// facts about cmote, `Decrqss(Sgr)` from the live pen.
#[derive(Debug, PartialEq, Eq)]
pub enum Query {
	/// XTVERSION (`CSI > q`): answer with cmote's name and version.
	Version,
	/// DECRQSS (`DCS $ q <sel> ST`): answer with the current setting, or that it is unsupported.
	Decrqss(Decrqss),
	/// XTGETTCAP (`DCS + q <hex>[;<hex>…] ST`): the raw hex-encoded capability names, each to be
	/// answered from the small map of facts cmote can state (`known_capability`).
	Capabilities(Vec<Vec<u8>>),
}

/// Where the scanner sits in the byte stream. Only the two shapes cmote answers are tracked in
/// detail — `CSI >` up to its final byte, and a recognised `DCS $ q` / `DCS + q` up to its
/// terminator; every other sequence resets straight back to `Text`. An unrecognised DCS is
/// followed to its terminator all the same (`DcsIgnore`), so its arbitrary data — the one place a
/// stream legitimately carries raw bytes — cannot masquerade as a fresh query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Scan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; a CSI starts on `[`, a DCS on `P`.
	Esc,
	/// Saw `ESC [`; the sequence is one we care about only if the next byte is the `>` marker.
	Csi,
	/// Inside `ESC [ > …`, collecting parameter digits until the final byte.
	CsiGt,
	/// Saw `ESC P`; a DCS we answer starts on the `$` or `+` intermediate.
	Dcs,
	/// Saw `ESC P $`; a DECRQSS request if the next byte is the `q` final.
	DcsDollar,
	/// Saw `ESC P +`; an XTGETTCAP request if the next byte is the `q` final.
	DcsPlus,
	/// Inside a recognised DCS's data string, collecting until the terminator.
	DcsData(DcsKind),
	/// Saw ESC inside a recognised DCS's data; a `\` completes it (`ESC \` is the terminator).
	DcsDataEsc(DcsKind),
	/// Following an unrecognised DCS to its terminator, accumulating nothing.
	DcsIgnore,
	/// Saw ESC inside an unrecognised DCS; a `\` ends it.
	DcsIgnoreEsc,
}

/// The query sniffer (§33). Feed it every byte of shell output; it returns any identity queries
/// that completed in the chunk and ignores everything else. Carries its state across calls, so a
/// query split over a chunk boundary is answered on the chunk that finishes it.
#[derive(Debug, Default)]
pub struct Queries {
	state: Scan,
	params: Vec<u8>,
	data: Vec<u8>,
}

impl Queries {
	/// Scan a chunk of shell output and return the queries that completed in it (usually none).
	/// Safe at any chunk boundary — the state machine carries over between calls.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<Query> {
		let mut found = Vec::new();
		for &byte in bytes {
			match self.state {
				Scan::Text => {
					if byte == ESC {
						self.state = Scan::Esc;
					}
				}
				Scan::Esc => {
					self.state = match byte {
						b'[' => Scan::Csi,
						b'P' => Scan::Dcs,
						// ESC ESC: still waiting for the sequence's real first byte.
						ESC => Scan::Esc,
						_ => Scan::Text,
					};
				}
				Scan::Csi => {
					self.state = match byte {
						b'>' => {
							self.params.clear();
							Scan::CsiGt
						}
						ESC => Scan::Esc,
						// Any other CSI (an SGR colour, a cursor move, `ESC [ c` DA) — not ours.
						_ => Scan::Text,
					};
				}
				Scan::CsiGt => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						// A run longer than any real payload is malformed; drop the sequence.
						if self.params.len() > MAX_PARAMS {
							self.state = Scan::Text;
						}
					}
					b'q' => {
						// XTVERSION is `CSI > q`, or `CSI > 0 q`. A non-zero parameter marks some
						// other private query (a DA2 variant), so only the empty/zero form answers.
						if self.params.is_empty() || self.params.iter().all(|&b| b == b'0') {
							found.push(Query::Version);
						}
						self.state = Scan::Text;
					}
					ESC => self.state = Scan::Esc,
					// `m` (XTMODKEYS), `u` (kitty keyboard), `c` (DA2): handled by modkeys/engine.
					_ => self.state = Scan::Text,
				},
				Scan::Dcs => {
					self.state = match byte {
						b'$' => Scan::DcsDollar,
						b'+' => Scan::DcsPlus,
						ESC => Scan::Esc,
						// Some other DCS (a sixel image, a reply): follow it to its terminator so
						// its data cannot be mistaken for a query, but read nothing from it.
						_ => Scan::DcsIgnore,
					};
				}
				Scan::DcsDollar => {
					self.data.clear();
					self.state = match byte {
						b'q' => Scan::DcsData(DcsKind::Decrqss),
						ESC => Scan::Esc,
						_ => Scan::DcsIgnore,
					};
				}
				Scan::DcsPlus => {
					self.data.clear();
					self.state = match byte {
						b'q' => Scan::DcsData(DcsKind::GetTcap),
						ESC => Scan::Esc,
						_ => Scan::DcsIgnore,
					};
				}
				Scan::DcsData(kind) => match byte {
					// `ESC \` is the canonical string terminator; watch for its ESC.
					ESC => self.state = Scan::DcsDataEsc(kind),
					// BEL is the alternate terminator some programs use.
					BEL => {
						self.complete_dcs(kind, &mut found);
						self.state = Scan::Text;
					}
					_ => {
						self.data.push(byte);
						// Past any real selector or name list: abandon rather than buffer on.
						if self.data.len() > MAX_DATA {
							self.state = Scan::DcsIgnore;
						}
					}
				},
				Scan::DcsDataEsc(kind) => match byte {
					b'\\' => {
						self.complete_dcs(kind, &mut found);
						self.state = Scan::Text;
					}
					ESC => self.state = Scan::DcsDataEsc(kind),
					// A stray ESC that did not form `ESC \`: abandon this DCS.
					_ => self.state = Scan::Text,
				},
				Scan::DcsIgnore => match byte {
					ESC => self.state = Scan::DcsIgnoreEsc,
					BEL => self.state = Scan::Text,
					_ => {}
				},
				Scan::DcsIgnoreEsc => match byte {
					b'\\' => self.state = Scan::Text,
					ESC => self.state = Scan::DcsIgnoreEsc,
					_ => self.state = Scan::Text,
				},
			}
		}
		found
	}

	/// Turn a finished DCS's data string into a `Query`. DECRQSS reads only the SGR setting from
	/// real state; XTGETTCAP hands its (possibly several) hex-encoded capability names on whole.
	fn complete_dcs(&mut self, kind: DcsKind, found: &mut Vec<Query>) {
		match kind {
			DcsKind::Decrqss => {
				// `m` is the SGR selector, the one setting cmote reports truthfully; anything else
				// is answered unsupported (an honest `ps=0`).
				let request = if self.data == b"m" {
					Decrqss::Sgr
				} else {
					Decrqss::Unsupported
				};
				found.push(Query::Decrqss(request));
			}
			DcsKind::GetTcap => {
				// The names are `;`-separated hex; keep them raw for `known_capability` to decode.
				let names = self
					.data
					.split(|&b| b == b';')
					.map(<[u8]>::to_vec)
					.collect();
				found.push(Query::Capabilities(names));
			}
		}
	}
}

/// The XTVERSION reply for an identity string: `DCS > | <id> ST` (§33). `id` is cmote's name and
/// version, e.g. `cmote(3.1.0)`; a program reads it to fingerprint the terminal and enable
/// features. Built by `term::mod` from the crate version so this module carries no build detail.
pub fn version_reply(id: &str) -> Vec<u8> {
	let mut reply = Vec::with_capacity(id.len() + 6);
	reply.extend_from_slice(b"\x1bP>|");
	reply.extend_from_slice(id.as_bytes());
	reply.extend_from_slice(b"\x1b\\");
	reply
}

/// The valid DECRQSS reply for the SGR setting: `DCS 1 $ r <params> m ST` (§33). `params` is the
/// current pen rebuilt as an SGR parameter string (e.g. `0` for a reset pen, `0;1;31` for bold
/// red); the leading `1` marks the request valid and the trailing `m` echoes the setting's own
/// final byte, as DECRQSS requires.
pub fn decrqss_sgr_reply(params: &str) -> Vec<u8> {
	let mut reply = Vec::with_capacity(params.len() + 8);
	reply.extend_from_slice(b"\x1bP1$r");
	reply.extend_from_slice(params.as_bytes());
	reply.extend_from_slice(b"m\x1b\\");
	reply
}

/// The invalid DECRQSS reply: `DCS 0 $ r ST` (§33). The leading `0` tells the program the setting
/// is not reported — the honest answer for a setting cmote renders fixed or cannot read — which it
/// takes as "unsupported" and moves on, rather than waiting on a report that never comes.
pub fn decrqss_unsupported_reply() -> Vec<u8> {
	b"\x1bP0$r\x1b\\".to_vec()
}

/// The XTGETTCAP reply for a list of hex-encoded capability names (§33). Each name is answered on
/// its own: a known capability as `DCS 1 + r <NAME>=<VALUE> ST` with both sides hex-encoded, an
/// unknown one as `DCS 0 + r <NAME> ST` echoing the requested name. cmote reports only the two
/// facts it can state without lying — its terminal name and colour count — and answers every other
/// capability unknown, which is what a well-behaved querier expects for a capability a terminal
/// does not advertise.
pub fn gettcap_reply(names: &[Vec<u8>]) -> Vec<u8> {
	let mut reply = Vec::new();
	for name in names {
		if let Some(decoded) = hex_decode(name)
			&& let Some(value) = known_capability(&decoded)
		{
			// Known: re-encode the decoded name to canonical upper-case hex, matching xterm.
			reply.extend_from_slice(b"\x1bP1+r");
			reply.extend_from_slice(hex_encode(&decoded).as_bytes());
			reply.push(b'=');
			reply.extend_from_slice(hex_encode(value).as_bytes());
			reply.extend_from_slice(b"\x1b\\");
		} else {
			// Unknown (or unparseable hex): echo the requested name verbatim after a `0` status.
			reply.extend_from_slice(b"\x1bP0+r");
			reply.extend_from_slice(name);
			reply.extend_from_slice(b"\x1b\\");
		}
	}
	reply
}

/// The value cmote reports for a terminfo/termcap capability name, or `None` for one it does not
/// advertise (§33). Only two are stated, because only two are facts cmote can give truthfully: the
/// terminal name it requested for the remote pty (`TN`, `xterm-256color`) and its colour count
/// (`Co`/`colors`, 256). Truecolor and the rest are left unknown — their wire values are ambiguous
/// and 24-bit SGR works whether or not a capability query confirms it.
fn known_capability(name: &[u8]) -> Option<&'static [u8]> {
	match name {
		b"TN" => Some(b"xterm-256color"),
		b"Co" | b"colors" => Some(b"256"),
		_ => None,
	}
}

/// Decode an ASCII-hex string (`544E`) to its bytes (`TN`), or `None` if it is empty, odd-length,
/// or holds a non-hex digit. XTGETTCAP hex-encodes every capability name both ways on the wire.
fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
	if hex.is_empty() || !hex.len().is_multiple_of(2) {
		return None;
	}
	let mut out = Vec::with_capacity(hex.len() / 2);
	for pair in hex.chunks_exact(2) {
		out.push((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?);
	}
	Some(out)
}

/// One ASCII-hex digit as its value, accepting either case, or `None` for a non-hex byte.
fn hex_digit(byte: u8) -> Option<u8> {
	match byte {
		b'0'..=b'9' => Some(byte - b'0'),
		b'a'..=b'f' => Some(byte - b'a' + 10),
		b'A'..=b'F' => Some(byte - b'A' + 10),
		_ => None,
	}
}

/// Encode bytes as upper-case ASCII hex, the form xterm uses in an XTGETTCAP reply.
fn hex_encode(bytes: &[u8]) -> String {
	use std::fmt::Write as _;
	let mut out = String::with_capacity(bytes.len() * 2);
	for &byte in bytes {
		// `write!` to a String never fails; the result is discarded deliberately.
		let _ = write!(out, "{byte:02X}");
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Feed one byte slice to a fresh scanner and return the queries it found.
	fn scan(bytes: &[u8]) -> Vec<Query> {
		Queries::default().feed(bytes)
	}

	#[test]
	fn a_version_request_is_recognised() {
		// XTVERSION `CSI > q`, and the `CSI > 0 q` spelling with an explicit zero parameter.
		assert_eq!(scan(b"\x1b[>q"), vec![Query::Version]);
		assert_eq!(scan(b"\x1b[>0q"), vec![Query::Version]);
	}

	#[test]
	fn other_private_csi_sequences_are_not_version_requests() {
		// The `CSI >` prefix is shared: XTMODKEYS ends in `m`, kitty in `u`, DA2 in `c`, and a
		// non-zero-parametered `q` is a different private query — none is XTVERSION.
		assert!(scan(b"\x1b[>4;2m").is_empty());
		assert!(scan(b"\x1b[>1u").is_empty());
		assert!(scan(b"\x1b[>c").is_empty());
		assert!(scan(b"\x1b[>1q").is_empty());
	}

	#[test]
	fn a_decrqss_sgr_request_is_recognised() {
		// `DCS $ q m ST` asks for the current SGR — the one DECRQSS setting cmote reports.
		assert_eq!(scan(b"\x1bP$qm\x1b\\"), vec![Query::Decrqss(Decrqss::Sgr)]);
	}

	#[test]
	fn other_decrqss_requests_are_unsupported() {
		// Cursor shape (`SP q`), scroll margins (`r`), conformance (`"p`): cmote renders them fixed
		// or cannot read them, so each is honestly reported unsupported rather than guessed at.
		assert_eq!(
			scan(b"\x1bP$q q\x1b\\"),
			vec![Query::Decrqss(Decrqss::Unsupported)]
		);
		assert_eq!(
			scan(b"\x1bP$qr\x1b\\"),
			vec![Query::Decrqss(Decrqss::Unsupported)]
		);
		assert_eq!(
			scan(b"\x1bP$q\"p\x1b\\"),
			vec![Query::Decrqss(Decrqss::Unsupported)]
		);
	}

	#[test]
	fn an_xtgettcap_request_carries_its_hex_names() {
		// `DCS + q 544E ST` asks for `TN` (hex 54 4E); the scanner hands the raw hex on.
		assert_eq!(
			scan(b"\x1bP+q544E\x1b\\"),
			vec![Query::Capabilities(vec![b"544E".to_vec()])]
		);
		// Several names are `;`-separated: `Co` (436F) and `TN` (544E).
		assert_eq!(
			scan(b"\x1bP+q436F;544E\x1b\\"),
			vec![Query::Capabilities(vec![
				b"436F".to_vec(),
				b"544E".to_vec()
			])]
		);
	}

	#[test]
	fn a_sixel_dcs_does_not_trip_the_scanner() {
		// An unrecognised DCS (here a sixel `DCS q … ST`) is followed to its terminator and read
		// as nothing — even though its data contains bytes that look like a `>q` version request.
		assert!(scan(b"\x1bPq\"1;1;10;10#0>q\x1b\\").is_empty());
	}

	#[test]
	fn a_request_split_across_chunks_is_answered_on_completion() {
		// Output arrives in arbitrary chunks; a version request split after the `>` still reads,
		// and a DCS split mid-data completes on the chunk that terminates it.
		let mut queries = Queries::default();
		assert!(queries.feed(b"text\x1b[>").is_empty());
		assert_eq!(queries.feed(b"qmore"), vec![Query::Version]);
		assert!(queries.feed(b"\x1bP+q54").is_empty());
		assert_eq!(
			queries.feed(b"4E\x1b\\"),
			vec![Query::Capabilities(vec![b"544E".to_vec()])]
		);
	}

	#[test]
	fn a_bell_terminates_a_dcs_query() {
		// BEL is accepted as the string terminator in place of `ESC \`.
		assert_eq!(scan(b"\x1bP$qm\x07"), vec![Query::Decrqss(Decrqss::Sgr)]);
	}

	#[test]
	fn the_version_reply_frames_the_identity() {
		// `DCS > | <id> ST` around the identity string.
		assert_eq!(
			version_reply("cmote(3.1.0)"),
			b"\x1bP>|cmote(3.1.0)\x1b\\".to_vec()
		);
	}

	#[test]
	fn the_sgr_reply_frames_the_parameters() {
		// A reset pen reports `0`; the reply is `DCS 1 $ r 0 m ST`.
		assert_eq!(decrqss_sgr_reply("0"), b"\x1bP1$r0m\x1b\\".to_vec());
		assert_eq!(
			decrqss_sgr_reply("0;1;31"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	#[test]
	fn the_unsupported_reply_is_a_zero_status() {
		assert_eq!(decrqss_unsupported_reply(), b"\x1bP0$r\x1b\\".to_vec());
	}

	#[test]
	fn a_known_capability_is_answered_with_its_value() {
		// `TN` (544E) -> `xterm-256color`, name and value both upper-case hex, framed `DCS 1 + r`.
		// `xterm-256color` = 78 74 65 72 6D 2D 32 35 36 63 6F 6C 6F 72.
		let reply = gettcap_reply(&[b"544E".to_vec()]);
		assert_eq!(
			reply,
			b"\x1bP1+r544E=787465726D2D323536636F6C6F72\x1b\\".to_vec()
		);
	}

	#[test]
	fn the_colour_count_is_answered() {
		// `Co` (436F) -> `256` (32 35 36).
		assert_eq!(
			gettcap_reply(&[b"436F".to_vec()]),
			b"\x1bP1+r436F=323536\x1b\\".to_vec()
		);
	}

	#[test]
	fn an_unknown_capability_is_answered_with_a_zero_status() {
		// `zz` (7A7A) is not advertised: `DCS 0 + r 7A7A ST`, echoing the requested name.
		assert_eq!(
			gettcap_reply(&[b"7A7A".to_vec()]),
			b"\x1bP0+r7A7A\x1b\\".to_vec()
		);
	}

	#[test]
	fn a_lower_case_hex_name_still_decodes() {
		// The requested hex may be either case; `544e` is still `TN`, echoed as canonical `544E`.
		assert_eq!(
			gettcap_reply(&[b"544e".to_vec()]),
			b"\x1bP1+r544E=787465726D2D323536636F6C6F72\x1b\\".to_vec()
		);
	}
}

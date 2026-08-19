// term/query.rs — answer the terminal-identity queries the engine drops (PLAN §9, §33).
//
// `alacritty_terminal` answers the queries that touch the grid itself — DSR, DA, DECRQM,
// cursor-position and text-area reports — and cmote drains those replies straight through
// (`term::mod`). Five queries it does NOT answer: its VT parser treats every DCS string as a
// no-op (its `hook`/`put`/`unhook` just log at debug level), it has no CSI arm for the version
// request or for the graphics-capability one, and its device-attributes handler covers only the
// primary and secondary forms (the `=` intermediate falls to a debug log), so all five fall on the
// floor:
//
//   CSI > q            XTVERSION  — "what terminal are you, and which version?"
//   DCS $ q <sel> ST   DECRQSS    — "what is setting <sel> right now?" (Request Status String)
//   DCS + q <hex> ST   XTGETTCAP  — "what is your value for terminfo capability <hex>?"
//   CSI = c            DA3        — "what is your unit id?" (DECRQTSR / tertiary attributes, §36)
//   CSI ? Pi;Pa;Pv S   XTSMGRAPHICS — "how big a picture, and how many colours?" (§41)
//
// The graphics one is what makes inline images (§41) usable rather than merely supported: a program
// deciding HOW to show a picture asks how many colour registers and how large an image the terminal
// will take, and one that gets no answer falls back to text art or gives up. It is answered from the
// limits `term::sixel` actually enforces, so the reply is a promise cmote keeps.
//
// One reply here is not to a query cmote sniffed but an AMENDMENT to one the engine wrote:
// `with_sixel_attribute` adds the sixel capability to the engine's DA1 answer (see it for why that
// is a rewrite rather than a second reply).
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

/// The longest parameter run we buffer inside a `CSI >` or `CSI =` sequence. XTVERSION and DA3
/// carry none (or a lone `0`); a longer run is some other private query, and refusing to grow past
/// this keeps a hostile stream from ballooning our memory (§12).
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

/// An XTSMGRAPHICS request (`CSI ? Pi ; Pa ; Pv S`, §41): `item` is what is being asked about
/// (1 colour registers, 2 sixel geometry, 3 ReGIS geometry) and `action` what is wanted of it
/// (1 read, 2 reset to default, 3 set, 4 read the maximum). cmote's graphics limits are fixed, so a
/// read of either kind is answered with the real number and a *set* is honestly refused — see
/// `graphics_reply`. The trailing `Pv` values a set would carry are not kept for exactly that reason.
#[derive(Debug, PartialEq, Eq)]
pub struct Graphics {
	pub item: u16,
	pub action: u16,
}

/// A completed query the scanner found in the stream (§33, §36, §41). Usually none arrive in a
/// chunk; when one does, `term::mod` turns it into reply bytes — `Version`, `Capabilities`, `UnitId`
/// and `Graphics` from static facts about cmote, `Decrqss(Sgr)` from the live pen.
#[derive(Debug, PartialEq, Eq)]
pub enum Query {
	/// XTVERSION (`CSI > q`): answer with cmote's name and version.
	Version,
	/// DECRQSS (`DCS $ q <sel> ST`): answer with the current setting, or that it is unsupported.
	Decrqss(Decrqss),
	/// XTGETTCAP (`DCS + q <hex>[;<hex>…] ST`): the raw hex-encoded capability names, each to be
	/// answered from the small map of facts cmote can state (`known_capability`).
	Capabilities(Vec<Vec<u8>>),
	/// DA3, tertiary device attributes (`CSI = c`): answer with cmote's unit id (§36).
	UnitId,
	/// XTSMGRAPHICS (`CSI ? Pi ; Pa ; Pv S`): answer with cmote's graphics limits (§41).
	Graphics(Graphics),
}

/// Where the scanner sits in the byte stream. Only the shapes cmote answers are tracked in
/// detail — `CSI >` and `CSI =` up to their final byte, and a recognised `DCS $ q` / `DCS + q` up
/// to its terminator; every other sequence resets straight back to `Text`. An unrecognised DCS is
/// followed to its terminator all the same (`DcsIgnore`), so its arbitrary data — the one place a
/// stream legitimately carries raw bytes — cannot masquerade as a fresh query.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum QueryScan {
	/// Ordinary output; waiting for an ESC.
	#[default]
	Text,
	/// Saw ESC; a CSI starts on `[`, a DCS on `P`.
	Esc,
	/// Saw `ESC [`; the sequence is one we care about only if the next byte is the `>` or `=`
	/// private marker.
	Csi,
	/// Inside `ESC [ > …`, collecting parameter digits until the final byte.
	CsiGt,
	/// Inside `ESC [ = …`, collecting parameter digits until the final byte (DA3, §36).
	CsiEq,
	/// Inside `ESC [ ? …`, collecting parameter digits until the final byte (XTSMGRAPHICS, §41).
	/// Every DECSET/DECRST (`CSI ? 1049 h`) passes through here too and leaves on its own final
	/// byte, unread — the engine owns those.
	CsiQuestion,
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
	state: QueryScan,
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
				QueryScan::Text => {
					if byte == ESC {
						self.state = QueryScan::Esc;
					}
				}
				QueryScan::Esc => {
					self.state = match byte {
						b'[' => QueryScan::Csi,
						b'P' => QueryScan::Dcs,
						// ESC ESC: still waiting for the sequence's real first byte.
						ESC => QueryScan::Esc,
						_ => QueryScan::Text,
					};
				}
				QueryScan::Csi => {
					self.state = match byte {
						b'>' => {
							self.params.clear();
							QueryScan::CsiGt
						}
						b'=' => {
							self.params.clear();
							QueryScan::CsiEq
						}
						b'?' => {
							self.params.clear();
							QueryScan::CsiQuestion
						}
						ESC => QueryScan::Esc,
						// Any other CSI (an SGR colour, a cursor move, `ESC [ c` DA1) — not ours.
						_ => QueryScan::Text,
					};
				}
				QueryScan::CsiGt => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						// A run longer than any real payload is malformed; drop the sequence.
						if self.params.len() > MAX_PARAMS {
							self.state = QueryScan::Text;
						}
					}
					b'q' => {
						// XTVERSION is `CSI > q`, or `CSI > 0 q`. A non-zero parameter marks some
						// other private query (a DA2 variant), so only the empty/zero form answers.
						if self.default_params() {
							found.push(Query::Version);
						}
						self.state = QueryScan::Text;
					}
					ESC => self.state = QueryScan::Esc,
					// `m` (XTMODKEYS), `u` (kitty keyboard), `c` (DA2): handled by modkeys/engine.
					_ => self.state = QueryScan::Text,
				},
				QueryScan::CsiEq => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						// Same bound as `CSI >`: a longer run is not a query we answer (§12).
						if self.params.len() > MAX_PARAMS {
							self.state = QueryScan::Text;
						}
					}
					b'c' => {
						// DA3 is `CSI = c`, or `CSI = 0 c` — the tertiary device-attributes request
						// (§36). The engine's `identify_terminal` handles the no-intermediate (DA1)
						// and `>` (DA2) forms and drops this one, so it falls to cmote. As with DA1
						// and DA2, only the empty/zero parameter form is the request.
						if self.default_params() {
							found.push(Query::UnitId);
						}
						self.state = QueryScan::Text;
					}
					ESC => self.state = QueryScan::Esc,
					// Any other `CSI =` final byte is a private sequence cmote does not answer.
					_ => self.state = QueryScan::Text,
				},
				QueryScan::CsiQuestion => match byte {
					b'0'..=b'9' | b';' => {
						self.params.push(byte);
						// Same bound as the other private forms; a DECSET parameter list never
						// approaches it (§12).
						if self.params.len() > MAX_PARAMS {
							self.state = QueryScan::Text;
						}
					}
					b'S' => {
						// XTSMGRAPHICS (§41). The engine has no arm for the `?` form of `CSI S` — its
						// only `S` is SU, scroll-up, with no intermediate — so the whole request falls
						// to cmote. A request naming neither an item nor an action is malformed and
						// left unanswered rather than guessed at.
						if let Some(request) = graphics_request(&self.params) {
							found.push(Query::Graphics(request));
						}
						self.state = QueryScan::Text;
					}
					ESC => self.state = QueryScan::Esc,
					// Every other `CSI ?` sequence — DECSET/DECRST (`h`/`l`), DECRQM (`$p`), the
					// kitty keyboard query (`u`) — belongs to the engine.
					_ => self.state = QueryScan::Text,
				},
				QueryScan::Dcs => {
					self.state = match byte {
						b'$' => QueryScan::DcsDollar,
						b'+' => QueryScan::DcsPlus,
						ESC => QueryScan::Esc,
						// Some other DCS (a sixel image, a reply): follow it to its terminator so
						// its data cannot be mistaken for a query, but read nothing from it.
						_ => QueryScan::DcsIgnore,
					};
				}
				QueryScan::DcsDollar => {
					self.data.clear();
					self.state = match byte {
						b'q' => QueryScan::DcsData(DcsKind::Decrqss),
						ESC => QueryScan::Esc,
						_ => QueryScan::DcsIgnore,
					};
				}
				QueryScan::DcsPlus => {
					self.data.clear();
					self.state = match byte {
						b'q' => QueryScan::DcsData(DcsKind::GetTcap),
						ESC => QueryScan::Esc,
						_ => QueryScan::DcsIgnore,
					};
				}
				QueryScan::DcsData(kind) => match byte {
					// `ESC \` is the canonical string terminator; watch for its ESC.
					ESC => self.state = QueryScan::DcsDataEsc(kind),
					// BEL is the alternate terminator some programs use.
					BEL => {
						self.complete_dcs(kind, &mut found);
						self.state = QueryScan::Text;
					}
					_ => {
						self.data.push(byte);
						// Past any real selector or name list: abandon rather than buffer on.
						if self.data.len() > MAX_DATA {
							self.state = QueryScan::DcsIgnore;
						}
					}
				},
				QueryScan::DcsDataEsc(kind) => match byte {
					b'\\' => {
						self.complete_dcs(kind, &mut found);
						self.state = QueryScan::Text;
					}
					ESC => self.state = QueryScan::DcsDataEsc(kind),
					// A stray ESC that did not form `ESC \`: abandon this DCS.
					_ => self.state = QueryScan::Text,
				},
				QueryScan::DcsIgnore => match byte {
					ESC => self.state = QueryScan::DcsIgnoreEsc,
					BEL => self.state = QueryScan::Text,
					_ => {}
				},
				QueryScan::DcsIgnoreEsc => match byte {
					b'\\' => self.state = QueryScan::Text,
					ESC => self.state = QueryScan::DcsIgnoreEsc,
					_ => self.state = QueryScan::Text,
				},
			}
		}
		found
	}

	/// Whether the parameter run collected so far is the *default* one — empty, or nothing but
	/// zeros. Both private queries cmote answers (XTVERSION `CSI > q`, DA3 `CSI = c`) are defined
	/// only in that form; a non-zero parameter on the same final byte is a different private
	/// sequence, so the scanner stays silent rather than answer a question it was not asked.
	fn default_params(&self) -> bool {
		self.params.is_empty() || self.params.iter().all(|&byte| byte == b'0')
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

/// Read an XTSMGRAPHICS request out of a collected parameter run (§41). The first two parameters are
/// the item and the action; anything after them belongs to a *set*, which cmote refuses, so it is
/// not read. `None` when either is missing or unparseable — an unanswered malformed request is
/// better than an answer about something the program did not ask for.
fn graphics_request(params: &[u8]) -> Option<Graphics> {
	let mut fields = params.split(|&byte| byte == b';');
	let item = number(fields.next()?)?;
	let action = number(fields.next()?)?;
	Some(Graphics { item, action })
}

/// One decimal parameter as a number, or `None` when it is empty or too long to be one. Saturating
/// digits, so a remote's overlong run clamps instead of wrapping into a small plausible value (§12).
fn number(field: &[u8]) -> Option<u16> {
	if field.is_empty() {
		return None;
	}
	let mut value = 0u16;
	for &byte in field {
		let digit = byte.checked_sub(b'0').filter(|digit| *digit <= 9)?;
		value = value.saturating_mul(10).saturating_add(u16::from(digit));
	}
	Some(value)
}

/// The XTSMGRAPHICS reply: `CSI ? Pi ; Ps ; Pv S`, where `Ps` is the status — 0 success, 1 an item
/// this terminal knows nothing about, 3 a failure (§41).
///
/// cmote's graphics limits are fixed by the decoder (`term::sixel`), which is what makes the answers
/// simple: a READ (action 1) or a read-of-the-maximum (action 4) reports the real number, and so does
/// a RESET (action 2), since resetting a fixed capacity to its default lands on that same number. A
/// SET (action 3) is refused with a status 3 and the unchanged value — the honest answer, and the one
/// that leaves the program correctly believing what cmote will actually accept. Anything but the
/// colour registers (item 1) and the sixel geometry (item 2) is answered "unknown item": cmote has no
/// ReGIS (item 3) and never will, so claiming a geometry for it would be a lie a program could act on.
pub fn graphics_reply(request: &Graphics, registers: u16, geometry: (u16, u16)) -> Vec<u8> {
	// A set is the one action that cannot succeed; every other one reports the fixed truth.
	let status = if request.action == 3 { 3 } else { 0 };
	let (width, height) = geometry;
	match request.item {
		1 => format!("\x1b[?1;{status};{registers}S").into_bytes(),
		2 => format!("\x1b[?2;{status};{width};{height}S").into_bytes(),
		item => format!("\x1b[?{item};1S").into_bytes(),
	}
}

/// Add the sixel capability to a DA1 reply the ENGINE wrote (§41).
///
/// DA1 (`CSI c`, "what terminal are you?") is one of the queries `alacritty_terminal` answers itself
/// — `CSI ? 6 c`, a VT102 — and a terminal advertises inline images by listing attribute **4** in
/// that answer. Programs that pick a picture format at startup (chafa's auto mode, lsix, ranger's
/// previewer) read exactly that; without the 4 they fall back to text art, and cmote's images would
/// go unused however well they work.
///
/// So this rewrites the engine's own reply instead of sending a second one: two DA1 answers to one
/// query would leave the program parsing the second as input, and suppressing the engine's would mean
/// cutting bytes out of the stream on their way IN — surgery on a byte stream a program is mid-
/// sequence in. Amending what cmote is about to send is the same fact stated once, at the only point
/// where both halves of the answer are known.
///
/// A reply that already names 4, and any other reply passing through (DECRQM, a cursor report, the
/// kitty keyboard answer), is returned untouched: only `CSI ? <digits and semicolons> c` is a DA1.
pub fn with_sixel_attribute(reply: Vec<u8>) -> Vec<u8> {
	// The overwhelmingly common case is a chunk with no DA1 in it (usually no reply at all), so it
	// costs one scan for the prefix and no allocation.
	if !reply
		.windows(DA1_PREFIX.len())
		.any(|window| window == DA1_PREFIX)
	{
		return reply;
	}
	let mut out = Vec::with_capacity(reply.len() + SIXEL_ATTRIBUTE.len());
	let mut index = 0;
	while index < reply.len() {
		let rest = &reply[index..];
		if let Some(params) = rest.strip_prefix(DA1_PREFIX)
			&& let Some(end) = da1_params_end(params)
		{
			let params = &params[..end];
			out.extend_from_slice(DA1_PREFIX);
			out.extend_from_slice(params);
			// Already advertised (a future engine version might): say it once.
			if !params
				.split(|&byte| byte == b';')
				.any(|field| field == b"4")
			{
				out.extend_from_slice(SIXEL_ATTRIBUTE);
			}
			out.push(b'c');
			// Past the whole reply, terminator included.
			index += DA1_PREFIX.len() + end + 1;
			continue;
		}
		out.push(reply[index]);
		index += 1;
	}
	out
}

/// How the engine opens a DA1 reply, and the parameter that says "this terminal draws sixels".
/// Separated so `with_sixel_attribute` reads as the shape it is looking for.
const DA1_PREFIX: &[u8] = b"\x1b[?";
const SIXEL_ATTRIBUTE: &[u8] = b";4";

/// How many bytes of `params` are a DA1's parameter run — i.e. the offset of its closing `c`, or
/// `None` when what follows the `CSI ?` is not a DA1 at all (a DECRQM report ends in `$y`, a kitty
/// keyboard report in `u`). Only digits and semicolons may precede the `c`.
fn da1_params_end(params: &[u8]) -> Option<usize> {
	params
		.iter()
		.position(|&byte| !matches!(byte, b'0'..=b'9' | b';'))
		.filter(|&end| params[end] == b'c')
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

/// The DA3 reply, DECRPTUI: `DCS ! | <unit id> ST` (§36). `unit_id` is eight hex digits — a
/// two-digit manufacturing site followed by a six-digit terminal id — and comes from `term::mod`,
/// so this module carries no identity detail (the same split `version_reply` uses).
///
/// SECURITY: on real DEC hardware those digits were the unit's serial number, which is exactly why
/// a *constant* is the right answer here: a per-machine value would hand every remote host a stable
/// fingerprint of the client machine for free, on a query the user never sees. cmote reports the
/// same eight digits from every install, so the reply identifies the *program*, not the person.
pub fn da3_reply(unit_id: &str) -> Vec<u8> {
	let mut reply = Vec::with_capacity(unit_id.len() + 6);
	reply.extend_from_slice(b"\x1bP!|");
	reply.extend_from_slice(unit_id.as_bytes());
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
	fn a_tertiary_attributes_request_is_recognised() {
		// DA3 `CSI = c`, and the `CSI = 0 c` spelling with an explicit zero parameter (§36).
		assert_eq!(scan(b"\x1b[=c"), vec![Query::UnitId]);
		assert_eq!(scan(b"\x1b[=0c"), vec![Query::UnitId]);
	}

	#[test]
	fn the_other_device_attributes_forms_are_left_to_the_engine() {
		// DA1 (`CSI c`) and DA2 (`CSI > c`) the engine answers itself, so the scanner must not
		// also claim them — a doubled reply would confuse the program that asked. And a
		// parametered `CSI = 1 c` is some other private sequence, not the unit-id request.
		assert!(scan(b"\x1b[c").is_empty());
		assert!(scan(b"\x1b[0c").is_empty());
		assert!(scan(b"\x1b[>c").is_empty());
		assert!(scan(b"\x1b[=1c").is_empty());
		// A `CSI =` sequence ending in some other final byte is not DA3 either.
		assert!(scan(b"\x1b[=m").is_empty());
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
	fn the_unit_id_reply_frames_the_digits() {
		// DECRPTUI: `DCS ! | <unit id> ST` around the eight hex digits (§36).
		assert_eq!(da3_reply("00434D45"), b"\x1bP!|00434D45\x1b\\".to_vec());
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
	fn a_graphics_request_carries_its_item_and_action() {
		// XTSMGRAPHICS `CSI ? 1 ; 1 S` — "read the colour register count" (§41). The trailing values
		// a set would carry are not part of the request cmote answers.
		assert_eq!(
			scan(b"\x1b[?1;1S"),
			vec![Query::Graphics(Graphics { item: 1, action: 1 })]
		);
		assert_eq!(
			scan(b"\x1b[?2;4S"),
			vec![Query::Graphics(Graphics { item: 2, action: 4 })]
		);
		assert_eq!(
			scan(b"\x1b[?1;3;16S"),
			vec![Query::Graphics(Graphics { item: 1, action: 3 })]
		);
	}

	#[test]
	fn the_engines_own_private_modes_are_not_graphics_requests() {
		// The `CSI ?` prefix is shared with every DECSET/DECRST there is, plus DECRQM and the kitty
		// keyboard query — all the engine's, and none of them ending in `S`. A request with too few
		// parameters is malformed and stays unanswered.
		assert!(scan(b"\x1b[?1049h").is_empty());
		assert!(scan(b"\x1b[?25l").is_empty());
		assert!(scan(b"\x1b[?2026$p").is_empty());
		assert!(scan(b"\x1b[?u").is_empty());
		assert!(scan(b"\x1b[?1S").is_empty());
		assert!(scan(b"\x1b[?S").is_empty());
	}

	#[test]
	fn the_graphics_reply_states_the_registers_and_the_geometry() {
		// A read of item 1 reports the colour register count with a success status; a read of item 2
		// reports the maximum image size (§41).
		assert_eq!(
			graphics_reply(&Graphics { item: 1, action: 1 }, 256, (4096, 4096)),
			b"\x1b[?1;0;256S".to_vec()
		);
		assert_eq!(
			graphics_reply(&Graphics { item: 2, action: 4 }, 256, (4096, 2048)),
			b"\x1b[?2;0;4096;2048S".to_vec()
		);
	}

	#[test]
	fn setting_a_graphics_limit_is_refused_with_the_real_value() {
		// cmote's limits are the decoder's, so a set cannot succeed: status 3, and the value it will
		// in fact keep to — which leaves the program believing something true.
		assert_eq!(
			graphics_reply(&Graphics { item: 1, action: 3 }, 256, (4096, 4096)),
			b"\x1b[?1;3;256S".to_vec()
		);
		// A reset lands on that same fixed number, so it succeeds.
		assert_eq!(
			graphics_reply(&Graphics { item: 1, action: 2 }, 256, (4096, 4096)),
			b"\x1b[?1;0;256S".to_vec()
		);
	}

	#[test]
	fn an_unknown_graphics_item_is_answered_unknown() {
		// Item 3 is ReGIS, which cmote has none of: status 1 rather than a geometry it cannot honour.
		assert_eq!(
			graphics_reply(&Graphics { item: 3, action: 1 }, 256, (4096, 4096)),
			b"\x1b[?3;1S".to_vec()
		);
	}

	#[test]
	fn the_engines_da1_reply_gains_the_sixel_attribute() {
		// The engine answers DA1 with `CSI ? 6 c`; cmote draws sixels, so the reply must say 4 (§41).
		assert_eq!(
			with_sixel_attribute(b"\x1b[?6c".to_vec()),
			b"\x1b[?6;4c".to_vec()
		);
		// A DA1 among other replies is amended in place, and the rest is passed through byte for byte.
		assert_eq!(
			with_sixel_attribute(b"\x1b[0n\x1b[?6c\x1b[1;1R".to_vec()),
			b"\x1b[0n\x1b[?6;4c\x1b[1;1R".to_vec()
		);
	}

	#[test]
	fn a_reply_that_is_not_a_da1_is_left_exactly_as_it_was() {
		// Other `CSI ?` replies share the prefix: a DECRQM report (`$y`) and the kitty keyboard
		// report (`u`) must pass through untouched, or a program would read a mangled answer.
		assert_eq!(
			with_sixel_attribute(b"\x1b[?2026;2$y".to_vec()),
			b"\x1b[?2026;2$y".to_vec()
		);
		assert_eq!(
			with_sixel_attribute(b"\x1b[?1u".to_vec()),
			b"\x1b[?1u".to_vec()
		);
		// And an empty reply — the usual case — is returned as it came.
		assert!(with_sixel_attribute(Vec::new()).is_empty());
	}

	#[test]
	fn a_da1_that_already_advertises_sixel_is_not_amended_twice() {
		// If a future engine version lists 4 itself, the attribute is stated once — and `14` is not a
		// `4`, so a reply carrying it still gains the real parameter.
		assert_eq!(
			with_sixel_attribute(b"\x1b[?62;4;22c".to_vec()),
			b"\x1b[?62;4;22c".to_vec()
		);
		assert_eq!(
			with_sixel_attribute(b"\x1b[?62;14c".to_vec()),
			b"\x1b[?62;14;4c".to_vec()
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

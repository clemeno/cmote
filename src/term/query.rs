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
// reply. The scanner only PARSES here (it holds no engine state); `term::mod::decrqss_report` fills
// every DECRQSS reply from live state, because each of those five settings is a thing the grid is
// currently doing rather than a fact about cmote (§123).
//
// Any sequence can be split anywhere, even between the ESC and the `[`/`P`, because output arrives in
// arbitrary chunks. Neither half of that framing is this module's any more: `csi::Framer` cuts out the
// three private CSI forms and `dcs::Framer` the two control strings (§111). What is left here is the
// only part that was ever query-specific — the marker, intermediate and final-byte tables that say
// which question a finished sequence asked, and how to answer it.

/// The longest data string we buffer inside a recognised DCS. A DECRQSS selector is one or two
/// bytes and an XTGETTCAP name list is short; anything longer is malformed or a different DCS
/// (a sixel image), so the framer abandons it rather than accumulate without bound (§12).
const MAX_DATA: usize = 256;

/// A DECRQSS request, reduced to what cmote can answer (§33, §123).
///
/// Each variant is a setting cmote holds live state for and can therefore report truthfully; the
/// state itself is read by `term::mod`, because this module parses and never touches the engine.
/// Everything else is `Unsupported` — an honest `Ps=0` that says "I do not report that", which stops
/// the program waiting far more cheaply than a lie about state would cost in wrong behaviour.
///
/// §66 inherited three of these from the partial row it retired, on the finding that a mark reading
/// "partial" had never been asked *which part*: the declined half was not a refusal, it was
/// unwritten reporting code over state that already existed. `Margins` is the fourth, and it exists
/// for the same reason one section later — §102 gave cmote the left and right margins, so the answer
/// was sitting there too.
#[derive(Debug, PartialEq, Eq)]
pub enum Decrqss {
	/// `m` — SGR, the pen the grid actually paints with.
	Sgr,
	/// `SP q` — DECSCUSR, the cursor's shape (§23).
	CursorStyle,
	/// `" q` — DECSCA, whether what is printed now is protected from a selective erase (§56).
	Protection,
	/// `r` — DECSTBM, the top and bottom lines of the scrolling region (§102).
	ScrollRegion,
	/// `s` — DECSLRM, the left and right margins (§102).
	Margins,
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
/// and `Graphics` from static facts about cmote, every `Decrqss` from live state (§123).
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

/// The query sniffer (§33). Feed it every byte of shell output; it returns any identity queries
/// that completed in the chunk and ignores everything else. Carries its state across calls, so a
/// query split over a chunk boundary is answered on the chunk that finishes it.
///
/// Two framers and no state of its own, which is what §111 left this module as: one grammar for the
/// three private CSI forms, one for the two control strings, and a table apiece for what they mean.
#[derive(Debug, Default)]
pub struct Queries {
	/// The CSI grammar, shared with the other scanners (§111) — the three private forms cmote answers.
	framer: super::csi::Framer,
	/// The control-string grammar, shared with `graphics` (§111). Bounded at [`MAX_DATA`], which is
	/// where a sixel payload arriving on this scanner gets dropped rather than buffered.
	strings: super::dcs::Framer<MAX_DATA>,
}

impl Queries {
	/// Scan a chunk of shell output and return the queries that completed in it (usually none).
	/// Safe at any chunk boundary — the state machine carries over between calls.
	///
	/// **In stream order**, which matters here more than anywhere else in the directory: these turn
	/// into REPLY BYTES sent back to the remote, and a program that asked two questions matches the
	/// answers to them by position. Two passes over the chunk would otherwise put every CSI answer
	/// after every DCS one, so each is collected with the offset it completed at and the two are merged
	/// on it. The offsets are then dropped — no caller wants them, and none of these events is fed back
	/// to the engine.
	pub fn feed(&mut self, bytes: &[u8]) -> Vec<Query> {
		let mut found = Vec::new();
		// Destructured so both framers can be borrowed in turn while the closures hold `found`.
		let Self { framer, strings } = self;
		strings.feed(bytes, |span, control| {
			if let super::dcs::Control::String(dcs) = control
				&& let Some(query) = asked_string(dcs)
			{
				found.push((span.past(), query));
			}
		});
		framer.feed(bytes, |span, csi| {
			if let Some(query) = asked(csi) {
				found.push((span.past(), query));
			}
		});
		found.sort_by_key(|&(offset, _)| offset);
		found.into_iter().map(|(_, query)| query).collect()
	}
}

/// Which query a finished control string is, or `None` when it is not one cmote answers.
///
/// The two DCS forms, told apart by their intermediate byte — `$` for DECRQSS, `+` for XTGETTCAP —
/// which is all that separates them from a sixel image, since all three end in `q` (§41).
///
/// Both are defined with NO parameters and no private marker, so a string carrying either is some
/// other sequence on the same spelling and goes unanswered. `param_count` is what says so, and it
/// counts an empty parameter: `DCS ; $ q` names two of them, not none.
fn asked_string(dcs: &super::dcs::Dcs<'_>) -> Option<Query> {
	if dcs.final_byte() != b'q' || dcs.marker().is_some() || dcs.param_count() != 0 {
		return None;
	}
	match dcs.intermediates() {
		// DECRQSS. The payload is the queried sequence's own intermediates and final byte, so each
		// arm below is spelled exactly as that sequence is: DECSCUSR is `CSI Ps SP q`, hence a space
		// then `q`, and DECSCA is `CSI Ps " q`, hence a quote then `q` with no space between. Any
		// selector cmote holds no state for is answered unsupported (an honest `Ps=0`).
		[b'$'] => Some(Query::Decrqss(match dcs.payload() {
			b"m" => Decrqss::Sgr,
			b" q" => Decrqss::CursorStyle,
			b"\"q" => Decrqss::Protection,
			b"r" => Decrqss::ScrollRegion,
			b"s" => Decrqss::Margins,
			_ => Decrqss::Unsupported,
		})),
		// XTGETTCAP. The names are `;`-separated hex; keep them raw for `known_capability` to decode.
		[b'+'] => Some(Query::Capabilities(
			dcs.payload()
				.split(|&byte| byte == b';')
				.map(<[u8]>::to_vec)
				.collect(),
		)),
		_ => None,
	}
}

/// Read an XTSMGRAPHICS request out of a collected parameter run (§41). The first two parameters are
/// the item and the action; anything after them belongs to a *set*, which cmote refuses, so it is
/// not read. `None` when either is missing or unparseable — an unanswered malformed request is
/// better than an answer about something the program did not ask for.
fn graphics_request(csi: &super::csi::Csi<'_>) -> Option<Graphics> {
	// Both required and neither defaulted: a request naming no item or no action is malformed, and
	// `param` reports an omitted one as `None` for exactly this (§111). The trailing `Pv` values a
	// SET would carry are not read, for the reason on `Graphics`.
	let item = csi.param(0)?;
	let action = csi.param(1)?;
	Some(Graphics { item, action })
}

/// Which query a finished CSI is, or `None` when it is not one cmote answers.
///
/// The three private forms, told apart by their marker and final byte together — which is the
/// near-miss rule §56 wrote down, and it has to be all three parts here because each of these final
/// bytes carries other sequences under a different marker. `CSI c` is DA1 and `CSI > c` is DA2, both
/// the engine's; `CSI ? 1049 h` is a private mode; `CSI S` is scroll-up.
///
/// No intermediate belongs to any of them, and a sub-parameter is a spelling none of them defines —
/// the same policy `sgrstack` and `modkeys` take, and for the same reason: the engine has no arm
/// behind any of these three, so cmote is the only actor and refusing an undefined spelling costs
/// nothing (`Csi::sub_parameters`).
fn asked(csi: &super::csi::Csi<'_>) -> Option<Query> {
	if !csi.intermediates().is_empty() || csi.sub_parameters() {
		return None;
	}
	match (csi.marker(), csi.final_byte()) {
		// XTVERSION. Only the empty or zero form is the request; a non-zero parameter on this final
		// byte marks some other private query.
		(Some(b'>'), b'q') => default_params(csi).then_some(Query::Version),
		// DA3, the tertiary device attributes (§36). The engine's `identify_terminal` covers the
		// no-marker (DA1) and `>` (DA2) forms and drops this one, so it falls to cmote — and as with
		// those two, only the empty or zero form is the request.
		(Some(b'='), b'c') => default_params(csi).then_some(Query::UnitId),
		// XTSMGRAPHICS (§41). The engine has no arm for the `?` form of `CSI S` — its only `S` is SU,
		// scroll-up, with no marker at all — so the whole request falls to cmote.
		(Some(b'?'), b'S') => graphics_request(csi).map(Query::Graphics),
		_ => None,
	}
}

/// Whether the parameter run is the *default* one — absent, or a single zero.
///
/// Both private queries that use it (XTVERSION `CSI > q`, DA3 `CSI = c`) are defined only in that
/// form, so a non-zero parameter on the same final byte is a different private sequence and the
/// scanner stays silent rather than answer a question it was not asked.
///
/// A second parameter disqualifies it even when both are zero, which is what `param_count` is for:
/// `CSI > 0 ; 0 q` names two, and neither query takes two. The hand-rolled test this replaces said the
/// same thing by accident — it required every BYTE of the run to be `0`, and a `;` is not — so the
/// count says it on purpose now (§111).
fn default_params(csi: &super::csi::Csi<'_>) -> bool {
	csi.param_count() == 0 || (csi.param_count() == 1 && csi.param(0) == Some(0))
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

/// The valid DECRQSS reply: `DCS 1 $ r <params> <selector> ST` (§33, §123). The leading `1` marks
/// the request valid, `params` is the setting's current value as a parameter string (`0;1;31` for a
/// bold red pen, `1;24` for a full-page scrolling region), and `selector` echoes the queried
/// sequence's own intermediates and final byte, as DECRQSS requires — so an SGR report ends `m` and
/// a DECSCUSR report ends with a space and a `q`.
///
/// One builder for all of them rather than one per setting: the only thing that differs between the
/// five is those two strings, and a second function would have been the same bytes with a different
/// tail hard-coded (§109).
pub fn decrqss_reply(params: &str, selector: &str) -> Vec<u8> {
	let mut reply = Vec::with_capacity(params.len() + selector.len() + 7);
	reply.extend_from_slice(b"\x1bP1$r");
	reply.extend_from_slice(params.as_bytes());
	reply.extend_from_slice(selector.as_bytes());
	reply.extend_from_slice(b"\x1b\\");
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
/// advertise (§33, §123). Three are stated, because three are facts cmote can give truthfully: the
/// terminal name it requested for the remote pty (`TN`, `xterm-256color`), its colour count
/// (`Co`/`colors`, 256), and its direct-colour depth (`RGB`, 8 bits a channel).
///
/// **`RGB` is the third special name XTGETTCAP defines**, alongside `TN` and `Co`, and its type is
/// the thing §66's "their wire values are ambiguous" had not looked up. ncurses' `user_caps(5)`
/// defines `RGB` as boolean, numeric *or* string, and says what a numeric one means: the bits per
/// channel that `setaf`/`setab` take. So the numeric form is the one that fits this reply grammar —
/// which has a value slot and no way to spell a bare boolean — and `8` is the truth about cmote,
/// whose SGR 38;2 / 48;2 path takes a full byte per channel.
///
/// **`Tc` is deliberately not here, and that is a decision rather than an omission.** It is tmux's
/// own extension: it appears in neither xterm's list of XTGETTCAP special names nor ncurses'
/// recognised user capabilities, and it is a pure boolean — a flag whose presence is the whole
/// message. This grammar answers a recognised name as `<NAME>=<VALUE>`, so answering `Tc` would mean
/// inventing a value the capability does not define, on the authority of nobody. A program that
/// wants to know whether cmote takes 24-bit colour has `RGB` to ask, and it is answered.
fn known_capability(name: &[u8]) -> Option<&'static [u8]> {
	match name {
		b"TN" => Some(b"xterm-256color"),
		b"Co" | b"colors" => Some(b"256"),
		b"RGB" => Some(b"8"),
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
	// `as_chunks::<2>().0` — a `&[[u8; 2]]`, so the two indexes below are bounds-checked once here
	// rather than per digit. The remainder is empty: the guard above refuses an odd length outright,
	// which it must, because dropping a trailing half-byte would decode `54E` as if it were `54`.
	for pair in hex.as_chunks::<2>().0 {
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

	/// The queries come back IN STREAM ORDER across both halves, which is what the offset merge in
	/// `feed` exists for and the one thing two unordered passes would have broken (§111).
	///
	/// It matters here and nowhere else in the directory: these become reply bytes sent to the remote,
	/// and a program that asks two questions matches the answers to them by position. Two passes put
	/// every CSI answer after every DCS one, which would have swapped the first case below.
	#[test]
	fn the_answers_come_back_in_the_order_the_questions_were_asked() {
		assert_eq!(
			scan(b"\x1b[>q\x1bP$qm\x1b\\"),
			vec![Query::Version, Query::Decrqss(Decrqss::Sgr)],
			"a CSI question before a DCS one"
		);
		assert_eq!(
			scan(b"\x1bP$qm\x1b\\\x1b[>q"),
			vec![Query::Decrqss(Decrqss::Sgr), Query::Version],
			"and the other way round"
		);
		// Three, alternating, so the merge is doing more than putting one list before the other.
		assert_eq!(
			scan(b"\x1b[>q\x1bP$qm\x1b\\\x1b[=c"),
			vec![Query::Version, Query::Decrqss(Decrqss::Sgr), Query::UnitId]
		);
	}

	/// A padded parameter run is still the default one. This was the divergence §111 measured: the
	/// module's own `MAX_PARAMS` counted BYTES and abandoned the sequence past sixteen of them, so a
	/// program padding its XTVERSION got no answer from cmote and a dispatched sequence from the
	/// engine. The framer clamps the digits instead and the sequence lives.
	#[test]
	fn a_padded_request_is_still_the_default_one() {
		let padded = |marker: u8, final_byte: u8, zeros: usize| {
			let mut bytes = vec![0x1b, b'[', marker];
			bytes.extend(std::iter::repeat_n(b'0', zeros));
			bytes.push(final_byte);
			bytes
		};
		assert_eq!(scan(&padded(b'>', b'q', 16)), vec![Query::Version]);
		assert_eq!(scan(&padded(b'>', b'q', 17)), vec![Query::Version]);
		assert_eq!(scan(&padded(b'>', b'q', 500)), vec![Query::Version]);
		assert_eq!(scan(&padded(b'=', b'c', 500)), vec![Query::UnitId]);
	}

	/// A SECOND parameter disqualifies the default form even when both are zero — neither query takes
	/// two. The hand-rolled test this replaces said so by accident, requiring every byte of the run to
	/// be `0` when a `;` is not; `param_count` says it on purpose (§111).
	#[test]
	fn a_second_parameter_is_not_the_default_form() {
		assert!(scan(b"\x1b[>0;0q").is_empty());
		assert!(scan(b"\x1b[>;q").is_empty());
		assert!(scan(b"\x1b[=0;0c").is_empty());
	}

	/// A sub-parameter is a spelling none of the three defines, and the engine has no arm behind any of
	/// them — so cmote is the only actor and refusing costs nothing (`Csi::sub_parameters`).
	#[test]
	fn a_sub_parameter_is_none_of_these_queries() {
		assert!(scan(b"\x1b[>0:0q").is_empty());
		assert!(scan(b"\x1b[?1:2S").is_empty());
		// The `;` spelling of the graphics request IS one, so this is about the separator.
		assert_eq!(
			scan(b"\x1b[?1;2S"),
			vec![Query::Graphics(Graphics { item: 1, action: 2 })]
		);
	}

	/// An intermediate byte makes it some other sequence on the same marker and final byte — the
	/// near-miss rule §56 wrote down, which this scanner used to get by accident: its old machine had
	/// no state for an intermediate at all and abandoned the sequence on one.
	#[test]
	fn an_intermediate_byte_rules_all_three_out() {
		assert!(scan(b"\x1b[> q").is_empty());
		assert!(scan(b"\x1b[= c").is_empty());
		assert!(scan(b"\x1b[?1;2 S").is_empty());
		assert!(scan(b"\x1b[?4$p").is_empty(), "DECRQM, the engine's");
	}

	/// A byte the engine reads STRAIGHT THROUGH must not change what this module makes of a sequence —
	/// the §106 rule, which `query` was the last CSI scanner not to obey (§111).
	#[test]
	fn a_byte_the_engine_reads_through_does_not_abandon_the_query() {
		assert_eq!(scan(b"\x1b[>0\nq"), vec![Query::Version]);
		assert_eq!(scan(b"\x1b[=\x7fc"), vec![Query::UnitId]);
		// CAN and SUB are the only two bytes that really cancel a sequence in flight.
		assert!(scan(b"\x1b[>0\x18q").is_empty());
		assert!(scan(b"\x1b[=\x1ac").is_empty());
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

	/// The four selectors §123 added, each recognised by the exact spelling of the sequence it asks
	/// about — the space in DECSCUSR's and the quote in DECSCA's are part of the selector, not
	/// padding to be trimmed.
	#[test]
	fn the_reportable_decrqss_selectors_are_told_apart() {
		assert_eq!(
			scan(b"\x1bP$q q\x1b\\"),
			vec![Query::Decrqss(Decrqss::CursorStyle)],
			"DECSCUSR is `CSI Ps SP q`"
		);
		assert_eq!(
			scan(b"\x1bP$q\"q\x1b\\"),
			vec![Query::Decrqss(Decrqss::Protection)],
			"DECSCA is `CSI Ps \" q`, with no space"
		);
		assert_eq!(
			scan(b"\x1bP$qr\x1b\\"),
			vec![Query::Decrqss(Decrqss::ScrollRegion)]
		);
		assert_eq!(
			scan(b"\x1bP$qs\x1b\\"),
			vec![Query::Decrqss(Decrqss::Margins)]
		);
	}

	#[test]
	fn other_decrqss_requests_are_unsupported() {
		// Conformance level (`"p`, DECSCL): cmote holds no state for it, so it is honestly reported
		// unsupported rather than guessed at. Same for a selector that is not a setting at all.
		assert_eq!(
			scan(b"\x1bP$q\"p\x1b\\"),
			vec![Query::Decrqss(Decrqss::Unsupported)]
		);
		assert_eq!(
			scan(b"\x1bP$qZ\x1b\\"),
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
		assert_eq!(decrqss_reply("0", "m"), b"\x1bP1$r0m\x1b\\".to_vec());
		assert_eq!(
			decrqss_reply("0;1;31", "m"),
			b"\x1bP1$r0;1;31m\x1b\\".to_vec()
		);
	}

	/// The selector is echoed verbatim, intermediates included — which is the part a per-setting
	/// builder would have hard-coded five times (§123). DECSCUSR's space and DECSCA's quote are the
	/// two that would be easy to lose.
	#[test]
	fn a_reply_echoes_the_selector_it_was_asked_about() {
		assert_eq!(decrqss_reply("2", " q"), b"\x1bP1$r2 q\x1b\\".to_vec());
		assert_eq!(decrqss_reply("1", "\"q"), b"\x1bP1$r1\"q\x1b\\".to_vec());
		assert_eq!(decrqss_reply("1;24", "r"), b"\x1bP1$r1;24r\x1b\\".to_vec());
		assert_eq!(decrqss_reply("1;80", "s"), b"\x1bP1$r1;80s\x1b\\".to_vec());
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

	/// The third special name XTGETTCAP defines, and the one §66 left unanswered on a claim about
	/// ambiguity that turned out to be checkable (§123). ncurses' `user_caps(5)` says a NUMERIC `RGB`
	/// is the bits per channel, and this reply grammar has a value slot, so the numeric form is the
	/// one that fits: 8, which is what cmote's SGR 38;2 actually takes.
	#[test]
	fn the_direct_colour_depth_is_answered() {
		// `RGB` (524742) -> `8` (38).
		assert_eq!(
			gettcap_reply(&[b"524742".to_vec()]),
			b"\x1bP1+r524742=38\x1b\\".to_vec()
		);
	}

	/// `Tc` is refused, and this test is the refusal rather than a gap being recorded (§123). It is
	/// tmux's own extension — in neither xterm's special-name list nor ncurses' recognised user
	/// capabilities — and it is a pure boolean, which this grammar has no way to spell. Answering it
	/// would mean inventing a value on nobody's authority; `RGB` is the question that has an answer.
	#[test]
	fn the_tmux_truecolor_flag_is_not_answered() {
		// `Tc` (5463) -> `DCS 0 + r 5463 ST`, the same honest "unknown" any other name gets.
		assert_eq!(
			gettcap_reply(&[b"5463".to_vec()]),
			b"\x1bP0+r5463\x1b\\".to_vec()
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

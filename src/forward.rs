// forward.rs — the PURE description of a port forward (PLAN §27).
//
// A "forward" (a tunnel) carries TCP traffic through the live SSH connection. There are
// three kinds, exactly as OpenSSH's `-L` / `-R` / `-D`:
//
//   Local (-L)   — cmote listens on a local port; each connection is carried to a target
//                  reached FROM the server. `localhost:5432 → db.internal:5432`.
//   Remote (-R)  — the SERVER listens on one of its ports; each connection there is carried
//                  back and dialed FROM this machine. `server:9090 → localhost:3000`.
//   Dynamic (-D) — cmote runs a local SOCKS5 proxy; every connection names its OWN target,
//                  each carried from the server. One tunnel, any destination.
//
// This module is the *data*: the kind, the bind/target addresses, how a user's two text
// fields parse into a spec, and how a spec reads back as a label. It is pure and has no
// async, no sockets, no russh — so it is easy to test and can be shared by everything that
// touches a forward: the bridge vocabulary (§4), the saved target (persisted with the
// target, §14/§22), the app's UI state, and the network layer that actually runs it
// (`ssh::forward`). Splitting the pure part out mirrors how `files` (the model) is kept
// apart from `ssh::browse` (the network).

use serde::{Deserialize, Serialize};

/// The default interface a listener binds. Loopback, deliberately: a forward opened without
/// thinking should never expose the tunnel to the whole network. A user who wants that types
/// `0.0.0.0` (local/dynamic) or a bind address (remote) explicitly.
pub const DEFAULT_BIND: &str = "127.0.0.1";

/// Which of the three tunnel shapes a forward is (§27). `Copy` because it is one tag; the
/// serde names are the lowercase words so a hand-edited `targets.json` reads naturally.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
	/// `-L`: listen locally, carry each connection to a target reached from the server. The
	/// default the add form opens on — the commonest tunnel.
	#[default]
	Local,
	/// `-R`: the server listens, each connection is carried back and dialed from here.
	Remote,
	/// `-D`: a local SOCKS5 proxy; every connection names its own target.
	Dynamic,
}

impl ForwardKind {
	/// The word shown in the UI and used in a spec's label.
	pub fn word(self) -> &'static str {
		match self {
			ForwardKind::Local => "Local",
			ForwardKind::Remote => "Remote",
			ForwardKind::Dynamic => "Dynamic",
		}
	}

	/// The single letter that prefixes a spec's one-line summary (`L`/`R`/`D`), echoing the
	/// OpenSSH flag so the label reads familiarly.
	pub fn letter(self) -> char {
		match self {
			ForwardKind::Local => 'L',
			ForwardKind::Remote => 'R',
			ForwardKind::Dynamic => 'D',
		}
	}

	/// Whether this kind carries a fixed target (Local/Remote) or lets each connection choose
	/// its own (Dynamic). The add form hides the target field for a kind with no fixed target,
	/// and parsing skips it.
	pub fn has_target(self) -> bool {
		!matches!(self, ForwardKind::Dynamic)
	}
}

/// One port forward, fully resolved — the shape that is persisted, moved across the bridge to
/// the SSH task, and run by `ssh::forward`. `target_host`/`target_port` are empty/zero for a
/// Dynamic forward, which has no single target. Every field is plain and owned so the spec
/// moves across a channel without borrowing GUI state, and `Eq` lets the app dedupe identical
/// forwards (see `Self::same_endpoint`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardSpec {
	pub kind: ForwardKind,
	/// The interface the listener binds. Local/Dynamic: a local address (defaults to loopback).
	/// Remote: the address the SERVER binds (loopback unless its GatewayPorts allows otherwise).
	pub listen_host: String,
	/// The port the listener binds. A Remote (`-R`) forward may be 0 — "let the server choose the
	/// port" — and the port it assigns is reported back and shown on the row (`ForwardEntry::
	/// label`); the spec keeps 0 so a reconnect asks for a fresh port rather than pinning an
	/// ephemeral one. Local/Dynamic require a real port (there is nothing to assign one).
	pub listen_port: u16,
	/// The target's host, reached from whichever end dials it. Empty for Dynamic.
	#[serde(default, skip_serializing_if = "String::is_empty")]
	pub target_host: String,
	/// The target's port. Zero for Dynamic.
	#[serde(default, skip_serializing_if = "is_zero")]
	pub target_port: u16,
}

/// Serde skip predicate for a `u16` that defaults to zero (a Dynamic forward's target port):
/// keeps it out of the written JSON so the file stays tidy.
fn is_zero(port: &u16) -> bool {
	*port == 0
}

impl ForwardSpec {
	/// Build a spec from the add form's two text fields (§27): `listen` is the bind side
	/// (`port` or `host:port`, host optional — defaults to loopback), and `to` the target
	/// (`host:port`, required for Local/Remote, ignored for Dynamic). Returns a human message
	/// on any parse error, shown inline in the dialog. Split out and pure so the whole parse
	/// surface is unit-tested without a running session.
	pub fn parse(kind: ForwardKind, listen: &str, to: &str) -> Result<Self, String> {
		// The bind side allows a bare port, filling in the loopback default — the common case
		// (`8080`) is the shortest to type, and an explicit interface (`0.0.0.0:8080`) still works.
		// A Remote forward additionally allows port 0: `-R 0` asks the server to choose the port.
		let (listen_host, listen_port) = parse_endpoint(
			listen,
			Some(DEFAULT_BIND),
			matches!(kind, ForwardKind::Remote),
		)
		.map_err(|reason| format!("Listen address: {reason}"))?;

		if kind.has_target() {
			// The target needs a host — there is no sensible default for "where to" — so a bare
			// port is rejected here (`None` default host), and a target port of 0 is never valid.
			let (target_host, target_port) = parse_endpoint(to, None, false)
				.map_err(|reason| format!("Target address: {reason}"))?;
			Ok(Self {
				kind,
				listen_host,
				listen_port,
				target_host,
				target_port,
			})
		} else {
			// Dynamic carries no fixed target; the SOCKS handshake names one per connection.
			Ok(Self {
				kind,
				listen_host,
				listen_port,
				target_host: String::new(),
				target_port: 0,
			})
		}
	}

	/// The `host:port` the listener binds, ready for `TcpListener::bind` / `tcpip_forward`, with
	/// an IPv6 host bracketed so the string parses back (a bare `::1:8080` does not).
	pub fn listen_addr(&self) -> String {
		join_host_port(&self.listen_host, self.listen_port)
	}

	/// Two forwards clash if they bind the same interface and port for the same kind's side —
	/// the OS (local/dynamic) or the server (remote) will refuse the second bind, so the app
	/// rejects a duplicate before it is even sent. Kind matters because a local and a remote
	/// forward on the same number are two different listeners on two different machines.
	pub fn same_endpoint(&self, other: &Self) -> bool {
		// A `-R 0` names no concrete port (the server assigns one), so two of them can never be
		// proven to clash — excluding port 0 here keeps the second from being refused as a
		// duplicate of the first.
		self.kind == other.kind
			&& self.listen_port != 0
			&& self.listen_port == other.listen_port
			&& self.listen_host == other.listen_host
	}

	/// The one-line summary shown on a forward's row (§27): the bind side, an arrow, and the
	/// target — or `(SOCKS)` for a Dynamic forward, which has no single target to name.
	pub fn summary(&self) -> String {
		self.summary_on(self.listen_port)
	}

	/// `summary`, but with an explicit listen port — used to show the port the SERVER assigned a
	/// `-R 0` forward, where the spec's own `listen_port` is still the authored 0 (§27).
	fn summary_on(&self, listen_port: u16) -> String {
		let listen = join_host_port(&self.listen_host, listen_port);
		match self.kind {
			ForwardKind::Dynamic => format!("{listen} (SOCKS)"),
			_ => format!(
				"{listen} → {}",
				join_host_port(&self.target_host, self.target_port)
			),
		}
	}

	/// The full label with the kind letter, e.g. `L 127.0.0.1:8080 → db:5432`.
	pub fn label(&self) -> String {
		self.label_on(self.listen_port)
	}

	/// `label`, but with an explicit listen port (see `summary_on`).
	fn label_on(&self, listen_port: u16) -> String {
		format!("{}  {}", self.kind.letter(), self.summary_on(listen_port))
	}
}

/// Where a forward is in its short life (§27), tracked per entry so the tunnels dialog can show
/// each row's state. Still pure data — no iced, no sockets — so it lives beside the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardStatus {
	/// Requested; waiting for the listener to bind (or the server to accept the remote listen).
	Starting,
	/// Up and carrying connections.
	Active,
	/// Could not start, with the reason for the row (a taken port, a refused remote listen).
	Failed(String),
}

/// One forward as the app tracks it (§27): the runtime id it was given, the spec it runs, and
/// its current status. A plain owned struct the tunnels dialog renders and the app mutates.
#[derive(Debug, Clone)]
pub struct ForwardEntry {
	pub id: u64,
	pub spec: ForwardSpec,
	pub status: ForwardStatus,
	/// The port the SERVER assigned a `-R 0` forward, learned from its `ForwardReady` (§27).
	/// `None` for every forward that named its own port; `Some` only after a 0-request came up,
	/// so the row can show where the server is actually listening while the spec keeps its 0.
	pub bound_port: Option<u16>,
	/// The connections flowing through this forward right now (§27) — raised by a
	/// `ForwardConnectionOpened` event and lowered by a `ForwardConnectionClosed`, so the tunnels
	/// dialog can show a live gauge of what the tunnel is actually carrying.
	pub open_count: u32,
	/// Every connection this forward has EVER carried (§27): a running tally that only grows, so
	/// the gauge tells an idle-but-used tunnel (`0 open · 5 total`) from a never-used one.
	pub total_count: u32,
}

impl ForwardEntry {
	/// The label for this forward's row (§27). A `-R 0` forward the server has assigned a port
	/// shows that port in place of the authored 0; every other forward reads its spec's own label.
	pub fn label(&self) -> String {
		match self.bound_port {
			Some(port) => self.spec.label_on(port),
			None => self.spec.label(),
		}
	}

	/// A connection started flowing through this forward (§27): one more open, and one more ever.
	pub fn connection_opened(&mut self) {
		self.open_count += 1;
		self.total_count += 1;
	}

	/// A connection through this forward ended (§27): one fewer open. Saturating, so a close whose
	/// open was never seen (a stale event after the row was reset) can never underflow the count.
	/// The total is untouched — it counts connections carried, not connections still live.
	pub fn connection_closed(&mut self) {
		self.open_count = self.open_count.saturating_sub(1);
	}

	/// The live activity gauge for the row (§27): the connections open now and the total ever, as
	/// `N open · M total`. Shown only for an Active forward (the caller checks the status), where it
	/// turns a static row into a monitor of the traffic actually crossing the tunnel.
	pub fn activity_gauge(&self) -> String {
		format!("{} open · {} total", self.open_count, self.total_count)
	}
}

/// Join a host and a port back into the `host:port` string a listener binds or a row shows.
/// An IPv6 literal (the host still carries a colon) is bracketed — `::1` → `[::1]:22` — so the
/// result is unambiguous and parses back; `TcpListener::bind` rejects a bare `::1:22` outright.
/// A hostname or IPv4 has no colon and is joined plainly.
fn join_host_port(host: &str, port: u16) -> String {
	if host.contains(':') {
		format!("[{host}]:{port}")
	} else {
		format!("{host}:{port}")
	}
}

/// Parse a `host:port` (or, when `default_host` is `Some`, a bare `port`) into its parts.
/// `allow_zero` permits port 0 — a Remote forward's `-R 0`, "let the server choose the port";
/// every other caller passes `false`, so 0 stays the invalid port it is for a listener that
/// binds a real port here (a local bind) or a target that must be dialed (any target).
///
/// An IPv6 literal is written bracketed, exactly as a URL or OpenSSH does it: `[::1]:22`. The
/// brackets are what make the split unambiguous — a bare `::1:22` cannot say which colon divides
/// the address from the port — so an unbracketed address that still carries a colon is refused
/// with a message pointing at the bracket form. The host is stored WITHOUT its brackets (`::1`);
/// `join_host_port` (above) puts them back whenever the pair is joined for a bind string or label.
fn parse_endpoint(
	input: &str,
	default_host: Option<&str>,
	allow_zero: bool,
) -> Result<(String, u16), String> {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		return Err("cannot be empty".to_owned());
	}

	// An IPv6 literal is bracketed (`[::1]:22`): the address sits inside the brackets and the
	// port follows the `]`. This is the ONE form where the address itself holds colons, so it is
	// parsed first, on its own terms, before the ordinary last-colon split.
	let (host, port_text) = if let Some(rest) = trimmed.strip_prefix('[') {
		let (host, after) = rest
			.split_once(']')
			.ok_or_else(|| "an IPv6 address opened with '[' has no closing ']'".to_owned())?;
		if host.is_empty() {
			return Err("the brackets hold no address ([::1]:port)".to_owned());
		}
		// After the `]` must come `:port` — not a bare `]`, nor any other trailing text.
		let port = after
			.strip_prefix(':')
			.ok_or_else(|| "a port must follow the ']' ([::1]:port)".to_owned())?;
		(host, port)
	} else {
		match trimmed.rsplit_once(':') {
			// A colon splits host from port; an empty host before it means "use the default".
			Some((host, port)) => {
				// A colon left in the HOST is an unbracketed IPv6 literal — ambiguous, since the
				// split took the LAST colon. Point at the bracket form rather than guess.
				if host.contains(':') {
					return Err("an IPv6 address must be bracketed ([::1]:port)".to_owned());
				}
				let host = if host.is_empty() {
					match default_host {
						Some(default) => default,
						None => return Err("a host is required (host:port)".to_owned()),
					}
				} else {
					host
				};
				(host, port)
			}
			// No colon: only allowed when a default host lets a bare port stand alone.
			None => match default_host {
				Some(default) => (default, trimmed),
				None => return Err("expected host:port".to_owned()),
			},
		}
	};

	let port: u16 = port_text
		.parse()
		.map_err(|_| format!("'{port_text}' is not a valid port"))?;
	if port == 0 && !allow_zero {
		return Err("port must be between 1 and 65535".to_owned());
	}
	Ok((host.to_owned(), port))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn local_forward_parses_port_and_target() {
		// Arrange / Act
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "db.internal:5432").unwrap();

		// Assert: the bare listen port took the loopback default; the target kept its host.
		assert_eq!(spec.listen_host, "127.0.0.1");
		assert_eq!(spec.listen_port, 8080);
		assert_eq!(spec.target_host, "db.internal");
		assert_eq!(spec.target_port, 5432);
	}

	#[test]
	fn an_explicit_bind_interface_is_kept() {
		let spec = ForwardSpec::parse(ForwardKind::Local, "0.0.0.0:8080", "h:1").unwrap();
		assert_eq!(spec.listen_host, "0.0.0.0");
		assert_eq!(spec.listen_port, 8080);
	}

	#[test]
	fn dynamic_forward_ignores_the_target_field() {
		// A Dynamic forward has no single target, so whatever is in the "to" box is not read.
		let spec = ForwardSpec::parse(ForwardKind::Dynamic, "1080", "ignored:99").unwrap();
		assert_eq!(spec.listen_port, 1080);
		assert!(spec.target_host.is_empty());
		assert_eq!(spec.target_port, 0);
		assert_eq!(spec.summary(), "127.0.0.1:1080 (SOCKS)");
	}

	#[test]
	fn a_target_needs_a_host() {
		// Local/Remote must name where the traffic goes: a bare port is not enough.
		let error = ForwardSpec::parse(ForwardKind::Remote, "9090", "5432").unwrap_err();
		assert!(error.contains("Target address"));
	}

	#[test]
	fn a_bad_port_is_rejected_with_a_message() {
		assert!(ForwardSpec::parse(ForwardKind::Local, "notaport", "h:1").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "0", "h:1").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "70000", "h:1").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "8080", "h:0").is_err());
	}

	#[test]
	fn an_empty_field_is_rejected() {
		assert!(ForwardSpec::parse(ForwardKind::Local, "", "h:1").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "8080", "  ").is_err());
	}

	#[test]
	fn same_endpoint_ignores_the_target_but_not_the_kind() {
		// Two locals on the same bind clash even if their targets differ (the OS refuses the
		// second bind); a local and a remote on the same port do not (two different machines).
		let a = ForwardSpec::parse(ForwardKind::Local, "8080", "a:1").unwrap();
		let b = ForwardSpec::parse(ForwardKind::Local, "8080", "b:2").unwrap();
		let r = ForwardSpec::parse(ForwardKind::Remote, "8080", "c:3").unwrap();
		assert!(a.same_endpoint(&b));
		assert!(!a.same_endpoint(&r));
	}

	#[test]
	fn a_spec_round_trips_through_json() {
		// The persisted form must survive a save/load (§22): a Dynamic forward drops its empty
		// target from the JSON and reads back the same.
		let specs = vec![
			ForwardSpec::parse(ForwardKind::Local, "127.0.0.1:8080", "db:5432").unwrap(),
			ForwardSpec::parse(ForwardKind::Dynamic, "1080", "").unwrap(),
		];
		let json = serde_json::to_string(&specs).unwrap();
		// The whole array still mentions the local forward's target; the dynamic one on its own
		// drops its empty target from the JSON entirely.
		let dynamic_json = serde_json::to_string(&specs[1]).unwrap();
		assert!(!dynamic_json.contains("target_host"));
		assert!(!dynamic_json.contains("target_port"));
		let back: Vec<ForwardSpec> = serde_json::from_str(&json).unwrap();
		assert_eq!(back, specs);
	}

	#[test]
	fn the_label_reads_with_its_kind_letter() {
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "db:5432").unwrap();
		assert_eq!(spec.label(), "L  127.0.0.1:8080 → db:5432");
	}

	#[test]
	fn a_remote_forward_may_ask_the_server_to_choose_its_port() {
		// `-R 0`: the bind port is 0, meaning "let the server assign one"; the loopback default
		// still fills in the bind host and the target is kept as usual.
		let spec = ForwardSpec::parse(ForwardKind::Remote, "0", "localhost:3000").unwrap();
		assert_eq!(spec.listen_port, 0);
		assert_eq!(spec.listen_host, "127.0.0.1");
		assert_eq!(spec.target_host, "localhost");
		assert_eq!(spec.target_port, 3000);
	}

	#[test]
	fn only_a_remote_forward_may_bind_port_zero() {
		// A local or dynamic listener needs a real port; 0 there is still the error it was.
		assert!(ForwardSpec::parse(ForwardKind::Local, "0", "h:1").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Dynamic, "0", "").is_err());
		// Only the server-chosen BIND port may be 0 — a remote target port still may not.
		assert!(ForwardSpec::parse(ForwardKind::Remote, "0", "h:0").is_err());
	}

	#[test]
	fn two_server_assigned_remote_forwards_do_not_clash() {
		// Two `-R 0` both read as port 0, but the server assigns each a distinct port, so the app
		// must not treat them as the same bind and refuse the second.
		let a = ForwardSpec::parse(ForwardKind::Remote, "0", "a:1").unwrap();
		let b = ForwardSpec::parse(ForwardKind::Remote, "0", "b:2").unwrap();
		assert!(!a.same_endpoint(&b));
		// A concrete remote port still dedupes exactly as before.
		let c = ForwardSpec::parse(ForwardKind::Remote, "9090", "a:1").unwrap();
		let d = ForwardSpec::parse(ForwardKind::Remote, "9090", "b:2").unwrap();
		assert!(c.same_endpoint(&d));
	}

	#[test]
	fn a_server_assigned_remote_forward_shows_its_real_port_on_the_row() {
		// A `-R 0` (authored port 0); once the server picks 38217 the row shows THAT, not the 0.
		let spec = ForwardSpec::parse(ForwardKind::Remote, "0", "localhost:3000").unwrap();
		let entry = ForwardEntry {
			id: 1,
			spec,
			status: ForwardStatus::Active,
			bound_port: Some(38217),
			open_count: 0,
			total_count: 0,
		};
		assert_eq!(entry.label(), "R  127.0.0.1:38217 → localhost:3000");
		// The spec itself is untouched, so a reconnect still asks for a fresh server port.
		assert_eq!(entry.spec.listen_port, 0);
	}

	#[test]
	fn a_forward_with_no_assigned_port_reads_its_specs_label() {
		// The ordinary case: no server assignment, so the entry label is just the spec's label.
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "db:5432").unwrap();
		let entry = ForwardEntry {
			id: 1,
			spec,
			status: ForwardStatus::Active,
			bound_port: None,
			open_count: 0,
			total_count: 0,
		};
		assert_eq!(entry.label(), "L  127.0.0.1:8080 → db:5432");
	}

	#[test]
	fn a_forward_counts_its_open_and_total_connections() {
		// The gauge rises with each connection and, on close, the live count falls while the total
		// stands — so an idle-but-used tunnel still shows what it has carried.
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "db:5432").unwrap();
		let mut entry = ForwardEntry {
			id: 1,
			spec,
			status: ForwardStatus::Active,
			bound_port: None,
			open_count: 0,
			total_count: 0,
		};
		assert_eq!(entry.activity_gauge(), "0 open · 0 total");
		entry.connection_opened();
		entry.connection_opened();
		assert_eq!(entry.activity_gauge(), "2 open · 2 total");
		entry.connection_closed();
		assert_eq!(entry.activity_gauge(), "1 open · 2 total");
	}

	#[test]
	fn a_forwards_open_count_never_underflows() {
		// A close with no matching open (a stale event) must not wrap the unsigned count around.
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "db:5432").unwrap();
		let mut entry = ForwardEntry {
			id: 1,
			spec,
			status: ForwardStatus::Active,
			bound_port: None,
			open_count: 0,
			total_count: 0,
		};
		entry.connection_closed();
		assert_eq!(entry.activity_gauge(), "0 open · 0 total");
	}

	#[test]
	fn a_bracketed_ipv6_bind_parses_and_binds_back_bracketed() {
		// `[::1]:8080` stores the address WITHOUT its brackets, but `listen_addr` puts them back so
		// the string a listener binds is unambiguous (a bare `::1:8080` would not parse).
		let spec = ForwardSpec::parse(ForwardKind::Local, "[::1]:8080", "db:5432").unwrap();
		assert_eq!(spec.listen_host, "::1");
		assert_eq!(spec.listen_port, 8080);
		assert_eq!(spec.listen_addr(), "[::1]:8080");
	}

	#[test]
	fn a_bracketed_ipv6_target_is_kept_and_labelled_bracketed() {
		// A full IPv6 target survives too, and the row re-brackets it so the label reads back.
		let spec = ForwardSpec::parse(ForwardKind::Local, "8080", "[2001:db8::1]:5432").unwrap();
		assert_eq!(spec.target_host, "2001:db8::1");
		assert_eq!(spec.target_port, 5432);
		assert_eq!(spec.label(), "L  127.0.0.1:8080 → [2001:db8::1]:5432");
	}

	#[test]
	fn an_unbracketed_ipv6_points_at_the_bracket_form() {
		// The last-colon split cannot say where the address ends, so it is refused with guidance
		// rather than silently mis-parsed.
		let error = ForwardSpec::parse(ForwardKind::Local, "::1:8080", "db:5432").unwrap_err();
		assert!(error.contains("bracketed"));
	}

	#[test]
	fn a_bracketed_ipv6_still_needs_a_well_formed_port() {
		// A bare `[::1]` names no port; an empty one and an unclosed bracket are rejected too.
		assert!(ForwardSpec::parse(ForwardKind::Local, "[::1]", "db:5432").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "[::1]:", "db:5432").is_err());
		assert!(ForwardSpec::parse(ForwardKind::Local, "[::1", "db:5432").is_err());
	}

	#[test]
	fn a_remote_ipv6_bind_may_ask_the_server_to_choose_its_port() {
		// `-R [::]:0`: a bracketed all-interfaces IPv6 bind still allows the server-chosen port.
		let spec = ForwardSpec::parse(ForwardKind::Remote, "[::]:0", "localhost:3000").unwrap();
		assert_eq!(spec.listen_host, "::");
		assert_eq!(spec.listen_port, 0);
		assert_eq!(spec.listen_addr(), "[::]:0");
	}
}

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
// profile, §14/§22), the app's UI state, and the network layer that actually runs it
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
	/// The port the listener binds. Non-zero (a server-assigned `-R 0` is a deferred nicety —
	/// see `parse`).
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
		let (listen_host, listen_port) = parse_endpoint(listen, Some(DEFAULT_BIND))
			.map_err(|reason| format!("Listen address: {reason}"))?;

		if kind.has_target() {
			// The target needs a host — there is no sensible default for "where to" — so a bare
			// port is rejected here (`None` default host).
			let (target_host, target_port) =
				parse_endpoint(to, None).map_err(|reason| format!("Target address: {reason}"))?;
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

	/// The `host:port` the listener binds, ready for `TcpListener::bind` / `tcpip_forward`.
	pub fn listen_addr(&self) -> String {
		format!("{}:{}", self.listen_host, self.listen_port)
	}

	/// Two forwards clash if they bind the same interface and port for the same kind's side —
	/// the OS (local/dynamic) or the server (remote) will refuse the second bind, so the app
	/// rejects a duplicate before it is even sent. Kind matters because a local and a remote
	/// forward on the same number are two different listeners on two different machines.
	pub fn same_endpoint(&self, other: &Self) -> bool {
		self.kind == other.kind
			&& self.listen_port == other.listen_port
			&& self.listen_host == other.listen_host
	}

	/// The one-line summary shown on a forward's row (§27): the bind side, an arrow, and the
	/// target — or `(SOCKS)` for a Dynamic forward, which has no single target to name.
	pub fn summary(&self) -> String {
		match self.kind {
			ForwardKind::Dynamic => format!("{} (SOCKS)", self.listen_addr()),
			_ => format!(
				"{} → {}:{}",
				self.listen_addr(),
				self.target_host,
				self.target_port
			),
		}
	}

	/// The full label with the kind letter, e.g. `L 127.0.0.1:8080 → db:5432`.
	pub fn label(&self) -> String {
		format!("{}  {}", self.kind.letter(), self.summary())
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
}

/// Parse a `host:port` (or, when `default_host` is `Some`, a bare `port`) into its parts.
///
/// The host/port split is on the LAST colon, so an unbracketed IPv6 literal is NOT handled —
/// `::1` would be read as host `:` port `1`. `ponytail:` bracketed IPv6 (`[::1]:22`) is a
/// later nicety; the common hostname / IPv4 case is covered and the error is clear otherwise.
fn parse_endpoint(input: &str, default_host: Option<&str>) -> Result<(String, u16), String> {
	let trimmed = input.trim();
	if trimmed.is_empty() {
		return Err("cannot be empty".to_owned());
	}

	let (host, port_text) = match trimmed.rsplit_once(':') {
		// A colon splits host from port; an empty host before it means "use the default".
		Some((host, port)) => {
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
	};

	let port: u16 = port_text
		.parse()
		.map_err(|_| format!("'{port_text}' is not a valid port"))?;
	if port == 0 {
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
}

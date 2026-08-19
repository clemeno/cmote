// ssh/forward.rs — running port forwards over the live SSH connection (PLAN §27).
//
// `forward.rs` (top level) is the pure *description* of a tunnel; THIS module is the moving
// parts: the local TCP listeners, the SOCKS proxy, the per-connection byte pumps, and the
// bridge to russh's remote-forward callback. It runs entirely on the tokio thread (§4).
//
// One hard constraint shapes everything here. russh's `Handle` (the session) is NOT `Clone`
// and NOT `Sync` — it owns a reply receiver — so ONLY the one task that owns it (`client::
// stream`) may open channels on it. A listener therefore cannot open its own SSH channel:
// instead it accepts a TCP connection, does any SOCKS negotiation itself, and hands the raw
// socket back to `stream` over a channel (`Accepted`). `stream` opens the `direct-tcpip`
// channel and spawns a detached pump — which, once the channel is open, needs neither the
// session nor any shared state and so lives happily on its own.
//
// The three kinds map to two code paths:
//   * Local / Dynamic — OUTBOUND: a local listener here, `direct-tcpip` opened by `stream`.
//   * Remote          — INBOUND: the SERVER listens (`tcpip_forward`) and opens `forwarded-
//                       tcpip` channels back; russh delivers them to our `Handler`, which
//                       looks up the local target in a table shared with this module and
//                       dials it. Removal cancels the server listen and prunes the table.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use russh::Channel;
use russh::ChannelOpenFailure;
use russh::client::Handle;
use russh::client::Handler as RusshHandler;
use russh::client::{ChannelOpenHandle, Msg};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::bridge::SshEvent;
use crate::forward::{ForwardKind, ForwardSpec};

/// The table shared between the Handler and the `Forwards` manager for REMOTE forwards
/// (§27): the server's bound port → the local `(host, port, forward id)` to dial when a
/// connection arrives on it. The `(host, port)` is where to dial; the id lets the Handler's
/// pump attribute the connection to the right row for the live gauge. The Handler reads it in
/// `server_channel_open_forwarded_tcpip`; `Forwards` writes it as remote forwards are added and
/// removed. A `std::sync::Mutex` is right here — every critical section is a single map op with
/// no `.await` held across the lock.
///
/// `ponytail:` keyed by port only, not `(address, port)`. Two remote forwards that bind the
/// same port on different server interfaces would collide; binding distinct ports (the usual
/// case) is unaffected.
pub type RemoteTable = Arc<Mutex<HashMap<u16, (String, u16, u64)>>>;

/// A fresh, empty remote-forward table. Made once per session in `connect_and_run` and cloned
/// into both the Handler and the `Forwards` manager.
pub fn remote_table() -> RemoteTable {
	Arc::new(Mutex::new(HashMap::new()))
}

/// Lock the table, recovering from a poisoned mutex rather than panicking: a critical section
/// here is a single infallible map operation, so a poisoning holder cannot have left the map
/// half-updated — taking the inner value is safe and keeps a forward glitch from crashing the
/// session.
fn lock(table: &RemoteTable) -> std::sync::MutexGuard<'_, HashMap<u16, (String, u16, u64)>> {
	table
		.lock()
		.unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// A TCP connection a local/dynamic listener accepted, handed back to `stream` so it can open
/// the SSH channel the listener is not allowed to (see the module header). Carries the raw
/// socket plus the target the tunnel goes to — fixed for a Local forward, SOCKS-negotiated for
/// a Dynamic one — and the peer address, reported to the server as the tunnel's originator. It
/// also carries the `id` of the forward that accepted it, so the detached pump can report this
/// connection's start and end and move that row's live gauge (§27).
pub struct Accepted {
	pub id: u64,
	pub target_host: String,
	pub target_port: u16,
	pub tcp: TcpStream,
	pub peer: SocketAddr,
}

/// The live forwards on one session, owned by `client::stream` (§27). Local/dynamic listeners
/// are spawned tasks aborted on removal (or when this is dropped at session end); remote
/// forwards are entries in the shared table plus a server-side listen cancelled on removal.
pub struct Forwards {
	/// Local/dynamic listener tasks by forward id. Aborting the task stops it accepting AND —
	/// because it owns its accept loop — drops it cleanly.
	local: HashMap<u64, JoinHandle<()>>,
	/// Remote forwards by id → the `(bind_host, bind_port)` used to cancel the server listen and
	/// prune the shared table on removal.
	remote: HashMap<u64, (String, u16)>,
	/// The table shared with the Handler (see `RemoteTable`).
	table: RemoteTable,
	/// Cloned into every local/dynamic listener so it can hand accepted sockets back to `stream`.
	accepted_tx: mpsc::Sender<Accepted>,
}

impl Forwards {
	/// Build the manager over the session's shared table and the channel `stream` drains for
	/// accepted connections.
	pub fn new(table: RemoteTable, accepted_tx: mpsc::Sender<Accepted>) -> Self {
		Self {
			local: HashMap::new(),
			remote: HashMap::new(),
			table,
			accepted_tx,
		}
	}

	/// Start the forward `id` described by `spec` (§27). Local/Dynamic spawn a listener that
	/// reports its own readiness/failure from inside the task (binding is async); Remote inserts
	/// the target into the shared table and asks the server to listen, reporting the outcome
	/// here. Generic over the handler type like `ssh::open_sftp`, so it never names cmote's own.
	pub async fn add<H: RusshHandler>(
		&mut self,
		session: &Handle<H>,
		events: &mpsc::Sender<SshEvent>,
		id: u64,
		spec: ForwardSpec,
	) {
		match spec.kind {
			ForwardKind::Local | ForwardKind::Dynamic => {
				let task = tokio::spawn(listen(spec, id, self.accepted_tx.clone(), events.clone()));
				self.local.insert(id, task);
			}
			ForwardKind::Remote => {
				let requested = spec.listen_port;
				// A concrete port is inserted into the table BEFORE the request returns, so a
				// connection the server opens the instant it starts listening already finds its
				// mapping. A `-R 0` asks the server to CHOOSE the port, so the port is not known
				// until the reply — its mapping is inserted then instead. A connection in that
				// sub-millisecond gap is declined and the client retries, the tiny window OpenSSH
				// has too.
				if requested != 0 {
					lock(&self.table)
						.insert(requested, (spec.target_host.clone(), spec.target_port, id));
				}
				if let Ok(assigned) = session
					.tcpip_forward(spec.listen_host.clone(), u32::from(requested))
					.await
				{
					// russh returns the server-chosen port for a 0 request and 0 for a concrete
					// one (RFC 4254), so the port actually bound is the assignment when we asked
					// for 0, else exactly what we asked for.
					let bound = if requested == 0 {
						match u16::try_from(assigned) {
							Ok(port) if port != 0 => port,
							// A server that accepted `-R 0` without naming a real port leaves
							// nothing to map or cancel: treat it as a refusal rather than dangle.
							_ => {
								let _ = events
									.send(SshEvent::ForwardFailed {
										id,
										reason: "the server assigned no port for the remote \
    											         forward"
											.to_owned(),
									})
									.await;
								return;
							}
						}
					} else {
						requested
					};
					// For a `-R 0` the mapping could not be inserted up front — do it now, keyed
					// by the port the server actually bound (what its `forwarded-tcpip` channels
					// will report).
					if requested == 0 {
						lock(&self.table)
							.insert(bound, (spec.target_host.clone(), spec.target_port, id));
					}
					// Remember the BOUND port (not the requested 0) so removal cancels and prunes
					// the right one.
					self.remote.insert(id, (spec.listen_host.clone(), bound));
					// Tell the GUI the assigned port only when the server chose it; a concrete
					// request already knows its own port.
					let assigned_port = (requested == 0).then_some(bound);
					let _ = events
						.send(SshEvent::ForwardReady { id, assigned_port })
						.await;
				} else {
					// The server refused: undo the bookkeeping so nothing dangles. (Nothing was
					// inserted for a `-R 0`, so the remove is a harmless no-op there.)
					if requested != 0 {
						lock(&self.table).remove(&requested);
					}
					let _ = events
						.send(SshEvent::ForwardFailed {
							id,
							reason: "the server refused the remote forward".to_owned(),
						})
						.await;
				}
			}
		}
	}

	/// Tear down the forward `id` (§27). A local/dynamic listener is aborted (its in-flight
	/// tunnels are detached and end when their sockets close); a remote forward is pruned from
	/// the table and its server listen cancelled. An unknown id is a no-op.
	pub async fn remove<H: RusshHandler>(&mut self, session: &Handle<H>, id: u64) {
		if let Some(task) = self.local.remove(&id) {
			task.abort();
		} else if let Some((host, port)) = self.remote.remove(&id) {
			lock(&self.table).remove(&port);
			let _ = session.cancel_tcpip_forward(host, u32::from(port)).await;
		}
	}
}

impl Drop for Forwards {
	/// End of session: stop every local listener at once. Remote forwards need no action — the
	/// server's listeners die when the SSH connection (and its `Handle`) drops.
	fn drop(&mut self) {
		for (_, task) in self.local.drain() {
			task.abort();
		}
	}
}

/// A local (or dynamic) listener task (§27). Binds the port, reports readiness or a clear
/// failure, then accepts forever: each connection resolves its target — fixed for Local, via a
/// SOCKS5 handshake for Dynamic — and is handed to `stream` to open the SSH channel. Returns
/// when aborted (removal / session end) or when `stream` has gone.
async fn listen(
	spec: ForwardSpec,
	id: u64,
	accepted_tx: mpsc::Sender<Accepted>,
	events: mpsc::Sender<SshEvent>,
) {
	let listener = match TcpListener::bind(spec.listen_addr()).await {
		Ok(listener) => listener,
		Err(error) => {
			// The common cause is "address already in use"; surface a short, honest reason.
			let _ = events
				.send(SshEvent::ForwardFailed {
					id,
					reason: format!("could not bind {}: {error}", spec.listen_addr()),
				})
				.await;
			return;
		}
	};
	// A local/dynamic listener binds its own concrete port, so there is no server-assigned port
	// to report.
	let _ = events
		.send(SshEvent::ForwardReady {
			id,
			assigned_port: None,
		})
		.await;

	loop {
		// A transient accept error (a connection reset before we took it) is not fatal — keep
		// listening rather than tearing the whole forward down.
		let Ok((tcp, peer)) = listener.accept().await else {
			continue;
		};

		let resolved = match spec.kind {
			ForwardKind::Local => Some((spec.target_host.clone(), spec.target_port, tcp)),
			// Dynamic learns the target from the client's SOCKS request; a failed handshake
			// just drops this one connection.
			ForwardKind::Dynamic => socks_handshake(tcp).await.ok(),
			// A remote forward never runs a local listener (it is handled server-side).
			ForwardKind::Remote => None,
		};

		if let Some((target_host, target_port, tcp)) = resolved {
			let accepted = Accepted {
				id,
				target_host,
				target_port,
				tcp,
				peer,
			};
			// `stream` gone means the session is winding down: stop accepting.
			if accepted_tx.send(accepted).await.is_err() {
				break;
			}
		}
	}
}

/// Open the SSH `direct-tcpip` channel for one accepted local/dynamic connection and pump it
/// (§27). Called by `stream`, the only task allowed to touch the session. Once the channel is
/// open the pump is fully detached — it owns the socket and the channel stream and needs
/// nothing shared — so a slow or long-lived tunnel never holds up the shell. A refused channel
/// simply drops the socket, closing the client's connection.
pub async fn open_local_tunnel<H: RusshHandler>(
	session: &Handle<H>,
	accepted: Accepted,
	events: mpsc::Sender<SshEvent>,
) {
	let Accepted {
		id,
		target_host,
		target_port,
		tcp,
		peer,
	} = accepted;

	if let Ok(channel) = session
		.channel_open_direct_tcpip(
			target_host,
			u32::from(target_port),
			peer.ip().to_string(),
			u32::from(peer.port()),
		)
		.await
	{
		let mut stream = channel.into_stream();
		tokio::spawn(async move {
			let mut tcp = tcp;
			// The channel is open, so a connection is now flowing: raise the row's gauge, pump
			// until either end closes (errors here are ordinary connection ends), then lower it.
			let _ = events.send(SshEvent::ForwardConnectionOpened { id }).await;
			let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
			let _ = events.send(SshEvent::ForwardConnectionClosed { id }).await;
		});
	} else {
		// The server would not open the channel (target refused, policy): let `tcp` drop,
		// which closes the client's side. Nothing flowed, so the gauge is left untouched.
	}
}

/// The Handler's side of a REMOTE forward (§27): the server opened a `forwarded-tcpip` channel
/// for a connection on `connected_port`. Look the port up in the shared table; if a local
/// target is mapped, accept the channel, dial the target, and pump; otherwise reject it. All
/// the tunnel plumbing lives here (not in `client.rs`), so the Handler callback is one line.
pub async fn accept_remote(
	table: &RemoteTable,
	channel: Channel<Msg>,
	reply: ChannelOpenHandle,
	connected_port: u16,
	events: mpsc::Sender<SshEvent>,
) {
	// Copy the target (and the forward's id, for the gauge) out under a short lock, then release
	// it before any await.
	let target = lock(table).get(&connected_port).cloned();
	let Some((host, port, id)) = target else {
		// Nothing mapped for that port (a stale connection after removal): decline the open.
		reply.reject(ChannelOpenFailure::ConnectFailed).await;
		return;
	};

	// Confirm the channel, then dial the local target and pump detached. A dial that fails just
	// drops the channel, closing the server's side — the visible outcome of an unreachable target.
	reply.accept().await;
	tokio::spawn(async move {
		if let Ok(mut tcp) = TcpStream::connect((host.as_str(), port)).await {
			let mut stream = channel.into_stream();
			// Only a connection that actually reached its target counts on the gauge: raise it here,
			// after the successful dial, and lower it when the pump ends.
			let _ = events.send(SshEvent::ForwardConnectionOpened { id }).await;
			let _ = tokio::io::copy_bidirectional(&mut tcp, &mut stream).await;
			let _ = events.send(SshEvent::ForwardConnectionClosed { id }).await;
		}
	});
}

/// A minimal SOCKS5 server handshake for a Dynamic forward (§27), run on the freshly accepted
/// socket to learn where THIS connection wants to go. Supports the no-auth method and the
/// CONNECT command only — exactly what `ssh -D` offers a browser or `curl --socks5`. Returns
/// the resolved target and the socket, ready to tunnel.
///
/// `ponytail:` the success reply is written OPTIMISTICALLY, before the SSH channel is opened,
/// because the listener cannot open it (see the module header). Real `ssh -D` replies only
/// once the channel succeeds; here a channel that then fails shows the client a closed
/// connection rather than a SOCKS error code — the same visible outcome, one step later.
async fn socks_handshake(mut tcp: TcpStream) -> anyhow::Result<(String, u16, TcpStream)> {
	// Greeting: version, method count, then that many method bytes.
	let mut greeting = [0u8; 2];
	tcp.read_exact(&mut greeting).await?;
	if greeting[0] != 0x05 {
		anyhow::bail!("not a SOCKS5 client");
	}
	let mut methods = vec![0u8; greeting[1] as usize];
	tcp.read_exact(&mut methods).await?;
	// Choose "no authentication required".
	tcp.write_all(&[0x05, 0x00]).await?;

	// Request: version, command, reserved, address type, address, port.
	let mut request = [0u8; 4];
	tcp.read_exact(&mut request).await?;
	if request[0] != 0x05 {
		anyhow::bail!("bad SOCKS5 request");
	}
	// 0x01 is CONNECT; BIND and UDP ASSOCIATE are not supported.
	if request[1] != 0x01 {
		let _ = tcp
			.write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
			.await;
		anyhow::bail!("unsupported SOCKS5 command");
	}

	let host = match request[3] {
		// IPv4: four raw bytes.
		0x01 => {
			let mut addr = [0u8; 4];
			tcp.read_exact(&mut addr).await?;
			std::net::Ipv4Addr::from(addr).to_string()
		}
		// Domain name: a length byte then that many UTF-8 bytes.
		0x03 => {
			let mut len = [0u8; 1];
			tcp.read_exact(&mut len).await?;
			let mut name = vec![0u8; len[0] as usize];
			tcp.read_exact(&mut name).await?;
			String::from_utf8(name)?
		}
		// IPv6: sixteen raw bytes.
		0x04 => {
			let mut addr = [0u8; 16];
			tcp.read_exact(&mut addr).await?;
			std::net::Ipv6Addr::from(addr).to_string()
		}
		_ => {
			let _ = tcp
				.write_all(&[0x05, 0x08, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
				.await;
			anyhow::bail!("unsupported SOCKS5 address type");
		}
	};

	let mut port = [0u8; 2];
	tcp.read_exact(&mut port).await?;
	let port = u16::from_be_bytes(port);

	// Reply "succeeded" with a zero bound address (the client ignores it for a CONNECT).
	tcp.write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
		.await?;
	Ok((host, port, tcp))
}

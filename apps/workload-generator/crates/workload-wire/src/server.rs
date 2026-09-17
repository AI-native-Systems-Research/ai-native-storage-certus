//! The server half: an accept loop, a per-connection frame loop, and nothing else.
//!
//! # Transport only
//!
//! This module knows how to read a frame, dispatch it and write a reply. It knows nothing
//! about mailboxes, keys or caches: the work belongs to [`Service`], which the node agent
//! implements. Keeping them apart is what lets the whole protocol be exercised in-process
//! with no agent, no server and no accelerator.
//!
//! # `&mut self` is how the causal ordering rule is enforced
//!
//! A session's turns are causally dependent — turn *n+1*'s path contains the blocks turn
//! *n* stored — so two turns of one session must never be in flight together. A lane owns
//! a fixed set of sessions on its own connection, so the rule reduces to: **one
//! connection's frames are handled one at a time, in arrival order**.
//!
//! [`Service`] therefore takes `&mut self` and is created per connection by
//! [`ServiceFactory::accept`]. That makes the requirement structural rather than
//! documentary: a handler cannot be shared across a connection's frames without the
//! compiler objecting, so the thread pool that would silently reorder a session's turns
//! cannot be written by accident.
//!
//! Concurrency comes from having several connections, which is what several lanes are.
//! Everything past submission — the mailbox's parallel channels, the server's own threads —
//! is Certus-internal reordering and is deliberately not this module's concern.
//!
//! # Idleness is not death, but a closed socket is
//!
//! An agent must survive arbitrarily long gaps between frames. Under paced mode a session's
//! think time is real waiting, so a node can legitimately receive nothing for minutes; an agent
//! that took silence for failure would exit in the middle of the workload it was serving. So
//! there is **no idle timeout on an established connection** — the read timeout below exists
//! only to poll the stop flag.
//!
//! What *does* mean the run is over is the **socket closing**. TCP tells the agent for free
//! that the control process has gone, whether it exited, panicked or was killed, and that is
//! more reliable than any timeout: a timeout has to guess how long silence is acceptable, while
//! a closed socket is not a guess.
//!
//! So [`Server::serve`] exits when every connection it once had is gone. Without that, a
//! generator killed with `SIGKILL` would leave the agent listening forever, holding mailbox
//! channels and a device allocation — and FR-053's "release resources even when the generator
//! exits abnormally" would be deferred to whenever someone next started a run rather than
//! actually satisfied.
//!
//! Two details keep it from firing wrongly. A [`Server::with_linger`] grace period tolerates
//! the gap while a generator opens its lanes one at a time, or reconnects one; and the exit only
//! applies **after** at least one connection has been served, so a freshly launched agent is not
//! killed by having no clients yet. For that case there is a separate and much longer
//! [`Server::with_first_connect_timeout`]: an agent nobody ever connects to was launched for a
//! run that never came, and sitting there forever is the same leak by a slower route.
//!
//! # An idle connection must not block teardown
//!
//! A connection thread spends most of its life blocked reading the next frame, so an agent
//! that joined its threads on the way out would wait for a peer that has no intention of
//! sending anything. Teardown has to be *verified* rather than hoped for (FR-053) — a
//! teardown bug that left resources held has already invalidated an A/B series in this
//! repository, and it failed nondeterministically rather than visibly.
//!
//! So an accepted socket gets a read timeout, and a timeout **between frames** is not an
//! error: it is the moment to check whether the run has been stopped. A timeout *part-way
//! through* a frame is different — the peer is mid-send — and is retried, because giving up
//! there would turn a slow network into a protocol error.
//!
//! # A protocol violation closes the connection
//!
//! There is no error reply in the frame set, and inventing one would mean a peer could keep
//! a connection alive by sending nonsense. An oversized `len`, an unknown opcode, a body
//! that does not decode, or any frame before `Hello`, all close the connection. The
//! generator sees that as [`crate::client::ClientError::Closed`], names the node and aborts
//! the run (FR-064) — which is the correct outcome, since a peer that cannot speak the
//! protocol cannot be measured.
//!
//! # Examples
//!
//! A whole conversation over loopback. The [`Service`] is all the work; the server is only
//! the accept loop and the frame loop:
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use workload_wire::client::Client;
//! use workload_wire::frame::{Hello, HelloAck, SubmitTurn, TurnOutcome};
//! use workload_wire::handshake;
//! use workload_wire::server::{FnFactory, Server, Service};
//!
//! /// One of these exists per connection, and it sees that connection's frames one at a
//! /// time — which is what keeps a session's causally dependent turns from overlapping.
//! struct EveryKeyResident {
//!     turns: usize,
//! }
//!
//! impl Service for EveryKeyResident {
//!     fn hello(&mut self, hello: &Hello) -> HelloAck {
//!         // The fail-closed comparison is the implementor's: this module cannot know
//!         // what a correct build_id is, so it does not try.
//!         handshake::answer(hello, 8, 32768)
//!     }
//!
//!     fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome {
//!         self.turns += 1;
//!         TurnOutcome {
//!             resident: turn.path.len() as u32,
//!             blocks_read: turn.path.len() as u32,
//!             ..Default::default()
//!         }
//!     }
//! }
//!
//! let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(EveryKeyResident { turns: 0 })))
//!     .unwrap()
//!     // Zero linger is what a test wants and what a run does not: a generator opening
//!     // its lanes one at a time would be cut off by it.
//!     .with_linger(Duration::ZERO);
//! let addr = server.local_addr().unwrap();
//!
//! let serving = std::thread::spawn(move || {
//!     server.serve(Arc::new(AtomicBool::new(false))).unwrap();
//! });
//!
//! let mut client = Client::connect(addr, 4, Some(Duration::from_secs(10))).unwrap();
//! client.handshake("localhost", "/dev/shm/certus-shmq", 4, 32768).unwrap();
//!
//! client.submit(&SubmitTurn { session: 1, flags: 0, path: vec![10, 11, 12] }).unwrap();
//! let outcomes = client.finish().unwrap();
//! assert_eq!(outcomes[0].resident, 3);
//!
//! // `shutdown` stops the accept loop, so `serve` returns and the agent releases its
//! // resources rather than leaving them to whoever next starts a run (FR-053).
//! client.shutdown().unwrap();
//! serving.join().unwrap();
//! ```
//!
//! A frame before `Hello` closes the connection rather than being served, because a peer
//! that has not identified itself must be refused rather than measured:
//!
//! ```
//! use std::io::Write;
//! use std::net::TcpStream;
//!
//! use workload_wire::frame::{opcode, SubmitTurn, Writer};
//! use workload_wire::server::{serve_tcp, ServerError, Service};
//! # use workload_wire::frame::{Hello, HelloAck, TurnOutcome};
//! # struct S;
//! # impl Service for S {
//! #     fn hello(&mut self, h: &Hello) -> HelloAck { workload_wire::handshake::answer(h, 8, 32768) }
//! #     fn submit_turn(&mut self, _t: &SubmitTurn) -> TurnOutcome { TurnOutcome::default() }
//! # }
//!
//! let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
//! let addr = listener.local_addr().unwrap();
//!
//! let sending = std::thread::spawn(move || {
//!     let mut socket = TcpStream::connect(addr).unwrap();
//!     let mut w = Writer::new();
//!     w.bytes(&SubmitTurn { session: 1, flags: 0, path: vec![7] }.encode());
//!     socket.write_all(&w.frame(opcode::SUBMIT_TURN, 0, 1)).unwrap();
//!     // Hold the socket open so the server's error is the protocol one, not a close.
//!     std::thread::sleep(std::time::Duration::from_millis(200));
//! });
//!
//! let (mut accepted, _) = listener.accept().unwrap();
//! let err = serve_tcp(&mut accepted, &mut S, 65536).unwrap_err();
//! assert!(matches!(err, ServerError::HelloNotFirst { opcode: 2 }));
//! sending.join().unwrap();
//! ```

use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::client::DEFAULT_MAX_BODY;
use crate::frame::{
    opcode, DrainAck, Header, Hello, HelloAck, ShutdownAck, Stats, SubmitTurn, TurnOutcome,
    WireError, Writer, HEADER_BYTES,
};

/// How long the accept loop waits between polls when idle.
const ACCEPT_POLL: Duration = Duration::from_millis(2);

/// How long a connection waits for the next frame before checking whether to stop.
///
/// Short enough that teardown is prompt, long enough that an idle connection is not a
/// busy-wait. It bounds only the *gap between* frames, never a frame in transit, and it is
/// **not** an idle timeout: a connection that receives nothing for an hour stays open.
pub const IDLE_POLL: Duration = Duration::from_millis(50);

/// Grace period after the last connection closes before the agent exits.
///
/// A generator opens one connection per lane, and may reconnect one. Exiting the instant the
/// count reached zero would race that, so the agent waits — briefly, because the common reason
/// the count reached zero is that the run is over.
pub const DEFAULT_LINGER: Duration = Duration::from_secs(5);

/// How long a freshly launched agent waits for its first connection.
///
/// Long, because a generator may be starting agents on many nodes before it connects to any of
/// them, and because the cost of being wrong is refusing a run. But finite: an agent nobody ever
/// connects to was launched for a run that never came, and sitting there forever holds mailbox
/// channels and a device allocation just as surely as a leaked one.
pub const DEFAULT_FIRST_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);

/// What one connection does, in arrival order.
///
/// `&mut self` is deliberate; see the module docs. One of these exists per connection and
/// handles that connection's frames one at a time, which is what keeps a session's
/// dependent turns from overlapping.
pub trait Service: Send + 'static {
    /// Answer the handshake.
    ///
    /// The fail-closed comparison is the implementor's: this module cannot know what a
    /// correct `build_id` is, and a transport that guessed would be worse than one that
    /// does not try.
    fn hello(&mut self, hello: &Hello) -> HelloAck;

    /// Run one turn: check the path, load what is resident, store what is absent.
    fn submit_turn(&mut self, turn: &SubmitTurn) -> TurnOutcome;

    /// Report outstanding work.
    fn drain(&mut self) -> DrainAck {
        DrainAck::default()
    }

    /// Report what the mailbox-facing code measured — for the generator's report only.
    fn stats(&mut self) -> Stats {
        Stats::default()
    }

    /// Final tally.
    fn shutdown(&mut self) -> ShutdownAck {
        ShutdownAck::default()
    }
}

/// Makes one [`Service`] per accepted connection.
///
/// Per connection rather than per server because a connection is a lane, and a lane claims
/// its own mailbox channel: the mailbox is depth-1 per channel, so sharing one across lanes
/// would serialise them silently.
pub trait ServiceFactory: Send + Sync + 'static {
    /// The per-connection handler.
    type Service: Service;

    /// Build a handler for a new connection.
    ///
    /// # Errors
    ///
    /// If the resources a connection needs — a mailbox channel, a payload slot — cannot be
    /// obtained. The connection is then refused rather than served badly.
    fn accept(&self) -> io::Result<Self::Service>;
}

/// Why a connection loop ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// The peer closed cleanly.
    Closed,
    /// The peer asked the agent to stop.
    ShutdownRequested,
}

/// What went wrong serving a connection.
#[derive(Debug)]
pub enum ServerError {
    /// The socket failed.
    Io(io::Error),
    /// The peer sent something the protocol does not allow. The connection is closed.
    Protocol(WireError),
    /// A frame arrived before `Hello`.
    HelloNotFirst {
        /// What arrived instead.
        opcode: u16,
    },
    /// An opcode this build does not serve.
    UnknownOpcode(u16),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "connection failed: {e}"),
            Self::Protocol(e) => write!(f, "peer spoke badly: {e}"),
            Self::HelloNotFirst { opcode } => write!(
                f,
                "opcode {opcode} arrived before Hello; the handshake is mandatory because a \
                 stale peer must be refused rather than measured"
            ),
            Self::UnknownOpcode(op) => write!(f, "unknown opcode {op}"),
        }
    }
}

impl std::error::Error for ServerError {}

impl From<io::Error> for ServerError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WireError> for ServerError {
    fn from(e: WireError) -> Self {
        Self::Protocol(e)
    }
}

/// Serve one connection until it closes or asks for shutdown.
///
/// Generic over the transport, so a whole conversation can be driven over an in-memory pipe.
///
/// # Errors
///
/// On any protocol violation or socket failure. Every one of those closes the connection;
/// see the module docs on why there is no error reply.
pub fn serve_connection<S: Read + Write, H: Service>(
    stream: &mut S,
    service: &mut H,
    max_body: u32,
) -> Result<Served, ServerError> {
    serve_connection_until(stream, service, max_body, &|| false)
}

/// Serve one connection, giving up between frames when `stop` says so.
///
/// `stop` is consulted only in the gap *between* frames, never mid-frame: a run that ended
/// must not leave a half-read message looking like a protocol error.
///
/// # Errors
///
/// As [`serve_connection`].
pub fn serve_connection_until<S: Read + Write, H: Service>(
    stream: &mut S,
    service: &mut H,
    max_body: u32,
    stop: &dyn Fn() -> bool,
) -> Result<Served, ServerError> {
    let mut greeted = false;
    let mut body = Vec::new();
    loop {
        let mut head = [0u8; HEADER_BYTES];
        match read_header(stream, &mut head, stop) {
            Ok(Next::Frame) => {}
            // A clean close between frames is how a run ends, not a failure.
            Ok(Next::Closed) => return Ok(Served::Closed),
            Ok(Next::Stopped) => return Ok(Served::Closed),
            Err(e) => return Err(ServerError::Io(e)),
        }
        let header = Header::decode(&head, max_body)?;
        body.clear();
        body.resize(header.len as usize, 0);
        // A header arrived, so the body is owed: a close here is truncation, which `fill`
        // reports as `UnexpectedEof` rather than as the clean end of a run.
        if !fill(stream, &mut body).map_err(ServerError::Io)? {
            return Err(ServerError::Protocol(WireError::Truncated {
                need: header.len as usize,
                have: 0,
            }));
        }

        if !greeted && header.opcode != opcode::HELLO {
            return Err(ServerError::HelloNotFirst {
                opcode: header.opcode,
            });
        }

        let reply = match header.opcode {
            opcode::HELLO => {
                greeted = true;
                service.hello(&Hello::decode(&body)?).encode()
            }
            opcode::SUBMIT_TURN => {
                let turn = SubmitTurn::decode(&body)?;
                service.submit_turn(&turn).encode()
            }
            opcode::DRAIN => service.drain().encode(),
            opcode::STATS => service.stats().encode(),
            opcode::SHUTDOWN => {
                let ack = service.shutdown().encode();
                write_frame(stream, header.opcode, header.corr, &ack).map_err(ServerError::Io)?;
                return Ok(Served::ShutdownRequested);
            }
            other => return Err(ServerError::UnknownOpcode(other)),
        };
        write_frame(stream, header.opcode, header.corr, &reply).map_err(ServerError::Io)?;
    }
}

/// Write a reply, echoing the request's opcode and correlation id.
fn write_frame<S: Write>(stream: &mut S, opcode: u16, corr: u32, body: &[u8]) -> io::Result<()> {
    let mut w = Writer::with_capacity(body.len());
    w.bytes(body);
    stream.write_all(&w.frame(opcode, 0, corr))?;
    stream.flush()
}

/// What waiting for the next frame produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// A header arrived.
    Frame,
    /// The peer closed between frames.
    Closed,
    /// The run was stopped while the connection was idle.
    Stopped,
}

/// Wait for the next frame's header, tolerating idle gaps and honouring `stop`.
///
/// A read timeout with nothing yet received is an idle gap, not a failure. Once any byte of
/// the header has arrived the peer is mid-send, so a timeout is retried rather than treated
/// as the end: giving up there would make a slow network indistinguishable from a broken
/// peer.
fn read_header<S: Read>(
    stream: &mut S,
    buf: &mut [u8; HEADER_BYTES],
    stop: &dyn Fn() -> bool,
) -> io::Result<Next> {
    let mut at = 0usize;
    loop {
        if at == 0 && stop() {
            return Ok(Next::Stopped);
        }
        match stream.read(&mut buf[at..]) {
            Ok(0) if at == 0 => return Ok(Next::Closed),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("peer closed {at} bytes into a frame header"),
                ))
            }
            Ok(n) => {
                at += n;
                if at == buf.len() {
                    return Ok(Next::Frame);
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
}

/// Fill `buf`.
///
/// `Ok(false)` means the peer closed with **none** of it read, which is how a run ends and
/// is not a failure. A close *part-way* through is a truncated frame and is an error: the
/// two must not be conflated, or a half-delivered `SubmitTurn` would end the connection
/// quietly as though the generator had finished.
fn fill<S: Read>(stream: &mut S, buf: &mut [u8]) -> io::Result<bool> {
    if buf.is_empty() {
        return Ok(true);
    }
    let mut at = 0usize;
    while at < buf.len() {
        match stream.read(&mut buf[at..]) {
            Ok(0) if at == 0 => return Ok(false),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("peer closed {at} bytes into a {}-byte read", buf.len()),
                ))
            }
            Ok(n) => at += n,
            // Mid-frame: a timeout means the peer is still sending, so wait rather than
            // turn a slow network into a protocol error.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// A listening agent.
#[derive(Debug)]
pub struct Server<F> {
    listener: TcpListener,
    factory: Arc<F>,
    max_body: u32,
    linger: Duration,
    first_connect_timeout: Duration,
}

impl<F: ServiceFactory> Server<F> {
    /// Bind a port.
    ///
    /// # Errors
    ///
    /// If the address cannot be bound.
    pub fn bind<A: ToSocketAddrs>(addr: A, factory: F) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr)?,
            factory: Arc::new(factory),
            max_body: DEFAULT_MAX_BODY,
            linger: DEFAULT_LINGER,
            first_connect_timeout: DEFAULT_FIRST_CONNECT_TIMEOUT,
        })
    }

    /// The bound address, which is how a test learns the port when it asked for zero.
    ///
    /// # Errors
    ///
    /// If the socket cannot report it.
    pub fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.listener.local_addr()
    }

    /// Set the largest body this server will accept.
    pub fn with_max_body(mut self, max_body: u32) -> Self {
        self.max_body = max_body;
        self
    }

    /// How long to wait after the last connection closes before exiting.
    ///
    /// Zero exits as soon as the count reaches zero, which is what a test wants and what a run
    /// does not: a generator opening its lanes one at a time would be cut off.
    pub fn with_linger(mut self, linger: Duration) -> Self {
        self.linger = linger;
        self
    }

    /// How long to wait for the very first connection before giving up.
    pub fn with_first_connect_timeout(mut self, timeout: Duration) -> Self {
        self.first_connect_timeout = timeout;
        self
    }

    /// Accept connections until `stop` is set or a peer asks for shutdown.
    ///
    /// One thread per connection: a connection is a lane, and a lane's frames must be
    /// handled in order, so a thread per connection is the shape the ordering rule wants
    /// rather than a convenience. Concurrency across lanes is what remains.
    ///
    /// # Errors
    ///
    /// If the listener cannot be polled. A failure on one connection is logged to stderr
    /// and closes that connection; it does not stop the agent, because the generator will
    /// see the close, name the node and abort the run itself.
    pub fn serve(&self, stop: Arc<AtomicBool>) -> io::Result<()> {
        self.listener.set_nonblocking(true)?;
        let mut threads = Vec::new();
        // Connections currently open, and whether there has ever been one. Together they let a
        // closed socket mean "the run is over" without a freshly launched agent exiting for
        // having no clients yet.
        let live = Arc::new(AtomicUsize::new(0));
        let mut ever_connected = false;
        let started = std::time::Instant::now();
        let mut empty_since: Option<std::time::Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, peer)) => {
                    stream.set_nonblocking(false)?;
                    // Nagle would batch small frames and add latency to exactly the
                    // messages whose latency is the measurement.
                    stream.set_nodelay(true)?;
                    let mut service = match self.factory.accept() {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("refusing {peer}: {e}");
                            continue;
                        }
                    };
                    // A read timeout is what lets an idle connection notice the run has
                    // ended, so teardown can be verified rather than hoped for (FR-053).
                    stream.set_read_timeout(Some(IDLE_POLL))?;
                    let max_body = self.max_body;
                    let stop = Arc::clone(&stop);
                    ever_connected = true;
                    empty_since = None;
                    live.fetch_add(1, Ordering::Relaxed);
                    let live_for_thread = Arc::clone(&live);
                    threads.push(thread::spawn(move || {
                        let mut stream = stream;
                        let stopped = Arc::clone(&stop);
                        let result =
                            serve_connection_until(&mut stream, &mut service, max_body, &|| {
                                stopped.load(Ordering::Relaxed)
                            });
                        match result {
                            Ok(Served::ShutdownRequested) => stop.store(true, Ordering::Relaxed),
                            Ok(Served::Closed) => {}
                            Err(e) => eprintln!("connection from {peer} closed: {e}"),
                        }
                        live_for_thread.fetch_sub(1, Ordering::Relaxed);
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(ACCEPT_POLL);
                    let open = live.load(Ordering::Relaxed);
                    if !ever_connected {
                        // Nobody has ever connected. An agent launched for a run that never
                        // came must not sit here holding channels indefinitely.
                        if started.elapsed() > self.first_connect_timeout {
                            eprintln!(
                                "no generator connected within {:?}; exiting rather than \
                                 holding this node's mailbox channels",
                                self.first_connect_timeout
                            );
                            break;
                        }
                        continue;
                    }
                    if open == 0 {
                        // Every connection is gone. TCP has told us the control process is no
                        // longer there, which is more reliable than any silence-based guess —
                        // but a generator opening its lanes one at a time can pass through zero,
                        // so wait out the linger before believing it.
                        let since = *empty_since.get_or_insert_with(std::time::Instant::now);
                        if since.elapsed() >= self.linger {
                            eprintln!(
                                "every connection closed and none reopened within {:?}; the \
                                 control process is gone, so releasing this node's resources",
                                self.linger
                            );
                            break;
                        }
                    } else {
                        empty_since = None;
                    }
                }
                Err(e) => return Err(e),
            }
        }
        for t in threads {
            let _ = t.join();
        }
        Ok(())
    }
}

/// A [`ServiceFactory`] built from a closure, for tests and for a single-purpose agent.
pub struct FnFactory<T>(pub T);

impl<T, S> ServiceFactory for FnFactory<T>
where
    T: Fn() -> io::Result<S> + Send + Sync + 'static,
    S: Service,
{
    type Service = S;

    fn accept(&self) -> io::Result<S> {
        (self.0)()
    }
}

impl<T> std::fmt::Debug for FnFactory<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FnFactory")
    }
}

/// A [`Service`] wrapper over a [`TcpStream`], so a connection can be served directly.
///
/// # Errors
///
/// As [`serve_connection`].
pub fn serve_tcp<H: Service>(
    stream: &mut TcpStream,
    service: &mut H,
    max_body: u32,
) -> Result<Served, ServerError> {
    stream.set_nodelay(true)?;
    serve_connection(stream, service, max_body)
}

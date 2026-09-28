//! The client half: framing I/O, a pipelining window, and correlation-id matching.
//!
//! # Pipelining is bounded, configurable, and independent of lane count
//!
//! Sustained batch rate is pipelining depth divided by round-trip time, so a synchronous
//! request-per-turn client would cap at one turn per RTT however fast the node is. The
//! window fixes that, and its depth is a parameter of the *transport* rather than of the
//! workload: FR-072 requires that concurrency choices not change what is issued, so depth
//! must be settable without touching lane count, and lane count must be settable without
//! touching depth.
//!
//! # What ordering the generator owns, and what belongs to Certus
//!
//! Only one ordering property is the generator's, and it is a **causal** one: a session's
//! turns depend on each other, because turn *n+1*'s path contains the blocks turn *n*
//! stored. So two turns of the same session must not be in flight together **on one
//! instance**. Pipelining preserves that because the agent takes one connection's frames in
//! arrival order and a lane owns a fixed set of sessions on its own connection, so overlap
//! comes from having several lanes rather than from reordering within one. An agent that
//! handed a connection's frames to a thread pool would break it.
//!
//! ## A migration is the exception, and it is benign by construction
//!
//! The premise above — a lane owns a fixed set of sessions — does not survive a migration.
//! A session's within-instance lane index does not change, but its *instance* does, so its
//! turns move to a different queue served by a different thread, and nothing orders the two.
//! Turn *n* can still be in flight on the old instance while turn *n+1* goes out on the new
//! one. Most likely in work-conserving mode, where no think time separates them.
//!
//! It is harmless for two reasons worth stating rather than assuming. The two turns touch
//! **different caches**, so turn *n+1* misses on arrival whether or not turn *n* has
//! finished — nothing moved the blocks. And the generator never reads anything back to build
//! a turn: a path is computed by the model from the session's own history, so a pending turn
//! cannot corrupt the plan. The causal dependency is a claim about what a cache ought to
//! hold, not a data dependency in the driver.
//!
//! **One deployment weakens the first reason**: with Certus's remote lookup enabled the
//! caches are not independent, and whether the old instance's store has landed decides
//! whether the new one's remote fetch finds the prefix. That race is bounded by the
//! migration count and falls inside a class the measurement methodology already answers —
//! see `contracts/hardware.md`.
//!
//! The symptom if it were broken is worth knowing, because it is quiet: turn *n+1* would
//! `CHECK` a prefix turn *n* had not yet stored, find it absent and store it **again**.
//! The run would still complete, with inflated store counts and a depressed hit rate.
//!
//! **Everything past submission is Certus's own business.** The mailbox has parallel
//! channels and the server runs multiple threads, so two requests already submitted may be
//! processed in either order, and there is no way to prevent that without collapsing to one
//! channel and giving up the parallelism the measurement exists to exercise. That
//! reordering is Certus-internal and is *not* a generator concern: this client makes no
//! end-to-end ordering claim, and nothing here should be changed to try to enforce one.
//! `CHECK`'s `PENDING` state exists precisely because Certus expects concurrent stores of
//! the same key and reports them, so the design already assumes this.
//!
//! Correlation ids are matched rather than assumed for the same reason the reply stream is
//! not trusted to be ordered: the contract permits a multiplexed connection, and an
//! in-order reply stream must not become an unstated assumption of the client.
//!
//! # A read timeout, because a hang is not an abort
//!
//! FR-064 requires a run to abort and name a node that becomes unreachable. A peer that
//! accepts a connection and then never answers is exactly that case, and without a read
//! timeout the generator blocks in `read` forever — the run neither finishes nor fails, which
//! is worse than either. Found by a leftover-detection test against a socket that accepted and
//! stayed silent.
//!
//! [`DEFAULT_READ_TIMEOUT`] is generous, because a legitimate turn does real work on the far
//! side: the agent checks a path, loads and stores against a live mailbox, and that may take a
//! while under load. A probe that only wants to know whether *anything* is there should set a
//! short one with [`Client::with_read_timeout`] rather than wait for the default.
//!
//! # The window is a buffer bound, not only a pipeline
//!
//! [`Client::submit`] reads a reply only when the window is full, so `inflight` counts
//! **replies not yet read** rather than work the far side has not finished. Under a finite
//! pacing rate the window therefore still fills and stays full, with the replies sitting
//! unread in the receive buffer — it does not go idle, which is the opposite of what the
//! name suggests.
//!
//! That makes the window load-bearing for two reasons beyond hiding a round trip, and both
//! survive eager reaping:
//!
//! 1. **The plan queue must be the only buffer.** Lateness is stamped when a frame is handed
//!    to the socket, so turns idling in a kernel send buffer would record as on time while
//!    the agent ran minutes behind. The window bounds that gap to `depth` turns; without it
//!    a run that missed its schedule could report itself punctual, and FR-062's underrun
//!    count would go quiet at the same time. The metric would lie in the flattering
//!    direction.
//! 2. **A hung peer must abort, not hang.** At the limit we block in a *read*, which
//!    [`DEFAULT_READ_TIMEOUT`] turns into a bounded `NodeLost`. Writing until the socket
//!    blocked instead would park us in `write_all`, and a run that neither finishes nor
//!    fails cannot even be reported as invalid.
//!
//! # Reaping replies eagerly, so a lost node is noticed now
//!
//! Because a full window drains exactly one reply, a node that dies is noticed only when its
//! turn reaches the front of the queue — up to `depth` turns late, which under pacing can be
//! many seconds. [`Client::reap_ready`] closes that: it takes every reply already waiting
//! without blocking, so death is seen at the next submission rather than `depth` submissions
//! later. It costs one non-blocking read per submit, usually `EWOULDBLOCK`.
//!
//! It is deliberately **not** a separate reaper thread. `inflight` is driven through
//! `&mut self`, so a reaper would need a mutex on the submit path and would buy nothing that
//! a non-blocking read on the same thread does not.
//!
//! # `TCP_NODELAY`, always
//!
//! Nagle's algorithm withholds a small write until the previous one is acknowledged, which
//! is precisely the wrong behaviour for a stream of small frames whose latency is the thing
//! being measured. It is set on connect and is not optional.
//!
//! # Examples
//!
//! The window is the thing to understand. At depth *d*, the *d+1*-th [`Client::submit`]
//! has to drain one reply to make room, and it hands that reply back — so a caller that
//! ignores the return value still gets correct backpressure and merely loses a
//! measurement:
//!
//! ```
//! use std::sync::atomic::AtomicBool;
//! use std::sync::Arc;
//! use std::time::Duration;
//!
//! use workload_wire::client::Client;
//! use workload_wire::frame::SubmitTurn;
//! use workload_wire::server::{FnFactory, Server, Service};
//! # use workload_wire::frame::{Hello, HelloAck, TurnOutcome};
//! # struct EveryKeyResident;
//! # impl Service for EveryKeyResident {
//! #     fn hello(&mut self, h: &Hello) -> HelloAck { workload_wire::handshake::answer(h, 8, 32768) }
//! #     fn submit_turn(&mut self, t: &SubmitTurn) -> TurnOutcome {
//! #         TurnOutcome { resident: t.path.len() as u32, ..Default::default() }
//! #     }
//! # }
//!
//! let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(EveryKeyResident)))
//!     .unwrap()
//!     .with_linger(Duration::ZERO);
//! let addr = server.local_addr().unwrap();
//! let serving = std::thread::spawn(move || {
//!     server.serve(Arc::new(AtomicBool::new(false))).unwrap();
//! });
//!
//! // Depth 2: a transport choice, and deliberately not a workload one (FR-072).
//! let mut client = Client::connect(addr, 2, Some(Duration::from_secs(10))).unwrap();
//! client.handshake("localhost", "/dev/shm/certus-shmq", 2, 32768).unwrap();
//! assert_eq!(client.depth(), 2);
//!
//! let turn = |session: u64, path: Vec<u64>| SubmitTurn { session, flags: 0, path };
//!
//! // Two fit in the window, so nothing comes back yet.
//! assert!(client.submit(&turn(1, vec![10])).unwrap().1.is_none());
//! assert!(client.submit(&turn(2, vec![20, 21])).unwrap().1.is_none());
//! assert_eq!(client.inflight(), 2);
//!
//! // The third displaces the oldest, and returns its outcome.
//! let (_corr, displaced) = client.submit(&turn(3, vec![30, 31, 32])).unwrap();
//! assert_eq!(displaced.unwrap().resident, 1);
//! assert_eq!(client.inflight(), 2);
//!
//! let rest = client.finish().unwrap();
//! assert_eq!(rest.len(), 2);
//! assert_eq!(rest[1].resident, 3);
//!
//! client.shutdown().unwrap();
//! serving.join().unwrap();
//! ```
//!
//! The synchronous calls — [`Client::drain`], [`Client::stats`], [`Client::shutdown`] —
//! refuse while replies are outstanding, rather than reading a `TurnOutcome` as though it
//! were their own reply:
//!
//! ```
//! # use std::sync::atomic::AtomicBool;
//! # use std::sync::Arc;
//! # use std::time::Duration;
//! # use workload_wire::client::Client;
//! # use workload_wire::frame::SubmitTurn;
//! # use workload_wire::server::{FnFactory, Server, Service};
//! # use workload_wire::frame::{Hello, HelloAck, TurnOutcome};
//! # struct S;
//! # impl Service for S {
//! #     fn hello(&mut self, h: &Hello) -> HelloAck { workload_wire::handshake::answer(h, 8, 32768) }
//! #     fn submit_turn(&mut self, _t: &SubmitTurn) -> TurnOutcome { TurnOutcome::default() }
//! # }
//! # let server = Server::bind("127.0.0.1:0", FnFactory(|| Ok(S))).unwrap().with_linger(Duration::ZERO);
//! # let addr = server.local_addr().unwrap();
//! # let serving = std::thread::spawn(move || { server.serve(Arc::new(AtomicBool::new(false))).unwrap(); });
//! let mut client = Client::connect(addr, 4, Some(Duration::from_secs(10))).unwrap();
//! client.handshake("localhost", "/dev/shm/certus-shmq", 4, 32768).unwrap();
//!
//! client.submit(&SubmitTurn { session: 1, flags: 0, path: vec![1] }).unwrap();
//! assert!(client.drain().is_err()); // one reply is still outstanding
//!
//! client.finish().unwrap();
//! assert_eq!(client.drain().unwrap().pending, 0);
//! # client.shutdown().unwrap();
//! # serving.join().unwrap();
//! ```

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::frame::{
    opcode, ClearCacheAck, DrainAck, Header, Hello, HelloAck, ShutdownAck, Stats, SubmitTurn,
    TurnOutcome, WireError, Writer, HEADER_BYTES,
};
use crate::handshake::{self, Refused};

/// Why a verified handshake did not complete.
///
/// The two are kept apart because they call for different actions: a transport failure may be
/// a network or a launch problem, while a refusal means the deployment is wrong and rerunning
/// will not help.
#[derive(Debug)]
pub enum HandshakeError {
    /// The exchange itself failed.
    Transport(ClientError),
    /// The peer was reached and is not acceptable.
    Refused(Refused),
}

impl fmt::Display for HandshakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::Refused(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for HandshakeError {}

/// Default pipelining depth, and the only one — there is deliberately no flag for it.
///
/// The contract's arithmetic: at 1M keys/second in 64-key batches — beyond anything measured
/// on this hardware — a 50 µs round trip needs 0.78 batches in flight, so 8 is an order of
/// magnitude of headroom. Measured on loopback, depth 8 at a ~30 µs round trip is about
/// 260 000 turns/second against roughly 1 000 observed.
///
/// # Why a bigger window would not cost a latency measurement
///
/// Do not reach for the tempting argument that raising it "would only add queueing delay to a
/// latency measurement" — it does not hold here. FR-066 makes the mailbox-facing code the sole
/// collector, and since FR-079 that is always the agent, timing its own mailbox requests, so
/// wire queueing sits outside every reported latency figure either way.
///
/// The real reason not to raise it is weaker and simpler: there is nothing to gain. A deeper
/// window hides round-trip latency, and none is currently being felt. It is also worth knowing
/// that depth buys **no concurrency at all** — the agent handles one connection's frames one at
/// a time in arrival order, so a deeper window only keeps the socket from idling between turns.
/// Concurrency comes from having several lanes.
///
/// Under a finite pacing rate the window is nearly irrelevant, since turns are submitted when
/// they are due rather than as fast as possible. If a rate sweep ever reports the generator as
/// the limit, this is the first number to try raising.
pub const DEFAULT_DEPTH: usize = 8;

/// Default read timeout.
///
/// Generous on purpose: the far side is doing real work per turn. It exists to turn an
/// unreachable peer into an error rather than a hang, not to bound normal latency.
pub const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Default write timeout.
///
/// Not a live hazard while the pipelining window holds — a `SubmitTurn` with a 64-key path is
/// about 526 bytes, so a write blocks only once buffers are full, which the window prevents.
/// It closes the same class of gap [`DEFAULT_READ_TIMEOUT`] closed: an unbounded block is a run
/// that neither finishes nor fails, and that cannot even be reported as invalid.
pub const DEFAULT_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Default largest body accepted, in bytes.
///
/// A `SubmitTurn` is 14 bytes plus 8 per key, so this admits a path of about 130 000 keys —
/// far beyond any single turn — while still refusing a corrupt or hostile length.
pub const DEFAULT_MAX_BODY: u32 = 1 << 20;

/// What can go wrong talking to an agent.
#[derive(Debug)]
pub enum ClientError {
    /// The socket failed.
    Io(io::Error),
    /// The peer sent something this protocol does not allow.
    Protocol(WireError),
    /// The peer closed the connection.
    ///
    /// Distinguished from an I/O error because a run must name the lost node and abort
    /// rather than continue on the survivors (FR-064).
    Closed,
    /// A reply arrived whose correlation id was never sent, or was already answered.
    Unexpected {
        /// The id the peer echoed.
        corr: u32,
    },
    /// A reply carried an opcode the request did not ask for.
    Mismatched {
        /// What was expected.
        want: u16,
        /// What arrived.
        got: u16,
    },
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "agent connection failed: {e}"),
            Self::Protocol(e) => write!(f, "agent spoke badly: {e}"),
            Self::Closed => write!(f, "the agent closed the connection"),
            Self::Unexpected { corr } => write!(
                f,
                "the agent echoed correlation id {corr}, which was never sent or was \
                 already answered"
            ),
            Self::Mismatched { want, got } => {
                write!(f, "expected a reply to opcode {want}, got opcode {got}")
            }
        }
    }
}

impl std::error::Error for ClientError {}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<WireError> for ClientError {
    fn from(e: WireError) -> Self {
        Self::Protocol(e)
    }
}

/// A framed connection to one agent.
///
/// Generic over the transport so the protocol can be exercised over an in-memory pipe: a
/// client that can only be tested against a live agent is one that gets tested rarely.
#[derive(Debug)]
pub struct Client<S> {
    stream: S,
    depth: usize,
    max_body: u32,
    next_corr: u32,
    /// Correlation ids sent and not yet answered, oldest first.
    inflight: VecDeque<u32>,
    body: Vec<u8>,
}

impl Client<TcpStream> {
    /// Connect to an agent, with `TCP_NODELAY` set.
    ///
    /// # Errors
    ///
    /// If the address does not resolve, the connection fails, or `TCP_NODELAY` cannot be
    /// set — which is a refusal rather than a warning, since Nagle would add latency to
    /// exactly the frames whose latency is being measured.
    pub fn connect<A: ToSocketAddrs>(
        addr: A,
        depth: usize,
        timeout: Option<Duration>,
    ) -> Result<Self, ClientError> {
        let addrs: Vec<_> = addr.to_socket_addrs()?.collect();
        let first = addrs
            .first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address resolved"))?;
        let stream = match timeout {
            Some(t) => TcpStream::connect_timeout(first, t)?,
            None => TcpStream::connect(&addrs[..])?,
        };
        stream.set_nodelay(true)?;
        // Without this, a peer that accepts and never answers hangs the run instead of failing
        // it, which FR-064 forbids in substance if not in words.
        stream.set_read_timeout(Some(DEFAULT_READ_TIMEOUT))?;
        // The same gap on the writing side. The window makes it unreachable in practice; it is
        // set anyway, because the cost is nothing and the failure it prevents is a hang.
        stream.set_write_timeout(Some(DEFAULT_WRITE_TIMEOUT))?;
        Ok(Self::with_transport(stream, depth))
    }
}

impl Client<TcpStream> {
    /// Set the read timeout, replacing [`DEFAULT_READ_TIMEOUT`].
    ///
    /// A short one suits a probe that only wants to know whether anything is listening; the
    /// default suits driving turns, where the far side is doing real work.
    ///
    /// # Errors
    ///
    /// If the socket refuses it.
    pub fn with_read_timeout(self, timeout: Duration) -> Result<Self, ClientError> {
        self.stream.set_read_timeout(Some(timeout))?;
        Ok(self)
    }

    /// Take every reply already waiting, without blocking, and return how many.
    ///
    /// Call it before [`Client::submit`]. It returns credits to the window early, so a peer that
    /// has died is seen at the next submission instead of `depth` submissions later — under a
    /// paced run, seconds rather than turns. With nothing waiting it costs one `EWOULDBLOCK`.
    ///
    /// A count rather than the outcomes themselves: the driver discards them, and allocating a
    /// vector per submission to be dropped would be a cost on the hot path for nothing. The
    /// outcomes are still available through [`Client::submit`] and [`Client::finish`].
    ///
    /// # Errors
    ///
    /// If the peer closed, sent something malformed, or echoed an unknown correlation id.
    /// Draining is where a dead node is now discovered, so these are real failures rather than
    /// conditions to skip past.
    pub fn reap_ready(&mut self) -> Result<usize, ClientError> {
        let mut reaped = 0usize;
        // Guarded on `inflight`: with nothing outstanding there is no reply to be had, and a
        // read would steal a frame that belongs to a later synchronous call.
        while !self.inflight.is_empty() {
            let Some((header, body)) = self.reap_one()? else {
                break;
            };
            self.match_outcome(&header, &body)?;
            reaped += 1;
        }
        Ok(reaped)
    }

    /// Read one whole frame if one has already arrived, otherwise `None`.
    ///
    /// Only the *first* look is non-blocking. Bytes cannot be pushed back onto a stream, so
    /// once any byte of a header is taken the frame must be completed — blocking, under the
    /// read timeout, exactly as [`Client::recv_outcome`] would have.
    fn reap_one(&mut self) -> Result<Option<(Header, Vec<u8>)>, ClientError> {
        let mut head = [0u8; HEADER_BYTES];
        let got = self.read_available(&mut head)?;
        if got == 0 {
            return Ok(None);
        }
        if got < HEADER_BYTES {
            Self::fill(&mut self.stream, &mut head[got..])?;
        }
        let header = Header::decode(&head, self.max_body)?;
        // Taken out of `self` so the read borrows the stream and the buffer separately, then put
        // back so the allocation is reused across frames.
        let mut body = std::mem::take(&mut self.body);
        body.clear();
        body.resize(header.len as usize, 0);
        let read = Self::fill(&mut self.stream, &mut body);
        let out = body.clone();
        self.body = body;
        read?;
        Ok(Some((header, out)))
    }

    /// Read whatever is available into `buf` without blocking, returning how many bytes.
    ///
    /// Zero means nothing had arrived. A short count means a frame is part-way here and the
    /// caller must finish it.
    fn read_available(&mut self, buf: &mut [u8]) -> Result<usize, ClientError> {
        self.stream.set_nonblocking(true)?;
        let outcome = {
            let mut at = 0usize;
            loop {
                match self.stream.read(&mut buf[at..]) {
                    // End of stream, whether or not part of a header arrived first: a run must
                    // name the lost node and abort rather than carry on (FR-064).
                    Ok(0) => break Err(ClientError::Closed),
                    Ok(n) => {
                        at += n;
                        if at == buf.len() {
                            break Ok(at);
                        }
                    }
                    // The whole point of the call: nothing more is here yet.
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break Ok(at),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => break Err(ClientError::Io(e)),
                }
            }
        };
        // Restored on every path, including the failing ones. A socket left non-blocking would
        // turn every later blocking read into a spurious `WouldBlock`, which the read timeout is
        // indistinguishable from — so the run would report a lost node for a bookkeeping slip.
        let restored = self.stream.set_nonblocking(false);
        let n = outcome?;
        restored?;
        Ok(n)
    }
}

impl<S: Read + Write> Client<S> {
    /// A client over an arbitrary transport.
    ///
    /// # Panics
    ///
    /// If `depth` is zero: a window of nothing could never send, and silently promoting it
    /// to one would hide a caller's arithmetic error behind a working run.
    pub fn with_transport(stream: S, depth: usize) -> Self {
        assert!(depth > 0, "a pipelining depth of 0 could never send");
        Self {
            stream,
            depth,
            max_body: DEFAULT_MAX_BODY,
            next_corr: 1,
            inflight: VecDeque::with_capacity(depth),
            body: Vec::new(),
        }
    }

    /// Set the largest body this client will accept from the agent.
    pub fn with_max_body(mut self, max_body: u32) -> Self {
        self.max_body = max_body;
        self
    }

    /// The pipelining depth, which is a transport choice and never a workload one.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Frames sent and not yet answered.
    pub fn inflight(&self) -> usize {
        self.inflight.len()
    }

    /// The underlying transport, for a property only it can answer — `TCP_NODELAY`.
    pub fn transport(&self) -> &S {
        &self.stream
    }

    /// Consume the client and return its transport, so a test can inspect what was written.
    pub fn into_transport(self) -> S {
        self.stream
    }

    /// Perform the handshake and return the agent's reply, unverified.
    ///
    /// Prefer [`Client::handshake`], which refuses a peer that is not this build. This exists
    /// for tests that need to see an unacceptable reply rather than an error.
    ///
    /// # Errors
    ///
    /// If the socket fails or the reply is malformed.
    pub fn hello(&mut self, hello: &Hello) -> Result<HelloAck, ClientError> {
        let body = self.request(opcode::HELLO, 0, &hello.encode())?;
        Ok(HelloAck::decode(&body)?)
    }

    /// Perform the handshake and **refuse** a peer that is not this build (FR-051).
    ///
    /// `node` appears in every refusal, because on a cluster the useful part of the message is
    /// which machine is wrong. `lanes` and `block_bytes` are checked against the capacity the
    /// agent reports: the mailbox is depth-1 per channel, so over-subscribing it does not fail
    /// but serialises silently, which would read as a slow server rather than a misconfigured
    /// run.
    ///
    /// # Errors
    ///
    /// [`HandshakeError::Transport`] if the exchange failed, or
    /// [`HandshakeError::Refused`] if the peer is unacceptable. There is no lenient mode.
    pub fn handshake(
        &mut self,
        node: &str,
        mailbox: &str,
        lanes: usize,
        block_bytes: u32,
    ) -> Result<HelloAck, HandshakeError> {
        let ack = self
            .hello(&handshake::hello(mailbox))
            .map_err(HandshakeError::Transport)?;
        handshake::verify(node, &ack).map_err(HandshakeError::Refused)?;
        handshake::check_capacity(node, &ack, lanes, block_bytes)
            .map_err(HandshakeError::Refused)?;
        Ok(ack)
    }

    /// Submit one turn, pipelined.
    ///
    /// Returns the outcome of an **earlier** turn when the window was full and one had to
    /// be drained to make room, so a caller that ignores the return value still applies
    /// backpressure correctly — it just discards a measurement. The correlation id of the
    /// turn now in flight is returned alongside.
    ///
    /// # Errors
    ///
    /// If the socket fails, the agent closes, or a reply is malformed.
    pub fn submit(&mut self, turn: &SubmitTurn) -> Result<(u32, Option<TurnOutcome>), ClientError> {
        let drained = if self.inflight.len() >= self.depth {
            Some(self.recv_outcome()?)
        } else {
            None
        };
        let corr = self.send(opcode::SUBMIT_TURN, turn.flags, &turn.encode())?;
        Ok((corr, drained.map(|(_, outcome)| outcome)))
    }

    /// Wait for the oldest outstanding turn's outcome.
    ///
    /// # Errors
    ///
    /// If nothing is outstanding, the socket fails, or the reply does not match.
    pub fn recv_outcome(&mut self) -> Result<(u32, TurnOutcome), ClientError> {
        // Checked before reading rather than after: with nothing outstanding there is no reply
        // coming, and blocking here would consume a frame belonging to a later synchronous call.
        if self.inflight.is_empty() {
            return Err(ClientError::Unexpected { corr: 0 });
        }
        let (header, body) = self.read_frame()?;
        self.match_outcome(&header, &body)
    }

    /// Account one `SUBMIT_TURN` reply against the window and decode it.
    ///
    /// Shared by the blocking and the eager paths, so the two cannot drift on which replies
    /// they accept — a divergence that would show up as a correlation-id error under load and
    /// nowhere else.
    fn match_outcome(
        &mut self,
        header: &Header,
        body: &[u8],
    ) -> Result<(u32, TurnOutcome), ClientError> {
        if header.opcode != opcode::SUBMIT_TURN {
            return Err(ClientError::Mismatched {
                want: opcode::SUBMIT_TURN,
                got: header.opcode,
            });
        }
        // Matched rather than assumed: the contract permits a multiplexed connection, so an
        // in-order reply stream must not become an unstated assumption here.
        if self.inflight.front().copied() == Some(header.corr) {
            self.inflight.pop_front();
        } else if self.inflight.iter().any(|c| *c == header.corr) {
            self.inflight.retain(|c| *c != header.corr);
        } else {
            return Err(ClientError::Unexpected { corr: header.corr });
        }
        Ok((header.corr, TurnOutcome::decode(body)?))
    }

    /// Wait for every outstanding turn.
    ///
    /// # Errors
    ///
    /// If the socket fails or a reply is malformed.
    pub fn finish(&mut self) -> Result<Vec<TurnOutcome>, ClientError> {
        let mut out = Vec::with_capacity(self.inflight.len());
        while !self.inflight.is_empty() {
            out.push(self.recv_outcome()?.1);
        }
        Ok(out)
    }

    /// Ask the agent to finish outstanding work.
    ///
    /// # Errors
    ///
    /// If anything is still pipelined — draining with replies outstanding would read a
    /// `TurnOutcome` as a `DrainAck` — or if the socket fails.
    pub fn drain(&mut self) -> Result<DrainAck, ClientError> {
        self.require_quiet("drain")?;
        let body = self.request(opcode::DRAIN, 0, &[])?;
        Ok(DrainAck::decode(&body)?)
    }

    /// Collect the agent's per-operation latency histograms.
    ///
    /// Histograms, not percentiles: quantiles do not merge across nodes, so the caller
    /// merges the histograms and only then takes a quantile.
    ///
    /// # Errors
    ///
    /// If anything is still pipelined, or the socket fails.
    pub fn stats(&mut self) -> Result<Stats, ClientError> {
        self.require_quiet("stats")?;
        let body = self.request(opcode::STATS, 0, &[])?;
        Ok(Stats::decode(&body)?)
    }

    /// Ask the node to clear its memory tier, before the timed window opens (FR-046).
    ///
    /// The generator has no mailbox of its own under FR-079, so this is the only way a run can
    /// start from a known cache state. It is **setup**: never issued inside the timed window, and
    /// never mid-run, because clearing then would be the generator evicting on the policy's
    /// behalf (FR-043).
    ///
    /// # Errors
    ///
    /// If anything is still pipelined, or the socket fails. A node that *answers* but could not
    /// clear reports that in [`ClearCacheAck::error`], which the caller must not ignore.
    pub fn clear_cache(&mut self) -> Result<ClearCacheAck, ClientError> {
        self.require_quiet("clear_cache")?;
        let body = self.request(opcode::CLEAR_CACHE, 0, &[])?;
        Ok(ClearCacheAck::decode(&body)?)
    }

    /// Stop the agent.
    ///
    /// # Errors
    ///
    /// If anything is still pipelined, or the socket fails.
    pub fn shutdown(&mut self) -> Result<ShutdownAck, ClientError> {
        self.require_quiet("shutdown")?;
        let body = self.request(opcode::SHUTDOWN, 0, &[])?;
        Ok(ShutdownAck::decode(&body)?)
    }

    /// Refuse a synchronous call while replies are outstanding.
    ///
    /// Without this the next frame off the socket would be a `TurnOutcome` and would be
    /// decoded as whatever was asked for — a silent misread rather than an error.
    fn require_quiet(&self, what: &'static str) -> Result<(), ClientError> {
        if self.inflight.is_empty() {
            Ok(())
        } else {
            Err(ClientError::Mismatched {
                want: opcode::SUBMIT_TURN,
                got: match what {
                    "drain" => opcode::DRAIN,
                    "stats" => opcode::STATS,
                    "clear_cache" => opcode::CLEAR_CACHE,
                    _ => opcode::SHUTDOWN,
                },
            })
        }
    }

    /// Send a frame and wait for its reply, with nothing else in flight.
    fn request(&mut self, opcode: u16, flags: u16, body: &[u8]) -> Result<Vec<u8>, ClientError> {
        let corr = self.send(opcode, flags, body)?;
        let (header, reply) = self.read_frame()?;
        if header.opcode != opcode {
            return Err(ClientError::Mismatched {
                want: opcode,
                got: header.opcode,
            });
        }
        if header.corr != corr {
            return Err(ClientError::Unexpected { corr: header.corr });
        }
        self.inflight.retain(|c| *c != corr);
        Ok(reply)
    }

    /// Write one frame and record its correlation id.
    fn send(&mut self, opcode: u16, flags: u16, body: &[u8]) -> Result<u32, ClientError> {
        let corr = self.next_corr;
        // Wrapping, skipping zero: zero is reserved so an uninitialised field cannot look
        // like a legitimate id.
        self.next_corr = self.next_corr.wrapping_add(1).max(1);
        let mut w = Writer::with_capacity(body.len());
        w.bytes(body);
        let frame = w.frame(opcode, flags, corr);
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        self.inflight.push_back(corr);
        Ok(corr)
    }

    /// Read one whole frame, refusing an oversized body before allocating for it.
    fn read_frame(&mut self) -> Result<(Header, Vec<u8>), ClientError> {
        let mut head = [0u8; HEADER_BYTES];
        self.read_exact(&mut head)?;
        let header = Header::decode(&head, self.max_body)?;
        // Taken out of `self` so the read borrows the stream and the buffer separately,
        // then put back so the allocation is reused across frames.
        let mut body = std::mem::take(&mut self.body);
        body.clear();
        body.resize(header.len as usize, 0);
        let read = Self::fill(&mut self.stream, &mut body);
        let out = body.clone();
        self.body = body;
        read?;
        Ok((header, out))
    }

    /// Fill `buf`, reporting a clean close as [`ClientError::Closed`].
    fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ClientError> {
        Self::fill(&mut self.stream, buf)
    }

    /// Fill `buf` from `stream`, reporting a clean close as [`ClientError::Closed`].
    ///
    /// A zero-length read is the peer closing, not an error to retry: a run must name the
    /// lost node and abort rather than continue on the survivors (FR-064).
    fn fill(stream: &mut S, buf: &mut [u8]) -> Result<(), ClientError> {
        let mut at = 0usize;
        while at < buf.len() {
            match stream.read(&mut buf[at..]) {
                Ok(0) => return Err(ClientError::Closed),
                Ok(n) => at += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(ClientError::Io(e)),
            }
        }
        Ok(())
    }
}

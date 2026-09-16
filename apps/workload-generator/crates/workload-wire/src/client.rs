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
//! stored. So two turns of the same session must not be in flight together. Pipelining
//! preserves that because the agent takes one connection's frames in arrival order and a
//! lane owns a fixed set of sessions on its own connection, so overlap comes from having
//! several lanes rather than from reordering within one. An agent that handed a
//! connection's frames to a thread pool would break it.
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
//! # `TCP_NODELAY`, always
//!
//! Nagle's algorithm withholds a small write until the previous one is acknowledged, which
//! is precisely the wrong behaviour for a stream of small frames whose latency is the thing
//! being measured. It is set on connect and is not optional.

use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::frame::{
    opcode, DrainAck, Header, Hello, HelloAck, ShutdownAck, Stats, SubmitTurn, TurnOutcome,
    WireError, Writer, HEADER_BYTES,
};

/// Default pipelining depth.
///
/// The contract's arithmetic: at 1M keys/second in 64-key batches — beyond anything measured
/// on this hardware — a 50 µs round trip needs 0.78 batches in flight, so 8 is an order of
/// magnitude of headroom. Bigger would only add queueing delay to a latency measurement.
pub const DEFAULT_DEPTH: usize = 8;

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
        Ok(Self::with_transport(stream, depth))
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

    /// Perform the handshake and return the agent's reply.
    ///
    /// This sends and receives; the fail-closed comparison of `proto_version` and
    /// `build_id` is the caller's, so that the refusal can name the node (T067).
    ///
    /// # Errors
    ///
    /// If the socket fails or the reply is malformed.
    pub fn hello(&mut self, hello: &Hello) -> Result<HelloAck, ClientError> {
        let body = self.request(opcode::HELLO, 0, &hello.encode())?;
        Ok(HelloAck::decode(&body)?)
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
        let expect = self
            .inflight
            .front()
            .copied()
            .ok_or(ClientError::Unexpected { corr: 0 })?;
        let (header, body) = self.read_frame()?;
        if header.opcode != opcode::SUBMIT_TURN {
            return Err(ClientError::Mismatched {
                want: opcode::SUBMIT_TURN,
                got: header.opcode,
            });
        }
        // Matched rather than assumed: the contract permits a multiplexed connection, so an
        // in-order reply stream must not become an unstated assumption here.
        if header.corr != expect {
            let known = self.inflight.iter().any(|c| *c == header.corr);
            if !known {
                return Err(ClientError::Unexpected { corr: header.corr });
            }
            self.inflight.retain(|c| *c != header.corr);
        } else {
            self.inflight.pop_front();
        }
        Ok((header.corr, TurnOutcome::decode(&body)?))
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

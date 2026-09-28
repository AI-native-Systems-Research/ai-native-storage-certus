//! The 12-byte frame header and the body codecs, per `contracts/node-agent-wire.md`.
//!
//! # Everything is little-endian and explicitly sized
//!
//! No padding, no native alignment assumptions. The two ends are the same binary today —
//! `Hello` refuses a peer whose `build_id` differs — but a wire format that depends on the
//! compiler's layout choices is one that breaks silently the first time that stops being
//! true.
//!
//! # `len` is never trusted
//!
//! A reader that allocated `len` bytes before checking it would let any peer, or any
//! corrupted stream, ask for an arbitrary allocation. [`Header::decode`] therefore reports
//! an oversized frame as a protocol error and the connection is closed rather than served.
//! The bound is a parameter rather than a constant because the largest legitimate frame is
//! a `SubmitTurn` of `--batch-keys`-many keys, which is a per-run option (FR-069).
//!
//! # A short read is not a parse
//!
//! Every decoder either consumes exactly the fields it declares or fails. A truncated body
//! is [`WireError::Truncated`], never a partial value with the rest left at zero: a
//! half-decoded `SubmitTurn` would submit a turn with keys the generator never sent.
//!
//! # Examples
//!
//! A frame is a body plus a header, and every body round-trips:
//!
//! ```
//! use workload_wire::frame::{opcode, submit_flags, Header, SubmitTurn, Writer, HEADER_BYTES};
//!
//! let turn = SubmitTurn {
//!     session: 17,
//!     flags: submit_flags::POLL_EVENTS,
//!     path: vec![0xaaaa, 0xbbbb, 0xcccc],
//! };
//!
//! let body = turn.encode();
//! let mut writer = Writer::new();
//! writer.bytes(&body);
//! let frame = writer.frame(opcode::SUBMIT_TURN, 0, 9);
//!
//! let header = Header::decode(&frame, 4096).unwrap();
//! assert_eq!(header.opcode, opcode::SUBMIT_TURN);
//! assert_eq!(header.corr, 9);
//! assert_eq!(header.len as usize, body.len());
//!
//! let back = SubmitTurn::decode(&frame[HEADER_BYTES..]).unwrap();
//! assert_eq!(back, turn);
//! assert!(back.polls_events());
//! ```
//!
//! The two ways a decode refuses, both of which exist because the bytes come from a
//! peer. An oversized `len` is rejected before anything is sized from it:
//!
//! ```
//! use workload_wire::frame::{Header, WireError};
//!
//! let mut frame = [0u8; 12];
//! frame[0..4].copy_from_slice(&(64 * 1024u32).to_le_bytes());
//! assert_eq!(
//!     Header::decode(&frame, 4096),
//!     Err(WireError::TooLarge { len: 65536, max: 4096 }),
//! );
//! ```
//!
//! And a truncated body is an error rather than a partial value:
//!
//! ```
//! use workload_wire::frame::{SubmitTurn, WireError};
//!
//! let full = SubmitTurn { session: 1, flags: 0, path: vec![7, 8, 9] }.encode();
//! // One key short of what the body's own count declares.
//! let clipped = &full[..full.len() - 8];
//! assert!(matches!(
//!     SubmitTurn::decode(clipped),
//!     Err(WireError::Truncated { .. }),
//! ));
//! ```

use std::fmt;

/// Bytes in a frame header.
pub const HEADER_BYTES: usize = 12;

/// The protocol version this build speaks.
///
/// Version 2 replaced a per-operation `Submit` with a per-turn `SubmitTurn`; version 3 added
/// `ClearCache`, which FR-079 needs because the generator no longer touches a mailbox and so
/// cannot issue `CLEAR_MEMORY_TIER` itself. See the contract. `Hello` compares this exactly and
/// refuses a mismatch, so an old agent cannot be driven by a new generator and quietly do
/// something reasonable-looking.
pub const PROTO_VERSION: u32 = 3;

/// Bytes in a `build_id` digest.
pub const BUILD_ID_BYTES: usize = 32;

/// Message opcodes.
///
/// These are the *wire's* opcodes and have nothing to do with the mailbox's; the agent
/// translates. Numbering them separately is what keeps a change to one from silently
/// redefining the other.
pub mod opcode {
    /// Handshake. Must be the first frame on every connection.
    pub const HELLO: u16 = 1;
    /// One turn's key path, root of the prefix through the end of the new growth.
    pub const SUBMIT_TURN: u16 = 2;
    /// Wait for outstanding work.
    pub const DRAIN: u16 = 3;
    /// Stop the agent.
    pub const SHUTDOWN: u16 = 4;
    /// Per-operation latency histograms.
    pub const STATS: u16 = 5;
    /// Clear the node's memory tier, once, before the timed window opens (FR-046).
    ///
    /// A frame rather than something the generator does for itself, because under FR-079 the
    /// generator has no mailbox of its own to issue `CLEAR_MEMORY_TIER` on. It is setup and is
    /// never part of the measured stream — see [`super::ClearCacheAck`] on why it answers with a
    /// refusal rather than a zero when a service cannot do it.
    pub const CLEAR_CACHE: u16 = 6;
}

/// The `op_kind` values a `Stats` reply's histograms are keyed by.
///
/// # These are the **mailbox's** opcode numbers, and that is deliberate
///
/// A histogram is per operation *as Certus saw it*, and the agent is what timed it, so the key is
/// the opcode the agent sent to the mailbox rather than a re-numbering of the plan's abstract
/// kinds. The names here are the only thing the generator needs — it renders them and never acts
/// on them — and they live in the wire because the field does: one end fills it and the other
/// prints it, so a table in either alone would be a private opinion about a shared field.
///
/// The numbers are read from `lib/shmq-dispatcher/src/wire.rs` and are pinned to it by a test in
/// `workload-node-agent`, which is the only crate that can see both. That test is the reason this
/// duplication is safe rather than a second source of truth.
pub mod op_kind {
    /// Ask which of a set of keys is resident.
    pub const CHECK: u8 = 1;
    /// Update eviction ordering for keys already present.
    pub const TOUCH: u8 = 2;
    /// Reserve space for keys about to be stored.
    pub const RESERVE: u8 = 3;
    /// Copy payload from the client's device buffer into the cache.
    pub const COPY_TO_STORE: u8 = 4;
    /// Make a reserved-and-copied key visible.
    pub const COMMIT_STORE: u8 = 5;
    /// Abandon a reservation.
    pub const ABORT_STORE: u8 = 6;
    /// Load resident keys into the client's device buffer.
    pub const LOOKUP: u8 = 9;
    /// Collect cache events.
    pub const TAKE_EVENTS: u8 = 10;

    /// The name for an `op_kind`, for a report a human reads.
    ///
    /// An unknown value is named rather than refused: a `Stats` reply carrying an opcode this
    /// build does not know is a reporting curiosity, and losing the whole run's latency figures
    /// over it would be the wrong trade.
    pub fn name(kind: u32) -> &'static str {
        match u8::try_from(kind) {
            Ok(CHECK) => "CHECK",
            Ok(TOUCH) => "TOUCH",
            Ok(RESERVE) => "RESERVE",
            Ok(COPY_TO_STORE) => "COPY_TO_STORE",
            Ok(COMMIT_STORE) => "COMMIT_STORE",
            Ok(ABORT_STORE) => "ABORT_STORE",
            Ok(LOOKUP) => "LOOKUP",
            Ok(TAKE_EVENTS) => "TAKE_EVENTS",
            _ => "other",
        }
    }
}

/// `SubmitTurn` flag bits.
pub mod submit_flags {
    /// Poll for cache events after the turn. The plan decides how often, so this stays
    /// with the generator rather than becoming an agent policy.
    pub const POLL_EVENTS: u16 = 1 << 0;
}

/// What went wrong decoding a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The body was shorter than the fields it declared.
    Truncated {
        /// Bytes wanted.
        need: usize,
        /// Bytes available.
        have: usize,
    },
    /// `len` exceeded the configured maximum. The connection must close.
    TooLarge {
        /// The declared length.
        len: u32,
        /// The configured maximum.
        max: u32,
    },
    /// An opcode this build does not know.
    UnknownOpcode(u16),
    /// A field held a value the protocol does not allow.
    Invalid(&'static str),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { need, have } => {
                write!(f, "truncated frame: need {need} bytes, have {have}")
            }
            Self::TooLarge { len, max } => write!(
                f,
                "frame declares {len} bytes, above the {max}-byte maximum; closing rather \
                 than allocating"
            ),
            Self::UnknownOpcode(op) => write!(f, "unknown opcode {op}"),
            Self::Invalid(what) => write!(f, "invalid frame: {what}"),
        }
    }
}

impl std::error::Error for WireError {}

/// A frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Body length, excluding this header.
    pub len: u32,
    /// What the frame is.
    pub opcode: u16,
    /// Opcode-specific flags.
    pub flags: u16,
    /// Correlation id, echoed in the reply so a pipelined response can be matched.
    pub corr: u32,
}

impl Header {
    /// Encode into 12 bytes.
    pub fn encode(&self) -> [u8; HEADER_BYTES] {
        let mut b = [0u8; HEADER_BYTES];
        b[0..4].copy_from_slice(&self.len.to_le_bytes());
        b[4..6].copy_from_slice(&self.opcode.to_le_bytes());
        b[6..8].copy_from_slice(&self.flags.to_le_bytes());
        b[8..12].copy_from_slice(&self.corr.to_le_bytes());
        b
    }

    /// Decode from 12 bytes, refusing a body longer than `max_body`.
    ///
    /// # Errors
    ///
    /// [`WireError::Truncated`] if fewer than [`HEADER_BYTES`] are available, or
    /// [`WireError::TooLarge`] if `len` exceeds `max_body` — checked **before** any
    /// allocation is sized from it.
    pub fn decode(bytes: &[u8], max_body: u32) -> Result<Self, WireError> {
        if bytes.len() < HEADER_BYTES {
            return Err(WireError::Truncated {
                need: HEADER_BYTES,
                have: bytes.len(),
            });
        }
        let len = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes"));
        if len > max_body {
            return Err(WireError::TooLarge { len, max: max_body });
        }
        Ok(Self {
            len,
            opcode: u16::from_le_bytes(bytes[4..6].try_into().expect("2 bytes")),
            flags: u16::from_le_bytes(bytes[6..8].try_into().expect("2 bytes")),
            corr: u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes")),
        })
    }
}

/// A cursor that fails rather than reading past the end.
#[derive(Debug)]
pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// A reader over one frame body.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self
            .at
            .checked_add(n)
            .ok_or(WireError::Invalid("length overflow"))?;
        if end > self.bytes.len() {
            return Err(WireError::Truncated {
                need: end,
                have: self.bytes.len(),
            });
        }
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    /// Read a `u8`.
    ///
    /// # Errors
    ///
    /// If the body is exhausted.
    pub fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    /// Read a little-endian `u16`.
    ///
    /// # Errors
    ///
    /// If the body is exhausted.
    pub fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    /// Read a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// If the body is exhausted.
    pub fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    /// Read a little-endian `u64`.
    ///
    /// # Errors
    ///
    /// If the body is exhausted.
    pub fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }

    /// Read `n` bytes.
    ///
    /// # Errors
    ///
    /// If fewer than `n` remain.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        self.take(n)
    }

    /// Read a `build_id`.
    ///
    /// # Errors
    ///
    /// If fewer than [`BUILD_ID_BYTES`] remain.
    pub fn build_id(&mut self) -> Result<[u8; BUILD_ID_BYTES], WireError> {
        let b = self.take(BUILD_ID_BYTES)?;
        let mut out = [0u8; BUILD_ID_BYTES];
        out.copy_from_slice(b);
        Ok(out)
    }

    /// Bytes not yet consumed.
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
}

/// A growable frame body.
#[derive(Debug, Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// An empty body.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty body with room for `n` bytes.
    pub fn with_capacity(n: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(n),
        }
    }

    /// Append a `u8`.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes.push(v);
        self
    }

    /// Append a little-endian `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a little-endian `u32`.
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append a little-endian `u64`.
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.bytes.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Append raw bytes.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(v);
        self
    }

    /// The body written so far.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    /// Take the body.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Prefix the body with a header for `opcode` and return the whole frame.
    ///
    /// # Panics
    ///
    /// If the body exceeds `u32::MAX`, which no legitimate frame approaches.
    pub fn frame(self, opcode: u16, flags: u16, corr: u32) -> Vec<u8> {
        let len = u32::try_from(self.bytes.len()).expect("a body under 4 GiB");
        let header = Header {
            len,
            opcode,
            flags,
            corr,
        };
        let mut out = Vec::with_capacity(HEADER_BYTES + self.bytes.len());
        out.extend_from_slice(&header.encode());
        out.extend_from_slice(&self.bytes);
        out
    }
}

/// `Hello`, the mandatory first frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    /// The sender's protocol version. Compared exactly.
    pub proto_version: u32,
    /// Digest of the sender's binary. Compared exactly; see the contract on why a stale
    /// remote binary has already invalidated measurements in this repository.
    pub build_id: [u8; BUILD_ID_BYTES],
    /// The mailbox path the agent should attach to.
    pub mailbox: String,
}

impl Hello {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(4 + BUILD_ID_BYTES + 2 + self.mailbox.len());
        w.u32(self.proto_version)
            .bytes(&self.build_id)
            .u16(u16::try_from(self.mailbox.len()).unwrap_or(u16::MAX))
            .bytes(self.mailbox.as_bytes());
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated, or the mailbox path is not UTF-8.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        let proto_version = r.u32()?;
        let build_id = r.build_id()?;
        let n = r.u16()? as usize;
        let mailbox = std::str::from_utf8(r.bytes(n)?)
            .map_err(|_| WireError::Invalid("mailbox path is not UTF-8"))?
            .to_string();
        Ok(Self {
            proto_version,
            build_id,
            mailbox,
        })
    }
}

/// `Hello`'s reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloAck {
    /// The agent's protocol version.
    pub proto_version: u32,
    /// The agent's build digest.
    pub build_id: [u8; BUILD_ID_BYTES],
    /// Mailbox channels the node actually has, so the generator can check its lane count:
    /// the mailbox is depth-1 per channel and over-subscription serialises silently.
    pub channels: u32,
    /// Bytes per block, so a description's geometry can be checked against the node's.
    pub block_bytes: u32,
    /// Zero on acceptance.
    pub status: u16,
}

impl HelloAck {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(4 + BUILD_ID_BYTES + 4 + 4 + 2);
        w.u32(self.proto_version)
            .bytes(&self.build_id)
            .u32(self.channels)
            .u32(self.block_bytes)
            .u16(self.status);
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        Ok(Self {
            proto_version: r.u32()?,
            build_id: r.build_id()?,
            channels: r.u32()?,
            block_bytes: r.u32()?,
            status: r.u16()?,
        })
    }
}

/// `SubmitTurn`: one turn's key path.
///
/// The path runs from the **root** of the session's prefix through the end of its new
/// growth (FR-072a). The agent checks it, loads what is resident and stores what is
/// absent; the generator decides none of that, and decides all of the workload.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubmitTurn {
    /// The session whose turn this is. Needed for `RESERVE`, which is per-session.
    pub session: u64,
    /// [`submit_flags`].
    pub flags: u16,
    /// The key path, in order.
    pub path: Vec<u64>,
}

impl SubmitTurn {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(8 + 2 + 4 + self.path.len() * 8);
        w.u64(self.session)
            .u16(self.flags)
            .u32(u32::try_from(self.path.len()).expect("a path under 4 G keys"));
        for key in &self.path {
            w.u64(*key);
        }
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated. The key count is checked against the bytes actually present before
    /// the vector is sized, so a declared count cannot drive an allocation.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        let session = r.u64()?;
        let flags = r.u16()?;
        let n = r.u32()? as usize;
        // Check first: `n` is peer-supplied, and `Vec::with_capacity(n)` before validating
        // it would let a 4-byte field ask for 32 GB.
        if r.remaining() < n * 8 {
            return Err(WireError::Truncated {
                need: n * 8,
                have: r.remaining(),
            });
        }
        let mut path = Vec::with_capacity(n);
        for _ in 0..n {
            path.push(r.u64()?);
        }
        Ok(Self {
            session,
            flags,
            path,
        })
    }

    /// Whether an event poll was requested.
    pub fn polls_events(&self) -> bool {
        self.flags & submit_flags::POLL_EVENTS != 0
    }
}

/// `SubmitTurn`'s reply: what the cache decided, aggregated for one turn.
///
/// The per-key `CHECK` states are **not** returned. The agent acted on them, and sending
/// them would put the reactive decision back on the network for no purpose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TurnOutcome {
    /// Path keys the cache held.
    pub resident: u32,
    /// Path keys another lane was already storing.
    pub pending: u32,
    /// Path keys absent, and so offered for storing.
    pub missing: u32,
    /// Of those, the ones `RESERVE` granted.
    pub granted: u32,
    /// Blocks whose payload came from the cache.
    pub blocks_read: u32,
    /// Blocks whose payload went to it.
    pub blocks_written: u32,
    /// How long the agent took, so the generator can separate agent time from wire time.
    pub elapsed_ns: u64,
}

impl TurnOutcome {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(6 * 4 + 8);
        w.u32(self.resident)
            .u32(self.pending)
            .u32(self.missing)
            .u32(self.granted)
            .u32(self.blocks_read)
            .u32(self.blocks_written)
            .u64(self.elapsed_ns);
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        Ok(Self {
            resident: r.u32()?,
            pending: r.u32()?,
            missing: r.u32()?,
            granted: r.u32()?,
            blocks_read: r.u32()?,
            blocks_written: r.u32()?,
            elapsed_ns: r.u64()?,
        })
    }
}

/// One operation's latency, as a serialized histogram.
///
/// A histogram rather than percentiles, and that is arithmetic rather than taste: the
/// median of two nodes' medians is not the median of their requests. Merging percentiles
/// produces a number belonging to no distribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpHistogram {
    /// Which plan operation kind, in this protocol's own numbering.
    pub op_kind: u8,
    /// Requests recorded, so a count too small to quote can be marked (FR-066a).
    pub requests: u64,
    /// The serialized histogram.
    pub histogram: Vec<u8>,
}

/// What the mailbox-facing code counted.
///
/// # One collector, both paths
///
/// Latency and bandwidth are measured by whatever code talks to the shared-memory
/// mailbox — the generator's own executor on a local node, the agent's on a remote one —
/// and never inferred from the wire. The agent is the only thing near a remote mailbox, so
/// it is the only thing that can time a `LOOKUP` or count a block that moved.
///
/// Because both paths run the *same* executor, these counters mean the same thing wherever
/// they were gathered, which is what makes a local number and a remote number comparable.
/// Had each path counted for itself, a local/remote difference would be unattributable
/// between the cache and the instrument.
///
/// They travel back **for reporting only**. Nothing the generator does depends on them: they
/// do not gate validity, steer submission, or re-enter the workload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    /// Requests issued to the mailbox.
    pub requests: u64,
    /// Key references across those requests.
    pub key_references: u64,
    /// `CHECK` keys reported resident.
    pub check_resident: u64,
    /// `CHECK` keys reported pending — another lane's store in flight.
    pub check_pending: u64,
    /// `CHECK` keys reported absent.
    pub check_miss: u64,
    /// `LOOKUP` keys that returned data.
    pub lookup_hits: u64,
    /// `LOOKUP` keys that did not.
    pub lookup_misses: u64,
    /// Keys `RESERVE` was asked for, so a decline has a denominator.
    pub reserves_attempted: u64,
    /// Keys `RESERVE` declined.
    pub reserves_declined: u64,
    /// Keys `COPY_TO_STORE` was asked for.
    pub transfers_attempted: u64,
    /// Keys it declined.
    pub transfers_declined: u64,
    /// Keys `COMMIT_STORE` was asked for.
    pub commits_attempted: u64,
    /// Keys it declined.
    pub commits_declined: u64,
    /// Loaded blocks whose key stamp did not match the key asked for.
    ///
    /// **Non-zero means Certus returned the wrong block**, which is a correctness failure
    /// rather than a cache outcome — unlike every other counter here. Only meaningful when a
    /// run asked for verification; zero otherwise because nothing was checked.
    pub payload_mismatches: u64,
    /// Loaded blocks whose stamp was checked, so a mismatch count has a denominator.
    pub payloads_verified: u64,
}

impl Counters {
    /// Blocks whose payload came out of the cache.
    ///
    /// **Derived, not stored.** A `LOOKUP` hit is the only thing that brings a payload back,
    /// so this is `lookup_hits` — and it was briefly a field of its own, which a live run
    /// immediately caught reporting zero while the per-turn outcomes said eight. Two
    /// representations of one quantity can disagree; one cannot.
    pub fn blocks_read(&self) -> u64 {
        self.lookup_hits
    }

    /// Blocks whose payload went into the cache.
    ///
    /// An accepted `COPY_TO_STORE` is the only thing that sends one, so a declined transfer
    /// moved nothing. Derived for the same reason as [`Counters::blocks_read`].
    pub fn blocks_written(&self) -> u64 {
        self.transfers_attempted
            .saturating_sub(self.transfers_declined)
    }

    /// Add another node's counters.
    ///
    /// Counters sum exactly, which is why bandwidth can be aggregated across nodes while a
    /// percentile cannot — see [`OpHistogram`].
    pub fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.key_references += other.key_references;
        self.check_resident += other.check_resident;
        self.check_pending += other.check_pending;
        self.check_miss += other.check_miss;
        self.lookup_hits += other.lookup_hits;
        self.lookup_misses += other.lookup_misses;
        self.reserves_attempted += other.reserves_attempted;
        self.reserves_declined += other.reserves_declined;
        self.transfers_attempted += other.transfers_attempted;
        self.transfers_declined += other.transfers_declined;
        self.commits_attempted += other.commits_attempted;
        self.commits_declined += other.commits_declined;
        self.payload_mismatches += other.payload_mismatches;
        self.payloads_verified += other.payloads_verified;
    }

    /// Encode, in declaration order.
    pub fn encode(&self, w: &mut Writer) {
        for v in self.fields() {
            w.u64(v);
        }
    }

    /// Decode, in declaration order.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            requests: r.u64()?,
            key_references: r.u64()?,
            check_resident: r.u64()?,
            check_pending: r.u64()?,
            check_miss: r.u64()?,
            lookup_hits: r.u64()?,
            lookup_misses: r.u64()?,
            reserves_attempted: r.u64()?,
            reserves_declined: r.u64()?,
            transfers_attempted: r.u64()?,
            transfers_declined: r.u64()?,
            commits_attempted: r.u64()?,
            commits_declined: r.u64()?,
            payload_mismatches: r.u64()?,
            payloads_verified: r.u64()?,
        })
    }

    fn fields(&self) -> [u64; 15] {
        [
            self.requests,
            self.key_references,
            self.check_resident,
            self.check_pending,
            self.check_miss,
            self.lookup_hits,
            self.lookup_misses,
            self.reserves_attempted,
            self.reserves_declined,
            self.transfers_attempted,
            self.transfers_declined,
            self.commits_attempted,
            self.commits_declined,
            self.payload_mismatches,
            self.payloads_verified,
        ]
    }
}

/// `Stats`'s reply: what the mailbox-facing code measured, for reporting only.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    /// Counters, which sum across nodes.
    pub counters: Counters,
    /// One entry per operation kind the agent issued. Histograms, which merge; never
    /// percentiles, which do not.
    pub ops: Vec<OpHistogram>,
}

impl Stats {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.counters.encode(&mut w);
        w.u16(u16::try_from(self.ops.len()).expect("few operation kinds"));
        for op in &self.ops {
            w.u8(op.op_kind)
                .u64(op.requests)
                .u32(u32::try_from(op.histogram.len()).expect("a histogram under 4 GiB"))
                .bytes(&op.histogram);
        }
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        let counters = Counters::decode(&mut r)?;
        let n = r.u16()? as usize;
        let mut ops = Vec::with_capacity(n.min(64));
        for _ in 0..n {
            let op_kind = r.u8()?;
            let requests = r.u64()?;
            let len = r.u32()? as usize;
            let histogram = r.bytes(len)?.to_vec();
            ops.push(OpHistogram {
                op_kind,
                requests,
                histogram,
            });
        }
        Ok(Self { counters, ops })
    }
}

/// `Shutdown`'s reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShutdownAck {
    /// Requests the agent issued to its mailbox.
    pub ops_submitted: u64,
    /// Requests that failed.
    pub ops_failed: u64,
}

impl ShutdownAck {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(16);
        w.u64(self.ops_submitted).u64(self.ops_failed);
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        Ok(Self {
            ops_submitted: r.u64()?,
            ops_failed: r.u64()?,
        })
    }
}

/// `ClearCache`'s reply.
///
/// # Why a failure is a field rather than a silent zero
///
/// `--clear-cache` exists so a run starts from a known cache state (FR-046). An agent that
/// could not clear and answered "0 entries" would produce a run that quietly measured a warm
/// cache while its report said the cache had been cleared — a plausible number for a different
/// experiment, which is exactly the failure class the constitution's measurement principles
/// name. So the reply carries the reason, and the generator refuses to run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClearCacheAck {
    /// Entries the clear dropped.
    pub entries: u64,
    /// Empty when the clear succeeded; otherwise why it did not.
    pub error: String,
}

impl ClearCacheAck {
    /// A successful clear of `entries` entries.
    pub fn cleared(entries: u64) -> Self {
        Self {
            entries,
            error: String::new(),
        }
    }

    /// A clear that did not happen, and why.
    pub fn failed(why: impl Into<String>) -> Self {
        Self {
            entries: 0,
            error: why.into(),
        }
    }

    /// Whether the clear happened.
    pub fn is_ok(&self) -> bool {
        self.error.is_empty()
    }

    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::with_capacity(12 + self.error.len());
        w.u64(self.entries)
            .u32(u32::try_from(self.error.len()).unwrap_or(u32::MAX))
            .bytes(self.error.as_bytes());
        w.into_bytes()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated. A reason that is not UTF-8 is replaced rather than refused: losing the
    /// wording of a failure must not turn it into a protocol error, which would report the
    /// wrong cause.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(body);
        let entries = r.u64()?;
        let len = r.u32()? as usize;
        let text = r.bytes(len)?;
        Ok(Self {
            entries,
            error: String::from_utf8_lossy(text).into_owned(),
        })
    }
}

/// `Drain`'s reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DrainAck {
    /// Work still outstanding.
    pub pending: u32,
}

impl DrainAck {
    /// Encode the body.
    pub fn encode(&self) -> Vec<u8> {
        Writer::with_capacity(4)
            .u32(self.pending)
            .as_slice()
            .to_vec()
    }

    /// Decode the body.
    ///
    /// # Errors
    ///
    /// If truncated.
    pub fn decode(body: &[u8]) -> Result<Self, WireError> {
        Ok(Self {
            pending: Reader::new(body).u32()?,
        })
    }
}

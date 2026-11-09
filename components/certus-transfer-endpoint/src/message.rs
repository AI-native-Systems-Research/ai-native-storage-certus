//! Wire codecs: the header, the `OP_FETCH` entry vector, the reply outcomes,
//! and the frozen `OP_HELLO`, `OP_HELLO_REPLY` and `OP_ERROR` messages.
//!
//! Values never travel in a message: they go by one-sided RDMA WRITE. Messages
//! carry only the control plane. See
//! `specs/001-rdma-cache-transfer/contracts/wire-protocol.md`, which this module
//! implements exactly; every layout below is asserted by a test against the
//! offsets that document gives.
//!
//! # Parsing order
//!
//! A receiver parses the [`Prefix`] first — bytes 0-15, frozen in every version
//! of every kind, forever — so that whatever it cannot understand it can still
//! name, refuse with a reason, and correlate. Only then is the rest parsed, and
//! the version is checked once there, against the connection's negotiated one
//! ([`Header::parse`]). The opcode names a kind only; the version is a field
//! (D21), so an unchanged kind keeps its opcode across versions.
//!
//! Everything is little-endian, and every reserved field must be zero on
//! receipt — a non-zero one is a malformed message, not ignored, so that a
//! future use of the field can never be misread by an old receiver.

use interfaces::{CacheKey, EntryOutcome, RemoteRegion};

/// Size of every message's header.
pub(crate) const HEADER_LEN: usize = 24;
/// Size of one `OP_FETCH` entry.
pub(crate) const ENTRY_LEN: usize = 24;
/// Size of a posted receive buffer, and so the largest message.
pub(crate) const MESSAGE_LEN: usize = 8192;
/// Entries one `OP_FETCH` can carry: `(8192 - 24) / 24`.
pub(crate) const MAX_ENTRIES: u32 = ((MESSAGE_LEN - HEADER_LEN) / ENTRY_LEN) as u32;
/// Size of `OP_HELLO`, header included. Frozen.
pub(crate) const HELLO_LEN: usize = 40;
/// Size of `OP_ERROR`, header included. Frozen.
pub(crate) const ERROR_LEN: usize = 32;

/// The protocol versions this build speaks.
pub(crate) const VERSION_MIN: u16 = 1;
pub(crate) const VERSION_MAX: u16 = 1;

const _: () = assert!(MAX_ENTRIES == 340);

/// A message kind. The high 16 bits of the opcode word are zero for every
/// kind that exists; a word with any of them set is an unknown kind.
///
/// The low bit says which side of an exchange a message is: a request's
/// opcode is even, and its success reply is the same opcode with the low bit
/// set. So even an opcode this build does not know is known to be a request,
/// to answer with `OP_ERROR`, or a reply, never to answer ([`is_reply_opcode`]).
/// `OP_ERROR`, the reply to any request, is odd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Fetch,
    FetchReply,
    /// Frozen at version 0; header only, no content, ever.
    HelloReply,
    /// Frozen at version 0.
    Hello,
    /// Frozen at version 0.
    Error,
}

impl Kind {
    pub(crate) const fn opcode(self) -> u32 {
        match self {
            Kind::Fetch => 0x0000_0002,
            Kind::FetchReply => 0x0000_0003,
            Kind::Hello => 0x0000_FFFC,
            Kind::HelloReply => 0x0000_FFFD,
            Kind::Error => 0x0000_FFFF,
        }
    }

    fn from_opcode(op: u32) -> Option<Self> {
        Some(match op {
            0x0000_0002 => Kind::Fetch,
            0x0000_0003 => Kind::FetchReply,
            0x0000_FFFC => Kind::Hello,
            0x0000_FFFD => Kind::HelloReply,
            0x0000_FFFF => Kind::Error,
            _ => return None,
        })
    }

    /// The frozen kinds: always version 0, parseable by every peer.
    pub(crate) fn is_frozen(self) -> bool {
        matches!(self, Kind::Hello | Kind::HelloReply | Kind::Error)
    }

    /// Whether this kind is a reply. Replies are never answered. The receive
    /// path classifies by [`is_reply_opcode`], which covers unknown kinds too.
    #[cfg(test)]
    pub(crate) fn is_reply(self) -> bool {
        is_reply_opcode(self.opcode())
    }
}

/// Whether an opcode word, known or not, is a reply: its low bit is set.
pub(crate) const fn is_reply_opcode(op: u32) -> bool {
    op & 1 == 1
}

/// `OP_ERROR` reasons. The space is open: unknown values are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    UnknownOpcode,
    UnsupportedVersion,
    Malformed,
    TooManyEntries,
    /// 0, or anything >= 5: a generic protocol failure, never success.
    Other(u16),
}

impl Reason {
    pub(crate) fn code(self) -> u16 {
        match self {
            Reason::UnknownOpcode => 1,
            Reason::UnsupportedVersion => 2,
            Reason::Malformed => 3,
            Reason::TooManyEntries => 4,
            Reason::Other(c) => c,
        }
    }

    fn from_code(c: u16) -> Self {
        match c {
            1 => Reason::UnknownOpcode,
            2 => Reason::UnsupportedVersion,
            3 => Reason::Malformed,
            4 => Reason::TooManyEntries,
            other => Reason::Other(other),
        }
    }
}

/// Why a message could not be parsed: the reason to answer with, if it was a
/// request, and to log in any case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Malformed {
    pub(crate) reason: Reason,
    pub(crate) what: &'static str,
}

fn malformed(what: &'static str) -> Malformed {
    Malformed {
        reason: Reason::Malformed,
        what,
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

// --- the frozen prefix ---------------------------------------------------------

/// Bytes 0-15 of every message, in every version: the minimum a receiver
/// needs to refuse a message it cannot understand, say why, and correlate the
/// refusal with the request. Parsing this never depends on the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Prefix {
    /// The whole opcode word as received, for `OP_ERROR::offending_opcode`.
    pub(crate) opcode: u32,
    pub(crate) flags: u16,
    pub(crate) version: u16,
    pub(crate) context_id: u64,
}

impl Prefix {
    pub(crate) const LEN: usize = 16;

    /// The prefix of any buffer at least 16 bytes long. `None` only if it is
    /// shorter — then there is nothing to correlate a refusal with.
    pub(crate) fn parse(b: &[u8]) -> Option<Self> {
        (b.len() >= Self::LEN).then(|| Prefix {
            opcode: u32_at(b, 0),
            flags: u16_at(b, 4),
            version: u16_at(b, 6),
            context_id: u64_at(b, 8),
        })
    }

    pub(crate) fn kind(&self) -> Option<Kind> {
        Kind::from_opcode(self.opcode)
    }
}

// --- the header ----------------------------------------------------------------

/// A parsed, validated 24-byte header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) kind: Kind,
    pub(crate) version: u16,
    pub(crate) context_id: u64,
    pub(crate) n_entries: u32,
}

impl Header {
    pub(crate) fn encode(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.kind.opcode().to_le_bytes());
        out[4..6].copy_from_slice(&0u16.to_le_bytes());
        out[6..8].copy_from_slice(&self.version.to_le_bytes());
        out[8..16].copy_from_slice(&self.context_id.to_le_bytes());
        out[16..20].copy_from_slice(&self.n_entries.to_le_bytes());
        out[20..24].copy_from_slice(&0u32.to_le_bytes());
    }

    /// Parse and validate a header against the connection's negotiated
    /// `version` — checked **once**, here, never dispatched on (D21). The
    /// frozen kinds must carry version 0 and are accepted whatever was
    /// negotiated, since they are what a peer must parse before negotiating.
    pub(crate) fn parse(b: &[u8], negotiated: u16) -> Result<Self, Malformed> {
        if b.len() < HEADER_LEN {
            return Err(malformed("shorter than a header"));
        }
        let p = Prefix::parse(b).expect("length checked");
        let Some(kind) = p.kind() else {
            return Err(Malformed {
                reason: Reason::UnknownOpcode,
                what: "unknown opcode",
            });
        };
        if p.flags != 0 {
            return Err(malformed("reserved flags set"));
        }
        if u32_at(b, 20) != 0 {
            return Err(malformed("reserved header field set"));
        }
        let expected = if kind.is_frozen() { 0 } else { negotiated };
        if p.version != expected {
            return Err(Malformed {
                reason: Reason::UnsupportedVersion,
                what: "version is not the one negotiated for this connection",
            });
        }
        Ok(Header {
            kind,
            version: p.version,
            context_id: p.context_id,
            n_entries: u32_at(b, 16),
        })
    }
}

// --- OP_FETCH ------------------------------------------------------------------

/// Encode an `OP_FETCH` for `entries` into `out`, returning its length.
/// The trailing `(size, rkey)` pair is written by name: `RemoteRegion` holds
/// them in the opposite order, and both are `u32`, so a wholesale copy would
/// silently swap a length for a key.
pub(crate) fn encode_fetch(
    version: u16,
    context_id: u64,
    entries: &[(CacheKey, RemoteRegion)],
    out: &mut [u8],
) -> usize {
    assert!(
        entries.len() <= MAX_ENTRIES as usize,
        "chunk before encoding"
    );
    Header {
        kind: Kind::Fetch,
        version,
        context_id,
        n_entries: entries.len() as u32,
    }
    .encode(out);
    for (i, (key, r)) in entries.iter().enumerate() {
        let e = &mut out[HEADER_LEN + i * ENTRY_LEN..][..ENTRY_LEN];
        e[0..8].copy_from_slice(&key.to_le_bytes());
        e[8..16].copy_from_slice(&r.addr.to_le_bytes());
        e[16..20].copy_from_slice(&r.length.to_le_bytes()); // size
        e[20..24].copy_from_slice(&r.rkey.to_le_bytes());
    }
    HEADER_LEN + entries.len() * ENTRY_LEN
}

/// The entries of an `OP_FETCH` whose header has been parsed. `msg` is the
/// whole message as received; its length must match `n_entries` exactly.
pub(crate) fn decode_fetch(
    h: &Header,
    msg: &[u8],
) -> Result<Vec<(CacheKey, RemoteRegion)>, Malformed> {
    if h.n_entries > MAX_ENTRIES {
        return Err(Malformed {
            reason: Reason::TooManyEntries,
            what: "n_entries exceeds the posted buffer",
        });
    }
    let n = h.n_entries as usize;
    if msg.len() != HEADER_LEN + n * ENTRY_LEN {
        return Err(malformed("length does not match n_entries"));
    }
    Ok((0..n)
        .map(|i| {
            let e = &msg[HEADER_LEN + i * ENTRY_LEN..][..ENTRY_LEN];
            (
                u64_at(e, 0),
                RemoteRegion {
                    addr: u64_at(e, 8),
                    length: u32_at(e, 16), // size
                    rkey: u32_at(e, 20),
                },
            )
        })
        .collect())
}

// --- OP_FETCH_REPLY ------------------------------------------------------------

fn outcome_byte(o: EntryOutcome) -> u8 {
    match o {
        EntryOutcome::Miss => 0,
        EntryOutcome::HitMemoryTier => 1,
        EntryOutcome::HitBlockTier => 2,
        EntryOutcome::Failed => 3,
    }
}

fn byte_outcome(b: u8) -> EntryOutcome {
    match b {
        1 => EntryOutcome::HitMemoryTier,
        2 => EntryOutcome::HitBlockTier,
        3 => EntryOutcome::Failed,
        // 0, and anything a future source adds: a miss, never an error.
        _ => EntryOutcome::Miss,
    }
}

fn padded(n: usize) -> usize {
    n.div_ceil(8) * 8
}

/// Encode an `OP_FETCH_REPLY`: one outcome byte per entry, in request order,
/// zero-padded to an 8-byte boundary. Returns its length.
pub(crate) fn encode_reply(
    version: u16,
    context_id: u64,
    outcomes: &[EntryOutcome],
    out: &mut [u8],
) -> usize {
    Header {
        kind: Kind::FetchReply,
        version,
        context_id,
        n_entries: outcomes.len() as u32,
    }
    .encode(out);
    let body = &mut out[HEADER_LEN..HEADER_LEN + padded(outcomes.len())];
    body.fill(0);
    for (b, o) in body.iter_mut().zip(outcomes) {
        *b = outcome_byte(*o);
    }
    HEADER_LEN + padded(outcomes.len())
}

/// The outcomes of an `OP_FETCH_REPLY` whose header has been parsed.
pub(crate) fn decode_reply(h: &Header, msg: &[u8]) -> Result<Vec<EntryOutcome>, Malformed> {
    let n = h.n_entries as usize;
    if n > MAX_ENTRIES as usize || msg.len() != HEADER_LEN + padded(n) {
        return Err(malformed("length does not match n_entries"));
    }
    let body = &msg[HEADER_LEN..];
    if body[n..].iter().any(|b| *b != 0) {
        return Err(malformed("non-zero padding"));
    }
    Ok(body[..n].iter().map(|b| byte_outcome(*b)).collect())
}

// --- OP_HELLO and OP_HELLO_REPLY (frozen) --------------------------------------

/// What `OP_HELLO` advertises.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hello {
    /// Largest `OP_FETCH` the sender can receive.
    pub(crate) max_entries: u32,
    /// How many requests the **peer** may have outstanding against the sender:
    /// the sender's inbound-request quota.
    pub(crate) max_outstanding_requests: u32,
    pub(crate) version_min: u16,
    pub(crate) version_max: u16,
}

pub(crate) fn encode_hello(h: &Hello, out: &mut [u8]) -> usize {
    Header {
        kind: Kind::Hello,
        version: 0,
        context_id: 0,
        n_entries: 0,
    }
    .encode(out);
    out[24..28].copy_from_slice(&h.max_entries.to_le_bytes());
    out[28..32].copy_from_slice(&h.max_outstanding_requests.to_le_bytes());
    out[32..34].copy_from_slice(&h.version_min.to_le_bytes());
    out[34..36].copy_from_slice(&h.version_max.to_le_bytes());
    out[36..40].copy_from_slice(&0u32.to_le_bytes());
    HELLO_LEN
}

/// An `OP_HELLO` whose header has been parsed. Its layout is frozen, so an
/// exact length is required — a longer one is not a future extension, it is
/// malformed: `OP_HELLO` extends by content, never by layout.
pub(crate) fn decode_hello(h: &Header, msg: &[u8]) -> Result<Hello, Malformed> {
    if h.context_id != 0 || h.n_entries != 0 {
        return Err(malformed("OP_HELLO with a context id or entries"));
    }
    if msg.len() != HELLO_LEN {
        return Err(malformed("OP_HELLO is not 40 bytes"));
    }
    if u32_at(msg, 36) != 0 {
        return Err(malformed("OP_HELLO reserved field set"));
    }
    Ok(Hello {
        max_entries: u32_at(msg, 24),
        max_outstanding_requests: u32_at(msg, 28),
        version_min: u16_at(msg, 32),
        version_max: u16_at(msg, 34),
    })
}

/// `OP_HELLO_REPLY`: a header and nothing else, ever. It exists only so every
/// request draws one reply and the credits balance (D20).
pub(crate) fn encode_hello_reply(out: &mut [u8]) -> usize {
    Header {
        kind: Kind::HelloReply,
        version: 0,
        context_id: 0,
        n_entries: 0,
    }
    .encode(out);
    HEADER_LEN
}

pub(crate) fn decode_hello_reply(h: &Header, msg: &[u8]) -> Result<(), Malformed> {
    if h.context_id != 0 || h.n_entries != 0 || msg.len() != HEADER_LEN {
        return Err(malformed("OP_HELLO_REPLY carries content"));
    }
    Ok(())
}

// --- OP_ERROR (frozen) ---------------------------------------------------------

/// What `OP_ERROR` carries: the refused request's context, what it was, why,
/// and the highest version the refuser speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ErrorReply {
    pub(crate) context_id: u64,
    pub(crate) offending_opcode: u32,
    pub(crate) reason: Reason,
    /// 0 if the offending kind is unknown entirely.
    pub(crate) max_supported_version: u16,
}

pub(crate) fn encode_error(e: &ErrorReply, out: &mut [u8]) -> usize {
    Header {
        kind: Kind::Error,
        version: 0,
        context_id: e.context_id,
        n_entries: 0,
    }
    .encode(out);
    out[24..28].copy_from_slice(&e.offending_opcode.to_le_bytes());
    out[28..30].copy_from_slice(&e.reason.code().to_le_bytes());
    out[30..32].copy_from_slice(&e.max_supported_version.to_le_bytes());
    ERROR_LEN
}

pub(crate) fn decode_error(h: &Header, msg: &[u8]) -> Result<ErrorReply, Malformed> {
    if h.n_entries != 0 || msg.len() != ERROR_LEN {
        return Err(malformed("OP_ERROR is not 32 bytes"));
    }
    Ok(ErrorReply {
        context_id: h.context_id,
        offending_opcode: u32_at(msg, 24),
        reason: Reason::from_code(u16_at(msg, 28)),
        max_supported_version: u16_at(msg, 30),
    })
}

/// The `OP_ERROR` answering an unparseable request. `None` when the bytes do
/// not even hold a prefix, or are a reply — of a known kind or not, by the
/// opcode's low bit: replies are never answered.
pub(crate) fn refusal(msg: &[u8], why: Malformed) -> Option<ErrorReply> {
    let p = Prefix::parse(msg)?;
    if is_reply_opcode(p.opcode) {
        return None;
    }
    Some(ErrorReply {
        context_id: p.context_id,
        offending_opcode: p.opcode,
        reason: why.reason,
        max_supported_version: if p.kind().is_some() { VERSION_MAX } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(addr: u64, length: u32, rkey: u32) -> RemoteRegion {
        RemoteRegion { addr, rkey, length }
    }

    /// The frozen prefix sits where the contract says, in every kind.
    #[test]
    fn header_layout_matches_the_contract() {
        let mut b = [0xAAu8; HEADER_LEN];
        Header {
            kind: Kind::Fetch,
            version: 0x0102,
            context_id: 0x0807_0605_0403_0201,
            n_entries: 0x0A0B_0C0D,
        }
        .encode(&mut b);
        assert_eq!(&b[0..4], &[2, 0, 0, 0], "opcode, little-endian");
        assert_eq!(&b[4..6], &[0, 0], "flags");
        assert_eq!(&b[6..8], &[0x02, 0x01], "version at offset 6");
        assert_eq!(&b[8..16], &[1, 2, 3, 4, 5, 6, 7, 8], "context_id");
        assert_eq!(&b[16..20], &[0x0D, 0x0C, 0x0B, 0x0A], "n_entries");
        assert_eq!(&b[20..24], &[0, 0, 0, 0], "reserved");
        let p = Prefix::parse(&b).unwrap();
        assert_eq!(
            (p.opcode, p.version, p.context_id),
            (2, 0x0102, 0x0807_0605_0403_0201)
        );
    }

    #[test]
    fn opcodes_name_kinds_with_the_high_half_zero() {
        for k in [
            Kind::Fetch,
            Kind::FetchReply,
            Kind::HelloReply,
            Kind::Hello,
            Kind::Error,
        ] {
            assert_eq!(k.opcode() >> 16, 0, "{k:?}");
            assert_eq!(Kind::from_opcode(k.opcode()), Some(k));
        }
        assert_eq!(Kind::from_opcode(0x0001_0002), None, "high half set");
        assert_eq!(Kind::from_opcode(4), None);
        // The numbers are frozen: they are on the wire.
        assert_eq!(
            [
                Kind::Fetch,
                Kind::FetchReply,
                Kind::Hello,
                Kind::HelloReply,
                Kind::Error
            ]
            .map(Kind::opcode),
            [0x0002, 0x0003, 0xFFFC, 0xFFFD, 0xFFFF]
        );
    }

    /// A reply is its request's opcode with the low bit set, and every reply,
    /// `OP_ERROR` included, is odd.
    #[test]
    fn the_low_bit_marks_a_reply() {
        assert_eq!(Kind::FetchReply.opcode(), Kind::Fetch.opcode() | 1);
        assert_eq!(Kind::HelloReply.opcode(), Kind::Hello.opcode() | 1);
        for k in [Kind::FetchReply, Kind::HelloReply, Kind::Error] {
            assert!(k.is_reply(), "{k:?}");
        }
        for k in [Kind::Fetch, Kind::Hello] {
            assert!(!k.is_reply(), "{k:?}");
        }
        assert!(
            is_reply_opcode(0x41) && !is_reply_opcode(0x42),
            "unknown too"
        );
    }

    #[test]
    fn version_is_checked_once_against_the_negotiated_one() {
        let mut b = [0u8; HEADER_LEN];
        let h = |kind, version| Header {
            kind,
            version,
            context_id: 9,
            n_entries: 0,
        };
        h(Kind::Fetch, 1).encode(&mut b);
        assert!(Header::parse(&b, 1).is_ok());
        let e = Header::parse(&b, 2).unwrap_err();
        assert_eq!(e.reason, Reason::UnsupportedVersion);
        // Frozen kinds are version 0, whatever was negotiated.
        h(Kind::Hello, 0).encode(&mut b);
        assert!(Header::parse(&b, 1).is_ok());
        h(Kind::Hello, 1).encode(&mut b);
        assert_eq!(
            Header::parse(&b, 1).unwrap_err().reason,
            Reason::UnsupportedVersion
        );
    }

    #[test]
    fn reserved_fields_and_unknown_opcodes_are_refused() {
        let mut b = [0u8; HEADER_LEN];
        Header {
            kind: Kind::Fetch,
            version: 1,
            context_id: 0,
            n_entries: 0,
        }
        .encode(&mut b);
        let mut flags = b;
        flags[4] = 1;
        assert_eq!(
            Header::parse(&flags, 1).unwrap_err().reason,
            Reason::Malformed
        );
        let mut reserved = b;
        reserved[23] = 1;
        assert_eq!(
            Header::parse(&reserved, 1).unwrap_err().reason,
            Reason::Malformed
        );
        let mut unknown = b;
        unknown[0] = 7;
        assert_eq!(
            Header::parse(&unknown, 1).unwrap_err().reason,
            Reason::UnknownOpcode
        );
        assert!(Header::parse(&b[..23], 1).is_err());
    }

    #[test]
    fn fetch_round_trips_and_names_size_before_rkey() {
        let entries = [
            (11, region(0x1000, 64, 0xAA)),
            (u64::MAX, region(u64::MAX, u32::MAX, 1)),
        ];
        let mut b = [0u8; MESSAGE_LEN];
        let n = encode_fetch(1, 5, &entries, &mut b);
        assert_eq!(n, HEADER_LEN + 2 * ENTRY_LEN);
        // The wire puts size at 16 and rkey at 20 — the reverse of the struct.
        let e0 = &b[HEADER_LEN..HEADER_LEN + ENTRY_LEN];
        assert_eq!(u32_at(e0, 16), 64, "size");
        assert_eq!(u32_at(e0, 20), 0xAA, "rkey");
        let h = Header::parse(&b[..n], 1).unwrap();
        assert_eq!((h.kind, h.context_id, h.n_entries), (Kind::Fetch, 5, 2));
        assert_eq!(decode_fetch(&h, &b[..n]).unwrap(), entries);
    }

    #[test]
    fn a_full_fetch_fits_one_buffer_and_one_more_does_not() {
        let entries = vec![(1, region(0, 1, 1)); MAX_ENTRIES as usize];
        let mut b = [0u8; MESSAGE_LEN];
        let n = encode_fetch(1, 0, &entries, &mut b);
        assert!(n <= MESSAGE_LEN);
        let mut h = Header::parse(&b[..n], 1).unwrap();
        assert_eq!(decode_fetch(&h, &b[..n]).unwrap().len(), 340);
        h.n_entries = MAX_ENTRIES + 1;
        assert_eq!(
            decode_fetch(&h, &b[..n]).unwrap_err().reason,
            Reason::TooManyEntries
        );
    }

    #[test]
    fn fetch_length_must_match_n_entries() {
        let mut b = [0u8; MESSAGE_LEN];
        let n = encode_fetch(1, 0, &[(1, region(0, 1, 1))], &mut b);
        let h = Header::parse(&b[..n], 1).unwrap();
        assert!(decode_fetch(&h, &b[..n - 1]).is_err(), "short");
        assert!(decode_fetch(&h, &b[..n + 1]).is_err(), "long");
    }

    #[test]
    fn reply_round_trips_pads_and_reads_unknown_outcomes_as_miss() {
        use EntryOutcome::*;
        let outcomes = [HitMemoryTier, Miss, HitBlockTier, Failed, Miss];
        let mut b = [0xFFu8; MESSAGE_LEN];
        let n = encode_reply(1, 3, &outcomes, &mut b);
        assert_eq!(n, HEADER_LEN + 8, "five bytes padded to eight");
        assert_eq!(&b[HEADER_LEN..n], &[1, 0, 2, 3, 0, 0, 0, 0]);
        let h = Header::parse(&b[..n], 1).unwrap();
        assert_eq!(decode_reply(&h, &b[..n]).unwrap(), outcomes);
        // A future source's outcome 7 reads as a miss.
        b[HEADER_LEN] = 7;
        assert_eq!(decode_reply(&h, &b[..n]).unwrap()[0], Miss);
        b[HEADER_LEN + 6] = 1;
        assert!(decode_reply(&h, &b[..n]).is_err(), "padding must be zero");
    }

    /// The frozen messages' sizes and offsets: what a later change would
    /// silently break.
    #[test]
    fn hello_layout_is_frozen() {
        let hello = Hello {
            max_entries: 340,
            max_outstanding_requests: 8,
            version_min: 1,
            version_max: 3,
        };
        let mut b = [0u8; MESSAGE_LEN];
        assert_eq!(encode_hello(&hello, &mut b), 40);
        assert_eq!(u32_at(&b, 0), 0x0000_FFFC);
        assert_eq!(u16_at(&b, 6), 0, "version 0, forever");
        assert_eq!(
            (
                u32_at(&b, 24),
                u32_at(&b, 28),
                u16_at(&b, 32),
                u16_at(&b, 34)
            ),
            (340, 8, 1, 3)
        );
        let h = Header::parse(&b[..40], 1).unwrap();
        assert_eq!(decode_hello(&h, &b[..40]).unwrap(), hello);
        assert!(
            decode_hello(&h, &b[..41]).is_err(),
            "extends by content, not layout"
        );
        b[37] = 1;
        assert!(decode_hello(&h, &b[..40]).is_err(), "reserved");
    }

    #[test]
    fn hello_reply_is_a_bare_header() {
        let mut b = [0u8; MESSAGE_LEN];
        assert_eq!(encode_hello_reply(&mut b), 24);
        assert_eq!(u32_at(&b, 0), 0x0000_FFFD);
        let h = Header::parse(&b[..24], 1).unwrap();
        assert!(decode_hello_reply(&h, &b[..24]).is_ok());
        assert!(
            decode_hello_reply(&h, &b[..25]).is_err(),
            "no content, ever"
        );
    }

    #[test]
    fn error_layout_is_frozen_and_reasons_stay_open() {
        let e = ErrorReply {
            context_id: 42,
            offending_opcode: 0x1234_5678,
            reason: Reason::TooManyEntries,
            max_supported_version: 1,
        };
        let mut b = [0u8; MESSAGE_LEN];
        assert_eq!(encode_error(&e, &mut b), 32);
        assert_eq!(u32_at(&b, 0), 0x0000_FFFF);
        assert_eq!(u64_at(&b, 8), 42, "echoes the refused context");
        assert_eq!(
            (u32_at(&b, 24), u16_at(&b, 28), u16_at(&b, 30)),
            (0x1234_5678, 4, 1)
        );
        let h = Header::parse(&b[..32], 1).unwrap();
        assert_eq!(decode_error(&h, &b[..32]).unwrap(), e);
        // An unknown reason is kept, never read as success.
        b[28] = 9;
        assert_eq!(decode_error(&h, &b[..32]).unwrap().reason, Reason::Other(9));
        b[28] = 0;
        assert_eq!(decode_error(&h, &b[..32]).unwrap().reason, Reason::Other(0));
    }

    /// A refusal names what was refused, and replies are never answered.
    #[test]
    fn refusals_answer_requests_only() {
        let mut b = [0u8; HEADER_LEN];
        Header {
            kind: Kind::Fetch,
            version: 1,
            context_id: 77,
            n_entries: 0,
        }
        .encode(&mut b);
        b[0] = 8; // an unknown kind, even: a request
        let why = Header::parse(&b, 1).unwrap_err();
        let r = refusal(&b, why).unwrap();
        assert_eq!(
            (
                r.context_id,
                r.offending_opcode,
                r.reason,
                r.max_supported_version
            ),
            (77, 8, Reason::UnknownOpcode, 0)
        );
        b[0] = 9; // an unknown kind, odd: a reply, never answered
        assert_eq!(refusal(&b, Header::parse(&b, 1).unwrap_err()), None);
        for reply in [Kind::FetchReply, Kind::HelloReply, Kind::Error] {
            b[0..4].copy_from_slice(&reply.opcode().to_le_bytes());
            assert_eq!(
                refusal(&b, malformed("x")),
                None,
                "{reply:?} is not answered"
            );
        }
        assert_eq!(refusal(&b[..15], malformed("x")), None, "no prefix");
    }
}

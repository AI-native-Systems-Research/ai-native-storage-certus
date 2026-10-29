//! The connect-time identity carried in `rdma_cm` private data (research D24).
//!
//! An inbound link is filed under the initiator's [`PeerId`], which the
//! connection itself cannot supply — the source address carries an ephemeral
//! port, not an instance — so the initiator names itself in the connect
//! private data. The acceptor echoes its own name in the accept private data so
//! a naming mismatch fails loudly instead of yielding two connections, and a
//! reject carries a reason so the simultaneous-connect loser (D23) can tell a
//! lost tie-break from a refusal.
//!
//! These bytes are exchanged **before** `OP_HELLO`, so — like `OP_HELLO` and
//! `OP_ERROR` (D20) — their layout is frozen forever. Each starts with a magic
//! and a format byte; a new format is a new value of that byte, never a change
//! to format 1. Received private data is zero-padded to the transport's limit,
//! so the identity carries an explicit length.
//!
//! ```text
//! identity (connect, accept):  'C' 'T' 0x01 len  <len bytes: normalized PeerId, UTF-8>
//! reject:                      'C' 'T' 0x01 reason
//! ```

use super::transport::MAX_CONNECT_PRIVATE_DATA;
use super::PeerId;

const MAGIC: [u8; 2] = *b"CT";
const FORMAT_1: u8 = 1;
const HEADER: usize = 4;

/// The longest normalized [`PeerId`] that fits a connect request.
pub const MAX_IDENTITY_LEN: usize = MAX_CONNECT_PRIVATE_DATA - HEADER;

/// Why an inbound connect was refused. Wire values are frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// Simultaneous connect: the other connection has the lower-`PeerId`
    /// initiator and survives (D23). Not a failure — wait for that link.
    Duplicate,
    /// The request's identity was absent, unparseable, or named the acceptor.
    MalformedIdentity,
    /// The acceptor is still tearing down a connection to this peer, and
    /// FR-023 permits no fresh one until that finishes. Retry on next need.
    Draining,
    /// A reason this build does not know, kept rather than guessed.
    Unknown(u8),
}

impl RejectReason {
    fn code(self) -> u8 {
        match self {
            RejectReason::Duplicate => 1,
            RejectReason::MalformedIdentity => 2,
            RejectReason::Draining => 3,
            RejectReason::Unknown(c) => c,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            1 => RejectReason::Duplicate,
            2 => RejectReason::MalformedIdentity,
            3 => RejectReason::Draining,
            other => RejectReason::Unknown(other),
        }
    }
}

/// Encode `id` for connect or accept private data. `None` if it is longer
/// than [`MAX_IDENTITY_LEN`]; the table checks its own name at construction.
pub fn encode_identity(id: &PeerId) -> Option<Vec<u8>> {
    let name = id.to_string();
    if name.len() > MAX_IDENTITY_LEN {
        return None;
    }
    let mut v = Vec::with_capacity(HEADER + name.len());
    v.extend_from_slice(&MAGIC);
    v.push(FORMAT_1);
    v.push(name.len() as u8);
    v.extend_from_slice(name.as_bytes());
    Some(v)
}

/// Decode an identity, ignoring zero padding. `None` if it is not format 1 or
/// does not hold a valid [`PeerId`].
pub fn decode_identity(data: &[u8]) -> Option<PeerId> {
    let (head, rest) = data.split_at(HEADER.min(data.len()));
    if head.len() < HEADER || head[..2] != MAGIC || head[2] != FORMAT_1 {
        return None;
    }
    let name = rest.get(..head[3] as usize)?;
    PeerId::parse(std::str::from_utf8(name).ok()?).ok()
}

pub fn encode_reject(reason: RejectReason) -> Vec<u8> {
    vec![MAGIC[0], MAGIC[1], FORMAT_1, reason.code()]
}

/// Decode a reject reason. `None` if the reject carried no certus reason —
/// a reject from something that is not this endpoint, or by the CM itself.
pub fn decode_reject(data: &[u8]) -> Option<RejectReason> {
    match data {
        [m0, m1, FORMAT_1, code, ..] if [*m0, *m1] == MAGIC => Some(RejectReason::from_code(*code)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(s: &str) -> PeerId {
        PeerId::parse(s).unwrap()
    }

    #[test]
    fn identity_round_trips_through_zero_padding() {
        let id = peer("10.0.0.102:4791");
        let mut wire = encode_identity(&id).unwrap();
        assert_eq!(&wire[..4], b"CT\x01\x0f");
        wire.resize(MAX_CONNECT_PRIVATE_DATA, 0);
        assert_eq!(decode_identity(&wire), Some(id));
    }

    #[test]
    fn identity_length_limit() {
        let fits = format!("{}:1", "h".repeat(MAX_IDENTITY_LEN - 2));
        assert!(encode_identity(&peer(&fits)).is_some());
        let over = format!("{}:1", "h".repeat(MAX_IDENTITY_LEN - 1));
        assert!(encode_identity(&peer(&over)).is_none());
    }

    #[test]
    fn malformed_identities_rejected() {
        assert_eq!(decode_identity(&[]), None);
        assert_eq!(decode_identity(&[0; 56]), None, "all padding");
        assert_eq!(decode_identity(b"CT\x02\x03a:1"), None, "unknown format");
        assert_eq!(
            decode_identity(b"CT\x01\x09a:1"),
            None,
            "length past the data"
        );
        assert_eq!(decode_identity(b"CT\x01\x01a"), None, "no port");
        assert_eq!(decode_identity(b"XX\x01\x03a:1"), None, "bad magic");
        assert_eq!(decode_identity(b"CT\x01\x02\xff\xfe"), None, "not UTF-8");
    }

    #[test]
    fn reject_reasons_round_trip_and_unknown_is_kept() {
        for r in [
            RejectReason::Duplicate,
            RejectReason::MalformedIdentity,
            RejectReason::Draining,
            RejectReason::Unknown(200),
        ] {
            let mut wire = encode_reject(r);
            wire.resize(148, 0);
            assert_eq!(decode_reject(&wire), Some(r));
        }
        assert_eq!(decode_reject(&[0; 148]), None, "not ours");
    }
}

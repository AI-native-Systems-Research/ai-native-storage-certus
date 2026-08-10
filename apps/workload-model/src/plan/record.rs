//! The fixed-width `events.bin` record.
//!
//! 40 bytes, little-endian, every field naturally aligned and the record size a
//! multiple of 8 — so an array of records keeps its `u64` fields aligned and a
//! decoder needs no packed intermediate. Fixed width is what makes the file
//! memory-mappable and indexable by ordinal, which is in turn what keeps event
//! fetch allocation-free on the issuing path (spec FR-037).
//!
//! The layout is normative in `contracts/plan-format.md`. Changing any field's
//! presence, order or width MUST bump `plan_format` in the manifest, because the
//! record has no length prefix and that version is a decoder's only signal of
//! the width.

use crate::keys::{CacheKey, SessionId};

/// Size of one encoded record.
pub const RECORD_BYTES: usize = 40;

/// `flags` bits. 4-7 are reserved and MUST be zero.
pub mod flags {
    /// First key of a request.
    pub const REQUEST_START: u8 = 1 << 0;
    /// Last key of a request.
    pub const REQUEST_END: u8 = 1 << 1;
    /// Inside the warmup window; excluded from steady-state statistics.
    pub const WARMUP: u8 = 1 << 2;
    /// Warmup deliberately did not pre-request this key.
    ///
    /// **Not** a predicted miss. It states a fact about the trace; whether a
    /// consumer misses on it is entirely the consumer's affair, and one that
    /// hits has violated nothing.
    pub const COLD: u8 = 1 << 3;
    /// Bits that must be zero on write and are rejected on read.
    pub const RESERVED_MASK: u8 = 0xF0;
}

/// One plan event: a single block reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanEvent {
    /// Absolute nanoseconds from the plan's time origin; non-decreasing.
    pub t_ns: u64,
    /// The block referenced.
    pub key: CacheKey,
    /// Payload bytes; a pure function of `key` (spec FR-011).
    pub size: u32,
    /// Groups the keys of one request; ascending.
    pub request_id: u32,
    /// The owning session. Stored, not derived — see `contracts/plan-format.md`.
    pub session_id: SessionId,
    /// Trie depth of this key.
    pub depth: u32,
    /// 1-based turn index within the session.
    pub turn: u16,
    /// Index into `topology.nodes`.
    pub node: u16,
    /// Which `workload.mix` entry the session was drawn from.
    pub mix_index: u8,
    /// See [`flags`].
    pub flags: u8,
}

/// A record that could not be decoded.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer was not `RECORD_BYTES` long.
    ShortBuffer(usize),
    /// A reserved flag bit or the reserved field was non-zero.
    ///
    /// Rejected rather than ignored so that a later additive change cannot be
    /// silently misread by an older decoder.
    ReservedNonZero,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::ShortBuffer(n) => {
                write!(f, "record buffer is {n} bytes, expected {RECORD_BYTES}")
            }
            DecodeError::ReservedNonZero => {
                write!(
                    f,
                    "reserved bits are non-zero; refusing to guess the layout"
                )
            }
        }
    }
}

impl std::error::Error for DecodeError {}

impl PlanEvent {
    /// Encode into exactly [`RECORD_BYTES`] bytes.
    pub fn encode(&self) -> [u8; RECORD_BYTES] {
        let mut b = [0u8; RECORD_BYTES];
        b[0..8].copy_from_slice(&self.t_ns.to_le_bytes());
        b[8..16].copy_from_slice(&self.key.0.to_le_bytes());
        b[16..20].copy_from_slice(&self.size.to_le_bytes());
        b[20..24].copy_from_slice(&self.request_id.to_le_bytes());
        b[24..28].copy_from_slice(&self.session_id.0.to_le_bytes());
        b[28..32].copy_from_slice(&self.depth.to_le_bytes());
        b[32..34].copy_from_slice(&self.turn.to_le_bytes());
        b[34..36].copy_from_slice(&self.node.to_le_bytes());
        b[36] = self.mix_index;
        b[37] = self.flags;
        // b[38..40] reserved, left zero.
        b
    }

    /// Decode, rejecting anything with reserved bits set.
    pub fn decode(b: &[u8]) -> Result<PlanEvent, DecodeError> {
        if b.len() < RECORD_BYTES {
            return Err(DecodeError::ShortBuffer(b.len()));
        }
        let flags = b[37];
        if flags & flags::RESERVED_MASK != 0 || b[38] != 0 || b[39] != 0 {
            return Err(DecodeError::ReservedNonZero);
        }
        Ok(PlanEvent {
            t_ns: u64::from_le_bytes(b[0..8].try_into().unwrap()),
            key: CacheKey(u64::from_le_bytes(b[8..16].try_into().unwrap())),
            size: u32::from_le_bytes(b[16..20].try_into().unwrap()),
            request_id: u32::from_le_bytes(b[20..24].try_into().unwrap()),
            session_id: SessionId(u32::from_le_bytes(b[24..28].try_into().unwrap())),
            depth: u32::from_le_bytes(b[28..32].try_into().unwrap()),
            turn: u16::from_le_bytes(b[32..34].try_into().unwrap()),
            node: u16::from_le_bytes(b[34..36].try_into().unwrap()),
            mix_index: b[36],
            flags,
        })
    }

    /// Whether this event carries `bit`.
    pub fn has(&self, bit: u8) -> bool {
        self.flags & bit != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PlanEvent {
        PlanEvent {
            t_ns: 1_234_567_890,
            key: CacheKey(0xDEAD_BEEF_CAFE_F00D),
            size: 131_072,
            request_id: 42,
            session_id: SessionId(7),
            depth: 19,
            turn: 3,
            node: 2,
            mix_index: 1,
            flags: flags::REQUEST_START | flags::COLD,
        }
    }

    #[test]
    fn record_is_forty_bytes_and_field_offsets_are_aligned() {
        // contracts/plan-format.md. Offsets are asserted explicitly because the
        // decoder's only signal of the layout is the manifest's plan_format.
        assert_eq!(RECORD_BYTES, 40);
        assert_eq!(RECORD_BYTES % 8, 0, "array would need packed intermediate");
        // Natural alignment: u64s at 0 and 8, u32s at 16/20/24/28, u16s at 32/34.
        for (off, width) in [
            (0, 8),
            (8, 8),
            (16, 4),
            (20, 4),
            (24, 4),
            (28, 4),
            (32, 2),
            (34, 2),
        ] {
            assert_eq!(off % width, 0, "field at {off} is not {width}-aligned");
        }
    }

    #[test]
    fn round_trips() {
        let e = sample();
        assert_eq!(PlanEvent::decode(&e.encode()).unwrap(), e);
    }

    #[test]
    fn reserved_flag_bits_are_rejected_not_ignored() {
        let mut b = sample().encode();
        b[37] |= 0x10;
        assert_eq!(PlanEvent::decode(&b), Err(DecodeError::ReservedNonZero));
    }

    #[test]
    fn reserved_trailing_bytes_are_rejected() {
        let mut b = sample().encode();
        b[38] = 1;
        assert_eq!(PlanEvent::decode(&b), Err(DecodeError::ReservedNonZero));
    }

    #[test]
    fn short_buffer_is_reported_with_its_length() {
        assert_eq!(
            PlanEvent::decode(&[0u8; 39]),
            Err(DecodeError::ShortBuffer(39))
        );
    }

    #[test]
    fn cold_is_not_a_predicted_miss() {
        // Documented semantics, pinned so a future reader does not "helpfully"
        // treat COLD as an expected outcome.
        let e = sample();
        assert!(e.has(flags::COLD));
        assert!(!e.has(flags::WARMUP));
    }
}

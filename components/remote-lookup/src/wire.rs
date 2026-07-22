//! Hand-rolled, little-endian, explicitly-framed wire codec for the v1
//! remote-lookup zyre protocol (research Decision 3; `contracts/wire-protocol.md`).
//!
//! Every frame is `[version: u8 = 1][msg_type: u8][op_id: u64]` followed by a
//! type-specific payload. Encode/decode are pure and unit-testable in isolation;
//! an unknown `msg_type` (or an unsupported `version`) decodes to a
//! `WireMessage::Unknown` variant so callers can log-and-ignore (FR-018), and a
//! truncated/malformed frame is a hard `WireError`.

use interfaces::{CacheKey, Endpoint};

/// Protocol version stamped in every frame header.
pub const WIRE_VERSION: u8 = 1;

// Message type tags (header byte 1).
const MSG_KEY_QUERY: u8 = 1;
const MSG_KEY_RESPONSE: u8 = 2;
const MSG_RDMA_REQUEST: u8 = 3;
const MSG_RDMA_STATUS: u8 = 4;

/// Per-key availability reported in a KEY_RESPONSE (`avail` tag on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Avail {
    /// The peer does not hold the key (at the requested size).
    None,
    /// The peer holds the key resident in its memory tier.
    Memory,
    /// The peer holds the key on its block/disk tier.
    Disk,
}

impl Avail {
    fn to_u8(self) -> u8 {
        match self {
            Avail::None => 0,
            Avail::Memory => 1,
            Avail::Disk => 2,
        }
    }

    fn from_u8(v: u8) -> Option<Avail> {
        match v {
            0 => Some(Avail::None),
            1 => Some(Avail::Memory),
            2 => Some(Avail::Disk),
            _ => None,
        }
    }
}

/// Per-key RDMA outcome reported in an RDMA_STATUS (`status` tag on the wire).
///
/// Maps the initiator's `PushStatus` per FR-016: `KeyNotFound`/`SizeMismatch`
/// both fold to [`RdmaStatusCode::KeyNoLongerAvailable`] defensively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RdmaStatusCode {
    /// The one-sided write completed; the value is now in the landing slot.
    Success,
    /// The serving peer could not connect back to the requester's responder.
    UnableToConnect,
    /// The key was evicted / size-mismatched by the time the peer served it.
    KeyNoLongerAvailable,
}

impl RdmaStatusCode {
    fn to_u8(self) -> u8 {
        match self {
            RdmaStatusCode::Success => 0,
            RdmaStatusCode::UnableToConnect => 1,
            RdmaStatusCode::KeyNoLongerAvailable => 2,
        }
    }

    fn from_u8(v: u8) -> Option<RdmaStatusCode> {
        match v {
            0 => Some(RdmaStatusCode::Success),
            1 => Some(RdmaStatusCode::UnableToConnect),
            2 => Some(RdmaStatusCode::KeyNoLongerAvailable),
            _ => None,
        }
    }
}

/// One landing slot advertised to a serving peer in an RDMA_REQUEST.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotDesc {
    /// The key whose value the peer should RDMA-write.
    pub key: CacheKey,
    /// Requester pool address the value must land at.
    pub addr: u64,
    /// Expected value length in bytes (== requested size).
    pub length: u32,
}

/// A decoded v1 wire message. Each variant carries the header `op_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireMessage {
    /// KEY_QUERY (SHOUT): which peers hold each `(key, size)`.
    KeyQuery {
        op_id: u64,
        entries: Vec<(CacheKey, u32)>,
    },
    /// KEY_RESPONSE (WHISPER): the responder's endpoint + per-key availability.
    KeyResponse {
        op_id: u64,
        endpoint: Endpoint,
        entries: Vec<(CacheKey, u32, Avail)>,
    },
    /// RDMA_REQUEST (WHISPER): requester endpoint + pool rkey + landing slots.
    RdmaRequest {
        op_id: u64,
        endpoint: Endpoint,
        rkey: u32,
        slots: Vec<SlotDesc>,
    },
    /// RDMA_STATUS (WHISPER): per-key outcome after the serving peer's `push`.
    RdmaStatus {
        op_id: u64,
        entries: Vec<(CacheKey, RdmaStatusCode)>,
    },
    /// A frame whose `version` is unsupported or whose `msg_type` is unknown.
    /// Callers log-and-ignore it (FR-018); the header fields are preserved for
    /// diagnostics.
    Unknown {
        version: u8,
        msg_type: u8,
        op_id: u64,
    },
}

/// Errors from decoding a wire frame. (Encoding is infallible.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The buffer ended before a fixed-size field or declared record could be read.
    Truncated,
    /// A tagged field (availability / status) held a value outside its domain.
    BadTag,
    /// A length-prefixed field (endpoint IP) was not valid UTF-8.
    BadUtf8,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::Truncated => write!(f, "wire frame truncated"),
            WireError::BadTag => write!(f, "wire frame has an out-of-range tag"),
            WireError::BadUtf8 => write!(f, "wire frame endpoint is not valid UTF-8"),
        }
    }
}

impl std::error::Error for WireError {}

impl WireMessage {
    /// The header `op_id` of this message.
    pub fn op_id(&self) -> u64 {
        match self {
            WireMessage::KeyQuery { op_id, .. }
            | WireMessage::KeyResponse { op_id, .. }
            | WireMessage::RdmaRequest { op_id, .. }
            | WireMessage::RdmaStatus { op_id, .. }
            | WireMessage::Unknown { op_id, .. } => *op_id,
        }
    }

    /// Encode this message into a fresh byte vector.
    ///
    /// [`WireMessage::Unknown`] is a decode-only sentinel and encodes to just its
    /// (already-unknown) header — it is never produced by this crate's senders.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(16);
        match self {
            WireMessage::KeyQuery { op_id, entries } => {
                put_header(&mut buf, MSG_KEY_QUERY, *op_id);
                put_u32(&mut buf, entries.len() as u32);
                for (key, size) in entries {
                    put_u64(&mut buf, *key);
                    put_u32(&mut buf, *size);
                }
            }
            WireMessage::KeyResponse {
                op_id,
                endpoint,
                entries,
            } => {
                put_header(&mut buf, MSG_KEY_RESPONSE, *op_id);
                put_endpoint(&mut buf, endpoint);
                put_u32(&mut buf, entries.len() as u32);
                for (key, size, avail) in entries {
                    put_u64(&mut buf, *key);
                    put_u32(&mut buf, *size);
                    buf.push(avail.to_u8());
                }
            }
            WireMessage::RdmaRequest {
                op_id,
                endpoint,
                rkey,
                slots,
            } => {
                put_header(&mut buf, MSG_RDMA_REQUEST, *op_id);
                put_endpoint(&mut buf, endpoint);
                put_u32(&mut buf, *rkey);
                put_u32(&mut buf, slots.len() as u32);
                for slot in slots {
                    put_u64(&mut buf, slot.key);
                    put_u64(&mut buf, slot.addr);
                    put_u32(&mut buf, slot.length);
                }
            }
            WireMessage::RdmaStatus { op_id, entries } => {
                put_header(&mut buf, MSG_RDMA_STATUS, *op_id);
                put_u32(&mut buf, entries.len() as u32);
                for (key, status) in entries {
                    put_u64(&mut buf, *key);
                    buf.push(status.to_u8());
                }
            }
            WireMessage::Unknown {
                version,
                msg_type,
                op_id,
            } => {
                buf.push(*version);
                buf.push(*msg_type);
                put_u64(&mut buf, *op_id);
            }
        }
        buf
    }

    /// Decode a frame. An unsupported `version` or unknown `msg_type` yields
    /// [`WireMessage::Unknown`] (log-and-ignore, FR-018); structural problems
    /// yield a [`WireError`].
    pub fn decode(bytes: &[u8]) -> Result<WireMessage, WireError> {
        let mut r = Reader::new(bytes);
        let version = r.u8()?;
        let msg_type = r.u8()?;
        let op_id = r.u64()?;

        if version != WIRE_VERSION {
            return Ok(WireMessage::Unknown {
                version,
                msg_type,
                op_id,
            });
        }

        match msg_type {
            MSG_KEY_QUERY => {
                let count = r.u32()? as usize;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = r.u64()?;
                    let size = r.u32()?;
                    entries.push((key, size));
                }
                Ok(WireMessage::KeyQuery { op_id, entries })
            }
            MSG_KEY_RESPONSE => {
                let endpoint = r.endpoint()?;
                let count = r.u32()? as usize;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = r.u64()?;
                    let size = r.u32()?;
                    let avail = Avail::from_u8(r.u8()?).ok_or(WireError::BadTag)?;
                    entries.push((key, size, avail));
                }
                Ok(WireMessage::KeyResponse {
                    op_id,
                    endpoint,
                    entries,
                })
            }
            MSG_RDMA_REQUEST => {
                let endpoint = r.endpoint()?;
                let rkey = r.u32()?;
                let count = r.u32()? as usize;
                let mut slots = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = r.u64()?;
                    let addr = r.u64()?;
                    let length = r.u32()?;
                    slots.push(SlotDesc { key, addr, length });
                }
                Ok(WireMessage::RdmaRequest {
                    op_id,
                    endpoint,
                    rkey,
                    slots,
                })
            }
            MSG_RDMA_STATUS => {
                let count = r.u32()? as usize;
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let key = r.u64()?;
                    let status = RdmaStatusCode::from_u8(r.u8()?).ok_or(WireError::BadTag)?;
                    entries.push((key, status));
                }
                Ok(WireMessage::RdmaStatus { op_id, entries })
            }
            _ => Ok(WireMessage::Unknown {
                version,
                msg_type,
                op_id,
            }),
        }
    }
}

// --- little-endian writers -------------------------------------------------

fn put_header(buf: &mut Vec<u8>, msg_type: u8, op_id: u64) {
    buf.push(WIRE_VERSION);
    buf.push(msg_type);
    put_u64(buf, op_id);
}

fn put_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(buf: &mut Vec<u8>, v: u64) {
    buf.extend_from_slice(&v.to_le_bytes());
}

fn put_endpoint(buf: &mut Vec<u8>, ep: &Endpoint) {
    let ip = ep.ip.as_bytes();
    put_u16(buf, ip.len() as u16);
    buf.extend_from_slice(ip);
    put_u16(buf, ep.port);
}

// --- little-endian reader --------------------------------------------------

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let end = self.pos.checked_add(n).ok_or(WireError::Truncated)?;
        if end > self.buf.len() {
            return Err(WireError::Truncated);
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, WireError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, WireError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, WireError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn endpoint(&mut self) -> Result<Endpoint, WireError> {
        let ip_len = self.u16()? as usize;
        let ip_bytes = self.take(ip_len)?;
        let ip = std::str::from_utf8(ip_bytes)
            .map_err(|_| WireError::BadUtf8)?
            .to_string();
        let port = self.u16()?;
        Ok(Endpoint { ip, port })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ep() -> Endpoint {
        Endpoint {
            ip: "192.0.2.10".into(),
            port: 49152,
        }
    }

    fn roundtrip(msg: &WireMessage) {
        let bytes = msg.encode();
        let decoded = WireMessage::decode(&bytes).expect("decode");
        assert_eq!(&decoded, msg);
    }

    #[test]
    fn key_query_roundtrip() {
        roundtrip(&WireMessage::KeyQuery {
            op_id: 7,
            entries: vec![(1, 4096), (2, 8192), (u64::MAX, u32::MAX)],
        });
    }

    #[test]
    fn key_query_empty_roundtrip() {
        roundtrip(&WireMessage::KeyQuery {
            op_id: 0,
            entries: vec![],
        });
    }

    #[test]
    fn key_response_roundtrip() {
        roundtrip(&WireMessage::KeyResponse {
            op_id: 42,
            endpoint: ep(),
            entries: vec![
                (1, 4096, Avail::Memory),
                (2, 4096, Avail::Disk),
                (3, 4096, Avail::None),
            ],
        });
    }

    #[test]
    fn rdma_request_roundtrip() {
        roundtrip(&WireMessage::RdmaRequest {
            op_id: 99,
            endpoint: ep(),
            rkey: 0xDEAD_BEEF,
            slots: vec![
                SlotDesc {
                    key: 1,
                    addr: 0x7f00_1000,
                    length: 4096,
                },
                SlotDesc {
                    key: 2,
                    addr: 0x7f00_2000,
                    length: 8192,
                },
            ],
        });
    }

    #[test]
    fn rdma_status_roundtrip() {
        roundtrip(&WireMessage::RdmaStatus {
            op_id: 5,
            entries: vec![
                (1, RdmaStatusCode::Success),
                (2, RdmaStatusCode::UnableToConnect),
                (3, RdmaStatusCode::KeyNoLongerAvailable),
            ],
        });
    }

    #[test]
    fn header_layout_is_version_type_opid() {
        let bytes = WireMessage::KeyQuery {
            op_id: 0x0102_0304_0506_0708,
            entries: vec![],
        }
        .encode();
        assert_eq!(bytes[0], WIRE_VERSION);
        assert_eq!(bytes[1], MSG_KEY_QUERY);
        // op_id is little-endian in bytes[2..10].
        assert_eq!(&bytes[2..10], &0x0102_0304_0506_0708u64.to_le_bytes());
    }

    #[test]
    fn unknown_msg_type_decodes_to_unknown() {
        let mut bytes = vec![WIRE_VERSION, 250]; // version, unknown type
        bytes.extend_from_slice(&123u64.to_le_bytes());
        let decoded = WireMessage::decode(&bytes).expect("decode");
        assert_eq!(
            decoded,
            WireMessage::Unknown {
                version: WIRE_VERSION,
                msg_type: 250,
                op_id: 123,
            }
        );
    }

    #[test]
    fn unsupported_version_decodes_to_unknown() {
        let mut bytes = vec![99, MSG_KEY_QUERY]; // unsupported version
        bytes.extend_from_slice(&7u64.to_le_bytes());
        let decoded = WireMessage::decode(&bytes).expect("decode");
        assert!(matches!(
            decoded,
            WireMessage::Unknown {
                version: 99,
                msg_type: MSG_KEY_QUERY,
                op_id: 7,
            }
        ));
    }

    #[test]
    fn truncated_header_is_error() {
        assert_eq!(WireMessage::decode(&[1, 1]), Err(WireError::Truncated));
    }

    #[test]
    fn truncated_payload_is_error() {
        // KEY_QUERY declaring 1 entry but no entry bytes.
        let mut bytes = vec![WIRE_VERSION, MSG_KEY_QUERY];
        bytes.extend_from_slice(&1u64.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // count = 1
        assert_eq!(WireMessage::decode(&bytes), Err(WireError::Truncated));
    }

    #[test]
    fn bad_avail_tag_is_error() {
        let mut bytes = vec![WIRE_VERSION, MSG_KEY_RESPONSE];
        bytes.extend_from_slice(&1u64.to_le_bytes());
        // endpoint: ip_len=0, port=0
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // count = 1
        bytes.extend_from_slice(&1u64.to_le_bytes()); // key
        bytes.extend_from_slice(&4096u32.to_le_bytes()); // size
        bytes.push(9); // bad avail tag
        assert_eq!(WireMessage::decode(&bytes), Err(WireError::BadTag));
    }

    #[test]
    fn trailing_bytes_after_frame_are_ignored() {
        // A valid frame followed by junk still decodes the frame (framing is by
        // record counts, not buffer length).
        let mut bytes = WireMessage::RdmaStatus {
            op_id: 1,
            entries: vec![(1, RdmaStatusCode::Success)],
        }
        .encode();
        bytes.extend_from_slice(&[0xAA, 0xBB]);
        let decoded = WireMessage::decode(&bytes).expect("decode");
        assert_eq!(
            decoded,
            WireMessage::RdmaStatus {
                op_id: 1,
                entries: vec![(1, RdmaStatusCode::Success)],
            }
        );
    }
}

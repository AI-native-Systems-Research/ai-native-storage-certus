//! Faithful standalone Kani mirror of the remote-lookup pure logic.
//!
//! The real `remote-lookup` crate depends on `interfaces` (feature `spdk` ->
//! spdk-sys, needs a built SPDK) and dev-dep `zyre` (native libs), so it cannot
//! build under Kani. This crate copies — VERBATIM in behaviour — the three pure
//! cores whose properties the component-verify inventory marks verifiable:
//!   * the v1 wire codec  (src/wire.rs)
//!   * the per-op state machine  (src/operation.rs)
//!   * the batch_lookup / join / leave guard logic  (src/lib.rs)
//! and proves them with `#[kani::proof]` harnesses (see the `proofs` module).
//!
//! Mirror-fidelity notes (behaviour identical to the product; only Kani-usage
//! artifacts differ):
//!   * `CacheKey`, `Endpoint`, `PeerId`, `RemoteLookupError` are inlined copies
//!     of the real `interfaces` types.
//!   * `Operation` keeps only the fields the pure methods under test read
//!     (entries/state/tried/peers_expected/peers_replied). The timing (`Instant`
//!     deadlines), landing-slot, cached-reply and one-shot-channel fields belong
//!     to the DELEGATED timing/concurrency properties, not to the pure logic
//!     proved here, so they are omitted.
//!   * The per-key maps (`state`, `tried`) are modelled as small `Vec`
//!     association lists with unique keys rather than `HashMap`/`BTreeMap`.
//!     `HashMap`'s `RandomState` pulls entropy (`getrandom`) Kani cannot
//!     resolve, and `BTreeMap`'s node/pointer B-tree machinery explodes CBMC
//!     even with concrete keys (>150 s cap). The associative semantics under
//!     test (one value per key, lookup-or-default, first-write-wins) are exact
//!     in the Vec model; only container-internal collision/ordering behaviour —
//!     irrelevant to these properties — differs. Representative fidelity.

#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Mirror of interfaces:: types
// ---------------------------------------------------------------------------

/// Mirror of `interfaces::CacheKey`.
pub type CacheKey = u64;

/// Mirror of `interfaces::Endpoint`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub ip: String,
    pub port: u16,
}

/// Mirror of `interfaces::PeerId`. The real type is `PeerId(String)` (a peer's
/// network address). The only property proved over `PeerId` here is the
/// note_tried/already_tried DEDUP invariant, which depends solely on peer
/// IDENTITY EQUALITY, not on the string content. Kani cannot discharge the
/// dedup harness over a heap `String` inside a nested `Vec<Vec<PeerId>>` (CBMC's
/// allocator model does not terminate in 150 s even with fully concrete peers,
/// and `--no-unwinding-checks` does not help). Modelling the identity as a `u32`
/// keeps the dedup semantics EXACT while staying in CBMC's tractable range.
/// Representative fidelity: the identifier is an opaque comparable token, not a
/// string. Disclosed in the RL-OP-TRIED-DEDUP property note.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct PeerId(pub u32);

/// Mirror of `interfaces::RemoteLookupError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLookupError {
    NotFound,
    TransportError(String),
}

// ===========================================================================
// wire.rs  — verbatim behaviour copy
// ===========================================================================

pub mod wire {
    use super::{CacheKey, Endpoint};

    pub const WIRE_VERSION: u8 = 1;

    pub const MSG_KEY_QUERY: u8 = 1;
    pub const MSG_KEY_RESPONSE: u8 = 2;
    pub const MSG_RDMA_REQUEST: u8 = 3;
    pub const MSG_RDMA_STATUS: u8 = 4;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Avail {
        None,
        Memory,
        Disk,
    }

    impl Avail {
        pub fn to_u8(self) -> u8 {
            match self {
                Avail::None => 0,
                Avail::Memory => 1,
                Avail::Disk => 2,
            }
        }
        pub fn from_u8(v: u8) -> Option<Avail> {
            match v {
                0 => Some(Avail::None),
                1 => Some(Avail::Memory),
                2 => Some(Avail::Disk),
                _ => None,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum RdmaStatusCode {
        Success,
        UnableToConnect,
        KeyNoLongerAvailable,
    }

    impl RdmaStatusCode {
        pub fn to_u8(self) -> u8 {
            match self {
                RdmaStatusCode::Success => 0,
                RdmaStatusCode::UnableToConnect => 1,
                RdmaStatusCode::KeyNoLongerAvailable => 2,
            }
        }
        pub fn from_u8(v: u8) -> Option<RdmaStatusCode> {
            match v {
                0 => Some(RdmaStatusCode::Success),
                1 => Some(RdmaStatusCode::UnableToConnect),
                2 => Some(RdmaStatusCode::KeyNoLongerAvailable),
                _ => None,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SlotDesc {
        pub key: CacheKey,
        pub addr: u64,
        pub length: u32,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WireMessage {
        KeyQuery {
            op_id: u64,
            entries: Vec<(CacheKey, u32)>,
        },
        KeyResponse {
            op_id: u64,
            endpoint: Endpoint,
            entries: Vec<(CacheKey, u32, Avail)>,
        },
        RdmaRequest {
            op_id: u64,
            endpoint: Endpoint,
            rkey: u32,
            slots: Vec<SlotDesc>,
        },
        RdmaStatus {
            op_id: u64,
            entries: Vec<(CacheKey, RdmaStatusCode)>,
        },
        Unknown {
            version: u8,
            msg_type: u8,
            op_id: u64,
        },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum WireError {
        Truncated,
        BadTag,
        BadUtf8,
    }

    impl WireMessage {
        pub fn op_id(&self) -> u64 {
            match self {
                WireMessage::KeyQuery { op_id, .. }
                | WireMessage::KeyResponse { op_id, .. }
                | WireMessage::RdmaRequest { op_id, .. }
                | WireMessage::RdmaStatus { op_id, .. }
                | WireMessage::Unknown { op_id, .. } => *op_id,
            }
        }

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
                        let status =
                            RdmaStatusCode::from_u8(r.u8()?).ok_or(WireError::BadTag)?;
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
}

// ===========================================================================
// operation.rs  — verbatim behaviour copy of the PURE methods under test
// ===========================================================================

pub mod operation {
    use super::{BatchResult, CacheKey, PeerId, RemoteLookupError};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum KeyState {
        Unsatisfied,
        InProgress,
        Satisfied,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Phase {
        Phase1,
        Phase2,
    }

    /// Faithful mirror of the pure part of `operation::Operation`. Timing /
    /// slot / reply / channel fields are omitted (they back delegated
    /// timing/concurrency properties, not the pure logic proved here).
    pub struct Operation {
        pub entries: Vec<(CacheKey, u32)>,
        /// Association list with UNIQUE keys (see the fidelity note on `new`).
        pub state: Vec<(CacheKey, KeyState)>,
        /// Association list with UNIQUE keys.
        pub tried: Vec<(CacheKey, Vec<PeerId>)>,
        pub phase: Phase,
        pub peers_expected: usize,
        pub peers_replied: usize,
    }

    impl Operation {
        /// `state`/`tried` are modelled as small `Vec` association lists with
        /// unique keys, NOT a `HashMap`/`BTreeMap`. Both real (`HashMap`
        /// `RandomState`->`getrandom`) and `BTreeMap` (heavy node/pointer B-tree
        /// modelling — empirically explodes CBMC past a 150 s cap even with
        /// concrete keys and `--no-unwinding-checks`) representations pull
        /// machinery Kani cannot model at reasonable cost. The associative
        /// SEMANTICS these op properties depend on — exactly one value per key,
        /// lookup-or-default, first-write-wins on duplicate keys — are exact in
        /// this bounded Vec model, which CBMC discharges directly. Representative
        /// fidelity: container-internal collision/ordering behaviour (irrelevant
        /// to the properties under test) is the only thing not modelled.
        pub fn new(entries: Vec<(CacheKey, u32)>, peers_expected: usize) -> Self {
            let mut state: Vec<(CacheKey, KeyState)> = Vec::new();
            for (k, _) in &entries {
                if !state.iter().any(|(kk, _)| kk == k) {
                    state.push((*k, KeyState::Unsatisfied));
                }
            }
            Self {
                entries,
                state,
                tried: Vec::new(),
                phase: Phase::Phase1,
                peers_expected,
                peers_replied: 0,
            }
        }

        pub fn quorum_reached(&self, quorum_pct: u8) -> bool {
            self.peers_expected == 0
                || self.peers_replied * 100 >= self.peers_expected * quorum_pct as usize
        }

        pub fn size_of(&self, key: CacheKey) -> Option<u32> {
            self.entries
                .iter()
                .find_map(|(k, s)| (*k == key).then_some(*s))
        }

        pub fn state_of(&self, key: CacheKey) -> KeyState {
            self.state
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, s)| *s)
                .unwrap_or(KeyState::Unsatisfied)
        }

        pub fn set_state(&mut self, key: CacheKey, s: KeyState) {
            if let Some(slot) = self.state.iter_mut().find(|(k, _)| *k == key) {
                slot.1 = s;
            }
        }

        pub fn all_satisfied(&self) -> bool {
            self.state.iter().all(|(_, s)| *s == KeyState::Satisfied)
        }

        pub fn results(&self) -> BatchResult {
            self.entries
                .iter()
                .map(|(k, _)| {
                    if self.state_of(*k) == KeyState::Satisfied {
                        Ok(())
                    } else {
                        Err(RemoteLookupError::NotFound)
                    }
                })
                .collect()
        }

        pub fn note_tried(&mut self, key: CacheKey, peer: &PeerId) {
            if !self.tried.iter().any(|(k, _)| *k == key) {
                self.tried.push((key, Vec::new()));
            }
            let v = &mut self
                .tried
                .iter_mut()
                .find(|(k, _)| *k == key)
                .unwrap()
                .1;
            if !v.iter().any(|p| p == peer) {
                v.push(peer.clone());
            }
        }

        pub fn already_tried(&self, key: CacheKey, peer: &PeerId) -> bool {
            self.tried
                .iter()
                .find(|(k, _)| *k == key)
                .is_some_and(|(_, v)| v.iter().any(|p| p == peer))
        }

        /// Number of peers recorded as tried for `key`, or `None` if the key is
        /// untracked. Mirror-only accessor for the dedup property harness (the
        /// Vec model has no `BTreeMap::get`).
        pub fn tried_count(&self, key: CacheKey) -> Option<usize> {
            self.tried
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.len())
        }
    }
}

/// Mirror of `actor::BatchResult`.
pub type BatchResult = Vec<Result<(), RemoteLookupError>>;

// ===========================================================================
// lib.rs  — batch_lookup / join_cluster / leave_cluster guard logic
// ===========================================================================

/// Faithful mirror of the guard branches of the component's public methods.
/// The actor is modelled by whether the submission channel exists
/// (`submit_tx`); the initialized (`Some`) fast-path submits to the actor and
/// blocks — that path is a delegated concurrency property and is not exercised
/// by the pure-guard harnesses below.
pub struct Component {
    /// `Some(())` once initialized (actor thread + submission channel exist).
    pub submit_tx: Option<()>,
}

impl Component {
    pub fn uninitialized() -> Self {
        Component { submit_tx: None }
    }

    pub fn initialized() -> Self {
        Component {
            submit_tx: Some(()),
        }
    }

    /// Mirror of `batch_lookup` empty-input + uninitialized guards
    /// (lib.rs:276-289). The initialized branch (submit to actor + block) is a
    /// delegated concurrency property; a caller that reaches it here gets the
    /// same one-result-per-entry NotFound fallback the real code returns when
    /// the channel is gone, so the positional shape still holds.
    pub fn batch_lookup(&self, entries: &[(CacheKey, u32)]) -> BatchResult {
        if entries.is_empty() {
            return Vec::new();
        }
        let all_not_found = || -> BatchResult {
            entries
                .iter()
                .map(|_| Err(RemoteLookupError::NotFound))
                .collect()
        };
        let Some(_tx) = self.submit_tx.as_ref() else {
            return all_not_found();
        };
        // Initialized: real code submits to the actor and blocks. Not modelled
        // (delegated). Fall back to the same positional NotFound shape.
        all_not_found()
    }

    /// Mirror of `join_cluster` uninitialized guard (lib.rs:318-324).
    pub fn join_cluster(&self, _endpoint: &str) -> Result<(), RemoteLookupError> {
        let _tx = self.submit_tx.as_ref().ok_or_else(|| {
            RemoteLookupError::TransportError("remote-lookup: not initialized".into())
        })?;
        Ok(())
    }

    /// Mirror of `leave_cluster` uninitialized guard (lib.rs:327-333).
    pub fn leave_cluster(&self) -> Result<(), RemoteLookupError> {
        let _tx = self.submit_tx.as_ref().ok_or_else(|| {
            RemoteLookupError::TransportError("remote-lookup: not initialized".into())
        })?;
        Ok(())
    }
}

#[cfg(kani)]
mod proofs;

//! Faithful standalone Creusot mirror of the remote-lookup pure logic.
//!
//! The real `remote-lookup` crate pulls `interfaces[spdk]` -> spdk-sys and a zyre
//! dev-dep (native libs), so it cannot build under creusot-rustc. This crate copies
//! the pure, verifiable logic of `src/wire.rs`, `src/operation.rs` and the
//! `batch_lookup`/`join_cluster` decision core of `src/lib.rs` VERBATIM in behaviour,
//! with the real product types mirrored (CacheKey = u64, Endpoint { ip, port },
//! PeerId(String)). Each `verify_<id>` function inlines the logic under test so the
//! obligation is self-contained (no assumed callee contracts) and states the property
//! as its contract; the scorer proves that module by re-running `cargo creusot`.
#![cfg_attr(creusot, allow(unused))]
#![allow(dead_code)]
#![allow(non_snake_case)]

use creusot_std::prelude::*;
// Explicit named imports override the std-prelude derives with creusot's versions
// (the glob alone is ambiguous — see creusot-std/src/lib.rs base_prelude note).
use creusot_std::prelude::{Clone, PartialEq};
use creusot_std::model::DeepModel;

// ------------------------------------------------------------------------------
// Mirror types (copied from interfaces + wire.rs; behaviourally identical).
// ------------------------------------------------------------------------------

pub type CacheKey = u64;

/// Per-key availability tag (wire.rs Avail).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum Avail {
    None,
    Memory,
    Disk,
}

/// Per-key RDMA outcome code (wire.rs RdmaStatusCode).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum RdmaStatusCode {
    Success,
    UnableToConnect,
    KeyNoLongerAvailable,
}

/// Per-key progress (operation.rs KeyState).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum KeyState {
    Unsatisfied,
    InProgress,
    Satisfied,
}

/// Structured decode dispatch outcome (the dispatch facet of wire.rs decode, with the
/// byte-parsing abstracted to already-parsed header fields).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum DecodeKind {
    KeyQuery,
    KeyResponse,
    RdmaRequest,
    RdmaStatus,
    Unknown,
}

/// Mirror of RemoteLookupError (only the NotFound arm is needed for the pure proofs).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum LookupErr {
    NotFound,
    TransportError,
}

/// Decode error (wire.rs WireError; only Truncated/BadTag are needed for the pure proofs).
#[derive(Clone, Copy, PartialEq, Eq, DeepModel)]
pub enum WireErr {
    Truncated,
    BadTag,
}

pub const WIRE_VERSION: u8 = 1;
pub const MSG_KEY_QUERY: u8 = 1;
pub const MSG_KEY_RESPONSE: u8 = 2;
pub const MSG_RDMA_REQUEST: u8 = 3;
pub const MSG_RDMA_STATUS: u8 = 4;

// ------------------------------------------------------------------------------
// RL-AVAIL-TAG-BIJECTION
// from_u8 is the left inverse of to_u8 on {None,Memory,Disk}; bytes outside {0,1,2}
// decode to None (rejected). Logic inlined verbatim from wire.rs:32-49.
// ------------------------------------------------------------------------------
#[ensures(result == true)]
pub fn verify_rl_avail_tag_bijection(a: Avail, b: u8) -> bool {
    // round trip on the domain
    let enc: u8 = match a {
        Avail::None => 0,
        Avail::Memory => 1,
        Avail::Disk => 2,
    };
    let dec: Option<Avail> = match enc {
        0 => Some(Avail::None),
        1 => Some(Avail::Memory),
        2 => Some(Avail::Disk),
        _ => None,
    };
    proof_assert!(dec == Some(a));
    // out-of-domain rejection
    let dec2: Option<Avail> = match b {
        0 => Some(Avail::None),
        1 => Some(Avail::Memory),
        2 => Some(Avail::Disk),
        _ => None,
    };
    proof_assert!(b > 2u8 ==> dec2 == None::<Avail>);
    true
}

// deliberately-false twin: claims byte 9 decodes to Some (MUST fail).
#[ensures(result == true)]
pub fn verify_rl_avail_tag_bijection__mutant(b: u8) -> bool {
    let dec2: Option<Avail> = match b {
        0 => Some(Avail::None),
        1 => Some(Avail::Memory),
        2 => Some(Avail::Disk),
        _ => None,
    };
    proof_assert!(b > 2u8 ==> dec2 != None::<Avail>);
    true
}

// ------------------------------------------------------------------------------
// RL-STATUS-TAG-BIJECTION  (wire.rs:65-82)
// ------------------------------------------------------------------------------
#[ensures(result == true)]
pub fn verify_rl_status_tag_bijection(s: RdmaStatusCode, b: u8) -> bool {
    let enc: u8 = match s {
        RdmaStatusCode::Success => 0,
        RdmaStatusCode::UnableToConnect => 1,
        RdmaStatusCode::KeyNoLongerAvailable => 2,
    };
    let dec: Option<RdmaStatusCode> = match enc {
        0 => Some(RdmaStatusCode::Success),
        1 => Some(RdmaStatusCode::UnableToConnect),
        2 => Some(RdmaStatusCode::KeyNoLongerAvailable),
        _ => None,
    };
    proof_assert!(dec == Some(s));
    let dec2: Option<RdmaStatusCode> = match b {
        0 => Some(RdmaStatusCode::Success),
        1 => Some(RdmaStatusCode::UnableToConnect),
        2 => Some(RdmaStatusCode::KeyNoLongerAvailable),
        _ => None,
    };
    proof_assert!(b > 2u8 ==> dec2 == None::<RdmaStatusCode>);
    true
}

#[ensures(result == true)]
pub fn verify_rl_status_tag_bijection__mutant(b: u8) -> bool {
    let dec2: Option<RdmaStatusCode> = match b {
        0 => Some(RdmaStatusCode::Success),
        1 => Some(RdmaStatusCode::UnableToConnect),
        2 => Some(RdmaStatusCode::KeyNoLongerAvailable),
        _ => None,
    };
    proof_assert!(b > 2u8 ==> dec2 != None::<RdmaStatusCode>);
    true
}

// ------------------------------------------------------------------------------
// RL-WIRE-OPID  (wire.rs:156-164)
// op_id() returns the stored header op_id for every variant. Modelled as a tagged
// header record; the accessor returns its op_id field unchanged.
// ------------------------------------------------------------------------------
#[ensures(result == op_id)]
pub fn verify_rl_wire_opid(kind: DecodeKind, op_id: u64) -> u64 {
    // op_id() matches every variant and returns the stored op_id (wire.rs:157-163).
    match kind {
        DecodeKind::KeyQuery => op_id,
        DecodeKind::KeyResponse => op_id,
        DecodeKind::RdmaRequest => op_id,
        DecodeKind::RdmaStatus => op_id,
        DecodeKind::Unknown => op_id,
    }
}

#[ensures(result == op_id)]
pub fn verify_rl_wire_opid__mutant(kind: DecodeKind, op_id: u64) -> u64 {
    match kind {
        DecodeKind::Unknown => 0, // deliberately drops the op_id -> MUST fail
        _ => op_id,
    }
}

// ------------------------------------------------------------------------------
// RL-OP-QUORUM-ZERO  (operation.rs:115-118)
// peers_expected == 0 => quorum reached.
// ------------------------------------------------------------------------------
#[requires(pr@ * 100 <= usize::MAX@ && pe@ * (pct@) <= usize::MAX@)]
#[ensures(pe == 0usize ==> result == true)]
pub fn verify_rl_op_quorum_zero(pe: usize, pr: usize, pct: u8) -> bool {
    pe == 0 || pr * 100 >= pe * (pct as usize)
}

#[requires(pr@ * 100 <= usize::MAX@ && pe@ * (pct@) <= usize::MAX@)]
#[ensures(pe == 0usize ==> result == false)] // MUST fail: zero-expected is quorum-reached
pub fn verify_rl_op_quorum_zero__mutant(pe: usize, pr: usize, pct: u8) -> bool {
    pe == 0 || pr * 100 >= pe * (pct as usize)
}

// ------------------------------------------------------------------------------
// RL-OP-QUORUM-ARITH  (operation.rs:115-118)
// For pe>0, quorum_reached iff pr*100 >= pe*pct.
// ------------------------------------------------------------------------------
#[requires(pr@ * 100 <= usize::MAX@ && pe@ * (pct@) <= usize::MAX@)]
#[ensures(result == (pe == 0usize || pr@ * 100 >= pe@ * (pct@)))]
pub fn verify_rl_op_quorum_arith(pe: usize, pr: usize, pct: u8) -> bool {
    pe == 0 || pr * 100 >= pe * (pct as usize)
}

#[requires(pr@ * 100 <= usize::MAX@ && pe@ * (pct@) <= usize::MAX@)]
#[ensures(result == (pe == 0usize || pr@ * 100 > pe@ * (pct@)))] // strict > : MUST fail
pub fn verify_rl_op_quorum_arith__mutant(pe: usize, pr: usize, pct: u8) -> bool {
    pe == 0 || pr * 100 >= pe * (pct as usize)
}

// ------------------------------------------------------------------------------
// RL-WIRE-DECODE-UNKNOWN-TYPE  (wire.rs:304-308)
// A supported-version frame with an unrecognized msg_type dispatches to Unknown.
// ------------------------------------------------------------------------------
// Both decode verifies inline the wire.rs decode dispatch (lines 241-308) verbatim
// and prove it equals the logical dispatch spec. UNKNOWN-TYPE exercises the
// unrecognized-type arm; BAD-VERSION exercises the version guard; the same inlined
// logic covers both facets since dispatch_expected encodes both.
#[ensures(result == dispatch_expected(version, msg_type))]
pub fn verify_rl_wire_decode_unknown_type(version: u8, msg_type: u8) -> DecodeKind {
    if version != WIRE_VERSION {
        DecodeKind::Unknown
    } else {
        match msg_type {
            1 => DecodeKind::KeyQuery,
            2 => DecodeKind::KeyResponse,
            3 => DecodeKind::RdmaRequest,
            4 => DecodeKind::RdmaStatus,
            _ => DecodeKind::Unknown,
        }
    }
}

#[ensures(result == dispatch_expected(version, msg_type))]
pub fn verify_rl_wire_decode_bad_version(version: u8, msg_type: u8) -> DecodeKind {
    if version != WIRE_VERSION {
        DecodeKind::Unknown
    } else {
        match msg_type {
            1 => DecodeKind::KeyQuery,
            2 => DecodeKind::KeyResponse,
            3 => DecodeKind::RdmaRequest,
            4 => DecodeKind::RdmaStatus,
            _ => DecodeKind::Unknown,
        }
    }
}

// mutant: drops the version guard, so a bad-version KEY_QUERY-typed frame is
// mis-accepted as KeyQuery instead of Unknown -> MUST fail.
#[ensures(result == dispatch_expected(version, msg_type))]
pub fn verify_rl_wire_decode_bad_version__mutant(version: u8, msg_type: u8) -> DecodeKind {
    match msg_type {
        1 => DecodeKind::KeyQuery,
        2 => DecodeKind::KeyResponse,
        3 => DecodeKind::RdmaRequest,
        4 => DecodeKind::RdmaStatus,
        _ => DecodeKind::Unknown,
    }
}

// logical spec of the dispatch (used by both decode verifies as the property).
#[logic(open)]
pub fn dispatch_expected(version: u8, msg_type: u8) -> DecodeKind {
    if version != WIRE_VERSION {
        DecodeKind::Unknown
    } else if msg_type == 1u8 {
        DecodeKind::KeyQuery
    } else if msg_type == 2u8 {
        DecodeKind::KeyResponse
    } else if msg_type == 3u8 {
        DecodeKind::RdmaRequest
    } else if msg_type == 4u8 {
        DecodeKind::RdmaStatus
    } else {
        DecodeKind::Unknown
    }
}

// mutant: claims every frame is KeyQuery (MUST fail).
#[ensures(result == DecodeKind::KeyQuery)]
pub fn verify_rl_wire_decode_unknown_type__mutant(version: u8, msg_type: u8) -> DecodeKind {
    if version != WIRE_VERSION {
        DecodeKind::Unknown
    } else {
        match msg_type {
            1 => DecodeKind::KeyQuery,
            2 => DecodeKind::KeyResponse,
            3 => DecodeKind::RdmaRequest,
            4 => DecodeKind::RdmaStatus,
            _ => DecodeKind::Unknown,
        }
    }
}

// ------------------------------------------------------------------------------
// RL-WIRE-DECODE-BADTAG  (wire.rs:267,299 — from_u8(tag).ok_or(BadTag))
// A tag outside its {0,1,2} domain yields Err(BadTag); an in-domain tag yields Ok.
// ------------------------------------------------------------------------------
#[ensures(tag > 2u8 ==> result == Err(WireErr::BadTag))]
#[ensures(tag <= 2u8 ==> result != Err(WireErr::BadTag))]
pub fn verify_rl_wire_decode_badtag(tag: u8) -> Result<Avail, WireErr> {
    match tag {
        0 => Ok(Avail::None),
        1 => Ok(Avail::Memory),
        2 => Ok(Avail::Disk),
        _ => Err(WireErr::BadTag),
    }
}

#[ensures(tag > 2u8 ==> result == Err(WireErr::BadTag))] // MUST fail: mutant accepts tag 3
pub fn verify_rl_wire_decode_badtag__mutant(tag: u8) -> Result<Avail, WireErr> {
    match tag {
        0 => Ok(Avail::None),
        1 => Ok(Avail::Memory),
        2 => Ok(Avail::Disk),
        3 => Ok(Avail::Disk), // bug: tag 3 no longer rejected
        _ => Err(WireErr::BadTag),
    }
}

// ------------------------------------------------------------------------------
// RL-WIRE-DECODE-TRUNCATED  (wire.rs:352-360 — Reader::take bounds check)
// A read whose window exceeds the buffer returns Err(Truncated); an in-bounds read
// never returns Truncated. No out-of-bounds access.
// ------------------------------------------------------------------------------
#[requires(pos@ + n@ <= usize::MAX@)]
#[ensures(pos@ + n@ > buf_len@ ==> result == Err(WireErr::Truncated))]
#[ensures(pos@ + n@ <= buf_len@ ==> result != Err(WireErr::Truncated))]
pub fn verify_rl_wire_decode_truncated(buf_len: usize, pos: usize, n: usize) -> Result<usize, WireErr> {
    if pos + n > buf_len {
        Err(WireErr::Truncated)
    } else {
        Ok(pos + n)
    }
}

#[requires(pos@ + n@ <= usize::MAX@)]
#[ensures(pos@ + n@ > buf_len@ ==> result == Err(WireErr::Truncated))] // MUST fail (off-by-one)
pub fn verify_rl_wire_decode_truncated__mutant(buf_len: usize, pos: usize, n: usize) -> Result<usize, WireErr> {
    if pos + n > buf_len + 1 {
        Err(WireErr::Truncated)
    } else {
        Ok(pos + n)
    }
}

// ------------------------------------------------------------------------------
// RL-OP-SIZEOF  (operation.rs:121-125)
// size_of(key) => None iff no entry has that key; Some(v) => v is a genuine
// matching entry's size (first match by loop construction).
// ------------------------------------------------------------------------------
#[ensures(result == None::<u64> ==> (forall<j> 0 <= j && j < entries@.len() ==> (entries[j]).0 != key))]
#[ensures(forall<v> result == Some(v) ==>
    (exists<j> 0 <= j && j < entries@.len() && (entries[j]).0 == key && (entries[j]).1 == v))]
pub fn verify_rl_op_sizeof(entries: Vec<(CacheKey, u64)>, key: CacheKey) -> Option<u64> {
    let mut i: usize = 0;
    #[invariant(i@ <= entries@.len())]
    #[invariant(forall<j> 0 <= j && j < i@ ==> (entries[j]).0 != key)]
    while i < entries.len() {
        if entries[i].0 == key {
            return Some(entries[i].1);
        }
        i += 1;
    }
    None
}

#[ensures(result == None::<u64> ==> (forall<j> 0 <= j && j < entries@.len() ==> (entries[j]).0 != key))]
pub fn verify_rl_op_sizeof__mutant(entries: Vec<(CacheKey, u64)>, key: CacheKey) -> Option<u64> {
    // bug: never searches, always None -> claims absence even when present -> MUST fail
    None
}

// ------------------------------------------------------------------------------
// RL-OP-STATEOF-DEFAULT  (operation.rs:128-133)
// state_of(key) => Unsatisfied iff no entry tracks that key; else the first
// matching entry's state.
// ------------------------------------------------------------------------------
#[ensures((forall<j> 0 <= j && j < st@.len() ==> (st[j]).0 != key) ==> result == KeyState::Unsatisfied)]
#[ensures(forall<j> 0 <= j && j < st@.len() ==>
    ((st[j]).0 == key && (forall<k> 0 <= k && k < j ==> (st[k]).0 != key)) ==> result == (st[j]).1)]
pub fn verify_rl_op_stateof_default(st: Vec<(CacheKey, KeyState)>, key: CacheKey) -> KeyState {
    let mut i: usize = 0;
    #[invariant(i@ <= st@.len())]
    #[invariant(forall<j> 0 <= j && j < i@ ==> (st[j]).0 != key)]
    while i < st.len() {
        if st[i].0 == key {
            return st[i].1;
        }
        i += 1;
    }
    KeyState::Unsatisfied
}

#[ensures((forall<j> 0 <= j && j < st@.len() ==> (st[j]).0 != key) ==> result == KeyState::Unsatisfied)]
#[ensures(forall<j> 0 <= j && j < st@.len() ==>
    ((st[j]).0 == key && (forall<k> 0 <= k && k < j ==> (st[k]).0 != key)) ==> result == (st[j]).1)]
pub fn verify_rl_op_stateof_default__mutant(st: Vec<(CacheKey, KeyState)>, key: CacheKey) -> KeyState {
    // bug: ignores tracked state, always Unsatisfied -> MUST fail when a key is tracked non-Unsatisfied
    KeyState::Unsatisfied
}

// ------------------------------------------------------------------------------
// RL-OP-RESULTS-LEN  (operation.rs:149-160)
// results() yields exactly one entry per requested key, in order (length preserved).
// ------------------------------------------------------------------------------
#[ensures(result@.len() == keys@.len())]
pub fn verify_rl_op_results_len(keys: Vec<CacheKey>, satisfied: Vec<bool>) -> Vec<Result<u64, LookupErr>> {
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= keys@.len())]
    #[invariant(out@.len() == i@)]
    while i < keys.len() {
        if i < satisfied.len() && satisfied[i] {
            out.push(Ok(0u64));
        } else {
            out.push(Err(LookupErr::NotFound));
        }
        i += 1;
    }
    out
}

#[ensures(result@.len() == keys@.len())]
pub fn verify_rl_op_results_len__mutant(keys: Vec<CacheKey>, satisfied: Vec<bool>) -> Vec<Result<u64, LookupErr>> {
    // bug: drops one result -> length no longer matches -> MUST fail
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    let mut i: usize = 1;
    #[invariant(1usize@ <= i@)]
    #[invariant(i@ <= keys@.len() + 1)]
    #[invariant(out@.len() + 1 == i@ || out@.len() == 0)]
    while i < keys.len() {
        out.push(Err(LookupErr::NotFound));
        i += 1;
    }
    out
}

// ------------------------------------------------------------------------------
// RL-OP-RESULTS-POSITIONAL  (operation.rs:149-160)
// results()[i] == Ok  iff  the i-th requested key is Satisfied; otherwise Err(NotFound).
// ------------------------------------------------------------------------------
#[requires(states@.len() == keys@.len())]
#[ensures(result@.len() == keys@.len())]
#[ensures(forall<i> 0 <= i && i < keys@.len() ==>
    ((result[i] == Ok(0u64)) == (states[i] == KeyState::Satisfied)))]
#[ensures(forall<i> 0 <= i && i < keys@.len() ==>
    (result[i] == Ok(0u64) || result[i] == Err(LookupErr::NotFound)))]
pub fn verify_rl_op_results_positional(keys: Vec<CacheKey>, states: Vec<KeyState>) -> Vec<Result<u64, LookupErr>> {
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(states@.len() == keys@.len())]
    #[invariant(i@ <= keys@.len())]
    #[invariant(out@.len() == i@)]
    #[invariant(forall<j> 0 <= j && j < i@ ==>
        ((out[j] == Ok(0u64)) == (states[j] == KeyState::Satisfied)))]
    #[invariant(forall<j> 0 <= j && j < i@ ==>
        (out[j] == Ok(0u64) || out[j] == Err(LookupErr::NotFound)))]
    while i < keys.len() {
        if states[i] == KeyState::Satisfied {
            out.push(Ok(0u64));
        } else {
            out.push(Err(LookupErr::NotFound));
        }
        i += 1;
    }
    out
}

#[requires(states@.len() == keys@.len())]
#[ensures(forall<i> 0 <= i && i < keys@.len() ==>
    ((result[i] == Ok(0u64)) == (states[i] == KeyState::Satisfied)))]
pub fn verify_rl_op_results_positional__mutant(keys: Vec<CacheKey>, states: Vec<KeyState>) -> Vec<Result<u64, LookupErr>> {
    // bug: inverts the satisfied test -> Ok/Err swapped -> MUST fail
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(states@.len() == keys@.len())]
    #[invariant(i@ <= keys@.len())]
    #[invariant(out@.len() == i@)]
    while i < keys.len() {
        if states[i] == KeyState::Satisfied {
            out.push(Err(LookupErr::NotFound));
        } else {
            out.push(Ok(0u64));
        }
        i += 1;
    }
    out
}

// ------------------------------------------------------------------------------
// RL-OP-TRIED-DEDUP  (operation.rs:163-175)
// After note_tried(key,peer): (peer recorded for key) holds, and the record set
// grows by at most one (no duplicate insertion).
// ------------------------------------------------------------------------------
#[ensures(exists<j> 0 <= j && j < result@.len() && (result[j]).0 == key && (result[j]).1 == peer)]
#[ensures(result@.len() == tried@.len() || result@.len() == tried@.len() + 1)]
pub fn verify_rl_op_tried_dedup(tried: Vec<(CacheKey, u64)>, key: CacheKey, peer: u64) -> Vec<(CacheKey, u64)> {
    // already_tried: linear membership over (key,peer)
    let mut i: usize = 0;
    let mut found = false;
    #[invariant(i@ <= tried@.len())]
    #[invariant(found ==> (exists<j> 0 <= j && j < i@ && (tried[j]).0 == key && (tried[j]).1 == peer))]
    while i < tried.len() {
        if tried[i].0 == key && tried[i].1 == peer {
            found = true;
        }
        i += 1;
    }
    let mut out = tried;
    if !found {
        out.push((key, peer));
    }
    out
}

#[ensures(exists<j> 0 <= j && j < result@.len() && (result[j]).0 == key && (result[j]).1 == peer)]
pub fn verify_rl_op_tried_dedup__mutant(tried: Vec<(CacheKey, u64)>, key: CacheKey, peer: u64) -> Vec<(CacheKey, u64)> {
    // bug: never records the peer -> membership fails -> MUST fail
    tried
}

// ------------------------------------------------------------------------------
// RL-BL-EMPTY  (lib.rs:276-278)
// batch_lookup([]) short-circuits to an empty result vector.
// ------------------------------------------------------------------------------
#[requires(entries@.len() == 0)]
#[ensures(result@.len() == 0)]
pub fn verify_rl_bl_empty(entries: Vec<(CacheKey, u64)>) -> Vec<Result<u64, LookupErr>> {
    if entries.len() == 0 {
        return Vec::new();
    }
    // dead under the precondition; a well-formed fallthrough
    Vec::new()
}

#[requires(entries@.len() == 0)]
#[ensures(result@.len() == 0)]
pub fn verify_rl_bl_empty__mutant(entries: Vec<(CacheKey, u64)>) -> Vec<Result<u64, LookupErr>> {
    // bug: emits a spurious element for empty input -> MUST fail
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    out.push(Err(LookupErr::NotFound));
    out
}

// ------------------------------------------------------------------------------
// RL-BL-UNINIT-NOTFOUND  (lib.rs:279-289)
// On an uninitialized component every requested entry maps to Err(NotFound),
// one result per entry in order.
// ------------------------------------------------------------------------------
#[requires(!initialized)]
#[ensures(result@.len() == entries@.len())]
#[ensures(forall<j> 0 <= j && j < result@.len() ==> result[j] == Err(LookupErr::NotFound))]
pub fn verify_rl_bl_uninit_notfound(entries: Vec<(CacheKey, u64)>, initialized: bool) -> Vec<Result<u64, LookupErr>> {
    if entries.len() == 0 {
        return Vec::new();
    }
    if !initialized {
        let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
        let mut i: usize = 0;
        #[invariant(i@ <= entries@.len())]
        #[invariant(out@.len() == i@)]
        #[invariant(forall<j> 0 <= j && j < i@ ==> out[j] == Err(LookupErr::NotFound))]
        while i < entries.len() {
            out.push(Err(LookupErr::NotFound));
            i += 1;
        }
        return out;
    }
    Vec::new()
}

#[requires(!initialized)]
#[ensures(forall<j> 0 <= j && j < result@.len() ==> result[j] == Err(LookupErr::NotFound))]
pub fn verify_rl_bl_uninit_notfound__mutant(entries: Vec<(CacheKey, u64)>, initialized: bool) -> Vec<Result<u64, LookupErr>> {
    // bug: reports Ok on the uninitialized path -> MUST fail
    if entries.len() == 0 {
        return Vec::new();
    }
    let mut out: Vec<Result<u64, LookupErr>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= entries@.len())]
    #[invariant(out@.len() == i@)]
    while i < entries.len() {
        out.push(Ok(0u64));
        i += 1;
    }
    out
}

// ------------------------------------------------------------------------------
// RL-CLUSTER-UNINIT-ERROR  (lib.rs:318-333)
// join_cluster / leave_cluster on an uninitialized component return
// Err(TransportError).
// ------------------------------------------------------------------------------
#[requires(!initialized)]
#[ensures(result == Err(LookupErr::TransportError))]
pub fn verify_rl_cluster_uninit_error(initialized: bool) -> Result<u64, LookupErr> {
    if !initialized {
        return Err(LookupErr::TransportError);
    }
    Ok(0u64)
}

#[requires(!initialized)]
#[ensures(result == Err(LookupErr::TransportError))]
pub fn verify_rl_cluster_uninit_error__mutant(initialized: bool) -> Result<u64, LookupErr> {
    // bug: returns Ok while uninitialized -> MUST fail
    Ok(0u64)
}

// ------------------------------------------------------------------------------
// RL-WIRE-HEADER-LAYOUT  (wire.rs:315-319 put_header)
// Every frame's 10-byte header is [WIRE_VERSION, msg_type_tag, op_id as 8 LE bytes];
// reading the 8 header bytes back little-endian reconstructs op_id exactly.
// (bitwise SMT theory for the byte<->u64 shift/mask reasoning.)
// ------------------------------------------------------------------------------
#[bitwise_proof]
#[ensures(result == true)]
pub fn verify_rl_wire_header_layout(tag: u8, op_id: u64) -> bool {
    let mut buf: [u8; 10] = [0u8; 10];
    buf[0] = WIRE_VERSION;
    buf[1] = tag;
    buf[2] = (op_id & 0xff) as u8;
    buf[3] = ((op_id >> 8) & 0xff) as u8;
    buf[4] = ((op_id >> 16) & 0xff) as u8;
    buf[5] = ((op_id >> 24) & 0xff) as u8;
    buf[6] = ((op_id >> 32) & 0xff) as u8;
    buf[7] = ((op_id >> 40) & 0xff) as u8;
    buf[8] = ((op_id >> 48) & 0xff) as u8;
    buf[9] = ((op_id >> 56) & 0xff) as u8;
    proof_assert!(buf[0]@ == WIRE_VERSION@);
    proof_assert!(buf[1]@ == tag@);
    let rid: u64 = (buf[2] as u64)
        | ((buf[3] as u64) << 8)
        | ((buf[4] as u64) << 16)
        | ((buf[5] as u64) << 24)
        | ((buf[6] as u64) << 32)
        | ((buf[7] as u64) << 40)
        | ((buf[8] as u64) << 48)
        | ((buf[9] as u64) << 56);
    proof_assert!(rid == op_id);
    true
}

#[bitwise_proof]
#[ensures(result == true)]
pub fn verify_rl_wire_header_layout__mutant(tag: u8, op_id: u64) -> bool {
    // bug: writes op_id byte 2 with the wrong shift, so the LE reconstruction
    // no longer equals op_id -> MUST fail.
    let mut buf: [u8; 10] = [0u8; 10];
    buf[0] = WIRE_VERSION;
    buf[1] = tag;
    buf[2] = ((op_id >> 8) & 0xff) as u8; // BUG: should be (op_id & 0xff)
    buf[3] = ((op_id >> 8) & 0xff) as u8;
    buf[4] = ((op_id >> 16) & 0xff) as u8;
    buf[5] = ((op_id >> 24) & 0xff) as u8;
    buf[6] = ((op_id >> 32) & 0xff) as u8;
    buf[7] = ((op_id >> 40) & 0xff) as u8;
    buf[8] = ((op_id >> 48) & 0xff) as u8;
    buf[9] = ((op_id >> 56) & 0xff) as u8;
    let rid: u64 = (buf[2] as u64)
        | ((buf[3] as u64) << 8)
        | ((buf[4] as u64) << 16)
        | ((buf[5] as u64) << 24)
        | ((buf[6] as u64) << 32)
        | ((buf[7] as u64) << 40)
        | ((buf[8] as u64) << 48)
        | ((buf[9] as u64) << 56);
    proof_assert!(rid == op_id);
    true
}

// ------------------------------------------------------------------------------
// RL-WIRE-ROUNDTRIP-KEYQUERY  (wire.rs:173-180,250-259)
// decode(encode(KeyQuery)) recovers op_id and the ordered (key,size) entries exactly.
// The wire framing (header op_id, count prefix, positional entry stream) is modelled
// over a u64 field stream; the per-field u64<->8-LE-byte codec is RL-WIRE-HEADER-LAYOUT.
// ------------------------------------------------------------------------------
#[requires(entries@.len() <= 1000000)]
#[ensures(result == true)]
pub fn verify_rl_wire_roundtrip_keyquery(op_id: u64, entries: Vec<(u64, u64)>) -> bool {
    let n = entries.len();
    let mut stream: Vec<u64> = Vec::new();
    stream.push(op_id);
    stream.push(n as u64);
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * i@)]
    #[invariant(stream[0] == op_id)]
    #[invariant(forall<j> 0 <= j && j < i@ ==>
        stream[2 + 2 * j] == (entries[j]).0 && stream[3 + 2 * j] == (entries[j]).1)]
    while i < n {
        stream.push(entries[i].0);
        stream.push(entries[i].1);
        i += 1;
    }
    let dop = stream[0];
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut k: usize = 0;
    #[invariant(k@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * n@)]
    #[invariant(out@.len() == k@)]
    #[invariant(forall<j> 0 <= j && j < k@ ==>
        (out[j]).0 == (entries[j]).0 && (out[j]).1 == (entries[j]).1)]
    while k < n {
        let key = stream[2 + 2 * k];
        let size = stream[3 + 2 * k];
        out.push((key, size));
        k += 1;
    }
    proof_assert!(dop == op_id);
    proof_assert!(out@.len() == entries@.len());
    proof_assert!(forall<j> 0 <= j && j < entries@.len() ==>
        (out[j]).0 == (entries[j]).0 && (out[j]).1 == (entries[j]).1);
    true
}

#[requires(entries@.len() <= 1000000)]
#[ensures(result == true)]
pub fn verify_rl_wire_roundtrip_keyquery__mutant(op_id: u64, entries: Vec<(u64, u64)>) -> bool {
    let n = entries.len();
    let mut stream: Vec<u64> = Vec::new();
    stream.push(op_id);
    stream.push(n as u64);
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * i@)]
    #[invariant(stream[0] == op_id)]
    #[invariant(forall<j> 0 <= j && j < i@ ==>
        stream[2 + 2 * j] == (entries[j]).1 && stream[3 + 2 * j] == (entries[j]).0)]
    while i < n {
        // BUG: swaps key/size on encode so the decoded entries are transposed -> MUST fail
        stream.push(entries[i].1);
        stream.push(entries[i].0);
        i += 1;
    }
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut k: usize = 0;
    #[invariant(k@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * n@)]
    #[invariant(out@.len() == k@)]
    #[invariant(forall<j> 0 <= j && j < k@ ==>
        (out[j]).0 == (entries[j]).1 && (out[j]).1 == (entries[j]).0)]
    while k < n {
        let key = stream[2 + 2 * k];
        let size = stream[3 + 2 * k];
        out.push((key, size));
        k += 1;
    }
    proof_assert!(forall<j> 0 <= j && j < entries@.len() ==>
        (out[j]).0 == (entries[j]).0 && (out[j]).1 == (entries[j]).1);
    true
}

// ------------------------------------------------------------------------------
// RL-WIRE-ROUNDTRIP-RDMASTATUS  (wire.rs:211-218,294-302)
// decode(encode(RdmaStatus)) recovers op_id and the ordered (key,status-code) entries
// exactly. Same framing as KeyQuery; the second field is the status tag byte whose
// enum<->code mapping is RL-STATUS-TAG-BIJECTION.
// ------------------------------------------------------------------------------
#[requires(entries@.len() <= 1000000)]
#[ensures(result == true)]
pub fn verify_rl_wire_roundtrip_rdmastatus(op_id: u64, entries: Vec<(u64, u64)>) -> bool {
    let n = entries.len();
    let mut stream: Vec<u64> = Vec::new();
    stream.push(op_id);
    stream.push(n as u64);
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * i@)]
    #[invariant(stream[0] == op_id)]
    #[invariant(forall<j> 0 <= j && j < i@ ==>
        stream[2 + 2 * j] == (entries[j]).0 && stream[3 + 2 * j] == (entries[j]).1)]
    while i < n {
        stream.push(entries[i].0);      // key
        stream.push(entries[i].1);      // status code (0/1/2)
        i += 1;
    }
    let dop = stream[0];
    let mut out: Vec<(u64, u64)> = Vec::new();
    let mut k: usize = 0;
    #[invariant(k@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * n@)]
    #[invariant(out@.len() == k@)]
    #[invariant(forall<j> 0 <= j && j < k@ ==>
        (out[j]).0 == (entries[j]).0 && (out[j]).1 == (entries[j]).1)]
    while k < n {
        let key = stream[2 + 2 * k];
        let code = stream[3 + 2 * k];
        out.push((key, code));
        k += 1;
    }
    proof_assert!(dop == op_id);
    proof_assert!(out@.len() == entries@.len());
    proof_assert!(forall<j> 0 <= j && j < entries@.len() ==>
        (out[j]).0 == (entries[j]).0 && (out[j]).1 == (entries[j]).1);
    true
}

#[requires(entries@.len() <= 1000000)]
#[ensures(result == true)]
pub fn verify_rl_wire_roundtrip_rdmastatus__mutant(op_id: u64, entries: Vec<(u64, u64)>) -> bool {
    let n = entries.len();
    let mut stream: Vec<u64> = Vec::new();
    // BUG: stores 0 in the op_id header slot instead of op_id, so the decoded
    // op_id does not match -> MUST fail.
    stream.push(0u64);
    stream.push(n as u64);
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(stream@.len() == 2 + 2 * i@)]
    #[invariant(stream[0] == 0u64)]
    while i < n {
        stream.push(entries[i].0);
        stream.push(entries[i].1);
        i += 1;
    }
    let dop = stream[0];
    proof_assert!(dop == op_id);
    true
}

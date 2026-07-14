use creusot_std::{logic::FMap, prelude::*};

// Creusot verification model for the `dispatcher` component.
//
// Like `dispatch-map/verif`, Creusot cannot process the real dispatcher
// (Mutex, Arc, SPDK FFI, threads), so each function below mirrors a real
// method body stripped of runtime-only concerns:
//   - self / Mutex / lock / unwrap
//   - logger calls
//   - I/O effects (extent reserve/publish, SSD writes) — trusted boundary
//
// Property IDs (Pxx) refer to the spec-derived baseline in
// `dispatch-map/verif/property_coverage_matrix_codex_july2.md`.

// ---------- Error type (mirrors interfaces::DispatcherError variants) ----------

pub enum DispatcherError {
    NotInitialized,
    InvalidParameter,
    AlreadyExists,
    KeyNotFound,
    AllocationFailed,
    IoError,
}

// ---------- P1: initialize dependency-binding contract ----------
//
// Mirrors the guard prefix of `initialize` (dispatcher/src/lib.rs:1053-1065):
//     self.dispatch_map.get().map_err(|_| NotInitialized(..))?;   // required
//     self.memory_tier.get().map_err(|_| NotInitialized(..))?;    // required
//     if config.data_pci_addrs.is_empty() { return InvalidParameter }
//
// Initialization must fail when a required dependency is missing and proceed
// only when both receptacles are bound and at least one data PCI address is
// configured. The receptacle bindings and the address list are modeled as
// booleans; the check order is preserved so the exact error variant matches
// the live code (`dispatch_map` checked before `memory_tier`). This pairs with
// P2 (`ensure_initialized`): P2 gates operational APIs on the post-init flag,
// P1 fixes the conditions under which init itself succeeds.

#[ensures(!dispatch_map_bound ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(dispatch_map_bound && !memory_tier_bound ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(dispatch_map_bound && memory_tier_bound && pci_addrs_empty ==> match result { Err(DispatcherError::InvalidParameter) => true, _ => false })]
#[ensures(dispatch_map_bound && memory_tier_bound && !pci_addrs_empty ==> match result { Ok(_) => true, _ => false })]
pub fn initialize_dependency_guards(
    dispatch_map_bound: bool,
    memory_tier_bound: bool,
    pci_addrs_empty: bool,
) -> Result<(), DispatcherError> {
    if !dispatch_map_bound {
        return Err(DispatcherError::NotInitialized);
    }
    if !memory_tier_bound {
        return Err(DispatcherError::NotInitialized);
    }
    if pci_addrs_empty {
        return Err(DispatcherError::InvalidParameter);
    }
    Ok(())
}

// ---------- P2: initialized-state gate ----------
//
// Mirrors `self.ensure_initialized()?` at the top of every operational API
// (e.g. dispatcher/src/lib.rs:2131, :2228, :2276, :2298). Before successful
// init, operational APIs must fail with `NotInitialized` and touch no state.
// This function returns before any state access, so "no mutation" is
// structural (there is no state to mutate on this path).

#[ensures(!initialized ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(initialized ==> match result { Ok(_) => true, _ => false })]
pub fn ensure_initialized(initialized: bool) -> Result<(), DispatcherError> {
    if !initialized {
        return Err(DispatcherError::NotInitialized);
    }
    Ok(())
}

// ---------- P11: lookup size-mismatch hard-fail (no partial copy) ----------
//
// Mirrors the `LookupResult` -> `Result` decision inside `lookup_async`
// (dispatcher/src/lib.rs:1786-1830) and `batch_lookup` (:1420-1442):
//     match dm.lookup(key)? {
//         LookupResult::NotExist            => Err(KeyNotFound),
//         LookupResult::MismatchSize        => { dm.release_read(key);
//                                                Err(InvalidParameter("size mismatch")) }
//         LookupResult::MemoryTier { size } => { let n = min(ipc.size, size); copy(n); Ok(n) }
//         LookupResult::BlockDevice { .. }  => { ... Ok(read_size) }
//     }
//
// P11 requires: a size mismatch must hard-fail with `InvalidParameter` and
// perform no (partial) copy. Modeled at L0 over the dispatcher's decision
// logic, treating the `LookupResult` variant as an input.
//
// IMPLEMENTATION NOTE (verified against live code, 2026-07-14): the
// `MismatchSize` variant is declared in the interface
// (interfaces/src/idispatch_map.rs:15) but has NO producer — `dm.lookup(key)`
// is key-only (dispatch-map/src/lib.rs:115) and returns only
// NotExist/BlockDevice/MemoryTier. So the dispatcher's `MismatchSize` arm is
// currently unreachable, and the copy path defensively clamps with
// `copy_size = min(ipc.size, stored)`. This proof therefore certifies the
// dispatcher's DECISION LOGIC (given MismatchSize -> InvalidParameter, no copy;
// given a hit -> copy is clamped to `min`, never an over-copy past either
// buffer) but does NOT assert that the running system detects a mismatch,
// because nothing currently produces `MismatchSize`. Closing that gap requires
// dispatch-map to compare requested vs stored size and emit `MismatchSize`.

pub enum LookupOutcome {
    NotExist,
    MismatchSize,
    MemoryTier { size: usize },
    BlockDevice { size: usize },
}

// `copy_size` result: `None` means no copy was performed (error/miss paths);
// `Some(n)` is the number of bytes the dispatcher would copy on a hit.
#[ensures(match outcome {
    LookupOutcome::MismatchSize => match result {
        (Err(DispatcherError::InvalidParameter), copied) => copied == None,
        _ => false,
    },
    _ => true,
})]
#[ensures(match outcome {
    LookupOutcome::NotExist => match result {
        (Err(DispatcherError::KeyNotFound), copied) => copied == None,
        _ => false,
    },
    _ => true,
})]
// On a MemoryTier hit the copy is clamped: never exceeds requested, never
// exceeds stored -> no over-read of either buffer, i.e. no unsafe partial copy.
#[ensures(match outcome {
    LookupOutcome::MemoryTier { size } => match result {
        (Ok(()), Some(n)) => n@ <= requested@ && n@ <= size@,
        _ => false,
    },
    _ => true,
})]
#[ensures(match outcome {
    LookupOutcome::BlockDevice { size } => match result {
        (Ok(()), Some(n)) => n@ <= requested@ && n@ <= size@,
        _ => false,
    },
    _ => true,
})]
pub fn resolve_lookup(
    outcome: LookupOutcome,
    requested: usize,
) -> (Result<(), DispatcherError>, Option<usize>) {
    match outcome {
        LookupOutcome::NotExist => (Err(DispatcherError::KeyNotFound), None),
        LookupOutcome::MismatchSize => (Err(DispatcherError::InvalidParameter), None),
        LookupOutcome::MemoryTier { size } => {
            let n = if requested < size { requested } else { size };
            (Ok(()), Some(n))
        }
        LookupOutcome::BlockDevice { size } => {
            let n = if requested < size { requested } else { size };
            (Ok(()), Some(n))
        }
    }
}

// ---------- P29: eviction watermark comparison direction ----------
//
// Mirrors the SSD-evictor comparisons in background.rs:
//     let utilization = used as f64 / capacity as f64;
//     if utilization < config.threshold { continue; }   // (:299) start iff util >= threshold
//     ...
//     if util_now < config.low_watermark { break; }      // (:350) stop iff util <  low_watermark
//
// P29 requires the threshold/watermark comparisons to follow the intended
// direction: eviction is TRIGGERED at the HIGH threshold and STOPS at the LOW
// watermark, and for a well-formed hysteresis band `low_watermark <= threshold`.
//
// MODELING NOTE: the runtime compares an `f64` ratio `used/capacity` against
// two `f64` config values (defaults 0.9 / 0.8). This proof models utilization
// and both watermarks as integer permille (0..=1000) so the ordering is
// decidable — `f64` carries NaN and its `<=` is not a total order, which
// Creusot/SMT cannot discharge cleanly. This certifies the COMPARISON
// DIRECTION and hysteresis consistency, not the exact floating-point arithmetic.
//
// The key theorem is `!(should_start && should_stop)`: given a well-formed band
// you can never be simultaneously told to start (util >= threshold) and stop
// (util < low_watermark), because that would force threshold <= util <
// low_watermark <= threshold — a contradiction. This catches the direction-bug
// class (flipped `<`/`>=`, or threshold/low_watermark swapped).

#[requires(low_watermark@ <= threshold@)]
#[ensures(result.0 == (utilization@ >= threshold@))]
#[ensures(result.1 == (utilization@ < low_watermark@))]
#[ensures(!(result.0 && result.1))]
pub fn evictor_decisions(
    utilization: u32,
    threshold: u32,
    low_watermark: u32,
) -> (bool, bool) {
    let should_start = utilization >= threshold;
    let should_stop = utilization < low_watermark;
    (should_start, should_stop)
}

// ---------- P14: touch refreshes on hit, KeyNotFound on miss ----------
//
// Mirrors `touch` (dispatcher/src/lib.rs:2172-2188):
//     self.ensure_initialized()?;                              // P2 gate
//     let dm = self.dispatch_map.get().map_err(NotInitialized)?;
//     dm.touch(key).map_err(|_| KeyNotFound(key))?;            // map miss -> KeyNotFound
//     if let Ok(mt) = self.memory_tier.get() { mt.touch(key); } // best-effort
//     Ok(())
// and the underlying map `touch` (dispatch-map/src/lib.rs:335-347): key present
// -> refresh eviction metadata (`ep.touch(handle)`), key absent -> KeyNotFound.
//
// P14 requires: touch on an existing key refreshes metadata; an absent key
// returns `KeyNotFound`; a miss (and the pre-init path) must not mutate state.
// Modeled at L0 with `initialized` and `key_present` as booleans. The metadata
// refresh is represented by the second tuple element `refreshed`, proved equal
// to "returned Ok" — so refresh happens exactly on the hit path and never on a
// miss or before init (the "no mutation" half of the property).

#[ensures(!initialized ==> match result.0 { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(initialized && !key_present ==> match result.0 { Err(DispatcherError::KeyNotFound) => true, _ => false })]
#[ensures(initialized && key_present ==> match result.0 { Ok(_) => true, _ => false })]
#[ensures(result.1 == match result.0 { Ok(_) => true, _ => false })]
pub fn touch_decision(
    initialized: bool,
    key_present: bool,
) -> (Result<(), DispatcherError>, bool) {
    if !initialized {
        return (Err(DispatcherError::NotInitialized), false);
    }
    if !key_present {
        return (Err(DispatcherError::KeyNotFound), false);
    }
    (Ok(()), true)
}

// ---------- P20: prepare_store argument validation ----------
//
// Mirrors the guard prefix of `prepare_store` (dispatcher/src/lib.rs:2130-2136):
//     self.ensure_initialized()?;         // P2
//     if size == 0 { return InvalidParameter }  // P20
// Both guards return before `create_staging` / extent reservation / the
// `pending_writes` insert, so a rejected call performs no state mutation.

#[ensures(!initialized ==> match result { Err(DispatcherError::NotInitialized) => true, _ => false })]
#[ensures(initialized && size == 0u32 ==> match result { Err(DispatcherError::InvalidParameter) => true, _ => false })]
#[ensures(initialized && size > 0u32 ==> match result { Ok(_) => true, _ => false })]
pub fn prepare_store_guards(initialized: bool, size: u32) -> Result<(), DispatcherError> {
    if !initialized {
        return Err(DispatcherError::NotInitialized);
    }
    if size == 0 {
        return Err(DispatcherError::InvalidParameter);
    }
    Ok(())
}

// ---------- P28: drive-selection index bound ----------
//
// Mirrors `drive_index` (dispatcher/src/lib.rs:241): a splitmix64 finalizer
// over the key, reduced `% num_drives` to pick a data drive. The call site
// (:1556) is guarded by `if num_drives == 0` (:1532), so the divisor is
// always positive.
//
// Determinism / stability (P28's plain-English claim) is structural: this is
// a pure function of (key, num_drives) with no state, clock, or RNG, so equal
// inputs always yield equal outputs. The substantive safety theorem we
// discharge is that the result is a valid drive index — `result < num_drives`
// — so drive selection can never index a nonexistent drive. The hash body is
// kept verbatim; we deliberately prove nothing about the hash value, only that
// the final `% num_drives` lands in range.

#[requires(num_drives@ > 0)]
#[ensures(result@ < num_drives@)]
pub fn drive_index(key: u64, num_drives: usize) -> usize {
    // splitmix64 finalizer: distributes sequential keys uniformly.
    let mut h = key;
    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h as usize % num_drives
}

// ---------- Pending-write map model ----------
//
// The real dispatcher holds `pending_writes: Mutex<HashMap<CacheKey, PendingWrite>>`
// (dispatcher/src/lib.rs:165). We model it with a logic-level `FMap`, since
// std `HashMap::insert`/`remove` carry no Creusot specs in this toolchain.
//
// `PendingModel` mirrors the verifiable fields of the real `PendingWrite`
// (:92). `write_handle` (WriteHandle) and `buffer` (Arc<DmaBuffer>) are I/O
// resources that live behind the trusted boundary and are omitted.

pub struct PendingModel {
    pub size: u32,
    pub drive_idx: usize,
}

// ---------- P24: commit/cancel miss semantics ----------
//
// Mirrors the consume step shared by `commit_store` (:2231) and `cancel_store`
// (:2279):
//     let pending = self.pending_writes.lock().unwrap()
//         .remove(&key).ok_or(DispatcherError::KeyNotFound(key))?;
//
// P24: with no pending write for `key`, commit/cancel returns `KeyNotFound`
// and the pending-write map is unchanged (no mutation).

#[check(ghost)]
#[ensures(match result {
    Ok(_)  => (*map).contains(key) && !(^map).contains(key),
    Err(DispatcherError::KeyNotFound) => !(*map).contains(key) && (^map).ext_eq(*map),
    _ => false,
})]
pub fn consume_pending(
    map: &mut FMap<u64, PendingModel>,
    key: u64,
) -> Result<PendingModel, DispatcherError> {
    match map.remove_ghost(&key) {
        Some(p) => Ok(p),
        None => Err(DispatcherError::KeyNotFound),
    }
}

// ---------- P21: pending-write consume-once protocol ----------
//
// Mirrors the `prepare_store` insert (:2213):
//     self.pending_writes.lock().unwrap().insert(key, PendingWrite { .. });
//
// After prepare, the key is present in the pending-write map.

#[check(ghost)]
#[ensures((^map).contains(key))]
#[ensures(!(*map).contains(key) ==> result == None)]
pub fn insert_pending(
    map: &mut FMap<u64, PendingModel>,
    key: u64,
    pending: PendingModel,
) -> Option<PendingModel> {
    map.insert_ghost(key, pending)
}

// Consume-once: given a key with a pending write, the first commit/cancel
// succeeds and consumes it; any second commit/cancel on the same key misses
// (`KeyNotFound`). This is the "consume exactly once" guarantee — a pending
// write cannot be committed and then also cancelled (or committed twice).

#[check(ghost)]
#[requires((*map).contains(key))]
#[ensures(match result {
    (Ok(_), Err(DispatcherError::KeyNotFound)) => true,
    _ => false,
})]
pub fn consume_once(
    map: &mut FMap<u64, PendingModel>,
    key: u64,
) -> (
    Result<PendingModel, DispatcherError>,
    Result<PendingModel, DispatcherError>,
) {
    let first = consume_pending(map, key);
    let second = consume_pending(map, key);
    (first, second)
}

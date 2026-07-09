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

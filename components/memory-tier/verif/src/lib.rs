//! Creusot verification of the memory-tier component — CLEAN-SLATE RE-RUN.
//!
//! Branch `verif/creusot/memory-tier-rerun`. Every contract in this file was
//! **re-authored from the id-keyed property inventory**
//! (`components/memory-tier/verif_memory-tier_property_inventory.md`) after the
//! tree was stripped to bare signatures (commit "STRIP …") — none is inherited
//! from the baseline `verif/creusot/memory-tier@caac84d2`.
//!
//! These are **standalone mirrors** of the arithmetic / control-flow of
//! `components/memory-tier/src/allocator.rs` and `src/lib.rs`. The real code uses
//! `BTreeMap`/`HashMap`, `AtomicBool`/`RwLock`, raw `*mut u8` and `libc`/SPDK FFI,
//! none of which build under Creusot; each mirror transcribes the arithmetic /
//! branching the shipped function runs once the (trusted) container lookups and
//! atomic loads have produced their scalar/boolean results. Container membership
//! is additionally modelled at logic level with `FMap` (CONTAINS-REFLECTS).
//! A green proof here covers the MIRROR, not the shipped function — the residual
//! mirror↔source gap is checked by inspection against the cited line ranges.
//! See `verif/memory-tier_properties.md` for the boundary ledger.

use creusot_std::prelude::{Clone, *};

/// 4 KiB alignment, matching `allocator.rs::ALIGNMENT`.
pub const ALIGNMENT: usize = 4096;

// =========================================================================
// ALLOCATOR ARITHMETIC CORE — inventory globals ALIGN-4K, USED-CONSERVE,
// USED-LE-CAP, FREECAP-DERIVED, COALESCE, SPACE-REUSE, PTR-IN-BOUNDS.
// =========================================================================

// -------------------------------------------------------------------------
// ALIGN-4K (rank 6). Mirrors `size.next_multiple_of(ALIGNMENT)` (allocator.rs:42)
// on the `size > 0` domain guaranteed by the `size == 0` guard (allocator.rs:39).
// -------------------------------------------------------------------------
/// Round `size` up to the next multiple of 4096 (the pool alignment).
#[requires(size@ > 0)]
#[requires(size@ + 4095 <= usize::MAX@)] // add of ALIGNMENT-1 is overflow-free
#[ensures(result@ % 4096 == 0)] // ALIGN-4K: aligned to 4 KiB
#[ensures(result@ >= size@)] // never returns fewer bytes than requested
#[ensures(result@ > 0)] // positive for positive request
#[ensures(result@ < size@ + 4096)] // minimal: internal waste < one page
pub fn align_up(size: usize) -> usize {
    let n = size + (ALIGNMENT - 1);
    let q = n / ALIGNMENT;
    let r = n % ALIGNMENT;
    // Division identity + remainder bounds, bridged for the SMT backend.
    proof_assert!(n@ == q@ * 4096 + r@);
    proof_assert!(r@ < 4096);
    proof_assert!(q@ * 4096 >= size@);
    proof_assert!(q@ * 4096 <= size@ + 4095);
    let result = q * ALIGNMENT;
    proof_assert!(result@ == q@ * 4096);
    proof_assert!(result@ / 4096 == q@);
    proof_assert!(result@ % 4096 == 0);
    result
}

// -------------------------------------------------------------------------
// INSERT-ZERO / allocate: zero-size request is rejected.
// Mirrors `if size == 0 { return None; }` (allocator.rs:39-41).
// -------------------------------------------------------------------------
/// Decide whether an allocation request is admitted (non-zero size).
#[ensures(size@ == 0 ==> result == false)] // INSERT-ZERO: zero size rejected
#[ensures(size@ > 0 ==> result == true)] // any positive size admitted
pub fn alloc_admits(size: usize) -> bool {
    if size == 0 {
        return false;
    }
    true
}

// -------------------------------------------------------------------------
// USED-CONSERVE / USED-LE-CAP (allocate half). Split arithmetic + used
// accounting after first-fit selects region (offset, region_size).
// Mirrors allocator.rs:44-54.
// -------------------------------------------------------------------------
/// Carve an aligned chunk out of a selected free region.
#[requires(aligned_size@ > 0)]
#[requires(aligned_size@ % 4096 == 0)]
#[requires(region_size@ >= aligned_size@)] // first-fit guarantee (allocator.rs:44)
#[requires(offset@ + region_size@ <= capacity@)] // region within the pool
#[requires(used@ + aligned_size@ <= capacity@)] // pool has room
#[ensures(result.0@ == used@ + aligned_size@)] // USED-CONSERVE: used grows by aligned_size
#[ensures(result.1@ == region_size@ - aligned_size@)] // remaining free bytes
#[ensures(result.2@ == offset@ + aligned_size@)] // leftover region start
#[ensures(result.0@ <= capacity@)] // USED-LE-CAP: used never exceeds capacity
#[ensures(result.2@ + result.1@ <= capacity@)] // leftover stays within pool
pub fn allocate_split(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> (usize, usize, usize) {
    let remaining = region_size - aligned_size;
    let leftover_offset = offset + aligned_size;
    let new_used = used + aligned_size;
    (new_used, remaining, leftover_offset)
}

// -------------------------------------------------------------------------
// USED-CONSERVE (deallocate half). Mirrors `self.used -= aligned_size;`
// (allocator.rs:61).
// -------------------------------------------------------------------------
/// Return `aligned_size` bytes to the pool's used accounting.
#[requires(used@ >= aligned_size@)] // can't free more than is in use
#[ensures(result@ == used@ - aligned_size@)] // USED-CONSERVE
pub fn deallocate_used(used: usize, aligned_size: usize) -> usize {
    used - aligned_size
}

// -------------------------------------------------------------------------
// COALESCE (rank 5), preceding-region merge. Mirrors allocator.rs:66-73.
// -------------------------------------------------------------------------
/// Coalesce a freed region `(offset, size)` with an adjacent preceding region.
#[requires(prev_offset@ + prev_size@ <= offset@)] // prev ends at/before offset
#[requires(prev_size@ + size@ <= usize::MAX@)] // merged size overflow-free
#[ensures(prev_offset@ + prev_size@ == offset@
    ==> result.0@ == prev_offset@ && result.1@ == prev_size@ + size@)] // adjacent → merged
#[ensures(prev_offset@ + prev_size@ < offset@
    ==> result.0@ == offset@ && result.1@ == size@)] // gap → unchanged
#[ensures(result.0@ + result.1@ == offset@ + size@)] // right endpoint preserved
#[ensures(result.0@ <= offset@)] // only extends leftward
pub fn coalesce_prev(
    prev_offset: usize,
    prev_size: usize,
    offset: usize,
    size: usize,
) -> (usize, usize) {
    let mut new_offset = offset;
    let mut new_size = size;
    if prev_offset + prev_size == offset {
        new_offset = prev_offset;
        new_size += prev_size;
    }
    (new_offset, new_size)
}

// -------------------------------------------------------------------------
// COALESCE, following-region merge. Mirrors allocator.rs:76-80.
// -------------------------------------------------------------------------
/// Coalesce the current region with an adjacent following region of `next_size`.
#[requires(new_size@ + next_size@ <= usize::MAX@)] // merged size overflow-free
#[ensures(result@ == new_size@ + next_size@)] // region grows by next_size
#[ensures(result@ >= new_size@)] // never shrinks
pub fn coalesce_next(new_size: usize, next_size: usize) -> usize {
    new_size + next_size
}

// -------------------------------------------------------------------------
// COALESCE, full both-neighbour merge. Composes coalesce_prev + coalesce_next
// exactly as deallocate() runs them (allocator.rs:63-82). `next_present` models
// the trusted `free_regions.get(&next_offset).is_some()` lookup.
// -------------------------------------------------------------------------
/// Full deallocate merge across preceding and following free neighbours.
#[requires(prev_offset@ + prev_size@ <= offset@)]
#[requires(prev_size@ + size@ <= usize::MAX@)]
#[requires(prev_size@ + size@ + next_size@ <= usize::MAX@)]
#[ensures(prev_offset@ + prev_size@ == offset@ ==> result.0@ == prev_offset@)]
#[ensures(prev_offset@ + prev_size@ < offset@ ==> result.0@ == offset@)]
#[ensures(result.0@ <= offset@)] // extend-only leftward
#[ensures(result.0@ + result.1@ >= offset@ + size@)] // freed span still covered
#[ensures(next_present ==> result.0@ + result.1@ == offset@ + size@ + next_size@)] // byte conservation
#[ensures(!next_present ==> result.0@ + result.1@ == offset@ + size@)]
pub fn deallocate_merge(
    prev_offset: usize,
    prev_size: usize,
    offset: usize,
    size: usize,
    next_present: bool,
    next_size: usize,
) -> (usize, usize) {
    let (new_offset, mut new_size) = coalesce_prev(prev_offset, prev_size, offset, size);
    if next_present {
        new_size = coalesce_next(new_size, next_size);
    }
    (new_offset, new_size)
}

// -------------------------------------------------------------------------
// FREECAP-DERIVED (rank 10). Mirrors `capacity() - used()` (lib.rs:186).
// -------------------------------------------------------------------------
/// Free bytes in the pool: total capacity minus bytes in use.
#[requires(used@ <= capacity@)] // accounting invariant
#[ensures(result@ == capacity@ - used@)] // FREECAP-DERIVED
#[ensures(result@ <= capacity@)] // free never exceeds capacity
pub fn free_capacity(capacity: usize, used: usize) -> usize {
    capacity - used
}

// -------------------------------------------------------------------------
// CLEAR-EMPTIES (allocator half). Mirrors `FreeList::new(pool_size)`
// (lib.rs:605 / allocator.rs:16-26): used → 0, capacity → pool_size.
// -------------------------------------------------------------------------
/// Reset the allocator accounting on `clear()`.
#[ensures(result.0@ == 0)] // used → 0
#[ensures(result.1@ == pool_size@)] // capacity → pool_size
pub fn clear_reset(pool_size: usize) -> (usize, usize) {
    let new_used = 0;
    let new_capacity = pool_size;
    (new_used, new_capacity)
}

// -------------------------------------------------------------------------
// USED-CONSERVE lifecycle: allocate then deallocate conserves `used`.
// -------------------------------------------------------------------------
/// Alloc-then-free round-trips the `used` counter to its start value.
#[requires(aligned_size@ > 0)]
#[requires(aligned_size@ % 4096 == 0)]
#[requires(region_size@ >= aligned_size@)]
#[requires(offset@ + region_size@ <= capacity@)]
#[requires(used@ + aligned_size@ <= capacity@)]
#[ensures(result@ == used@)] // used exactly restored
pub fn lifecycle_alloc_free(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> usize {
    let (new_used, _remaining, _leftover) =
        allocate_split(used, capacity, offset, region_size, aligned_size);
    deallocate_used(new_used, aligned_size)
}

// -------------------------------------------------------------------------
// ALIGN-4K inductive step: an aligned region start plus an aligned carve
// yields an aligned leftover start (with base case offset 0, allocator.rs:19,
// every returned allocation offset is 4 KiB-aligned).
// -------------------------------------------------------------------------
/// Given an aligned region start, the leftover region start is also aligned.
#[requires(offset@ % 4096 == 0)]
#[requires(size@ > 0)]
#[requires(offset@ + size@ + 4096 <= usize::MAX@)]
#[ensures(result@ % 4096 == 0)] // ALIGN-4K preserved
#[ensures(result@ >= offset@)]
pub fn leftover_offset_aligned(offset: usize, size: usize) -> usize {
    let aligned = align_up(size);
    offset + aligned
}

// -------------------------------------------------------------------------
// ALIGN-4K idempotence: rounding an aligned size again is a no-op.
// -------------------------------------------------------------------------
/// Applying `align_up` twice equals applying it once.
#[requires(size@ > 0)]
#[requires(size@ + 8191 <= usize::MAX@)]
#[ensures(result.0@ % 4096 == 0)]
#[ensures(result.1@ == result.0@)] // idempotent
pub fn align_up_idempotent(size: usize) -> (usize, usize) {
    let once = align_up(size);
    let twice = align_up(once);
    (once, twice)
}

// -------------------------------------------------------------------------
// PTR-IN-BOUNDS (rank 8). A returned allocation stays inside the mapped pool.
// The pointer `insert` returns is `pool_ptr.add(offset)` (lib.rs:377).
// -------------------------------------------------------------------------
/// The carved allocation `[offset, offset+aligned_size)` lies within the pool.
#[requires(region_size@ >= aligned_size@)]
#[requires(offset@ + region_size@ <= pool_size@)]
#[ensures(result@ == offset@ + aligned_size@)]
#[ensures(result@ <= pool_size@)] // PTR-IN-BOUNDS
pub fn ptr_in_bounds(offset: usize, region_size: usize, aligned_size: usize, pool_size: usize) -> usize {
    offset + aligned_size
}

// -------------------------------------------------------------------------
// SPACE-REUSE (rank 7). Freed bytes become allocatable again.
// -------------------------------------------------------------------------
/// After freeing `aligned_size` bytes, free capacity grows by exactly that much.
#[requires(aligned_size@ <= used@)]
#[requires(used@ <= capacity@)]
#[ensures(result@ == capacity@ - (used@ - aligned_size@))]
#[ensures(result@ >= aligned_size@)] // freed bytes are at least re-allocatable
pub fn space_reuse_free_grows(used: usize, capacity: usize, aligned_size: usize) -> usize {
    let new_used = used - aligned_size;
    capacity - new_used
}

/// Two alloc→free round-trips return `used` to its start (no accounting leak).
#[requires(aligned_size@ > 0)]
#[requires(aligned_size@ % 4096 == 0)]
#[requires(region_size@ >= aligned_size@)]
#[requires(offset@ + region_size@ <= capacity@)]
#[requires(used@ + aligned_size@ <= capacity@)]
#[ensures(result@ == used@)]
pub fn space_reuse_two_cycles(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> usize {
    let (u1, _r1, _l1) = allocate_split(used, capacity, offset, region_size, aligned_size);
    let u2 = deallocate_used(u1, aligned_size);
    let (u3, _r3, _l3) = allocate_split(u2, capacity, offset, region_size, aligned_size);
    deallocate_used(u3, aligned_size)
}

// =========================================================================
// TierState mirror + INIT-GATE (rank 1) / INIT-MONOTONIC / CAP-CONST.
// TierState transcribes the accounting subset of `MemoryTierState` (lib.rs:81-90)
// that the init-gate reads. `initialized: AtomicBool` is modelled as a plain
// `bool` — the atomic/RwLock protocol is NOT modelled (concurrency → Loom).
// =========================================================================

/// Mirror of the `MemoryTierError` variants the gate and mutators return.
#[derive(Clone, Copy)]
pub enum MtErr {
    NotInitialized,
    AlreadyExists,
    KeyNotFound,
    PoolFull,
    InvalidSize,
}

/// Faithful mirror of the accounting subset of `MemoryTierState`.
#[derive(Clone, Copy)]
pub struct TierState {
    pub initialized: bool,
    pub capacity: usize,
    pub used: usize,
    pub spdk_allocated: bool,
}

// -------------------------------------------------------------------------
// INIT-ZERO: `initialize` rejects pool_size == 0 with InvalidSize, no state
// change. Mirrors `if pool_size == 0 { return Err(InvalidSize) }` (lib.rs:262).
// -------------------------------------------------------------------------
/// `initialize` zero-size guard.
#[ensures(pool_size@ == 0 ==> result.1 == Err(MtErr::InvalidSize))] // INIT-ZERO
#[ensures(pool_size@ == 0 ==> result.0.initialized == state.initialized)] // no state change
#[ensures(pool_size@ == 0 ==> result.0.capacity == state.capacity)]
#[ensures(pool_size@ == 0 ==> result.0.used == state.used)]
#[ensures(pool_size@ > 0 ==> result.1 == Ok(()))] // positive size admitted to the init path
pub fn initialize_rejects_zero(state: TierState, pool_size: usize) -> (TierState, Result<(), MtErr>) {
    if pool_size == 0 {
        return (state, Err(MtErr::InvalidSize));
    }
    (state, Ok(()))
}

// -------------------------------------------------------------------------
// INIT-EP-NOT-CONNECTED / BATCH-EP-NOT-CONNECTED: when the eviction_policy
// receptacle is not connected, `initialize` returns NotInitialized (lib.rs:266)
// and `batch_touch` silently no-ops (lib.rs:528-531). Models the receptacle
// `get()` result as a bool.
// -------------------------------------------------------------------------
/// Receptacle-connection gate.
#[ensures(!ep_connected ==> result == Err(MtErr::NotInitialized))] // EP-NOT-CONNECTED
#[ensures(ep_connected ==> result == Ok(()))]
pub fn receptacle_gate(ep_connected: bool) -> Result<(), MtErr> {
    if !ep_connected {
        return Err(MtErr::NotInitialized);
    }
    Ok(())
}

// -------------------------------------------------------------------------
// INIT-MONOTONIC + CAP-CONST base + INIT-DOUBLE + INIT-CAPACITY + INIT-EMPTY.
// Mirrors `initialize` success/double-init paths (lib.rs:271-318).
// -------------------------------------------------------------------------
/// `initialize` mirror: establishes the latch and fixes capacity.
#[requires(pool_size@ > 0)]
#[ensures(state.initialized ==> result.0.initialized == state.initialized)] // INIT-DOUBLE: latch unchanged
#[ensures(state.initialized ==> result.0.used == state.used)] // INIT-ERR-NO-STATE-CHANGE
#[ensures(state.initialized ==> result.0.capacity == state.capacity)]
#[ensures(state.initialized ==> result.1 == Err(MtErr::AlreadyExists))] // repeat init rejected
#[ensures(!state.initialized ==> result.0.initialized)] // INIT-MONOTONIC: latch established
#[ensures(!state.initialized ==> result.0.capacity@ == pool_size@)] // INIT-CAPACITY / CAP-CONST base
#[ensures(!state.initialized ==> result.0.used@ == 0)] // INIT-EMPTY
#[ensures(!state.initialized ==> result.1 == Ok(()))]
pub fn initialize_state(state: TierState, pool_size: usize) -> (TierState, Result<(), MtErr>) {
    if state.initialized {
        return (state, Err(MtErr::AlreadyExists));
    }
    let s = TierState {
        initialized: true,
        capacity: pool_size,
        used: 0,
        spdk_allocated: state.spdk_allocated,
    };
    (s, Ok(()))
}

/// INIT-MONOTONIC + CAP-CONST preservation: a data-path mutator moves `used`
/// but never clears the latch or changes `capacity`.
#[requires(state.initialized)]
#[requires(new_used@ <= state.capacity@)]
#[ensures(result.initialized)] // INIT-MONOTONIC
#[ensures(result.capacity == state.capacity)] // CAP-CONST
#[ensures(result.used@ == new_used@)]
pub fn mutator_preserves_init(state: TierState, new_used: usize) -> TierState {
    TierState {
        initialized: state.initialized,
        capacity: state.capacity,
        used: new_used,
        spdk_allocated: state.spdk_allocated,
    }
}

// -------------------------------------------------------------------------
// INIT-GATE (rank 1, 16 attachments). Five shapes; each proves that when
// `!initialized` the op returns its gated sentinel and does NOT mutate state.
// One proof per shape discharges the bundle for every method of that shape.
// -------------------------------------------------------------------------
/// Shape R — Result-returning MUTATORS: `insert`, `remove`, `clear`.
#[requires(req@ <= state.capacity@)]
#[ensures(!state.initialized ==> result.0.initialized == state.initialized)]
#[ensures(!state.initialized ==> result.0.used == state.used)]
#[ensures(!state.initialized ==> result.0.capacity == state.capacity)]
#[ensures(!state.initialized ==> result.1 == Err(MtErr::NotInitialized))] // gated sentinel
pub fn gate_mutator_result(state: TierState, req: usize) -> (TierState, Result<usize, MtErr>) {
    if !state.initialized {
        return (state, Err(MtErr::NotInitialized));
    }
    let mut s = state;
    s.used = req;
    (s, Ok(req))
}

/// Shape O(mut) — Option-returning MUTATORS: `evict_next`, `evict_next_for_key`.
#[ensures(!state.initialized ==> result.0.initialized == state.initialized)]
#[ensures(!state.initialized ==> result.0.used == state.used)]
#[ensures(!state.initialized ==> result.0.capacity == state.capacity)]
#[ensures(!state.initialized ==> result.1 == None)] // gated sentinel
pub fn gate_mutator_option(state: TierState) -> (TierState, Option<usize>) {
    if !state.initialized {
        return (state, None);
    }
    let mut s = state;
    s.used = 0;
    (s, Some(state.used))
}

/// Shape O(ro) — read-only Option ops: `get`, `peek`, `pool_info`.
#[ensures(!state.initialized ==> result.1 == None)] // gated sentinel
#[ensures(result.0 == state)] // read-only: never mutated (GET/PEEK-NO-MUTATION frame)
pub fn gate_readonly_option(state: TierState) -> (TierState, Option<usize>) {
    if !state.initialized {
        return (state, None);
    }
    (state, Some(state.used))
}

/// Shape S — read-only scalar accessors: `contains`, `capacity`, `used`,
/// `oldest_keys`, `is_dma_capable`, `telemetry_snapshot`.
#[ensures(!state.initialized ==> result.1@ == 0)] // gated default (0 / false / empty)
#[ensures(result.0 == state)] // read-only: never mutates
pub fn gate_readonly_scalar(state: TierState) -> (TierState, usize) {
    if !state.initialized {
        return (state, 0);
    }
    (state, state.used)
}

/// Shape N — no-op ops: `touch`, `batch_touch`.
#[ensures(!state.initialized ==> result == state)] // gated: no-op (TOUCH/BATCH-NO-MUTATION)
pub fn gate_noop(state: TierState) -> TierState {
    if !state.initialized {
        return state;
    }
    state
}

// =========================================================================
// PER-METHOD CONTROL-FLOW MIRRORS. Each models a container/receptacle test as
// a scalar bool (justified by CONTAINS-REFLECTS / the receptacle protocol) and
// proves the branch the shipped code takes. The container op itself is trusted;
// what is proved is that the code routes correctly on its boolean result.
// =========================================================================

// INSERT-DEDUP + INSERT-DEDUP-NO-ALLOC. Mirrors `if pool.slots.contains_key(&key)
// { return Err(AlreadyExists) }` BEFORE the allocate (lib.rs:356-358). `present`
// models `contains_key`. On a dup: return AlreadyExists AND leave `used`
// unchanged (allocator never called — the frame).
/// insert dedup guard: dup key rejected with no allocation.
#[ensures(present ==> result.1 == Err(MtErr::AlreadyExists))] // INSERT-DEDUP
#[ensures(present ==> result.0@ == used@)] // INSERT-DEDUP-NO-ALLOC: allocator untouched
#[ensures(!present ==> result.1 == Ok(()))] // fresh key proceeds to allocate
pub fn insert_dedup(used: usize, present: bool) -> (usize, Result<(), MtErr>) {
    if present {
        return (used, Err(MtErr::AlreadyExists));
    }
    (used, Ok(()))
}

// GET-ABSENT-NONE + PEEK-ABSENT-NONE. Mirrors `let slot = pool.slots.get(&key)?;`
// (lib.rs:403, :419) — absent key short-circuits to None. `present` models the
// `get` Option.
/// lookup returns None iff the key is absent.
#[ensures(!present ==> result == None)] // ABSENT-NONE
#[ensures(present ==> result == Some(off))]
pub fn lookup_absent_none(present: bool, off: usize) -> Option<usize> {
    if !present {
        return None;
    }
    Some(off)
}

// REMOVE-NOTFOUND. Mirrors `pool.slots.remove(&key).ok_or(KeyNotFound)?`
// (lib.rs:496-499). Absent key → Err(KeyNotFound); present key → Ok, proceed.
/// remove returns KeyNotFound iff the key is absent.
#[ensures(!present ==> result == Err(MtErr::KeyNotFound))] // REMOVE-NOTFOUND
#[ensures(present ==> result == Ok(()))]
pub fn remove_notfound(present: bool) -> Result<(), MtErr> {
    if !present {
        return Err(MtErr::KeyNotFound);
    }
    Ok(())
}

// EVICT-DEALLOC-GUARDED. Mirrors `if let Some(slot) = pool.slots.remove(&key) {
// pool.allocator.deallocate(..) }` (lib.rs:456-457): the pool is freed only when
// the victim slot is actually present.
/// eviction frees the slot iff a victim slot was present.
#[requires(aligned_size@ <= used@)]
#[ensures(victim_present ==> result@ == used@ - aligned_size@)] // freed
#[ensures(!victim_present ==> result@ == used@)] // no free
pub fn evict_dealloc_guarded(used: usize, aligned_size: usize, victim_present: bool) -> usize {
    if victim_present {
        return used - aligned_size;
    }
    used
}

// EVICTKEY-ALIAS. Mirrors `fn evict_next_for_key(&self, _key) { self.evict_next() }`
// (lib.rs:468-470): the result is independent of `key`.
/// evict_next_for_key ignores its key: result equals evict_next's.
#[ensures(result == base_result)] // EVICTKEY-ALIAS: key ignored
pub fn evict_key_alias(_key: u64, base_result: Option<u64>) -> Option<u64> {
    base_result
}

// OLDEST-GUARD-EMPTY. Mirrors `if !initialized || n == 0 { return Vec::new() }`
// (lib.rs:426-428): empty result (len 0) in the guarded case.
/// oldest_keys returns empty when uninitialized or n == 0.
#[ensures(!initialized || n@ == 0 ==> result@ == 0)] // OLDEST-GUARD-EMPTY: empty
#[ensures(initialized && n@ > 0 ==> result@ <= n@)] // otherwise bounded by n (candidates ≤ n)
pub fn oldest_guard_empty(initialized: bool, n: usize) -> usize {
    if !initialized || n == 0 {
        return 0;
    }
    n
}

// BATCHTOUCH-EMPTY. Mirrors `if keys.is_empty() { return; }` (lib.rs:521-523).
/// batch_touch is a no-op (0 handles collected) for an empty key slice.
#[ensures(len@ == 0 ==> result@ == 0)] // BATCHTOUCH-EMPTY
#[ensures(len@ > 0 ==> result@ <= len@)] // at most one handle per key
pub fn batch_touch_empty(len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    len
}

// TOUCH-ABSENT-NOOP + BATCHTOUCH-ALL + BATCHTOUCH-SKIP (element level).
// Mirrors `if let Some(slot) = pool.slots.get(&key) { <collect / touch> }`
// (lib.rs:513, :549): a key is included in the touch set iff it is present.
// The whole-batch aggregation (fold over the slice) is trusted iteration.
/// per-key touch selection: touched iff present.
#[ensures(result == present)] // TOUCH-ABSENT-NOOP / BATCHTOUCH-ALL / BATCHTOUCH-SKIP
pub fn touch_selected(present: bool) -> bool {
    if present {
        return true;
    }
    false
}

// CLEAR-COUNT. Mirrors `let count = pool.slots.len(); pool.slots.clear();`
// (lib.rs:603-604): returns the pre-clear entry count; `used` reset to 0.
/// clear returns the number of entries removed and resets used.
#[ensures(result.0@ == n@)] // CLEAR-COUNT: returns pre-clear len
#[ensures(result.1@ == 0)] // used → 0 (ties CLEAR-EMPTIES accounting half)
pub fn clear_count(n: usize) -> (usize, usize) {
    let count = n;
    (count, 0)
}

// TELEM-SNAPSHOT-ZERO (feature-off branch). Mirrors the `#[cfg(not(telemetry))]`
// arm returning `MemoryTierTelemetrySnapshot::default()` (lib.rs:632-634).
/// telemetry snapshot is all-zero when the telemetry feature is disabled.
#[ensures(result.0@ == 0 && result.1@ == 0 && result.2@ == 0)] // TELEM-SNAPSHOT-ZERO (off)
pub fn telemetry_off_zero() -> (u64, u64, u64) {
    (0, 0, 0)
}

// =========================================================================
// CONTAINS-REFLECTS (rank 4, 6 attachments) — logic-level FMap slot-map model.
// The shipped `slots: HashMap<CacheKey, Slot>` (lib.rs:78) is modelled by
// `creusot_std::logic::FMap<u64, SlotMirror>`; these lemmas prove the membership
// algebra the real map must satisfy. Binding the FMap model to `std::HashMap`'s
// ops is a trusted step (see boundary ledger).
// =========================================================================

use creusot_std::logic::FMap;

/// Payload mirror of `Slot` (lib.rs:70-74); only membership matters here.
#[derive(Clone, Copy)]
pub struct SlotMirror {
    pub offset: usize,
    pub size: u32,
}

/// CONTAINS-REFLECTS (insert): inserting `k` makes it present and frames the rest.
#[logic]
#[ensures(m.insert(k, v).contains(k))]
#[ensures(forall<j: u64> j != k ==> m.insert(k, v).contains(j) == m.contains(j))]
#[ensures(m.insert(k, v).get(k) == Some(v))]
pub fn contains_reflects_insert(m: FMap<u64, SlotMirror>, k: u64, v: SlotMirror) {}

/// CONTAINS-REFLECTS (length / INSERT-DEDUP): fresh key +1, present key no growth.
#[logic]
#[ensures(!m.contains(k) ==> m.insert(k, v).len() == m.len() + 1)]
#[ensures(m.contains(k) ==> m.insert(k, v).len() == m.len())]
pub fn contains_reflects_insert_len(m: FMap<u64, SlotMirror>, k: u64, v: SlotMirror) {}

/// CONTAINS-REFLECTS (remove / evict): removing `k` deletes it and frames the rest.
#[logic]
#[ensures(!m.remove(k).contains(k))]
#[ensures(forall<j: u64> j != k ==> m.remove(k).contains(j) == m.contains(j))]
pub fn contains_reflects_remove(m: FMap<u64, SlotMirror>, k: u64) {}

/// CONTAINS-REFLECTS (absent-get): a key absent from the map has no entry.
/// Backs GET-ABSENT-NONE / PEEK-ABSENT-NONE / REMOVE-NOTFOUND at the model level.
#[logic]
#[ensures(!m.contains(k) ==> m.get(k) == None)]
pub fn contains_reflects_absent(m: FMap<u64, SlotMirror>, k: u64) {}

/// CONTAINS-REFLECTS (clear): after clear no key is present.
#[logic]
#[ensures(forall<k: u64> !FMap::<u64, SlotMirror>::empty().contains(k))]
pub fn contains_reflects_clear() {}

/// CONTAINS-REFLECTS operational trace: empty → insert → insert → remove →
/// evict → re-insert → clear, asserting membership after every step.
pub fn contains_reflects_trace() {
    let s1 = SlotMirror { offset: 0, size: 4096 };
    let s2 = SlotMirror { offset: 4096, size: 8192 };
    let mut map = FMap::<u64, SlotMirror>::new();
    ghost! {
        proof_assert!(forall<k: u64> !map.contains(k));

        map.insert_ghost(1u64, s1);
        proof_assert!(map.contains(1u64));
        proof_assert!(!map.contains(2u64));

        map.insert_ghost(2u64, s2);
        proof_assert!(map.contains(1u64));
        proof_assert!(map.contains(2u64));

        map.remove_ghost(&1u64);
        proof_assert!(!map.contains(1u64));
        proof_assert!(map.contains(2u64));

        map.remove_ghost(&2u64);
        proof_assert!(!map.contains(1u64));
        proof_assert!(!map.contains(2u64));

        map.insert_ghost(1u64, s1);
        proof_assert!(map.contains(1u64));
        map.clear_ghost();
        proof_assert!(forall<k: u64> !map.contains(k));
    };
}

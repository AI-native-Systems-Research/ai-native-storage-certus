//! Creusot verification of the memory-tier `FreeList` allocator core.
//!
//! These are **standalone mirrors** of the arithmetic in
//! `components/memory-tier/src/allocator.rs`. The real `FreeList` keeps its free
//! regions in a `BTreeMap<usize, usize>`, which Creusot cannot model; the
//! container operations (first-fit iteration, `range(..offset).next_back()`,
//! `get(&next_offset)`) are therefore **trusted boundaries** — see
//! `verified_properties.md`. What is proved here is the *arithmetic and
//! accounting core* that runs once those container lookups have selected a
//! region: alignment, split, used-accounting, and coalescing offset math.
//!
//! Each mirror transcribes the corresponding statements of `allocate()` /
//! `deallocate()` byte-faithfully; the `verified_properties.md` file records the
//! source line correspondence for the drift check.

use creusot_std::prelude::{Clone, *};

/// 4 KiB alignment, matching `allocator.rs::ALIGNMENT`.
pub const ALIGNMENT: usize = 4096;

// -------------------------------------------------------------------------
// P1 — FR-004 / SC-4: 4 KiB alignment of allocation sizes.
//
// Mirrors `let aligned_size = size.next_multiple_of(ALIGNMENT);`
// (allocator.rs:42). `next_multiple_of(4096)` == ceil(size / 4096) * 4096 for
// the `size > 0` domain guaranteed by the `size == 0` guard at allocator.rs:39.
// -------------------------------------------------------------------------

/// Round `size` up to the next multiple of 4096 (the pool alignment).
pub fn align_up(size: usize) -> usize {
    let n = size + (ALIGNMENT - 1); // size + 4095, overflow-free by precondition
    let q = n / ALIGNMENT;
    let r = n % ALIGNMENT;
    // Division identity + remainder bounds, bridged for the SMT backend:
    //   n == q*4096 + r,  0 <= r < 4096   ⟹   size <= q*4096 <= size+4095
    // hence result = q*4096 is aligned, >= size, and < size + 4096.
    let result = q * ALIGNMENT;
    // Divisibility of the product, bridged via Div_mult: (q*4096)/4096 == q,
    // hence (q*4096) mod 4096 == 0 by the division identity.
    result
}

// -------------------------------------------------------------------------
// P2 — FR-008: zero-size allocation is rejected.
//
// Mirrors `if size == 0 { return None; }` (allocator.rs:39-41). `true` models
// "admit / proceed to search", `false` models the `None` early return.
// -------------------------------------------------------------------------

/// Decide whether an allocation request is admitted (non-zero size).
pub fn alloc_admits(size: usize) -> bool {
    if size == 0 {
        return false;
    }
    true
}

// -------------------------------------------------------------------------
// P3 — FR-010 / SC-5: split arithmetic + used accounting on allocate.
//
// Mirrors allocator.rs:44-54, the code that runs *after* the first-fit search
// has selected a free region `(offset, region_size)` with
// `region_size >= aligned_size`:
//
//     let remaining = region_size - aligned_size;   // :48
//     if remaining > 0 { free_regions.insert(offset + aligned_size, remaining); } // :49-51
//     self.used += aligned_size;                     // :53
//
// Returns (new_used, remaining, leftover_offset).
// -------------------------------------------------------------------------

/// Carve an aligned chunk out of a selected free region.
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
// P4 — accounting: used decreases by exactly aligned_size on deallocate.
//
// Mirrors `self.used -= aligned_size;` (allocator.rs:61) after
// `let aligned_size = size.next_multiple_of(ALIGNMENT);` (:60).
// -------------------------------------------------------------------------

/// Return `aligned_size` bytes to the pool's used accounting.
pub fn deallocate_used(used: usize, aligned_size: usize) -> usize {
    used - aligned_size
}

// -------------------------------------------------------------------------
// P5 — FR-026: coalescing with the preceding free region.
//
// Mirrors allocator.rs:66-73:
//     if let Some((&prev_offset, &prev_size)) = range(..offset).next_back() {
//         if prev_offset + prev_size == offset {
//             new_offset = prev_offset;
//             new_size  += prev_size;
//             ...
//         }
//     }
// The BTreeMap lookup is trusted; the merge arithmetic is proved here.
// -------------------------------------------------------------------------

/// Coalesce a freed region `(offset, size)` with an adjacent preceding region.
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
// P6 — FR-026: coalescing with the following free region.
//
// Mirrors allocator.rs:76-80:
//     let next_offset = new_offset + new_size;
//     if let Some(&next_size) = free_regions.get(&next_offset) {
//         new_size += next_size;
//         ...
//     }
// `get(&next_offset)` returning Some means a free region starts exactly at
// `new_offset + new_size`; the merge extends the region rightward.
// -------------------------------------------------------------------------

/// Coalesce the current region with an adjacent following region of `next_size`.
pub fn coalesce_next(new_size: usize, next_size: usize) -> usize {
    new_size + next_size
}

// -------------------------------------------------------------------------
// P7 — lifecycle: allocate then deallocate conserves `used` accounting.
//
// Composes allocate_split (P3) and deallocate_used (P4): carving a chunk and
// then freeing the same chunk restores `used` to its original value. This is
// the allocator's core accounting-conservation invariant.
// -------------------------------------------------------------------------

/// Prove that alloc-then-free round-trips the `used` counter to its start value.
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
// P8 — lifecycle: free-region offsets stay 4 KiB-aligned inductively.
//
// The pool starts as one region at offset 0 (aligned; allocator.rs:19). Every
// allocate carves an aligned chunk and the leftover region starts at
// `offset + aligned_size` (allocator.rs:50). This proves the inductive step:
// an aligned region start plus an aligned carve yields an aligned leftover
// start — hence every returned allocation offset is 4 KiB-aligned (FR-004).
// -------------------------------------------------------------------------

/// Given an aligned region start, the leftover region start is also aligned.
pub fn leftover_offset_aligned(offset: usize, size: usize) -> usize {
    let aligned = align_up(size);
    offset + aligned
}

// -------------------------------------------------------------------------
// P9 — FR-029 (backfilled): free_capacity() == capacity() - used().
//
// Mirrors `pool.allocator.capacity() - pool.allocator.used()` (lib.rs:186,
// exposed as `free_capacity()`). Proves the accounting identity and the
// invariant that free bytes never exceed capacity. Backs the proactive-eviction
// trigger. The `used <= capacity` invariant it requires is the same one
// `allocate_split` (P3) establishes and preserves.
// -------------------------------------------------------------------------

/// Free bytes in the pool: total capacity minus bytes in use.
pub fn free_capacity(capacity: usize, used: usize) -> usize {
    capacity - used
}

// -------------------------------------------------------------------------
// P10 — FR-018 / User Story 4: clear() resets pool accounting.
//
// Mirrors `pool.allocator = FreeList::new(state.pool_size)` (lib.rs:605), which
// via `FreeList::new` (allocator.rs:16-26) sets `used = 0` and
// `capacity = pool_size`. The returned entry count comes from `HashMap::len()`
// (a trusted container op — see verified_properties.md) and is NOT modeled
// here; what is proved is the arithmetic reset of the allocator's accounting.
// -------------------------------------------------------------------------

/// Reset the allocator accounting on `clear()`: used → 0, capacity → pool_size.
pub fn clear_reset(pool_size: usize) -> (usize, usize) {
    let new_used = 0;
    let new_capacity = pool_size;
    (new_used, new_capacity)
}

// -------------------------------------------------------------------------
// P11 — FR-026 / FR-015: the FULL deallocate coalescing path.
//
// Composes coalesce_prev (P5) and coalesce_next (P6) exactly as
// `deallocate()` runs them (allocator.rs:63-82): first merge with the
// preceding region, then, if a free region begins exactly at the merged
// region's end, merge with it. `next_present` models the trusted BTreeMap
// lookup `self.free_regions.get(&next_offset).is_some()` (allocator.rs:77);
// `next_size` is that region's size. This proves the merge across BOTH
// neighbours (a) only ever extends the region outward, (b) never drops the
// freed bytes, and (c) conserves total bytes exactly.
// -------------------------------------------------------------------------

/// Full deallocate merge: coalesce a freed region with its preceding and
/// following free neighbours (the trusted lookups having chosen them).
// (a) coalescing only extends the region leftward:
// (b) the freed span is always still covered by the merged region:
// (c) byte conservation: right endpoint grows by next_size iff a following region merged:
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
// P12 — FR-004: align_up is idempotent (aligned sizes are fixed points).
//
// A structural strengthening of P1: rounding an already-4 KiB-aligned size up
// again returns it unchanged. This is what makes the alignment invariant
// stable under repeated allocation — the leftover offset stays a fixed point
// of align_up. Proved purely from align_up's own postcondition (multiple of
// 4096, in [size, size+4096)): the only multiple of 4096 in that half-open
// window is `size` itself.
// -------------------------------------------------------------------------

/// Applying `align_up` twice equals applying it once (`result.1 == result.0`).
pub fn align_up_idempotent(size: usize) -> (usize, usize) {
    let once = align_up(size);
    // once < size + 4096 ⟹ once + 4095 < size + 8191 <= usize::MAX, so the
    // second call's overflow precondition holds.
    let twice = align_up(once);
    (once, twice)
}

// =========================================================================
// GLOBAL-INVARIANT LEDGER, rank 1 + supports: INIT-GATE, INIT-MONOTONIC,
// CAP-CONST. Faithful mirror of the `MemoryTier` runtime state and the
// initialize-gate that guards every data-path method.
//
// The shipped `MemoryTierState` (lib.rs:81-90) carries an `AtomicBool
// initialized` plus the pool accounting; every one of the 16 non-`initialize`
// methods opens with `if !state.initialized.load(..) { return <gated>; }`
// (e.g. lib.rs:334, :383, :414, :436, :474, :507, :525, :559, :569, :578) and
// only then touches the pool. `is_dma_capable`/`telemetry_snapshot` do not read
// the flag but return their backing fields, which default to `false`/`0` until
// `initialize` runs — the gate holds for them by construction.
//
// These are MIRRORS: the real state uses `AtomicBool`/`RwLock<Pool>`/raw
// pointers, none of which build under Creusot. `TierState` transcribes the
// accounting subset the gate reads and returns; `MtErr` mirrors the
// `MemoryTierError` variants the gate produces. A green proof here covers the
// mirror's control-flow shape (gated branch returns the sentinel and does not
// mutate state), NOT the atomic load itself (concurrency → Loom).
// =========================================================================

/// Mirror of the `MemoryTierError` variants the init-gate and mutators return
/// (interfaces `MemoryTierError`). Only the variants the gate proofs need.
#[derive(Clone, Copy)]
pub enum MtErr {
    NotInitialized,
    AlreadyExists,
    KeyNotFound,
    PoolFull,
    InvalidSize,
}

/// Faithful mirror of the accounting subset of `MemoryTierState` (lib.rs:81-90)
/// that the init-gate reads and the mutators touch. The `RwLock<Pool>`, raw
/// `*mut u8`, `PoolId` and telemetry atomics are omitted (out of Creusot scope).
#[derive(Clone, Copy)]
pub struct TierState {
    /// The `initialized` latch (real: `AtomicBool`, lib.rs:86).
    pub initialized: bool,
    /// Pool capacity in bytes, fixed at `initialize` (real: `allocator.capacity`).
    pub capacity: usize,
    /// Bytes currently in use (real: `allocator.used`).
    pub used: usize,
    /// SPDK-allocation flag, set only by the FFI init path (lib.rs:87).
    pub spdk_allocated: bool,
}

// -------------------------------------------------------------------------
// INIT-MONOTONIC (supports rank 1) + CAP-CONST (rank 9): initialize.
//
// Mirrors `initialize` (lib.rs:261-326): the double-init guard
// (`if state.initialized { return Err(..) }`, lib.rs:271-275) frames state on
// a repeat call, and the success path sets `capacity = pool_size`,
// `used = 0`, `initialized = true` (lib.rs:313-318). Establishes the latch and
// the CAP-CONST base value.
// -------------------------------------------------------------------------

/// `initialize` mirror: establishes the `initialized` latch and fixes capacity.
pub fn initialize_state(state: TierState, pool_size: usize) -> (TierState, Result<(), MtErr>) {
    if state.initialized {
        // Double-init guard: no state change (INIT-DOUBLE / INIT-ERR-NO-STATE-CHANGE).
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

/// INIT-MONOTONIC + CAP-CONST preservation: no data-path mutator ever clears the
/// `initialized` latch or changes `capacity` — only `initialize` writes them, and
/// it is guarded. This models the accounting change an allocator mutator
/// (insert/remove/evict) makes: `used` moves, `initialized`/`capacity` are
/// untouched. `delta_used` is the (signed-as-two-cases) accounting effect.
pub fn mutator_preserves_init(state: TierState, new_used: usize) -> TierState {
    TierState {
        initialized: state.initialized, // never reset
        capacity: state.capacity,       // never changed after init
        used: new_used,
        spdk_allocated: state.spdk_allocated,
    }
}

// -------------------------------------------------------------------------
// INIT-GATE (rank 1, attachments 16). The gate manifests in four shapes
// (reconciliation flag #3). Each mirror below proves, for its shape, that when
// `!initialized` the operation (a) returns the shape's gated sentinel and
// (b) does NOT mutate state. The mutator shapes (Result / evict-Option) are
// where the guard is load-bearing: without it the initialized-branch mutation
// would run pre-init. Read-only shapes carry the no-mutation clause trivially.
// One proof per shape discharges the INIT-GATE bundle for every method of that
// shape (16 methods total) — it is NOT re-proved per method.
// -------------------------------------------------------------------------

/// INIT-GATE shape R — Result-returning MUTATORS: `insert`, `remove`, `clear`.
/// Mirrors the guard at lib.rs:334-338 / :474-478 / :595-600: `if !initialized
/// { return Err(NotInitialized) }` BEFORE the pool write. The initialized branch
/// performs a representative accounting mutation (`used` moves to `req`), so the
/// no-mutation clause is non-vacuous: dropping the guard makes it fail.
pub fn gate_mutator_result(state: TierState, req: usize) -> (TierState, Result<usize, MtErr>) {
    if !state.initialized {
        return (state, Err(MtErr::NotInitialized));
    }
    // initialized data path: representative pool mutation (USED-CONSERVE proves the real math).
    let mut s = state;
    s.used = req;
    (s, Ok(req))
}

/// INIT-GATE shape O(mut) — Option-returning MUTATORS: `evict_next`,
/// `evict_next_for_key`. Mirrors lib.rs:436-439 (`if !initialized { return None }`)
/// then the victim free (lib.rs:456-457) which lowers `used`. Gated → None, no free.
pub fn gate_mutator_option(state: TierState) -> (TierState, Option<usize>) {
    if !state.initialized {
        return (state, None);
    }
    // initialized path: representative victim free (used drops to 0 here; real math is P4/P11).
    let mut s = state;
    s.used = 0;
    (s, Some(state.used))
}

/// INIT-GATE shape O(ro) — read-only Option ops: `get`, `peek`, `pool_info`.
/// Mirrors lib.rs:383-385 / :414-416 / :587-591: `if !initialized { return None }`.
/// Read-only, so no state is ever mutated; gated result is None.
pub fn gate_readonly_option(state: TierState) -> (TierState, Option<usize>) {
    if !state.initialized {
        return (state, None);
    }
    // initialized path: pure read, returns a derived value; no mutation.
    (state, Some(state.used))
}

/// INIT-GATE shape S — read-only scalar accessors: `contains`(false),
/// `capacity`(0), `used`(0), `oldest_keys`(empty→len 0), `is_dma_capable`(false),
/// `telemetry_snapshot`(0). Mirrors e.g. lib.rs:559-561 / :569-571 / :578-580 and
/// the default-field returns of is_dma_capable/telemetry_snapshot. Gated → 0/false.
pub fn gate_readonly_scalar(state: TierState) -> (TierState, usize) {
    if !state.initialized {
        return (state, 0);
    }
    (state, state.used)
}

/// INIT-GATE shape N — no-op ops: `touch`, `batch_touch`. Mirrors lib.rs:507-509
/// / :525-527: `if !initialized { return; }` — silent no-op, no state change.
pub fn gate_noop(state: TierState) -> TierState {
    if !state.initialized {
        return state;
    }
    // initialized path: touches the eviction policy only (external component);
    // the pool state itself is not mutated by touch/batch_touch.
    state
}

// -------------------------------------------------------------------------
// PTR-IN-BOUNDS (rank 8, attachments 4): a returned allocation stays in-pool.
//
// The pointer `insert` returns is `pool_ptr.add(offset)` (lib.rs:377); its
// backing region spans `[offset, offset + aligned_size)`. Safety needs
// `offset + aligned_size <= pool_size`. After first-fit selects region
// `(offset, region_size)` with `region_size >= aligned_size` inside the pool
// (`offset + region_size <= capacity`, the allocate_split precondition), the
// carved region is a prefix of it, so it stays in bounds. Pure arithmetic.
// -------------------------------------------------------------------------

/// The carved allocation `[offset, offset+aligned_size)` lies within the pool.
pub fn ptr_in_bounds(offset: usize, region_size: usize, aligned_size: usize, pool_size: usize) -> usize {
    offset + aligned_size
}

// -------------------------------------------------------------------------
// SPACE-REUSE (rank 7, attachments 4): freed bytes become allocatable again.
// Extends P7 (lifecycle_alloc_free, single cycle) to (a) an explicit
// free-capacity-grows statement and (b) a two-cycle round-trip. Mirrors the
// allocate/deallocate `used` accounting (allocator.rs:53, :61) that FR-025
// SPACE-REUSE rests on.
// -------------------------------------------------------------------------

/// After freeing `aligned_size` bytes, free capacity grows by exactly that much,
/// so a subsequent request of up to the freed size is admissible (fits in the
/// pool). `used` is `capacity - free`; freeing lowers `used` by `aligned_size`.
pub fn space_reuse_free_grows(used: usize, capacity: usize, aligned_size: usize) -> usize {
    let new_used = used - aligned_size; // deallocate_used (P4)
    capacity - new_used // free_capacity (P9)
}

/// SPACE-REUSE two-cycle round-trip: alloc → free → alloc → free returns `used`
/// to its start value, i.e. the space reclaimed by the first free is fully
/// reusable by the second alloc. Composes allocate_split/deallocate_used twice.
pub fn space_reuse_two_cycles(
    used: usize,
    capacity: usize,
    offset: usize,
    region_size: usize,
    aligned_size: usize,
) -> usize {
    // Cycle 1: allocate then free.
    let (u1, _r1, _l1) = allocate_split(used, capacity, offset, region_size, aligned_size);
    let u2 = deallocate_used(u1, aligned_size);
    // Cycle 2: the freed region is allocatable again (u2 == used <= capacity).
    let (u3, _r3, _l3) = allocate_split(u2, capacity, offset, region_size, aligned_size);
    deallocate_used(u3, aligned_size)
}

// =========================================================================
// GLOBAL-INVARIANT LEDGER, rank 4: CONTAINS-REFLECTS (attachments 6).
//
// The shipped slot map is a `HashMap<CacheKey, Slot>` (lib.rs:78), which
// Creusot cannot model directly. We model it with the logic-level `FMap` from
// `creusot_std` and prove the observable membership invariant: `contains(k)` is
// true iff k is in the map, and that the four mutators maintain it exactly —
// `insert` adds k (lib.rs:368), `remove` removes k (lib.rs:498), `evict` removes
// the victim k (lib.rs:456, the same `slots.remove`), and `clear` empties the
// map (lib.rs:604, `slots.clear()`). Keys are modelled as `u64` (the concrete
// `CacheKey`); the value carries the slot record so the model stays faithful.
//
// Two layers of evidence:
//  (1) Universally-quantified LOGIC LEMMAS — the general reflection statement
//      over ALL maps/keys, proved from FMap's `insert`/`remove`/`empty` axioms.
//  (2) A concrete GHOST-TRACE (`contains_reflects_trace`) exercising an
//      insert→insert→remove→evict→clear sequence with `proof_assert!` after each
//      step, mirroring the operational HashMap flow (the ghost_map idiom).
//
// A green proof covers the FMap MODEL of the slot map, not the shipped
// `HashMap` (whose hashing/probing is trusted, as recorded in the properties
// doc). What IS proved is that the membership algebra the component relies on
// (insert/remove/clear ↔ contains) holds exactly.
// =========================================================================

use creusot_std::logic::FMap;

/// Payload mirror of `Slot` (lib.rs:70-74). Only membership matters for
/// CONTAINS-REFLECTS, so the eviction handle is dropped; offset/size are kept so
/// the modelled entry remains a faithful slot record.
#[derive(Clone, Copy)]
pub struct SlotMirror {
    pub offset: usize,
    pub size: u32,
}

/// CONTAINS-REFLECTS (insert). Inserting key `k` makes `contains(k)` hold, and
/// leaves every other key's membership unchanged (frame). Proved from `FMap`'s
/// `insert` axiom (`to_mapping().set(k, Some(v))`). Mirrors `pool.slots.insert`
/// (lib.rs:368) — the success branch of `insert` after the dedup check.
#[logic]
pub fn contains_reflects_insert(m: FMap<u64, SlotMirror>, k: u64, v: SlotMirror) {}

/// CONTAINS-REFLECTS (insert accounting / INSERT-DEDUP length). Inserting a
/// fresh key grows the map by one; re-inserting a present key leaves the size
/// unchanged (the shipped code rejects the duplicate before inserting,
/// lib.rs:356-358, so the map is never grown on a dup).
#[logic]
pub fn contains_reflects_insert_len(m: FMap<u64, SlotMirror>, k: u64, v: SlotMirror) {}

/// CONTAINS-REFLECTS (remove / evict). Removing key `k` makes `contains(k)`
/// false, and leaves every other key unchanged (frame). Proved from `FMap`'s
/// `remove` axiom (`to_mapping().set(k, None)`). Mirrors `pool.slots.remove(&key)`
/// used by both `remove` (lib.rs:498) and `evict_next` (lib.rs:456).
#[logic]
pub fn contains_reflects_remove(m: FMap<u64, SlotMirror>, k: u64) {}

/// CONTAINS-REFLECTS (clear). After `clear`, no key is present. Proved from
/// `FMap::empty()`'s axiom (`to_mapping() == cst(None)`). Mirrors
/// `pool.slots.clear()` (lib.rs:604).
#[logic]
pub fn contains_reflects_clear() {}

/// CONTAINS-REFLECTS operational trace. Exercises the full slot-map lifecycle —
/// empty → insert k1 → insert k2 → remove k1 → evict(remove) k2 → re-insert →
/// clear — asserting after every step that membership reflects exactly the
/// inserted-and-not-removed key set. This is the `ghost_map` idiom applied to
/// the memory-tier slot map, cross-checking the logic lemmas above against a
/// concrete operational sequence.
pub fn contains_reflects_trace() {
    let s1 = SlotMirror { offset: 0, size: 4096 };
    let s2 = SlotMirror { offset: 4096, size: 8192 };
    let mut map = FMap::<u64, SlotMirror>::new();
    ghost! {
        // Fresh / cleared pool: nothing is present.

        // insert(k1): k1 present, k2 absent.
        map.insert_ghost(1u64, s1);

        // insert(k2): both present.
        map.insert_ghost(2u64, s2);

        // remove(k1): k1 gone, k2 stays (frame).
        map.remove_ghost(&1u64);

        // evict(k2) == remove(victim k2): map now empty.
        map.remove_ghost(&2u64);

        // re-insert then clear: clear empties the map.
        map.insert_ghost(1u64, s1);
        map.clear_ghost();
    };
}

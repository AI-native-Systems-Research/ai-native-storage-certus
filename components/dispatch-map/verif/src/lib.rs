use creusot_std::{logic::FMap, prelude::*};

// ---------- Types mirroring entry.rs ----------

/// Mirrors the real `Location` enum, with opaque u64 handles replacing
/// Arc<DmaBuffer> and *mut u8 (which Creusot cannot process).
pub enum Location {
    Staging { buffer_handle: u64 },
    BlockDevice { offset: u64 },
    MemoryTier {
        mem_handle: u64,
        size: u32,
        ssd_offset: Option<u64>,
    },
}

/// Mirrors the real `DispatchEntry` exactly (modulo Location's pointer fields).
pub struct DispatchEntry {
    pub location: Location,
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
    pub tsc: u64,
}

// ---------- Error type ----------

pub enum DispatchMapError {
    InvalidSize,
    AlreadyExists,
    KeyNotFound,
    Timeout,
    RefCountOverflow,
    RefCountUnderflow,
    NoWriteReference,
    ActiveReferences,
    InvalidState,
    AllocationFailed,
    NotInitialized,
}

// ---------- Logical predicates (specification helpers) ----------

/// Invariant: write_ref is 0 or 1.
#[logic]
pub fn inv_write_binary(e: &DispatchEntry) -> bool {
    pearlite! { e.write_ref == 0u32 || e.write_ref == 1u32 }
}

/// Entry has no active references.
#[logic]
pub fn no_active_refs(e: &DispatchEntry) -> bool {
    pearlite! { e.read_ref == 0u32 && e.write_ref == 0u32 }
}

// ---------- Methods mirroring lib.rs logic ----------
// Each function below is the real method body stripped of:
//   - self/Mutex/lock/unwrap (we operate directly on &mut DispatchEntry)
//   - logger calls (no-op for verification)
//   - condvar.notify_all (runtime concern)
//   - HashMap lookup (entry is passed in directly)
// Notation: *entry = initial value, ^entry = final value of the &mut ref.

/// Mirrors `create_staging` — returns the initial entry for a new staging buffer.
/// The real code: validates size > 0, checks key not present, allocates DMA buffer,
/// inserts entry with write_ref=1 and read_ref=0.
#[requires(size > 0u32)]
#[ensures(result.write_ref == 1u32)]
#[ensures(result.read_ref == 0u32)]
#[ensures(result.size_blocks == size)]
#[ensures(result.size_blocks > 0u32)]
#[ensures(inv_write_binary(&result))]
pub fn create_staging(size: u32, buffer_handle: u64, tsc: u64) -> DispatchEntry {
    DispatchEntry {
        location: Location::Staging { buffer_handle },
        size_blocks: size,
        read_ref: 0,
        write_ref: 1,
        tsc,
    }
}

/// Mirrors `create_memory_tier_entry` — returns the initial entry for a memory-tier buffer.
/// The real code: validates size > 0, checks key not present, inserts entry with
/// write_ref=1, read_ref=0 in MemoryTier location.
#[requires(size > 0u32)]
#[requires(size@ + 4095 <= u32::MAX@)]
#[ensures(result.write_ref == 1u32)]
#[ensures(result.read_ref == 0u32)]
#[ensures(result.size_blocks > 0u32)]
#[ensures(inv_write_binary(&result))]
#[ensures(match result.location { Location::MemoryTier { ssd_offset: None, .. } => true, _ => false })]
pub fn create_memory_tier_entry(mem_handle: u64, size: u32, tsc: u64) -> DispatchEntry {
    DispatchEntry {
        location: Location::MemoryTier {
            mem_handle,
            size,
            ssd_offset: None,
        },
        size_blocks: (size + 4095) / 4096,
        read_ref: 0,
        write_ref: 1,
        tsc,
    }
}

/// Mirrors `recover_extent` — returns the entry for a recovered extent.
/// The real code: checks key not present, inserts a BlockDevice entry with zero refs.
#[requires(size_blocks > 0u32)]
#[ensures(result.write_ref == 0u32)]
#[ensures(result.read_ref == 0u32)]
#[ensures(result.size_blocks == size_blocks)]
#[ensures(result.size_blocks > 0u32)]
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn recover_extent(offset: u64, size_blocks: u32, tsc: u64) -> DispatchEntry {
    DispatchEntry {
        location: Location::BlockDevice { offset },
        size_blocks,
        read_ref: 0,
        write_ref: 0,
        tsc,
    }
}

/// Mirrors `lookup` — the logic after the wait_for guard succeeds.
/// The real code: waits for write_ref==0, then increments read_ref and returns location.
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[requires(inv_write_binary(entry))]
#[ensures((^entry).read_ref == (*entry).read_ref + 1u32)]
#[ensures((^entry).write_ref == 0u32)]
#[ensures(inv_write_binary(&^entry))]
pub fn lookup(entry: &mut DispatchEntry, tsc: u64) -> Result<(), DispatchMapError> {
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return Err(DispatchMapError::RefCountOverflow),
    };
    entry.tsc = tsc;
    Ok(())
}

/// Mirrors `take_read` — the logic after the wait_for guard succeeds.
/// The real code: checked_add(1), or RefCountOverflow.
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[requires(inv_write_binary(entry))]
#[ensures((^entry).read_ref == (*entry).read_ref + 1u32)]
#[ensures((^entry).write_ref == 0u32)]
#[ensures(inv_write_binary(&^entry))]
pub fn take_read(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return Err(DispatchMapError::RefCountOverflow),
    };
    Ok(())
}

/// Mirrors `take_write` — the logic after the wait_for guard succeeds.
/// The real code: entry.write_ref = 1.
#[requires((*entry).read_ref == 0u32)]
#[requires((*entry).write_ref == 0u32)]
#[ensures((^entry).write_ref == 1u32)]
#[ensures((^entry).read_ref == 0u32)]
#[ensures(inv_write_binary(&^entry))]
pub fn take_write(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    entry.write_ref = 1;
    Ok(())
}

/// Mirrors `release_read` — identical logic to the real code.
#[requires(inv_write_binary(entry))]
#[requires((*entry).read_ref > 0u32)]
#[ensures((^entry).read_ref == (*entry).read_ref - 1u32)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn release_read(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.read_ref == 0 {
        return Err(DispatchMapError::RefCountUnderflow);
    }
    entry.read_ref -= 1;
    Ok(())
}

/// Mirrors `release_write` — identical logic to the real code.
#[requires(inv_write_binary(entry))]
#[requires((*entry).write_ref > 0u32)]
#[ensures((^entry).write_ref == 0u32)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn release_write(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.write_ref == 0 {
        return Err(DispatchMapError::RefCountUnderflow);
    }
    entry.write_ref = 0;
    Ok(())
}

/// Mirrors `downgrade_reference` — identical logic to the real code.
#[requires(inv_write_binary(entry))]
#[requires((*entry).write_ref > 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[ensures((^entry).write_ref == 0u32)]
#[ensures((^entry).read_ref == (*entry).read_ref + 1u32)]
#[ensures(inv_write_binary(&^entry))]
#[ensures((^entry).location == (*entry).location)]
pub fn downgrade_reference(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.write_ref == 0 {
        return Err(DispatchMapError::NoWriteReference);
    }
    entry.write_ref = 0;
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return Err(DispatchMapError::RefCountOverflow),
    };
    Ok(())
}

/// Mirrors `remove` guard — identical logic to the real code.
#[requires(inv_write_binary(entry))]
#[ensures(match result {
    Ok(()) => no_active_refs(entry),
    Err(_) => (*entry).read_ref > 0u32 || (*entry).write_ref > 0u32,
})]
pub fn check_removable(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.read_ref > 0 || entry.write_ref > 0 {
        return Err(DispatchMapError::ActiveReferences);
    }
    Ok(())
}

/// Mirrors `convert_to_storage` state transition logic.
/// On Staging: transitions to BlockDevice.
/// On MemoryTier: sets ssd_offset.
/// On BlockDevice: returns InvalidState.
#[requires(inv_write_binary(entry))]
#[ensures(inv_write_binary(&^entry))]
#[ensures(match result {
    Ok(()) => (^entry).write_ref == (*entry).write_ref
        && ((*entry).read_ref == 0u32 ==> (^entry).read_ref == 0u32)
        && ((*entry).read_ref > 0u32 ==> (^entry).read_ref == (*entry).read_ref - 1u32),
    Err(_) => (^entry).write_ref == (*entry).write_ref && (^entry).read_ref == (*entry).read_ref,
})]
// Returns Ok when the entry is Staging or MemoryTier; Err only for BlockDevice.
#[ensures(match (*entry).location {
    Location::BlockDevice { .. } => match result { Ok(()) => false, Err(_) => true },
    _ => match result { Ok(()) => true, Err(_) => false },
})]
// When called on a MemoryTier entry with exactly one reader and no writer:
// both ref counts are zero after the call (write-through lifecycle safety).
#[ensures(match (*entry).location {
    Location::MemoryTier { .. } =>
        (*entry).read_ref == 1u32 && (*entry).write_ref == 0u32
        ==> (^entry).read_ref == 0u32 && (^entry).write_ref == 0u32,
    _ => true,
})]
pub fn convert_to_storage(
    entry: &mut DispatchEntry,
    new_offset: u64,
) -> Result<(), DispatchMapError> {
    match &mut entry.location {
        Location::Staging { .. } => {
            entry.location = Location::BlockDevice { offset: new_offset };
        }
        Location::MemoryTier { ssd_offset, .. } => {
            *ssd_offset = Some(new_offset);
        }
        Location::BlockDevice { .. } => {
            return Err(DispatchMapError::InvalidState);
        }
    }

    if entry.read_ref > 0 {
        entry.read_ref -= 1;
    }

    Ok(())
}

/// Mirrors `convert_memory_tier_to_block` — identical logic.
#[requires(inv_write_binary(entry))]
#[ensures(inv_write_binary(&^entry))]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
pub fn convert_memory_tier_to_block(
    entry: &mut DispatchEntry,
) -> Result<(), DispatchMapError> {
    match &entry.location {
        Location::MemoryTier {
            ssd_offset: Some(offset),
            ..
        } => {
            let offset = *offset;
            entry.location = Location::BlockDevice { offset };
        }
        Location::MemoryTier {
            ssd_offset: None, ..
        } => {
            return Err(DispatchMapError::InvalidState);
        }
        _ => {
            return Err(DispatchMapError::InvalidState);
        }
    }
    Ok(())
}

/// Mirrors `is_evictable` — identical logic.
#[requires(inv_write_binary(entry))]
pub fn is_evictable(entry: &DispatchEntry) -> bool {
    entry.read_ref == 0
        && entry.write_ref == 0
        && matches!(
            entry.location,
            Location::MemoryTier {
                ssd_offset: Some(_),
                ..
            }
        )
}

/// Mirrors `touch` — updates the entry's timestamp.
#[requires(inv_write_binary(entry))]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn touch(entry: &mut DispatchEntry, tsc: u64) {
    entry.tsc = tsc;
}

// ---------- Lifecycle proofs ----------
// These prove that correct sequences of operations maintain invariants.

/// Proves: create_staging → release_write → take_read → release_read leaves
/// the entry in a removable state. This is the canonical read path.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_staging_read() -> DispatchEntry {
    let mut e = create_staging(4, 1, 0);
    let _ = release_write(&mut e);
    let _ = take_read(&mut e);
    let _ = release_read(&mut e);
    e
}

/// Proves: create_staging → downgrade → release_read leaves removable state.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_downgrade() -> DispatchEntry {
    let mut e = create_staging(4, 1, 0);
    let _ = downgrade_reference(&mut e);
    let _ = release_read(&mut e);
    e
}

/// Proves: create → release_write → take_write → convert_to_storage preserves invariant.
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_staging_to_block() -> DispatchEntry {
    let mut e = create_staging(4, 1, 0);
    let _ = release_write(&mut e);
    let _ = take_write(&mut e);
    let _ = convert_to_storage(&mut e, 8192);
    e
}

/// Proves: recover_extent produces a removable entry with the write-binary invariant.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_recover_extent() -> DispatchEntry {
    recover_extent(4096, 8, 0)  // size_blocks=8 satisfies new #[requires(size_blocks > 0u32)]
}

/// Proves: create_memory_tier → convert_to_storage (sets ssd_offset) →
/// release_write → take_write → convert_memory_tier_to_block → release_write
/// produces a removable block-device entry.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_memory_tier_to_block() -> DispatchEntry {
    let mut e = create_memory_tier_entry(1, 4096, 0);
    let _ = convert_to_storage(&mut e, 8192);
    let _ = release_write(&mut e);
    let _ = take_write(&mut e);
    let _ = convert_memory_tier_to_block(&mut e);
    let _ = release_write(&mut e);
    e
}

/// Proves: create_staging → release_write → lookup → release_read leaves removable.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_lookup() -> DispatchEntry {
    let mut e = create_staging(4, 1, 0);
    let _ = release_write(&mut e);
    let _ = lookup(&mut e, 100);
    let _ = release_read(&mut e);
    e
}

// ============================================================
// System-level property from spec FR-003 / FR-004 / FR-005
// ============================================================
// Property: Write-through lifecycle safety — "write-before-evict"
//
// Derived from the Dispatcher Cache Interface specification:
//   FR-003: populate creates a MemoryTier entry with write_ref=1
//   FR-004: background writer downgrades write→read ref after GPU copy,
//           then releases the read ref on write-through completion
//   FR-005: entry remains in memory-tier after write-through
//
// The system-level invariant we prove:
//   DURING write-through  → entry has active references → NOT evictable
//   AFTER  write-through  → no active references       → IS evictable
//
// This closes the "write-before-evict" safety argument at the entry level:
// an entry can only be cleanly evicted after its data is durably on SSD
// and no reader holds a reference to it.

/// Spec property (FR-003/FR-004/FR-005): an entry is not evictable during
/// write-through and becomes evictable only after write-through completes.
///
/// Step 1 — populate: create_memory_tier_entry → write_ref=1, read_ref=0
///          NOT evictable: write_ref > 0
/// Step 2 — GPU copy done: downgrade_reference → write_ref=0, read_ref=1
///          background writer holds the read ref during SSD write
///          NOT evictable: read_ref > 0
/// Step 3 — write-through done: convert_to_storage sets ssd_offset AND
///          releases the read ref (decrements read_ref by 1)
///          NOW evictable: read_ref=0, write_ref=0
#[requires(size > 0u32)]
#[requires(size@ + 4095 <= u32::MAX@)]
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_write_through_safety(
    mem_handle: u64,
    size: u32,
    ssd_offset: u64,
    tsc: u64,
) -> DispatchEntry {
    // Step 1: populate — entry created with write_ref=1 (not evictable)
    let mut e = create_memory_tier_entry(mem_handle, size, tsc);

    // Step 2: GPU DMA copy complete — background writer downgrades to read ref
    // write_ref → 0, read_ref → 1.  Entry still NOT evictable: read_ref > 0.
    let _ = downgrade_reference(&mut e);

    // Bridge: establish the exact state before convert_to_storage.
    // read_ref=1 (0+1), write_ref=0 (downgraded), location=MemoryTier (unchanged).
    // These are the exact conditions the postcondition needs to fire.
    proof_assert!(e.read_ref  == 1u32);
    proof_assert!(e.write_ref == 0u32);
    proof_assert!(match e.location { Location::MemoryTier { .. } => true, _ => false });

    // Step 3: write-through completes — ssd_offset set, read ref released.
    // Entry is MemoryTier so convert_to_storage returns Ok and decrements read_ref.
    let r = convert_to_storage(&mut e, ssd_offset);
    proof_assert!(match r { Ok(()) => true, Err(_) => false }); // MemoryTier → Ok
    // From new postcondition: MemoryTier + read_ref=1 + write_ref=0 → no_active_refs
    proof_assert!(e.read_ref == 0u32 && e.write_ref == 0u32);
    // Now: read_ref=0, write_ref=0 → no_active_refs holds → IS evictable
    e
}

// ============================================================
// Ref-count balance: round-trip proofs
// ============================================================
// Property: every take_* must be matched by a corresponding release_*.
// A balanced sequence leaves read_ref and write_ref exactly unchanged.
// This rules out both reference leaks (take without release) and
// double-frees (release without a prior take).

/// Single read round-trip: take_read then release_read returns read_ref
/// to its original value. Net change = 0.
#[requires(inv_write_binary(entry))]
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn roundtrip_read(entry: &mut DispatchEntry) {
    let _ = take_read(entry);
    let _ = release_read(entry);
}

/// Single write round-trip: take_write then release_write returns write_ref
/// to its original value. Net change = 0.
#[requires(inv_write_binary(entry))]
#[requires((*entry).read_ref == 0u32)]
#[requires((*entry).write_ref == 0u32)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn roundtrip_write(entry: &mut DispatchEntry) {
    let _ = take_write(entry);
    let _ = release_write(entry);
}

/// Downgrade round-trip: take_write → downgrade_reference → release_read
/// returns both ref counts to their original values. Net change = 0.
/// This proves downgrade is not a leak: write ref is converted to read ref,
/// then released — no net ref is held after the sequence.
#[requires(inv_write_binary(entry))]
#[requires((*entry).read_ref == 0u32)]
#[requires((*entry).write_ref == 0u32)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn roundtrip_downgrade(entry: &mut DispatchEntry) {
    let _ = take_write(entry);
    let _ = downgrade_reference(entry);
    let _ = release_read(entry);
}

/// After take_read, the entry has an active reader (read_ref > 0).
/// Since is_evictable requires read_ref == 0, active readers always prevent eviction.
/// This closes the safety argument: data being read cannot be evicted from under the reader.
#[requires(inv_write_binary(entry))]
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[ensures((^entry).read_ref > 0u32)]
#[ensures(inv_write_binary(&^entry))]
pub fn take_read_prevents_eviction(entry: &mut DispatchEntry) {
    let _ = take_read(entry);
}

/// Two concurrent readers round-trip: two take_reads followed by two
/// release_reads returns read_ref to its original value.
/// Proves balance holds even when multiple readers hold refs simultaneously.
#[requires(inv_write_binary(entry))]
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ + 2 <= u32::MAX@)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn roundtrip_two_concurrent_reads(entry: &mut DispatchEntry) {
    let _ = take_read(entry);
    let _ = take_read(entry);
    let _ = release_read(entry);
    let _ = release_read(entry);
}

// ============================================================
// Eviction ordering: TSC-based fairness proofs
// ============================================================
// Property: oldest_keys() sorts entries by tsc ascending and returns
// the first n. Cold (low-tsc) entries are always evicted before hot ones.
//
// We cannot verify stdlib sort_unstable_by_key directly — that is a
// trusted axiom from the standard library. What we prove here is:
//   GIVEN a tsc-sorted list  →  the first n entries are all colder
//   than every entry not selected.
//
// This is the eviction fairness guarantee the evictor relies on.

/// Predicate: a Vec of DispatchEntry is sorted by tsc ascending (coldest first).
#[logic]
pub fn tsc_sorted(entries: &Vec<DispatchEntry>) -> bool {
    pearlite! {
        forall<i: Int, j: Int>
            0 <= i && i < j && j < entries@.len()
            ==> entries@[i].tsc@ <= entries@[j].tsc@
    }
}

/// Predicate: every entry at index < n has tsc ≤ every entry at index ≥ n.
/// This is the eviction fairness guarantee: the first n candidates returned
/// by oldest_keys are always at least as cold as every entry not selected.
#[logic]
pub fn prefix_colder_than_suffix(entries: &Vec<DispatchEntry>, n: usize) -> bool {
    pearlite! {
        forall<i: Int, j: Int>
            0 <= i && i < n@ && n@ <= j && j < entries@.len()
            ==> entries@[i].tsc@ <= entries@[j].tsc@
    }
}

/// Core eviction fairness lemma: a TSC-sorted list guarantees its first n
/// entries are all colder than (or equal to) all remaining entries.
///
/// Proof: take any i < n and j >= n. Since i < n <= j, we have i < j.
/// By tsc_sorted, entries[i].tsc <= entries[j].tsc. QED.
#[requires(n@ <= entries@.len())]
#[requires(tsc_sorted(entries))]
#[ensures(prefix_colder_than_suffix(entries, n))]
pub fn eviction_fairness(entries: &Vec<DispatchEntry>, n: usize) {}

/// Concrete lifecycle: cold entry (low tsc) always precedes hot entry (high tsc)
/// in a correctly sorted eviction list. If the evictor picks 1 entry, it is
/// always the cold one — the hot entry is never evicted first.
#[requires(cold_tsc@ < hot_tsc@)]
#[ensures(result@.len() == 2)]
#[ensures(result@[0].tsc@ == cold_tsc@)]
#[ensures(result@[1].tsc@ == hot_tsc@)]
#[ensures(tsc_sorted(&result))]
#[ensures(prefix_colder_than_suffix(&result, 1usize))]
pub fn lifecycle_cold_evicted_before_hot(
    cold_tsc: u64,
    hot_tsc: u64,
) -> Vec<DispatchEntry> {
    let cold = DispatchEntry {
        location: Location::BlockDevice { offset: 0 },
        size_blocks: 1,
        read_ref: 0,
        write_ref: 0,
        tsc: cold_tsc,
    };
    let hot = DispatchEntry {
        location: Location::BlockDevice { offset: 0 },
        size_blocks: 1,
        read_ref: 0,
        write_ref: 0,
        tsc: hot_tsc,
    };
    let mut result = Vec::new();
    result.push(cold);
    result.push(hot);
    proof_assert!(result@[0].tsc@ == cold_tsc@);
    proof_assert!(result@[1].tsc@ == hot_tsc@);
    proof_assert!(tsc_sorted(&result));
    result
}

// ========== P30 / P31: map-wide invariants (the L1 -> L2 lift) ==========
//
// Every function above reasons about ONE `&mut DispatchEntry`. That is the
// per-entry (L1) evidence: `inv_write_binary` is threaded through every op, and
// the refcount roundtrips prove local consistency. What was missing is the
// map-wide (L2) theorem: that these facts hold for EVERY key simultaneously and
// are preserved by the operations that change map membership. That gluing step
// -- quantifying a per-entry predicate over a whole `FMap` and showing each
// map mutation preserves it -- is exactly assumption A7's "per-entry + composition
// -> whole-map" gap made explicit and discharged.
//
// P30 (exclusive state): each present key is in exactly ONE logical Location
//   (Staging XOR BlockDevice XOR MemoryTier). In the runtime this is enforced by
//   construction -- `Location` is a Rust enum and each key maps to one
//   `DispatchEntry` -- so `exactly_one_state` is provably true for ANY entry
//   (`lemma_exclusive_state`, no precondition). Stating it explicitly turns the
//   "exactly one state" folklore into a machine-checked fact.
// P31 (refcount/state consistency): `write_ref` is binary (0 or 1) for every
//   present entry -- the cross-cutting refcount well-formedness the per-entry
//   ops already preserve locally.
//
// `map_inv` conjoins both over all present keys. We then prove the three shapes
// of map mutation preserve it:
//   * insert-fresh  (create a new key)      -> `map_create_entry`
//   * overwrite     (replace one key's entry, e.g. after an L1 refcount op)
//                                            -> `map_update_entry`
//   * remove        (delete a key)          -> `map_remove_entry`
// Any dispatch-map operation is one of these applied to a well-formed entry, so
// closing all three closes the map-wide invariant under the whole API.
//
// SCOPE: this is L2 (whole-map, sequential). It does NOT model concurrent
// interleavings -- the map is guarded by a Mutex in the runtime, collapsed to a
// sequential ghost map here (same boundary noted for P19/write-through).

/// P30 helper: the entry's location is the Staging variant.
#[logic(open)]
pub fn is_staging(e: DispatchEntry) -> bool {
    pearlite! { match e.location { Location::Staging { .. } => true, _ => false } }
}

/// P30 helper: the entry's location is the BlockDevice variant.
#[logic(open)]
pub fn is_block(e: DispatchEntry) -> bool {
    pearlite! { match e.location { Location::BlockDevice { .. } => true, _ => false } }
}

/// P30 helper: the entry's location is the MemoryTier variant.
#[logic(open)]
pub fn is_memtier(e: DispatchEntry) -> bool {
    pearlite! { match e.location { Location::MemoryTier { .. } => true, _ => false } }
}

/// P30 per-entry: the entry is in EXACTLY ONE logical state.
#[logic(open)]
pub fn exactly_one_state(e: DispatchEntry) -> bool {
    pearlite! {
        (is_staging(e) && !is_block(e) && !is_memtier(e))
     || (!is_staging(e) && is_block(e) && !is_memtier(e))
     || (!is_staging(e) && !is_block(e) && is_memtier(e))
    }
}

/// P31 per-entry: write_ref is binary (mirrors `inv_write_binary` as a by-value
/// predicate usable inside the map-wide quantifier).
#[logic(open)]
pub fn write_binary(e: DispatchEntry) -> bool {
    pearlite! { e.write_ref == 0u32 || e.write_ref == 1u32 }
}

/// The map-wide invariant: every present key satisfies P30 (exclusive state)
/// and P31 (binary write_ref).
#[logic(open)]
pub fn map_inv(m: FMap<u64, DispatchEntry>) -> bool {
    pearlite! {
        forall<k: u64> m.contains(k) ==> exactly_one_state(m.lookup(k)) && write_binary(m.lookup(k))
    }
}

/// P30 (unconditional): exclusive state holds for ANY entry -- a key can never be
/// in two Locations at once, because `Location` is a single-variant enum. Proved
/// with no precondition, so exclusivity is structural, not an assumption.
#[check(ghost)]
#[ensures(exactly_one_state(e))]
pub fn lemma_exclusive_state(e: DispatchEntry) {
    let _ = e;
}

/// P30 + P31 map-wide, insert-fresh: creating a new (absent) key with a
/// well-formed entry preserves the map invariant.
#[check(ghost)]
#[requires(map_inv(*m))]
#[requires(!(*m).contains(key))]
#[requires(write_binary(entry))]
#[ensures((^m).contains(key))]
#[ensures(map_inv(^m))]
pub fn map_create_entry(m: &mut FMap<u64, DispatchEntry>, key: u64, entry: DispatchEntry) {
    let _ = m.insert_ghost(key, entry);
}

/// P30 + P31 map-wide, overwrite: replacing one key's entry with a well-formed
/// entry (e.g. the result of an L1 refcount op) preserves the map invariant.
#[check(ghost)]
#[requires(map_inv(*m))]
#[requires(write_binary(entry))]
#[ensures(map_inv(^m))]
pub fn map_update_entry(m: &mut FMap<u64, DispatchEntry>, key: u64, entry: DispatchEntry) {
    let _ = m.insert_ghost(key, entry);
}

/// P30 + P31 map-wide, remove: deleting a key preserves the map invariant
/// (removing a key cannot break a forall-over-present-keys property).
#[check(ghost)]
#[requires(map_inv(*m))]
#[ensures(!(^m).contains(key))]
#[ensures(map_inv(^m))]
pub fn map_remove_entry(m: &mut FMap<u64, DispatchEntry>, key: u64) {
    let _ = m.remove_ghost(&key);
}

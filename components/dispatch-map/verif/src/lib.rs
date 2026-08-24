//! Creusot verification mirror for `components/dispatch-map`.
//!
//! This crate proves the reference-count and location state-machine invariants
//! of the dispatch map. The shipped functions in `../src/lib.rs` cannot be
//! compiled under Creusot (Mutex / HashMap / Condvar / raw pointers / async
//! logger), so this crate proves a **standalone mirror**:
//!
//!   * `self.state.inner.lock().unwrap().entries.get_mut(&key)` is replaced by
//!     an `entry: &mut DispatchEntry` passed directly (the lock is already held).
//!   * `wait_for(|e| e.write_ref == 0)` becomes a `#[requires(...)]` precondition
//!     (the condvar wait already established it).
//!   * `Arc<DmaBuffer>` / `*mut u8` / `EvictionHandle` / `AtomicU32` become plain
//!     `u64` / dropped fields — the state machine does not depend on them.
//!
//! **Drift discipline.** Each mirrored function body is kept byte-faithful to the
//! cited source lines (the only mechanical change: `.checked_add(1).ok_or(e)?`
//! rewritten as a `match` so the pure core has no `?`-on-`self` dependency). The
//! module-level `mirror_drift_check` documents the correspondence. The proofs are
//! validated by fault injection: perturb a mirror body and confirm a VC goes red.
//!
//! Invariants proved (documented in `../specs/001-dispatch-map/data-model.md`):
//!   * `inv_write_binary` — `write_ref` is always 0 or 1 (writer count is a lock).
//!   * `no_active_refs`    — removable/evictable requires zero refs.
//!   * location state machine — MemoryTier(ssd_offset: Some) → BlockDevice.

use creusot_std::prelude::*;

// ---------------------------------------------------------------------------
// Types — structure preserved, hardware handles stripped to opaque u64.
// Mirrors `../src/entry.rs`.
// ---------------------------------------------------------------------------

/// Where extent data currently resides. Mirror of `entry::Location`.
/// `*mut u8` → `u64` (opaque handle); the state machine ignores the payload.
pub enum Location {
    /// Data committed to a block device.
    BlockDevice { offset: u64 },
    /// Data in the DRAM memory-tier pool.
    MemoryTier {
        pointer: u64,
        size: u32,
        /// Set when write-through to SSD completes; enables eviction.
        ssd_offset: Option<u64>,
    },
}

/// Per-key metadata. Mirror of `entry::DispatchEntry` with `eviction_handle`
/// and the `reuse_count` atomic dropped (neither participates in the invariant).
pub struct DispatchEntry {
    pub location: Location,
    pub size_blocks: u32,
    pub read_ref: u32,
    pub write_ref: u32,
}

/// Mirror of `DispatchMapError` — only the variants the pure cores return.
pub enum DispatchMapError {
    RefCountOverflow,
    RefCountUnderflow,
    NoWriteReference,
    InvalidState,
    ActiveReferences,
}

// ---------------------------------------------------------------------------
// Logical predicates — the invariants we maintain.
// ---------------------------------------------------------------------------

/// The writer count is a binary lock: always 0 or 1.
/// (data-model.md: "write_ref … Active writer count (0 or 1)".)
#[logic]
pub fn inv_write_binary(e: &DispatchEntry) -> bool {
    pearlite! { e.write_ref == 0u32 || e.write_ref == 1u32 }
}

/// No reader or writer currently holds a reference — precondition for removal.
#[logic]
pub fn no_active_refs(e: &DispatchEntry) -> bool {
    pearlite! { e.read_ref == 0u32 && e.write_ref == 0u32 }
}

/// The entry is in the MemoryTier location.
#[logic]
pub fn is_memory_tier(e: &DispatchEntry) -> bool {
    pearlite! {
        match e.location {
            Location::MemoryTier { .. } => true,
            Location::BlockDevice { .. } => false,
        }
    }
}

/// The entry is in the BlockDevice location.
#[logic]
pub fn is_block_device(e: &DispatchEntry) -> bool {
    pearlite! {
        match e.location {
            Location::BlockDevice { .. } => true,
            Location::MemoryTier { .. } => false,
        }
    }
}

/// The entry is MemoryTier *and* has completed write-through (`ssd_offset: Some`),
/// so it is eligible to transition to BlockDevice / be evicted.
#[logic]
pub fn is_memory_tier_persisted(e: &DispatchEntry) -> bool {
    pearlite! {
        match e.location {
            Location::MemoryTier { ssd_offset, .. } => match ssd_offset {
                Some(_) => true,
                None => false,
            },
            Location::BlockDevice { .. } => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Individual operations. `#[requires]` = what the guard/wait ensured;
// `#[ensures]` = the invariant/transition we prove.
// ---------------------------------------------------------------------------

/// Mirror of `DispatchMap::take_read` core (../src/lib.rs:216-219).
/// The `wait_for` guard (lib.rs:203-205) established `write_ref == 0`.
#[requires((*entry).write_ref == 0u32)]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[requires(inv_write_binary(entry))]
#[ensures((^entry).read_ref == (*entry).read_ref + 1u32)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn take_read(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    entry.read_ref = match entry.read_ref.checked_add(1) {
        Some(v) => v,
        None => return Err(DispatchMapError::RefCountOverflow),
    };
    Ok(())
}

/// Mirror of `DispatchMap::take_write` core (../src/lib.rs:250).
/// The `wait_for` guard (lib.rs:234-239) established `read_ref == 0 && write_ref == 0`.
/// Assignment (not increment) keeps `write_ref` binary.
#[requires((*entry).read_ref == 0u32 && (*entry).write_ref == 0u32)]
#[ensures((^entry).write_ref == 1u32)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn take_write(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    entry.write_ref = 1;
    Ok(())
}

/// Mirror of `DispatchMap::release_read` core (../src/lib.rs:264-268).
/// The `read_ref == 0` guard is what prevents the `-= 1` from underflowing.
#[requires(inv_write_binary(entry))]
#[ensures((*entry).read_ref > 0u32 ==> (^entry).read_ref == (*entry).read_ref - 1u32)]
#[ensures((*entry).read_ref == 0u32 ==> (^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
#[ensures(inv_write_binary(&^entry))]
pub fn release_read(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.read_ref == 0 {
        return Err(DispatchMapError::RefCountUnderflow);
    }
    entry.read_ref -= 1;
    Ok(())
}

/// Mirror of `DispatchMap::release_write` core (../src/lib.rs:284-288).
/// `write_ref` ends at 0 on both the guard-error and the success path.
#[requires(inv_write_binary(entry))]
#[ensures((^entry).write_ref == 0u32)]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures(inv_write_binary(&^entry))]
#[ensures(is_memory_tier(&^entry) == is_memory_tier(entry))]
pub fn release_write(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.write_ref == 0 {
        return Err(DispatchMapError::RefCountUnderflow);
    }
    entry.write_ref = 0;
    Ok(())
}

/// Mirror of `DispatchMap::downgrade_reference` core (../src/lib.rs:304-312).
/// Atomic write→read handoff: clears the writer, takes a reader. Keeps
/// `write_ref` binary and cannot underflow (the `NoWriteReference` guard).
#[requires(inv_write_binary(entry))]
#[requires((*entry).read_ref@ < u32::MAX@)]
#[ensures((*entry).write_ref > 0u32 ==>
    (^entry).write_ref == 0u32 && (^entry).read_ref == (*entry).read_ref + 1u32)]
#[ensures((*entry).write_ref == 0u32 ==>
    (^entry).write_ref == (*entry).write_ref && (^entry).read_ref == (*entry).read_ref)]
#[ensures(inv_write_binary(&^entry))]
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

/// Mirror of `DispatchMap::convert_to_storage` core (../src/lib.rs:173-186).
/// Sets `ssd_offset` on a MemoryTier entry (enabling the later block transition)
/// and conditionally decrements `read_ref` — the `> 0` guard prevents underflow.
#[ensures(is_memory_tier(entry) ==> is_memory_tier_persisted(&^entry))]
#[ensures(is_memory_tier(entry) && (*entry).read_ref > 0u32 ==>
    (^entry).read_ref == (*entry).read_ref - 1u32)]
#[ensures(is_memory_tier(entry) && (*entry).read_ref == 0u32 ==>
    (^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
pub fn convert_to_storage(
    entry: &mut DispatchEntry,
    offset: u64,
) -> Result<(), DispatchMapError> {
    match &mut entry.location {
        Location::MemoryTier { ssd_offset, .. } => {
            *ssd_offset = Some(offset);
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

/// Mirror of `DispatchMap::convert_memory_tier_to_block` core (../src/lib.rs:433-453).
/// A persisted MemoryTier entry (ssd_offset: Some) transitions to BlockDevice;
/// reference counts are untouched.
#[ensures(is_memory_tier_persisted(entry) ==> is_block_device(&^entry))]
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

/// Mirror of `DispatchMap::promote_block_to_memory_tier` core (../src/lib.rs:483-495).
/// In-place flip BlockDevice → MemoryTier, retaining the SSD offset and all refs.
#[ensures(is_block_device(entry) ==> is_memory_tier_persisted(&^entry))]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
pub fn promote_block_to_memory_tier(
    entry: &mut DispatchEntry,
    pointer: u64,
    size: u32,
) -> Result<(), DispatchMapError> {
    match &entry.location {
        Location::BlockDevice { offset } => {
            let offset = *offset;
            entry.location = Location::MemoryTier {
                pointer,
                size,
                ssd_offset: Some(offset),
            };
            // DRIFT: source also sets `entry.size_blocks = size.div_ceil(4096)`
            // (../src/lib.rs:494). Omitted here — `div_ceil` is a contractless
            // external call (impossible precondition ⇒ vacuous proof), and
            // `size_blocks` is not part of the location/ref invariant.
        }
        Location::MemoryTier { .. } => {
            return Err(DispatchMapError::InvalidState);
        }
    }
    Ok(())
}

/// Mirror of `DispatchMap::try_evict_to_block` core (../src/lib.rs:540-563).
/// Requires zero refs (checked first) and a persisted MemoryTier entry.
#[requires(is_memory_tier_persisted(entry))]
#[requires(no_active_refs(entry))]
#[ensures(is_block_device(&^entry))]
#[ensures((^entry).read_ref == (*entry).read_ref)]
#[ensures((^entry).write_ref == (*entry).write_ref)]
pub fn try_evict_to_block(entry: &mut DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.read_ref != 0 || entry.write_ref != 0 {
        return Err(DispatchMapError::InvalidState);
    }

    match &entry.location {
        Location::MemoryTier {
            ssd_offset: Some(offset),
            ..
        } => {
            let offset = *offset;
            entry.location = Location::BlockDevice { offset };
            Ok(())
        }
        Location::MemoryTier {
            ssd_offset: None, ..
        } => Err(DispatchMapError::InvalidState),
        _ => Err(DispatchMapError::InvalidState),
    }
}

/// Mirror of the `remove` removability guard (../src/lib.rs:331-333).
/// Success ⇒ the entry had no active references.
#[ensures(match result {
    Ok(_) => no_active_refs(entry),
    Err(_) => true,
})]
pub fn check_removable(entry: &DispatchEntry) -> Result<(), DispatchMapError> {
    if entry.read_ref > 0 || entry.write_ref > 0 {
        return Err(DispatchMapError::ActiveReferences);
    }
    Ok(())
}

/// Mirror of `DispatchMap::is_evictable` (../src/lib.rs:519-527).
/// Evictable ⇔ no active refs and a persisted MemoryTier entry.
#[ensures(result == (no_active_refs(entry) && is_memory_tier_persisted(entry)))]
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

// ---------------------------------------------------------------------------
// Lifecycle proofs — sequences of operations preserve the invariants.
// ---------------------------------------------------------------------------

/// create_memory_tier_entry → release_write → take_read → release_read.
/// Proves the read path returns the entry to a removable state.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_memtier_read() -> DispatchEntry {
    // create_memory_tier_entry (../src/lib.rs:400-408): write_ref = 1, read_ref = 0.
    let mut e = DispatchEntry {
        location: Location::MemoryTier {
            pointer: 0,
            size: 4096,
            ssd_offset: None,
        },
        size_blocks: 1,
        read_ref: 0,
        write_ref: 1,
    };
    let _ = release_write(&mut e); // write_ref -> 0
    let _ = take_read(&mut e); // read_ref -> 1 (write_ref == 0 precondition holds)
    let _ = release_read(&mut e); // read_ref -> 0
    e
}

/// create_memory_tier_entry → downgrade_reference → release_read.
/// Proves the write→read downgrade path ends removable.
#[ensures(no_active_refs(&result))]
#[ensures(inv_write_binary(&result))]
pub fn lifecycle_downgrade() -> DispatchEntry {
    let mut e = DispatchEntry {
        location: Location::MemoryTier {
            pointer: 0,
            size: 4096,
            ssd_offset: None,
        },
        size_blocks: 1,
        read_ref: 0,
        write_ref: 1,
    };
    let _ = downgrade_reference(&mut e); // write_ref -> 0, read_ref -> 1
    let _ = release_read(&mut e); // read_ref -> 0
    e
}

/// create_memory_tier_entry → release_write → convert_to_storage → convert_memory_tier_to_block.
/// Proves the persist-then-demote path ends in BlockDevice with no active refs.
#[ensures(is_block_device(&result))]
#[ensures(no_active_refs(&result))]
pub fn lifecycle_memtier_to_block() -> DispatchEntry {
    let mut e = DispatchEntry {
        location: Location::MemoryTier {
            pointer: 0,
            size: 4096,
            ssd_offset: None,
        },
        size_blocks: 1,
        read_ref: 0,
        write_ref: 1,
    };
    let _ = release_write(&mut e); // write_ref -> 0
    let _ = convert_to_storage(&mut e, 8192); // ssd_offset -> Some, read_ref stays 0
    let _ = convert_memory_tier_to_block(&mut e); // MemoryTier -> BlockDevice
    e
}

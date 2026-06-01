use creusot_std::prelude::*;

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
#[ensures(inv_write_binary(&result))]
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
#[ensures(result.write_ref == 0u32)]
#[ensures(result.read_ref == 0u32)]
#[ensures(result.size_blocks == size_blocks)]
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
    recover_extent(4096, 8, 0)
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

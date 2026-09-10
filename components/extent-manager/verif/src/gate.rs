//! Global invariant **EM-INIT-GATE** (rank 1, attachments 8 — highest leverage).
//!
//! No data-path op acts before `format`/`initialize` populates `regions`+`shared`.
//! The source expresses this uniformly as `Option::as_ref().ok_or_else(...)`
//! (lib.rs:216-218, 234-236, 279-281, 688-690) or `.map_or(default, ...)`
//! (lib.rs:700, 712, 634, 660). Per reconciliation flag F-6 the ONE obligation
//! manifests three ways; this module mirrors each shape and proves the gate is
//! taken exactly when the state is absent, returning the sentinel with no state
//! change. Discharging it here propagates to all 8 data-path methods by id.
//!
//! Also folds **EM-INSTANCEID-RETURNS** (gated read of the stored id) and
//! **EM-FORMAT-META-NOT-CONNECTED** (receptacle `.get()` absent -> NotInitialized,
//! the identical Option gate). Source: FR-026, US3.

use creusot_std::prelude::*;

/// Mirror of the `interfaces::ExtentManagerError` variants reached on the gated
/// paths (interfaces/src/iextent_manager.rs:18-25). Payload strings are elided —
/// the observable obligation is the variant, not the message text.
#[derive(DeepModel)]
pub enum EmError {
    CorruptMetadata,
    IoError,
    NotInitialized,
    OffsetNotFound,
    OutOfSpace,
}

/// **Shape 1 — Err(NotInitialized).** Mirror of `regions.as_ref().ok_or_else(||
/// not_initialized(...))` used by reserve_extent, remove_extent, checkpoint,
/// get_instance_id, and by `metadata_device.get()` at format
/// (EM-FORMAT-META-NOT-CONNECTED). Absent state gates to NotInitialized; present
/// state passes the value through. No mutation on either branch (by-value read).
#[ensures(state == None ==> result == Err(EmError::NotInitialized))]
#[ensures(forall<x: u64> state == Some(x) ==> result == Ok(x))]
pub fn gate_result(state: Option<u64>) -> Result<u64, EmError> {
    match state {
        Some(x) => Ok(x),
        None => Err(EmError::NotInitialized),
    }
}

/// **EM-INSTANCEID-RETURNS** (FR-026): a specialization of shape 1 — when
/// initialized, returns the `superblock.instance_id` stored at format
/// (lib.rs:686-692); otherwise NotInitialized.
#[ensures(shared == None ==> result == Err(EmError::NotInitialized))]
#[ensures(forall<id: u64> shared == Some(id) ==> result == Ok(id))]
pub fn get_instance_id(shared: Option<u64>) -> Result<u64, EmError> {
    gate_result(shared)
}

/// **Shape 2 — default scalar 0.** Mirror of `regions.as_ref().map_or(0, ...)`
/// used by used_bytes/capacity_bytes (lib.rs:700, 712): absent state yields 0
/// without touching any region.
#[ensures(state == None ==> result@ == 0)]
#[ensures(forall<x: u64> state == Some(x) ==> result == x)]
pub fn gate_scalar(state: Option<u64>) -> u64 {
    match state {
        Some(x) => x,
        None => 0,
    }
}

/// **Shape 3 — empty enumeration / no-op.** Mirror of the `match regions.as_ref()
/// { Some => .., None => Vec::new() }` in get_extents (lib.rs:632-655) and the
/// `if let Some(..)` no-op in for_each_extent (lib.rs:659-677): when the
/// component is uninitialized, zero extents are produced and the callback never
/// fires. `n` is the count that would be produced when initialized.
#[ensures(state == None ==> result@ == 0)]
#[ensures(forall<k: usize> state == Some(k) ==> result == k)]
pub fn gate_count(state: Option<usize>) -> usize {
    match state {
        Some(k) => k,
        None => 0,
    }
}

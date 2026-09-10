//! **EM-SHARD-BY-KEY** (reserve_extent, |methods|=1) and the region-index half
//! of **EM-REMOVE-OFFSET-ROUTING**.
//!
//! `region_for_key`: mirror of `key as usize & (regions.len() - 1)` (lib.rs:219).
//! `region_count` is validated a positive power of two at format (lib.rs:397),
//! so the masked index is always in `[0, region_count)` and `regions[idx]`
//! (lib.rs:220) is in bounds. `region_for_offset_index`: mirror of the
//! `(offset - data_start)/region_bytes` division (lib.rs:249-250) with its
//! explicit `idx >= len` guard. Source: FR-022, FR-012.

use creusot_std::prelude::*;

/// Mirror of `key as usize & (regions.len() - 1)` (lib.rs:219).
#[bitwise_proof]
#[requires(region_count != 0usize)]
#[requires(region_count & (region_count - 1usize) == 0usize)]
#[ensures(result@ < region_count@)]
pub fn region_for_key(key: u64, region_count: usize) -> usize {
    key as usize & (region_count - 1)
}

/// Mirror of the offset-routing index computation in `region_for_offset`
/// (lib.rs:249-253): `idx = (offset.saturating_sub(data_start))/region_bytes`,
/// with the source's `if idx >= len { OffsetNotFound }` guard. The proven
/// property is that whenever `Some(idx)` is returned, `idx < region_count` — so
/// `regions[idx]` (lib.rs:254) is in bounds and out-of-range offsets are
/// rejected (part of EM-REMOVE-NOTFOUND). `region_bytes > 0` is the source's
/// prior `region_bytes == 0` guard (lib.rs:245-247).
#[requires(region_bytes@ > 0)]
#[ensures(forall<i: usize> result == Some(i) ==> i@ < region_count@)]
#[ensures(relative_offset@ / region_bytes@ >= region_count@ ==> result == None)]
pub fn region_for_offset_index(
    relative_offset: u64,
    region_bytes: u64,
    region_count: usize,
) -> Option<usize> {
    let idx = (relative_offset / region_bytes) as usize;
    if idx >= region_count {
        None
    } else {
        Some(idx)
    }
}

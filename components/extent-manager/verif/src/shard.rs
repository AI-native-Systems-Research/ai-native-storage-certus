//! Mirror of the region-sharding index computation
//! `key as usize & (regions.len() - 1)` (src/lib.rs:219).
//!
//! Spec: FR-022 — "Keys MUST be sharded to regions by
//! `key & (region_count - 1)`." Region count is validated to be a positive
//! power of two at format time (src/lib.rs:397, FR-002). The proven property
//! is that the resulting index is always in range `[0, region_count)`, so the
//! subsequent `regions[idx]` access (lib.rs:220) is in bounds.

use creusot_std::prelude::*;

/// `region_count` mirrors `regions.len()`, which equals the validated
/// `region_count` FormatParams field (a positive power of two).
#[bitwise_proof]
#[requires(region_count != 0usize)]
// Power-of-two: exactly the bit pattern format() enforces via is_power_of_two().
#[requires(region_count & (region_count - 1usize) == 0usize)]
#[ensures(result@ < region_count@)]
pub fn region_for_key(key: u64, region_count: usize) -> usize {
    key as usize & (region_count - 1)
}

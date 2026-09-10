//! Global invariant **EM-SECTOR-ALIGN** (rank 3, attachments 3) — arithmetic core.
//!
//! Faithful mirror of `RegionState::align_to_sector_size` (src/region.rs:37-39)
//! and the identical `aligned_size` step in `reserve_extent` (lib.rs:591):
//! `(size + sector_size - 1) / sector_size * sector_size`.
//!
//! Proves the returned size is THE smallest multiple of `sector_size` that is
//! >= the request: multiple-of, covers, minimal (< size + sector_size). This is
//! the alignment lemma the inventory cites for EM-SECTOR-ALIGN. Bundles:
//! reserve_extent, get_extents, for_each_extent. Source: FR-005, FR-002.

use creusot_std::prelude::*;

#[requires(sector_size@ > 0)]
// No overflow in `size + sector_size` (shared latent requirement of the source).
#[requires(size@ + sector_size@ <= u32::MAX@)]
#[ensures(result@ % sector_size@ == 0)]
#[ensures(result@ >= size@)]
#[ensures(result@ < size@ + sector_size@)]
pub fn align_to_sector_size(size: u32, sector_size: u32) -> u32 {
    let n = size + sector_size - 1;
    let q = n / sector_size;
    proof_assert!(n@ == q@ * sector_size@ + n@ % sector_size@);
    proof_assert!(n@ % sector_size@ < sector_size@);
    q * sector_size
}

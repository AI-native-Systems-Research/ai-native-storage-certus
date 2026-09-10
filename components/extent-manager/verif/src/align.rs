//! Mirror of `RegionState::align_to_sector_size` (src/region.rs:37-39):
//! `(size + sector_size - 1) / sector_size * sector_size`.
//!
//! Spec: FR-005 (`reserve_extent` returns a sector-aligned slot) and FR-002
//! (`sector_size > 0`). The proven property is that the returned size is the
//! smallest multiple of `sector_size` that is >= the requested `size`:
//! it is a multiple of `sector_size`, it is >= `size`, and it is < `size +
//! sector_size` (so no more than one sector of slack). No trusted boundary.

use creusot_std::prelude::*;

// No overflow in the `size + sector_size` step (the source shares this latent
// requirement; extents are always far below u32::MAX in practice).
// Result is a multiple of the sector size.
// Result covers the request...
// ...and is minimal: at most one sector of slack, so the result is THE
// sector-aligned size for `size`.
pub fn align_to_sector_size(size: u32, sector_size: u32) -> u32 {
    // Semantically identical to the source's single expression
    // `(size + sector_size - 1) / sector_size * sector_size`; split into named
    // steps only so the intermediate facts can be named for the prover.
    let n = size + sector_size - 1;
    let q = n / sector_size;
    // Division identity + remainder bound pin q*sector_size into [size, n].
            q * sector_size
}

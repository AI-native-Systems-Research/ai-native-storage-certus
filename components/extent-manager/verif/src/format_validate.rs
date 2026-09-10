//! `format`/`reserve_extent` guard mirrors (lib.rs:384-452, region.rs:68-70).
//! Each faithfully reproduces a source branch and proves it returns the exact
//! error variant on the bad condition, and passes otherwise:
//!
//!  * **EM-FORMAT-VALIDATE-SECTOR**  — sector_size == 0 -> CorruptMetadata (lib:384)
//!  * **EM-FORMAT-VALIDATE-SLABMULT**— slab_size % sector_size != 0 -> ~ (lib:387)
//!  * **EM-FORMAT-VALIDATE-MAXEXT**  — max_extent_size > slab_size -> ~ (lib:392)
//!  * **EM-FORMAT-VALIDATE-REGPOW2** — region_count 0 / not pow2 -> ~ (lib:397)
//!  * **EM-FORMAT-META-TOOSMALL**    — checkpoint_region_size == 0 -> ~ (lib:434)
//!  * **EM-FORMAT-NO-DATA-SPACE**    — usable_data_size == 0 -> ~ (lib:448)
//!  * **EM-FORMAT-DATA-LAYOUT**      — data_start offset arithmetic (lib:442-446)
//!  * **EM-RESERVE-OVERSIZE**        — aligned elem > slab_size -> OutOfSpace (region:68)
//!
//! Source: FR-002, FR-035. Re-authored from the inventory in this worktree.

use creusot_std::prelude::*;
use crate::gate::EmError;

/// Mirror of the first three param-validation branches of `format`
/// (lib.rs:384-395), in source order.
/// **EM-FORMAT-VALIDATE-SECTOR / -SLABMULT / -MAXEXT.**
#[ensures(sector_size@ == 0 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(sector_size@ != 0 && slab_size@ % sector_size@ != 0
    ==> result == Err(EmError::CorruptMetadata))]
#[ensures(sector_size@ != 0 && slab_size@ % sector_size@ == 0
    && max_extent_size@ > slab_size@ ==> result == Err(EmError::CorruptMetadata))]
// All three pass => Ok. Confirms the guards are exhaustive and non-vacuous.
#[ensures(sector_size@ != 0 && slab_size@ % sector_size@ == 0
    && max_extent_size@ <= slab_size@ ==> result == Ok(()))]
pub fn validate_format_params(
    sector_size: u32,
    slab_size: u64,
    max_extent_size: u32,
) -> Result<(), EmError> {
    if sector_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    if slab_size % sector_size as u64 != 0 {
        return Err(EmError::CorruptMetadata);
    }
    if max_extent_size as u64 > slab_size {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// **EM-FORMAT-VALIDATE-REGPOW2** (lib.rs:397): `region_count` must be a nonzero
/// power of two, expressed with the standard `n & (n-1) == 0` mask idiom (the
/// same identity `shard::region_for_key` relies on to bound its masked index).
/// The nested-if form keeps `region_count - 1` guarded by `region_count != 0`.
#[bitwise_proof]
#[ensures(region_count == 0u32 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(region_count != 0u32 && region_count & (region_count - 1u32) != 0u32
    ==> result == Err(EmError::CorruptMetadata))]
#[ensures(region_count != 0u32 && region_count & (region_count - 1u32) == 0u32
    ==> result == Ok(()))]
pub fn check_region_count_pow2(region_count: u32) -> Result<(), EmError> {
    if region_count == 0 {
        return Err(EmError::CorruptMetadata);
    }
    if region_count & (region_count - 1) != 0 {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// **EM-FORMAT-DATA-LAYOUT** (FR-035): shared-device (`metadata_region_size > 0`)
/// puts data after the two checkpoint regions; separate-device (`== 0`) puts
/// data at offset 0. Mirror of lib.rs:442-446.
#[requires(checkpoint_region_offset@ + 2 * checkpoint_region_size@ <= u64::MAX@)]
#[ensures(metadata_region_size@ > 0
    ==> result@ == checkpoint_region_offset@ + 2 * checkpoint_region_size@)]
#[ensures(metadata_region_size@ == 0 ==> result@ == 0)]
pub fn data_start_offset(
    metadata_region_size: u64,
    checkpoint_region_offset: u64,
    checkpoint_region_size: u64,
) -> u64 {
    if metadata_region_size > 0 {
        checkpoint_region_offset + 2 * checkpoint_region_size
    } else {
        0
    }
}

/// **EM-FORMAT-META-TOOSMALL** (lib.rs:434): a zero checkpoint region size means
/// the metadata device is too small.
#[ensures(checkpoint_region_size@ == 0 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(checkpoint_region_size@ != 0 ==> result == Ok(()))]
pub fn check_checkpoint_region_size(checkpoint_region_size: u64) -> Result<(), EmError> {
    if checkpoint_region_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// **EM-FORMAT-NO-DATA-SPACE** (lib.rs:448): no usable data space after the
/// metadata reservation.
#[ensures(usable_data_size@ == 0 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(usable_data_size@ != 0 ==> result == Ok(()))]
pub fn check_usable_data_space(usable_data_size: u64) -> Result<(), EmError> {
    if usable_data_size == 0 {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// **EM-RESERVE-OVERSIZE** (region.rs:68-70): a request whose sector-aligned
/// element size exceeds the slab size cannot fit -> OutOfSpace.
#[ensures(element_size@ > slab_size@ ==> result == Err(EmError::OutOfSpace))]
#[ensures(element_size@ <= slab_size@ ==> result == Ok(()))]
pub fn check_element_fits(element_size: u64, slab_size: u64) -> Result<(), EmError> {
    if element_size > slab_size {
        return Err(EmError::OutOfSpace);
    }
    Ok(())
}

//! Recovery / initialize validation (FR-005, FR-024; US2).
//!
//!  * **EM-INIT-VALIDATE-MAGIC** (superblock.rs deserialize, lib.rs:520): a
//!    superblock whose magic != `SUPERBLOCK_MAGIC` is rejected as CorruptMetadata.
//!  * **EM-INIT-VALIDATE-CRC** (superblock.rs deserialize): a superblock whose
//!    stored CRC != the CRC recomputed over its body is rejected as
//!    CorruptMetadata.
//!  * **EM-RECOVER-ROUNDTRIP** (recovery.rs `slab_from_descriptor`): after
//!    recovery the allocation bitmap is derived from the persisted dense key
//!    vector — bit `i` is set **iff** `keys[i] != FREE_KEY`. This is the exact
//!    inverse of the format-time invariant and the reason a checkpoint's key
//!    vector fully reconstructs the allocator state.
//!
//! The CRC computation itself (crc32 over serialized bytes) is a byte-level I/O
//! concern proved by inspection against the standard `crc32fast` output and is
//! recorded as a tool-boundary residual; here we prove the *comparison guard*
//! and the *key -> bitmap derivation*, which are the pure obligations.
//! Re-authored from the inventory.

use creusot_std::prelude::*;
use crate::gate::EmError;

pub const FREE_KEY: u64 = u64::MAX;

/// **EM-INIT-VALIDATE-MAGIC**: accept iff the read magic equals the expected
/// constant. Mirror of the magic guard in `Superblock::deserialize`.
#[ensures(read_magic@ != expected_magic@ ==> result == Err(EmError::CorruptMetadata))]
#[ensures(read_magic@ == expected_magic@ ==> result == Ok(()))]
pub fn validate_magic(read_magic: u64, expected_magic: u64) -> Result<(), EmError> {
    if read_magic != expected_magic {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// **EM-INIT-VALIDATE-CRC**: accept iff the stored CRC equals the CRC recomputed
/// over the body. Mirror of the CRC guard in `Superblock::deserialize`.
#[ensures(stored_crc@ != computed_crc@ ==> result == Err(EmError::CorruptMetadata))]
#[ensures(stored_crc@ == computed_crc@ ==> result == Ok(()))]
pub fn validate_crc(stored_crc: u32, computed_crc: u32) -> Result<(), EmError> {
    if stored_crc != computed_crc {
        return Err(EmError::CorruptMetadata);
    }
    Ok(())
}

/// The recovery-derived allocation predicate for one slot: allocated iff its
/// persisted key is not the free sentinel (recovery.rs `slab_from_descriptor`:
/// `if key != FREE_KEY { mark_slot_allocated(i) }`).
#[logic(open)]
pub fn key_allocated(k: u64) -> bool {
    pearlite! { k@ != FREE_KEY@ }
}

/// **EM-RECOVER-ROUNDTRIP**: derive the bitmap from the dense key vector. The
/// resulting bitmap reflects the keys exactly — `bit[i] <-> keys[i] != FREE_KEY`
/// — reconstructing allocator state from a checkpoint. Mirror of the
/// `slab_from_descriptor` loop.
#[ensures(result@.len() == keys@.len())]
#[ensures(forall<i: Int> 0 <= i && i < keys@.len()
    ==> result@[i] == key_allocated(keys@[i]))]
pub fn bitmap_from_keys(keys: &Vec<u64>) -> Vec<bool> {
    let mut bitmap: Vec<bool> = Vec::new();
    let mut i = 0usize;
    let n = keys.len();
    #[invariant(i@ <= n@)]
    #[invariant(bitmap@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> bitmap@[j] == key_allocated(keys@[j]))]
    while i < n {
        let allocated = keys[i] != FREE_KEY;
        bitmap.push(allocated);
        i += 1;
    }
    bitmap
}

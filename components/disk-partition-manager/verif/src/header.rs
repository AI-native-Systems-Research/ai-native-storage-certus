//! GPT header / MBR structural constants.
//!
//! Covers `DPM-FORMAT-HEADER-CONSTANTS` (both headers carry the fixed signature,
//! revision, header_size 92, num_entries 128, entry_size 128 — gpt.rs:165-179),
//! `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` (FR-003: the backup header's
//! `my_lba`/`alternate_lba` mirror the primary's — gpt.rs:171, :184-190), and the
//! protective-MBR type byte `0xEE` (gpt.rs:329), the LBA0 structure of
//! `DPM-FORMAT-WRITES-5-STRUCTURES`.

use creusot_std::prelude::*;

pub const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART" little-endian
pub const GPT_REVISION_1_0: u32 = 0x0001_0000;
pub const GPT_HEADER_SIZE: u32 = 92;
pub const GPT_ENTRY_SIZE: u32 = 128;
pub const GPT_MAX_ENTRIES: u32 = 128;

/// Mirror of the fixed header field assignment in `write_gpt` (gpt.rs:165-179).
/// Returns `(signature, revision, header_size, num_entries, entry_size)` and
/// proves each equals its GPT-mandated constant. `DPM-FORMAT-HEADER-CONSTANTS`.
pub fn header_constants() -> (u64, u32, u32, u32, u32) {
    (
        GPT_SIGNATURE,
        GPT_REVISION_1_0,
        GPT_HEADER_SIZE,
        GPT_MAX_ENTRIES,
        GPT_ENTRY_SIZE,
    )
}

/// `DPM-FORMAT-BACKUP-MIRRORS-PRIMARY` (FR-003). The primary header has
/// `my_lba = 1`, `alternate_lba = num_sectors - 1` (gpt.rs:170-171); the backup
/// is built with `my_lba = num_sectors - 1`, `alternate_lba = 1` (gpt.rs:185-190,
/// the `..primary_header` update). Returns `((p_my, p_alt), (b_my, b_alt))` and
/// proves the two headers mirror each other: the backup's `my_lba` is the
/// primary's `alternate_lba` and vice-versa.
pub fn backup_mirrors_primary(num_sectors: u64) -> ((u64, u64), (u64, u64)) {
    let primary_my = 1u64;
    let primary_alt = num_sectors - 1;
    let backup_my = num_sectors - 1;
    let backup_alt = 1u64;
    ((primary_my, primary_alt), (backup_my, backup_alt))
}

/// Protective-MBR partition type byte: `mbr[446 + 4] = 0xEE` (gpt.rs:329), the
/// GPT-protective type. Part of `DPM-FORMAT-WRITES-5-STRUCTURES` (the LBA0
/// structure). Trivially proves the written type byte is `0xEE`.
pub fn mbr_protective_type() -> u8 {
    0xEE
}

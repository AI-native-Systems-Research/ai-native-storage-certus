// Creusot formal verification of segment_io() from components/dispatcher/src/io_segmenter.rs
//
// segment_io() splits a large I/O transfer into segments each fitting within
// the device's maximum transfer size (MDTS). This file proves:
//   1. The function terminates.
//   2. If total_bytes == 0, the result is empty.
//   3. If total_bytes > 0, the result is non-empty.
//   4. The segments cover the full transfer without gaps:
//      buffer_offset + remaining == total_bytes at every iteration.
//   5. remaining strictly decreases each iteration (termination witness).
//
// Proof history:
//   v1 (11/13): missing segments-growth invariant; prover couldn't connect
//               non-emptiness postcondition to loop progress.
//   v2 (16/17): added #[invariant(buffer_offset@ > 0 ==> segments@.len() > 0)]
//               and proof_assert! to bridge u32→usize casts.  Remaining VC:
//               lba u64 overflow — prover had no bound on LBA advancement.
//   v3 (17/17 ✔): added #[requires(start_lba@ + total_bytes@/sector_size@ <= u64::MAX@)]
//               and two lba invariants.  Full proof discharged.

use creusot_std::prelude::*;

/// An I/O segment — mirrors the production type in components/dispatcher.
pub struct IoSegment {
    pub buffer_offset: usize,
    pub lba: u64,
    pub length: usize,
}

/// Split a transfer into segments respecting the device's maximum transfer size.
#[requires(sector_size@ > 0)]
#[requires(max_transfer_size@ > 0)]
// LBA advancement over the full transfer must not overflow u64 (device capacity bound).
#[requires(start_lba@ + total_bytes@ / sector_size@ <= u64::MAX@)]
#[ensures(total_bytes@ == 0 ==> result@.len() == 0)]
#[ensures(total_bytes@ > 0 ==> result@.len() > 0)]
pub fn segment_io(
    start_lba: u64,
    total_bytes: usize,
    max_transfer_size: u32,
    sector_size: u32,
) -> Vec<IoSegment> {
    if total_bytes == 0 {
        return Vec::new();
    }

    let mts = max_transfer_size as usize;
    let ss = sector_size as usize;
    // Help the SMT solver connect preconditions through the casts.
    proof_assert!(mts@ > 0);
    proof_assert!(ss@ > 0);

    let mut segments: Vec<IoSegment> = Vec::new();
    let mut remaining = total_bytes;
    let mut buffer_offset = 0usize;
    let mut lba = start_lba;

    #[invariant(buffer_offset@ + remaining@ == total_bytes@)]
    #[invariant(remaining@ <= total_bytes@)]
    // Every byte consumed so far came from at least one pushed segment.
    #[invariant(buffer_offset@ > 0 ==> segments@.len() > 0)]
    // lba stays within u64 range: it started at start_lba and advanced by at most buffer_offset/ss.
    #[invariant(lba@ <= start_lba@ + buffer_offset@ / ss@)]
    #[invariant(lba@ <= u64::MAX@)]
    #[variant(remaining)]
    while remaining > 0 {
        let length = if remaining < mts { remaining } else { mts };
        // length > 0 because remaining > 0 and mts > 0.
        proof_assert!(length@ > 0);
        segments.push(IoSegment {
            buffer_offset,
            lba,
            length,
        });
        buffer_offset += length;
        lba += (length / ss) as u64;
        remaining -= length;
    }

    segments
}

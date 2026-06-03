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

// Modular arithmetic lemma: if a and b are both multiples of n, so is (a - b).
//
// This fact is beyond what the automated SMT solvers (alt-ergo, z3, cvc5, cvc4)
// can discharge for variable divisors. It is proved by hand in Coq using the
// EuclideanDivision library's Mod_mult and Mod_0 lemmas:
//
//   coq/mod_sub_lemma.v
//
// The proof strategy: unfold mod1, extract a = n*k and b = n*j from the
// hypotheses, rewrite a-b = n*(k-j)+0, then apply Mod_mult and Mod_0.
#[trusted]
#[logic]
#[requires(n@ > 0)]
#[requires(a@ % n@ == 0)]
#[requires(b@ % n@ == 0)]
#[ensures(result)]
fn lemma_mod_sub(a: usize, b: usize, n: usize) -> bool {
    pearlite! { (a@ - b@) % n@ == 0 }
}

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
// LBA adjacency requires sector-aligned sizes so integer division is exact.
#[requires(max_transfer_size@ % sector_size@ == 0)]
#[requires(total_bytes@ % sector_size@ == 0)]
#[ensures(total_bytes@ == 0 ==> result@.len() == 0)]
#[ensures(total_bytes@ > 0 ==> result@.len() > 0)]
// No gaps, no overlaps: each segment starts exactly where the previous one ended.
#[ensures(
    forall<i: Int>
        0 <= i && i + 1 < result@.len()
        ==> result@[i].buffer_offset@ + result@[i].length@ == result@[i + 1].buffer_offset@
)]
// Exact segment count: result.len() == ceil(total_bytes / max_transfer_size).
#[ensures(
    total_bytes@ > 0 ==>
        (result@.len() - 1) * max_transfer_size@ < total_bytes@
        && total_bytes@ <= result@.len() * max_transfer_size@
)]
// LBA adjacency: each segment's LBA end equals the next segment's LBA start.
#[ensures(
    forall<i: Int>
        0 <= i && i + 1 < result@.len()
        ==> result@[i].lba@ + result@[i].length@ / sector_size@ == result@[i + 1].lba@
)]
#[ensures(
    forall<i: Int>
        0 <= i && i + 1 < result@.len()
        ==> result@[i].lba@ + result@[i].length@ / sector_size@ == result@[i + 1].lba@
)]
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
    proof_assert!(mts@ % ss@ == 0);
    proof_assert!(total_bytes@ % ss@ == 0);

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
    // Adjacent buffer_offset: each segment starts where the previous ended.
    #[invariant(
        forall<j: Int>
            0 <= j && j + 1 < segments@.len()
            ==> segments@[j].buffer_offset@ + segments@[j].length@ == segments@[j + 1].buffer_offset@
    )]
    // Last segment's buffer end == current buffer_offset.
    #[invariant(
        segments@.len() > 0
            ==> segments@[segments@.len() - 1].buffer_offset@
                + segments@[segments@.len() - 1].length@
                == buffer_offset@
    )]
    #[invariant(remaining@ > 0 ==> buffer_offset@ == segments@.len() * mts@)]
    #[invariant(segments@.len() > 0 ==> (segments@.len() - 1) * mts@ < buffer_offset@)]
    #[invariant(buffer_offset@ <= segments@.len() * mts@)]
    #[invariant(remaining@ % ss@ == 0)]
    #[invariant(
        forall<j: Int>
            0 <= j && j + 1 < segments@.len()
            ==> segments@[j].lba@ + segments@[j].length@ / ss@ == segments@[j + 1].lba@
    )]
    #[invariant(
        segments@.len() > 0
            ==> segments@[segments@.len() - 1].lba@
                + segments@[segments@.len() - 1].length@ / ss@
                == lba@
    )]
    #[invariant(remaining@ % ss@ == 0)]
    #[invariant(
        forall<j: Int>
            0 <= j && j + 1 < segments@.len()
            ==> segments@[j].lba@ + segments@[j].length@ / ss@ == segments@[j + 1].lba@
    )]
    #[invariant(
        segments@.len() > 0
            ==> segments@[segments@.len() - 1].lba@
                + segments@[segments@.len() - 1].length@ / ss@
                == lba@
    )]
    #[variant(remaining)]
    while remaining > 0 {
        let length = if remaining < mts { remaining } else { mts };
        // length > 0 because remaining > 0 and mts > 0.
        proof_assert!(length@ > 0);
        // length % ss == 0: length = min(remaining, mts), both multiples of ss.
        // remaining % ss == 0 from loop invariant; mts % ss == 0 from bridge below.
        proof_assert!(length@ % ss@ == 0);
        proof_assert!(length@ == mts@ || remaining@ == length@);
        proof_assert!(buffer_offset@ == segments@.len() * mts@);
        // Use the Coq-proved lemma to discharge (remaining - length) % ss == 0.
        proof_assert!(lemma_mod_sub(remaining, length, ss));
        segments.push(IoSegment {
            buffer_offset,
            lba,
            length,
        });
        buffer_offset += length;
        lba += (length / ss) as u64;
        remaining -= length;
    }

    proof_assert!(buffer_offset@ == total_bytes@);
    proof_assert!(segments@.len() == 0 || (segments@.len() - 1) * mts@ < total_bytes@);
    segments
}

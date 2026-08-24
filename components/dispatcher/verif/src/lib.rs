//! Creusot verification mirror for `components/dispatcher`.
//!
//! The shipped dispatcher functions cannot compile under Creusot (they touch
//! `Arc<dyn IMemoryTier>`, `Mutex`, atomics, raw pointers, SPDK/CUDA FFI). This
//! crate proves **standalone, byte-faithful mirrors** of the dispatcher's pure
//! arithmetic cores — the parts whose correctness is numeric, not I/O:
//!
//!   * [`segment_io`] — the MDTS-aware I/O splitter (`../src/io_segmenter.rs`).
//!   * [`scan_widen`] — the eviction scan-window widening rule inside
//!     `evict_for_space` (`../src/lib.rs`).
//!   * [`evict_bound`] — the attempt-counted termination of the `evict_for_space`
//!     loop (`../src/lib.rs`).
//!
//! **Drift discipline.** Each mirror body transcribes the cited source lines.
//! The only mechanical changes: the two `assert!` domain guards in `segment_io`
//! become `#[requires]` preconditions (the panic documents the same domain), and
//! `Vec::with_capacity(total_bytes.div_ceil(mts))` becomes `Vec::new()` — the
//! capacity hint is a contractless external call with no bearing on the produced
//! segments. See `PROPERTIES.md` for the source-line correspondence and the
//! fault-injection log. Proofs are validated by perturbing a mirror body and
//! confirming a VC goes red.

use creusot_std::prelude::*;

// ===========================================================================
// segment_io — MDTS-aware I/O segmentation
// Mirror of `../src/io_segmenter.rs::segment_io` (spec FR-019, SC-012).
// ===========================================================================

/// An I/O segment. Mirror of `io_segmenter::IoSegment` (no derives — Creusot
/// dislikes ambiguous `Clone` on spec-visible structs).
pub struct IoSegment {
    /// Byte offset from the start of the buffer.
    pub buffer_offset: usize,
    /// Starting logical block address on the device.
    pub lba: u64,
    /// Number of bytes in this segment.
    pub length: usize,
}

/// Split a transfer into segments respecting the device's maximum transfer size.
///
/// The two `assert!(… > 0)` guards in the shipped function become preconditions:
/// they document exactly the non-panicking domain.
///
/// Proven properties (spec FR-019 "segmented to respect MDTS", SC-012):
///  * **Emptiness:** `total_bytes == 0` produces no segments; `> 0` produces ≥1.
///  * **MDTS bound:** every segment is at most `max_transfer_size` bytes.
///  * **Positivity:** every segment carries a positive length (no empty splits).
///  * **Coverage:** the segments tile `[0, total_bytes)` — the first starts at
///    offset 0 and the last ends exactly at `total_bytes`.
///  * **LBA floor:** every segment's LBA is ≥ `start_lba` (no address underflow).
#[requires(max_transfer_size@ > 0)]
#[requires(sector_size@ > 0)]
// Overflow-freedom for the running LBA: the total advance is bounded by
// total_bytes (since length/ss <= length and the lengths sum to total_bytes).
#[requires(start_lba@ + total_bytes@ <= u64::MAX@)]
#[ensures(total_bytes@ == 0 ==> result@.len() == 0)]
#[ensures(total_bytes@ > 0 ==> result@.len() > 0)]
// MDTS bound + positivity, for every produced segment.
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==>
    0 < result@[i].length@ && result@[i].length@ <= max_transfer_size@)]
// LBA floor.
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i].lba@ >= start_lba@)]
// Coverage endpoints: first segment starts at 0, last ends at total_bytes.
#[ensures(result@.len() > 0 ==> result@[0].buffer_offset@ == 0)]
#[ensures(result@.len() > 0 ==>
    result@[result@.len() - 1].buffer_offset@ + result@[result@.len() - 1].length@ == total_bytes@)]
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
    proof_assert!(mts@ == max_transfer_size@); // widening cast preserves value
    proof_assert!(ss@ == sector_size@);

    let mut segments: Vec<IoSegment> = Vec::new();
    let mut remaining = total_bytes;
    let mut buffer_offset = 0usize;
    let mut lba = start_lba;

    #[invariant(remaining@ <= total_bytes@)]
    #[invariant(buffer_offset@ + remaining@ == total_bytes@)]
    #[invariant(lba@ >= start_lba@)]
    #[invariant(lba@ <= start_lba@ + buffer_offset@)]
    #[invariant(buffer_offset@ == 0 ==> segments@.len() == 0)]
    #[invariant(buffer_offset@ > 0 ==> segments@.len() > 0)]
    #[invariant(forall<i: Int> 0 <= i && i < segments@.len() ==>
        0 < segments@[i].length@ && segments@[i].length@ <= mts@)]
    #[invariant(forall<i: Int> 0 <= i && i < segments@.len() ==> segments@[i].lba@ >= start_lba@)]
    #[invariant(segments@.len() > 0 ==> segments@[0].buffer_offset@ == 0)]
    #[invariant(segments@.len() > 0 ==>
        segments@[segments@.len() - 1].buffer_offset@ + segments@[segments@.len() - 1].length@ == buffer_offset@)]
    #[variant(remaining)]
    while remaining > 0 {
        let length = if remaining < mts { remaining } else { mts };
        segments.push(IoSegment {
            buffer_offset,
            lba,
            length,
        });
        let delta = (length / ss) as u64;
        proof_assert!(delta@ == length@ / ss@); // widening cast preserves value
        proof_assert!(length@ / ss@ <= length@); // ss >= 1 => quotient <= dividend
        buffer_offset += length;
        lba += delta;
        remaining -= length;
    }

    segments
}

// ===========================================================================
// scan_widen — eviction scan-window widening
// Mirror of `let scan = (MAX_SCAN * attempts).min(1024);` in
// `../src/lib.rs::evict_for_space` (spec FR-024: "MAX_SCAN=4 × attempts,
// capped at 1024").
// ===========================================================================

/// `MAX_SCAN`, matching `evict_for_space`/`evict_and_insert`.
pub const MAX_SCAN: usize = 4;

/// Compute the widening `oldest_keys` scan window for eviction attempt `attempts`.
///
/// Proven properties (spec FR-024):
///  * **Bounded:** the scan window never exceeds 1024, no matter the pressure —
///    the loop can never request an unbounded LRU scan.
///  * **Exact below the cap:** while `4·attempts ≤ 1024` the window is exactly
///    `4·attempts` (it widens by `MAX_SCAN` each attempt).
///  * **Floor:** with at least one attempt the window is at least `MAX_SCAN`.
#[requires(attempts@ >= 1)]
#[requires(MAX_SCAN@ * attempts@ <= usize::MAX@)] // caller bounds attempts by max_attempts
#[ensures(result@ <= 1024)]
#[ensures(result@ <= MAX_SCAN@ * attempts@)]
#[ensures(MAX_SCAN@ * attempts@ <= 1024 ==> result@ == MAX_SCAN@ * attempts@)]
#[ensures(result@ >= MAX_SCAN@)]
pub fn scan_widen(attempts: usize) -> usize {
    (MAX_SCAN * attempts).min(1024)
}

// ===========================================================================
// evict_bound — eviction-loop termination bound
// Mirror of the attempt counter in `../src/lib.rs::evict_for_space`
// (spec FR-024 / User Story 7 scenario 5: returns AllocationFailed after
// `max_eviction_attempts` iterations rather than blind-freeing).
// ===========================================================================

/// Models the `evict_for_space` loop on its worst-case path — when no space is
/// ever freed, the `while used + needed > capacity` guard stays true, so
/// termination rests entirely on the attempt counter. Returns the iteration
/// count at which the shipped code surfaces `AllocationFailed`.
///
/// Proven property (spec FR-024): the loop **terminates** and does so after
/// exactly `max_attempts + 1` iterations — it cannot spin unboundedly, and it
/// gives up (rather than blind-free a pinned slot) once the budget is spent.
///
/// Honest boundary: this proves the counter-driven bound. The real guard also
/// exits early when `evict_one_clean` frees enough bytes; that direction depends
/// on memory-tier `used()` monotonicity, which is an environment property
/// outside this mirror (see PROPERTIES.md).
#[requires(max_attempts@ < usize::MAX@)]
#[ensures(result@ == max_attempts@ + 1)]
pub fn evict_bound(max_attempts: usize) -> usize {
    let mut attempts = 0usize;
    #[invariant(attempts@ <= max_attempts@ + 1)]
    #[variant(max_attempts@ + 1 - attempts@)]
    while attempts <= max_attempts {
        attempts += 1;
    }
    attempts
}

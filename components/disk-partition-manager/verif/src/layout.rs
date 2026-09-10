//! Mirror of the GPT layout arithmetic in `gpt.rs`.
//!
//! Sources: `GptManager::entry_sectors` (gpt.rs:314-317),
//! `GptManager::write_gpt` LBA setup + too-small guard (gpt.rs:146-154),
//! `GptManager::compute_partition_layout` (gpt.rs:229-312).
//!
//! This is Creusot's sweet spot: logic-level reasoning over the LBA/sector
//! arithmetic that decides where partitions land. Covers inventory ids
//! `DPM-FORMAT-DEVICE-TOO-SMALL`, `DPM-FORMAT-PART-SIZE-CEIL`,
//! `DPM-FORMAT-RESTOFDISK-REMAINING`, `DPM-FORMAT-ERR-OVERSUBSCRIBED`,
//! `DPM-FORMAT-PART-NONOVERLAP`, `DPM-FORMAT-PART-WITHIN-USABLE`,
//! `DPM-FORMAT-ENTRY-ARRAY-128x128`, and the R4 panic-freedom preconditions.

use creusot_std::prelude::*;

/// The GPT entry-array byte size: 128 entries × 128 bytes = 16384 (gpt.rs:315,
/// `GPT_MAX_ENTRIES * GPT_ENTRY_SIZE`).
pub const ENTRY_ARRAY_BYTES: u64 = 16384;

/// Mirror of `GptManager::entry_sectors` (gpt.rs:314-317):
/// `(GPT_MAX_ENTRIES * GPT_ENTRY_SIZE).div_ceil(sector_size)`.
///
/// `div_ceil(16384, s)` == `(16384 + s - 1) / s` for `s > 0`; the mirror splits
/// it into named steps (semantically identical) so the ceil facts can be named.
/// Proves the sector count is the smallest number of sectors holding the entry
/// array: a multiple's-worth that covers it, minimal to within one sector.
#[requires(sector_size@ > 0)]
#[requires(16384 + sector_size@ <= u64::MAX@)] // add is overflow-free (16384 is tiny)
#[ensures(result@ * sector_size@ >= 16384)] // covers the entry array
#[ensures(result@ * sector_size@ < 16384 + sector_size@)] // minimal: < one sector of slack
#[ensures(result@ >= 1)] // at least one sector (16384 > 0)
pub fn entry_sectors(sector_size: u64) -> u64 {
    let n = ENTRY_ARRAY_BYTES + sector_size - 1;
    let q = n / sector_size;
    proof_assert!(n@ == q@ * sector_size@ + n@ % sector_size@);
    proof_assert!(n@ % sector_size@ < sector_size@);
    q
}

/// Mirror of the usable-LBA window computed at the top of `write_gpt`
/// (gpt.rs:146-148):
/// ```ignore
/// let entry_sectors = self.entry_sectors();
/// let first_usable_lba = 2 + entry_sectors as u64;
/// let last_usable_lba  = self.num_sectors - 1 - entry_sectors as u64 - 1;
/// ```
/// **R4 (latent underflow).** `last_usable_lba` is computed *before* the
/// too-small guard, so the subtractions underflow (panic in debug) unless
/// `num_sectors >= entry_sectors + 2`. This mirror pins that panic-freedom
/// precondition and proves the closed forms of both endpoints.
#[requires(es@ >= 1)]
#[requires(num_sectors@ >= es@ + 2)] // R4 panic-freedom precondition for the subtractions
#[ensures(result.0@ == 2 + es@)] // first_usable_lba
#[ensures(result.1@ == num_sectors@ - es@ - 2)] // last_usable_lba (= num_sectors-1-es-1)
pub fn usable_lbas(num_sectors: u64, es: u64) -> (u64, u64) {
    let first_usable_lba = 2 + es;
    let last_usable_lba = num_sectors - 1 - es - 1;
    (first_usable_lba, last_usable_lba)
}

/// Mirror of the too-small guard (gpt.rs:150-154): `if first_usable_lba >=
/// last_usable_lba { return LayoutError }`. Returns `true` when the device is
/// large enough to proceed (the `Ok`-path condition). Proves the guard is exactly
/// `first < last`, and — combined with `usable_lbas` — that a device passing the
/// guard has at least one usable sector.
#[ensures(result == (first_usable_lba@ < last_usable_lba@))]
pub fn device_large_enough(first_usable_lba: u64, last_usable_lba: u64) -> bool {
    !(first_usable_lba >= last_usable_lba)
}

/// Mirror of the fixed-partition sector count (gpt.rs:257, :273):
/// `size_bytes.div_ceil(self.sector_size as u64)`.
/// `DPM-FORMAT-PART-SIZE-CEIL` (FR-004): the sector count is `ceil(size_bytes /
/// sector_size)` — covers the request, minimal to within one sector.
#[requires(sector_size@ > 0)]
#[requires(size_bytes@ + sector_size@ <= u64::MAX@)] // overflow-free add
#[requires(size_bytes@ > 0)] // this branch is taken only for fixed (non-rest) partitions
#[ensures(result@ * sector_size@ >= size_bytes@)] // covers the requested bytes
#[ensures(result@ * sector_size@ < size_bytes@ + sector_size@)] // minimal (ceil, not more)
#[ensures(result@ >= 1)] // a positive request needs at least one sector
pub fn part_size_ceil(size_bytes: u64, sector_size: u64) -> u64 {
    let n = size_bytes + sector_size - 1;
    let q = n / sector_size;
    proof_assert!(n@ == q@ * sector_size@ + n@ % sector_size@);
    proof_assert!(n@ % sector_size@ < sector_size@);
    q
}

/// Mirror of the "rest of disk" sizing (gpt.rs:267, :270-271):
/// `rest_sectors = total_usable - fixed_sectors`, consumed by the single
/// `size_bytes == 0` partition. `DPM-FORMAT-RESTOFDISK-REMAINING` (FR-004).
/// The oversubscription guard (gpt.rs:260-265) ensures `fixed_sectors <=
/// total_usable`, so this subtraction does not underflow.
#[requires(fixed_sectors@ <= total_usable@)] // established by the oversubscribed guard
#[ensures(result@ == total_usable@ - fixed_sectors@)] // rest consumes exactly what's left
#[ensures(result@ + fixed_sectors@ == total_usable@)] // fixed + rest = all usable sectors
pub fn restofdisk_remaining(total_usable: u64, fixed_sectors: u64) -> u64 {
    total_usable - fixed_sectors
}

/// Mirror of the aggregate oversubscription guard (gpt.rs:260-265):
/// `if fixed_sectors > total_usable { return LayoutError }`.
/// `DPM-FORMAT-ERR-OVERSUBSCRIBED` (FR-006), first of the two guards. Returns
/// `true` when the fixed partitions fit (the `Ok`-path condition).
#[ensures(result == (fixed_sectors@ <= total_usable@))]
pub fn fixed_fits(fixed_sectors: u64, total_usable: u64) -> bool {
    !(fixed_sectors > total_usable)
}

/// Mirror of the per-placement remaining-fits guard (gpt.rs:276-281):
/// `if num_sectors > remaining { return LayoutError }`.
/// `DPM-FORMAT-ERR-OVERSUBSCRIBED` (FR-006), second guard. Returns `true` when
/// this partition fits in the sectors still free.
#[ensures(result == (num_sectors@ <= remaining@))]
pub fn placement_fits(num_sectors: u64, remaining: u64) -> bool {
    !(num_sectors > remaining)
}

/// Mirror of one iteration of the placement loop (gpt.rs:283, :295-296):
/// ```ignore
/// let ending_lba = current_lba + num_sectors - 1;   // :283
/// ...
/// current_lba = ending_lba + 1;                      // :295
/// remaining -= num_sectors;                          // :296
/// ```
/// `DPM-FORMAT-PART-NONOVERLAP` (US1-AS1) inductive step: returns
/// `(ending_lba, next_start)`. Proves the placed partition `[current_lba,
/// ending_lba]` is non-empty and disjoint-from-below, its successor starts at
/// `ending_lba + 1` (contiguous, no gap, no overlap), and the start strictly
/// increases.
#[requires(num_sectors@ >= 1)] // every placed partition covers at least one sector
#[requires(current_lba@ + num_sectors@ <= u64::MAX@)] // overflow-free
#[ensures(result.0@ == current_lba@ + num_sectors@ - 1)] // ending_lba closed form
#[ensures(result.0@ >= current_lba@)] // non-empty range (disjoint from below)
#[ensures(result.1@ == result.0@ + 1)] // successor is contiguous: next_start == ending+1
#[ensures(result.1@ > current_lba@)] // starts strictly increase ⇒ partitions are disjoint
pub fn place_step(current_lba: u64, num_sectors: u64) -> (u64, u64) {
    let ending_lba = current_lba + num_sectors - 1;
    let next_start = ending_lba + 1;
    (ending_lba, next_start)
}

/// `DPM-FORMAT-PART-WITHIN-USABLE` (US1-AS1 / FR-006) step: given the current
/// cursor lies at/after `first_usable` and this partition fits in the sectors
/// remaining up to `last_usable` (the `placement_fits` guard, where
/// `remaining == last_usable - current + 1`), the placed partition lies wholly
/// within `[first_usable, last_usable]` and the next cursor stays `<=
/// last_usable + 1`.
#[requires(num_sectors@ >= 1)]
#[requires(first_usable@ <= current_lba@)]
#[requires(current_lba@ <= last_usable@)]
#[requires(num_sectors@ <= last_usable@ - current_lba@ + 1)] // placement_fits guard
#[requires(last_usable@ < u64::MAX@)] // usable window strictly below the last LBA (backup header sits there)
#[ensures(result@ >= first_usable@)] // partition starts within the usable window
#[ensures(result@ <= last_usable@)] // and ends within it
pub fn within_usable_step(
    first_usable: u64,
    last_usable: u64,
    current_lba: u64,
    num_sectors: u64,
) -> u64 {
    // ending_lba = current + n - 1.  n <= last_usable - current + 1
    //   ⇒ current + n - 1 <= last_usable  (so no overflow: <= last_usable < u64::MAX).
    proof_assert!(current_lba@ + num_sectors@ - 1 <= last_usable@);
    current_lba + num_sectors - 1
}

/// Concrete two-partition composition of `place_step`
/// (`DPM-FORMAT-PART-NONOVERLAP`): placing p1 then p2 back-to-back yields
/// `p2.start == p1.end + 1` and `p1.end < p2.start` (contiguous & strictly
/// disjoint), the observable non-overlap claim for a placed sequence.
#[requires(n1@ >= 1 && n2@ >= 1)]
#[requires(first_usable@ + n1@ + n2@ <= u64::MAX@)]
#[ensures(result.1@ == result.0@ + 1)] // p2.start == p1.end + 1 (contiguous)
#[ensures(result.0@ < result.1@)] // p1.end < p2.start (disjoint, strictly increasing)
pub fn two_partitions_disjoint(first_usable: u64, n1: u64, n2: u64) -> (u64, u64) {
    let (end1, start2) = place_step(first_usable, n1);
    let (_end2, _start3) = place_step(start2, n2);
    (end1, start2)
}

/// Mirror of the entry-array padding + sizing (gpt.rs:299-311, :422-424):
/// after placing `placed` real entries, the loop pads with all-zero entries
/// until `entries.len() == GPT_MAX_ENTRIES (128)`, and `serialize_entries`
/// emits `GPT_MAX_ENTRIES * GPT_ENTRY_SIZE` bytes.
/// `DPM-FORMAT-ENTRY-ARRAY-128x128` (FR-001): the array is exactly 128 slots ×
/// 128 bytes. Returns `(total_slots, pad_count, total_bytes)`.
#[requires(placed@ <= 128)] // at most 128 partitions can be placed
#[ensures(result.0@ == 128)] // exactly 128 slots after padding
#[ensures(result.1@ == 128 - placed@)] // the pad count fills the remainder (unused = zero slots)
#[ensures(result.2@ == 128 * 128)] // serialized byte size is 128 × 128 = 16384
pub fn entry_array_128(placed: u64) -> (u64, u64, u64) {
    let total_slots = 128u64;
    let pad_count = 128u64 - placed;
    let total_bytes = 128u64 * 128u64;
    (total_slots, pad_count, total_bytes)
}

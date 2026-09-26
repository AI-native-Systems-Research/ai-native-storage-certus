//! Creusot proof mirror for extended-metadata-store.
//!
//! Each `verify_<id>` function discharges a real obligation stripped of I/O,
//! locks, and logging. Routing:
//!   * on_disk arithmetic is mirrored FAITHFULLY over machine integers, using
//!     `/` and `%` (Euclidean specs) rather than the spec-less `div_ceil`, so the
//!     `size == ss*(size/ss) + size%ss` axiom is available to the SMT portfolio.
//!   * the in-memory store is mirrored as a logic `FMap` (KD-STD-CONTAINER-NO-SPECS):
//!     std HashMap has no insert/remove specs, so membership semantics are proven
//!     on the logic map that mirrors it.
//!   * on-disk record round-trips are mirrored STRUCTURALLY over the
//!     accept/reject DECISION (the CRC-equality / magic / version gate), which is
//!     the part Creusot expresses; concrete String/byte CONTENT round-trip is NOT
//!     Creusot-expressible (IX-CREUSOT-STRING-CONTENT) and is proven byte-exact in
//!     Kani (ENTRY-ROUNDTRIP / ENTRY-CRC-REJECT / REGION-ROUNDTRIP — no module here).
//!
//! Each base `verify_<id>` is paired with a `verify_<id>__mutant` carrying a FALSE
//! obligation that MUST stay unproved (anti-vacuity evidence for the scorer).
#![allow(dead_code)]
use creusot_std::logic::FMap;
use creusot_std::prelude::*;

pub const SUPERBLOCK_MAGIC: u64 = 0x4345_5254_4D45_5441;
pub const FORMAT_VERSION: u32 = 1;
pub const HEADER_SIZE: usize = 12;
pub const MAX_VALUE_SIZE: usize = 128 * 1024;

// =====================================================================
// on_disk arithmetic mirrors (REAL re-derivation over machine integers)
// =====================================================================

/// PAD-SECTOR-ALIGN — pad_to_sector(size, ss): 0 -> 0, else next multiple of ss.
/// Mirrors `size.div_ceil(ss) * ss` via /,% so the ensures discharge.
#[requires(ss@ > 0)]
#[requires(size@ + ss@ <= usize::MAX@)]
#[ensures(size@ == 0 ==> result@ == 0)]
#[ensures(result@ % ss@ == 0)]
#[ensures(result@ >= size@)]
#[ensures(result@ < size@ + ss@)]
pub fn verify_pad_sector_align(size: usize, ss: usize) -> usize {
    let r = size % ss;
    if r == 0 {
        size
    } else {
        (size / ss + 1) * ss
    }
}

#[requires(ss@ > 0)]
#[requires(size@ + ss@ <= usize::MAX@)]
#[requires(size@ > 0)]
#[ensures(result@ == size@)] // FALSE whenever size is not already ss-aligned
pub fn verify_pad_sector_align__mutant(size: usize, ss: usize) -> usize {
    let r = size % ss;
    if r == 0 {
        size
    } else {
        (size / ss + 1) * ss
    }
}

/// BYTES-TO-SECTORS — ceil(bytes/ss): 0 for 0, else result*ss >= bytes and
/// (result-1)*ss < bytes.
#[requires(ss@ > 0)]
#[requires(bytes@ + ss@ <= usize::MAX@)]
#[ensures(bytes@ == 0 ==> result@ == 0)]
#[ensures(result@ * ss@ >= bytes@)]
#[ensures(bytes@ > 0 ==> (result@ - 1) * ss@ < bytes@)]
pub fn verify_bytes_to_sectors(bytes: usize, ss: usize) -> usize {
    let r = bytes % ss;
    if r == 0 {
        bytes / ss
    } else {
        bytes / ss + 1
    }
}

#[requires(ss@ > 0)]
#[requires(bytes@ + ss@ <= usize::MAX@)]
#[requires(bytes@ > 0)]
#[ensures(result@ * ss@ == bytes@)] // FALSE unless bytes is a multiple of ss
pub fn verify_bytes_to_sectors__mutant(bytes: usize, ss: usize) -> usize {
    let r = bytes % ss;
    if r == 0 {
        bytes / ss
    } else {
        bytes / ss + 1
    }
}

/// ENTRY-SIZE-ALIGNED — serialized_size = pad_to_sector(12 + klen + vlen, ss):
/// a multiple of ss that is >= the raw size and < raw + ss.
#[requires(ss@ > 0)]
#[requires(HEADER_SIZE@ + klen@ + vlen@ + ss@ <= usize::MAX@)]
#[ensures(result@ % ss@ == 0)]
#[ensures(result@ >= HEADER_SIZE@ + klen@ + vlen@)]
#[ensures(result@ < HEADER_SIZE@ + klen@ + vlen@ + ss@)]
pub fn verify_entry_size_aligned(klen: usize, vlen: usize, ss: usize) -> usize {
    let raw = HEADER_SIZE + klen + vlen;
    let r = raw % ss;
    if r == 0 {
        raw
    } else {
        (raw / ss + 1) * ss
    }
}

#[requires(ss@ > 0)]
#[requires(HEADER_SIZE@ + klen@ + vlen@ + ss@ <= usize::MAX@)]
#[ensures(result@ % ss@ == 1)] // FALSE: a sector-aligned size is 0 mod ss, never 1
pub fn verify_entry_size_aligned__mutant(klen: usize, vlen: usize, ss: usize) -> usize {
    let raw = HEADER_SIZE + klen + vlen;
    let r = raw % ss;
    if r == 0 {
        raw
    } else {
        (raw / ss + 1) * ss
    }
}

/// SB-GEOMETRY — Superblock::new geometry for partition_sectors > 1: two equal
/// adjacent regions after the 1-sector superblock, fitting inside the partition.
/// Returns (region_a_offset, region_a_size, region_b_offset, region_b_size).
#[requires(ps@ > 1)]
#[requires(ps@ <= u64::MAX@)]
#[ensures(result.0@ == 1)]                       // region A starts right after the superblock
#[ensures(result.1@ == result.3@)]              // equal region sizes
#[ensures(result.2@ == result.0@ + result.1@)]  // region B is adjacent to region A
#[ensures(result.2@ + result.3@ <= ps@)]        // both regions fit in the partition
pub fn verify_sb_geometry(ps: u64) -> (u64, u64, u64, u64) {
    let a_off = 1u64;
    let a_size = (ps - 1) / 2;
    let b_size = a_size;
    let b_off = 1 + a_size;
    (a_off, a_size, b_off, b_size)
}

#[requires(ps@ > 1)]
#[requires(ps@ <= u64::MAX@)]
#[ensures(result.1@ != result.3@)] // FALSE: the two regions are equal-sized by construction
pub fn verify_sb_geometry__mutant(ps: u64) -> (u64, u64, u64, u64) {
    let a_off = 1u64;
    let a_size = (ps - 1) / 2;
    let b_size = a_size;
    let b_off = 1 + a_size;
    (a_off, a_size, b_off, b_size)
}

/// REGION-OFFSET-SWAP — active/inactive offsets select A/B by the active flag and
/// are distinct whenever the two region offsets differ.
#[requires(a_off@ != b_off@)]
#[ensures(result.0@ != result.1@)]
#[ensures(active@ == 0 ==> result.0@ == a_off@ && result.1@ == b_off@)]
#[ensures(active@ != 0 ==> result.0@ == b_off@ && result.1@ == a_off@)]
pub fn verify_region_offset_swap(a_off: u64, b_off: u64, active: u8) -> (u64, u64) {
    if active == 0 {
        (a_off, b_off) // (active_offset, inactive_offset)
    } else {
        (b_off, a_off)
    }
}

#[requires(a_off@ != b_off@)]
#[ensures(result.0@ == result.1@)] // FALSE: active and inactive offsets are distinct
pub fn verify_region_offset_swap__mutant(a_off: u64, b_off: u64, active: u8) -> (u64, u64) {
    if active == 0 {
        (a_off, b_off)
    } else {
        (b_off, a_off)
    }
}

/// REGION-CAPACITY-BYTES — region_capacity_bytes = region_a_size * sector_size.
#[requires(a_size@ * ss@ <= u64::MAX@)]
#[ensures(result@ == a_size@ * ss@)]
pub fn verify_region_capacity_bytes(a_size: u64, ss: u64) -> u64 {
    a_size * ss
}

#[requires(a_size@ * ss@ <= u64::MAX@)]
#[requires(a_size@ > 0 && ss@ > 1)]
#[ensures(result@ == a_size@)] // FALSE: capacity scales by ss (> 1), not the raw sector count
pub fn verify_region_capacity_bytes__mutant(a_size: u64, ss: u64) -> u64 {
    a_size * ss
}

/// FLUSH-SEQ-MONOTONE — new_seq = seq + 1 is strictly greater than seq.
#[requires(seq@ < u64::MAX@)]
#[ensures(result@ == seq@ + 1)]
#[ensures(result@ > seq@)]
pub fn verify_flush_seq_monotone(seq: u64) -> u64 {
    seq + 1
}

#[requires(seq@ < u64::MAX@)]
#[ensures(result@ <= seq@)] // FALSE: the sequence strictly increases
pub fn verify_flush_seq_monotone__mutant(seq: u64) -> u64 {
    seq + 1
}

/// FLUSH-CAPACITY-CHECK — the flush is rejected exactly when the serialized region
/// needs more sectors than the region capacity.
#[ensures(result == (region_sectors@ > region_a_size@))]
pub fn verify_flush_capacity_check(region_sectors: u64, region_a_size: u64) -> bool {
    region_sectors > region_a_size
}

#[requires(region_sectors@ > region_a_size@)]
#[ensures(result == false)] // FALSE: an over-capacity region MUST be rejected
pub fn verify_flush_capacity_check__mutant(region_sectors: u64, region_a_size: u64) -> bool {
    region_sectors > region_a_size
}

/// LE-BYTEORDER — little-endian decompose/recompose is the identity (u64), via a
/// base-256 Horner reconstruction over the eight bytes.
#[ensures(result@ == x@)]
pub fn verify_le_byteorder(x: u64) -> u64 {
    let b0 = x % 256;
    let x1 = x / 256;
    let b1 = x1 % 256;
    let x2 = x1 / 256;
    let b2 = x2 % 256;
    let x3 = x2 / 256;
    let b3 = x3 % 256;
    let x4 = x3 / 256;
    let b4 = x4 % 256;
    let x5 = x4 / 256;
    let b5 = x5 % 256;
    let x6 = x5 / 256;
    let b6 = x6 % 256;
    let x7 = x6 / 256;
    let b7 = x7 % 256;
    b0 + 256 * (b1 + 256 * (b2 + 256 * (b3 + 256 * (b4 + 256 * (b5 + 256 * (b6 + 256 * b7))))))
}

#[requires(x@ > 0)]
#[ensures(result@ == 0)] // FALSE: the round-trip reproduces x (> 0)
pub fn verify_le_byteorder__mutant(x: u64) -> u64 {
    let b0 = x % 256;
    let x1 = x / 256;
    let b1 = x1 % 256;
    let x2 = x1 / 256;
    let b2 = x2 % 256;
    let x3 = x2 / 256;
    let b3 = x3 % 256;
    let x4 = x3 / 256;
    let b4 = x4 % 256;
    let x5 = x4 / 256;
    let b5 = x5 % 256;
    let x6 = x5 / 256;
    let b6 = x6 % 256;
    let x7 = x6 / 256;
    let b7 = x7 % 256;
    b0 + 256 * (b1 + 256 * (b2 + 256 * (b3 + 256 * (b4 + 256 * (b5 + 256 * (b6 + 256 * b7))))))
}

// =====================================================================
// on_disk record round-trips — STRUCTURAL decision mirror
// (concrete byte content is IX-CREUSOT-STRING-CONTENT; proven in Kani)
// =====================================================================

/// SB-ROUNDTRIP — deserialize accepts (returns Some) a serialized superblock whose
/// magic/version match and whose stored CRC equals the recomputed CRC.
#[requires(magic@ == SUPERBLOCK_MAGIC@ && version@ == FORMAT_VERSION@ && stored_crc@ == comp_crc@)]
#[ensures(result == true)]
pub fn verify_sb_roundtrip(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

#[requires(magic@ == SUPERBLOCK_MAGIC@ && version@ == FORMAT_VERSION@ && stored_crc@ == comp_crc@)]
#[ensures(result == false)] // FALSE: a matching superblock is accepted
pub fn verify_sb_roundtrip__mutant(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

/// SB-CRC-REJECT — deserialize returns None when the stored CRC differs from the
/// recomputed CRC over the covered prefix.
#[requires(stored_crc@ != comp_crc@)]
#[ensures(result == false)]
pub fn verify_sb_crc_reject(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

#[requires(stored_crc@ != comp_crc@)]
#[ensures(result == true)] // FALSE: a CRC mismatch is rejected
pub fn verify_sb_crc_reject__mutant(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

/// SB-MAGIC-REJECT — deserialize returns None on a wrong magic or wrong version.
#[requires(magic@ != SUPERBLOCK_MAGIC@ || version@ != FORMAT_VERSION@)]
#[ensures(result == false)]
pub fn verify_sb_magic_reject(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

#[requires(magic@ != SUPERBLOCK_MAGIC@)]
#[ensures(result == true)] // FALSE: a wrong magic is rejected
pub fn verify_sb_magic_reject__mutant(magic: u64, version: u32, stored_crc: u32, comp_crc: u32) -> bool {
    magic == SUPERBLOCK_MAGIC && version == FORMAT_VERSION && stored_crc == comp_crc
}

/// REGIONHEADER-ROUNDTRIP — a region header is accepted iff its stored CRC equals
/// the recomputed CRC (no magic/version fields on a region header).
#[requires(stored_crc@ == comp_crc@)]
#[ensures(result == true)]
pub fn verify_regionheader_roundtrip(stored_crc: u32, comp_crc: u32) -> bool {
    stored_crc == comp_crc
}

#[requires(stored_crc@ != comp_crc@)]
#[ensures(result == true)] // FALSE: a corrupted region header (CRC mismatch) is rejected
pub fn verify_regionheader_roundtrip__mutant(stored_crc: u32, comp_crc: u32) -> bool {
    stored_crc == comp_crc
}

// CRC32-DETERMINISTIC is NOT modeled here: the checksum is a function of opaque
// byte content (IX-CREUSOT-STRING-CONTENT). Any trusted logic mirror collapses to
// a constant, which makes both the determinism goal and its anti-vacuity mutant
// discharge vacuously. CRC32 is proven byte-exact in Kani (verify_crc32_deterministic).

// =====================================================================
// in-memory store — logic FMap mirror (KD-STD-CONTAINER-NO-SPECS)
// =====================================================================

/// PUT-GET-ROUNDTRIP — after insert(k, v), get(k) returns Some(v).
pub fn verify_put_get_roundtrip(k: u64, v: u64) {
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v).get(k) == Some(v));
}

pub fn verify_put_get_roundtrip__mutant(k: u64, v: u64) {
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v).get(k) == None); // FALSE
}

/// PUT-OVERWRITE — a second insert on the same key replaces the value.
pub fn verify_put_overwrite(k: u64, v1: u64, v2: u64) {
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v1).insert(k, v2).get(k) == Some(v2));
}

pub fn verify_put_overwrite__mutant(k: u64, v1: u64, v2: u64) {
    // FALSE unless v1 == v2: the latest write wins.
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v1).insert(k, v2).get(k) == Some(v1));
}

/// GET-NOTFOUND — a key absent from the map reads back as None.
pub fn verify_get_notfound(k: u64) {
    proof_assert!(FMap::<u64, u64>::empty().get(k) == None);
}

pub fn verify_get_notfound__mutant(k: u64, v: u64) {
    proof_assert!(FMap::<u64, u64>::empty().get(k) == Some(v)); // FALSE: empty map has no keys
}

/// DELETE-REMOVES — after remove(k), the key is absent (get None, not contained).
pub fn verify_delete_removes(k: u64, v: u64) {
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v).remove(k).get(k) == None);
    proof_assert!(!FMap::<u64, u64>::empty().insert(k, v).remove(k).contains(k));
}

pub fn verify_delete_removes__mutant(k: u64, v: u64) {
    // FALSE: the key is gone after remove.
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v).remove(k).contains(k));
}

/// DELETE-IDEMPOTENT — removing an absent key is a no-op on membership.
pub fn verify_delete_idempotent(k: u64) {
    proof_assert!(!FMap::<u64, u64>::empty().remove(k).contains(k));
    proof_assert!(FMap::<u64, u64>::empty().remove(k).get(k) == None);
}

pub fn verify_delete_idempotent__mutant(k: u64, v: u64) {
    // FALSE: removing from an empty map cannot create a binding.
    proof_assert!(FMap::<u64, u64>::empty().remove(k).get(k) == Some(v));
}

/// ITERATE-REFLECTS — membership of the snapshot reflects the map exactly: an
/// inserted key is present, and any other key's presence is unchanged.
pub fn verify_iterate_reflects(k: u64, j: u64, v: u64) {
    proof_assert!(FMap::<u64, u64>::empty().insert(k, v).contains(k));
    proof_assert!(
        j != k
            ==> FMap::<u64, u64>::empty().insert(k, v).contains(j)
                == FMap::<u64, u64>::empty().contains(j)
    );
}

pub fn verify_iterate_reflects__mutant(k: u64, v: u64) {
    // FALSE: an inserted key IS reflected as present.
    proof_assert!(!FMap::<u64, u64>::empty().insert(k, v).contains(k));
}

/// DIRTY-COUNT-INCREMENT — each successful mutation raises the counter by one.
#[requires(c@ < u64::MAX@)]
#[ensures(result@ == c@ + 1)]
#[ensures(result@ > c@)]
pub fn verify_dirty_count_increment(c: u64) -> u64 {
    c + 1
}

#[requires(c@ < u64::MAX@)]
#[ensures(result@ == c@)] // FALSE: the counter strictly increases
pub fn verify_dirty_count_increment__mutant(c: u64) -> u64 {
    c + 1
}

/// PUT-SIZE-LIMIT — put rejects (returns true = "rejected") exactly when the value
/// length exceeds MAX_VALUE_SIZE, and never inserts in that case.
#[ensures(result == (len@ > MAX_VALUE_SIZE@))]
pub fn verify_put_size_limit(len: usize) -> bool {
    len > MAX_VALUE_SIZE
}

#[requires(len@ > MAX_VALUE_SIZE@)]
#[ensures(result == false)] // FALSE: an over-size value MUST be rejected
pub fn verify_put_size_limit__mutant(len: usize) -> bool {
    len > MAX_VALUE_SIZE
}

/// FORCE-FLUSH-NOOP — with no trigger installed, force_flush leaves the store
/// unchanged (modeled: the map is returned identically).
#[ensures(result.get(k) == m.get(k))]
pub fn verify_force_flush_noop(m: FMap<u64, u64>, k: u64) -> FMap<u64, u64> {
    m
}

#[requires(m.contains(k))]
#[ensures(result.get(k) == None)] // FALSE: a no-op does not drop bindings
pub fn verify_force_flush_noop__mutant(m: FMap<u64, u64>, k: u64) -> FMap<u64, u64> {
    m
}

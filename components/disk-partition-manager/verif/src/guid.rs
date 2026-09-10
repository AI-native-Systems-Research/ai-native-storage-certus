//! `DPM-FORMAT-DISK-GUID-V4` / `DPM-FORMAT-PART-GUID-V4` (FR-008): generated
//! GUIDs carry the RFC-4122 version-4 nibble and variant bits.
//!
//! Source: `generate_guid` (gpt.rs:554-564):
//! ```ignore
//! guid[6] = (guid[6] & 0x0F) | 0x40; // version 4
//! guid[8] = (guid[8] & 0x3F) | 0x80; // variant 1
//! ```
//! These are bit-level facts, proved with `#[bitwise_proof]` (the SMT portfolio
//! reasons over `&`/`|` on the bytes as bit vectors, via the `nth_bit` builtin).
//! The `/dev/urandom` entropy and GUID *uniqueness* (R6) are environmental and
//! NOT proved here — only the structural version/variant nibbles, exactly as the
//! inventory scopes it.

use creusot_std::prelude::*;
use creusot_std::logic::ops::NthBitLogic;

/// Version nibble: `(b & 0x0F) | 0x40`. The version nibble (bits 4..7) becomes
/// `0100` = 4, and the low nibble (bits 0..3, clock-seq high) is preserved.
#[bitwise_proof]
#[ensures(!result.nth_bit(7))] // bits 4..7 == 0100 == version 4
#[ensures(result.nth_bit(6))]
#[ensures(!result.nth_bit(5))]
#[ensures(!result.nth_bit(4))]
#[ensures(result.nth_bit(0) == b.nth_bit(0))] // low nibble preserved (representative bits)
#[ensures(result.nth_bit(3) == b.nth_bit(3))]
pub fn guid_version4(b: u8) -> u8 {
    (b & 0x0F) | 0x40
}

/// Variant bits: `(b & 0x3F) | 0x80`. The top two bits (7,6) become `10`
/// (RFC-4122 variant 1), and the low six bits are preserved.
#[bitwise_proof]
#[ensures(result.nth_bit(7))] // bits 7,6 == 10 == variant 1
#[ensures(!result.nth_bit(6))]
#[ensures(result.nth_bit(0) == b.nth_bit(0))] // low six bits preserved (representative bits)
#[ensures(result.nth_bit(5) == b.nth_bit(5))]
pub fn guid_variant(b: u8) -> u8 {
    (b & 0x3F) | 0x80
}

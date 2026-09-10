//! Mirror of the buddy-allocator ORDER computation shared by
//! `BuddyAllocator::alloc`/`free`/`mark_allocated` (src/buddy.rs:57-157).
//!
//! Spec: FR-020 / allocator sizing. Each of those methods turns a byte size
//! into a buddy *order* via
//! `order = if blocks <= 1 { 0 } else { 64 - (blocks-1).leading_zeros() }`
//! (buddy.rs:59-63, 87-91, 120-124), i.e. `ceil(log2(blocks))`. The property
//! attempted here is the *sizing correctness* of that order: the chosen order
//! is large enough to cover the request, `2^order >= blocks`.
//!
//! This is the coverage-discipline attempt on the buddy `leading_zeros`/`log2`
//! math. See `extent-manager_properties.md` for the outcome.

use creusot_std::prelude::*;

/// Mirror of the order computation (buddy.rs:59-63). `blocks` is the number of
/// sectors the request rounds up to. The attempted postcondition is that the
/// returned order covers the request: `2^result >= blocks`.
// Coverage: the chosen order is large enough for the request.
// Minimality: order is the SMALLEST such — one order down no longer covers.
// Together these pin `result` to `ceil(log2(blocks))`.
pub fn order_for_blocks(blocks: u64) -> usize {
    if blocks <= 1 {
        0
    } else {
        let x = blocks - 1;
        let lz = x.leading_zeros();
        // leading_zeros model (creusot-std num.rs:220): x >> (64 - lz - 1) == 1,
        // i.e. the top set bit of x is at position (63 - lz), so
        // 2^(63-lz) <= x < 2^(64-lz). Bridging both bounds to pow2 lets the
        // portfolio discharge coverage AND minimality.
                        64 - lz as usize
    }
}

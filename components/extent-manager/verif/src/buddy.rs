//! Buddy-allocator ORDER computation (src/buddy.rs:57-63, 87-91, 120-124),
//! shared by `alloc`/`free`/`mark_allocated`:
//! `order = if blocks <= 1 { 0 } else { 64 - (blocks-1).leading_zeros() }`.
//!
//! Backs the slab-sizing half of **EM-RESERVE-SIZECLASS** / reserve allocation:
//! the chosen order is large enough (`2^order >= blocks`) and minimal
//! (`2^(order-1) < blocks`), i.e. exactly `ceil(log2(blocks))`. This is the
//! `leading_zeros -> log2` bridge, proved with `#[bitwise_proof]` + `pow2`.
//! Source: FR-019 / FR-020. Re-authored from the inventory in this worktree.

use creusot_std::prelude::*;

#[bitwise_proof]
#[requires(blocks@ >= 1)]
#[ensures(result@.pow2() >= blocks@)]
#[ensures(result@ == 0 || (result@ - 1).pow2() < blocks@)]
pub fn order_for_blocks(blocks: u64) -> usize {
    if blocks <= 1 {
        0
    } else {
        let x = blocks - 1;
        let lz = x.leading_zeros();
        // leading_zeros model: top set bit of x is at position (63 - lz), so
        // 2^(63-lz) <= x < 2^(64-lz). Bridge both bounds to pow2.
        proof_assert!(x@ < (64u32 - lz)@.pow2());
        proof_assert!((63u32 - lz)@.pow2() <= x@);
        64 - lz as usize
    }
}

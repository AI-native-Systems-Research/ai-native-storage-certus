//! Mirror of `BuddyAllocator` — `components/extent-manager/src/buddy.rs:1-167`.
//!
//! Faithful whole-function mirrors. Disclosed differences (semantics-preserving):
//!  * `for x in a..b` / `(a..b).rev()` / `a..=b` loops are written as `while` loops
//!    over the same index sequence;
//!  * three repeated expressions are factored into proved helpers with the SAME
//!    text: [`order_for_blocks`] (`if b <= 1 {0} else {64 - (b-1).leading_zeros()}`,
//!    buddy.rs:59-63/87-91/120-124), [`top_bit`] (`63 - x.leading_zeros()`,
//!    buddy.rs:13/31), [`pow2_u64`] (`1u64 << k`, buddy.rs:32/77/95/135/146),
//!    [`mask_down`] (`x & !(s - 1)`, buddy.rs:136);
//!  * `list.iter().position(|&o| o == x)` → [`find_pos`] (index loop),
//!    `Vec::swap_remove` → [`swap_remove`] (swap with last + pop),
//!    `!v.is_empty()` → `v.len() != 0`, `a.min(b)` → `if a <= b {a} else {b}`;
//!  * `current_offset ^ block_span` (buddy.rs:96) → [`buddy_xor`] — see its doc.
//!
//! Logic view: `span(ss, o) = 2^o * ss` bytes is the size of an order-`o` block.
use crate::model::l2::*;
use crate::model::l2m::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

pub struct BuddyAllocator {
    pub base_offset: u64,
    pub total_usable_size: u64,
    pub sector_size: u32,
    pub max_order: usize,
    pub free_lists: Vec<Vec<u64>>,
}

/// Size in bytes of an order-`o` block.
#[logic(open)]
pub fn span(ss: Int, o: Int) -> Int {
    pearlite! { o.pow2() * ss }
}

/// `r` is the buddy order for `b` blocks: the least `r` with `2^r >= b`.
#[logic(open)]
pub fn is_order(r: Int, b: Int) -> bool {
    pearlite! { 0 <= r && r <= 64 && r.pow2() >= b && (r == 0 || (r - 1).pow2() < b) }
}

// ----------------------------------------------------------------- arithmetic

#[logic]
#[variant(o)]
#[requires(0 <= o)]
#[ensures(o.pow2() >= 1)]
#[ensures((o + 1).pow2() == 2 * o.pow2())]
pub fn lemma_pow2(o: Int) {
    if o > 0 {
        lemma_pow2(o - 1)
    }
}

#[logic]
#[requires(0 <= a && a <= b)]
#[variant(b - a)]
#[ensures(a.pow2() <= b.pow2())]
pub fn lemma_pow2_mono(a: Int, b: Int) {
    lemma_pow2(a);
    if a < b {
        lemma_pow2(b - 1);
        lemma_pow2_mono(a, b - 1)
    }
}

/// `pow2` is strictly monotone, so a strict `pow2` bound orders the exponents.
#[logic]
#[requires(0 <= a && 0 <= b)]
#[ensures(a.pow2() < b.pow2() ==> a < b)]
#[ensures(a.pow2() <= b.pow2() ==> a <= b || a.pow2() == b.pow2())]
pub fn lemma_pow2_strict(a: Int, b: Int) {
    if b <= a {
        lemma_pow2_mono(b, a)
    }
}

/// Alignment (in blocks) to `2^b` implies alignment to `2^a`, `a <= b`.
#[logic]
#[requires(0 <= a && a <= b && x >= 0)]
#[requires(x % b.pow2() == 0)]
#[variant(b - a)]
#[ensures(x % a.pow2() == 0)]
pub fn lemma_pow2_div(x: Int, a: Int, b: Int) {
    if a < b {
        lemma_pow2(b - 1);
        lemma_mod_double(x, (b - 1).pow2());
        lemma_pow2_div(x, a, b - 1)
    }
}

#[logic]
#[requires(0 <= o)]
#[ensures(span(ss, o + 1) == 2 * span(ss, o))]
pub fn lemma_span_succ(ss: Int, o: Int) {
    lemma_pow2(o);
    proof_assert! { (o + 1).pow2() == 2 * o.pow2() };
    proof_assert! { (2 * o.pow2()) * ss == 2 * (o.pow2() * ss) }
}

#[logic]
#[requires(0 <= a && a <= b && 0 <= ss)]
#[ensures(span(ss, a) <= span(ss, b))]
pub fn lemma_span_mono(ss: Int, a: Int, b: Int) {
    lemma_pow2(a);
    lemma_pow2_mono(a, b);
    lemma_mul_le(a.pow2(), b.pow2(), ss)
}

#[logic]
#[requires(ss > 0 && 0 <= o)]
#[ensures(span(ss, o) >= ss)]
#[ensures(span(ss, o) > 0)]
pub fn lemma_span_pos(ss: Int, o: Int) {
    lemma_pow2(o);
    lemma_mul_le(1, o.pow2(), ss)
}

/// Subtracting one `m` from a multiple of `m` lowers the quotient by one.
#[logic]
#[requires(m > 0 && x >= m && x % m == 0)]
#[ensures((x - m) / m == x / m - 1)]
#[ensures((x - m) % m == 0)]
pub fn lemma_div_sub(x: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_mul_mod0(m, x / m - 1)
}

/// A multiple `q*m` of `m` with `q` even is a multiple of `2m`.
#[logic]
#[requires(m > 0 && x >= 0 && x % m == 0 && (x / m) % 2 == 0)]
#[ensures(x % (2 * m) == 0)]
pub fn lemma_even_mult(x: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_divmod(x / m, 2);
    lemma_mul_mod0(2 * m, (x / m) / 2)
}

/// Two exponents bracketing the same value are equal.
#[logic]
#[requires(0 <= a && 0 <= b)]
#[requires(a.pow2() <= x && x < (a + 1).pow2())]
#[requires(b.pow2() <= x && x < (b + 1).pow2())]
#[ensures(a == b)]
pub fn lemma_top_unique(x: Int, a: Int, b: Int) {
    if a < b {
        lemma_pow2_mono(a + 1, b)
    } else if b < a {
        lemma_pow2_mono(b + 1, a)
    }
}

#[logic]
#[requires(0 <= a && a <= b && 0 <= c)]
#[ensures(a * c <= b * c)]
pub fn lemma_mul_le(a: Int, b: Int, c: Int) {}

#[logic]
#[ensures((a * b) * c == a * (b * c))]
pub fn lemma_assoc(a: Int, b: Int, c: Int) {}

#[logic]
#[ensures((a + b) * c == a * c + b * c)]
pub fn lemma_distrib(a: Int, b: Int, c: Int) {}

#[logic]
#[requires(m > 0 && k >= 0)]
#[ensures((m * k) % m == 0)]
#[ensures((m * k) / m == k)]
pub fn lemma_mul_mod0(m: Int, k: Int) {}

#[logic]
#[requires(m > 0)]
#[ensures(m % m == 0)]
pub fn lemma_mod_self(m: Int) {
    lemma_mul_mod0(m, 1)
}

#[logic]
#[requires(m > 0 && x >= 0)]
#[ensures(x == m * (x / m) + x % m)]
#[ensures(0 <= x % m && x % m < m)]
#[ensures(x / m >= 0)]
pub fn lemma_divmod(x: Int, m: Int) {}

#[logic]
#[requires(m > 0 && x >= 0)]
#[requires(x % (2 * m) == 0)]
#[ensures(x % m == 0)]
pub fn lemma_mod_double(x: Int, m: Int) {
    lemma_divmod(x, 2 * m);
    lemma_mul_mod0(m, 2 * (x / (2 * m)))
}

/// Alignment to a larger block implies alignment to every smaller one.
#[logic]
#[requires(ss > 0 && 0 <= a && a <= b && x >= 0)]
#[requires(x % span(ss, b) == 0)]
#[variant(b - a)]
#[ensures(x % span(ss, a) == 0)]
pub fn lemma_span_div(x: Int, ss: Int, a: Int, b: Int) {
    if a < b {
        lemma_pow2(b - 1);
        lemma_pow2(0);
        lemma_mod_double(x, span(ss, b - 1));
        lemma_span_div(x, ss, a, b - 1)
    }
}

#[logic]
#[requires(m > 0 && x >= 0 && y >= 0 && x % m == 0 && y % m == 0)]
#[ensures((x + y) % m == 0)]
#[ensures(x >= y ==> (x - y) % m == 0)]
pub fn lemma_mod_add(x: Int, y: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_divmod(y, m);
    lemma_mul_mod0(m, x / m + y / m);
    if x >= y {
        lemma_mul_mod0(m, x / m - y / m)
    }
}

#[logic]
#[requires(p > 0 && c > 0 && x >= 0 && x % p == 0)]
#[ensures((x * c) % (p * c) == 0)]
pub fn lemma_mul_mod(x: Int, p: Int, c: Int) {
    lemma_divmod(x, p);
    lemma_mul_mod0(p * c, x / p)
}

// ------------------------------------------------------------------- helpers

/// `offset * ss` (buddy.rs:33) for an `offset` aligned to `2^o` blocks: the byte offset is
/// aligned to the order-`o` span.
#[requires(*o >= 0 && ss@ > 0 && offset@ % o.pow2() == 0 && offset@ * ss@ <= u64::MAX@)]
#[ensures(result@ == offset@ * ss@)]
#[ensures((offset@ * ss@) % (o.pow2() * ss@) == 0)]
pub fn mul_aligned(offset: u64, ss: u64, o: Snapshot<Int>) -> u64 {
    proof_assert! { lemma_pow2(*o); o.pow2() >= 1 };
    proof_assert! { lemma_mul_mod(offset@, o.pow2(), ss@); (offset@ * ss@) % (o.pow2() * ss@) == 0 };
    offset * ss
}

/// `1u64 << k` (buddy.rs:32,77,95,135,146).
#[bitwise_proof]
#[requires(k@ < 64)]
#[ensures(result@ == k@.pow2())]
pub fn pow2_u64(k: usize) -> u64 {
    1u64 << k
}

/// `63 - x.leading_zeros() as usize` (buddy.rs:13,31): the index of the top set bit.
#[bitwise_proof]
#[requires(x@ > 0)]
#[ensures(result@ < 64)]
#[ensures(result@.pow2() <= x@ && x@ < (result@ + 1).pow2())]
#[ensures(result@ == 63 - x.leading_zeros_logic()@)]
pub fn top_bit(x: u64) -> usize {
    let lz = x.leading_zeros();
    proof_assert!(x@ < (64u32 - lz)@.pow2());
    proof_assert!((63u32 - lz)@.pow2() <= x@);
    63 - lz as usize
}

/// `if blocks <= 1 { 0 } else { 64 - (blocks - 1).leading_zeros() as usize }`
/// (buddy.rs:59-63, 87-91, 120-124).
#[bitwise_proof]
#[ensures(is_order(result@, blocks@))]
pub fn order_for_blocks_bv(blocks: u64) -> usize {
    if blocks <= 1 {
        0
    } else {
        let x = blocks - 1;
        let lz = x.leading_zeros();
        proof_assert!(x@ < (64u32 - lz)@.pow2());
        proof_assert!((63u32 - lz)@.pow2() <= x@);
        64 - lz as usize
    }
}

/// The least `k >= start` (capped at 64) with `2^k >= b`.
#[logic(open)]
#[variant(64 - k)]
pub fn ord_from(b: Int, k: Int) -> Int {
    pearlite! { if k >= 64 || k < 0 || k.pow2() >= b { k } else { ord_from(b, k + 1) } }
}

/// The buddy order for `b` blocks.
#[logic(open)]
pub fn ord(b: Int) -> Int {
    pearlite! { ord_from(b, 0) }
}

#[logic]
#[requires(0 <= k && k <= r && is_order(r, b))]
#[requires(k == 0 || (k - 1).pow2() < b)]
#[variant(r - k)]
#[ensures(ord_from(b, k) == r)]
pub fn lemma_ord_eq(b: Int, k: Int, r: Int) {
    if k < r {
        lemma_pow2_mono(k, r - 1);
        lemma_ord_eq(b, k + 1, r)
    }
}

#[logic]
#[requires(0 <= k && k <= 64 && 0 <= b && b <= 18446744073709551616)]
#[variant(64 - k)]
#[ensures(ord_from(b, k).pow2() >= b)]
#[ensures(k <= ord_from(b, k) && ord_from(b, k) <= 64)]
pub fn lemma_ord_from_ge(b: Int, k: Int) {
    if k < 64 && k.pow2() < b {
        lemma_ord_from_ge(b, k + 1)
    } else if k == 64 {
        proof_assert! { 64.pow2() == 18446744073709551616 }
    }
}

/// `2^ord(b) >= b` and `0 <= ord(b) <= 64` for every u64-sized `b`.
#[logic]
#[requires(0 <= b && b <= 18446744073709551616)]
#[ensures(ord(b).pow2() >= b)]
#[ensures(0 <= ord(b) && ord(b) <= 64)]
pub fn lemma_ord_ge(b: Int) {
    lemma_ord_from_ge(b, 0)
}

/// `order_for_blocks` with its result named by the logic function [`ord`].
#[ensures(is_order(result@, blocks@))]
#[ensures(result@ == ord(blocks@))]
pub fn order_for_blocks(blocks: u64) -> usize {
    let r = order_for_blocks_bv(blocks);
    proof_assert! { lemma_ord_eq(blocks@, 0, r@); ord(blocks@) == r@ };
    r
}

/// `offset & !(block_span - 1)` (buddy.rs:136).
#[bitwise_proof]
#[requires(s@ > 0)]
#[ensures(result == x & !(s - 1u64))]
pub fn mask_down(x: u64, s: u64) -> u64 {
    x & !(s - 1)
}

/// `current_offset ^ block_span` (buddy.rs:96).
///
/// TRUSTED arithmetic fact F (the only non-std trusted item in the buddy model):
/// if `a` is a multiple of `s > 0` and `a ^ s` is also a multiple of `s`, then
/// `a ^ s` is the arithmetic buddy of `a` (`a + s` when `a/s` is even, `a - s`
/// when odd). Paper proof: write `s = 2^j * m`, `m` odd; `a ^ s = a + s - 2(a & s)`
/// so `S | a^s` forces `a & s ∈ {0, s}` (`s/2` is never a sub-mask of `s`); bit `j`
/// of `a = q*s` is the parity of `q`, which decides which case holds. For a
/// power-of-two `s` this is ordinary XOR buddy arithmetic. Brute-force checked for
/// every `s = 2^k * ss` with `ss < 300`, `k < 6`, `q < 3000`
/// (`~/fv-runs/scratch/em/` reproduction). A native `#[bitwise_proof]` of F
/// was attempted and did not discharge (nonlinear bit-vector `urem`/`udiv`).
#[trusted]
#[requires(s@ > 0 && a@ % s@ == 0 && a@ + s@ <= u64::MAX@)]
#[ensures(result == a ^ s)]
#[ensures(result@ <= a@ + s@)]
#[ensures(result@ % s@ == 0 ==>
    ((a@ / s@) % 2 == 0 && result@ == a@ + s@) || ((a@ / s@) % 2 == 1 && result@ == a@ - s@))]
pub fn buddy_xor(a: u64, s: u64) -> u64 {
    a ^ s
}

/// Mirror of `list.iter().position(|&o| o == x)` (buddy.rs:100-102,124-126,140-142).
#[ensures(match result {
    Some(p) => p@ < v@.len() && v@[p@] == x && forall<q: Int> 0 <= q && q < p@ ==> v@[q] != x,
    None => forall<q: Int> 0 <= q && q < v@.len() ==> v@[q] != x,
})]
pub fn find_pos(v: &Vec<u64>, x: u64) -> Option<usize> {
    let mut i: usize = 0;
    #[invariant(i@ <= v@.len())]
    #[invariant(forall<q: Int> 0 <= q && q < i@ ==> v@[q] != x)]
    while i < v.len() {
        if v[i] == x {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Mirror of `Vec::swap_remove(pos)` (buddy.rs:104,128,144): the last element
/// takes the removed element's place.
#[requires(p@ < v@.len())]
#[ensures((^v)@.len() == v@.len() - 1)]
#[ensures(forall<k: Int> 0 <= k && k < (^v)@.len() ==>
    (^v)@[k] == if k == p@ { v@[v@.len() - 1] } else { v@[k] })]
pub fn swap_remove(v: &mut Vec<u64>, p: usize) {
    let n = v.len();
    v.swap(p, n - 1);
    let _ = v.pop();
}

// ---------------------------------------------------------------- invariants

/// Shape facts every method relies on.
#[logic(open)]
pub fn bd_shape(b: BuddyAllocator) -> bool {
    pearlite! {
        b.sector_size@ > 0
        && b.max_order@ < 64
        && b.free_lists@.len() == b.max_order@ + 1
        && b.base_offset@ + b.total_usable_size@ <= u64::MAX@
        && (b.max_order@ == 0 || span(b.sector_size@, b.max_order@) <= b.total_usable_size@)
    }
}

/// EM-BUDDY-ALLOC-ALIGNED (free half): a free order-`o` block starts at a multiple
/// of its own size, relative to the region base.
#[logic(open)]
pub fn bd_aligned(b: BuddyAllocator) -> bool {
    pearlite! {
        forall<o: Int, k: Int> 0 <= o && o < b.free_lists@.len() && 0 <= k && k < b.free_lists@[o]@.len() ==>
            b.free_lists@[o]@[k]@ % span(b.sector_size@, o) == 0
    }
}

/// Every free block lies inside the region's usable bytes.
#[logic(open)]
pub fn bd_inside(b: BuddyAllocator) -> bool {
    pearlite! {
        forall<o: Int, k: Int> 0 <= o && o < b.free_lists@.len() && 0 <= k && k < b.free_lists@[o]@.len() ==>
            b.free_lists@[o]@[k]@ + span(b.sector_size@, o) <= b.total_usable_size@
    }
}

#[logic(open)]
pub fn bd_inv(b: BuddyAllocator) -> bool {
    pearlite! { bd_shape(b) && bd_aligned(b) && bd_inside(b) }
}

/// An allocated (or to-be-freed) block of order `o` at relative offset `x`.
#[logic(open)]
pub fn good_block(b: BuddyAllocator, x: Int, o: Int) -> bool {
    pearlite! { 0 <= o && x % span(b.sector_size@, o) == 0 && x + span(b.sector_size@, o) <= b.total_usable_size@ }
}

/// Freeing the order-`o` block at relative offset `x` merges nothing: the XOR buddy
/// `x ^ span(o)` that buddy.rs:96 computes is out of range (buddy.rs:98) or not on the
/// order-`o` free list (buddy.rs:100-102).
#[logic(open)]
pub fn no_merge(b: BuddyAllocator, x: u64, o: Int) -> bool {
    pearlite! {
        forall<s: u64> s@ == span(b.sector_size@, o) ==>
            ((x ^ s)@ + s@ > b.total_usable_size@
             || forall<q: Int> 0 <= q && q < b.free_lists@[o]@.len() ==> b.free_lists@[o]@[q] != (x ^ s))
    }
}

/// mark_allocated's single-split case (buddy.rs:134-155 with `search_order = target + 1`):
/// the exact block `x` is not on list `t`, and list `t+1` holds exactly the block `a` that
/// the mask `x & !(span(t+1) - 1)` (buddy.rs:136) names.
#[logic(open)]
pub fn split1_pre(b: BuddyAllocator, x: u64, t: Int, a: u64) -> bool {
    pearlite! {
        0 <= t && t + 1 <= b.max_order@
        && (forall<q: Int> 0 <= q && q < b.free_lists@[t]@.len() ==> b.free_lists@[t]@[q] != x)
        && (forall<sp: u64> sp@ == span(b.sector_size@, t + 1) ==> a == (x & !(sp - 1u64)))
        && b.free_lists@[t + 1]@ == Seq::singleton(a)
    }
}

/// The half of block `a` that buddy.rs:147-152 puts back on list `t`.
#[logic(open)]
pub fn split1_val(b: BuddyAllocator, x: u64, t: Int, a: u64) -> Int {
    pearlite! { if x@ >= a@ + span(b.sector_size@, t) { a@ } else { a@ + span(b.sector_size@, t) } }
}

/// Its exact effect: block `a` leaves list `t+1`, its other half joins list `t`.
#[logic(open)]
pub fn split1_post(b: BuddyAllocator, n: BuddyAllocator, x: u64, t: Int, a: u64) -> bool {
    pearlite! {
        n.free_lists@.len() == b.free_lists@.len()
        && n.free_lists@[t + 1]@.len() == 0
        && pushed(n.free_lists@[t]@, b.free_lists@[t]@, split1_val(b, x, t, a))
        && (forall<p: Int> 0 <= p && p < b.free_lists@.len() && p != t && p != t + 1 ==> n.free_lists@[p] == b.free_lists@[p])
    }
}

/// Lists agree everywhere except possibly at order `o`.
#[logic(open)]
pub fn same_except(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, o: Int) -> bool {
    pearlite! { a.len() == b.len() && forall<p: Int> 0 <= p && p < a.len() && p != o ==> a[p] == b[p] }
}

/// Free-space accounting (`total_free`) — its own module so that `tf`'s body stays private.
pub mod tfm {
    use super::*;
    /// Sum of free bytes: what `total_free` (buddy.rs:160-166) computes, over orders `0..n`.
    #[logic]
    #[variant(n)]
    pub fn tf(fl: Seq<Vec<u64>>, ss: Int, n: Int) -> Int {
        pearlite! { if n <= 0 { 0 } else { tf(fl, ss, n - 1) + fl[n - 1]@.len() * span(ss, n - 1) } }
    }

    /// Partial sums of `tf` are bounded by the total.
    #[logic]
    #[variant(n)]
    #[requires(ss > 0 && 0 <= n)]
    #[ensures(forall<m: Int> 0 <= m && m <= n ==> 0 <= tf(fl, ss, m) && tf(fl, ss, m) <= tf(fl, ss, n))]
    pub fn lemma_tf_mono(fl: Seq<Vec<u64>>, ss: Int, n: Int) {
        pearlite! {
            if n > 0 {
                lemma_tf_mono(fl, ss, n - 1);
                lemma_span_pos(ss, n - 1);
                lemma_mul_le(0, fl[n - 1]@.len(), span(ss, n - 1))
            }
        }
    }

    /// `tf` over lists whose lengths differ only at order `o`.
    #[logic]
    #[variant(n)]
    #[requires(0 <= n && n <= a.len() && a.len() == b.len())]
    #[requires(forall<p: Int> 0 <= p && p < a.len() && p != o ==> a[p]@.len() == b[p]@.len())]
    #[ensures(tf(a, ss, n) == tf(b, ss, n) + if 0 <= o && o < n { (a[o]@.len() - b[o]@.len()) * span(ss, o) } else { 0 })]
    pub fn lemma_tf_one(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, n: Int) {
        pearlite! {
            if n > 0 {
                lemma_tf_one(a, b, ss, o, n - 1);
                lemma_distrib(a[n - 1]@.len() - b[n - 1]@.len(), b[n - 1]@.len(), span(ss, n - 1))
            }
        }
    }

    /// One block of order `o` more at list `o`: `tf` grows by `span(o)`.
    #[logic]
    #[requires(0 <= o && o < a.len() && a.len() == b.len())]
    #[requires(forall<p: Int> 0 <= p && p < a.len() && p != o ==> a[p] == b[p])]
    #[requires(a[o]@.len() == b[o]@.len() + 1)]
    #[requires(d == span(ss, o))]
    #[ensures(tf(a, ss, a.len()) == tf(b, ss, b.len()) + d)]
    pub fn lemma_tf_inc(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, d: Int) {
        lemma_tf_one(a, b, ss, o, a.len())
    }

    /// One block of order `o` fewer at list `o`: `tf` shrinks by `span(o)`.
    #[logic]
    #[requires(0 <= o && o < a.len() && a.len() == b.len())]
    #[requires(forall<p: Int> 0 <= p && p < a.len() && p != o ==> a[p] == b[p])]
    #[requires(a[o]@.len() == b[o]@.len() - 1)]
    #[requires(d == span(ss, o))]
    #[ensures(tf(a, ss, a.len()) == tf(b, ss, b.len()) - d)]
    pub fn lemma_tf_dec(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, d: Int) {
        lemma_tf_one(a, b, ss, o, a.len())
    }

    /// `tf` of all-empty lists is 0.
    #[logic]
    #[variant(n)]
    #[requires(0 <= n && n <= a.len())]
    #[requires(forall<p: Int> 0 <= p && p < n ==> a[p]@.len() == 0)]
    #[ensures(tf(a, ss, n) == 0)]
    pub fn lemma_tf_empty(a: Seq<Vec<u64>>, ss: Int, n: Int) {
        if n > 0 {
            lemma_tf_empty(a, ss, n - 1)
        }
    }

    /// One unfolding step of `tf` (its body is private to this module: an open recursive
    /// `tf` made the large buddy VCs time out through definitional unfolding).
    #[logic]
    #[requires(n > 0)]
    #[ensures(tf(fl, ss, n) == tf(fl, ss, n - 1) + fl[n - 1]@.len() * span(ss, n - 1))]
    pub fn lemma_tf_step(fl: Seq<Vec<u64>>, ss: Int, n: Int) {}

    /// `a * b` behind a private body (keeps a nonlinear product out of loop invariants).
    #[logic]
    pub fn mulw(a: Int, b: Int) -> Int {
        pearlite! { a * b }
    }

    #[logic]
    #[ensures(mulw(a, b) == a * b)]
    pub fn lemma_mulw(a: Int, b: Int) {}

    /// One step of `BuddyAllocator::new`'s accounting: `tf == offset * ss` is preserved
    /// when `blocks` sectors are pushed and `offset` advances by `blocks`.
    #[logic]
    #[requires(tf(b, ss, b.len()) == mulw(off, ss))]
    #[requires(tf(a, ss, a.len()) == tf(b, ss, b.len()) + mulw(blocks, ss))]
    #[ensures(tf(a, ss, a.len()) == mulw(off + blocks, ss))]
    pub fn lemma_acct(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, off: Int, blocks: Int) {
        lemma_distrib(off, blocks, ss)
    }

    /// `free_lists[o].push(v)` (buddy.rs:34, 78, 114, 148, 151), with its effect on `tf`.
    #[requires(o@ < fl@.len())]
    #[ensures((^fl)@.len() == fl@.len())]
    #[ensures((^fl)@[o@]@ == fl@[o@]@.push_back(v))]
    #[ensures(forall<p: Int> 0 <= p && p < fl@.len() && p != o@ ==> (^fl)@[p] == fl@[p])]
    #[ensures(tf((^fl)@, *ss, (^fl)@.len()) == tf(fl@, *ss, fl@.len()) + span(*ss, o@))]
    pub fn push_at(fl: &mut Vec<Vec<u64>>, o: usize, v: u64, ss: Snapshot<Int>) {
        let before = snapshot! { *fl };
        fl[o].push(v);
        proof_assert! { forall<p: Int> 0 <= p && p < fl@.len() && p != o@ ==> fl@[p] == before@[p] };
        proof_assert! { lemma_tf_inc(fl@, before@, *ss, o@, span(*ss, o@));
            tf(fl@, *ss, fl@.len()) == tf(before@, *ss, before@.len()) + span(*ss, o@) };
    }

    /// `tf` over zero orders.
    #[logic]
    #[ensures(tf(fl, ss, 0) == 0)]
    pub fn lemma_tf_zero(fl: Seq<Vec<u64>>, ss: Int) {}

    /// `tf` of a one-order allocator, spelled out (LEVEL-3 phase D-A).
    #[logic]
    #[requires(fl.len() == 1)]
    #[ensures(tf(fl, ss, 1) == fl[0]@.len() * span(ss, 0))]
    pub fn lemma_tf1(fl: Seq<Vec<u64>>, ss: Int) {}

    /// `tf` of a two-order allocator, spelled out.
    #[logic]
    #[requires(fl.len() == 2)]
    #[ensures(tf(fl, ss, 2) == fl[0]@.len() * span(ss, 0) + fl[1]@.len() * span(ss, 1))]
    pub fn lemma_tf2(fl: Seq<Vec<u64>>, ss: Int) {}

    /// `tf` of a three-order allocator, spelled out.
    #[logic]
    #[requires(fl.len() == 3)]
    #[ensures(tf(fl, ss, 3) == fl[0]@.len() * span(ss, 0) + fl[1]@.len() * span(ss, 1) + fl[2]@.len() * span(ss, 2))]
    pub fn lemma_tf3(fl: Seq<Vec<u64>>, ss: Int) {}

    /// `tf` of a four-order allocator, spelled out.
    #[logic]
    #[requires(fl.len() == 4)]
    #[ensures(tf(fl, ss, 4) == fl[0]@.len() * span(ss, 0) + fl[1]@.len() * span(ss, 1) + fl[2]@.len() * span(ss, 2) + fl[3]@.len() * span(ss, 3))]
    pub fn lemma_tf4(fl: Seq<Vec<u64>>, ss: Int) {}


}
pub use tfm::*;

/// First non-empty order in `o..=mo` (or -1): the order `alloc`'s search loop finds.
#[logic(open)]
#[variant(mo - o)]
pub fn first_ne(fl: Seq<Vec<u64>>, o: Int, mo: Int) -> Int {
    pearlite! {
        if o > mo || o < 0 { -1 } else if fl[o]@.len() > 0 { o } else { first_ne(fl, o + 1, mo) }
    }
}

#[logic]
#[variant(mo - o)]
#[requires(0 <= o)]
#[ensures(first_ne(fl, o, mo) == -1 ==> forall<p: Int> o <= p && p <= mo ==> fl[p]@.len() == 0)]
#[ensures(first_ne(fl, o, mo) != -1 ==> o <= first_ne(fl, o, mo) && first_ne(fl, o, mo) <= mo
    && fl[first_ne(fl, o, mo)]@.len() > 0
    && forall<p: Int> o <= p && p < first_ne(fl, o, mo) ==> fl[p]@.len() == 0)]
pub fn lemma_first_ne(fl: Seq<Vec<u64>>, o: Int, mo: Int) {
    pearlite! {
        if o <= mo && fl[o]@.len() == 0 {
            lemma_first_ne(fl, o + 1, mo)
        }
    }
}

impl BuddyAllocator {
    /// Mirror of `BuddyAllocator::new` (buddy.rs:10-46).
    #[requires(sector_size@ > 0)]
    #[requires(base_offset@ + total_usable_size@ <= u64::MAX@)]
    #[ensures(result.base_offset == base_offset && result.total_usable_size == total_usable_size)]
    #[ensures(result.sector_size == sector_size)]
    #[ensures(bd_shape(result))]
    #[ensures(bd_aligned(result))]
    #[ensures(bd_inside(result))]
    #[ensures(bd_inv(result))]
    #[ensures(total_usable_size@ / sector_size@ >= 1 ==> total_usable_size@ / sector_size@ < (result.max_order@ + 1).pow2())]
    #[ensures(forall<m: Int> 0 <= m && m.pow2() == total_usable_size@ / sector_size@ ==>
        result.max_order@ == m && result.free_lists@[m]@ == Seq::singleton(0u64)
        && forall<o: Int> 0 <= o && o < m ==> result.free_lists@[o]@.len() == 0)]
    #[ensures(result.max_order@.pow2() == total_usable_size@ / sector_size@ ==>
        result.free_lists@[result.max_order@]@ == Seq::singleton(0u64)
        && forall<o: Int> 0 <= o && o < result.max_order@ ==> result.free_lists@[o]@.len() == 0)]
    #[ensures(total_usable_size@ / sector_size@ >= 1 ==> result.max_order@.pow2() <= total_usable_size@ / sector_size@)]
    #[ensures(tf(result.free_lists@, sector_size@, result.free_lists@.len()) == (total_usable_size@ / sector_size@) * sector_size@)]
    #[ensures(new_e(result.free_lists@, sector_size@, (total_usable_size@ / sector_size@) * sector_size@))]
    pub fn new(base_offset: u64, total_usable_size: u64, sector_size: u32) -> Self {
        let ss = sector_size as u64;
        let usable_blocks = total_usable_size / ss;
        proof_assert! { usable_blocks@ * ss@ <= total_usable_size@ };
        let max_order = if usable_blocks > 1 {
            top_bit(usable_blocks)
        } else if usable_blocks == 1 {
            proof_assert! { lemma_pow2(0); 1.pow2() == 2 };
            0
        } else {
            // `free_lists: vec![Vec::new()]` (buddy.rs:22)
            let mut one: Vec<Vec<u64>> = Vec::new();
            one.push(Vec::new());
            proof_assert! { one@.len() == 1 && one@[0]@.len() == 0 };
            proof_assert! { lemma_tf_empty(one@, ss@, 1); tf(one@, ss@, 1) == 0 };
            proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss@, o); span(ss@, o) > 0 } };
            proof_assert! { lemma_cov0(one@, ss@); fdisj(one@, ss@) && cov(one@, ss@, 0) };
            proof_assert! { usable_blocks@ == 0 };
            proof_assert! { lemma_new_e_intro(one@, ss@, 0); new_e(one@, ss@, 0) };
            return Self { base_offset, total_usable_size, sector_size, max_order: 0, free_lists: one };
        };
        snapshot! { lemma_pow2(max_order@) };
        proof_assert! { max_order@.pow2() <= usable_blocks@ };
        proof_assert! { forall<m: Int> 0 <= m && m.pow2() == usable_blocks@ ==> { lemma_pow2(m); lemma_top_unique(usable_blocks@, m, max_order@); m == max_order@ } };
        let is_p2 = snapshot! { max_order@.pow2() == usable_blocks@ };
        proof_assert! { lemma_mul_le(max_order@.pow2(), usable_blocks@, ss@); max_order@.pow2() * ss@ <= usable_blocks@ * ss@ };
        proof_assert! { span(ss@, max_order@) <= total_usable_size@ };

        let mut free_lists: Vec<Vec<u64>> = creusot_std::vec![Vec::new(); max_order + 1];

        let mut offset: u64 = 0;
        let mut remaining = usable_blocks;
        let mut last = snapshot! { max_order@ + 1 };
        proof_assert! { lemma_tf_empty(free_lists@, ss@, free_lists@.len()); tf(free_lists@, ss@, free_lists@.len()) == 0 };
        proof_assert! { lemma_mulw(0, ss@); mulw(0, ss@) == 0 };
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss@, o); span(ss@, o) > 0 } };
        proof_assert! { forall<o: Int> 0 <= o && o < free_lists@.len() ==> free_lists@[o]@.len() == 0 };
        proof_assert! { lemma_cov0(free_lists@, ss@); fdisj(free_lists@, ss@) && cov(free_lists@, ss@, 0 * ss@) };
        #[invariant(free_lists@.len() == max_order@ + 1)]
        #[invariant(*is_p2 && remaining@ > 0 ==> remaining@ == usable_blocks@ && offset@ == 0 && *last == max_order@ + 1
                && forall<o: Int> 0 <= o && o <= max_order@ ==> free_lists@[o]@.len() == 0)]
        #[invariant(*is_p2 && remaining@ == 0 ==> free_lists@[max_order@]@ == Seq::singleton(0u64)
                && forall<o: Int> 0 <= o && o < max_order@ ==> free_lists@[o]@.len() == 0)]
        #[invariant(offset@ + remaining@ == usable_blocks@)]
        #[invariant(tf(free_lists@, ss@, free_lists@.len()) == mulw(offset@, ss@))]
        #[invariant(0 <= *last && *last <= max_order@ + 1)]
        #[invariant(remaining@ < (*last).pow2())]
        #[invariant(offset@ % (*last).pow2() == 0)]
        #[invariant(forall<o: Int, k: Int> 0 <= o && o < free_lists@.len() && 0 <= k && k < free_lists@[o]@.len() ==>
            free_lists@[o]@[k]@ % span(ss@, o) == 0 && free_lists@[o]@[k]@ + span(ss@, o) <= offset@ * ss@)]
        #[invariant(fdisj(free_lists@, ss@))]
        #[invariant(cov(free_lists@, ss@, offset@ * ss@))]
        while remaining > 0 {
            let order = top_bit(remaining);
            proof_assert! { lemma_pow2(order@); order@.pow2() >= 1 };
            proof_assert! { *is_p2 ==> { lemma_top_unique(remaining@, order@, max_order@); order@ == max_order@ } };
            snapshot! { lemma_pow2_strict(order@, *last) };
            proof_assert! { order@ < *last };
            let blocks = pow2_u64(order);
            snapshot! { lemma_pow2_div(offset@, order@, *last) };
            proof_assert! { offset@ % order@.pow2() == 0 };
            proof_assert! { lemma_mul_le(offset@, usable_blocks@, ss@); offset@ * ss@ <= usable_blocks@ * ss@ };
            let byte_offset = mul_aligned(offset, ss, snapshot! { order@ }); // `offset * sector_size as u64` (buddy.rs:33)
            proof_assert! { (offset@ * ss@) % (order@.pow2() * ss@) == 0 };
            let before = snapshot! { free_lists };
            push_at(&mut free_lists, order, byte_offset, snapshot! { ss@ }); // `free_lists[order].push(byte_offset)`
            proof_assert! { pu_rel(before@, free_lists@, order@, byte_offset) };
            proof_assert! { lemma_pow2_div(offset@, order@ + 1, *last); offset@ % (order@ + 1).pow2() == 0 };
            proof_assert! { byte_offset@ == offset@ * ss@ && ss@ > 0 && 0 <= order@ && offset@ >= 0 };
            proof_assert! { forall<o2: Int, k2: Int> fv(before@, o2, k2) ==> fs(before@, o2, k2) + span(ss@, o2) <= offset@ * ss@ };
            proof_assert! { fdisj(before@, ss@) && cov(before@, ss@, offset@ * ss@) };
            new_step_g(snapshot! { before@ }, snapshot! { free_lists@ }, snapshot! { ss@ }, snapshot! { offset@ }, snapshot! { order@ }, byte_offset);
            proof_assert! { free_lists@[order@]@ == before@[order@]@.push_back(byte_offset) };
            proof_assert! { forall<o: Int> 0 <= o && o < free_lists@.len() && o != order@ ==> free_lists@[o] == before@[o] };
            proof_assert! { lemma_distrib(offset@, blocks@, ss@); (offset@ + blocks@) * ss@ == offset@ * ss@ + blocks@ * ss@ };
            proof_assert! { lemma_mul_le(offset@, offset@ + blocks@, ss@); offset@ * ss@ <= (offset@ + blocks@) * ss@ };
            proof_assert! { lemma_mod_self(order@.pow2()); blocks@ % order@.pow2() == 0 };
            proof_assert! { lemma_mod_add(offset@, blocks@, order@.pow2()); (offset@ + blocks@) % order@.pow2() == 0 };
            proof_assert! { span(ss@, order@) == order@.pow2() * ss@ };
            proof_assert! { byte_offset@ % span(ss@, order@) == 0 };
            proof_assert! { order@.pow2() * ss@ == blocks@ * ss@ };
            proof_assert! { byte_offset@ == offset@ * ss@ };
            proof_assert! { byte_offset@ + span(ss@, order@) == (offset@ + blocks@) * ss@ };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < free_lists@.len() && 0 <= k && k < free_lists@[o]@.len() ==>
                free_lists@[o]@[k] == if o == order@ && k == before@[o]@.len() { byte_offset } else { before@[o]@[k] } };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < before@.len() && 0 <= k && k < before@[o]@.len() ==>
                before@[o]@[k]@ + span(ss@, o) <= (offset@ + blocks@) * ss@ };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < free_lists@.len() && 0 <= k && k < free_lists@[o]@.len() ==>
                free_lists@[o]@[k]@ + span(ss@, o) <= (offset@ + blocks@) * ss@ };
            let off_before = snapshot! { offset };
            offset += blocks;
            proof_assert! { offset@ == off_before@ + blocks@ };
            proof_assert! { lemma_mulw(blocks@, ss@); span(ss@, order@) == mulw(blocks@, ss@) };
            acct_g(snapshot! { free_lists@ }, snapshot! { before@ }, snapshot! { ss@ }, snapshot! { off_before@ }, snapshot! { blocks@ });
            proof_assert! { tf(free_lists@, ss@, free_lists@.len()) == mulw(offset@, ss@) };
            proof_assert! { offset@ * ss@ == (off_before@ + blocks@) * ss@ };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < free_lists@.len() && 0 <= k && k < free_lists@[o]@.len() ==>
                free_lists@[o]@[k]@ + span(ss@, o) <= offset@ * ss@ };
            remaining -= blocks;
            last = snapshot! { order@ };
        }

        proof_assert! { offset@ == usable_blocks@ };
        proof_assert! { offset@ * ss@ <= total_usable_size@ };
        proof_assert! { cov(free_lists@, ss@, usable_blocks@ * ss@) };
        proof_assert! { lemma_new_e_intro(free_lists@, ss@, usable_blocks@ * ss@); new_e(free_lists@, ss@, usable_blocks@ * ss@) };
        proof_assert! { lemma_mulw(offset@, ss@); tf(free_lists@, ss@, free_lists@.len()) == usable_blocks@ * ss@ };
        Self { base_offset, total_usable_size, sector_size, max_order, free_lists }
    }

    /// Mirror of `BuddyAllocator::free` (buddy.rs:84-115).
    ///
    /// Precondition: the freed block is a well-formed order-`K` block (aligned,
    /// inside) with `K <= max_order` — what `alloc` returned for this slab.
    /// `total + span(max_order) <= u64::MAX` keeps `buddy_offset + block_span`
    /// (buddy.rs:98) from overflowing; see the crate-level note on this bound.
    #[requires(bd_inv(*self))]
    #[requires(abs_offset@ >= self.base_offset@)]
    #[requires(ord(size@ / self.sector_size@) <= self.max_order@)]
    #[requires(good_block(*self, abs_offset@ - self.base_offset@, ord(size@ / self.sector_size@)))]
    #[requires(self.total_usable_size@ + span(self.sector_size@, self.max_order@) <= u64::MAX@)]
    #[ensures(bd_shape(^self))]
    #[ensures(bd_aligned(^self))]
    #[ensures(bd_inside(^self))]
    #[ensures(bd_inv(^self))]
    #[ensures((^self).base_offset == self.base_offset && (^self).total_usable_size == self.total_usable_size)]
    #[ensures((^self).sector_size == self.sector_size && (^self).max_order == self.max_order)]
    #[ensures((^self).free_lists@.len() == self.free_lists@.len())]
    #[ensures(ord(size@ / self.sector_size@) == self.max_order@
        || self.free_lists@[ord(size@ / self.sector_size@)]@.len() == 0 ==>
        same_except((^self).free_lists@, self.free_lists@, ord(size@ / self.sector_size@))
        && pushed((^self).free_lists@[ord(size@ / self.sector_size@)]@, self.free_lists@[ord(size@ / self.sector_size@)]@,
                  abs_offset@ - self.base_offset@))]
    #[ensures(tf((^self).free_lists@, self.sector_size@, (^self).free_lists@.len())
        == tf(self.free_lists@, self.sector_size@, self.free_lists@.len()) + span(self.sector_size@, ord(size@ / self.sector_size@)))]
    #[ensures(forall<x: u64> x@ == abs_offset@ - self.base_offset@ && no_merge(*self, x, ord(size@ / self.sector_size@)) ==>
        same_except((^self).free_lists@, self.free_lists@, ord(size@ / self.sector_size@))
        && pushed((^self).free_lists@[ord(size@ / self.sector_size@)]@, self.free_lists@[ord(size@ / self.sector_size@)]@, x@))]
    #[ensures(forall<j: Int> ord(size@ / self.sector_size@) <= j && j <= self.max_order@ && self.free_lists@[j]@.len() == 0 ==>
        forall<p: Int> j < p && p < self.free_lists@.len() ==> (^self).free_lists@[p] == self.free_lists@[p])]
    #[ensures(free_e(self.free_lists@, (^self).free_lists@, self.sector_size@, abs_offset@ - self.base_offset@,
        span(self.sector_size@, ord(size@ / self.sector_size@))))]
    #[ensures(free_m(self.free_lists@, (^self).free_lists@, self.sector_size@, abs_offset@ - self.base_offset@,
        span(self.sector_size@, ord(size@ / self.sector_size@))))]
    pub fn free(&mut self, abs_offset: u64, size: u64) {
        let ss = self.sector_size as u64;
        let offset = abs_offset - self.base_offset;
        let blocks = size / ss;
        let mut order = order_for_blocks(blocks);
        let k0 = snapshot! { order@ };
        let mut current_offset = offset;
        let old = snapshot! { *self };
        g_fm_init(snapshot! { old.free_lists@ }, snapshot! { ss@ }, snapshot! { offset@ }, snapshot! { span(ss@, *k0) }); // PROOF-ONLY (ghost)

        #[invariant(*k0 <= order@ && order@ <= self.max_order@)]
        #[invariant(good_block(*self, current_offset@, order@))]
        #[invariant(fbi(self.free_lists@, ss@, current_offset@, span(ss@, order@), old.free_lists@, offset@, span(ss@, *k0)))]
        #[invariant(coal(old.free_lists@, ss@) ==> coal(self.free_lists@, ss@))]
        #[invariant(self.free_lists@.len() == old.free_lists@.len())]
        #[invariant(self.base_offset == old.base_offset && self.total_usable_size == old.total_usable_size)]
        #[invariant(self.sector_size == old.sector_size && self.max_order == old.max_order)]
        #[invariant(bd_aligned(*self) && bd_inside(*self))]
        #[invariant(order@ == *k0 ==> *self == *old && current_offset == offset)]
        #[invariant(old.free_lists@[*k0]@.len() == 0 || *k0 == old.max_order@ ==> order@ == *k0)]
        #[invariant(tf(self.free_lists@, ss@, self.free_lists@.len()) + span(ss@, order@) == tf(old.free_lists@, ss@, old.free_lists@.len()) + span(ss@, *k0))]
        #[invariant(no_merge(*old, offset, *k0) ==> order@ == *k0)]
        #[invariant(forall<p: Int> order@ <= p && p < self.free_lists@.len() ==> self.free_lists@[p] == old.free_lists@[p])]
        #[invariant(forall<j: Int> *k0 <= j && j <= old.max_order@ && old.free_lists@[j]@.len() == 0 ==> order@ <= j)]
        #[invariant(fdisj(old.free_lists@, ss@) && faway(old.free_lists@, ss@, offset@, span(ss@, *k0)) ==>
            fdisj(self.free_lists@, ss@) && faway(self.free_lists@, ss@, current_offset@, span(ss@, order@)))]
        #[invariant(forall<y: Int, l: Int> 0 < l && faway(old.free_lists@, ss@, y, l) && disj(y, l, offset@, span(ss@, *k0)) ==>
            faway(self.free_lists@, ss@, y, l) && disj(y, l, current_offset@, span(ss@, order@)))]
        while order < self.max_order {
            proof_assert! { lemma_span_pos(ss@, order@); span(ss@, order@) > 0 };
            proof_assert! { lemma_span_mono(ss@, order@, self.max_order@); span(ss@, order@) <= span(ss@, self.max_order@) };
            proof_assert! { span(ss@, order@) <= u64::MAX@ };
            let block_span = pow2_u64(order) * ss;
            proof_assert! { block_span@ == span(ss@, order@) };
            let buddy_offset = buddy_xor(current_offset, block_span);
            g_xor(current_offset, block_span, snapshot! { ss@ }, snapshot! { order@ }); // PROOF-ONLY (ghost)

            if buddy_offset + block_span > self.total_usable_size {
                proof_assert! { p2(ss@) ==> (current_offset@ / block_span@) % 2 == 0 && buddy_offset@ == current_offset@ + block_span@ };
                proof_assert! { p2(ss@) ==> pnl(self.free_lists@, ss@, order@, current_offset@) };
                break;
            }

            match find_pos(&self.free_lists[order], buddy_offset) {
                Some(pos) => {
                    proof_assert! { self.free_lists@[order@] == old.free_lists@[order@] };
                    proof_assert! { old.free_lists@[order@]@.len() > 0 };
                    proof_assert! { order@ == *k0 ==> current_offset == offset };
                    proof_assert! { order@ == *k0 && no_merge(*old, offset, *k0) ==> {
                        lemma_span_pos(ss@, order@);
                        (offset ^ block_span)@ + block_span@ > old.total_usable_size@
                        || (forall<q: Int> 0 <= q && q < old.free_lists@[*k0]@.len() ==> old.free_lists@[*k0]@[q] != (offset ^ block_span)) } };
                    proof_assert! { !(order@ == *k0 && no_merge(*old, offset, *k0)) };
                    proof_assert! { buddy_offset@ % span(ss@, order@) == 0 };
                    proof_assert! { lemma_span_succ(ss@, order@); span(ss@, order@ + 1) == 2 * span(ss@, order@) };
                    let before = snapshot! { self.free_lists };
                    swap_remove(&mut self.free_lists[order], pos);
                    proof_assert! { forall<o: Int> 0 <= o && o < before@.len() && o != order@ ==> self.free_lists@[o] == before@[o] };
                    proof_assert! { sr_rel(before@, self.free_lists@, order@, pos@) };
                    proof_assert! { fs(before@, order@, pos@) == buddy_offset@ };
                    let mid = snapshot! { self.free_lists };
                    proof_assert! { lemma_sr(before@, mid@, ss@, order@, pos@); true };
                    proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                        exists<j: Int> 0 <= j && j < before@[o]@.len() && self.free_lists@[o]@[k] == before@[o]@[j] };
                    proof_assert! { self.free_lists@[order@]@.len() - before@[order@]@.len() == -1 };
                    proof_assert! { lemma_tf_dec(self.free_lists@, before@, ss@, order@, span(ss@, order@));
                        tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) - span(ss@, order@) };
                    // `current_offset = current_offset.min(buddy_offset)` (buddy.rs:105)
                    proof_assert! { (current_offset@ / span(ss@, order@)) % 2 == 1 ==> {
                        lemma_div_sub(current_offset@, span(ss@, order@));
                        (buddy_offset@ / span(ss@, order@)) % 2 == 0 } };
                    proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                        self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
                    proof_assert! { bd_aligned(*self) && bd_inside(*self) };
                    let cur_prev = snapshot! { current_offset };
                    proof_assert! { buddy_offset@ == cur_prev@ + span(ss@, order@) || buddy_offset@ == cur_prev@ - span(ss@, order@) };
                    proof_assert! { cur_prev@ + span(ss@, order@) <= self.total_usable_size@ && buddy_offset@ + span(ss@, order@) <= self.total_usable_size@ };
                    proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss@, o); span(ss@, o) > 0 } };
                    proof_assert! { fdisj(old.free_lists@, ss@) && faway(old.free_lists@, ss@, offset@, span(ss@, *k0)) ==> {
                        lemma_away_union(mid@, ss@, cur_prev@, buddy_offset@, span(ss@, order@));
                        fdisj(mid@, ss@) && faway(mid@, ss@, if cur_prev@ <= buddy_offset@ { cur_prev@ } else { buddy_offset@ }, 2 * span(ss@, order@)) } };
                    proof_assert! { forall<y: Int, l: Int> 0 < l && faway(old.free_lists@, ss@, y, l) && disj(y, l, offset@, span(ss@, *k0)) ==> {
                        lemma_disj_union(y, l, cur_prev@, buddy_offset@, span(ss@, order@));
                        faway(mid@, ss@, y, l) && disj(y, l, if cur_prev@ <= buddy_offset@ { cur_prev@ } else { buddy_offset@ }, 2 * span(ss@, order@)) } };
                    current_offset = if current_offset <= buddy_offset { current_offset } else { buddy_offset };
                    g_fm_merge(snapshot! { before@ }, snapshot! { mid@ }, snapshot! { ss@ }, snapshot! { order@ }, snapshot! { pos@ },
                        snapshot! { cur_prev@ }, snapshot! { buddy_offset@ }, snapshot! { span(ss@, order@) },
                        snapshot! { old.free_lists@ }, snapshot! { offset@ }, snapshot! { span(ss@, *k0) }); // PROOF-ONLY (ghost)
                    proof_assert! { current_offset@ + 2 * span(ss@, order@) <= self.total_usable_size@ };
                    proof_assert! { (current_offset@ / span(ss@, order@)) % 2 == 0 };
                    proof_assert! { current_offset@ % span(ss@, order@) == 0 };
                    proof_assert! { lemma_even_mult(current_offset@, span(ss@, order@)); current_offset@ % span(ss@, order@ + 1) == 0 };
                    proof_assert! { span(ss@, order@ + 1) == 2 * span(ss@, order@) && current_offset@ + span(ss@, order@ + 1) <= self.total_usable_size@ };
                    proof_assert! { good_block(*self, current_offset@, order@ + 1) };
                    order += 1;
                }
                None => {
                    proof_assert! { p2(ss@) ==> buddy_offset@ == bpart(ss@, order@, current_offset@) };
                    proof_assert! { p2(ss@) ==> pnl(self.free_lists@, ss@, order@, current_offset@) };
                    break
                }
            }
        }

        proof_assert! { p2(ss@) ==> pnl(self.free_lists@, ss@, order@, current_offset@) };
        proof_assert! { fal(self.free_lists@, ss@) };
        let before = snapshot! { self.free_lists };
        self.free_lists[order].push(current_offset);
        proof_assert! { forall<o: Int> 0 <= o && o < before@.len() && o != order@ ==> self.free_lists@[o] == before@[o] };
        proof_assert! { self.free_lists@[order@]@.len() - before@[order@]@.len() == 1 };
        proof_assert! { pu_rel(before@, self.free_lists@, order@, current_offset) };
        proof_assert! { lemma_pu(before@, self.free_lists@, ss@, order@, current_offset); true };
        g_fm_end(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { order@ }, current_offset,
            snapshot! { old.free_lists@ }, snapshot! { offset@ }, snapshot! { span(ss@, *k0) }); // PROOF-ONLY (ghost)
        proof_assert! { lemma_free_e_intro(old.free_lists@, self.free_lists@, ss@, offset@, span(ss@, *k0));
            free_e(old.free_lists@, self.free_lists@, ss@, offset@, span(ss@, *k0)) };
        proof_assert! { no_merge(*old, offset, *k0) ==> order@ == *k0 && *before == old.free_lists && current_offset == offset };
        proof_assert! { no_merge(*old, offset, *k0) ==>
            same_except(self.free_lists@, old.free_lists@, *k0) && pushed(self.free_lists@[*k0]@, old.free_lists@[*k0]@, offset@) };
        proof_assert! { forall<x: u64> x@ == offset@ ==> x == offset };
        proof_assert! { lemma_tf_inc(self.free_lists@, before@, ss@, order@, span(ss@, order@));
            tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) + span(ss@, order@) };
        proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
            self.free_lists@[o]@[k] == if o == order@ && k == before@[o]@.len() { current_offset } else { before@[o]@[k] } };
    }

    /// Mirror of `BuddyAllocator::mark_allocated` (buddy.rs:117-157).
    ///
    /// Spec: the free-list invariant is preserved, and when neither the exact
    /// block nor any masked candidate `offset & !(span(so) - 1)` is on its list,
    /// NOTHING changes (the silent no-op at buddy.rs:157).
    #[requires(bd_inv(*self))]
    #[requires(abs_offset@ >= self.base_offset@)]
    #[requires(ord(size@ / self.sector_size@) <= self.max_order@)]
    #[ensures(bd_shape(^self))]
    #[ensures(bd_aligned(^self))]
    #[ensures(bd_inside(^self))]
    #[ensures(bd_inv(^self))]
    #[ensures((^self).base_offset == self.base_offset && (^self).total_usable_size == self.total_usable_size)]
    #[ensures((^self).sector_size == self.sector_size && (^self).max_order == self.max_order)]
    #[ensures((^self).free_lists@.len() == self.free_lists@.len())]
    #[ensures((forall<q: Int> 0 <= q && q < self.free_lists@[ord(size@ / self.sector_size@)]@.len() ==>
                self.free_lists@[ord(size@ / self.sector_size@)]@[q]@ != abs_offset@ - self.base_offset@)
        && (forall<sk: Int, sp: u64, x: u64> ord(size@ / self.sector_size@) < sk && sk <= self.max_order@
                && sp@ == span(self.sector_size@, sk) && x@ == abs_offset@ - self.base_offset@ ==>
                forall<q: Int> 0 <= q && q < self.free_lists@[sk]@.len() ==> self.free_lists@[sk]@[q] != (x & !(sp - 1u64)))
        ==> ^self == *self)]
    #[ensures(forall<x: u64, a: u64> x@ == abs_offset@ - self.base_offset@ && split1_pre(*self, x, ord(size@ / self.sector_size@), a) ==>
        split1_post(*self, ^self, x, ord(size@ / self.sector_size@), a))]
    #[ensures(mk_spec(self.free_lists@, (^self).free_lists@, self.sector_size@, abs_offset@ - self.base_offset@, ord(size@ / self.sector_size@)))]
    #[ensures(mk_c(self.free_lists@, (^self).free_lists@, self.sector_size@, abs_offset@ - self.base_offset@, ord(size@ / self.sector_size@)))]
    pub fn mark_allocated(&mut self, abs_offset: u64, size: u64) {
        let ss = self.sector_size as u64;
        let offset = abs_offset - self.base_offset;
        let blocks = size / ss;
        let target_order = order_for_blocks(blocks);
        let hk = snapshot! { mk_hyp(self.free_lists@, ss@, offset@, target_order@) };
        let s0 = snapshot! { *self };
        proof_assert! { ss@ == self.sector_size@ && offset@ == abs_offset@ - self.base_offset@ && target_order@ == ord(size@ / self.sector_size@) };
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss@, o); span(ss@, o) > 0 } };

        if let Some(pos) = find_pos(&self.free_lists[target_order], offset) {
            proof_assert! { self.free_lists@[target_order@]@[pos@]@ == abs_offset@ - self.base_offset@ };
            proof_assert! { forall<x: u64> x@ == offset@ ==> x == offset };
            proof_assert! { self.free_lists@[target_order@]@[pos@] == offset };
            proof_assert! { forall<x: u64, a: u64> x@ == offset@ ==> !split1_pre(*self, x, target_order@, a) };
            let before = snapshot! { self.free_lists };
            swap_remove(&mut self.free_lists[target_order], pos);
            proof_assert! { forall<o: Int> 0 <= o && o < before@.len() && o != target_order@ ==> self.free_lists@[o] == before@[o] };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                exists<j: Int> 0 <= j && j < before@[o]@.len() && self.free_lists@[o]@[k] == before@[o]@[j] };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
            proof_assert! { bd_aligned(*self) && bd_inside(*self) && bd_shape(*self) };
            proof_assert! { sr_rel(before@, self.free_lists@, target_order@, pos@) };
            proof_assert! { self.free_lists@[target_order@]@.len() - before@[target_order@]@.len() == -1 };
            proof_assert! { lemma_tf_dec(self.free_lists@, before@, ss@, target_order@, span(ss@, target_order@));
                tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) - span(ss@, target_order@) };
            proof_assert! { fs(before@, target_order@, pos@) == offset@ };
            g_mk_exact(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { offset@ },
                snapshot! { target_order@ }, snapshot! { pos@ }); // PROOF-ONLY (ghost)
            proof_assert! { before@ == s0.free_lists@ };
            proof_assert! { mk_spec(s0.free_lists@, self.free_lists@, s0.sector_size@, abs_offset@ - s0.base_offset@, ord(size@ / s0.sector_size@)) };
            g_coal_sr(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { target_order@ }, snapshot! { pos@ }); // PROOF-ONLY (ghost)
            g_mk_c_intro(snapshot! { s0.free_lists@ }, snapshot! { self.free_lists@ }, snapshot! { s0.sector_size@ },
                snapshot! { abs_offset@ - s0.base_offset@ }, snapshot! { ord(size@ / s0.sector_size@) }); // PROOF-ONLY (ghost)
            return;
        }

        let orig = snapshot! { *self };
        proof_assert! { forall<q: Int> 0 <= q && q < orig.free_lists@[target_order@]@.len() ==> fs(orig.free_lists@, target_order@, q) != offset@ };
        proof_assert! { *hk ==> { lemma_inblk_up(orig.free_lists@, ss@, target_order@, offset@); true } };
        proof_assert! { forall<x: u64> x@ == offset@ ==> x == offset };
        proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==> target_order@ == ord(size@ / self.sector_size@) };
        let mut search_order = target_order + 1;
        #[invariant(target_order@ + 1 <= search_order@)]
        #[invariant(*self == *orig)]
        #[invariant(forall<sord: Int, sp: u64> target_order@ < sord && sord < search_order@ && sord <= self.max_order@
                && sp@ == span(ss@, sord) ==>
                forall<q: Int> 0 <= q && q < self.free_lists@[sord]@.len() ==> self.free_lists@[sord]@[q] != (offset & !(sp - 1u64)))]
        #[invariant(forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==> search_order@ == target_order@ + 1)]
        #[invariant(*hk ==> exists<o: Int, k: Int> fv(orig.free_lists@, o, k) && o >= search_order@ && fs(orig.free_lists@, o, k) <= offset@
            && offset@ + span(ss@, target_order@) <= fs(orig.free_lists@, o, k) + span(ss@, o))]
        while search_order <= self.max_order {
            proof_assert! { lemma_span_pos(ss@, search_order@); span(ss@, search_order@) > 0 };
            proof_assert! { self.max_order@ >= 1 };
            proof_assert! { span(ss@, self.max_order@) <= self.total_usable_size@ };
            proof_assert! { lemma_span_mono(ss@, search_order@, self.max_order@); span(ss@, search_order@) <= span(ss@, self.max_order@) };
            proof_assert! { search_order@.pow2() * ss@ <= self.total_usable_size@ };
            let block_span = pow2_u64(search_order) * ss;
            proof_assert! { block_span@ == span(ss@, search_order@) };
            let aligned_start = mask_down(offset, block_span);
            proof_assert! { forall<sp: u64> sp@ == span(ss@, search_order@) ==> sp == block_span };
            g_mask(offset, block_span, aligned_start, snapshot! { ss@ }, snapshot! { search_order@ }); // PROOF-ONLY (ghost)
            proof_assert! { *hk ==> p2(ss@) };

            if let Some(pos) = find_pos(&self.free_lists[search_order], aligned_start) {
                proof_assert! { self.free_lists@[search_order@]@[pos@] == (offset & !(block_span - 1u64)) };
                proof_assert! { fs(orig.free_lists@, search_order@, pos@) == aligned_start@ };
                proof_assert! { *hk ==> {
                    lemma_inblk_is(orig.free_lists@, ss@, target_order@, offset@, search_order@, pos@);
                    offset@ + span(ss@, target_order@) <= aligned_start@ + span(ss@, search_order@) } };
                proof_assert! { aligned_start@ % span(ss@, search_order@) == 0 };
                proof_assert! { aligned_start@ + span(ss@, search_order@) <= self.total_usable_size@ };
                let before = snapshot! { self.free_lists };
                swap_remove(&mut self.free_lists[search_order], pos);
                proof_assert! { forall<o: Int> 0 <= o && o < before@.len() && o != search_order@ ==> self.free_lists@[o] == before@[o] };
                proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                    exists<j: Int> 0 <= j && j < before@[o]@.len() && self.free_lists@[o]@[k] == before@[o]@[j] };
                proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                    self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
                proof_assert! { bd_aligned(*self) && bd_inside(*self) };
                proof_assert! { sr_rel(before@, self.free_lists@, search_order@, pos@) };
                proof_assert! { lemma_sr(before@, self.free_lists@, ss@, search_order@, pos@); true };
                proof_assert! { self.free_lists@[search_order@]@.len() - before@[search_order@]@.len() == -1 };
                proof_assert! { lemma_tf_dec(self.free_lists@, before@, ss@, search_order@, span(ss@, search_order@));
                    tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) - span(ss@, search_order@) };
                proof_assert! { *before == orig.free_lists };
                proof_assert! { *hk ==> forall<y: Int, l: Int> 0 < l && faway(orig.free_lists@, ss@, y, l) ==> disj(y, l, aligned_start@, span(ss@, search_order@)) };
                g_coal_sr(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { search_order@ }, snapshot! { pos@ }); // PROOF-ONLY (ghost)
                let cc = snapshot! { *hk && coal(orig.free_lists@, ss@) };
                let ar = snapshot! { self.free_lists };
                proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==>
                    aligned_start == a && pos@ == 0 && ar@[search_order@]@.len() == 0
                    && ar@[target_order@] == orig.free_lists@[target_order@]
                    && (forall<o: Int> 0 <= o && o < ar@.len() && o != search_order@ ==> ar@[o] == orig.free_lists@[o]) };

                let mut split_offset = aligned_start;
                let mut sord = search_order;
                #[invariant(target_order@ <= sord@ && sord@ <= search_order@)]
                #[invariant(self.free_lists@.len() == orig.free_lists@.len())]
                #[invariant(self.base_offset == orig.base_offset && self.total_usable_size == orig.total_usable_size)]
                #[invariant(self.sector_size == orig.sector_size && self.max_order == orig.max_order)]
                #[invariant(bd_aligned(*self) && bd_inside(*self))]
                #[invariant(split_offset@ % span(ss@, sord@) == 0)]
                #[invariant(split_offset@ + span(ss@, sord@) <= self.total_usable_size@)]
                #[invariant(sord@ == search_order@ ==> self.free_lists == *ar && split_offset == aligned_start)]
                #[invariant(forall<a: u64> split1_pre(*orig, offset, target_order@, a) && sord@ == target_order@ ==>
                    pushed(self.free_lists@[target_order@]@, ar@[target_order@]@, split1_val(*orig, offset, target_order@, a))
                    && forall<o: Int> 0 <= o && o < ar@.len() && o != target_order@ ==> self.free_lists@[o] == ar@[o])]
                #[invariant(*hk ==> fdisj(self.free_lists@, ss@) && faway(self.free_lists@, ss@, split_offset@, span(ss@, sord@)))]
                #[invariant(*hk ==> split_offset@ <= offset@ && offset@ + span(ss@, target_order@) <= split_offset@ + span(ss@, sord@))]
                #[invariant(*hk ==> aligned_start@ <= split_offset@ && split_offset@ + span(ss@, sord@) <= aligned_start@ + span(ss@, search_order@))]
                #[invariant(*hk ==> forall<y: Int, l: Int> 0 < l && faway(orig.free_lists@, ss@, y, l) ==> faway(self.free_lists@, ss@, y, l))]
                #[invariant(*hk ==> forall<z: Int> ffree(orig.free_lists@, ss@, z) && !(offset@ <= z && z < offset@ + span(ss@, target_order@)) ==>
                    ffree(self.free_lists@, ss@, z) || (split_offset@ <= z && z < split_offset@ + span(ss@, sord@)))]
                #[invariant(*hk ==> fold_k(self.free_lists@, orig.free_lists@, target_order@))]
                #[invariant(*cc ==> coal(self.free_lists@, ss@))]
                #[invariant(*hk ==> tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(orig.free_lists@, ss@, orig.free_lists@.len()) - span(ss@, sord@))]
                while sord > target_order {
                    let sp_prev = snapshot! { sord@ };
                    proof_assert! { split_offset@ + span(ss@, *sp_prev) <= self.total_usable_size@ };
                    sord -= 1;
                    proof_assert! { split_offset@ + span(ss@, sord@ + 1) <= self.total_usable_size@ };
                    proof_assert! { lemma_span_succ(ss@, sord@); span(ss@, sord@ + 1) == 2 * span(ss@, sord@) };
                    proof_assert! { lemma_span_pos(ss@, sord@); span(ss@, sord@) > 0 };
                    proof_assert! { lemma_span_div(split_offset@, ss@, sord@, sord@ + 1); split_offset@ % span(ss@, sord@) == 0 };
                    proof_assert! { lemma_mod_self(span(ss@, sord@)); span(ss@, sord@) % span(ss@, sord@) == 0 };
                    proof_assert! { lemma_mod_add(split_offset@, span(ss@, sord@), span(ss@, sord@));
                        (split_offset@ + span(ss@, sord@)) % span(ss@, sord@) == 0 };
                    proof_assert! { split_offset@ + 2 * span(ss@, sord@) <= self.total_usable_size@ };
                    proof_assert! { span(ss@, sord@) <= self.total_usable_size@ };
                    proof_assert! { sord@.pow2() * ss@ <= self.total_usable_size@ };
                    proof_assert! { sord@.pow2() * ss@ <= u64::MAX@ };
                    let p2 = pow2_u64(sord);
                    proof_assert! { p2@ * ss@ <= u64::MAX@ };
                    let half_span = p2 * ss;
                    proof_assert! { half_span@ == span(ss@, sord@) };
                    let before = snapshot! { self.free_lists };
                    let so0 = snapshot! { split_offset };
                    proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==>
                        *sp_prev == search_order@ && sord@ == target_order@ && *before == *ar && *so0 == a
                        && half_span@ == span(orig.sector_size@, target_order@) };
                    proof_assert! { *hk ==> {
                        lemma_span_div(split_offset@, ss@, target_order@, sord@);
                        lemma_span_mult(ss@, target_order@, sord@);
                        lemma_span_pos(ss@, target_order@);
                        lemma_mod_add(split_offset@, half_span@, span(ss@, target_order@));
                        (split_offset@ + half_span@) % span(ss@, target_order@) == 0 } };
                    proof_assert! { *hk && offset@ < split_offset@ + half_span@ ==> {
                        lemma_aligned_disj2(offset@, split_offset@ + half_span@, span(ss@, target_order@));
                        offset@ + span(ss@, target_order@) <= split_offset@ + half_span@ } };
                    proof_assert! { *hk ==> {
                        lemma_away_sub(before@, ss@, split_offset@, 2 * half_span@, split_offset@, half_span@);
                        lemma_away_sub(before@, ss@, split_offset@, 2 * half_span@, split_offset@ + half_span@, half_span@);
                        faway(before@, ss@, split_offset@, half_span@) && faway(before@, ss@, split_offset@ + half_span@, half_span@) } };
                    proof_assert! { fal(before@, ss@) };
                    if offset >= split_offset + half_span {
                        self.free_lists[sord].push(split_offset);
                        proof_assert! { pu_rel(before@, self.free_lists@, sord@, split_offset) };
                        g_mk_push(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { sord@ }, split_offset,
                            snapshot! { split_offset@ }, cc); // PROOF-ONLY (ghost)
                        proof_assert! { lemma_pu(before@, self.free_lists@, ss@, sord@, split_offset); true };
                        proof_assert! { lemma_tf_inc(self.free_lists@, before@, ss@, sord@, span(ss@, sord@));
                            tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) + span(ss@, sord@) };
                        proof_assert! { *hk ==> { lemma_fold_trans(orig.free_lists@, before@, self.free_lists@, target_order@);
                            fold_k(self.free_lists@, orig.free_lists@, target_order@) } };
                        proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                            self.free_lists@[o]@[k] == if o == sord@ && k == before@[o]@.len() { split_offset } else { before@[o]@[k] } };
                        proof_assert! { split_offset@ % span(ss@, sord@) == 0 && split_offset@ + span(ss@, sord@) <= self.total_usable_size@ };
                        proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                            self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
                        split_offset += half_span;
                    } else {
                        let up = split_offset + half_span;
                        self.free_lists[sord].push(up);
                        proof_assert! { pu_rel(before@, self.free_lists@, sord@, up) };
                        g_mk_push(snapshot! { before@ }, snapshot! { self.free_lists@ }, snapshot! { ss@ }, snapshot! { sord@ }, up,
                            snapshot! { split_offset@ }, cc); // PROOF-ONLY (ghost)
                        proof_assert! { lemma_pu(before@, self.free_lists@, ss@, sord@, up); true };
                        proof_assert! { lemma_tf_inc(self.free_lists@, before@, ss@, sord@, span(ss@, sord@));
                            tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) + span(ss@, sord@) };
                        proof_assert! { *hk ==> { lemma_fold_trans(orig.free_lists@, before@, self.free_lists@, target_order@);
                            fold_k(self.free_lists@, orig.free_lists@, target_order@) } };
                        proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                            self.free_lists@[o]@[k] == if o == sord@ && k == before@[o]@.len() { up } else { before@[o]@[k] } };
                        proof_assert! { up@ % span(ss@, sord@) == 0 && up@ + span(ss@, sord@) <= self.total_usable_size@ };
                        proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                            self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
                    }
                    proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                        self.free_lists@[o]@[k]@ % span(ss@, o) == 0 && self.free_lists@[o]@[k]@ + span(ss@, o) <= self.total_usable_size@ };
                    proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==>
                        pushed(self.free_lists@[sord@]@, before@[sord@]@, split1_val(*orig, offset, target_order@, a))
                        && forall<o: Int> 0 <= o && o < before@.len() && o != sord@ ==> self.free_lists@[o] == before@[o] };
                }
                proof_assert! { bd_aligned(*self) && bd_inside(*self) && bd_shape(*self) };
                proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==> split1_post(*orig, *self, offset, target_order@, a) };
                proof_assert! { *hk ==> split_offset@ == offset@ };
                proof_assert! { *hk ==> mk_post(orig.free_lists@, self.free_lists@, ss@, offset@, target_order@) };
                proof_assert! { lemma_mk_spec_intro(orig.free_lists@, self.free_lists@, ss@, offset@, target_order@); true };
                proof_assert! { orig.free_lists@ == s0.free_lists@ };
                proof_assert! { mk_spec(s0.free_lists@, self.free_lists@, s0.sector_size@, abs_offset@ - s0.base_offset@, ord(size@ / s0.sector_size@)) };
                proof_assert! { *hk == mk_hyp(s0.free_lists@, s0.sector_size@, abs_offset@ - s0.base_offset@, ord(size@ / s0.sector_size@)) };
                g_mk_c_intro(snapshot! { s0.free_lists@ }, snapshot! { self.free_lists@ }, snapshot! { s0.sector_size@ },
                    snapshot! { abs_offset@ - s0.base_offset@ }, snapshot! { ord(size@ / s0.sector_size@) }); // PROOF-ONLY (ghost)
                return;
            }
            proof_assert! { forall<a: u64> split1_pre(*orig, offset, target_order@, a) ==> search_order@ != target_order@ + 1 };
            proof_assert! { forall<sp: u64> sp@ == span(ss@, search_order@) ==> forall<q: Int> 0 <= q && q < self.free_lists@[search_order@]@.len() ==>
                self.free_lists@[search_order@]@[q] != (offset & !(sp - 1u64)) };
            proof_assert! { forall<sord: Int, sp: u64> target_order@ < sord && sord < search_order@ + 1 && sord <= self.max_order@
                && sp@ == span(ss@, sord) ==>
                forall<q: Int> 0 <= q && q < self.free_lists@[sord]@.len() ==> self.free_lists@[sord]@[q] != (offset & !(sp - 1u64)) };
            proof_assert! { forall<q: Int> 0 <= q && q < orig.free_lists@[search_order@]@.len() ==> fs(orig.free_lists@, search_order@, q) != aligned_start@ };
            proof_assert! { *hk ==> { lemma_inblk_step(orig.free_lists@, ss@, target_order@, search_order@, offset@, aligned_start@); true } };
            search_order += 1;
        }
        proof_assert! { !*hk };
        proof_assert! { lemma_mk_spec_intro(orig.free_lists@, self.free_lists@, ss@, offset@, target_order@); true };
        proof_assert! { orig.free_lists@ == s0.free_lists@ };
        proof_assert! { mk_spec(s0.free_lists@, self.free_lists@, s0.sector_size@, abs_offset@ - s0.base_offset@, ord(size@ / s0.sector_size@)) };
        proof_assert! { *hk == mk_hyp(s0.free_lists@, s0.sector_size@, abs_offset@ - s0.base_offset@, ord(size@ / s0.sector_size@)) };
        g_mk_c_intro(snapshot! { s0.free_lists@ }, snapshot! { self.free_lists@ }, snapshot! { s0.sector_size@ },
            snapshot! { abs_offset@ - s0.base_offset@ }, snapshot! { ord(size@ / s0.sector_size@) }); // PROOF-ONLY (ghost)
    }

    /// Mirror of `BuddyAllocator::total_free` (buddy.rs:159-166).
    #[requires(bd_shape(*self))]
    #[requires(tf(self.free_lists@, self.sector_size@, self.free_lists@.len()) <= u64::MAX@)]
    #[ensures(result@ == tf(self.free_lists@, self.sector_size@, self.free_lists@.len()))]
    pub fn total_free(&self) -> u64 {
        let ss = self.sector_size as u64;
        let mut total = 0u64;
        let mut order: usize = 0;
        proof_assert! { lemma_tf_zero(self.free_lists@, ss@); tf(self.free_lists@, ss@, 0) == 0 };
        proof_assert! { lemma_tf_mono(self.free_lists@, ss@, self.free_lists@.len());
            forall<m: Int> 0 <= m && m <= self.free_lists@.len() ==> 0 <= tf(self.free_lists@, ss@, m) && tf(self.free_lists@, ss@, m) <= tf(self.free_lists@, ss@, self.free_lists@.len()) };
        #[invariant(order@ <= self.free_lists@.len())]
        #[invariant(total@ == tf(self.free_lists@, ss@, order@))]
        while order < self.free_lists.len() {
            proof_assert! { tf(self.free_lists@, ss@, order@ + 1) <= u64::MAX@ };
            proof_assert! { lemma_pow2(order@); lemma_span_pos(ss@, order@); span(ss@, order@) > 0 };
            let len = self.free_lists[order].len() as u64;
            proof_assert! { lemma_mul_le(0, len@, order@.pow2()); len@ * order@.pow2() >= 0 };
            proof_assert! { lemma_mul_le(1, ss@, len@ * order@.pow2()); len@ * order@.pow2() <= (len@ * order@.pow2()) * ss@ };
            proof_assert! { lemma_assoc(len@, order@.pow2(), ss@); (len@ * order@.pow2()) * ss@ <= tf(self.free_lists@, ss@, self.free_lists@.len()) };
            proof_assert! { len@ * order@.pow2() <= u64::MAX@ };
            proof_assert! { (len@ * order@.pow2()) * ss@ <= u64::MAX@ };
            proof_assert! { lemma_assoc(len@, order@.pow2(), ss@); (len@ * order@.pow2()) * ss@ == len@ * span(ss@, order@) };
            proof_assert! { lemma_tf_step(self.free_lists@, ss@, order@ + 1); tf(self.free_lists@, ss@, order@ + 1) == tf(self.free_lists@, ss@, order@) + len@ * span(ss@, order@) };
            let step = len * pow2_u64(order) * ss;
            proof_assert! { step@ == len@ * span(ss@, order@) };
            total += step;
            order += 1;
        }
        total
    }

    /// Mirror of `BuddyAllocator::base_offset` (buddy.rs:48-51).
    #[ensures(result == self.base_offset)]
    pub fn base_offset(&self) -> u64 {
        self.base_offset
    }

    /// Mirror of `BuddyAllocator::total_usable_size` (buddy.rs:53-55).
    #[ensures(result == self.total_usable_size)]
    pub fn total_usable_size(&self) -> u64 {
        self.total_usable_size
    }

    /// Mirror of `BuddyAllocator::alloc` (buddy.rs:57-82). Exact specification:
    /// with `on = ord(ceil(size/ss))` and `fo` the first non-empty order in
    /// `on..=max_order`, the last block of list `fo` is popped, the upper half of
    /// each split pushed onto lists `on..fo`, and the lower order-`on` block returned.
    #[requires(bd_inv(*self))]
    #[requires(size@ + self.sector_size@ <= u64::MAX@)]
    #[ensures(bd_shape(^self))]
    #[ensures(bd_aligned(^self))]
    #[ensures(bd_inside(^self))]
    #[ensures(bd_inv(^self))]
    #[ensures((^self).base_offset == self.base_offset && (^self).total_usable_size == self.total_usable_size)]
    #[ensures((^self).sector_size == self.sector_size && (^self).max_order == self.max_order)]
    #[ensures((^self).free_lists@.len() == self.free_lists@.len())]
    #[ensures(result == None ==> ^self == *self
        && first_ne(self.free_lists@, ord((size@ + self.sector_size@ - 1) / self.sector_size@), self.max_order@) == -1)]
    #[ensures(forall<r: u64> result == Some(r) ==>
        alloc_post(*self, ^self, ord((size@ + self.sector_size@ - 1) / self.sector_size@),
                   first_ne(self.free_lists@, ord((size@ + self.sector_size@ - 1) / self.sector_size@), self.max_order@), r@))]
    #[ensures(forall<r: u64> result == Some(r) ==>
        tf((^self).free_lists@, self.sector_size@, (^self).free_lists@.len())
        == tf(self.free_lists@, self.sector_size@, self.free_lists@.len()) - span(self.sector_size@, ord((size@ + self.sector_size@ - 1) / self.sector_size@)))]
    pub fn alloc(&mut self, size: u64) -> Option<u64> {
        let ss = self.sector_size as u64;
        let blocks_needed = (size + ss - 1) / ss;
        let order_needed = order_for_blocks(blocks_needed);

        let mut found_order = None;
        let mut order = order_needed;
        #[invariant(order_needed@ <= order@)]
        #[invariant(order@ <= self.max_order@ + 1 || order@ == order_needed@)]
        #[invariant(forall<p: Int> order_needed@ <= p && p < order@ ==> self.free_lists@[p]@.len() == 0)]
        #[invariant(found_order == None)]
        while order <= self.max_order {
            if self.free_lists[order].len() != 0 {
                // `!self.free_lists[order].is_empty()` (buddy.rs:68)
                found_order = Some(order);
                break;
            }
            order += 1;
        }
        proof_assert! { lemma_first_ne(self.free_lists@, order_needed@, self.max_order@); true };

        let found_order = found_order?;
        proof_assert! { found_order@ == first_ne(self.free_lists@, order_needed@, self.max_order@) };
        let old = snapshot! { *self };
        proof_assert! { old.free_lists@[found_order@]@.len() > 0 };
        let local_offset = self.free_lists[found_order].pop().unwrap();
        proof_assert! { local_offset == old.free_lists@[found_order@]@[old.free_lists@[found_order@]@.len() - 1] };
        proof_assert! { forall<o: Int> 0 <= o && o < old.free_lists@.len() && o != found_order@ ==> self.free_lists@[o] == old.free_lists@[o] };
        proof_assert! { forall<k: Int> 0 <= k && k < self.free_lists@[found_order@]@.len() ==>
            self.free_lists@[found_order@]@[k] == old.free_lists@[found_order@]@[k] };
        proof_assert! { self.free_lists@[found_order@]@.len() - old.free_lists@[found_order@]@.len() == -1 };
        proof_assert! { lemma_tf_dec(self.free_lists@, old.free_lists@, ss@, found_order@, span(ss@, found_order@));
            tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(old.free_lists@, ss@, old.free_lists@.len()) - span(ss@, found_order@) };
        proof_assert! { local_offset@ % span(ss@, found_order@) == 0 };
        proof_assert! { local_offset@ + span(ss@, found_order@) <= self.total_usable_size@ };
        proof_assert! { lemma_span_div(local_offset@, ss@, order_needed@, found_order@); local_offset@ % span(ss@, order_needed@) == 0 };
        proof_assert! { lemma_pow2(order_needed@); order_needed@.pow2() >= 1 };
        proof_assert! { lemma_pow2_mono(order_needed@, found_order@); order_needed@.pow2() <= found_order@.pow2() };
        proof_assert! { lemma_span_mono(ss@, order_needed@, found_order@); span(ss@, order_needed@) <= span(ss@, found_order@) };

        let mut split_order = found_order;
        #[invariant(order_needed@ <= split_order@ && split_order@ <= found_order@)]
        #[invariant(self.free_lists@.len() == old.free_lists@.len())]
        #[invariant(self.base_offset == old.base_offset && self.total_usable_size == old.total_usable_size)]
        #[invariant(self.sector_size == old.sector_size && self.max_order == old.max_order)]
        #[invariant(self.free_lists@[found_order@]@ == old.free_lists@[found_order@]@.subsequence(0, old.free_lists@[found_order@]@.len() - 1))]
        #[invariant(forall<o: Int> split_order@ <= o && o < found_order@ ==>
            pushed(self.free_lists@[o]@, old.free_lists@[o]@, local_offset@ + span(ss@, o)))]
        #[invariant(forall<o: Int> 0 <= o && o < old.free_lists@.len() && (o < split_order@ || o > found_order@) ==>
            self.free_lists@[o] == old.free_lists@[o])]
        #[invariant(local_offset@ + span(ss@, split_order@) <= self.total_usable_size@)]
        #[invariant(local_offset@ % span(ss@, split_order@) == 0)]
        #[invariant(bd_aligned(*self))]
        #[invariant(bd_inside(*self))]
        #[invariant(tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(old.free_lists@, ss@, old.free_lists@.len()) - span(ss@, split_order@))]
        while split_order > order_needed {
            let so_prev = snapshot! { split_order@ };
            proof_assert! { local_offset@ + span(ss@, *so_prev) <= self.total_usable_size@ };
            split_order -= 1;
            proof_assert! { *so_prev == split_order@ + 1 };
            proof_assert! { local_offset@ + span(ss@, split_order@ + 1) <= self.total_usable_size@ };
            proof_assert! { lemma_span_succ(ss@, split_order@); span(ss@, split_order@ + 1) == 2 * span(ss@, split_order@) };
            proof_assert! { lemma_span_pos(ss@, split_order@); span(ss@, split_order@) > 0 };
            proof_assert! { local_offset@ + 2 * span(ss@, split_order@) <= self.total_usable_size@ };
            proof_assert! { split_order@.pow2() * ss@ <= self.total_usable_size@ };
            proof_assert! { lemma_span_div(local_offset@, ss@, split_order@, split_order@ + 1); local_offset@ % span(ss@, split_order@) == 0 };
            let step = pow2_u64(split_order) * ss;
            proof_assert! { step@ == span(ss@, split_order@) };
            let buddy_offset = local_offset + step; // `local_offset + ((1u64 << split_order) * ss)` (buddy.rs:77)
            proof_assert! { buddy_offset@ == local_offset@ + span(ss@, split_order@) };
            proof_assert! { lemma_mod_self(span(ss@, split_order@)); span(ss@, split_order@) % span(ss@, split_order@) == 0 };
            proof_assert! { lemma_mod_add(local_offset@, span(ss@, split_order@), span(ss@, split_order@)); buddy_offset@ % span(ss@, split_order@) == 0 };
            proof_assert! { buddy_offset@ + span(ss@, split_order@) <= self.total_usable_size@ };
            let before = snapshot! { self.free_lists };
            self.free_lists[split_order].push(buddy_offset);
            proof_assert! { self.free_lists@[split_order@]@ == before@[split_order@]@.push_back(buddy_offset) };
            proof_assert! { forall<o: Int> 0 <= o && o < before@.len() && o != split_order@ ==> self.free_lists@[o] == before@[o] };
            proof_assert! { forall<o: Int, k: Int> 0 <= o && o < self.free_lists@.len() && 0 <= k && k < self.free_lists@[o]@.len() ==>
                self.free_lists@[o]@[k] == if o == split_order@ && k == before@[o]@.len() { buddy_offset } else { before@[o]@[k] } };
            proof_assert! { self.free_lists@[split_order@]@.len() - before@[split_order@]@.len() == 1 };
            proof_assert! { lemma_tf_inc(self.free_lists@, before@, ss@, split_order@, span(ss@, split_order@));
                tf(self.free_lists@, ss@, self.free_lists@.len()) == tf(before@, ss@, before@.len()) + span(ss@, split_order@) };
        }

        let on = snapshot! { order_needed@ };
        let fo = snapshot! { found_order@ };
        proof_assert! { *on == ord((size@ + ss@ - 1) / ss@) };
        proof_assert! { ss@ == self.sector_size@ };
        proof_assert! { good_block(*old, local_offset@, *on) };
        proof_assert! { self.free_lists@[*fo]@ == old.free_lists@[*fo]@.subsequence(0, old.free_lists@[*fo]@.len() - 1) };
        proof_assert! { forall<o: Int> *on <= o && o < *fo ==>
            pushed(self.free_lists@[o]@, old.free_lists@[o]@, local_offset@ + span(old.sector_size@, o)) };
        proof_assert! { forall<o: Int> 0 <= o && o < old.free_lists@.len() && (o < *on || o > *fo) ==>
            self.free_lists@[o] == old.free_lists@[o] };
        let r = self.base_offset + local_offset;
        proof_assert! { alloc_post(*old, *self, *on, *fo, r@) };
        Some(r)
    }
}

/// The exact effect of a successful `alloc`: order `fo`'s last block popped,
/// the split halves pushed on `on..fo`, every other list untouched, and the
/// returned block is the lower order-`on` part of the popped block.
#[logic(open)]
pub fn alloc_post(old: BuddyAllocator, new: BuddyAllocator, on: Int, fo: Int, r: Int) -> bool {
    pearlite! {
        let l = old.free_lists@[fo]@;
        let local = l[l.len() - 1];
        0 <= on && on <= fo && fo <= old.max_order@ && l.len() > 0
        && r == old.base_offset@ + local@
        && good_block(old, local@, on)
        && local@ % span(old.sector_size@, fo) == 0
        && local@ + span(old.sector_size@, fo) <= old.total_usable_size@
        && new.free_lists@[fo]@ == l.subsequence(0, l.len() - 1)
        && (forall<o: Int> on <= o && o < fo ==>
                pushed(new.free_lists@[o]@, old.free_lists@[o]@, local@ + span(old.sector_size@, o)))
        && (forall<o: Int> 0 <= o && o < old.free_lists@.len() && (o < on || o > fo) ==>
                new.free_lists@[o] == old.free_lists@[o])
    }
}

/// `new` is `old` with one element of value `v` appended.
#[logic(open)]
pub fn pushed(new: Seq<u64>, old: Seq<u64>, v: Int) -> bool {
    pearlite! {
        new.len() == old.len() + 1
        && (forall<k: Int> 0 <= k && k < old.len() ==> new[k] == old[k])
        && new[old.len()]@ == v
    }
}

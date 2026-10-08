//! Level-2 (batch A) buddy theory: free-block disjointness, "away" (an interval avoids
//! every free block), free bytes, coverage of the fresh decomposition, and the
//! power-of-two mask fact (buddy.rs:136 under D-RANGE-FR-002-217ace).
//!
//! All predicates are over the free lists `fl` (`Seq<Vec<u64>>`) and the sector size
//! `ss`, so they apply both inside `BuddyAllocator::new` (before the struct exists) and
//! to an allocator's `free_lists`.
use crate::model::buddy::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// `x` is a power of two (std `is_power_of_two`: `x == 2^k` for some k).
#[logic(open)]
pub fn p2(x: Int) -> bool {
    pearlite! { exists<k: Int> 0 <= k && k.pow2() == x }
}

/// Position `(o, k)` is a free block.
#[logic(open)]
pub fn fv(fl: Seq<Vec<u64>>, o: Int, k: Int) -> bool {
    pearlite! { 0 <= o && o < fl.len() && 0 <= k && k < fl[o]@.len() }
}

/// Start (relative to the region base) of the free block at `(o, k)`.
#[logic(open)]
pub fn fs(fl: Seq<Vec<u64>>, o: Int, k: Int) -> Int {
    pearlite! { fl[o]@[k]@ }
}

/// `[x1, x1+l1)` and `[x2, x2+l2)` are disjoint.
#[logic(open)]
pub fn disj(x1: Int, l1: Int, x2: Int, l2: Int) -> bool {
    pearlite! { x1 + l1 <= x2 || x2 + l2 <= x1 }
}

/// EM-BUDDY-FREE-DISJOINT: distinct free blocks never overlap.
#[logic(open)]
pub fn fdisj(fl: Seq<Vec<u64>>, ss: Int) -> bool {
    pearlite! {
        forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(fl, o1, k1) && fv(fl, o2, k2) && (o1 != o2 || k1 != k2) ==>
            disj(fs(fl, o1, k1), span(ss, o1), fs(fl, o2, k2), span(ss, o2))
    }
}

/// The interval `[y, y+l)` overlaps no free block.
#[logic(open)]
pub fn faway(fl: Seq<Vec<u64>>, ss: Int, y: Int, l: Int) -> bool {
    pearlite! { forall<o: Int, k: Int> fv(fl, o, k) ==> disj(y, l, fs(fl, o, k), span(ss, o)) }
}

/// Byte `z` lies in some free block.
#[logic(open)]
pub fn ffree(fl: Seq<Vec<u64>>, ss: Int, z: Int) -> bool {
    pearlite! { exists<o: Int, k: Int> fv(fl, o, k) && fs(fl, o, k) <= z && z < fs(fl, o, k) + span(ss, o) }
}

/// Every free block of `nw` below order `kk` is a free block of `od` (same order, same start).
#[logic(open)]
pub fn fold_k(nw: Seq<Vec<u64>>, od: Seq<Vec<u64>>, kk: Int) -> bool {
    pearlite! {
        forall<o: Int, k: Int> fv(nw, o, k) && o < kk ==> exists<j: Int> fv(od, o, j) && fs(od, o, j) == fs(nw, o, k)
    }
}

/// Some free block of order `>= kk` contains `[t, t + span(kk))`.
#[logic(open)]
pub fn inblk(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int) -> bool {
    pearlite! {
        exists<o: Int, k: Int> fv(fl, o, k) && o >= kk && fs(fl, o, k) <= t && t + span(ss, kk) <= fs(fl, o, k) + span(ss, o)
    }
}

/// Every aligned order-`kk` block below `hi` lies inside one free block of order `>= kk`.
/// (Body private to this module: elsewhere use [`lemma_cov0`], [`lemma_cov_step`], [`lemma_cov_total`].)
#[logic]
pub fn cov(fl: Seq<Vec<u64>>, ss: Int, hi: Int) -> bool {
    pearlite! {
        forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= hi ==> inblk(fl, ss, kk, t)
    }
}

/// `b` is `a` with element `p` of list `o` swap-removed (buddy.rs:104,128,144).
#[logic(open)]
pub fn sr_rel(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, o: Int, p: Int) -> bool {
    pearlite! {
        a.len() == b.len() && 0 <= o && o < a.len() && 0 <= p && p < a[o]@.len()
        && b[o]@.len() == a[o]@.len() - 1
        && (forall<k: Int> 0 <= k && k < b[o]@.len() ==> b[o]@[k] == if k == p { a[o]@[a[o]@.len() - 1] } else { a[o]@[k] })
        && (forall<q: Int> 0 <= q && q < a.len() && q != o ==> b[q] == a[q])
    }
}

/// `b` is `a` with `v` pushed on list `o`.
#[logic(open)]
pub fn pu_rel(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, o: Int, v: u64) -> bool {
    pearlite! {
        a.len() == b.len() && 0 <= o && o < a.len()
        && b[o]@ == a[o]@.push_back(v)
        && (forall<q: Int> 0 <= q && q < a.len() && q != o ==> b[q] == a[q])
    }
}

/// Position of a `b`-entry in `a` after a swap-remove of `(o, p)`.
#[logic(open)]
pub fn srp(a: Seq<Vec<u64>>, o: Int, p: Int, o2: Int, k2: Int) -> Int {
    pearlite! { if o2 == o && k2 == p { a[o]@.len() - 1 } else { k2 } }
}

/// Position of an `a`-entry (other than `(o, p)`) in `b` after the swap-remove.
#[logic(open)]
pub fn srq(a: Seq<Vec<u64>>, o: Int, p: Int, o2: Int, k2: Int) -> Int {
    pearlite! { if o2 == o && k2 == a[o]@.len() - 1 { p } else { k2 } }
}

#[logic]
#[requires(sr_rel(a, b, o, p))]
#[ensures(forall<o2: Int, k2: Int> fv(b, o2, k2) ==>
    fv(a, o2, srp(a, o, p, o2, k2)) && fs(b, o2, k2) == fs(a, o2, srp(a, o, p, o2, k2))
    && (o2 != o || srp(a, o, p, o2, k2) != p))]
#[ensures(forall<o2: Int, k2: Int, o3: Int, k3: Int> fv(b, o2, k2) && fv(b, o3, k3) && (o2 != o3 || k2 != k3) ==>
    (o2 != o3 || srp(a, o, p, o2, k2) != srp(a, o, p, o3, k3)))]
#[ensures(forall<o2: Int, k2: Int> fv(a, o2, k2) && (o2 != o || k2 != p) ==>
    fv(b, o2, srq(a, o, p, o2, k2)) && fs(b, o2, srq(a, o, p, o2, k2)) == fs(a, o2, k2))]
pub fn lemma_sr_pos(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, o: Int, p: Int) {}

/// Swap-remove keeps disjointness and avoidance, frees exactly the removed block.
#[logic]
#[requires(sr_rel(a, b, o, p) && ss > 0)]
#[ensures(fdisj(a, ss) ==> fdisj(b, ss))]
#[ensures(forall<y: Int, l: Int> faway(a, ss, y, l) ==> faway(b, ss, y, l))]
#[ensures(fdisj(a, ss) ==> faway(b, ss, fs(a, o, p), span(ss, o)))]
#[ensures(forall<z: Int> ffree(a, ss, z) && !(fs(a, o, p) <= z && z < fs(a, o, p) + span(ss, o)) ==> ffree(b, ss, z))]
#[ensures(forall<z: Int> ffree(b, ss, z) ==> ffree(a, ss, z))]
#[ensures(forall<kk: Int> fold_k(b, a, kk))]
pub fn lemma_sr(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, p: Int) {
    lemma_sr_pos(a, b, o, p);
    proof_assert! { fdisj(a, ss) ==> forall<o1: Int, k1: Int, o2: Int, k2: Int> fv(b, o1, k1) && fv(b, o2, k2) && (o1 != o2 || k1 != k2) ==>
        fs(b, o1, k1) == fs(a, o1, srp(a, o, p, o1, k1)) && fs(b, o2, k2) == fs(a, o2, srp(a, o, p, o2, k2))
        && disj(fs(a, o1, srp(a, o, p, o1, k1)), span(ss, o1), fs(a, o2, srp(a, o, p, o2, k2)), span(ss, o2)) };
    proof_assert! { fdisj(a, ss) ==> forall<o2: Int, k2: Int> fv(b, o2, k2) ==>
        disj(fs(a, o, p), span(ss, o), fs(a, o2, srp(a, o, p, o2, k2)), span(ss, o2)) };
    proof_assert! { forall<z: Int> ffree(a, ss, z) && !(fs(a, o, p) <= z && z < fs(a, o, p) + span(ss, o)) ==>
        exists<o2: Int, k2: Int> fv(a, o2, k2) && (o2 != o || k2 != p) && fs(a, o2, k2) <= z && z < fs(a, o2, k2) + span(ss, o2) };
}

/// Push keeps disjointness when the pushed block avoids every free block.
#[logic]
#[requires(pu_rel(a, b, o, v) && ss > 0)]
#[ensures(fdisj(a, ss) && faway(a, ss, v@, span(ss, o)) ==> fdisj(b, ss))]
#[ensures(forall<y: Int, l: Int> faway(a, ss, y, l) && disj(y, l, v@, span(ss, o)) ==> faway(b, ss, y, l))]
#[ensures(forall<z: Int> ffree(a, ss, z) ==> ffree(b, ss, z))]
#[ensures(forall<z: Int> v@ <= z && z < v@ + span(ss, o) ==> ffree(b, ss, z))]
#[ensures(forall<z: Int> ffree(b, ss, z) ==> ffree(a, ss, z) || (v@ <= z && z < v@ + span(ss, o)))]
#[ensures(forall<kk: Int> o >= kk ==> fold_k(b, a, kk))]
#[ensures(forall<o2: Int, k2: Int> fv(b, o2, k2) ==> (o2 == o && k2 == a[o]@.len()) || (fv(a, o2, k2) && fs(b, o2, k2) == fs(a, o2, k2)))]
#[ensures(forall<o2: Int, k2: Int> fv(a, o2, k2) ==> fv(b, o2, k2) && fs(b, o2, k2) == fs(a, o2, k2))]
#[ensures(fv(b, o, a[o]@.len()) && fs(b, o, a[o]@.len()) == v@)]
pub fn lemma_pu(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, v: u64) {
    proof_assert! { forall<o2: Int, k2: Int> fv(b, o2, k2) ==> (o2 == o && k2 == a[o]@.len()) || (fv(a, o2, k2) && fs(b, o2, k2) == fs(a, o2, k2)) };
    proof_assert! { fv(b, o, a[o]@.len()) && fs(b, o, a[o]@.len()) == v@ };
}

/// Two adjacent equal halves avoided by an interval: the union is avoided.
#[logic]
#[requires(s > 0 && l > 0)]
#[requires(d == c + s || d == c - s)]
#[requires(disj(y, l, c, s) && disj(y, l, d, s))]
#[ensures(disj(y, l, if c <= d { c } else { d }, 2 * s))]
pub fn lemma_disj_union(y: Int, l: Int, c: Int, d: Int, s: Int) {}

/// `faway` of two adjacent halves gives `faway` of their union.
#[logic]
#[requires(s > 0 && (d == c + s || d == c - s))]
#[requires(faway(fl, ss, c, s) && faway(fl, ss, d, s))]
#[requires(forall<o: Int> 0 <= o ==> span(ss, o) > 0)]
#[ensures(faway(fl, ss, if c <= d { c } else { d }, 2 * s))]
pub fn lemma_away_union(fl: Seq<Vec<u64>>, ss: Int, c: Int, d: Int, s: Int) {
    proof_assert! { forall<o: Int, k: Int> fv(fl, o, k) ==> {
        lemma_disj_union(fs(fl, o, k), span(ss, o), c, d, s);
        disj(fs(fl, o, k), span(ss, o), if c <= d { c } else { d }, 2 * s) } };
}

/// A sub-interval of an avoided interval is avoided.
#[logic]
#[requires(faway(fl, ss, y, l) && y <= y2 && y2 + l2 <= y + l && l2 > 0)]
#[requires(forall<o: Int> 0 <= o ==> span(ss, o) > 0)]
#[ensures(faway(fl, ss, y2, l2))]
pub fn lemma_away_sub(fl: Seq<Vec<u64>>, ss: Int, y: Int, l: Int, y2: Int, l2: Int) {}

/// Laminarity: an aligned order-`a` block meeting an aligned order-`b >= a` block at
/// byte `t` lies inside it.
#[logic]
#[requires(m > 0 && n > 0 && n % m == 0 && t % m == 0 && g % m == 0 && t >= 0 && g >= 0)]
#[requires(g <= t && t < g + n)]
#[ensures(t + m <= g + n)]
pub fn lemma_lam(t: Int, g: Int, m: Int, n: Int) {
    lemma_mod_add(g, n, m);
    if t + m > g + n {
        lemma_aligned_disj2(t, g + n, m)
    }
}

/// Two distinct multiples of `m`: the smaller one is at least `m` below.
#[logic]
#[requires(m > 0 && x >= 0 && y >= 0 && x % m == 0 && y % m == 0 && x < y)]
#[ensures(x + m <= y)]
pub fn lemma_aligned_disj2(x: Int, y: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_divmod(y, m);
    lemma_mul_le(x / m + 1, y / m, m);
    lemma_distrib(x / m, 1, m)
}

/// `span(ss, b)` is a multiple of `span(ss, a)` for `a <= b`.
#[logic]
#[requires(ss > 0 && 0 <= a && a <= b)]
#[ensures(span(ss, b) % span(ss, a) == 0)]
pub fn lemma_span_mult(ss: Int, a: Int, b: Int) {
    lemma_span_pos(ss, b);
    lemma_mod_self(span(ss, b));
    lemma_span_div(span(ss, b), ss, a, b)
}

/// The block containing byte `x` at a power of two `m`: `x - x % m`.
#[logic]
#[requires(m > 0 && g >= 0 && x >= 0 && g % m == 0 && g <= x && x < g + m)]
#[ensures(g == x - x % m)]
pub fn lemma_blk_start(x: Int, g: Int, m: Int) {
    lemma_divmod(x, m);
    lemma_divmod(g, m);
    lemma_mul_le(g / m, x / m, m);
    if g / m < x / m {
        lemma_mul_le(g / m + 1, x / m, m);
        lemma_distrib(g / m, 1, m)
    } else if g / m > x / m {
        lemma_mul_le(x / m + 1, g / m, m);
        lemma_distrib(x / m, 1, m)
    }
}

/// `(a + b).pow2() == a.pow2() * b.pow2()`.
#[logic]
#[requires(0 <= a && 0 <= b)]
#[variant(b)]
#[ensures((a + b).pow2() == a.pow2() * b.pow2())]
pub fn lemma_pow2_add(a: Int, b: Int) {
    if b > 0 {
        lemma_pow2_add(a, b - 1);
        lemma_pow2(a + b - 1);
        lemma_pow2(b - 1)
    } else {
        proof_assert! { 0.pow2() == 1 }
    }
}

/// Integer log2 of a power of two.
#[logic(open)]
#[variant(x)]
pub fn log2(x: Int) -> Int {
    pearlite! { if x <= 1 { 0 } else { 1 + log2(x / 2) } }
}

#[logic]
#[requires(0 <= k)]
#[variant(k)]
#[ensures(log2(k.pow2()) == k)]
pub fn lemma_log2(k: Int) {
    if k > 0 {
        lemma_log2(k - 1);
        lemma_pow2(k - 1);
        proof_assert! { k.pow2() == 2 * (k - 1).pow2() };
        proof_assert! { (k - 1).pow2() >= 1 };
        proof_assert! { k.pow2() / 2 == (k - 1).pow2() }
    } else {
        proof_assert! { 0.pow2() == 1 }
    }
}

/// A power-of-two sector size makes every span a power of two: `span(ss, o) == 2^(o + log2 ss)`.
#[logic]
#[requires(p2(ss) && 0 <= o)]
#[ensures(0 <= log2(ss) && ss == log2(ss).pow2())]
#[ensures(span(ss, o) == (o + log2(ss)).pow2())]
pub fn lemma_span_p2(ss: Int, o: Int) {
    pearlite! {
        proof_assert! { forall<k: Int> 0 <= k && k.pow2() == ss ==> { lemma_log2(k); log2(ss) == k } };
        proof_assert! { 0 <= log2(ss) && ss == log2(ss).pow2() };
        lemma_pow2_add(o, log2(ss))
    }
}

// ------------------------------------------------- the mask fact (buddy.rs:136)

#[bitwise_proof]
#[requires(m < 64u64 && s == 1u64 << m)]
#[ensures(result@ == x@ / s@)]
#[ensures(result == x >> m)]
pub fn bf_div(x: u64, s: u64, m: u64) -> u64 {
    x / s
}

#[bitwise_proof]
#[requires(m < 64u64 && s == 1u64 << m && q@ * s@ <= u64::MAX@)]
#[ensures(result@ == q@ * s@)]
#[ensures(result == q << m)]
pub fn bf_mul(q: u64, s: u64, m: u64) -> u64 {
    q * s
}

#[bitwise_proof]
#[requires(m < 64u64)]
#[ensures((x & !((1u64 << m) - 1u64)) == (x >> m) << m)]
pub fn bf_maskshift(x: u64, m: u64) {}

#[bitwise_proof]
#[requires(m@ < 64 && s@ == m@.pow2())]
#[ensures(s == 1u64 << m)]
pub fn bf_shl(s: u64, m: u64) {}

/// PROOF-ONLY helper (no effect on the mirrored state): for a power-of-two `s = 2^e`,
/// the mask `x & !(s - 1)` of buddy.rs:136 rounds `x` down to a multiple of `s`.
#[ensures(0 <= *e && *e < 64 && s@ == e.pow2() ==> (x & !(s - 1u64))@ == x@ - x@ % s@)]
pub fn mask_fact(x: u64, s: u64, e: Snapshot<Int>) {
    let mut k: u64 = 0;
    #[invariant(k@ <= 63)]
    #[invariant(0 <= *e && *e < 64 && s@ == e.pow2() ==> k@ <= *e)]
    while k < 63 && pow2_u64(k as usize) != s {
        proof_assert! { 0 <= *e && *e < 64 && s@ == e.pow2() ==> k@ != *e };
        k += 1;
    }
    proof_assert! { 0 <= *e && *e < 64 && s@ == e.pow2() && k@ < *e ==> {
        lemma_pow2_strict(k@, *e); k@.pow2() < e.pow2() } };
    let pk = pow2_u64(k as usize);
    if pk == s {
        bf_shl(s, k);
        let q = bf_div(x, s, k);
        proof_assert! { lemma_divmod(x@, s@); q@ * s@ <= x@ };
        let y = bf_mul(q, s, k);
        bf_maskshift(x, k);
        proof_assert! { (x & !(s - 1u64)) == y };
        proof_assert! { lemma_divmod(x@, s@); y@ == x@ - x@ % s@ };
    }
}

// ------------------------------------------------ mark_allocated (buddy.rs:117-157)

/// mark_allocated's level-2 hypothesis: disjoint free lists, a power-of-two sector size,
/// and the aligned order-`kk` block at `x` lies inside one free block of order `>= kk`.
#[logic(open)]
pub fn mk_hyp(fl: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) -> bool {
    pearlite! { fdisj(fl, ss) && p2(ss) && 0 <= x && 0 <= kk && x % span(ss, kk) == 0 && inblk(fl, ss, kk, x) }
}

/// Its exact effect: exactly the block `[x, x + span(kk))` leaves the free space.
#[logic(open)]
pub fn mk_post(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) -> bool {
    pearlite! {
        fdisj(b, ss) && faway(b, ss, x, span(ss, kk))
        && (forall<y: Int, l: Int> 0 < l && faway(a, ss, y, l) ==> faway(b, ss, y, l))
        && (forall<z: Int> ffree(a, ss, z) && !(x <= z && z < x + span(ss, kk)) ==> ffree(b, ss, z))
        && fold_k(b, a, kk)
        && tf(b, ss, b.len()) == tf(a, ss, a.len()) - span(ss, kk)
    }
}

#[logic]
#[requires(fold_k(c, b, kk) && fold_k(b, a, kk))]
#[ensures(fold_k(c, a, kk))]
pub fn lemma_fold_trans(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, c: Seq<Vec<u64>>, kk: Int) {}

/// Two free blocks containing the same byte are the same position.
#[logic]
#[requires(fdisj(fl, ss) && fv(fl, o1, k1) && fv(fl, o2, k2))]
#[requires(fs(fl, o1, k1) <= z && z < fs(fl, o1, k1) + span(ss, o1))]
#[requires(fs(fl, o2, k2) <= z && z < fs(fl, o2, k2) + span(ss, o2))]
#[ensures(o1 == o2 && k1 == k2)]
pub fn lemma_same_blk(fl: Seq<Vec<u64>>, ss: Int, o1: Int, k1: Int, o2: Int, k2: Int, z: Int) {}

/// In a list set with disjoint blocks, an inblk witness containing `[x, x+span(kk))` and a
/// free block at `(o, p)` that contains `x` are the same block.
#[logic]
#[requires(fdisj(fl, ss) && inblk(fl, ss, kk, x) && fv(fl, o, p) && ss > 0 && 0 <= kk)]
#[requires(fs(fl, o, p) <= x && x < fs(fl, o, p) + span(ss, o))]
#[ensures(o >= kk && x + span(ss, kk) <= fs(fl, o, p) + span(ss, o))]
pub fn lemma_inblk_is(fl: Seq<Vec<u64>>, ss: Int, kk: Int, x: Int, o: Int, p: Int) {
    pearlite! {
        lemma_span_pos(ss, kk);
        proof_assert! { forall<o2: Int, k2: Int> fv(fl, o2, k2) && o2 >= kk && fs(fl, o2, k2) <= x && x + span(ss, kk) <= fs(fl, o2, k2) + span(ss, o2) ==> {
            lemma_same_blk(fl, ss, o, p, o2, k2, x); o2 == o && k2 == p } }
    }
}

// ------------------------------------------------ BuddyAllocator::new (buddy.rs:10-46)

/// One aligned block of the decomposition step (see [`lemma_cov_step`]).
#[logic]
#[requires(pu_rel(a, b, o, v) && v@ == off * ss && ss > 0 && 0 <= o && off >= 0)]
#[requires(off % (o + 1).pow2() == 0 && cov(a, ss, off * ss))]
#[requires(0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= off * ss + span(ss, o))]
#[ensures(inblk(b, ss, kk, t))]
pub fn lemma_cov_one(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, off: Int, o: Int, v: u64, kk: Int, t: Int) {
    lemma_pu(a, b, ss, o, v);
    lemma_span_pos(ss, kk);
    lemma_span_pos(ss, o);
    lemma_pow2(o);
    lemma_span_succ(ss, o);
    if t + span(ss, kk) <= off * ss {
        ()
    } else if kk >= o + 1 {
        lemma_mul_mod(off, (o + 1).pow2(), ss);
        lemma_span_mult(ss, o + 1, kk);
        lemma_span_div(t, ss, o + 1, kk);
        lemma_span_pos(ss, o + 1);
        lemma_mod_add(t, span(ss, kk), span(ss, o + 1));
        lemma_mul_le(0, off, ss);
        lemma_aligned_disj2(off * ss, t + span(ss, kk), span(ss, o + 1))
    } else {
        lemma_mul_mod(off, (o + 1).pow2(), ss);
        lemma_mul_le(0, off, ss);
        lemma_span_div(off * ss, ss, kk, o + 1);
        if t < off * ss {
            lemma_aligned_disj2(t, off * ss, span(ss, kk))
        }
    }
}

/// Pushing the next decomposition block `[off*ss, (off + 2^o)*ss)` extends the coverage.
#[logic]
#[requires(pu_rel(a, b, o, v) && v@ == off * ss && ss > 0 && 0 <= o && off >= 0)]
#[requires(off % (o + 1).pow2() == 0 && cov(a, ss, off * ss))]
#[ensures(cov(b, ss, off * ss + span(ss, o)))]
pub fn lemma_cov_step(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, off: Int, o: Int, v: u64) {
    pearlite! {
        proof_assert! { forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= off * ss + span(ss, o) ==> {
            lemma_cov_one(a, b, ss, off, o, v, kk, t); inblk(b, ss, kk, t) } }
    }
}

/// The fresh decomposition covers every aligned block that fits in the usable bytes.
#[logic]
#[requires(cov(fl, ss, (total / ss) * ss) && ss > 0 && total >= 0)]
#[requires(0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= total)]
#[ensures(inblk(fl, ss, kk, t))]
pub fn lemma_cov_total(fl: Seq<Vec<u64>>, ss: Int, total: Int, kk: Int, t: Int) {
    lemma_span_pos(ss, kk);
    lemma_mod_self(ss);
    lemma_span_div(span(ss, kk), ss, 0, kk);
    lemma_mod_self(span(ss, kk));
    lemma_span_div(t, ss, 0, kk);
    lemma_mod_add(t, span(ss, kk), ss);
    lemma_floor_le(t + span(ss, kk), ss, total)
}

/// A multiple `m` of `d` that is `<= n` is `<= (n / d) * d`.
#[logic]
#[requires(d > 0 && m >= 0 && n >= 0 && m % d == 0 && m <= n)]
#[ensures(m <= (n / d) * d)]
pub fn lemma_floor_le(m: Int, d: Int, n: Int) {
    lemma_divmod(m, d);
    lemma_divmod(n, d);
    lemma_mul_le(m / d, n / d, d)
}

#[logic]
#[requires(ss > 0)]
#[ensures(cov(fl, ss, 0))]
pub fn lemma_cov0(fl: Seq<Vec<u64>>, ss: Int) {
    pearlite! { proof_assert! { forall<kk: Int> 0 <= kk ==> { lemma_span_pos(ss, kk); span(ss, kk) > 0 } } }
}

/// One step of BuddyAllocator::new's decomposition loop (buddy.rs:30-37) keeps the free
/// blocks disjoint and extends the coverage.
#[logic]
#[requires(pu_rel(a, b, o, v) && v@ == off * ss && ss > 0 && 0 <= o && off >= 0)]
#[requires(forall<o2: Int, k2: Int> fv(a, o2, k2) ==> fs(a, o2, k2) + span(ss, o2) <= off * ss)]
#[requires(fdisj(a, ss) && off % (o + 1).pow2() == 0 && cov(a, ss, off * ss))]
#[ensures(fdisj(b, ss) && cov(b, ss, off * ss + span(ss, o)))]
pub fn lemma_new_step(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, off: Int, o: Int, v: u64) {
    pearlite! {
        proof_assert! { forall<o2: Int> 0 <= o2 ==> { lemma_span_pos(ss, o2); span(ss, o2) > 0 } };
        proof_assert! { faway(a, ss, v@, span(ss, o)) };
        lemma_pu(a, b, ss, o, v);
        lemma_cov_step(a, b, ss, off, o, v)
    }
}

/// PROOF-ONLY program wrapper of [`lemma_new_step`] (ghost arguments, no effect).
#[requires(pu_rel(*a, *b, *o, v) && v@ == *off * *ss && *ss > 0 && 0 <= *o && *off >= 0)]
#[requires(forall<o2: Int, k2: Int> fv(*a, o2, k2) ==> fs(*a, o2, k2) + span(*ss, o2) <= *off * *ss)]
#[requires(fdisj(*a, *ss) && *off % (*o + 1).pow2() == 0 && cov(*a, *ss, *off * *ss))]
#[ensures(fdisj(*b, *ss) && cov(*b, *ss, *off * *ss + span(*ss, *o)))]
pub fn new_step_g(a: Snapshot<Seq<Vec<u64>>>, b: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, off: Snapshot<Int>, o: Snapshot<Int>, v: u64) {
    proof_assert! { lemma_new_step(*a, *b, *ss, *off, *o, v); true };
}

/// PROOF-ONLY program wrapper of `lemma_acct` (ghost arguments, no effect).
#[requires(tf(*b, *ss, b.len()) == mulw(*off, *ss))]
#[requires(tf(*a, *ss, a.len()) == tf(*b, *ss, b.len()) + mulw(*blocks, *ss))]
#[ensures(tf(*a, *ss, a.len()) == mulw(*off + *blocks, *ss))]
pub fn acct_g(a: Snapshot<Seq<Vec<u64>>>, b: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, off: Snapshot<Int>, blocks: Snapshot<Int>) {
    proof_assert! { lemma_acct(*a, *b, *ss, *off, *blocks); true };
}

// ------------------------------------------------ opaque contract atoms (keep callers' VCs small)

/// BuddyAllocator::free's level-2 facts for the freed interval `[x, x + l0)`:
/// disjointness is kept when the interval avoided every free block, and every interval
/// that avoided the old free blocks and the freed interval avoids the new ones.
/// Opaque outside this module: use [`lemma_free_e_intro`] / [`lemma_free_e_elim`].
#[logic]
pub fn free_e(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, l0: Int) -> bool {
    pearlite! {
        (fdisj(a, ss) && faway(a, ss, x, l0) ==> fdisj(b, ss))
        && (forall<y: Int, l: Int> 0 < l && faway(a, ss, y, l) && disj(y, l, x, l0) ==> faway(b, ss, y, l))
    }
}

#[logic]
#[requires(fdisj(a, ss) && faway(a, ss, x, l0) ==> fdisj(b, ss))]
#[requires(forall<y: Int, l: Int> 0 < l && faway(a, ss, y, l) && disj(y, l, x, l0) ==> faway(b, ss, y, l))]
#[ensures(free_e(a, b, ss, x, l0))]
pub fn lemma_free_e_intro(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, l0: Int) {}

#[logic]
#[requires(free_e(a, b, ss, x, l0))]
#[ensures(fdisj(a, ss) && faway(a, ss, x, l0) ==> fdisj(b, ss))]
#[ensures(forall<y: Int, l: Int> 0 < l && faway(a, ss, y, l) && disj(y, l, x, l0) ==> faway(b, ss, y, l))]
pub fn lemma_free_e_elim(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, l0: Int) {}

/// BuddyAllocator::mark_allocated's level-2 contract (opaque outside this module).
#[logic]
pub fn mk_spec(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) -> bool {
    pearlite! { mk_hyp(a, ss, x, kk) ==> mk_post(a, b, ss, x, kk) }
}

#[logic]
#[requires(mk_hyp(a, ss, x, kk) ==> mk_post(a, b, ss, x, kk))]
#[ensures(mk_spec(a, b, ss, x, kk))]
pub fn lemma_mk_spec_intro(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) {}

#[logic]
#[requires(mk_spec(a, b, ss, x, kk) && mk_hyp(a, ss, x, kk))]
#[ensures(mk_post(a, b, ss, x, kk))]
pub fn lemma_mk_spec_elim(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) {}

/// BuddyAllocator::new's level-2 facts (opaque outside this module).
#[logic]
pub fn new_e(fl: Seq<Vec<u64>>, ss: Int, hi: Int) -> bool {
    pearlite! { fdisj(fl, ss) && cov(fl, ss, hi) }
}

#[logic]
#[requires(fdisj(fl, ss) && cov(fl, ss, hi))]
#[ensures(new_e(fl, ss, hi))]
pub fn lemma_new_e_intro(fl: Seq<Vec<u64>>, ss: Int, hi: Int) {}

#[logic]
#[requires(new_e(fl, ss, hi))]
#[ensures(fdisj(fl, ss) && cov(fl, ss, hi))]
pub fn lemma_new_e_elim(fl: Seq<Vec<u64>>, ss: Int, hi: Int) {}

/// PROOF-ONLY ghost helper: mark_allocated's exact-hit case (buddy.rs:126-131) — the block
/// itself is on its list and is swap-removed.
#[requires(sr_rel(*a, *b, *kk, *p) && fs(*a, *kk, *p) == *x && *ss > 0 && 0 <= *kk)]
#[requires(tf(*b, *ss, b.len()) == tf(*a, *ss, a.len()) - span(*ss, *kk))]
#[ensures(mk_spec(*a, *b, *ss, *x, *kk))]
pub fn g_mk_exact(a: Snapshot<Seq<Vec<u64>>>, b: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, x: Snapshot<Int>, kk: Snapshot<Int>, p: Snapshot<Int>) {
    proof_assert! { lemma_sr(*a, *b, *ss, *kk, *p); true };
    proof_assert! { mk_hyp(*a, *ss, *x, *kk) ==> mk_post(*a, *b, *ss, *x, *kk) };
    proof_assert! { lemma_mk_spec_intro(*a, *b, *ss, *x, *kk); true };
}

/// PROOF-ONLY ghost helper: the mask of buddy.rs:136 for a power-of-two sector size names the
/// aligned block of order `sq` that contains `x`.
#[requires(a == x & !(s - 1u64) && s@ == span(*ss, *sq) && *ss > 0 && 0 <= *sq && s@ > 0)]
#[ensures(p2(*ss) ==> a@ == x@ - x@ % s@ && a@ <= x@ && x@ < a@ + s@ && a@ % s@ == 0 && a@ >= 0)]
pub fn g_mask(x: u64, s: u64, a: u64, ss: Snapshot<Int>, sq: Snapshot<Int>) {
    mask_fact(x, s, snapshot! { *sq + log2(*ss) });
    proof_assert! { 64.pow2() == 18446744073709551616 };
    proof_assert! { p2(*ss) ==> { lemma_span_p2(*ss, *sq); 0 <= *sq + log2(*ss) && s@ == (*sq + log2(*ss)).pow2() } };
    proof_assert! { p2(*ss) ==> (*sq + log2(*ss) >= 64 ==> { lemma_pow2_mono(64, *sq + log2(*ss)); false }) };
    proof_assert! { p2(*ss) ==> { lemma_divmod(x@, s@); a@ <= x@ && x@ < a@ + s@ } };
    proof_assert! { p2(*ss) ==> { lemma_divmod(x@, s@); lemma_mul_mod0(s@, x@ / s@); a@ % s@ == 0 } };
}

/// The containing free block is not the exact block when the exact block is not on its list.
#[logic]
#[requires(inblk(fl, ss, kk, x) && ss > 0 && 0 <= kk && kk < fl.len())]
#[requires(forall<q: Int> 0 <= q && q < fl[kk]@.len() ==> fs(fl, kk, q) != x)]
#[ensures(exists<o: Int, k: Int> fv(fl, o, k) && o >= kk + 1 && fs(fl, o, k) <= x && x + span(ss, kk) <= fs(fl, o, k) + span(ss, o))]
pub fn lemma_inblk_up(fl: Seq<Vec<u64>>, ss: Int, kk: Int, x: Int) {
    pearlite! { lemma_span_pos(ss, kk) }
}

/// Search step (buddy.rs:134-142): if the masked candidate of order `sq` is not free, the
/// containing free block has order `> sq`.
#[logic]
#[requires(ss > 0 && 0 <= kk && kk <= sq && sq < fl.len())]
#[requires(exists<o: Int, k: Int> fv(fl, o, k) && o >= sq && fs(fl, o, k) <= x && x + span(ss, kk) <= fs(fl, o, k) + span(ss, o))]
#[requires(forall<q: Int> 0 <= q && q < fl[sq]@.len() ==> fs(fl, sq, q) != a)]
#[requires(a == x - x % span(ss, sq) && x >= 0)]
#[requires(forall<o: Int, k: Int> fv(fl, o, k) ==> fs(fl, o, k) % span(ss, o) == 0 && fs(fl, o, k) >= 0)]
#[ensures(exists<o: Int, k: Int> fv(fl, o, k) && o >= sq + 1 && fs(fl, o, k) <= x && x + span(ss, kk) <= fs(fl, o, k) + span(ss, o))]
pub fn lemma_inblk_step(fl: Seq<Vec<u64>>, ss: Int, kk: Int, sq: Int, x: Int, a: Int) {
    pearlite! {
        lemma_span_pos(ss, kk);
        lemma_span_pos(ss, sq);
        proof_assert! { forall<k: Int> fv(fl, sq, k) && fs(fl, sq, k) <= x && x + span(ss, kk) <= fs(fl, sq, k) + span(ss, sq) ==> {
            lemma_blk_start(x, fs(fl, sq, k), span(ss, sq)); false } }
    }
}

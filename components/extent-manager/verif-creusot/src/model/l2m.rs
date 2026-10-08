//! Level-2 buddy MERGE theory (EM-BUDDY-MERGE under D-RANGE-FR-002-217ace): the
//! coalescing invariant `coal` (no two free blocks of the same order below the top are
//! buddies), the theorem that an entirely free aligned block lies inside ONE free block
//! (`lemma_allfree_blk`), the power-of-two XOR fact for buddy.rs:96 (`g_xor`), and the
//! step lemmas for `alloc` (from its exact contract `alloc_post`) and `free` (`free_m`).
use crate::model::buddy::*;
use crate::model::l2::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

// ------------------------------------------------------------------ arithmetic

/// `(x + m) / m == x / m + 1` for a multiple `x` of `m`.
#[logic]
#[requires(m > 0 && x >= 0 && x % m == 0)]
#[ensures((x + m) / m == x / m + 1)]
#[ensures((x + m) % m == 0)]
pub fn lemma_div_succ(x: Int, m: Int) {
    lemma_mod_self(m);
    lemma_mod_add(x, m, m);
    lemma_div_sub(x + m, m)
}

/// A multiple of `2h` has an even quotient by `h`.
#[logic]
#[requires(h > 0 && t >= 0 && t % (2 * h) == 0)]
#[ensures((t / h) % 2 == 0)]
#[ensures(t % h == 0)]
pub fn lemma_even_q(t: Int, h: Int) {
    lemma_divmod(t, 2 * h);
    lemma_assoc(h, 2, t / (2 * h));
    lemma_mul_mod0(h, 2 * (t / (2 * h)));
    lemma_mul_mod0(2, t / (2 * h))
}

// ------------------------------------------------------------- the XOR fact (buddy.rs:96)

#[bitwise_proof]
#[requires(m < 64u64 && (a >> m) << m == a)]
#[ensures(((a ^ (1u64 << m)) >> m) << m == (a ^ (1u64 << m)))]
pub fn bf_xor_al(a: u64, m: u64) {}

/// PROOF-ONLY helper (no effect on the mirrored state): for `s = 2^e`, a multiple of `s`
/// XOR `s` is again a multiple of `s`.
#[requires(s@ > 0 && a@ % s@ == 0)]
#[ensures(0 <= *e && *e < 64 && s@ == e.pow2() ==> (a ^ s)@ % s@ == 0)]
pub fn xor_fact(a: u64, s: u64, e: Snapshot<Int>) {
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
        let q = bf_div(a, s, k);
        proof_assert! { lemma_divmod(a@, s@); q@ * s@ == a@ };
        let y = bf_mul(q, s, k);
        proof_assert! { y == a };
        bf_xor_al(a, k);
        let b = a ^ s;
        let qb = bf_div(b, s, k);
        proof_assert! { lemma_divmod(b@, s@); qb@ * s@ <= b@ };
        let yb = bf_mul(qb, s, k);
        proof_assert! { yb == b };
        proof_assert! { lemma_divmod(b@, s@); b@ % s@ == 0 };
    }
}

/// PROOF-ONLY ghost helper for buddy.rs:96: with a power-of-two sector size the XOR of an
/// aligned order-`o` offset with its span is aligned, so `buddy_xor` is the arithmetic buddy.
#[requires(s@ == span(*ss, *o) && *ss > 0 && 0 <= *o && s@ > 0 && a@ % s@ == 0)]
#[ensures(p2(*ss) ==> (a ^ s)@ % s@ == 0)]
pub fn g_xor(a: u64, s: u64, ss: Snapshot<Int>, o: Snapshot<Int>) {
    xor_fact(a, s, snapshot! { *o + log2(*ss) });
    proof_assert! { 64.pow2() == 18446744073709551616 };
    proof_assert! { p2(*ss) ==> { lemma_span_p2(*ss, *o); 0 <= *o + log2(*ss) && s@ == (*o + log2(*ss)).pow2() } };
    proof_assert! { p2(*ss) ==> (*o + log2(*ss) >= 64 ==> { lemma_pow2_mono(64, *o + log2(*ss)); false }) };
}

// ------------------------------------------------------------------ coalescing

/// The buddy of the order-`o` block at `c` (the arithmetic meaning of buddy.rs:96).
#[logic(open)]
pub fn bpart(ss: Int, o: Int, c: Int) -> Int {
    pearlite! { if (c / span(ss, o)) % 2 == 0 { c + span(ss, o) } else { c - span(ss, o) } }
}

/// EM-BUDDY-MERGE's invariant: no two free blocks of the same order below the top order
/// are buddies (a lower half `x` with `x / span` even, and its upper half `x + span`).
/// (Body private to this module: use the lemmas below.)
#[logic]
pub fn coal(fl: Seq<Vec<u64>>, ss: Int) -> bool {
    pearlite! {
        forall<o: Int, k1: Int, k2: Int> fv(fl, o, k1) && fv(fl, o, k2) && o + 1 < fl.len()
            && (fs(fl, o, k1) / span(ss, o)) % 2 == 0 ==> fs(fl, o, k2) != fs(fl, o, k1) + span(ss, o)
    }
}

/// LEVEL-3 (phase D-A): free lists holding at most one block per order are coalesced (no
/// two distinct blocks of one order exist to be buddies).
#[logic]
#[requires(ss > 0)]
#[requires(forall<o: Int> 0 <= o && o < fl.len() ==> fl[o]@.len() <= 1)]
#[ensures(coal(fl, ss))]
pub fn lemma_coal_single(fl: Seq<Vec<u64>>, ss: Int) {
    pearlite! { proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(ss, o); span(ss, o) > 0 } } }
}

/// The buddy of `c` at order `o` is not on list `o` (or `o` is the top order).
#[logic(open)]
pub fn pnl(fl: Seq<Vec<u64>>, ss: Int, o: Int, c: Int) -> bool {
    pearlite! { o + 1 >= fl.len() || forall<q: Int> 0 <= q && q < fl[o]@.len() ==> fl[o]@[q]@ != bpart(ss, o, c) }
}

/// Swap-remove keeps coalescing.
#[logic]
#[requires(sr_rel(a, b, o, p) && coal(a, ss))]
#[ensures(coal(b, ss))]
pub fn lemma_coal_sr(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, p: Int) {
    lemma_sr_pos(a, b, o, p)
}

/// Pushing a block whose buddy is not free keeps coalescing.
#[logic]
#[requires(pu_rel(a, b, o, v) && coal(a, ss) && pnl(a, ss, o, v@) && ss > 0 && 0 <= o && v@ % span(ss, o) == 0)]
#[requires(fal(a, ss))]
#[ensures(coal(b, ss))]
pub fn lemma_coal_pu(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, o: Int, v: u64) {
    pearlite! {
        lemma_span_pos(ss, o);
        lemma_pu(a, b, ss, o, v);
        lemma_div_succ(v@, span(ss, o));
        proof_assert! { forall<k: Int> fv(a, o, k) && o + 1 < a.len() && (fs(a, o, k) / span(ss, o)) % 2 == 0 && fs(a, o, k) + span(ss, o) == v@ ==> {
            lemma_div_succ(fs(a, o, k), span(ss, o)); false } };
        proof_assert! { forall<o2: Int, k1: Int, k2: Int> fv(b, o2, k1) && fv(b, o2, k2) && o2 + 1 < b.len()
            && (fs(b, o2, k1) / span(ss, o2)) % 2 == 0 ==> fs(b, o2, k2) != fs(b, o2, k1) + span(ss, o2) }
    }
}

/// Two free blocks at the same byte are the same position (one-order version).
#[logic]
#[requires(fdisj(fl, ss) && fv(fl, o1, k1) && fv(fl, o2, k2) && ss > 0 && 0 <= o1 && 0 <= o2)]
#[requires(fs(fl, o1, k1) <= z && z < fs(fl, o1, k1) + span(ss, o1))]
#[requires(fs(fl, o2, k2) <= z && z < fs(fl, o2, k2) + span(ss, o2))]
#[ensures(o1 == o2 && k1 == k2)]
pub fn lemma_one_blk(fl: Seq<Vec<u64>>, ss: Int, o1: Int, k1: Int, o2: Int, k2: Int, z: Int) {
    lemma_same_blk(fl, ss, o1, k1, o2, k2, z)
}

/// Every free block starts at a multiple of its own span.
#[logic(open)]
pub fn fal(fl: Seq<Vec<u64>>, ss: Int) -> bool {
    pearlite! { forall<o: Int, k: Int> fv(fl, o, k) ==> fs(fl, o, k) % span(ss, o) == 0 }
}

// Lemmas returning `true` (`*_b`): used as `{ lemma_x_b(..) && P }` inside quantified
// proof_asserts, so that the call term stays in the goal and triggers the lemma's axiom.

#[logic]
#[requires(fv(fl, o, k) && o >= kk && fs(fl, o, k) <= t && t + span(ss, kk) <= fs(fl, o, k) + span(ss, o))]
#[ensures(result && inblk(fl, ss, kk, t))]
pub fn lemma_inblk_intro(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int, o: Int, k: Int) -> bool { true }

#[logic]
#[requires(fv(fl, o, k) && fs(fl, o, k) <= z && z < fs(fl, o, k) + span(ss, o))]
#[ensures(result && ffree(fl, ss, z))]
pub fn lemma_ffree_intro(fl: Seq<Vec<u64>>, ss: Int, z: Int, o: Int, k: Int) -> bool { true }

/// A free block of order `>= kk` that meets the aligned order-`kk` block at `t` contains it.
#[logic]
#[requires(fal(fl, ss) && ss > 0 && fv(fl, o, k) && 0 <= kk && kk <= o)]
#[requires(t >= 0 && t % span(ss, kk) == 0 && t <= y && y < t + span(ss, kk))]
#[requires(fs(fl, o, k) <= y && y < fs(fl, o, k) + span(ss, o))]
#[ensures(result && fs(fl, o, k) <= t && t + span(ss, kk) <= fs(fl, o, k) + span(ss, o) && inblk(fl, ss, kk, t))]
pub fn lemma_ms_hi(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int, o: Int, k: Int, y: Int) -> bool {
    lemma_span_pos(ss, kk);
    lemma_span_mult(ss, kk, o);
    lemma_span_div(fs(fl, o, k), ss, kk, o);
    if fs(fl, o, k) > t {
        lemma_aligned_disj2(t, fs(fl, o, k), span(ss, kk));
        true
    } else {
        lemma_lam(t, fs(fl, o, k), span(ss, kk), span(ss, o));
        lemma_inblk_intro(fl, ss, kk, t, o, k)
    }
}

/// The merge step of [`lemma_allfree_blk`]: two halves each inside one free block give the
/// whole aligned block inside one free block (the halves cannot be two free buddies).
#[logic]
#[requires(coal(fl, ss) && fal(fl, ss) && ss > 0)]
#[requires(0 < kk && kk < fl.len() && t >= 0 && t % span(ss, kk) == 0)]
#[requires(inblk(fl, ss, kk - 1, t) && inblk(fl, ss, kk - 1, t + span(ss, kk - 1)))]
#[ensures(inblk(fl, ss, kk, t))]
pub fn lemma_merge_step(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int) {
    pearlite! {
        lemma_span_succ(ss, kk - 1);
        lemma_span_pos(ss, kk - 1);
        lemma_span_pos(ss, kk);
        proof_assert! { span(ss, kk) == 2 * span(ss, kk - 1) };
        lemma_even_q(t, span(ss, kk - 1));
        proof_assert! { forall<o: Int, k: Int> fv(fl, o, k) && o >= kk && fs(fl, o, k) <= t
            && t + span(ss, kk - 1) <= fs(fl, o, k) + span(ss, o) ==> {
            if lemma_ms_hi(fl, ss, kk, t, o, k, t) { inblk(fl, ss, kk, t) } else { false } } };
        proof_assert! { forall<o: Int, k: Int> fv(fl, o, k) && o >= kk && fs(fl, o, k) <= t + span(ss, kk - 1)
            && t + span(ss, kk - 1) + span(ss, kk - 1) <= fs(fl, o, k) + span(ss, o) ==> {
            if lemma_ms_hi(fl, ss, kk, t, o, k, t + span(ss, kk - 1)) { inblk(fl, ss, kk, t) } else { false } } };
        proof_assert! { forall<k1: Int, k2: Int> fv(fl, kk - 1, k1) && fv(fl, kk - 1, k2)
            && fs(fl, kk - 1, k1) <= t && t + span(ss, kk - 1) <= fs(fl, kk - 1, k1) + span(ss, kk - 1)
            && fs(fl, kk - 1, k2) <= t + span(ss, kk - 1)
            && t + span(ss, kk - 1) + span(ss, kk - 1) <= fs(fl, kk - 1, k2) + span(ss, kk - 1) ==>
            fs(fl, kk - 1, k1) == t && fs(fl, kk - 1, k2) == t + span(ss, kk - 1)
            && (fs(fl, kk - 1, k1) / span(ss, kk - 1)) % 2 == 0 && kk - 1 + 1 < fl.len() };
        proof_assert! { forall<k1: Int, k2: Int> fv(fl, kk - 1, k1) && fv(fl, kk - 1, k2)
            && fs(fl, kk - 1, k1) <= t && t + span(ss, kk - 1) <= fs(fl, kk - 1, k1) + span(ss, kk - 1)
            && fs(fl, kk - 1, k2) <= t + span(ss, kk - 1)
            && t + span(ss, kk - 1) + span(ss, kk - 1) <= fs(fl, kk - 1, k2) + span(ss, kk - 1) ==> false };
    }
}

/// An entirely free aligned block lies inside a single free block (given coalescing).
#[logic]
#[variant(kk)]
#[requires(coal(fl, ss) && fal(fl, ss) && ss > 0)]
#[requires(0 <= kk && kk < fl.len() && t >= 0 && t % span(ss, kk) == 0)]
#[requires(forall<z: Int> t <= z && z < t + span(ss, kk) ==> ffree(fl, ss, z))]
#[ensures(inblk(fl, ss, kk, t))]
pub fn lemma_allfree_blk(fl: Seq<Vec<u64>>, ss: Int, kk: Int, t: Int) {
    pearlite! {
        if kk == 0 {
            lemma_span_pos(ss, 0);
            proof_assert! { ffree(fl, ss, t) };
            proof_assert! { forall<o: Int, k: Int> fv(fl, o, k) && fs(fl, o, k) <= t && t < fs(fl, o, k) + span(ss, o) ==> {
                lemma_span_mult(ss, 0, o); lemma_span_div(fs(fl, o, k), ss, 0, o);
                if lemma_ms_hi(fl, ss, 0, t, o, k, t) { inblk(fl, ss, 0, t) } else { false } } }
        } else {
            lemma_span_succ(ss, kk - 1);
            lemma_span_pos(ss, kk - 1);
            lemma_span_div(t, ss, kk - 1, kk);
            lemma_div_succ(t, span(ss, kk - 1));
            lemma_allfree_blk(fl, ss, kk - 1, t);
            lemma_allfree_blk(fl, ss, kk - 1, t + span(ss, kk - 1));
            lemma_merge_step(fl, ss, kk, t)
        }
    }
}

/// Coalescing follows from the coverage `cov` of the fresh decomposition: two free buddies
/// would both overlap the free block that contains their parent.
#[logic]
#[requires(fdisj(fl, ss) && fal(fl, ss) && ss > 0 && total >= 0)]
#[requires(forall<o: Int, k: Int> fv(fl, o, k) ==> fs(fl, o, k) + span(ss, o) <= total)]
#[requires(forall<kk: Int, t: Int> 0 <= kk && 0 <= t && t % span(ss, kk) == 0 && t + span(ss, kk) <= total ==> inblk(fl, ss, kk, t))]
#[ensures(coal(fl, ss))]
pub fn lemma_cov_coal(fl: Seq<Vec<u64>>, ss: Int, total: Int) {
    pearlite! {
        proof_assert! { forall<o: Int, k1: Int, k2: Int> fv(fl, o, k1) && fv(fl, o, k2) && o + 1 < fl.len()
            && (fs(fl, o, k1) / span(ss, o)) % 2 == 0 && fs(fl, o, k2) == fs(fl, o, k1) + span(ss, o) ==> {
            lemma_span_pos(ss, o);
            lemma_span_succ(ss, o);
            lemma_even_mult(fs(fl, o, k1), span(ss, o));
            proof_assert! { inblk(fl, ss, o + 1, fs(fl, o, k1)) };
            proof_assert! { forall<o2: Int, j: Int> fv(fl, o2, j) && o2 >= o + 1 && fs(fl, o2, j) <= fs(fl, o, k1)
                && fs(fl, o, k1) + span(ss, o + 1) <= fs(fl, o2, j) + span(ss, o2) ==> {
                lemma_same_blk(fl, ss, o, k1, o2, j, fs(fl, o, k1)); false } };
            false } }
    }
}

// --------------------------------------------------------------- alloc (buddy.rs:57-82)

/// A byte of the popped block outside the returned order-`on` part lies in one split half.
#[logic]
#[variant(fo - on)]
#[requires(ss > 0 && 0 <= on && on <= fo)]
#[requires(loc + span(ss, on) <= z && z < loc + span(ss, fo))]
#[ensures(exists<o: Int> on <= o && o < fo && loc + span(ss, o) <= z && z < loc + span(ss, o) + span(ss, o))]
pub fn lemma_split_cover(ss: Int, loc: Int, on: Int, fo: Int, z: Int) {
    if on < fo {
        lemma_span_succ(ss, fo - 1);
        if z < loc + span(ss, fo - 1) {
            lemma_split_cover(ss, loc, on, fo - 1, z)
        }
    }
}

/// A byte of the popped block outside the returned part is free after `alloc`.
#[logic]
#[requires(alloc_post(a, b, on, fo, r) && a.sector_size@ > 0 && bd_shape(a) && b.free_lists@.len() == a.free_lists@.len())]
#[requires(fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, on) <= z
    && z < fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, fo))]
#[ensures(ffree(b.free_lists@, a.sector_size@, z))]
pub fn lemma_split_free(a: BuddyAllocator, b: BuddyAllocator, on: Int, fo: Int, r: Int, z: Int) {
    pearlite! {
        lemma_split_cover(a.sector_size@, fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), on, fo, z);
        proof_assert! { forall<o: Int> on <= o && o < fo
            && fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o) <= z
            && z < fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o) + span(a.sector_size@, o) ==> {
            if lemma_ffree_intro(b.free_lists@, a.sector_size@, z, o, a.free_lists@[o]@.len())
            { ffree(b.free_lists@, a.sector_size@, z) } else { false } } }
    }
}

/// `alloc` keeps coalescing and keeps every free byte outside the returned block free.
#[logic]
#[requires(alloc_post(a, b, on, fo, r) && bd_inv(a) && fdisj(a.free_lists@, a.sector_size@) && coal(a.free_lists@, a.sector_size@))]
#[requires(b.free_lists@.len() == a.free_lists@.len())]
#[ensures(coal(b.free_lists@, a.sector_size@))]
#[ensures(forall<z: Int> ffree(a.free_lists@, a.sector_size@, z)
    && !(r - a.base_offset@ <= z && z < r - a.base_offset@ + span(a.sector_size@, on)) ==> ffree(b.free_lists@, a.sector_size@, z))]
pub fn lemma_alloc_merge(a: BuddyAllocator, b: BuddyAllocator, on: Int, fo: Int, r: Int) {
    pearlite! {
        proof_assert! { forall<o: Int> 0 <= o ==> { lemma_span_pos(a.sector_size@, o); span(a.sector_size@, o) > 0 } };
        proof_assert! { fv(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) };
        proof_assert! { forall<o: Int, k: Int> fv(b.free_lists@, o, k) ==>
            (on <= o && o < fo && k == a.free_lists@[o]@.len()
                && fs(b.free_lists@, o, k) == fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o))
            || (fv(a.free_lists@, o, k) && fs(b.free_lists@, o, k) == fs(a.free_lists@, o, k)
                && (o != fo || k != a.free_lists@[fo]@.len() - 1)) };
        proof_assert! { forall<o: Int> on <= o && o < fo ==> {
            lemma_span_succ(a.sector_size@, o);
            lemma_span_div(fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), a.sector_size@, o + 1, fo);
            lemma_even_q(fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), span(a.sector_size@, o));
            lemma_div_succ(fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1), span(a.sector_size@, o));
            ((fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, o)) / span(a.sector_size@, o)) % 2 == 1 } };
        proof_assert! { forall<o: Int, k: Int> on <= o && o < fo && fv(a.free_lists@, o, k) ==> {
            lemma_span_mono(a.sector_size@, o, fo);
            fs(a.free_lists@, o, k) != fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) } };
        proof_assert! { coal(b.free_lists@, a.sector_size@) };
        proof_assert! { forall<o: Int, k: Int> fv(a.free_lists@, o, k) && (o != fo || k != a.free_lists@[fo]@.len() - 1) ==>
            fv(b.free_lists@, o, k) && fs(b.free_lists@, o, k) == fs(a.free_lists@, o, k) };
        proof_assert! { forall<z: Int> fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, on) <= z
            && z < fs(a.free_lists@, fo, a.free_lists@[fo]@.len() - 1) + span(a.sector_size@, fo) ==> {
            lemma_split_free(a, b, on, fo, r, z);
            ffree(b.free_lists@, a.sector_size@, z) } };
        proof_assert! { forall<z: Int> ffree(a.free_lists@, a.sector_size@, z)
            && !(r - a.base_offset@ <= z && z < r - a.base_offset@ + span(a.sector_size@, on)) ==> ffree(b.free_lists@, a.sector_size@, z) }
    }
}

// ---------------------------------------------------------------- free (buddy.rs:84-115)

/// `free`'s merge facts for the freed interval `[x, x + l0)`: coalescing is kept (power-of-two
/// sector size) and every old free byte and every freed byte is free afterwards.
/// Opaque outside this module: use [`lemma_free_m_elim`].
#[logic]
pub fn free_m(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, l0: Int) -> bool {
    pearlite! {
        (coal(a, ss) && p2(ss) ==> coal(b, ss))
        && forall<z: Int> ffree(a, ss, z) || (x <= z && z < x + l0) ==> ffree(b, ss, z)
    }
}

#[logic]
#[requires(free_m(a, b, ss, x, l0))]
#[ensures(coal(a, ss) && p2(ss) ==> coal(b, ss))]
#[ensures(forall<z: Int> ffree(a, ss, z) ==> ffree(b, ss, z))]
#[ensures(forall<z: Int> x <= z && z < x + l0 ==> ffree(b, ss, z))]
pub fn lemma_free_m_elim(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, l0: Int) {}

/// free()'s merge-loop invariant: every old free byte and every freed byte is free or in the
/// current (growing) block `[c, c + l)`. Opaque outside this module.
#[logic]
pub fn fbi(fl: Seq<Vec<u64>>, ss: Int, c: Int, l: Int, a: Seq<Vec<u64>>, x: Int, l0: Int) -> bool {
    pearlite! {
        forall<z: Int> ffree(a, ss, z) || (x <= z && z < x + l0) ==> ffree(fl, ss, z) || (c <= z && z < c + l)
    }
}

/// PROOF-ONLY ghost helper: free()'s merge-loop invariants hold on entry.
#[ensures(fbi(*a, *ss, *x, *l0, *a, *x, *l0))]
#[ensures(coal(*a, *ss) ==> coal(*a, *ss))]
pub fn g_fm_init(a: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, x: Snapshot<Int>, l0: Snapshot<Int>) {}

/// PROOF-ONLY ghost helper: one merge step of buddy.rs:100-106 (the buddy is swap-removed,
/// the current block grows to the union) keeps free()'s merge-loop invariants.
#[requires(sr_rel(*bf, *md, *o, *p) && fs(*bf, *o, *p) == *d && *s > 0 && *s == span(*ss, *o) && *ss > 0)]
#[requires(*d == *c + *s || *d == *c - *s)]
#[requires(fbi(*bf, *ss, *c, *s, *a, *x, *l0))]
#[requires(coal(*a, *ss) ==> coal(*bf, *ss))]
#[ensures(fbi(*md, *ss, if *c <= *d { *c } else { *d }, 2 * *s, *a, *x, *l0))]
#[ensures(coal(*a, *ss) ==> coal(*md, *ss))]
pub fn g_fm_merge(bf: Snapshot<Seq<Vec<u64>>>, md: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, o: Snapshot<Int>, p: Snapshot<Int>,
    c: Snapshot<Int>, d: Snapshot<Int>, s: Snapshot<Int>, a: Snapshot<Seq<Vec<u64>>>, x: Snapshot<Int>, l0: Snapshot<Int>) {
    proof_assert! { lemma_sr(*bf, *md, *ss, *o, *p); true };
    proof_assert! { coal(*a, *ss) ==> { lemma_coal_sr(*bf, *md, *ss, *o, *p); coal(*md, *ss) } };
    proof_assert! { forall<z: Int> ffree(*bf, *ss, z) ==>
        ffree(*md, *ss, z) || (fs(*bf, *o, *p) <= z && z < fs(*bf, *o, *p) + span(*ss, *o)) };
}

/// PROOF-ONLY ghost helper: the final push of buddy.rs:114 establishes `free_m`.
#[requires(pu_rel(*bf, *af, *o, c) && *ss > 0 && 0 <= *o && c@ % span(*ss, *o) == 0)]
#[requires(fbi(*bf, *ss, c@, span(*ss, *o), *a, *x, *l0))]
#[requires(coal(*a, *ss) ==> coal(*bf, *ss))]
#[requires(p2(*ss) ==> pnl(*bf, *ss, *o, c@))]
#[requires(fal(*bf, *ss))]
#[ensures(free_m(*a, *af, *ss, *x, *l0))]
pub fn g_fm_end(bf: Snapshot<Seq<Vec<u64>>>, af: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, o: Snapshot<Int>, c: u64,
    a: Snapshot<Seq<Vec<u64>>>, x: Snapshot<Int>, l0: Snapshot<Int>) {
    proof_assert! { lemma_pu(*bf, *af, *ss, *o, c); true };
    proof_assert! { coal(*a, *ss) && p2(*ss) ==> { lemma_coal_pu(*bf, *af, *ss, *o, c); coal(*af, *ss) } };
}

// ------------------------------------------------------ mark_allocated (buddy.rs:117-157)

/// mark_allocated's coalescing fact: marking a block that lies inside one free block keeps
/// coalescing. Opaque outside this module: use [`lemma_mk_c_elim`].
#[logic]
pub fn mk_c(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) -> bool {
    pearlite! { mk_hyp(a, ss, x, kk) && coal(a, ss) ==> coal(b, ss) }
}

#[logic]
#[requires(mk_c(a, b, ss, x, kk) && mk_hyp(a, ss, x, kk) && coal(a, ss))]
#[ensures(coal(b, ss))]
pub fn lemma_mk_c_elim(a: Seq<Vec<u64>>, b: Seq<Vec<u64>>, ss: Int, x: Int, kk: Int) {}

/// PROOF-ONLY ghost helper: nothing changed (buddy.rs:157 silent no-op).
#[ensures(mk_c(*a, *a, *ss, *x, *kk))]
pub fn g_mk_c_refl(a: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, x: Snapshot<Int>, kk: Snapshot<Int>) {}

/// PROOF-ONLY ghost helper: `mk_c` from its defining implication.
#[requires(mk_hyp(*a, *ss, *x, *kk) && coal(*a, *ss) ==> coal(*b, *ss))]
#[ensures(mk_c(*a, *b, *ss, *x, *kk))]
pub fn g_mk_c_intro(a: Snapshot<Seq<Vec<u64>>>, b: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, x: Snapshot<Int>, kk: Snapshot<Int>) {}

/// PROOF-ONLY ghost helper: a swap-remove keeps coalescing (buddy.rs:128, 144).
#[requires(sr_rel(*a, *b, *o, *p))]
#[ensures(coal(*a, *ss) ==> coal(*b, *ss))]
pub fn g_coal_sr(a: Snapshot<Seq<Vec<u64>>>, b: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, o: Snapshot<Int>, p: Snapshot<Int>) {
    proof_assert! { coal(*a, *ss) ==> { lemma_coal_sr(*a, *b, *ss, *o, *p); coal(*b, *ss) } };
}

/// PROOF-ONLY ghost helper: one split push of buddy.rs:147-152. The pushed half `v` of the
/// order-`o + 1` block at `sp` has the other half as buddy; that half avoids every free
/// block (it holds the marked block), so it is not on list `o` and coalescing is kept.
#[requires(pu_rel(*bf, *af, *o, v) && *ss > 0 && 0 <= *o && fal(*bf, *ss))]
#[requires(*sp >= 0 && *sp % span(*ss, *o + 1) == 0)]
#[requires(v@ == *sp || v@ == *sp + span(*ss, *o))]
#[requires(*c ==> faway(*bf, *ss, if v@ == *sp { *sp + span(*ss, *o) } else { *sp }, span(*ss, *o)) && coal(*bf, *ss))]
#[ensures(*c ==> coal(*af, *ss))]
pub fn g_mk_push(bf: Snapshot<Seq<Vec<u64>>>, af: Snapshot<Seq<Vec<u64>>>, ss: Snapshot<Int>, o: Snapshot<Int>, v: u64,
    sp: Snapshot<Int>, c: Snapshot<bool>) {
    proof_assert! { lemma_span_pos(*ss, *o); lemma_span_succ(*ss, *o); span(*ss, *o) > 0 && span(*ss, *o + 1) == 2 * span(*ss, *o) };
    proof_assert! { lemma_even_q(*sp, span(*ss, *o)); (*sp / span(*ss, *o)) % 2 == 0 && *sp % span(*ss, *o) == 0 };
    proof_assert! { lemma_div_succ(*sp, span(*ss, *o)); (*sp + span(*ss, *o)) / span(*ss, *o) == *sp / span(*ss, *o) + 1
        && (*sp + span(*ss, *o)) % span(*ss, *o) == 0 };
    proof_assert! { bpart(*ss, *o, v@) == if v@ == *sp { *sp + span(*ss, *o) } else { *sp } };
    proof_assert! { v@ % span(*ss, *o) == 0 };
    proof_assert! { *c ==> forall<q: Int> 0 <= q && q < bf[*o]@.len() ==> fv(*bf, *o, q) && bf[*o]@[q]@ != bpart(*ss, *o, v@) };
    proof_assert! { *c ==> pnl(*bf, *ss, *o, v@) };
    proof_assert! { *c ==> { lemma_coal_pu(*bf, *af, *ss, *o, v); coal(*af, *ss) } };
}

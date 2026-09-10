//! Global invariant **EM-USED-LE-CAP** (rank 6, attachments 2) plus the two
//! accessor definitions it composes with:
//!  * **EM-USEDBYTES-BUDDY-GRANULARITY** (FR-031): per region,
//!    `used = total_usable_size - total_free` (lib.rs:704);
//!  * **EM-CAPACITY-USABLE** (FR-032): per region, `cap = total_usable_size`
//!    (lib.rs:713);
//!  * **EM-USED-LE-CAP** (code-only, derived): `used_bytes() <= capacity_bytes()`
//!    always, because `total_free <= total_usable_size` in every region.
//!
//! Model: each region is `(usable, free)` with the buddy invariant
//! `free <= usable`. `used_bytes`/`capacity_bytes` sum over regions (lib.rs:700-714).
//! We prove the aggregate `used <= cap` rigorously with recursive sum specs, an
//! induction lemma, and a single overflow bound (total usable fits in u64).

use creusot_std::prelude::*;

/// One region's buddy accounting. Invariant `free <= usable` is maintained by
/// the buddy allocator (`total_free` never exceeds `total_usable_size`).
pub struct Region {
    pub usable: u64,
    pub free: u64,
}

#[logic(open)]
pub fn wf(r: Region) -> bool {
    pearlite! { r.free@ <= r.usable@ }
}

/// Σ usable over the prefix `regs[0..n]` (mirrors `capacity_bytes`, lib.rs:713).
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= regs.len())]
pub fn sum_usable(regs: Seq<Region>, n: Int) -> Int {
    pearlite! {
        if n == 0 { 0 } else { sum_usable(regs, n - 1) + regs[n - 1].usable@ }
    }
}

/// Σ (usable - free) over the prefix (mirrors `used_bytes`, lib.rs:704).
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= regs.len())]
pub fn sum_used(regs: Seq<Region>, n: Int) -> Int {
    pearlite! {
        if n == 0 { 0 } else { sum_used(regs, n - 1) + (regs[n - 1].usable@ - regs[n - 1].free@) }
    }
}

/// Lemma: when every region is wf (`free <= usable`), `sum_used <= sum_usable`
/// on every prefix — the heart of EM-USED-LE-CAP. Proved by induction on n.
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= regs.len())]
#[requires(forall<i: Int> 0 <= i && i < regs.len() ==> wf(regs[i]))]
#[ensures(sum_used(regs, n) <= sum_usable(regs, n))]
#[ensures(0 <= sum_used(regs, n))]
pub fn lemma_used_le_usable(regs: Seq<Region>, n: Int) {
    pearlite! {
        if n == 0 {
        } else {
            lemma_used_le_usable(regs, n - 1);
        }
    }
}

/// Lemma: sum_usable is monotone in the prefix length (so a prefix sum never
/// exceeds the total — needed to discharge the accumulator's overflow VC).
#[logic]
#[variant(regs.len() - n)]
#[requires(0 <= n && n <= regs.len())]
#[ensures(sum_usable(regs, n) <= sum_usable(regs, regs.len()))]
pub fn lemma_sum_usable_mono(regs: Seq<Region>, n: Int) {
    pearlite! {
        if n == regs.len() {
        } else {
            lemma_sum_usable_mono(regs, n + 1);
        }
    }
}

/// **EM-CAPACITY-USABLE** aggregate: `capacity_bytes = Σ usable`.
#[requires(forall<i: Int> 0 <= i && i < regs@.len() ==> wf(regs@[i]))]
#[requires(sum_usable(regs@, regs@.len()) <= u64::MAX@)]
#[ensures(result@ == sum_usable(regs@, regs@.len()))]
pub fn capacity_bytes(regs: &Vec<Region>) -> u64 {
    let mut cap = 0u64;
    let mut i = 0usize;
    let n = regs.len();
    #[invariant(i@ <= n@)]
    #[invariant(cap@ == sum_usable(regs@, i@))]
    while i < n {
        proof_assert!(lemma_sum_usable_mono(regs@, i@ + 1); true);
        cap += regs[i].usable;
        i += 1;
    }
    cap
}

/// **EM-USEDBYTES-BUDDY-GRANULARITY** + **EM-USED-LE-CAP** aggregate:
/// `used_bytes = Σ (usable - free)`, and it is `<= capacity_bytes`.
#[requires(forall<i: Int> 0 <= i && i < regs@.len() ==> wf(regs@[i]))]
#[requires(sum_usable(regs@, regs@.len()) <= u64::MAX@)]
#[ensures(result@ == sum_used(regs@, regs@.len()))]
#[ensures(result@ <= sum_usable(regs@, regs@.len()))]
pub fn used_bytes(regs: &Vec<Region>) -> u64 {
    proof_assert!(lemma_used_le_usable(regs@, regs@.len()); true);
    let mut used = 0u64;
    let mut i = 0usize;
    let n = regs.len();
    #[invariant(i@ <= n@)]
    #[invariant(used@ == sum_used(regs@, i@))]
    #[invariant(used@ <= sum_usable(regs@, i@))]
    while i < n {
        proof_assert!(lemma_used_le_usable(regs@, i@ + 1); true);
        proof_assert!(lemma_sum_usable_mono(regs@, i@ + 1); true);
        // regs[i].free <= regs[i].usable (wf), so the per-region used fits.
        used += regs[i].usable - regs[i].free;
        i += 1;
    }
    used
}

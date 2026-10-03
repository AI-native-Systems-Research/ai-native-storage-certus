#![recursion_limit = "4096"]
//! Creusot proof crate for `components/memory-tier` (pin d14dfb9a).
//!
//! The shipped crate cannot be translated by Creusot: it depends on
//! `component_framework::define_component!`, `libc::mmap`, `*mut u8`, `RwLock`
//! and `AtomicBool` — and `creusot-std` ships **no** model for `RwLock`/`Mutex`
//! and no mutation specs for `HashMap`/`BTreeMap`. So this crate is a *faithful
//! whole-function mirror* of the functions under proof, with the mirror choices
//! disclosed per property in `verif/creusot_advisory.yaml`.
//!
//! Mirror decisions (all disclosed):
//!  * `*mut u8` pool base  -> `usize` handle (`0` == null).
//!  * `BTreeMap<usize,usize>` free regions -> `Vec<Region>` kept sorted by
//!    offset; `BTreeMap::iter()` order == index order under `sorted_by_offset`.
//!  * `HashMap<CacheKey,Slot>` -> `Vec<SlotModel>` (program Seq mirror).
//!  * `RwLock` poisoning -> an explicit `poisoned: bool` plus one `#[trusted]`
//!    leaf (`lock_ok`) carrying std's documented poisoning semantics.
//!  * `mmap`/`spdk_zmalloc(.., 4096, ..)` -> one `#[trusted]` leaf
//!    (`alloc_pool`) whose contract is the OS page-alignment guarantee.
#![allow(clippy::needless_range_loop)]

use creusot_std::prelude::*;

/// `allocator.rs:5` — `const ALIGNMENT: usize = 4096;`
pub const ALIGNMENT: usize = 4096;

// ===========================================================================
// 1. Rounding (allocator.rs:42 / :60 — `size.next_multiple_of(ALIGNMENT)`)
// ===========================================================================

/// Logical counterpart of `usize::next_multiple_of(4096)` for a positive input.
#[logic(open)]
pub fn align_up_l(size: Int) -> Int {
    pearlite! { ((size - 1) / 4096 + 1) * 4096 }
}

/// Mirror of `size.next_multiple_of(ALIGNMENT)` (allocator.rs:42).
/// Proved natively — no trust: `next_multiple_of` on a positive `size` is
/// exactly this expression, and the `#[requires]` is the condition under which
/// `next_multiple_of` does not panic.
#[requires(size@ > 0)]
#[requires(size@ + 4095 <= usize::MAX@)]
#[ensures(result@ == align_up_l(size@))]
#[ensures(result@ % 4096 == 0)]
#[ensures(result@ >= size@)]
#[ensures(result@ < size@ + 4096)]
pub fn align_up(size: usize) -> usize {
    ((size - 1) / ALIGNMENT + 1) * ALIGNMENT
}

// ===========================================================================
// 2. FreeList mirror (allocator.rs:8-83)
// ===========================================================================

/// One free region: `BTreeMap` entry `offset -> size`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Region {
    pub offset: usize,
    pub size: usize,
}

/// Mirror of `allocator::FreeList` (`allocator.rs:8`).
pub struct FreeListModel {
    /// `free_regions: BTreeMap<usize, usize>` as a Vec sorted by offset.
    pub regions: Vec<Region>,
    pub capacity: usize,
    pub used: usize,
}

/// Every free-region start offset is 4 KiB aligned.
#[logic(open)]
pub fn offsets_aligned(r: Seq<Region>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < r.len() ==> r[i].offset@ % 4096 == 0 }
}

/// The Vec mirror reproduces `BTreeMap::iter()` order.
#[logic(open)]
pub fn sorted_by_offset(r: Seq<Region>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < r.len() ==> r[i].offset@ < r[j].offset@
    }
}

/// The real structural invariant of a `BTreeMap`-backed free list: regions are
/// non-empty, pairwise disjoint and in ascending offset order. This is what makes
/// `iter().find(..)` a first-fit scan and what `allocate` must preserve.
#[logic(open)]
pub fn disjoint_sorted(r: Seq<Region>) -> bool {
    pearlite! {
        (forall<i: Int> 0 <= i && i < r.len() ==> r[i].size@ > 0)
        && (forall<i: Int> 0 <= i && i + 1 < r.len()
            ==> r[i].offset@ + r[i].size@ <= r[i + 1].offset@)
    }
}

/// No single free run is large enough for `need` bytes (the fragmentation case).
#[logic(open)]
pub fn no_region_fits(r: Seq<Region>, need: Int) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < r.len() ==> r[i].size@ < need }
}

// ---------------------------------------------------------------------------
// 2a. Accounting core (added by batch 1): sums over the free runs, with the
//     inductive lemmas every mutator needs. `free_sum` is the total size of
//     the free runs; the composite accounting invariant is
//     `free_sum(regions) + used == capacity` (see `acct_ok`).
// ---------------------------------------------------------------------------

/// Total size of the free runs.
#[logic(open)]
#[variant(r.len())]
pub fn free_sum(r: Seq<Region>) -> Int {
    pearlite! {
        if r.len() <= 0 { 0 }
        else { free_sum(r.subsequence(0, r.len() - 1)) + r[r.len() - 1].size@ }
    }
}

/// `free_sum` distributes over concatenation (induction on `b`).
#[logic]
#[variant(b.len())]
#[ensures(free_sum(a.concat(b)) == free_sum(a) + free_sum(b))]
pub fn lem_fs_concat(a: Seq<Region>, b: Seq<Region>) {
    pearlite! {
        if b.len() <= 0 {
            proof_assert!(a.concat(b) == a); ()
        } else {
            proof_assert!(a.concat(b).subsequence(0, a.len() + b.len() - 1)
                          == a.concat(b.subsequence(0, b.len() - 1)));
            lem_fs_concat(a, b.subsequence(0, b.len() - 1))
        }
    }
}

/// Splitting a sequence at `i` around its `i`-th element.
#[logic]
#[requires(0 <= i && i < r.len())]
#[ensures(free_sum(r) == free_sum(r.subsequence(0, i)) + r[i].size@ + free_sum(r.subsequence(i + 1, r.len())))]
pub fn lem_fs_split(r: Seq<Region>, i: Int) {
    pearlite! {
        proof_assert!(r == r.subsequence(0, i).concat(Seq::singleton(r[i]).concat(r.subsequence(i + 1, r.len()))));
        lem_fs_concat(Seq::singleton(r[i]), r.subsequence(i + 1, r.len()));
        lem_fs_concat(r.subsequence(0, i), Seq::singleton(r[i]).concat(r.subsequence(i + 1, r.len())));
        proof_assert!(free_sum(Seq::singleton(r[i])) == r[i].size@);
        ()
    }
}

/// Removing the `i`-th run lowers the total by its size.
#[logic]
#[requires(0 <= i && i < r.len())]
#[ensures(free_sum(r.removed(i)) == free_sum(r) - r[i].size@)]
pub fn lem_fs_removed(r: Seq<Region>, i: Int) {
    pearlite! {
        lem_fs_split(r, i);
        lem_fs_concat(r.subsequence(0, i), r.subsequence(i + 1, r.len()));
        ()
    }
}

/// Replacing the `i`-th run changes the total by the size difference.
#[logic]
#[requires(0 <= i && i < r.len())]
#[ensures(free_sum(r.set(i, x)) == free_sum(r) - r[i].size@ + x.size@)]
pub fn lem_fs_set(r: Seq<Region>, i: Int, x: Region) {
    pearlite! {
        lem_fs_split(r, i);
        lem_fs_split(r.set(i, x), i);
        proof_assert!(r.set(i, x).subsequence(0, i) == r.subsequence(0, i));
        proof_assert!(r.set(i, x).subsequence(i + 1, r.len()) == r.subsequence(i + 1, r.len()));
        ()
    }
}

/// Inserting a run at position `q` raises the total by its size.
#[logic]
#[requires(0 <= q && q <= r.len())]
#[requires(s.len() == r.len() + 1)]
#[requires(forall<j: Int> 0 <= j && j < q ==> s[j] == r[j])]
#[requires(s[q] == x)]
#[requires(forall<j: Int> q < j && j < s.len() ==> s[j] == r[j - 1])]
#[ensures(free_sum(s) == free_sum(r) + x.size@)]
pub fn lem_fs_inserted(r: Seq<Region>, s: Seq<Region>, q: Int, x: Region) {
    pearlite! {
        lem_fs_split(s, q);
        proof_assert!(s.subsequence(0, q) == r.subsequence(0, q));
        proof_assert!(s.subsequence(q + 1, s.len()) == r.subsequence(q, r.len()));
        proof_assert!(r == r.subsequence(0, q).concat(r.subsequence(q, r.len())));
        lem_fs_concat(r.subsequence(0, q), r.subsequence(q, r.len()));
        ()
    }
}

/// With positive sizes the total bounds every run and is non-negative.
#[logic]
#[variant(r.len())]
#[requires(forall<j: Int> 0 <= j && j < r.len() ==> r[j].size@ > 0)]
#[ensures(free_sum(r) >= 0)]
#[ensures(forall<j: Int> 0 <= j && j < r.len() ==> r[j].size@ <= free_sum(r))]
pub fn lem_fs_bound(r: Seq<Region>) {
    pearlite! {
        if r.len() <= 0 { () } else { lem_fs_bound(r.subsequence(0, r.len() - 1)) }
    }
}

/// Interval `[a0, a1)` is disjoint from interval `[b0, b1)`.
#[logic(open)]
pub fn disj(a0: Int, a1: Int, b0: Int, b1: Int) -> bool {
    pearlite! { a1 <= b0 || b1 <= a0 }
}

/// `[x0, x1)` overlaps no free run.
#[logic(open)]
pub fn clear_of(r: Seq<Region>, x0: Int, x1: Int) -> bool {
    pearlite! {
        forall<j: Int> 0 <= j && j < r.len()
            ==> disj(x0, x1, r[j].offset@, r[j].offset@ + r[j].size@)
    }
}

/// All-pairs form of `disjoint_sorted` (free runs pairwise disjoint, ascending).
#[logic(open)]
pub fn disj_all(r: Seq<Region>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < r.len()
            ==> r[i].offset@ + r[i].size@ <= r[j].offset@
    }
}

/// Every free run lies inside `[0, capacity)`.
#[logic(open)]
pub fn in_cap(r: Seq<Region>, capacity: Int) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < r.len() ==> r[i].offset@ + r[i].size@ <= capacity }
}

/// Some single free run is large enough for `need` bytes.
#[logic(open)]
pub fn some_region_fits(r: Seq<Region>, need: Int) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < r.len() && r[i].size@ >= need }
}

/// Batch 2 (MT-INV-FREE-LIST-COALESCING): no two free runs TOUCH -- consecutive
/// runs are separated by at least one allocated byte. This is what coalescing on
/// release (allocator.rs:67-82) maintains; it is part of `fl_inv`.
#[logic(open)]
pub fn no_adj(r: Seq<Region>) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i + 1 < r.len() ==> r[i].offset@ + r[i].size@ < r[i + 1].offset@
    }
}

/// Removing a run keeps the runs separated.
#[logic]
#[requires(0 <= i && i < r.len())]
#[ensures(no_adj(r) ==> no_adj(r.removed(i)))]
pub fn lem_noadj_removed(r: Seq<Region>, i: Int) {
    pearlite! {
        proof_assert!(forall<j: Int> 0 <= j && j < i ==> r.removed(i)[j] == r[j]);
        proof_assert!(forall<j: Int> i <= j && j < r.len() - 1 ==> r.removed(i)[j] == r[j + 1]);
        proof_assert!(no_adj(r) && i > 0 && i + 1 < r.len()
                      ==> r[i - 1].offset@ + r[i - 1].size@ < r[i + 1].offset@);
        ()
    }
}

/// Inserting `fin[q]` between two runs it is separated from keeps the runs separated.
#[logic]
#[requires(0 <= q && q <= r.len() && fin.len() == r.len() + 1)]
#[requires(forall<j: Int> 0 <= j && j < q ==> fin[j] == r[j])]
#[requires(forall<j: Int> q < j && j < fin.len() ==> fin[j] == r[j - 1])]
#[requires(no_adj(r))]
#[requires(q > 0 ==> r[q - 1].offset@ + r[q - 1].size@ < fin[q].offset@)]
#[requires(q < r.len() ==> fin[q].offset@ + fin[q].size@ < r[q].offset@)]
#[ensures(no_adj(fin))]
pub fn lem_noadj_inserted(r: Seq<Region>, fin: Seq<Region>, q: Int) {
    pearlite! {
        proof_assert!(forall<i: Int> 0 <= i && i + 1 < q ==> fin[i] == r[i] && fin[i + 1] == r[i + 1]);
        proof_assert!(forall<i: Int> q < i && i + 1 < fin.len() ==> fin[i] == r[i - 1] && fin[i + 1] == r[i]);
        ()
    }
}

/// Inserting `fin[q]` keeps every interval clear that was clear of `r` and of `fin[q]`.
#[logic]
#[requires(0 <= q && q <= r.len() && fin.len() == r.len() + 1)]
#[requires(forall<j: Int> 0 <= j && j < q ==> fin[j] == r[j])]
#[requires(forall<j: Int> q < j && j < fin.len() ==> fin[j] == r[j - 1])]
#[ensures(forall<x0: Int, x1: Int> clear_of(r, x0, x1)
          && disj(x0, x1, fin[q].offset@, fin[q].offset@ + fin[q].size@) ==> clear_of(fin, x0, x1))]
pub fn lem_clear_inserted(r: Seq<Region>, fin: Seq<Region>, q: Int) {}

/// `[x0, x1)` lies inside ONE free run.
#[logic(open)]
pub fn covered(r: Seq<Region>, x0: Int, x1: Int) -> bool {
    pearlite! {
        exists<k: Int> 0 <= k && k < r.len() && r[k].offset@ <= x0 && x1 <= r[k].offset@ + r[k].size@
    }
}

/// Mirror of `FreeList::new` (`allocator.rs:16-26`) -- faithful to the
/// `if capacity > 0` guard, so `FreeList::new(0)` (used by `Default`) holds no run.
#[ensures(result.capacity@ == capacity@)]
#[ensures(result.used@ == 0)]
#[ensures(capacity@ > 0 ==> result.regions@.len() == 1)]
#[ensures(capacity@ > 0 ==> result.regions@[0].offset@ == 0)]
#[ensures(capacity@ > 0 ==> result.regions@[0].size@ == capacity@)]
#[ensures(capacity@ == 0 ==> result.regions@.len() == 0)]
#[ensures(free_sum(result.regions@) == capacity@)]
#[ensures(offsets_aligned(result.regions@))]
#[ensures(sorted_by_offset(result.regions@))]
#[ensures(disjoint_sorted(result.regions@))]
#[ensures(no_adj(result.regions@))]
#[ensures(fl_inv(result.regions@, result.capacity@))]
pub fn fl_new(capacity: usize) -> FreeListModel {
    let mut regions: Vec<Region> = Vec::new();
    if capacity > 0 {
        regions.push(Region {
            offset: 0,
            size: capacity,
        });
    }
    proof_assert!(capacity@ > 0 ==> regions@ == Seq::empty().push_back(Region { offset: 0usize, size: capacity }));
    FreeListModel {
        regions,
        capacity,
        used: 0,
    }
}

/// `remove(&offset); insert(offset + a, size - a)` on the run at index `i`
/// (allocator.rs:46-51, the `remaining > 0` case): shrink the run from the front.
#[requires(i@ < r@.len())]
#[requires(0 < a@ && a@ < r@[i@].size@)]
#[requires(a@ % 4096 == 0)]
#[requires(r@[i@].offset@ + r@[i@].size@ <= usize::MAX@)]
#[ensures((^r)@ == r@.set(i@, Region { offset: r@[i@].offset + a, size: r@[i@].size - a }))]
#[ensures((^r)@.len() == r@.len())]
#[ensures((^r)@[i@].offset@ == r@[i@].offset@ + a@ && (^r)@[i@].size@ == r@[i@].size@ - a@)]
#[ensures(forall<j: Int> 0 <= j && j < r@.len() && j != i@ ==> (^r)@[j] == r@[j])]
#[ensures(free_sum((^r)@) == free_sum(r@) - a@)]
#[ensures(sorted_by_offset(r@) && disj_all(r@) ==> sorted_by_offset((^r)@))]
#[ensures(disj_all(r@) ==> disj_all((^r)@))]
#[ensures(disjoint_sorted(r@) ==> disjoint_sorted((^r)@))]
#[ensures(offsets_aligned(r@) ==> offsets_aligned((^r)@))]
#[ensures(forall<c: Int> in_cap(r@, c) ==> in_cap((^r)@, c))]
#[ensures((forall<j: Int> 0 <= j && j < r@.len() ==> r@[j].size@ > 0)
          ==> (forall<j: Int> 0 <= j && j < (^r)@.len() ==> (^r)@[j].size@ > 0))]
#[ensures(forall<x0: Int, x1: Int> clear_of(r@, x0, x1) ==> clear_of((^r)@, x0, x1))]
#[ensures(forall<x0: Int, x1: Int> disj_all(r@) && r@[i@].offset@ <= x0 && x1 <= r@[i@].offset@ + a@
          ==> clear_of((^r)@, x0, x1))]
#[ensures(no_adj(r@) ==> no_adj((^r)@))]
pub fn shrink_at(r: &mut Vec<Region>, i: usize, a: usize) {
    let old = snapshot! { r@ };
    let reg = r[i];
    let nr = Region { offset: reg.offset + a, size: reg.size - a };
    snapshot! { lem_fs_set(*old, i@, nr) };
    r[i] = nr;
    proof_assert!(r@ == old.set(i@, nr));
    proof_assert!(free_sum(r@) == free_sum(old.set(i@, nr)));
}

/// Mirror of `FreeList::allocate` (`allocator.rs:38-55`).
///
/// `size` is a `u32` at the only call site (`lib.rs:366`, `insert(key, size: u32)`),
/// so `size as usize + 4095` cannot overflow a 64-bit `usize`. The `+= aligned_size`
/// at allocator.rs:53 cannot overflow either: either the caller bounds `used`
/// directly, or the accounting invariant `used + free_sum <= capacity` does
/// (then `aligned_size <= region size <= free_sum`).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + align_up_l(size@) + 4096 <= usize::MAX@
           || fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
// --- MT-INSERT-POST-ALIGNMENT: every handed-back offset is 4 KiB aligned ---
#[ensures(forall<o: usize> result == Some(o) ==> o@ % 4096 == 0)]
#[ensures(forall<o: usize> result == Some(o) ==> o@ + align_up_l(size@) <= fl.capacity@)]
#[ensures(forall<o: usize> result == Some(o) ==> o@ + size@ <= fl.capacity@)]
#[ensures((^fl).capacity == fl.capacity)]
#[ensures((^fl).used@ <= fl.used@ + align_up_l(size@))]
// --- the alignment invariant is MAINTAINED, so it holds for every later call ---
#[ensures(offsets_aligned((^fl).regions@))]
#[ensures(disjoint_sorted((^fl).regions@))]
#[ensures(forall<i: Int> 0 <= i && i < (^fl).regions@.len()
          ==> (^fl).regions@[i].offset@ + (^fl).regions@[i].size@ <= (^fl).capacity@)]
#[ensures(fl_inv((^fl).regions@, (^fl).capacity@))]
// --- MT-INSERT-ERR-POOL-FULL: None exactly when no single run is big enough ---
#[ensures(size@ > 0 && result == None
          ==> no_region_fits(fl.regions@, align_up_l(size@)))]
#[ensures(size@ > 0 && no_region_fits(fl.regions@, align_up_l(size@))
          ==> result == None)]
#[ensures(result == None ==> (^fl).regions@ == fl.regions@ && (^fl).used@ == fl.used@)]
#[ensures(size@ == 0 ==> result == None)]
// --- accounting (batch 1): exact bookkeeping of a successful allocation ---
#[ensures(forall<o: usize> result == Some(o) ==> o@ + align_up_l(size@) <= fl.capacity@ && o@ >= 0)]
#[ensures(forall<o: usize> result == Some(o) ==> (^fl).used@ == fl.used@ + align_up_l(size@))]
#[ensures(forall<o: usize> result == Some(o)
          ==> free_sum((^fl).regions@) == free_sum(fl.regions@) - align_up_l(size@))]
#[ensures(forall<o: usize> result == Some(o) ==> clear_of((^fl).regions@, o@, o@ + align_up_l(size@)))]
#[ensures(forall<o: usize, x0: Int, x1: Int> result == Some(o) && clear_of(fl.regions@, x0, x1)
          ==> disj(x0, x1, o@, o@ + align_up_l(size@)))]
#[ensures(forall<x0: Int, x1: Int> clear_of(fl.regions@, x0, x1) ==> clear_of((^fl).regions@, x0, x1))]
#[ensures(size@ > 0 && some_region_fits(fl.regions@, align_up_l(size@)) ==> result != None)]
// --- batch 2: MT-FREELIST-ALLOCATE-FRAME — exactly one run (the one at `i`) is touched ---
#[ensures(forall<o: usize> result == Some(o) ==> exists<i: Int> 0 <= i && i < fl.regions@.len()
          && fl.regions@[i].offset == o && alloc_step(fl.regions@, (^fl).regions@, i, align_up_l(size@)))]
#[ensures(forall<o: usize, j: Int> result == Some(o) && 0 <= j && j < fl.regions@.len() && fl.regions@[j].offset != o
          ==> exists<k: Int> 0 <= k && k < (^fl).regions@.len() && (^fl).regions@[k] == fl.regions@[j])]
#[ensures(forall<o: usize, k: Int> result == Some(o) && 0 <= k && k < (^fl).regions@.len()
          ==> (exists<j: Int> 0 <= j && j < fl.regions@.len() && fl.regions@[j].offset != o
                  && (^fl).regions@[k] == fl.regions@[j])
              || (^fl).regions@[k].offset@ == o@ + align_up_l(size@))]
// --- batch 3: MT-FREELIST-ALLOCATE-FIRST-FIT -- the taken run is the FIRST that fits ---
#[ensures(forall<o: usize> result == Some(o) ==> exists<i: Int> 0 <= i && i < fl.regions@.len()
          && fl.regions@[i].offset == o && fl.regions@[i].size@ >= align_up_l(size@)
          && (forall<j: Int> 0 <= j && j < i ==> fl.regions@[j].size@ < align_up_l(size@)))]
pub fn fl_allocate(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let aligned_size = align_up(size as usize);

    // `self.free_regions.iter().find(|(_, &s)| s >= aligned_size)` — first fit in
    // ascending-offset order (allocator.rs:44).
    let n = fl.regions.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == fl.regions@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> fl.regions@[j].size@ < aligned_size@)]
    while i < n {
        if fl.regions[i].size >= aligned_size {
            break;
        }
        i += 1;
    }
    if i == n {
        return None;
    }

    let reg = fl.regions[i];
    // Witness bridge: instantiate fl_inv's per-region bound at the found index so
    // the prover does not have to guess it (positional provenance).
    proof_assert!(reg.offset@ + reg.size@ <= fl.capacity@);
    proof_assert!(reg.size@ >= aligned_size@);
    proof_assert!(reg.offset@ + aligned_size@ <= fl.capacity@);
    snapshot! { lem_fs_bound(fl.regions@) };
    proof_assert!(reg.size@ <= free_sum(fl.regions@));
    proof_assert!(forall<x0: Int, x1: Int> clear_of(fl.regions@, x0, x1)
                  ==> disj(x0, x1, reg.offset@, reg.offset@ + reg.size@));
    // `remove(&offset)` then, if `remaining > 0`, `insert(offset + aligned_size,
    // remaining)` (allocator.rs:46-51) — i.e. shrink in place, else drop.
    let remaining = reg.size - aligned_size;
    let rb = snapshot! { fl.regions@ };
    proof_assert!(forall<j: Int> 0 <= j && j < i@ ==> rb[j].size@ < aligned_size@);
    proof_assert!(rb[i@] == reg && reg.size@ >= aligned_size@);
    if remaining > 0 {
        shrink_at(&mut fl.regions, i, aligned_size);
    } else {
        let _ = rm_at(&mut fl.regions, i);
    }
    proof_assert!(alloc_step(*rb, fl.regions@, i@, aligned_size@));
    proof_assert!(forall<j: Int> 0 <= j && j < rb.len() && j != i@ ==> rb[j].offset != reg.offset);
    proof_assert!(remaining@ == 0 ==> (forall<j: Int> 0 <= j && j < i@ ==> fl.regions@[j] == rb[j])
                  && (forall<j: Int> i@ < j && j < rb.len() ==> fl.regions@[j - 1] == rb[j]));
    proof_assert!(remaining@ > 0 ==> forall<j: Int> 0 <= j && j < rb.len() && j != i@ ==> fl.regions@[j] == rb[j]);
    proof_assert!(forall<j: Int> 0 <= j && j < rb.len() && rb[j].offset != reg.offset
                  ==> exists<k: Int> 0 <= k && k < fl.regions@.len() && fl.regions@[k] == rb[j]);
    proof_assert!(remaining@ == 0 ==> forall<k: Int> 0 <= k && k < fl.regions@.len()
                  ==> fl.regions@[k] == rb[if k < i@ { k } else { k + 1 }]);
    proof_assert!(forall<k: Int> 0 <= k && k < fl.regions@.len()
                  ==> (exists<j: Int> 0 <= j && j < rb.len() && rb[j].offset != reg.offset && fl.regions@[k] == rb[j])
                      || fl.regions@[k].offset@ == reg.offset@ + aligned_size@);
    fl.used += aligned_size;
    Some(reg.offset)
}

/// One first-fit carve of `a` bytes from run `i` (allocator.rs:46-51): the run is
/// either shrunk from the front in place or, if it was used up exactly, removed;
/// every other run keeps its position-relative identity.
#[logic(open)]
pub fn alloc_step(r: Seq<Region>, r2: Seq<Region>, i: Int, a: Int) -> bool {
    pearlite! {
        (r[i].size@ == a && r2 == r.removed(i))
        || (r[i].size@ > a && r2.len() == r.len()
            && (forall<j: Int> 0 <= j && j < r.len() && j != i ==> r2[j] == r[j])
            && r2[i].offset@ == r[i].offset@ + a && r2[i].size@ == r[i].size@ - a)
    }
}

/// Bytes `[offset, offset + aligned)` may be handed back: a healthy free list,
/// an aligned, in-pool, NOT-currently-free run, and `used` covers it. This is
/// `MT-FREELIST-DEALLOCATE-PRE` ("the exact offset of a currently allocated
/// run"), restated over what the free list itself can see. The code never checks it.
#[logic(open)]
pub fn dealloc_pre(fl: FreeListModel, offset: Int, aligned: Int) -> bool {
    pearlite! {
        fl_inv(fl.regions@, fl.capacity@)
        && aligned > 0
        && offset % 4096 == 0
        && aligned % 4096 == 0
        && offset + aligned <= fl.capacity@
        && clear_of(fl.regions@, offset, offset + aligned)
        && aligned <= fl.used@
    }
}

/// `BTreeMap::range(..key)` boundary: the first index whose key is `>= key`.
#[requires(sorted_by_offset(r@))]
#[ensures(result@ <= r@.len())]
#[ensures(forall<j: Int> 0 <= j && j < result@ ==> r@[j].offset@ < key@)]
#[ensures(forall<j: Int> result@ <= j && j < r@.len() ==> r@[j].offset@ >= key@)]
pub fn lower_bound(r: &Vec<Region>, key: usize) -> usize {
    let n = r.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == r@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> r@[j].offset@ < key@)]
    while i < n {
        if r[i].offset >= key {
            return i;
        }
        i += 1;
    }
    n
}

/// `BTreeMap::get(&key)` as a key scan.
#[ensures(match result {
    Some(j) => j@ < r@.len() && r@[j@].offset == key,
    None => forall<j: Int> 0 <= j && j < r@.len() ==> r@[j].offset != key,
})]
pub fn find_key(r: &Vec<Region>, key: usize) -> Option<usize> {
    let n = r.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == r@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> r@[j].offset != key)]
    while i < n {
        if r[i].offset == key {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// `BTreeMap::remove(&k)` for a key known to sit at index `i`, with positional
/// provenance and the free-sum delta.
#[requires(i@ < r@.len())]
#[ensures(result == r@[i@])]
#[ensures((^r)@ == r@.removed(i@))]
#[ensures((^r)@.len() == r@.len() - 1)]
#[ensures(forall<j: Int> 0 <= j && j < i@ ==> (^r)@[j] == r@[j])]
#[ensures(forall<j: Int> i@ <= j && j < (^r)@.len() ==> (^r)@[j] == r@[j + 1])]
#[ensures(free_sum((^r)@) == free_sum(r@) - r@[i@].size@)]
#[ensures(sorted_by_offset(r@) ==> sorted_by_offset((^r)@))]
#[ensures(disj_all(r@) ==> disj_all((^r)@))]
#[ensures(offsets_aligned(r@) ==> offsets_aligned((^r)@))]
#[ensures(forall<c: Int> in_cap(r@, c) ==> in_cap((^r)@, c))]
#[ensures((forall<j: Int> 0 <= j && j < r@.len() ==> r@[j].size@ > 0)
          ==> forall<j: Int> 0 <= j && j < (^r)@.len() ==> (^r)@[j].size@ > 0)]
#[ensures(forall<x0: Int, x1: Int> clear_of(r@, x0, x1) ==> clear_of((^r)@, x0, x1))]
#[ensures(disj_all(r@) && disjoint_sorted(r@) ==> disjoint_sorted((^r)@))]
#[ensures(forall<x0: Int, x1: Int> disj_all(r@) && r@[i@].offset@ <= x0
          && x1 <= r@[i@].offset@ + r@[i@].size@ ==> clear_of((^r)@, x0, x1))]
#[ensures(no_adj(r@) ==> no_adj((^r)@))]
pub fn rm_at(r: &mut Vec<Region>, i: usize) -> Region {
    let old = snapshot! { r@ };
    snapshot! { lem_fs_removed(*old, i@) };
    let res = r.remove(i);
    proof_assert!(disj_all(*old) ==> disj_all(r@));
    proof_assert!(disj_all(*old) && disjoint_sorted(*old) ==>
        forall<j: Int> 0 <= j && j + 1 < r@.len() ==> r@[j].offset@ + r@[j].size@ <= r@[j + 1].offset@);
    snapshot! { lem_noadj_removed(*old, i@) };
    res
}

/// `BTreeMap::insert(key, val)`: overwrite if `key` is present, else insert in
/// key order. Returns the index the entry now occupies.
#[requires(sorted_by_offset(r@))]
#[ensures(result@ < (^r)@.len())]
#[ensures((^r)@[result@] == Region { offset: key, size: val })]
#[ensures(sorted_by_offset((^r)@))]
#[ensures(forall<j: Int> 0 <= j && j < r@.len() && r@[j].offset != key
          ==> exists<k: Int> 0 <= k && k < (^r)@.len() && (^r)@[k] == r@[j])]
#[ensures(forall<k: Int> 0 <= k && k < (^r)@.len() && k != result@
          ==> exists<j: Int> 0 <= j && j < r@.len() && r@[j] == (^r)@[k] && r@[j].offset != key)]
#[ensures((forall<j: Int> 0 <= j && j < r@.len() ==> r@[j].offset != key)
          ==> free_sum((^r)@) == free_sum(r@) + val@ && (^r)@.len() == r@.len() + 1
              && (forall<j: Int> 0 <= j && j < result@ ==> (^r)@[j] == r@[j])
              && (forall<j: Int> result@ < j && j < (^r)@.len() ==> (^r)@[j] == r@[j - 1]))]
#[ensures(forall<j: Int> 0 <= j && j < r@.len() && r@[j].offset == key
          ==> (^r)@ == r@.set(j, Region { offset: key, size: val }))]
#[ensures(result@ <= r@.len())]
#[ensures(forall<j: Int> 0 <= j && j < result@ ==> r@[j].offset@ < key@)]
#[ensures(forall<j: Int> result@ < j && j < r@.len() ==> r@[j].offset@ > key@)]
#[ensures(result@ < r@.len() ==> r@[result@].offset@ >= key@)]
pub fn map_insert(r: &mut Vec<Region>, key: usize, val: usize) -> usize {
    let q = lower_bound(r, key);
    let x = Region { offset: key, size: val };
    let old = snapshot! { r@ };
    if q < r.len() && r[q].offset == key {
        r[q] = x;
        proof_assert!(forall<j: Int> 0 <= j && j < old.len() && old[j].offset == key ==> j == q@);
        q
    } else {
        r.insert(q, x);
        snapshot! { lem_fs_inserted(*old, r@, q@, x) };
        proof_assert!(forall<j: Int> 0 <= j && j < old.len() && j < q@ ==> r@[j] == old[j]);
        proof_assert!(forall<j: Int> 0 <= j && j < old.len() && j >= q@ ==> r@[j + 1] == old[j]);
        q
    }
}

/// `self.used -= aligned_size` (`allocator.rs:61`). In a release build
/// (overflow-checks off, the Cargo default) this WRAPS; in a debug/test build it
/// panics. Mirrored as the release-mode wrap, with the Int-level meaning exposed.
#[ensures(a@ <= used@ ==> result@ == used@ - a@)]
#[ensures(a@ > used@ ==> result@ == used@ - a@ + usize::MAX@ + 1)]
pub fn used_sub(used: usize, a: usize) -> usize {
    if a <= used {
        used - a
    } else {
        (usize::MAX - (a - used)) + 1
    }
}

/// Mirror of `FreeList::deallocate` (`allocator.rs:59-83`) — NO LONGER TRUSTED
/// (batch 1). Total: it runs on every shape-valid input exactly as the real code
/// does (BTreeMap ops mirrored by `lower_bound` / `find_key` / `rm_at` /
/// `map_insert` over the key-ordered Vec), including inputs that violate
/// `dealloc_pre`; the precise valid-case postconditions are conditional on it.
#[requires(sorted_by_offset(fl.regions@))]
#[requires(in_cap(fl.regions@, fl.capacity@))]
#[requires(size@ > 0)]
#[requires(offset@ + align_up_l(size@) <= fl.capacity@)]
// unconditional (what the code does whatever the caller passed)
#[ensures((^fl).capacity == fl.capacity)]
#[ensures(align_up_l(size@) <= fl.used@ ==> (^fl).used@ == fl.used@ - align_up_l(size@))]
#[ensures(align_up_l(size@) > fl.used@
          ==> (^fl).used@ == fl.used@ - align_up_l(size@) + usize::MAX@ + 1)]
#[ensures(sorted_by_offset((^fl).regions@))]
#[ensures(in_cap((^fl).regions@, (^fl).capacity@))]
// a run that is ALREADY free (a run starts at `offset`, none ends there, none
// starts right after it) is silently re-inserted over itself: nothing detects it
#[ensures(forall<j: Int> 0 <= j && j < fl.regions@.len() && fl.regions@[j].offset == offset
          && (forall<i: Int> 0 <= i && i < fl.regions@.len()
              ==> fl.regions@[i].offset@ + fl.regions@[i].size@ != offset@
                  && fl.regions@[i].offset@ != offset@ + align_up_l(size@))
          ==> exists<x: Region> x.offset == offset && x.size@ == align_up_l(size@)
              && (^fl).regions@ == fl.regions@.set(j, x))]
// valid case: MT-FREELIST-DEALLOCATE-POST
#[ensures(dealloc_pre(*fl, offset@, align_up_l(size@)) ==> fl_inv((^fl).regions@, (^fl).capacity@))]
#[ensures(dealloc_pre(*fl, offset@, align_up_l(size@))
          ==> free_sum((^fl).regions@) == free_sum(fl.regions@) + align_up_l(size@))]
#[ensures(forall<x0: Int, x1: Int> dealloc_pre(*fl, offset@, align_up_l(size@)) && x0 < x1
          && clear_of(fl.regions@, x0, x1) && disj(x0, x1, offset@, offset@ + align_up_l(size@))
          ==> clear_of((^fl).regions@, x0, x1))]
#[ensures(dealloc_pre(*fl, offset@, align_up_l(size@))
          ==> exists<j: Int> 0 <= j && j < (^fl).regions@.len()
              && (^fl).regions@[j].offset@ <= offset@
              && offset@ + align_up_l(size@) <= (^fl).regions@[j].offset@ + (^fl).regions@[j].size@)]
// --- batch 2: MT-INV-FREE-LIST-COALESCING -- a free neighbour on either side is merged in ---
#[ensures(forall<j: Int> dealloc_pre(*fl, offset@, align_up_l(size@)) && 0 <= j && j < fl.regions@.len()
          && fl.regions@[j].offset@ + fl.regions@[j].size@ == offset@
          ==> covered((^fl).regions@, fl.regions@[j].offset@, offset@ + align_up_l(size@)))]
#[ensures(forall<j: Int> dealloc_pre(*fl, offset@, align_up_l(size@)) && 0 <= j && j < fl.regions@.len()
          && fl.regions@[j].offset@ == offset@ + align_up_l(size@)
          ==> covered((^fl).regions@, offset@, fl.regions@[j].offset@ + fl.regions@[j].size@))]
#[ensures(dealloc_pre(*fl, offset@, align_up_l(size@)) ==> no_adj((^fl).regions@))]
pub fn fl_deallocate(fl: &mut FreeListModel, offset: usize, size: u32) {
    let pre = snapshot! { dealloc_pre(*fl, offset@, align_up_l(size@)) };
    let r0 = snapshot! { fl.regions@ };
    // allocator.rs:60-61
    let aligned_size = align_up(size as usize);
    fl.used = used_sub(fl.used, aligned_size);

    let mut new_offset = offset;
    let mut new_size = aligned_size;

    // allocator.rs:67 `self.free_regions.range(..offset).next_back()` = index p-1.
    let p = lower_bound(&fl.regions, offset);
    let mut pos = p;
    if p > 0 {
        let prev = fl.regions[p - 1];
        proof_assert!(prev.offset@ + prev.size@ <= fl.capacity@);
        if prev.offset + prev.size == offset {
            new_offset = prev.offset;
            new_size += prev.size;
            let _ = rm_at(&mut fl.regions, p - 1);
            pos = p - 1;
            proof_assert!(*pre && p@ >= 2 ==> r0[p@ - 2].offset@ + r0[p@ - 2].size@ < r0[p@ - 1].offset@);
            proof_assert!(*pre && p@ >= 2 ==> fl.regions@[p@ - 2] == r0[p@ - 2]);
            proof_assert!(*pre && pos@ > 0 ==> fl.regions@[pos@ - 1].offset@ + fl.regions@[pos@ - 1].size@ < new_offset@);
        } else {
            proof_assert!(*pre ==> prev.offset@ < offset@);
            proof_assert!(*pre ==> prev.offset@ + prev.size@ <= offset@);
            proof_assert!(*pre && pos@ > 0 ==> fl.regions@[pos@ - 1].offset@ + fl.regions@[pos@ - 1].size@ < new_offset@);
        }
    }
    let r1 = snapshot! { fl.regions@ };
    proof_assert!(*pre ==> new_offset@ + new_size@ == offset@ + aligned_size@);
    proof_assert!(*pre && pos@ > 0 ==> r1[pos@ - 1].offset@ + r1[pos@ - 1].size@ < new_offset@);
    proof_assert!(*pre ==> forall<j: Int> 0 <= j && j < pos@ ==> r1[j].offset@ + r1[j].size@ <= new_offset@);
    proof_assert!(*pre ==> forall<j: Int> pos@ <= j && j < r1.len() ==> r1[j].offset@ >= new_offset@ + new_size@);
    proof_assert!(new_offset@ + new_size@ <= fl.capacity@);

    // allocator.rs:76-80 coalesce with the following run.
    let next_offset = new_offset + new_size;
    match find_key(&fl.regions, next_offset) {
        Some(j) => {
            proof_assert!(*pre ==> j == pos);
            let next_size = fl.regions[j].size;
            proof_assert!(fl.regions@[j@].offset@ + fl.regions@[j@].size@ <= fl.capacity@);
            new_size += next_size;
            let _ = rm_at(&mut fl.regions, j);
        }
        None => {}
    }
    let r2 = snapshot! { fl.regions@ };
    proof_assert!(forall<x0: Int, x1: Int> *pre && x0 < x1 && clear_of(*r0, x0, x1)
                  ==> clear_of(*r2, x0, x1));
    proof_assert!(forall<x0: Int, x1: Int> *pre && x0 < x1 && clear_of(*r0, x0, x1)
                  && disj(x0, x1, offset@, offset@ + aligned_size@)
                  ==> disj(x0, x1, new_offset@, new_offset@ + new_size@));
    proof_assert!(*pre && pos@ > 0 ==> r2[pos@ - 1] == r1[pos@ - 1]);
    proof_assert!(*pre && pos@ > 0 ==> r2[pos@ - 1].offset@ + r2[pos@ - 1].size@ < new_offset@);
    proof_assert!(*pre && pos@ < r2.len() ==> new_offset@ + new_size@ < r2[pos@].offset@);
    proof_assert!(*pre ==> forall<j: Int> 0 <= j && j < pos@ ==> r2[j].offset@ + r2[j].size@ <= new_offset@);
    proof_assert!(*pre ==> forall<j: Int> pos@ <= j && j < r2.len() ==> r2[j].offset@ >= new_offset@ + new_size@);
    proof_assert!(*pre ==> forall<j: Int> 0 <= j && j < r2.len() ==> r2[j].offset != new_offset);
    proof_assert!(*pre ==> new_offset@ % 4096 == 0);
    proof_assert!(*pre ==> new_offset@ <= offset@ && offset@ + aligned_size@ <= new_offset@ + new_size@);
    proof_assert!(*pre ==> free_sum(*r2) + new_size@ == free_sum(*r0) + aligned_size@);

    // allocator.rs:82
    proof_assert!(*pre ==> disj_all(*r2) && offsets_aligned(*r2) && in_cap(*r2, fl.capacity@)
                  && (forall<j: Int> 0 <= j && j < r2.len() ==> r2[j].size@ > 0));
    proof_assert!(new_offset@ + new_size@ <= fl.capacity@ && in_cap(*r2, fl.capacity@));
    let q = map_insert(&mut fl.regions, new_offset, new_size);
    proof_assert!(forall<k: Int> 0 <= k && k < fl.regions@.len() && k != q@
                  ==> fl.regions@[k].offset@ + fl.regions@[k].size@ <= fl.capacity@);
    proof_assert!(in_cap(fl.regions@, fl.capacity@));
    proof_assert!(*pre ==> pos@ <= r2.len());
    proof_assert!(*pre ==> q == pos);
    proof_assert!(*pre ==> fl.regions@.len() == r2.len() + 1);
    proof_assert!(*pre ==> forall<j: Int> 0 <= j && j < q@ ==> fl.regions@[j] == r2[j]);
    proof_assert!(*pre ==> forall<j: Int> q@ < j && j < fl.regions@.len() ==> fl.regions@[j] == r2[j - 1]);
    proof_assert!(*pre ==> fl.regions@[q@].offset == new_offset && fl.regions@[q@].size == new_size);
    let fin = snapshot! { fl.regions@ };
    proof_assert!(*pre ==> forall<i: Int, j: Int> 0 <= i && i < j && j < q@
                  ==> fin[i].offset@ + fin[i].size@ <= fin[j].offset@);
    proof_assert!(*pre ==> forall<i: Int, j: Int> q@ < i && i < j && j < fin.len()
                  ==> fin[i].offset@ + fin[i].size@ <= fin[j].offset@);
    proof_assert!(*pre ==> forall<i: Int, j: Int> 0 <= i && i < q@ && q@ < j && j < fin.len()
                  ==> fin[i].offset@ + fin[i].size@ <= fin[j].offset@);
    proof_assert!(*pre ==> forall<i: Int> 0 <= i && i < q@
                  ==> fin[i].offset@ + fin[i].size@ <= fin[q@].offset@);
    proof_assert!(*pre ==> forall<j: Int> q@ < j && j < fin.len()
                  ==> fin[q@].offset@ + fin[q@].size@ <= fin[j].offset@);
    proof_assert!(*pre ==> disj_all(fl.regions@));
    proof_assert!(*pre ==> offsets_aligned(fl.regions@));
    proof_assert!(*pre ==> forall<j: Int> 0 <= j && j < fl.regions@.len() ==> fl.regions@[j].size@ > 0);
    proof_assert!(*pre ==> disjoint_sorted(fl.regions@));
    proof_assert!(*pre ==> in_cap(fl.regions@, fl.capacity@));
    proof_assert!(*pre ==> no_adj(*r2));
    proof_assert!(*pre && q@ > 0 ==> r2[q@ - 1].offset@ + r2[q@ - 1].size@ < fin[q@].offset@);
    proof_assert!(*pre && q@ < r2.len() ==> fin[q@].offset@ + fin[q@].size@ < r2[q@].offset@);
    snapshot! { if *pre { lem_noadj_inserted(*r2, *fin, q@) } else { () } };
    proof_assert!(*pre ==> no_adj(*fin));
    snapshot! { if *pre { lem_clear_inserted(*r2, *fin, q@) } else { () } };
    proof_assert!(forall<x0: Int, x1: Int> *pre && x0 < x1 && clear_of(*r0, x0, x1)
                  && disj(x0, x1, offset@, offset@ + aligned_size@) ==> clear_of(*fin, x0, x1));
    proof_assert!(*pre ==> covered(fl.regions@, new_offset@, new_offset@ + new_size@));
    let _ = r1;
}

// ===========================================================================
// 3. Component-state mirror (lib.rs:69-109)
// ===========================================================================

/// `interfaces::CacheKey`.
pub type Key = u64;

/// Mirror of `Slot` (`lib.rs:69`).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct SlotModel {
    pub key: Key,
    pub offset: usize,
    pub size: u32,
    pub handle: u64,
}

/// Mirror of `interfaces::MemoryTierError`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum MtError {
    InvalidSize,
    NotInitialized,
    AlreadyExists,
    PoolFull,
    KeyNotFound,
    AllocationFailed,
    /// `MemoryTierError::NotEvictable(CacheKey)` (`imemory_tier.rs:31-32`) — added
    /// by batch 2 so the mirror declares all SEVEN error kinds the interface does;
    /// `MT-INV-NEVER-NOT-EVICTABLE` proves no mirrored method ever returns it.
    NotEvictable,
}

/// Mirror of the BOUND `IEvictionPolicy` component's bookkeeping, at the
/// granularity its interface documents. `tracked` is the LRU order: index 0 is
/// the oldest (`LruList.head`), the last index the newest
/// (`eviction-policy-lru/src/lru_list.rs`).
pub struct PolicyModel {
    pub tracked: Vec<(Key, u64)>,
    pub next_handle: u64,
    pub pools: u64,
    /// GHOST (batch 1): the pool id carried by the most recent call into the
    /// policy. Lets `MT-INIT-POST-POLICY-POOL` state *which* pool each call used.
    pub last_pool: u64,
}

/// Mirror of `MemoryTierState` + the inner `Pool` (`lib.rs:74-87`), flattened.
pub struct StateModel {
    /// `pool_ptr: *mut u8` as a `usize` handle (`0` == null).
    pub pool_base: usize,
    pub pool_size: usize,
    pub pool_id: u64,
    pub allocator: FreeListModel,
    /// `Pool::slots: HashMap<CacheKey, Slot>` as a program Seq mirror.
    pub slots: Vec<SlotModel>,
    pub initialized: bool,
    pub spdk_allocated: bool,
    /// Poison flag of the two `RwLock`s, which the component always `.unwrap()`s.
    pub poisoned: bool,
    pub policy: PolicyModel,
}

#[logic(open)]
pub fn slot_at(s: Seq<SlotModel>, k: Key) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < s.len() && s[i].key == k }
}

#[logic(open)]
pub fn tracks(t: Seq<(Key, u64)>, k: Key) -> bool {
    pearlite! { exists<i: Int> 0 <= i && i < t.len() && (t[i]).0 == k }
}

#[logic(open)]
pub fn policy_keys_unique(t: Seq<(Key, u64)>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < t.len() ==> (t[i]).0 != (t[j]).0
    }
}

#[logic(open)]
pub fn slot_keys_unique(s: Seq<SlotModel>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < j && j < s.len() ==> s[i].key != s[j].key
    }
}

/// Every key present in both tables maps to the identical slot record (same
/// offset, size, handle): an entry is never moved or resized in place.
#[logic(open)]
pub fn slot_stable(a: Seq<SlotModel>, b: Seq<SlotModel>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < a.len() && 0 <= j && j < b.len() && a[i].key == b[j].key
            ==> a[i] == b[j]
    }
}

/// The structural invariant of the free list (`allocator.rs`), as the
/// conjunction of its named halves so that re-deriving it is an unfolding.
#[logic(open)]
pub fn fl_inv(r: Seq<Region>, capacity: Int) -> bool {
    pearlite! {
        offsets_aligned(r)
        && disjoint_sorted(r)
        && (forall<i: Int> 0 <= i && i < r.len() ==> r[i].offset@ + r[i].size@ <= capacity)
        && disj_all(r)
        && sorted_by_offset(r)
        && no_adj(r)
    }
}

/// The key-bookkeeping invariant that couples `Pool::slots` to the bound
/// eviction policy: a key is tracked by the policy iff the pool has a slot for
/// it, and neither side holds a key twice.
#[logic(open)]
pub fn key_inv(st: StateModel) -> bool {
    pearlite! {
        policy_keys_unique(st.policy.tracked@)
        && slot_keys_unique(st.slots@)
        && (forall<k: Key> slot_at(st.slots@, k) == tracks(st.policy.tracked@, k))
    }
}

/// Every live slot sits on a 4 KiB boundary inside the pool.
#[logic(open)]
pub fn slot_inv(s: Seq<SlotModel>, capacity: Int) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.len()
            ==> s[i].offset@ % 4096 == 0 && s[i].offset@ + s[i].size@ <= capacity
    }
}

/// `MT-INV-INITIALIZE-ONCE-ONLY`, as a maintained state predicate: the component
/// has exactly two states; in the not-set-up state nothing is tracked at all, and
/// in the set-up state the base address is page aligned, non-null, the pool fits
/// in the address space, and the allocator capacity equals the recorded pool size.
#[logic(open)]
pub fn init_inv(st: StateModel) -> bool {
    pearlite! {
        (st.initialized ==>
            st.pool_base@ % 4096 == 0
            && st.pool_base@ > 0
            && st.pool_size@ > 0
            && st.pool_base@ + st.allocator.capacity@ <= usize::MAX@
            && st.allocator.capacity@ == st.pool_size@)
        && (!st.initialized ==> st.slots@.len() == 0 && st.policy.tracked@.len() == 0)
    }
}

/// Rounded size of one slot (what `FreeList::allocate` actually carved out).
#[logic(open)]
pub fn sl_end(sl: SlotModel) -> Int {
    pearlite! { sl.offset@ + align_up_l(sl.size@) }
}

/// Total rounded size of the slots held (`MT-FREELIST-USED-POST`'s right-hand side).
#[logic(open)]
#[variant(s.len())]
pub fn slot_sum(s: Seq<SlotModel>) -> Int {
    pearlite! {
        if s.len() <= 0 { 0 }
        else { slot_sum(s.subsequence(0, s.len() - 1)) + align_up_l(s[s.len() - 1].size@) }
    }
}

#[logic]
#[ensures(slot_sum(s.push_back(e)) == slot_sum(s) + align_up_l(e.size@))]
pub fn lem_ss_push(s: Seq<SlotModel>, e: SlotModel) {
    pearlite! { proof_assert!(s.push_back(e).subsequence(0, s.len()) == s); () }
}

#[logic]
#[requires(0 <= i && i < s.len())]
#[ensures(slot_sum(s.subsequence(0, i + 1)) == slot_sum(s.subsequence(0, i)) + align_up_l(s[i].size@))]
pub fn lem_ss_prefix(s: Seq<SlotModel>, i: Int) {
    pearlite! { proof_assert!(s.subsequence(0, i + 1).subsequence(0, i) == s.subsequence(0, i)); () }
}

/// `align_up_l` facts for a positive request.
#[logic]
#[requires(x > 0)]
#[ensures(align_up_l(x) % 4096 == 0)]
#[ensures(align_up_l(x) >= x)]
#[ensures(align_up_l(x) >= 4096)]
#[ensures(align_up_l(x) < x + 4096)]
#[ensures(x % 4096 == 0 ==> align_up_l(x) == x)]
#[ensures(x % 4096 != 0 ==> align_up_l(x) > x)]
pub fn lem_al(x: Int) {}

/// With positive sizes every slot's rounded size is bounded by the total.
#[logic]
#[variant(s.len())]
#[requires(forall<j: Int> 0 <= j && j < s.len() ==> s[j].size@ > 0)]
#[ensures(slot_sum(s) >= 0)]
#[ensures(forall<j: Int> 0 <= j && j < s.len() ==> align_up_l(s[j].size@) <= slot_sum(s))]
pub fn lem_ss_bound(s: Seq<SlotModel>) {
    pearlite! {
        if s.len() <= 0 { proof_assert!(slot_sum(s) == 0); () } else {
            lem_al(s[s.len() - 1].size@);
            lem_ss_bound(s.subsequence(0, s.len() - 1));
            proof_assert!(slot_sum(s) == slot_sum(s.subsequence(0, s.len() - 1)) + align_up_l(s[s.len() - 1].size@));
            proof_assert!(forall<j: Int> 0 <= j && j < s.len() - 1 ==> s.subsequence(0, s.len() - 1)[j] == s[j]);
            ()
        }
    }
}

/// Every slot is non-empty and its rounded run lies inside the pool.
#[logic(open)]
pub fn slot_ivl_ok(s: Seq<SlotModel>, capacity: Int) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.len() ==> s[i].size@ > 0 && sl_end(s[i]) <= capacity
    }
}

/// `MT-INV-ENTRIES-DISJOINT`: two distinct entries never share a byte.
#[logic(open)]
pub fn slots_disj(s: Seq<SlotModel>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < s.len() && 0 <= j && j < s.len() && s[i].key != s[j].key
            ==> disj(s[i].offset@, sl_end(s[i]), s[j].offset@, sl_end(s[j]))
    }
}

/// No entry's bytes are also on the free list (no double booking).
#[logic(open)]
pub fn slots_clear(r: Seq<Region>, s: Seq<SlotModel>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < s.len() ==> clear_of(r, s[i].offset@, sl_end(s[i])) }
}

/// The accounting invariant (`MT-INV-NO-LEAKED-OR-DOUBLE-BOOKED-SPACE`), as the
/// conjunction of its named halves.
#[logic(open)]
pub fn acct_ok(st: StateModel) -> bool {
    pearlite! {
        st.allocator.used@ == slot_sum(st.slots@)
        && free_sum(st.allocator.regions@) + st.allocator.used@ == st.allocator.capacity@
        && slot_ivl_ok(st.slots@, st.allocator.capacity@)
        && slots_disj(st.slots@)
        && slots_clear(st.allocator.regions@, st.slots@)
    }
}

#[logic(open)]
pub fn mt_inv(st: StateModel) -> bool {
    pearlite! {
        fl_inv(st.allocator.regions@, st.allocator.capacity@)
        && key_inv(st)
        && init_inv(st)
        && slot_inv(st.slots@, st.allocator.capacity@)
        && acct_ok(st)
    }
}

// ===========================================================================
// 4. Trusted leaves: the two effects Creusot cannot model
// ===========================================================================

/// Mirror of the pool allocation in `initialize` (`lib.rs:278-299` SPDK path /
/// `lib.rs:184-220` mmap path).
///
/// `#[trusted]`: models the OS/SPDK guarantee that `mmap` returns a page-aligned
/// mapping and `spdk_zmalloc(size, 4096, ..)` returns 4096-aligned DMA memory, or
/// NULL. Creusot cannot model `mmap`/`spdk_zmalloc`. What this does NOT prove:
/// that the mapping is actually `pool_size` bytes long or readable/writable.
#[trusted]
#[ensures(forall<b: usize> result == Some(b)
          ==> b@ % 4096 == 0 && b@ > 0 && b@ + size@ <= usize::MAX@)]
pub fn alloc_pool(size: usize) -> Option<usize> {
    if size == 0 {
        None
    } else {
        Some(4096)
    }
}

/// Mirror of `self.state.read().unwrap()` / `state.pool.write().unwrap()`
/// (`lib.rs:270, 333, 354, 367, 494, 602` and every other lock site).
///
/// `#[trusted]`: encodes `std::sync::RwLock`'s documented poisoning semantics —
/// after a panic unwinds out of a guard the lock is poisoned FOREVER, `read()`
/// / `write()` return `Err(PoisonError)`, and `.unwrap()` on that `Err` panics.
/// `creusot-std` ships NO model for `RwLock`/`Mutex` at all, and Creusot proves
/// panic-FREEDOM rather than predicting that a given call panics, so this leaf
/// is the boundary. What this does NOT prove: that a panic inside a guard really
/// sets the flag (that is `std`'s contract), nor anything about interleaving.
#[trusted]
#[ensures(result == !poisoned)]
pub fn lock_ok(poisoned: bool) -> bool {
    !poisoned
}

/// A panic unwinding out of a held guard. `#[trusted]` for the same reason.
#[trusted]
#[ensures((^st).poisoned)]
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
pub fn panic_while_holding(st: &mut StateModel) {
    st.poisoned = true;
}

// ===========================================================================
// 5. Policy mirror (the bound IEvictionPolicy component)
// ===========================================================================

#[requires(p.pools@ < u64::MAX@)]
#[ensures((^p).tracked@ == p.tracked@)]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools@ == p.pools@ + 1)]
#[ensures(result@ == p.pools@)]
#[ensures((^p).last_pool == result)]
pub fn policy_create_pool(p: &mut PolicyModel) -> u64 {
    let id = p.pools;
    p.pools += 1;
    p.last_pool = id;
    id
}

/// `IEvictionPolicy::track` -> `LruList::push_back` (newest at the back).
#[requires(p.next_handle@ < u64::MAX@)]
#[requires(policy_keys_unique(p.tracked@))]
#[requires(!tracks(p.tracked@, key))]
#[ensures((^p).tracked@ == p.tracked@.push_back((key, result)))]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures(tracks((^p).tracked@, key))]
#[ensures(forall<k: Key> k != key ==> tracks((^p).tracked@, k) == tracks(p.tracked@, k))]
#[ensures((^p).next_handle@ == p.next_handle@ + 1)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
pub fn policy_track(p: &mut PolicyModel, pool: u64, key: Key) -> u64 {
    p.last_pool = pool;
    let h = p.next_handle;
    p.next_handle += 1;
    p.tracked.push((key, h));
    h
}

/// `IEvictionPolicy::identify_next_to_evict`, as DOCUMENTED on the interface
/// (`components/interfaces/src/ieviction_policy.rs:101-104`): "Select the next
/// key the policy would evict, **remove it from tracking**, and return it."
/// `eviction-policy-lru` implements it as `LruList::pop_front()`
/// (`eviction-policy-lru/src/lru_list.rs:99-104`), which calls
/// `remove(head_idx)` and so deactivates the node, unlinks it, frees its index
/// and decrements `len` (`lru_list.rs:121-145`).
///
/// Mirrored as "read the head key, then remove that key": identical to
/// `pop_front` because the policy never tracks a key twice
/// (`policy_keys_unique`).
#[requires(policy_keys_unique(p.tracked@))]
#[ensures(match result {
    Some(k) => p.tracked@.len() > 0
        && k == (p.tracked@[0]).0
        && !tracks((^p).tracked@, k),
    None => p.tracked@.len() == 0 && (^p).tracked@ == p.tracked@,
})]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures(forall<k: Key, k2: Key> result == Some(k) && k2 != k
          ==> tracks((^p).tracked@, k2) == tracks(p.tracked@, k2))]
#[ensures(forall<k: Key> tracks((^p).tracked@, k) ==> tracks(p.tracked@, k))]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
pub fn policy_identify_next_to_evict(p: &mut PolicyModel, pool: u64) -> Option<Key> {
    p.last_pool = pool;
    if p.tracked.len() == 0 {
        return None;
    }
    let k = p.tracked[0].0;
    policy_remove_key(p, pool, k);
    Some(k)
}

/// `IEvictionPolicy::remove(handle)` -> `LruList::remove(idx)` (`lib.rs:500`).
///
/// Mirrored as an order-preserving filter rather than an unlink: observationally
/// identical on the contents AND the order of `tracked`, which is all any proved
/// property reads. (`LruList::remove` is O(1); this mirror is O(n). Cost is not a
/// proved property.)
#[requires(policy_keys_unique(p.tracked@))]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures(!tracks((^p).tracked@, key))]
#[ensures(forall<k: Key> k != key ==> tracks((^p).tracked@, k) == tracks(p.tracked@, k))]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
#[ensures((^p).tracked@.len() <= p.tracked@.len())]
pub fn policy_remove_key(p: &mut PolicyModel, pool: u64, key: Key) {
    p.last_pool = pool;
    let n = p.tracked.len();
    let mut out: Vec<(Key, u64)> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == p.tracked@.len())]
    #[invariant(!tracks(out@, key))]
    #[invariant(policy_keys_unique(out@))]
    #[invariant(forall<k: Key> tracks(out@, k)
                ==> exists<j: Int> 0 <= j && j < i@ && (p.tracked@[j]).0 == k)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && (p.tracked@[j]).0 != key
                ==> tracks(out@, (p.tracked@[j]).0))]
    #[invariant(out@.len() <= i@)]
    while i < n {
        let e = p.tracked[i];
        if e.0 != key {
            out.push(e);
        }
        i += 1;
    }
    p.tracked = out;
}

/// Linear-scan membership test on the `Pool::slots` mirror
/// (`pool.slots.contains_key(&key)`, `lib.rs:357`).
#[ensures(result == slot_at(s@, k))]
pub fn slots_contains(s: &Vec<SlotModel>, k: Key) -> bool {
    let n = s.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> s@[j].key != k)]
    while i < n {
        if s[i].key == k {
            return true;
        }
        i += 1;
    }
    false
}

/// `pool.slots.remove(&key)` (`lib.rs:455`, `lib.rs:497`), mirrored as an
/// order-preserving filter that also returns the removed slot.
///
/// The `slot_inv` preservation clauses are quantified over `capacity` and stated
/// in the SAME (positional) SHAPE as `slot_inv` itself, so a caller re-derives
/// `slot_inv` by instantiation rather than by inventing a position.
#[requires(slot_keys_unique(s@))]
#[ensures(slot_keys_unique((^s)@))]
#[ensures(!slot_at((^s)@, k))]
#[ensures(forall<k2: Key> k2 != k ==> slot_at((^s)@, k2) == slot_at(s@, k2))]
#[ensures(match result {
    Some(sl) => sl.key == k && slot_at(s@, k),
    None => !slot_at(s@, k),
})]
#[ensures(forall<cap: Int> slot_inv(s@, cap) ==> slot_inv((^s)@, cap))]
#[ensures(forall<cap: Int, sl: SlotModel> slot_inv(s@, cap) && result == Some(sl)
          ==> sl.offset@ % 4096 == 0 && sl.offset@ + sl.size@ <= cap)]
// --- accounting (batch 1): sum delta + positional provenance + same-shape frames ---
#[ensures(slot_sum((^s)@) == slot_sum(s@) - match result { Some(sl) => align_up_l(sl.size@), None => 0 })]
#[ensures(forall<j: Int> 0 <= j && j < (^s)@.len()
          ==> exists<i: Int> 0 <= i && i < s@.len() && s@[i] == (^s)@[j] && (^s)@[j].key != k)]
#[ensures(forall<sl: SlotModel> result == Some(sl) ==> exists<i: Int> 0 <= i && i < s@.len() && s@[i] == sl)]
#[ensures(result == None ==> (^s)@ == s@)]
#[ensures(slot_stable(s@, (^s)@))]
#[ensures(forall<cap: Int> slot_ivl_ok(s@, cap) ==> slot_ivl_ok((^s)@, cap))]
#[ensures(forall<cap: Int, sl: SlotModel> slot_ivl_ok(s@, cap) && result == Some(sl)
          ==> sl.size@ > 0 && sl_end(sl) <= cap)]
#[ensures(slots_disj(s@) ==> slots_disj((^s)@))]
#[ensures(forall<r: Seq<Region>> slots_clear(r, s@) ==> slots_clear(r, (^s)@))]
#[ensures(forall<r: Seq<Region>, sl: SlotModel> slots_clear(r, s@) && result == Some(sl)
          ==> clear_of(r, sl.offset@, sl_end(sl)))]
#[ensures(forall<sl: SlotModel> slots_disj(s@) && result == Some(sl)
          ==> forall<j: Int> 0 <= j && j < (^s)@.len()
              ==> disj(sl.offset@, sl_end(sl), (^s)@[j].offset@, sl_end((^s)@[j])))]
#[ensures(forall<sl: SlotModel> result == Some(sl) && (forall<j: Int> 0 <= j && j < s@.len() ==> s@[j].size@ > 0)
          ==> align_up_l(sl.size@) <= slot_sum(s@))]
pub fn slots_remove(s: &mut Vec<SlotModel>, k: Key) -> Option<SlotModel> {
    let n = s.len();
    let mut out: Vec<SlotModel> = Vec::new();
    let mut found: Option<SlotModel> = None;
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == s@.len())]
    #[invariant(!slot_at(out@, k))]
    #[invariant(slot_keys_unique(out@))]
    #[invariant(forall<k2: Key> slot_at(out@, k2)
                ==> exists<j: Int> 0 <= j && j < i@ && s@[j].key == k2)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && s@[j].key != k ==> slot_at(out@, s@[j].key))]
    #[invariant(match found { Some(sl) => sl.key == k && slot_at(s@, k), None => true })]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && s@[j].key == k
                ==> match found { Some(_) => true, None => false })]
    #[invariant(forall<cap: Int> slot_inv(s@, cap) ==> slot_inv(out@, cap))]
    #[invariant(forall<cap: Int, sl: SlotModel> slot_inv(s@, cap) && found == Some(sl)
                ==> sl.offset@ % 4096 == 0 && sl.offset@ + sl.size@ <= cap)]
    #[invariant(forall<j: Int> 0 <= j && j < out@.len()
                ==> exists<i2: Int> 0 <= i2 && i2 < i@ && s@[i2] == out@[j] && out@[j].key != k)]
    #[invariant(match found { Some(sl) => exists<i2: Int> 0 <= i2 && i2 < i@ && s@[i2] == sl, None => true })]
    #[invariant(found == None ==> out@ == s@.subsequence(0, i@))]
    #[invariant(slot_sum(out@) + match found { Some(sl) => align_up_l(sl.size@), None => 0 }
                == slot_sum(s@.subsequence(0, i@)))]
    while i < n {
        let e = s[i];
        snapshot! { lem_ss_prefix(s@, i@) };
        snapshot! { lem_ss_push(out@, e) };
        if e.key == k {
            found = Some(e);
        } else {
            proof_assert!(!slot_at(out@, e.key));
            let o0 = snapshot! { out@ };
            out.push(e);
            proof_assert!(out@ == o0.push_back(e));
            proof_assert!(forall<j: Int> 0 <= j && j < o0.len() ==> out@[j] == o0[j]);
            proof_assert!(forall<k2: Key> slot_at(*o0, k2) ==> slot_at(out@, k2));
            proof_assert!(out@[out@.len() - 1] == e && slot_at(out@, e.key));
            proof_assert!(forall<k2: Key> slot_at(out@, k2) ==> slot_at(*o0, k2) || k2 == e.key);
            proof_assert!(e == s@[i@]);
            proof_assert!(slot_keys_unique(out@));
        }
        i += 1;
    }
    proof_assert!(s@.subsequence(0, n@) == s@);
    let old = snapshot! { s@ };
    snapshot! { lem_ss_bound(*old) };
    *s = out;
    found
}

/// `IEvictionPolicy::clear_pool` (`lib.rs:602`).
#[ensures((^p).tracked@.len() == 0)]
#[ensures(forall<k: Key> !tracks((^p).tracked@, k))]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
pub fn policy_clear_pool(p: &mut PolicyModel, pool: u64) {
    p.last_pool = pool;
    p.tracked = Vec::new();
}

/// `IEvictionPolicy::get_eviction_candidates(pool, n)` -> `LruList::peek_front_n`:
/// the first `n` tracked keys in eviction order, without removing them.
#[ensures((^p).tracked == p.tracked)]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
#[ensures(result@.len() <= n@)]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> tracks(p.tracked@, result@[i]))]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i] == (p.tracked@[i]).0)]
#[ensures(result@.len() == if n@ < p.tracked@.len() { n@ } else { p.tracked@.len() })]
pub fn policy_candidates(p: &mut PolicyModel, pool: u64, n: usize) -> Vec<Key> {
    p.last_pool = pool;
    let mut out: Vec<Key> = Vec::new();
    let len = p.tracked.len();
    let mut i: usize = 0;
    #[invariant(i@ <= len@)]
    #[invariant(len@ == p.tracked@.len())]
    #[invariant(out@.len() == i@)]
    #[invariant(i@ <= n@)]
    #[invariant(forall<j: Int> 0 <= j && j < out@.len() ==> out@[j] == (p.tracked@[j]).0)]
    while i < len && i < n {
        out.push(p.tracked[i].0);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// 5a. Batch 2: a FUNCTIONAL model of the LRU refresh, so that "the same result"
//     (MT-BATCH-TOUCH-POST-ALL-PRESENT) and "the refreshed entry is the newest"
//     (MT-EVICT-NEXT-POST-LRU-ORDER) can be stated as equalities.
// ---------------------------------------------------------------------------

/// Index of the first tracked entry for `k` (`t.len()` if none).
#[logic(open)]
#[variant(t.len())]
pub fn first_idx(t: Seq<(Key, u64)>, k: Key) -> Int {
    pearlite! {
        if t.len() <= 0 { 0 }
        else if (t[0]).0 == k { 0 }
        else { 1 + first_idx(t.subsequence(1, t.len()), k) }
    }
}

/// A linear scan that stopped at `i` computes `first_idx`.
#[logic]
#[variant(t.len())]
#[requires(0 <= i && i <= t.len())]
#[requires(forall<j: Int> 0 <= j && j < i ==> (t[j]).0 != k)]
#[requires(i < t.len() ==> (t[i]).0 == k)]
#[ensures(first_idx(t, k) == i)]
pub fn lem_fi(t: Seq<(Key, u64)>, k: Key, i: Int) {
    pearlite! {
        if t.len() <= 0 { () }
        else if i == 0 { () }
        else {
            proof_assert!(forall<j: Int> 0 <= j && j < t.len() - 1 ==> t.subsequence(1, t.len())[j] == t[j + 1]);
            lem_fi(t.subsequence(1, t.len()), k, i - 1)
        }
    }
}

/// No entry for `k` at all: `first_idx` is the length.
#[logic]
#[requires(forall<j: Int> 0 <= j && j < t.len() ==> (t[j]).0 != k)]
#[ensures(first_idx(t, k) == t.len())]
pub fn lem_fi_none(t: Seq<(Key, u64)>, k: Key) {
    pearlite! { lem_fi(t, k, t.len()) }
}

/// `LruList::move_to_back` of `k`'s entry, as a function of the order.
#[logic(open)]
pub fn touch_l(t: Seq<(Key, u64)>, k: Key) -> Seq<(Key, u64)> {
    pearlite! {
        if first_idx(t, k) < t.len() { t.removed(first_idx(t, k)).push_back(t[first_idx(t, k)]) } else { t }
    }
}

/// Refresh every key of `ks`, in order.
#[logic(open)]
#[variant(ks.len())]
pub fn touch_all_l(t: Seq<(Key, u64)>, ks: Seq<Key>) -> Seq<(Key, u64)> {
    pearlite! {
        if ks.len() <= 0 { t }
        else { touch_l(touch_all_l(t, ks.subsequence(0, ks.len() - 1)), ks[ks.len() - 1]) }
    }
}

/// The keys of `ks` that have a slot in `s`, in order (what `batch_touch` collects).
#[logic(open)]
#[variant(ks.len())]
pub fn filter_l(ks: Seq<Key>, s: Seq<SlotModel>) -> Seq<Key> {
    pearlite! {
        if ks.len() <= 0 { Seq::empty() }
        else if slot_at(s, ks[ks.len() - 1]) { filter_l(ks.subsequence(0, ks.len() - 1), s).push_back(ks[ks.len() - 1]) }
        else { filter_l(ks.subsequence(0, ks.len() - 1), s) }
    }
}

#[logic]
#[requires(0 <= i && i < ks.len())]
#[ensures(touch_all_l(t, ks.subsequence(0, i + 1)) == touch_l(touch_all_l(t, ks.subsequence(0, i)), ks[i]))]
pub fn lem_ta_step(t: Seq<(Key, u64)>, ks: Seq<Key>, i: Int) {
    pearlite! { proof_assert!(ks.subsequence(0, i + 1).subsequence(0, i) == ks.subsequence(0, i)); () }
}

#[logic]
#[ensures(touch_all_l(t, f.push_back(k)) == touch_l(touch_all_l(t, f), k))]
pub fn lem_ta_push(t: Seq<(Key, u64)>, f: Seq<Key>, k: Key) {
    pearlite! { proof_assert!(f.push_back(k).subsequence(0, f.len()) == f); () }
}

#[logic]
#[requires(0 <= i && i < ks.len())]
#[ensures(filter_l(ks.subsequence(0, i + 1), s)
          == if slot_at(s, ks[i]) { filter_l(ks.subsequence(0, i), s).push_back(ks[i]) }
             else { filter_l(ks.subsequence(0, i), s) })]
pub fn lem_filter_step(ks: Seq<Key>, s: Seq<SlotModel>, i: Int) {
    pearlite! { proof_assert!(ks.subsequence(0, i + 1).subsequence(0, i) == ks.subsequence(0, i)); () }
}

/// `IEvictionPolicy::touch(handle)` -> `LruList::move_to_back`: the entry becomes
/// the newest. Mirrored as "remove the key's entry, append it again" — same
/// contents, new order; no key is tracked or untracked.
#[requires(policy_keys_unique(p.tracked@))]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures(forall<k: Key> tracks((^p).tracked@, k) == tracks(p.tracked@, k))]
#[ensures((^p).tracked@.len() == p.tracked@.len())]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == pool)]
// --- batch 2: functional order spec + "the refreshed entry is now the newest" ---
#[ensures((^p).tracked@ == touch_l(p.tracked@, key))]
#[ensures(tracks(p.tracked@, key) ==> (^p).tracked@.len() > 0
          && ((^p).tracked@[(^p).tracked@.len() - 1]).0 == key)]
pub fn policy_touch_key(p: &mut PolicyModel, pool: u64, key: Key) {
    p.last_pool = pool;
    let t0 = snapshot! { p.tracked@ };
    let n = p.tracked.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == p.tracked@.len())]
    #[invariant(p.tracked@ == *t0)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> (p.tracked@[j]).0 != key)]
    while i < n {
        if p.tracked[i].0 == key {
            let e = p.tracked[i];
            let old = snapshot! { p.tracked@ };
            let _ = p.tracked.remove(i);
            proof_assert!(forall<j: Int> 0 <= j && j < i@ ==> p.tracked@[j] == old[j]);
            proof_assert!(forall<j: Int> i@ <= j && j < p.tracked@.len() ==> p.tracked@[j] == old[j + 1]);
            proof_assert!(forall<k: Key> tracks(p.tracked@, k) ==> tracks(*old, k));
            proof_assert!(forall<j: Int> 0 <= j && j < old.len() && j != i@
                          ==> tracks(p.tracked@, (old[j]).0));
            proof_assert!(forall<k: Key> k != key ==> tracks(p.tracked@, k) == tracks(*old, k));
            proof_assert!(!tracks(p.tracked@, key));
            let mid = snapshot! { p.tracked@ };
            p.tracked.push(e);
            proof_assert!(forall<j: Int> 0 <= j && j < mid.len() ==> p.tracked@[j] == mid[j]);
            proof_assert!(forall<k: Key> tracks(p.tracked@, k) == (tracks(*mid, k) || k == key));
            proof_assert!(forall<k: Key> k != key ==> tracks(p.tracked@, k) == tracks(*old, k));
            proof_assert!(tracks(p.tracked@, key));
            snapshot! { lem_fi(*t0, key, i@) };
            proof_assert!(first_idx(*t0, key) == i@);
            proof_assert!(*old == *t0);
            proof_assert!(*mid == old.removed(i@));
            proof_assert!(p.tracked@ == mid.push_back(e));
            proof_assert!(e == old[i@]);
            proof_assert!(p.tracked@ == touch_l(*t0, key));
            proof_assert!(p.tracked@[p.tracked@.len() - 1] == e);
            return;
        }
        i += 1;
    }
    proof_assert!(p.tracked@ == *t0 && n@ == t0.len());
    proof_assert!(forall<j: Int> 0 <= j && j < t0.len() ==> (t0[j]).0 != key);
    snapshot! { lem_fi_none(*t0, key) };
    proof_assert!(first_idx(*t0, key) == t0.len());
    proof_assert!(p.tracked@ == touch_l(*t0, key));
}

// ===========================================================================
// 6. The 17 IMemoryTier methods (lib.rs:260-630)
// ===========================================================================

/// Arithmetic side conditions of the MIRROR's policy counters (`next_handle`,
/// `pools` are u64 counters of the policy model). The former `used + 2^32 <=
/// usize::MAX` conjunct is GONE (batch 1): `used` is now bounded by the
/// accounting invariant `acct_ok` (`used + free_sum == capacity`), so
/// `FreeList::allocate`'s `used += aligned_size` is proved overflow-free.
#[logic(open)]
pub fn arith_ok(st: StateModel) -> bool {
    pearlite! {
        st.policy.next_handle@ + 1 < u64::MAX@
        && st.policy.pools@ + 1 < u64::MAX@
    }
}

/// `IMemoryTier::initialize` (`lib.rs:261-321`).
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 ==> pool_size@ + 4096 <= usize::MAX@)]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
// --- MT-INV-INITIALIZE-ONCE-ONLY ---
#[ensures(st.initialized ==> (^st).initialized)]
#[ensures(st.initialized ==> match result { Err(_) => true, Ok(_) => false })]
#[ensures(st.initialized && pool_size@ > 0 ==> result == Err(MtError::AllocationFailed))]
#[ensures(pool_size@ > 0 && result != Ok(()) ==> result == Err(MtError::AllocationFailed))]
#[ensures(st.initialized ==> (^st).pool_base == st.pool_base)]
#[ensures(st.initialized ==> (^st).pool_size == st.pool_size)]
#[ensures(st.initialized ==> (^st).allocator.capacity == st.allocator.capacity)]
#[ensures(st.initialized ==> (^st).allocator.used == st.allocator.used)]
#[ensures(match result { Ok(_) => (^st).initialized, Err(_) => true })]
#[ensures(match result { Ok(_) => (^st).pool_size@ == pool_size@
                                  && (^st).allocator.capacity@ == pool_size@
                                  && (^st).allocator.used@ == 0, Err(_) => true })]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
// contract exposure: how much of the pool-id space the call consumed
#[ensures((^st).policy.pools@ <= st.policy.pools@ + 1)]
// MT-INIT-POST-POLICY-POOL: exactly one fresh policy pool, and its id is stored
#[ensures(st.initialized ==> (^st).pool_id == st.pool_id && (^st).policy.pools == st.policy.pools
          && (^st).policy.last_pool == st.policy.last_pool && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures(match result { Ok(_) => (^st).pool_id@ == st.policy.pools@
                                  && (^st).policy.pools@ == st.policy.pools@ + 1
                                  && (^st).policy.last_pool == (^st).pool_id,
                         Err(_) => (^st).policy.pools == st.policy.pools
                                  && (^st).pool_id == st.pool_id
                                  && (^st).policy.last_pool == st.policy.last_pool })]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
// --- batch 2: MT-INIT-FRAME-ON-FAILURE / MT-INIT-POST-READY / MT-INV-NEVER-NOT-EVICTABLE ---
#[ensures(match result { Ok(_) => true, Err(_) => ^st == *st })]
#[ensures(match result { Ok(_) => (^st).slots@ == Seq::empty(), Err(_) => true })]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
// --- batch 4: MT-INV-DEFAULT-POOL-SIZE-NEVER-APPLIED -- a zero request never falls back ---
#[ensures(pool_size@ == 0 ==> result == Err(MtError::InvalidSize))]
pub fn mt_initialize(st: &mut StateModel, pool_size: usize) -> Result<(), MtError> {
    if pool_size == 0 {
        return Err(MtError::InvalidSize);
    }
    if st.initialized {
        return Err(MtError::AllocationFailed);
    }
    // non-`spdk` build: `self.alloc_mmap(pool_size, numa_node)?` (lib.rs:310)
    let a = alloc_pool(pool_size);
    // the logger receptacle is optional; its presence is immaterial (MT-INV-LOGGER-OPTIONAL)
    let mut sink = LogSink { lines: 0 };
    init_logged(st, pool_size, a, &mut sink, false)
}

/// GHOST (batch 1): what a connected `ILogger` has received.
pub struct LogSink {
    pub lines: u64,
}

/// `MemoryTierComponent::log_info` / `log_warn` (lib.rs:157-167):
/// `if let Ok(logger) = self.logger.get() { logger.info(msg); }` -- a missing
/// logger is silently skipped. Takes NO component state at all.
#[ensures(connected ==> (^lg).lines@ == if lg.lines@ < u64::MAX@ { lg.lines@ + 1 } else { lg.lines@ })]
#[ensures(!connected ==> ^lg == *lg)]
pub fn log_msg(lg: &mut LogSink, connected: bool) {
    if connected && lg.lines < u64::MAX {
        lg.lines += 1;
    }
}

/// Field-wise equality of two component states (views for the containers).
#[logic(open)]
pub fn same_state(a: StateModel, b: StateModel) -> bool {
    pearlite! {
        a.pool_base == b.pool_base && a.pool_size == b.pool_size && a.pool_id == b.pool_id
        && a.allocator.regions@ == b.allocator.regions@ && a.allocator.capacity == b.allocator.capacity
        && a.allocator.used == b.allocator.used && a.slots@ == b.slots@
        && a.initialized == b.initialized && a.spdk_allocated == b.spdk_allocated
        && a.poisoned == b.poisoned && a.policy.tracked@ == b.policy.tracked@
        && a.policy.next_handle == b.policy.next_handle && a.policy.pools == b.policy.pools
        && a.policy.last_pool == b.policy.last_pool
    }
}

/// The part of `initialize` after the guards (lib.rs:308-329): allocation
/// outcome `a` (from mmap / spdk_zmalloc), `create_pool`, the stores, the
/// final log line. Every field of the result is a function of the inputs.
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 && pool_size@ + 4096 <= usize::MAX@)]
#[requires(forall<b: usize> a == Some(b) ==> b@ % 4096 == 0 && b@ > 0 && b@ + pool_size@ <= usize::MAX@)]
#[ensures(a == None ==> result == Err(MtError::AllocationFailed) && ^st == *st && ^lg == *lg)]
#[ensures(forall<b: usize> a == Some(b) ==> result == Ok(())
          && (^st).pool_base == b && (^st).pool_size == pool_size && (^st).pool_id == st.policy.pools
          && (^st).allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: pool_size })
          && (^st).allocator.capacity == pool_size && (^st).allocator.used@ == 0
          && (^st).slots@ == Seq::empty() && (^st).initialized && !(^st).spdk_allocated
          && (^st).poisoned == st.poisoned && (^st).policy.tracked@ == st.policy.tracked@
          && (^st).policy.next_handle == st.policy.next_handle
          && (^st).policy.pools@ == st.policy.pools@ + 1 && (^st).policy.last_pool == st.policy.pools)]
#[ensures(a != None && connected ==> (^lg).lines@ == if lg.lines@ < u64::MAX@ { lg.lines@ + 1 } else { lg.lines@ })]
#[ensures(!connected ==> ^lg == *lg)]
#[ensures(mt_inv(^st))]
pub fn init_logged(
    st: &mut StateModel,
    pool_size: usize,
    a: Option<usize>,
    lg: &mut LogSink,
    connected: bool,
) -> Result<(), MtError> {
    let base = match a {
        Some(b) => b,
        // lib.rs:221-222 `Err(AllocationFailed("mmap failed"))` -- before any store
        None => return Err(MtError::AllocationFailed),
    };
    let pool_id = policy_create_pool(&mut st.policy); // lib.rs:312
    verify_mt_init_post_flag_published_last(st, base, pool_size, pool_id); // lib.rs:314-322
    proof_assert!(fl_inv(st.allocator.regions@, st.allocator.capacity@));
    proof_assert!(key_inv(*st) && init_inv(*st) && slot_inv(st.slots@, st.allocator.capacity@));
    proof_assert!(acct_ok(*st));
    log_msg(lg, connected); // lib.rs:324-327
    Ok(())
}

/// "Built or hidden": while the set-up flag is clear nothing is promised; once
/// it is set, EVERY part of the pool is the finished one.
#[logic(open)]
pub fn built_or_hidden(st: StateModel, base: usize, size: usize, id: u64) -> bool {
    pearlite! {
        st.initialized ==>
            st.pool_base == base && st.pool_size == size && st.pool_id == id
            && st.allocator.capacity == size && st.allocator.used@ == 0
            && st.allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: size })
            && st.slots@.len() == 0 && !st.spdk_allocated
    }
}

/// **MT-INIT-POST-FLAG-PUBLISHED-LAST** (postcondition, code-only, 15 attachments)
///
/// "The flag that marks the component as set up is published only as the very
/// last step of setting up the pool, after the memory region, the pool size, the
/// policy pool identifier and the fresh allocator have all been stored, so no
/// other operation can ever observe the component as set up while its pool is
/// still half built."
///
/// This IS the store sequence of `initialize` (lib.rs:314-322, in source order;
/// `mt_initialize` calls it), with "set ==> fully built" checked after EVERY
/// store -- i.e. at every point another operation could in principle observe.
#[requires(!st.initialized)]
#[requires(size@ > 0)]
#[ensures((^st).initialized)]
#[ensures(built_or_hidden(^st, base, size, id))]
#[ensures((^st).pool_base == base && (^st).pool_size == size && (^st).pool_id == id)]
#[ensures((^st).allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: size }))]
#[ensures((^st).allocator.capacity == size && (^st).allocator.used@ == 0)]
#[ensures((^st).slots@ == Seq::empty() && !(^st).spdk_allocated)]
#[ensures((^st).poisoned == st.poisoned && (^st).policy == st.policy)]
pub fn verify_mt_init_post_flag_published_last(st: &mut StateModel, base: usize, size: usize, id: u64) {
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.pool_base = base; // lib.rs:314 state.pool_ptr = ptr
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.pool_size = size; // lib.rs:315
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.pool_id = id; // lib.rs:316
    proof_assert!(built_or_hidden(*st, base, size, id));
    // lib.rs:317-320 state.pool = RwLock::new(Pool { FreeList::new(pool_size), HashMap::new() })
    st.allocator = fl_new(size);
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.slots = Vec::new();
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.spdk_allocated = false; // lib.rs:321 (non-`spdk` build)
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.initialized = true; // lib.rs:322 initialized.store(true, Release) -- LAST
    proof_assert!(st.allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: size }));
    proof_assert!(built_or_hidden(*st, base, size, id));
}

/// Anti-vacuity twin: MUST FAIL -- the same stores with the flag published FIRST.
#[requires(!st.initialized)]
#[requires(size@ > 0)]
pub fn verify_mt_init_post_flag_published_last__mutant(st: &mut StateModel, base: usize, size: usize, id: u64) {
    st.initialized = true;
    proof_assert!(st.initialized && size@ > 0);
    proof_assert!(built_or_hidden(*st, base, size, id));
    st.pool_base = base;
    st.pool_size = size;
    st.pool_id = id;
    st.allocator = fl_new(size);
    st.slots = Vec::new();
    st.spdk_allocated = false;
}

/// `IMemoryTier::insert` (`lib.rs:352-383`).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
// frame for MT-INV-INITIALIZE-ONCE-ONLY
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
// MT-INSERT-POST-ALIGNMENT
#[ensures(forall<p: usize> result == Ok(p) ==> p@ % 4096 == 0)]
// MT-INSERT-ERR-POOL-FULL
#[ensures(st.initialized && size@ > 0 && !slot_at(st.slots@, key)
          && no_region_fits(st.allocator.regions@, align_up_l(size@))
          ==> match result { Err(MtError::PoolFull) => true, _ => false })]
#[ensures(match result { Err(MtError::PoolFull) =>
              no_region_fits(st.allocator.regions@, align_up_l(size@))
              && (^st).slots@ == st.slots@
              && (^st).allocator.regions@ == st.allocator.regions@
              && (^st).allocator.used@ == st.allocator.used@
              && (^st).policy.tracked@ == st.policy.tracked@, _ => true })]
// contract exposure: how much the call consumed
#[ensures((^st).allocator.used@ <= st.allocator.used@ + align_up_l(size@))]
#[ensures((^st).policy.next_handle@ <= st.policy.next_handle@ + 1)]
#[ensures((^st).policy.pools == st.policy.pools)]
#[ensures((^st).pool_id == st.pool_id && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures((^st).policy.last_pool == st.policy.last_pool || (^st).policy.last_pool == st.pool_id)]
// --- batch 1: an insertion that some free run can hold succeeds ---
#[ensures(st.initialized && size@ > 0 && !slot_at(st.slots@, key)
          && some_region_fits(st.allocator.regions@, align_up_l(size@))
          ==> match result { Ok(_) => true, _ => false })]
#[ensures(match result { Ok(_) => slot_at((^st).slots@, key)
          && (^st).allocator.used@ == st.allocator.used@ + align_up_l(size@), _ => true })]
#[ensures(match result { Ok(_) => true, _ => (^st).slots@ == st.slots@ })]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures(!st.initialized ==> match result { Err(_) => true, Ok(_) => false })]
// --- batch 2: frame on other keys, the duplicate check, the new slot record, error kinds ---
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
#[ensures(st.initialized && size@ > 0 && slot_at(st.slots@, key) ==> result == Err(MtError::AlreadyExists))]
#[ensures(match result { Ok(_) => !slot_at(st.slots@, key)
          && exists<sl: SlotModel> (^st).slots@ == st.slots@.push_back(sl) && sl.key == key && sl.size == size,
          _ => true })]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
// --- batch 3: the returned address IS base + the new slot's offset; first fit; error frame ---
#[ensures(forall<p: usize> result == Ok(p) ==> (^st).slots@.len() == st.slots@.len() + 1
          && (^st).slots@[st.slots@.len()].key == key && (^st).slots@[st.slots@.len()].size == size
          && p@ == st.pool_base@ + (^st).slots@[st.slots@.len()].offset@)]
#[ensures(forall<p: usize> result == Ok(p) ==> exists<i: Int> 0 <= i && i < st.allocator.regions@.len()
          && p@ == st.pool_base@ + st.allocator.regions@[i].offset@
          && st.allocator.regions@[i].size@ >= align_up_l(size@)
          && (forall<j: Int> 0 <= j && j < i ==> st.allocator.regions@[j].size@ < align_up_l(size@)))]
#[ensures(match result { Ok(_) => true, Err(_) => same_state(*st, ^st) })]
#[ensures(size@ == 0 ==> match result { Err(MtError::InvalidSize) => true, _ => false })]
// --- batch 5: MT-INSERT-ERR-NOT-INITIALIZED -- the error KIND ---
#[ensures(size@ > 0 && !st.initialized ==> match result { Err(MtError::NotInitialized) => true, _ => false })]
pub fn mt_insert(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    if size == 0 {
        return Err(MtError::InvalidSize);
    }
    if !st.initialized {
        return Err(MtError::NotInitialized);
    }
    if slots_contains(&st.slots, key) {
        return Err(MtError::AlreadyExists);
    }
    let s0 = snapshot! { st.slots@ };
    let r0 = snapshot! { st.allocator.regions@ };
    let u0 = snapshot! { st.allocator.used@ };
    let offset = match fl_allocate(&mut st.allocator, size) {
        Some(o) => o,
        None => {
            proof_assert!(!some_region_fits(*r0, align_up_l(size@)));
            return Err(MtError::PoolFull);
        }
    };
    proof_assert!(st.allocator.used@ == *u0 + align_up_l(size@));
    let pid = st.pool_id;
    let handle = policy_track(&mut st.policy, pid, key);
    let ns = SlotModel {
        key,
        offset,
        size,
        handle,
    };
    snapshot! { lem_ss_push(*s0, ns) };
    snapshot! { lem_al(size@) };
    st.slots.push(ns);
    proof_assert!(st.slots@ == s0.push_back(ns));
    proof_assert!(forall<j: Int> 0 <= j && j < s0.len() ==> st.slots@[j] == s0[j]);
    proof_assert!(forall<j: Int> 0 <= j && j < s0.len()
                  ==> disj(s0[j].offset@, sl_end(s0[j]), offset@, offset@ + align_up_l(size@)));
    proof_assert!(st.slots@[s0.len()] == ns);
    proof_assert!(forall<k2: Key> slot_at(st.slots@, k2) == (slot_at(*s0, k2) || k2 == key));
    proof_assert!(slot_keys_unique(st.slots@));
    proof_assert!(slot_stable(*s0, st.slots@));
    proof_assert!(key_inv(*st));
    proof_assert!(slots_disj(*s0));
    let fin = snapshot! { st.slots@ };
    proof_assert!(forall<i: Int, j: Int> 0 <= i && i < s0.len() && 0 <= j && j < s0.len() && fin[i].key != fin[j].key
        ==> disj(fin[i].offset@, sl_end(fin[i]), fin[j].offset@, sl_end(fin[j])));
    proof_assert!(forall<i: Int> 0 <= i && i < s0.len()
        ==> disj(fin[i].offset@, sl_end(fin[i]), fin[s0.len()].offset@, sl_end(fin[s0.len()])));
    proof_assert!(forall<i: Int> 0 <= i && i < s0.len()
        ==> disj(fin[s0.len()].offset@, sl_end(fin[s0.len()]), fin[i].offset@, sl_end(fin[i])));
    proof_assert!(slots_disj(st.slots@));
    proof_assert!(slots_clear(st.allocator.regions@, st.slots@));
    proof_assert!(slot_at(st.slots@, key));
    proof_assert!(fl_inv(st.allocator.regions@, st.allocator.capacity@));
    proof_assert!(init_inv(*st));
    proof_assert!(slot_inv(st.slots@, st.allocator.capacity@));
    proof_assert!(acct_ok(*st));
    proof_assert!(mt_inv(*st));
    proof_assert!(st.slots@ == s0.push_back(ns) && ns.key == key && ns.size == size);
    proof_assert!(st.slots@.len() == s0.len() + 1 && st.slots@[s0.len()].offset == offset);
    let _ = r0;
    Ok(st.pool_base + offset)
}

/// What a lookup / refresh may change: only the ORDER of the policy's tracked
/// entries (plus the ghost `last_pool`). Everything memory-tier owns is frozen.
#[logic(open)]
pub fn refresh_frame(a: StateModel, b: StateModel) -> bool {
    pearlite! {
        b.slots@ == a.slots@
        && b.allocator.regions@ == a.allocator.regions@
        && b.allocator.used == a.allocator.used
        && b.allocator.capacity == a.allocator.capacity
        && b.initialized == a.initialized
        && b.pool_base == a.pool_base
        && b.pool_size == a.pool_size
        && b.pool_id == a.pool_id
        && b.poisoned == a.poisoned
        && b.spdk_allocated == a.spdk_allocated
        && b.policy.next_handle == a.policy.next_handle
        && b.policy.pools == a.policy.pools
        && b.policy.tracked@.len() == a.policy.tracked@.len()
        && (forall<k: Key> tracks(b.policy.tracked@, k) == tracks(a.policy.tracked@, k))
        && policy_keys_unique(b.policy.tracked@)
        && (b.policy.last_pool == a.policy.last_pool || b.policy.last_pool == a.pool_id)
    }
}

/// `IMemoryTier::get` (`lib.rs:385-414`): look the slot up, release the pool
/// lock, then `ep.touch(handle)` (an LRU move-to-back, mirrored by key).
#[requires(mt_inv(*st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures(forall<p: usize, s: u32> result == Some((p, s)) ==> p@ % 4096 == 0)]
#[ensures(match result { Some(_) => st.initialized, None => true })]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> result != None)]
#[ensures(!slot_at(st.slots@, key) ==> result == None)]
#[ensures(forall<p: usize, s: u32> result == Some((p, s)) ==> exists<i: Int> 0 <= i && i < st.slots@.len()
          && st.slots@[i].key == key && p@ == st.pool_base@ + st.slots@[i].offset@ && s == st.slots@[i].size)]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
// --- batch 2 ---
#[ensures((^st).policy.tracked@ == if st.initialized && slot_at(st.slots@, key) {
              touch_l(st.policy.tracked@, key) } else { st.policy.tracked@ })]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> (^st).policy.tracked@.len() > 0
          && ((^st).policy.tracked@[(^st).policy.tracked@.len() - 1]).0 == key)]
pub fn mt_get(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    if !st.initialized {
        return None;
    }
    let n = st.slots.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == st.slots@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> st.slots@[j].key != key)]
    while i < n {
        let sl = st.slots[i];
        if sl.key == key {
            let r = (st.pool_base + sl.offset, sl.size);
            let pid = st.pool_id;
            policy_touch_key(&mut st.policy, pid, key);
            return Some(r);
        }
        i += 1;
    }
    None
}

/// `IMemoryTier::peek` (`lib.rs:416-426`): the same lookup, NO policy call.
#[requires(mt_inv(*st))]
#[ensures(forall<p: usize, s: u32> result == Some((p, s)) ==> p@ % 4096 == 0)]
#[ensures(match result { Some(_) => st.initialized, None => true })]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> result != None)]
#[ensures(!slot_at(st.slots@, key) ==> result == None)]
#[ensures(forall<p: usize, s: u32> result == Some((p, s)) ==> exists<i: Int> 0 <= i && i < st.slots@.len()
          && st.slots@[i].key == key && p@ == st.pool_base@ + st.slots@[i].offset@ && s == st.slots@[i].size)]
pub fn mt_peek(st: &StateModel, key: Key) -> Option<(usize, u32)> {
    if !st.initialized {
        return None;
    }
    let n = st.slots.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> st.slots@[j].key != key)]
    while i < n {
        let sl = st.slots[i];
        if sl.key == key {
            return Some((st.pool_base + sl.offset, sl.size));
        }
        i += 1;
    }
    None
}

/// `IMemoryTier::oldest_keys` -> `ep.get_eviction_candidates(pool_id, n)`
/// (`lib.rs:428-436`) -> `LruList::peek_front_n`.
#[requires(mt_inv(*st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures((^st).policy.tracked == st.policy.tracked)]
#[ensures(mt_inv(^st))]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> tracks(st.policy.tracked@, result@[i]))]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i] == (st.policy.tracked@[i]).0)]
#[ensures(st.initialized && n@ > 0 && st.policy.tracked@.len() > 0 ==> result@.len() >= 1)]
#[ensures(result@.len() <= n@)]
#[ensures(!st.initialized ==> result@.len() == 0)]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures(st.initialized && n@ > 0 ==> result@.len() == if n@ < st.policy.tracked@.len() { n@ } else { st.policy.tracked@.len() })]
// --- batch 6: MT-OLDEST-KEYS-ERR-EMPTY -- n == 0 returns before the policy call ---
#[ensures(n@ == 0 ==> result@ == Seq::empty() && ^st == *st)]
#[ensures(!st.initialized ==> result@ == Seq::empty())]
// --- batch 6: MT-OLDEST-KEYS-POST-SHAPE -- the one policy call is for THIS pool ---
#[ensures(st.initialized && n@ > 0 ==> (^st).policy.last_pool == st.pool_id)]
pub fn mt_oldest_keys(st: &mut StateModel, n: usize) -> Vec<Key> {
    if !st.initialized || n == 0 {
        return Vec::new();
    }
    let pid = st.pool_id;
    policy_candidates(&mut st.policy, pid, n)
}

/// `IMemoryTier::evict_next` (`lib.rs:434-464`).
#[requires(mt_inv(*st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
#[ensures((^st).policy.pools == st.policy.pools)]
// --- MT-EVICT-NEXT-POST-UNTRACKED ---
#[ensures(forall<k: Key> result == Some(k) ==> !tracks((^st).policy.tracked@, k))]
#[ensures(forall<k: Key> result == Some(k) ==> !slot_at((^st).slots@, k))]
#[ensures(forall<k: Key> result == Some(k) ==> tracks(st.policy.tracked@, k))]
#[ensures(forall<k: Key> tracks((^st).policy.tracked@, k) ==> tracks(st.policy.tracked@, k))]
#[ensures(forall<k: Key> slot_at((^st).slots@, k) ==> slot_at(st.slots@, k))]
// --- batch 1: MT-EVICT-NEXT-POST-ENTRY-GONE-AND-SPACE-FREED ---
#[ensures(forall<k: Key> result == Some(k) ==> slot_at(st.slots@, k))]
#[ensures(forall<k: Key> result == Some(k) ==> st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0)]
#[ensures(forall<k: Key> result == Some(k) ==> exists<i: Int> 0 <= i && i < st.slots@.len()
          && st.slots@[i].key == k
          && (^st).allocator.used@ == st.allocator.used@ - align_up_l(st.slots@[i].size@)
          && some_region_fits((^st).allocator.regions@, align_up_l(st.slots@[i].size@)))]
#[ensures(forall<k: Key> slot_at((^st).slots@, k) == (slot_at(st.slots@, k) && result != Some(k)))]
#[ensures(result == None ==> (^st).slots@ == st.slots@ && (^st).allocator.used == st.allocator.used)]
#[ensures(st.initialized && st.slots@.len() > 0 ==> result != None)]
#[ensures((^st).pool_id == st.pool_id && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures((^st).policy.last_pool == st.policy.last_pool || (^st).policy.last_pool == st.pool_id)]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures(!st.initialized ==> result == None)]
// --- batch 4: MT-EVICT-NEXT-ERR-NO-VICTIM -- a `None` answer changes nothing observable ---
#[ensures(result == None ==> (^st).allocator.regions@ == st.allocator.regions@
          && (^st).policy.tracked@ == st.policy.tracked@)]
pub fn mt_evict_next(st: &mut StateModel) -> Option<Key> {
    if !st.initialized {
        return None;
    }
    let pid = st.pool_id;
    proof_assert!(st.slots@.len() > 0 ==> slot_at(st.slots@, st.slots@[0].key));
    proof_assert!(st.slots@.len() > 0 ==> st.policy.tracked@.len() > 0);
    match policy_identify_next_to_evict(&mut st.policy, pid) {
        None => None,
        Some(key) => {
            let s0 = snapshot! { st.slots@ };
            match slots_remove(&mut st.slots, key) {
                Some(slot) => {
                    snapshot! { lem_al(slot.size@) };
                    proof_assert!(dealloc_pre(st.allocator, slot.offset@, align_up_l(slot.size@)));
                    fl_deallocate(&mut st.allocator, slot.offset, slot.size);
                    proof_assert!(forall<j: Int> 0 <= j && j < st.slots@.len()
                        ==> disj(st.slots@[j].offset@, sl_end(st.slots@[j]), slot.offset@, sl_end(slot)));
                    proof_assert!(slots_clear(st.allocator.regions@, st.slots@));
                }
                None => (),
            }
            let _ = s0;
            Some(key)
        }
    }
}

/// `IMemoryTier::evict_next_for_key` (`lib.rs:466-468`) — delegates verbatim.
#[requires(mt_inv(*st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
#[ensures((^st).policy.pools == st.policy.pools)]
#[ensures(forall<k: Key> result == Some(k) ==> !tracks((^st).policy.tracked@, k))]
#[ensures((^st).pool_id == st.pool_id && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures((^st).policy.last_pool == st.policy.last_pool || (^st).policy.last_pool == st.pool_id)]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
// --- batch 3: the delegated contract, exposed ---
#[ensures(forall<k: Key> result == Some(k) ==> st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0)]
#[ensures(forall<k: Key> result == Some(k) ==> slot_at(st.slots@, k))]
#[ensures(result == None ==> (^st).slots@ == st.slots@ && (^st).allocator.used == st.allocator.used)]
#[ensures(!st.initialized ==> result == None)]
#[ensures(st.initialized && st.slots@.len() > 0 ==> result != None)]
// --- batch 4: more of the delegated contract, exposed (ERR-NO-VICTIM / ALIAS / SPACE-GLOBAL) ---
#[ensures(result == None ==> (^st).allocator.regions@ == st.allocator.regions@
          && (^st).policy.tracked@ == st.policy.tracked@)]
#[ensures(forall<k: Key> slot_at((^st).slots@, k) == (slot_at(st.slots@, k) && result != Some(k)))]
#[ensures(forall<k: Key> result == Some(k) ==> exists<i: Int> 0 <= i && i < st.slots@.len()
          && st.slots@[i].key == k
          && (^st).allocator.used@ == st.allocator.used@ - align_up_l(st.slots@[i].size@)
          && some_region_fits((^st).allocator.regions@, align_up_l(st.slots@[i].size@)))]
pub fn mt_evict_next_for_key(st: &mut StateModel, _key: Key) -> Option<Key> {
    mt_evict_next(st)
}

/// `IMemoryTier::remove` (`lib.rs:470-501`).
#[requires(mt_inv(*st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
#[ensures((^st).policy.pools == st.policy.pools)]
#[ensures(st.initialized ==> !tracks((^st).policy.tracked@, key) && !slot_at((^st).slots@, key))]
// --- batch 1: MT-REMOVE-POST-GONE-AND-FREED ---
#[ensures(st.initialized && slot_at(st.slots@, key) ==> match result { Ok(_) => true, Err(_) => false })]
#[ensures(match result { Ok(_) => exists<i: Int> 0 <= i && i < st.slots@.len()
          && st.slots@[i].key == key
          && (^st).allocator.used@ == st.allocator.used@ - align_up_l(st.slots@[i].size@)
          && some_region_fits((^st).allocator.regions@, align_up_l(st.slots@[i].size@)),
          Err(_) => (^st).slots@ == st.slots@ && (^st).allocator.used == st.allocator.used })]
#[ensures(forall<k: Key> k != key ==> slot_at((^st).slots@, k) == slot_at(st.slots@, k))]
#[ensures((^st).pool_id == st.pool_id && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures((^st).policy.last_pool == st.policy.last_pool || (^st).policy.last_pool == st.pool_id)]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures(!st.initialized ==> match result { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
// --- batch 2: MT-INV-FREE-LIST-COALESCING -- the released run is merged with a free neighbour ---
#[ensures(match result { Ok(_) => exists<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == key
    && covered((^st).allocator.regions@, st.slots@[i].offset@, sl_end(st.slots@[i]))
    && (forall<j: Int> 0 <= j && j < st.allocator.regions@.len()
        && st.allocator.regions@[j].offset@ + st.allocator.regions@[j].size@ == st.slots@[i].offset@
        ==> covered((^st).allocator.regions@, st.allocator.regions@[j].offset@, sl_end(st.slots@[i])))
    && (forall<j: Int> 0 <= j && j < st.allocator.regions@.len()
        && st.allocator.regions@[j].offset@ == sl_end(st.slots@[i])
        ==> covered((^st).allocator.regions@, st.slots@[i].offset@,
                    st.allocator.regions@[j].offset@ + st.allocator.regions@[j].size@)),
    Err(_) => true })]
// --- batch 6: MT-REMOVE-ERR-KEY-NOT-FOUND -- the absent key returns before ep.remove / deallocate ---
#[ensures(st.initialized && !slot_at(st.slots@, key) ==> result == Err(MtError::KeyNotFound) && same_state(*st, ^st))]
pub fn mt_remove(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    if !st.initialized {
        return Err(MtError::NotInitialized);
    }
    // lib.rs:500-506: `slots.remove(&key).ok_or(KeyNotFound)?` returns BEFORE
    // `ep.remove(..)` is reached, so the absent-key path touches nothing.
    match slots_remove(&mut st.slots, key) {
        None => Err(MtError::KeyNotFound),
        Some(slot) => {
            let pid = st.pool_id;
            policy_remove_key(&mut st.policy, pid, key);
            snapshot! { lem_al(slot.size@) };
            proof_assert!(dealloc_pre(st.allocator, slot.offset@, align_up_l(slot.size@)));
            let fr0 = snapshot! { st.allocator.regions@ };
            fl_deallocate(&mut st.allocator, slot.offset, slot.size);
            proof_assert!(forall<j: Int> 0 <= j && j < st.slots@.len()
                ==> disj(st.slots@[j].offset@, sl_end(st.slots@[j]), slot.offset@, sl_end(slot)));
            proof_assert!(slots_clear(st.allocator.regions@, st.slots@));
            proof_assert!(covered(st.allocator.regions@, slot.offset@, sl_end(slot)));
            proof_assert!(forall<j: Int> 0 <= j && j < fr0.len()
                && fr0[j].offset@ + fr0[j].size@ == slot.offset@
                ==> covered(st.allocator.regions@, fr0[j].offset@, sl_end(slot)));
            proof_assert!(forall<j: Int> 0 <= j && j < fr0.len() && fr0[j].offset@ == sl_end(slot)
                ==> covered(st.allocator.regions@, slot.offset@, fr0[j].offset@ + fr0[j].size@));
            Ok(())
        }
    }
}

/// `IMemoryTier::touch` (`lib.rs:509-522`) — reorders the policy only.
#[requires(mt_inv(*st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(mt_inv(^st))]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
// --- batch 2 ---
#[ensures((^st).policy.tracked@ == if st.initialized && slot_at(st.slots@, key) {
              touch_l(st.policy.tracked@, key) } else { st.policy.tracked@ })]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> (^st).policy.tracked@.len() > 0
          && ((^st).policy.tracked@[(^st).policy.tracked@.len() - 1]).0 == key)]
// --- batch 6: MT-TOUCH-SILENT-NOOP -- an absent key: no policy call, nothing at all changes ---
#[ensures(!slot_at(st.slots@, key) ==> ^st == *st)]
pub fn mt_touch(st: &mut StateModel, key: Key) {
    if !st.initialized {
        return;
    }
    if slots_contains(&st.slots, key) {
        let pid = st.pool_id;
        policy_touch_key(&mut st.policy, pid, key);
    }
}

/// `IEvictionPolicy::batch_touch(&handles)` (eviction-policy-lru: early `Ok` on
/// an empty batch, else `move_to_back` per handle), mirrored per key.
#[requires(policy_keys_unique(p.tracked@))]
#[ensures(policy_keys_unique((^p).tracked@))]
#[ensures(forall<k: Key> tracks((^p).tracked@, k) == tracks(p.tracked@, k))]
#[ensures((^p).tracked@.len() == p.tracked@.len())]
#[ensures((^p).next_handle == p.next_handle)]
#[ensures((^p).pools == p.pools)]
#[ensures((^p).last_pool == p.last_pool || (^p).last_pool == pool)]
#[ensures((^p).tracked@ == touch_all_l(p.tracked@, keys@))]
pub fn policy_batch_touch(p: &mut PolicyModel, pool: u64, keys: &Vec<Key>) {
    let n = keys.len();
    let mut i: usize = 0;
    let p0 = snapshot! { *p };
    #[invariant(i@ <= n@)]
    #[invariant(n@ == keys@.len())]
    #[invariant(p.tracked@ == touch_all_l(p0.tracked@, keys@.subsequence(0, i@)))]
    #[invariant(policy_keys_unique(p.tracked@))]
    #[invariant(forall<k: Key> tracks(p.tracked@, k) == tracks(p0.tracked@, k))]
    #[invariant(p.tracked@.len() == p0.tracked@.len())]
    #[invariant(p.next_handle == p0.next_handle && p.pools == p0.pools)]
    #[invariant(p.last_pool == p0.last_pool || p.last_pool == pool)]
    while i < n {
        snapshot! { lem_ta_step(p0.tracked@, keys@, i@) };
        policy_touch_key(p, pool, keys[i]);
        i += 1;
    }
    proof_assert!(keys@.subsequence(0, n@) == keys@);
}

/// `IMemoryTier::batch_touch` (`lib.rs:524-559`): early return on an empty batch
/// or an un-set-up pool; collect the handles of the keys present under the pool
/// READ lock; release it; one `ep.batch_touch` call.
#[requires(mt_inv(*st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
// --- batch 2 ---
#[ensures(st.initialized ==> (^st).policy.tracked@ == touch_all_l(st.policy.tracked@, filter_l(keys@, st.slots@)))]
// --- batch 4: MT-BATCH-TOUCH-EMPTY-NOOP -- the empty batch returns before anything ---
#[ensures(keys@.len() == 0 ==> ^st == *st)]
pub fn mt_batch_touch(st: &mut StateModel, keys: &[Key]) {
    if keys.len() == 0 {
        return;
    }
    if !st.initialized {
        return;
    }
    let n = keys.len();
    let mut handles: Vec<Key> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == keys@.len())]
    #[invariant(handles@ == filter_l(keys@.subsequence(0, i@), st.slots@))]
    while i < n {
        snapshot! { lem_filter_step(keys@, st.slots@, i@) };
        if slots_contains(&st.slots, keys[i]) {
            handles.push(keys[i]);
        }
        i += 1;
    }
    proof_assert!(keys@.subsequence(0, n@) == keys@);
    let pid = st.pool_id;
    policy_batch_touch(&mut st.policy, pid, &handles);
}

/// `IMemoryTier::contains` (`lib.rs:551-559`).
#[requires(mt_inv(*st))]
#[ensures(result == (st.initialized && slot_at(st.slots@, key)))]
pub fn mt_contains(st: &StateModel, key: Key) -> bool {
    if !st.initialized {
        return false;
    }
    slots_contains(&st.slots, key)
}

/// `IMemoryTier::capacity` (`lib.rs:561-568`).
#[requires(mt_inv(*st))]
#[ensures(st.initialized ==> result@ == st.allocator.capacity@)]
#[ensures(!st.initialized ==> result@ == 0)]
pub fn mt_capacity(st: &StateModel) -> usize {
    if !st.initialized {
        return 0;
    }
    st.allocator.capacity
}

/// `IMemoryTier::used` (`lib.rs:570-577`).
#[requires(mt_inv(*st))]
#[ensures(st.initialized ==> result@ == st.allocator.used@)]
#[ensures(!st.initialized ==> result@ == 0)]
pub fn mt_used(st: &StateModel) -> usize {
    if !st.initialized {
        return 0;
    }
    st.allocator.used
}

/// `IMemoryTier::pool_info` (`lib.rs:579-586`).
#[requires(mt_inv(*st))]
#[ensures(forall<b: usize, s: usize> result == Some((b, s)) ==> b@ % 4096 == 0 && b@ > 0)]
#[ensures(result == if st.initialized && st.pool_base@ != 0 { Some((st.pool_base, st.pool_size)) } else { None })]
pub fn mt_pool_info(st: &StateModel) -> Option<(usize, usize)> {
    if st.initialized && st.pool_base != 0 {
        Some((st.pool_base, st.pool_size))
    } else {
        None
    }
}

/// `IMemoryTier::clear` (`lib.rs:588-603`).
#[requires(mt_inv(*st))]
#[ensures(fl_inv((^st).allocator.regions@, (^st).allocator.capacity@))]
#[ensures(key_inv(^st))]
#[ensures(init_inv(^st))]
#[ensures(slot_inv((^st).slots@, (^st).allocator.capacity@))]
#[ensures(acct_ok(^st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
#[ensures((^st).initialized == st.initialized)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).pool_size == st.pool_size)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
#[ensures((^st).policy.pools == st.policy.pools)]
#[ensures(st.initialized ==> (^st).allocator.used@ == 0
          && (^st).allocator.capacity@ == st.pool_size@
          && (^st).slots@.len() == 0)]
#[ensures((^st).pool_id == st.pool_id && (^st).spdk_allocated == st.spdk_allocated)]
#[ensures((^st).policy.last_pool == st.policy.last_pool || (^st).policy.last_pool == st.pool_id)]
// --- batch 1: MT-CLEAR-POST-EMPTY ---
#[ensures(st.initialized ==> (^st).allocator.regions@.len() == 1
          && (^st).allocator.regions@[0].offset@ == 0
          && (^st).allocator.regions@[0].size@ == st.pool_size@)]
#[ensures(st.initialized ==> (^st).policy.tracked@.len() == 0)]
#[ensures(st.initialized ==> match result { Ok(c) => c@ == st.slots@.len(), Err(_) => false })]
#[ensures(!st.initialized ==> match result { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
pub fn mt_clear(st: &mut StateModel) -> Result<usize, MtError> {
    if !st.initialized {
        return Err(MtError::NotInitialized);
    }
    let count = st.slots.len();
    st.slots = Vec::new();
    st.allocator = fl_new(st.pool_size);
    let pid = st.pool_id;
    policy_clear_pool(&mut st.policy, pid);
    Ok(count)
}

/// `IMemoryTier::is_dma_capable` (`lib.rs:605-608`).
#[ensures(result == st.spdk_allocated)]
pub fn mt_is_dma_capable(st: &StateModel) -> bool {
    st.spdk_allocated
}

/// `IMemoryTier::telemetry_snapshot` (`lib.rs:610-630`).
pub fn mt_telemetry_snapshot(st: &StateModel) -> u64 {
    st.pool_id
}

// ===========================================================================
// 7. The lock-acquisition prologue shared by all 17 methods
// ===========================================================================

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Call {
    Panicked,
    Completed,
}

/// Batch 6: a set-up pool's id names a pool the bound policy created (`create_pool` hands out
/// `0, 1, 2, ..`, so "created" is `pool_id < pools`).
#[logic(open)]
pub fn pid_ok(st: StateModel) -> bool {
    pearlite! { st.initialized ==> st.pool_id@ < st.policy.pools@ }
}

/// One mirrored `IMemoryTier` call, WITH the lock acquisition that every public
/// method of the component performs and `.unwrap()`s
/// (`lib.rs:270, 333, 354, 367, 428, 438, 480, 494, 506, 523, 553, 563, 572,
/// 581, 592, 606, 613`).
#[requires(st.poisoned || (mt_inv(*st) && arith_ok(*st)
           && (psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)))]
#[ensures(mt_inv(*st) ==> mt_inv(^st))]
#[ensures((^st).poisoned == st.poisoned)]
#[ensures(mt_inv(*st) ==> slot_stable(st.slots@, (^st).slots@))]
// contract exposure for chained callers (batch 1)
#[ensures(st.initialized ==> (^st).initialized)]
#[ensures(st.initialized ==> (^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size
          && (^st).pool_id == st.pool_id && (^st).policy.pools == st.policy.pools)]
#[ensures((^st).policy.next_handle@ <= st.policy.next_handle@ + 1)]
#[ensures((^st).policy.pools@ <= st.policy.pools@ + 1)]
#[ensures(st.initialized ==> (^st).allocator.capacity == st.allocator.capacity)]
#[ensures(st.initialized ==> (^st).policy.last_pool == st.policy.last_pool
          || (^st).policy.last_pool == st.pool_id)]
// batch 4 (MT-DROP-POST-UNMAP): the pool's provenance flag never changes once set up
#[ensures(st.initialized ==> (^st).spdk_allocated == st.spdk_allocated)]
// batch 6 (MT-IS-DMA-CAPABLE-NO-INIT-GUARD / MT-INSERT-TRACK-FAILURE-PANICS): while the pool stays
// un-set-up the provenance flag is untouched; a set-up pool's id names a pool the policy created
#[ensures(!(^st).initialized ==> (^st).spdk_allocated == st.spdk_allocated)]
#[ensures(pid_ok(*st) ==> pid_ok(^st))]
// --- MT-INV-LOCK-POISONING-CASCADE: once poisoned, the call panics and the
//     whole component state is left exactly as it was (no recovery path) ---
#[ensures(st.poisoned ==> match result { Call::Panicked => true, _ => false })]
#[ensures(st.poisoned ==> (^st).poisoned
          && (^st).initialized == st.initialized
          && (^st).pool_base == st.pool_base
          && (^st).pool_size == st.pool_size
          && (^st).slots@ == st.slots@
          && (^st).policy.tracked@ == st.policy.tracked@
          && (^st).policy.next_handle == st.policy.next_handle
          && (^st).policy.pools == st.policy.pools
          && (^st).allocator.regions@ == st.allocator.regions@
          && (^st).allocator.used == st.allocator.used
          && (^st).allocator.capacity == st.allocator.capacity)]
pub fn mt_call(st: &mut StateModel, op: u8, key: Key, size: u32, psz: usize) -> Call {
    // `self.state.read().unwrap()` / `state.pool.write().unwrap()`
    if !lock_ok(st.poisoned) {
        // `.unwrap()` on `Err(PoisonError)` panics -> the call never returns.
        return Call::Panicked;
    }
    if op == 0 {
        let _ = mt_initialize(st, psz);
    } else if op == 1 {
        let _ = mt_insert(st, key, size);
    } else if op == 2 {
        let _ = mt_get(st, key);
    } else if op == 3 {
        let _ = mt_peek(st, key);
    } else if op == 4 {
        let _ = mt_evict_next(st);
    } else if op == 5 {
        let _ = mt_evict_next_for_key(st, key);
    } else if op == 6 {
        let _ = mt_oldest_keys(st, size as usize);
    } else if op == 7 {
        let _ = mt_remove(st, key);
    } else if op == 8 {
        mt_touch(st, key);
    } else if op == 9 {
        mt_batch_touch(st, &[key]);
    } else if op == 10 {
        let _ = mt_contains(st, key);
    } else if op == 11 {
        let _ = mt_capacity(st);
    } else if op == 12 {
        let _ = mt_used(st);
    } else if op == 13 {
        let _ = mt_pool_info(st);
    } else if op == 14 {
        let _ = mt_clear(st);
    } else if op == 15 {
        let _ = mt_is_dma_capable(st);
    } else {
        let _ = mt_telemetry_snapshot(st);
    }
    Call::Completed
}

// ===========================================================================
// 8. Property drivers — one why3 module per inventory property id
// ===========================================================================

/// **MT-INV-INITIALIZE-ONCE-ONLY** (invariant, 17 attachments)
///
/// "Setting up the pool is a one-way step: the component has exactly two states,
/// not set up and set up, the only legal transition is from the first to the
/// second exactly once, nothing ever clears the flag, and its capacity and base
/// address never change afterwards."
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(!st.poisoned)]
#[requires(sz1@ > 0 && sz1@ + 4096 <= usize::MAX@)]
#[requires(sz2@ > 0 && sz2@ + 4096 <= usize::MAX@)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[requires(st.policy.next_handle@ + 2 < u64::MAX@)]
#[ensures(result ==> (^st).initialized && (^st).pool_size@ == sz1@)]
pub fn verify_mt_inv_initialize_once_only(
    st: &mut StateModel,
    sz1: usize,
    sz2: usize,
    k: Key,
    size: u32,
) -> bool {
    // State A: not set up.
    proof_assert!(!st.initialized);
    let r1 = mt_initialize(st, sz1);
    match r1 {
        Ok(()) => {}
        Err(_) => return false, // mmap/spdk_zmalloc failed; no transition happened
    }
    // The one legal transition has happened.
    proof_assert!(st.initialized);
    let base0 = snapshot! { st.pool_base };
    let size0 = snapshot! { st.pool_size };
    let cap0 = snapshot! { st.allocator.capacity };
    proof_assert!(st.allocator.used@ == 0);

    // ... exactly once: a second attempt must fail and change nothing.
    let r2 = mt_initialize(st, sz2);
    proof_assert!(match r2 {
        Err(_) => true,
        Ok(_) => false,
    });
    proof_assert!(st.initialized);
    proof_assert!(st.pool_base == *base0 && st.pool_size == *size0);
    proof_assert!(st.allocator.capacity == *cap0);

    // ... and no later call on the component clears the flag or moves the pool.
    let _ = mt_insert(st, k, size);
    let _ = mt_evict_next(st);
    let _ = mt_evict_next_for_key(st, k);
    let _ = mt_remove(st, k);
    let _ = mt_clear(st);
    let _ = mt_get(st, k);
    let _ = mt_peek(st, k);
    let _ = mt_contains(st, k);
    let _ = mt_capacity(st);
    let _ = mt_used(st);
    let _ = mt_pool_info(st);
    let _ = mt_oldest_keys(st, 4);
    mt_touch(st, k);
    mt_batch_touch(st, &[k]);
    let _ = mt_is_dma_capable(st);
    let _ = mt_telemetry_snapshot(st);
    proof_assert!(st.initialized);
    proof_assert!(st.pool_base == *base0 && st.pool_size == *size0);
    proof_assert!(st.allocator.capacity == *cap0);
    true
}

/// Anti-vacuity twin of `verify_mt_inv_initialize_once_only`: MUST FAIL.
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(!st.poisoned)]
#[requires(sz1@ > 0 && sz1@ + 4096 <= usize::MAX@)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[ensures(result ==> !(^st).initialized)]
pub fn verify_mt_inv_initialize_once_only__mutant(st: &mut StateModel, sz1: usize) -> bool {
    match mt_initialize(st, sz1) {
        Ok(()) => {}
        Err(_) => return false,
    }
    true
}

/// **MT-INSERT-POST-ALIGNMENT** (postcondition, spec-only, 3 attachments)
///
/// "Every address handed back when adding a cache entry starts on a
/// four-kibibyte boundary, so the memory can be used directly as the source or
/// destination of a disk transfer without any extra copying or realignment."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.allocator.used@ + 4295000000 <= usize::MAX@)]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result == Ok(p) ==> p@ % 4096 == 0)]
pub fn verify_mt_insert_post_alignment(
    st: &mut StateModel,
    key: Key,
    size: u32,
) -> Result<usize, MtError> {
    proof_assert!(st.pool_base@ % 4096 == 0);
    let r = mt_insert(st, key, size);
    // the same address is handed back by every later lookup of the entry
    let g = mt_get(st, key);
    proof_assert!(forall<p: usize, sz: u32> g == Some((p, sz)) ==> p@ % 4096 == 0);
    let pk = mt_peek(st, key);
    proof_assert!(forall<p: usize, sz: u32> pk == Some((p, sz)) ==> p@ % 4096 == 0);
    r
}

/// Anti-vacuity twin of `verify_mt_insert_post_alignment`: MUST FAIL.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.allocator.used@ + 4295000000 <= usize::MAX@)]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result == Ok(p) ==> p@ % 4096 == 1)]
pub fn verify_mt_insert_post_alignment__mutant(
    st: &mut StateModel,
    key: Key,
    size: u32,
) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-INSERT-ERR-POOL-FULL** (error-case, 1 attachment)
///
/// "When no single free run of pool memory is large enough for the rounded
/// request, adding a cache entry fails with the pool-full error rather than
/// returning a short or overlapping block, and this can happen even when the
/// total amount of free space would have been sufficient had it not been
/// fragmented."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(size@ > 0)]
#[requires(!slot_at(st.slots@, key))]
#[requires(no_region_fits(st.allocator.regions@, align_up_l(size@)))]
#[requires(st.allocator.used@ + 4295000000 <= usize::MAX@)]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result {
    Err(MtError::PoolFull) => true,
    _ => false,
})]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).allocator.regions@ == st.allocator.regions@)]
#[ensures((^st).allocator.used@ == st.allocator.used@)]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
pub fn verify_mt_insert_err_pool_full(
    st: &mut StateModel,
    key: Key,
    size: u32,
) -> Result<usize, MtError> {
    // "... even when the total amount of free space would have been sufficient
    // had it not been fragmented": a concrete witness. 8192 free bytes in two
    // non-adjacent 4096-byte runs; an 8192-byte request still fails.
    let mut frag = FreeListModel {
        regions: Vec::new(),
        capacity: 16384,
        used: 8192,
    };
    frag.regions.push(Region {
        offset: 0,
        size: 4096,
    });
    frag.regions.push(Region {
        offset: 8192,
        size: 4096,
    });
    proof_assert!(align_up_l(8192) == 8192);
    let w = fl_allocate(&mut frag, 8192u32);
    proof_assert!(w == None);
    proof_assert!(frag.capacity@ - frag.used@ == 8192);

    mt_insert(st, key, size)
}

/// Anti-vacuity twin of `verify_mt_insert_err_pool_full`: MUST FAIL.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(size@ > 0)]
#[requires(!slot_at(st.slots@, key))]
#[requires(no_region_fits(st.allocator.regions@, align_up_l(size@)))]
#[requires(st.allocator.used@ + 4295000000 <= usize::MAX@)]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result {
    Ok(_) => true,
    _ => false,
})]
pub fn verify_mt_insert_err_pool_full__mutant(
    st: &mut StateModel,
    key: Key,
    size: u32,
) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-EVICT-NEXT-POST-UNTRACKED** (postcondition, 3 attachments)
///
/// "An evicted key is also removed from the external eviction-policy component's
/// bookkeeping, so it can no longer appear in the list of oldest keys and cannot
/// be nominated as a victim a second time."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(forall<k: Key> result == Some(k) ==> !tracks((^st).policy.tracked@, k))]
#[ensures(forall<k: Key> result == Some(k) ==> !slot_at((^st).slots@, k))]
pub fn verify_mt_evict_next_post_untracked(st: &mut StateModel) -> Option<Key> {
    let v = mt_evict_next(st);
    // "... so it can no longer appear in the list of oldest keys ..."
    let oldest = mt_oldest_keys(st, 64);
    proof_assert!(forall<k: Key, i: Int> v == Some(k) && 0 <= i && i < oldest@.len()
                  ==> oldest@[i] != k);
    // "... and cannot be nominated as a victim a second time."
    let v2 = mt_evict_next(st);
    proof_assert!(forall<k: Key> v == Some(k) ==> v2 != Some(k));
    v
}

/// Anti-vacuity twin of `verify_mt_evict_next_post_untracked`: MUST FAIL.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(forall<k: Key> result == Some(k) ==> tracks((^st).policy.tracked@, k))]
pub fn verify_mt_evict_next_post_untracked__mutant(st: &mut StateModel) -> Option<Key> {
    mt_evict_next(st)
}

/// **MT-INV-LOCK-POISONING-CASCADE** (invariant, code-only, 17 attachments)
///
/// "Every lock acquisition in the component unwraps its result, so once any
/// thread panics while holding either the component-state lock or the inner pool
/// lock, every later call on the component panics too and there is no way to
/// recover the pool."
#[requires(mt_inv(*st))]
#[requires(arith_ok(*st))]
#[requires(!st.poisoned)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).poisoned)]
pub fn verify_mt_inv_lock_poisoning_cascade(
    st: &mut StateModel,
    ops: &[u8],
    key: Key,
    size: u32,
    psz: usize,
) {
    // A panic unwinds out of a held guard: both RwLocks are poisoned from here on.
    panic_while_holding(st);
    proof_assert!(st.poisoned);
    // From here EVERY call on the component panics, for ANY sequence of calls over
    // ANY of the 17 interface methods, and nothing recovers the pool.
    let n = ops.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(st.poisoned)]
    while i < n {
        let c = mt_call(st, ops[i], key, size, psz);
        proof_assert!(match c {
            Call::Panicked => true,
            Call::Completed => false,
        });
        proof_assert!(st.poisoned);
        i += 1;
    }
    proof_assert!(st.poisoned);
}

/// Anti-vacuity twin of `verify_mt_inv_lock_poisoning_cascade`: MUST FAIL.
#[requires(mt_inv(*st))]
#[requires(arith_ok(*st))]
#[requires(!st.poisoned)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(!(^st).poisoned)]
pub fn verify_mt_inv_lock_poisoning_cascade__mutant(
    st: &mut StateModel,
    ops: &[u8],
    key: Key,
    size: u32,
    psz: usize,
) {
    panic_while_holding(st);
    let n = ops.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(st.poisoned)]
    while i < n {
        let _ = mt_call(st, ops[i], key, size, psz);
        i += 1;
    }
}

// ===========================================================================
// 9. Batch 1 — shared drivers
// ===========================================================================

/// ANY finite sequence of IMemoryTier calls `(op, key, size)` (op selects one of
/// the 17 methods, see `mt_call`), each through the lock-acquisition prologue.
/// Proved by loop invariant, so it covers every reachable state from `*st`.
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(mt_inv(^st))]
#[ensures(!(^st).poisoned)]
#[ensures(st.initialized ==> (^st).initialized)]
#[ensures(st.initialized ==> (^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size
          && (^st).pool_id == st.pool_id && (^st).policy.pools == st.policy.pools
          && (^st).allocator.capacity == st.allocator.capacity)]
#[ensures(st.initialized && st.policy.last_pool == st.pool_id ==> (^st).policy.last_pool == st.pool_id)]
#[ensures((^st).policy.next_handle@ <= st.policy.next_handle@ + ops@.len())]
#[ensures((^st).policy.pools@ <= st.policy.pools@ + ops@.len())]
#[ensures(st.initialized ==> (^st).spdk_allocated == st.spdk_allocated)]
// batch 6
#[ensures(!(^st).initialized ==> (^st).spdk_allocated == st.spdk_allocated)]
#[ensures(pid_ok(*st) ==> pid_ok(^st))]
pub fn mt_run(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    let st0 = snapshot! { *st };
    let n = ops.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(mt_inv(*st))]
    #[invariant(!st.poisoned)]
    #[invariant(st.policy.next_handle@ <= st0.policy.next_handle@ + i@)]
    #[invariant(st.policy.pools@ <= st0.policy.pools@ + i@)]
    #[invariant(st0.initialized ==> st.initialized)]
    #[invariant(st0.initialized ==> st.pool_base == st0.pool_base && st.pool_size == st0.pool_size
                && st.pool_id == st0.pool_id && st.policy.pools == st0.policy.pools
                && st.allocator.capacity == st0.allocator.capacity)]
    #[invariant(st0.initialized && st0.policy.last_pool == st0.pool_id ==> st.policy.last_pool == st0.pool_id)]
    #[invariant(st0.initialized ==> st.spdk_allocated == st0.spdk_allocated)]
    #[invariant(!st.initialized ==> st.spdk_allocated == st0.spdk_allocated)]
    #[invariant(pid_ok(*st0) ==> pid_ok(*st))]
    while i < n {
        let (op, key, size) = ops[i];
        let _ = mt_call(st, op, key, size, psz);
        i += 1;
    }
}

/// **MT-INV-USED-WITHIN-CAPACITY** (invariant, 8 attachments)
///
/// "At every point in the component's life the number of bytes reported as in use
/// lies between zero and the pool's total capacity inclusive, because space is
/// only ever marked used when it has been carved out of a free run that lies
/// inside the pool."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(0 <= result.0@ && result.0@ <= result.1@)]
#[ensures(0 <= (^st).allocator.used@ && (^st).allocator.used@ <= (^st).allocator.capacity@)]
pub fn verify_mt_inv_used_within_capacity(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) -> (usize, usize) {
    mt_run(st, ops, psz);
    proof_assert!(forall<j: Int> 0 <= j && j < st.allocator.regions@.len() ==> st.allocator.regions@[j].size@ > 0);
    snapshot! { lem_fs_bound(st.allocator.regions@) };
    proof_assert!(free_sum(st.allocator.regions@) >= 0);
    proof_assert!(free_sum(st.allocator.regions@) + st.allocator.used@ == st.allocator.capacity@);
    proof_assert!(st.allocator.used@ <= st.allocator.capacity@);
    let u = mt_used(st);
    let c = mt_capacity(st);
    (u, c)
}

/// Anti-vacuity twin: MUST FAIL (claims the pool can never be completely full).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).allocator.used@ < (^st).allocator.capacity@)]
pub fn verify_mt_inv_used_within_capacity__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
    snapshot! { lem_fs_bound(st.allocator.regions@) };
}

/// **MT-INV-NO-LEAKED-OR-DOUBLE-BOOKED-SPACE** (invariant, 8 attachments)
///
/// "Every byte of the pool is either free or belongs to exactly one entry currently
/// in the cache, so the sizes of all free runs plus the bytes-in-use total always
/// equal the capacity and the bytes-in-use total is always exactly the sum of the
/// rounded sizes of the entries held; repeatedly adding and removing entries can
/// therefore never lose space permanently and never hand the same space out twice."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// free runs + in-use == capacity
#[ensures(free_sum((^st).allocator.regions@) + (^st).allocator.used@ == (^st).allocator.capacity@)]
// in-use == sum of the rounded sizes of the entries held
#[ensures((^st).allocator.used@ == slot_sum((^st).slots@))]
// a byte belongs to at most one entry ...
#[ensures(forall<i: Int, j: Int, b: Int> 0 <= i && i < (^st).slots@.len() && 0 <= j && j < (^st).slots@.len()
          && i != j && (^st).slots@[i].offset@ <= b && b < sl_end((^st).slots@[i])
          ==> !((^st).slots@[j].offset@ <= b && b < sl_end((^st).slots@[j])))]
// ... and never to an entry AND the free list at once (no double booking)
#[ensures(forall<i: Int, j: Int, b: Int> 0 <= i && i < (^st).slots@.len()
          && 0 <= j && j < (^st).allocator.regions@.len()
          && (^st).slots@[i].offset@ <= b && b < sl_end((^st).slots@[i])
          ==> !((^st).allocator.regions@[j].offset@ <= b
                && b < (^st).allocator.regions@[j].offset@ + (^st).allocator.regions@[j].size@))]
// no permanent loss: with no entry held, all of the capacity is free again
#[ensures((^st).slots@.len() == 0 ==> free_sum((^st).allocator.regions@) == (^st).allocator.capacity@)]
pub fn verify_mt_inv_no_leaked_or_double_booked_space(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) {
    mt_run(st, ops, psz);
    proof_assert!(slot_keys_unique(st.slots@));
    proof_assert!(st.slots@.len() == 0 ==> slot_sum(st.slots@) == 0);
}

/// Anti-vacuity twin: MUST FAIL (claims in-use counts each entry's UNROUNDED size).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(free_sum((^st).allocator.regions@) + (^st).allocator.used@ < (^st).allocator.capacity@)]
pub fn verify_mt_inv_no_leaked_or_double_booked_space__mutant(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) {
    mt_run(st, ops, psz);
}

/// **MT-INV-ENTRIES-DISJOINT** (invariant, 7 attachments)
///
/// "No two entries that are in the cache at the same time occupy overlapping byte
/// ranges, so writing through the address obtained for one entry can never
/// corrupt the data of another."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(k1 != k2)]
// the byte ranges [p, p + size) handed out for two different live keys never overlap
#[ensures(forall<p1: usize, s1: u32, p2: usize, s2: u32> result == (Some((p1, s1)), Some((p2, s2)))
          ==> p1@ + s1@ <= p2@ || p2@ + s2@ <= p1@)]
#[ensures(forall<i: Int, j: Int> 0 <= i && i < (^st).slots@.len() && 0 <= j && j < (^st).slots@.len() && i != j
          ==> disj((^st).slots@[i].offset@, sl_end((^st).slots@[i]),
                   (^st).slots@[j].offset@, sl_end((^st).slots@[j])))]
pub fn verify_mt_inv_entries_disjoint(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
    k1: Key,
    k2: Key,
) -> (Option<(usize, u32)>, Option<(usize, u32)>) {
    mt_run(st, ops, psz);
    proof_assert!(slot_keys_unique(st.slots@));
    proof_assert!(forall<i: Int> 0 <= i && i < st.slots@.len() ==> st.slots@[i].size@ <= align_up_l(st.slots@[i].size@));
    let a = mt_peek(st, k1);
    let b = mt_peek(st, k2);
    (a, b)
}

/// Anti-vacuity twin: MUST FAIL (claims two live entries may never be ADJACENT).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<i: Int, j: Int> 0 <= i && i < (^st).slots@.len() && 0 <= j && j < (^st).slots@.len() && i != j
          ==> sl_end((^st).slots@[i]) < (^st).slots@[j].offset@ || sl_end((^st).slots@[j]) < (^st).slots@[i].offset@)]
pub fn verify_mt_inv_entries_disjoint__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
    proof_assert!(slot_keys_unique(st.slots@));
}

/// **MT-INV-POLICY-AND-POOL-AGREE** (invariant, 8 attachments, divergence note)
///
/// "Every key the eviction policy tracks for this pool corresponds to an entry the
/// cache actually holds and every entry the cache holds is tracked by the policy,
/// since otherwise eviction can free nothing, report phantom victims, or never
/// reach a real entry."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<k: Key> tracks((^st).policy.tracked@, k) == slot_at((^st).slots@, k))]
// consequences: the policy's victim is always a real entry (no phantom victim),
// evicting it really frees space, and a non-empty cache always yields a victim
#[ensures(forall<k: Key> result.2 == Some(k) ==> result.1@ < result.0@)]
#[ensures(result.3 ==> result.2 != None)]
pub fn verify_mt_inv_policy_and_pool_agree(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) -> (usize, usize, Option<Key>, bool) {
    mt_run(st, ops, psz);
    proof_assert!(forall<k: Key> tracks(st.policy.tracked@, k) == slot_at(st.slots@, k));
    let nonempty = st.slots.len() > 0;
    proof_assert!(nonempty ==> slot_at(st.slots@, st.slots@[0].key));
    let before = mt_used(st);
    let s0 = snapshot! { st.slots@ };
    let v = mt_evict_next(st);
    // no phantom victim: whatever the policy named was an entry the cache held
    proof_assert!(forall<k: Key> v == Some(k) ==> slot_at(*s0, k));
    let after = mt_used(st);
    (before, after, v, nonempty)
}

/// Anti-vacuity twin: MUST FAIL (claims the policy may track a key with NO entry).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(exists<k: Key> tracks((^st).policy.tracked@, k) && !slot_at((^st).slots@, k))]
pub fn verify_mt_inv_policy_and_pool_agree__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
}

/// **MT-INV-INITIALIZED-IMPLIES-USABLE-POOL** (invariant, code-only, 17 attachments)
///
/// "Whenever the component reports itself as set up, its stored base address is
/// non-null and its stored pool size is greater than zero, because a zero size is
/// rejected up front and neither allocation path can return a null address."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).initialized ==> (^st).pool_base@ != 0 && (^st).pool_size@ > 0)]
#[ensures((^st).initialized ==> result != None)]
pub fn verify_mt_inv_initialized_implies_usable_pool(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) -> Option<(usize, usize)> {
    // the zero-size rejection is the first statement of initialize (lib.rs:266)
    mt_run(st, ops, psz);
    // pool_info() therefore always reports the pool once set up (lib.rs:591)
    mt_pool_info(st)
}

/// Anti-vacuity twin: MUST FAIL (claims a set-up pool's base can be null).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).initialized ==> (^st).pool_base@ == 0)]
pub fn verify_mt_inv_initialized_implies_usable_pool__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
}

/// **MT-INV-POOL-BASE-STABLE** (invariant, divergent, 9 attachments)
///
/// "Once the pool exists its starting address and total size stay the same no
/// matter what sequence of insertions, lookups, deletions, evictions or wipes is
/// performed, and the capacity the component reports is always the same as the
/// size of the memory region it actually mapped."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size)]
#[ensures(result.0 == Some((st.pool_base, st.pool_size)))]
#[ensures(result.1 == st.pool_size)]
pub fn verify_mt_inv_pool_base_stable(
    st: &mut StateModel,
    ops: &[(u8, Key, u32)],
    psz: usize,
) -> (Option<(usize, usize)>, usize) {
    let info0 = mt_pool_info(st);
    proof_assert!(info0 == Some((st.pool_base, st.pool_size)));
    mt_run(st, ops, psz);
    let info = mt_pool_info(st);
    let cap = mt_capacity(st);
    (info, cap)
}

/// Anti-vacuity twin: MUST FAIL (claims the reported capacity can differ from the mapped size).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result != st.pool_size)]
pub fn verify_mt_inv_pool_base_stable__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) -> usize {
    mt_run(st, ops, psz);
    mt_capacity(st)
}

/// **MT-FREELIST-USED-POST** (postcondition, code-only, 7 attachments)
///
/// "The bytes-used figure a free list reports is the running total of the rounded
/// sizes of all runs currently allocated from it."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result@ == slot_sum((^st).slots@))]
pub fn verify_mt_freelist_used_post(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) -> usize {
    mt_run(st, ops, psz);
    mt_used(st)
}

/// Anti-vacuity twin: MUST FAIL (claims used() is the sum of the UNROUNDED sizes).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result@ < slot_sum((^st).slots@))]
pub fn verify_mt_freelist_used_post__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) -> usize {
    mt_run(st, ops, psz);
    mt_used(st)
}

/// **MT-EVICT-NEXT-POST-ENTRY-GONE-AND-SPACE-FREED** (postcondition, divergent, 8 attachments)
///
/// "After an entry is evicted that key is no longer reported as present and
/// looking it up reports absence, and its memory is given back to the pool so the
/// reported bytes in use drop by the space it occupied and a later insertion that
/// needs that much room succeeds where it would previously have failed."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> !result.1 && result.2 == None && result.3 == None)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> result.5@ + align_up_l(result.4@) == result.6@)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> match result.7 { Ok(_) => true, Err(_) => false })]
#[ensures(st.slots@.len() > 0 ==> result.0 != None)]
pub fn verify_mt_evict_next_post_entry_gone_and_space_freed(
    st: &mut StateModel,
) -> (Option<Key>, bool, Option<(usize, u32)>, Option<(usize, u32)>, u32, usize, usize, Result<usize, MtError>) {
    // the victim the policy will name, and the size it occupies
    let t0 = snapshot! { st.policy.tracked@ };
    let cand = mt_oldest_keys(st, 1);
    proof_assert!(st.policy.tracked@ == *t0);
    proof_assert!(t0.len() > 0 ==> cand@.len() == 1);
    let used0 = mt_used(st);
    let mut sz: u32 = 0;
    if cand.len() == 1 {
        match mt_peek(st, cand[0]) {
            Some((_, s)) => sz = s,
            None => {}
        }
    }
    proof_assert!(st.policy.tracked@ == *t0);
    let v = mt_evict_next(st);
    proof_assert!(v != None ==> t0.len() > 0);
    let mut present = false;
    let mut g = None;
    let mut pk = None;
    let used1 = mt_used(st);
    let mut ins: Result<usize, MtError> = Err(MtError::InvalidSize);
    match v {
        Some(k) => {
            proof_assert!(cand@.len() == 1 && cand@[0] == k);
            present = mt_contains(st, k);
            pk = mt_peek(st, k);
            g = mt_get(st, k);
            // a later insertion needing exactly the room the victim occupied
            ins = mt_insert(st, k, sz);
        }
        None => {}
    }
    (v, present, g, pk, sz, used1, used0, ins)
}

/// Anti-vacuity twin: MUST FAIL (claims eviction leaves used() unchanged).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> result.1 == result.2)]
pub fn verify_mt_evict_next_post_entry_gone_and_space_freed__mutant(st: &mut StateModel) -> (Option<Key>, usize, usize) {
    let u0 = mt_used(st);
    let v = mt_evict_next(st);
    let u1 = mt_used(st);
    (v, u1, u0)
}

/// **MT-REMOVE-POST-GONE-AND-FREED** (postcondition, 7 attachments)
///
/// "Deleting an entry that is present reports success, after which the key is no
/// longer reported as present and looking it up reports absence, the reported
/// bytes in use drop by the space that entry occupied, and a later insertion
/// needing that much room succeeds."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(slot_at(st.slots@, key))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => false })]
#[ensures(!result.1 && result.2 == None && result.3 == None)]
#[ensures(result.5@ + align_up_l(result.4@) == result.6@)]
#[ensures(match result.7 { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_remove_post_gone_and_freed(
    st: &mut StateModel,
    key: Key,
) -> (Result<(), MtError>, bool, Option<(usize, u32)>, Option<(usize, u32)>, u32, usize, usize, Result<usize, MtError>) {
    let used0 = mt_used(st);
    let sz = match mt_peek(st, key) {
        Some((_, s)) => s,
        None => 0,
    };
    let r = mt_remove(st, key);
    let present = mt_contains(st, key);
    let pk = mt_peek(st, key);
    let g = mt_get(st, key);
    let used1 = mt_used(st);
    let ins = mt_insert(st, key, sz);
    (r, present, g, pk, sz, used1, used0, ins)
}

/// Anti-vacuity twin: MUST FAIL (claims the key is still present after a successful remove).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(slot_at(st.slots@, key))]
#[ensures(result)]
pub fn verify_mt_remove_post_gone_and_freed__mutant(st: &mut StateModel, key: Key) -> bool {
    let _ = mt_remove(st, key);
    mt_contains(st, key)
}

/// **MT-FREELIST-DEALLOCATE-POST** (postcondition, code-only, 6 attachments)
///
/// "Releasing a run returns its rounded number of bytes to the free space and
/// reduces the bytes-used total by that same amount, making those bytes available
/// to later allocations."
#[requires(dealloc_pre(*fl, offset@, align_up_l(size@)))]
#[requires(size@ > 0)]
#[requires(fl.used@ + free_sum(fl.regions@) == fl.capacity@)]
#[ensures(result != None)]
#[ensures((^fl).used@ == fl.used@)]
pub fn verify_mt_freelist_deallocate_post(fl: &mut FreeListModel, offset: usize, size: u32) -> Option<usize> {
    let u0 = snapshot! { fl.used@ };
    let f0 = snapshot! { free_sum(fl.regions@) };
    fl_deallocate(fl, offset, size);
    // its rounded size is back on the free list ...
    proof_assert!(free_sum(fl.regions@) == *f0 + align_up_l(size@));
    // ... and the bytes-used total fell by exactly the same amount
    proof_assert!(fl.used@ == *u0 - align_up_l(size@));
    proof_assert!(fl_inv(fl.regions@, fl.capacity@));
    // the released bytes now lie inside ONE free run (coalesced), so ...
    proof_assert!(some_region_fits(fl.regions@, align_up_l(size@)));
    // ... a later allocation of that size succeeds
    fl_allocate(fl, size)
}

/// Anti-vacuity twin: MUST FAIL (claims the release leaves the bytes-used total unchanged).
#[requires(dealloc_pre(*fl, offset@, align_up_l(size@)))]
#[requires(size@ > 0)]
#[ensures((^fl).used@ == fl.used@)]
pub fn verify_mt_freelist_deallocate_post__mutant(fl: &mut FreeListModel, offset: usize, size: u32) {
    fl_deallocate(fl, offset, size);
}

/// **MT-FREELIST-DEALLOCATE-PRE** (precondition, code-only, 6 attachments)
///
/// "The caller of the release operation must pass the exact starting offset of a
/// run that is currently allocated together with the exact size that was
/// originally requested for it; this rule is stated only in a comment and is never
/// checked by the code."
///
/// Two halves. (1) DISCHARGED at both call sites: `mt_evict_next` (lib.rs:461) and
/// `mt_remove` (lib.rs:505) each `proof_assert!(dealloc_pre(..))` immediately
/// before `fl_deallocate(slot.offset, slot.size)` -- the slot just removed from
/// the table, whose run is allocated and not free (`acct_ok`). Those asserts are
/// goals of the `mt_evict_next` / `mt_remove` modules (named in the advisory
/// evidence). (2) NEVER CHECKED -- proved here: a call that violates it (a run
/// that is FREE, never allocated) is accepted silently and corrupts the list.
#[ensures(result.0@ == usize::MAX@ + 1 - 4096)]
#[ensures(result.1@ == 1 && result.2@ == 4096)]
pub fn verify_mt_freelist_deallocate_pre() -> (usize, usize, usize) {
    let mut fl = fl_new(8192);
    proof_assert!(fl.regions@.len() == 1 && fl.regions@[0].offset@ == 0 && fl.regions@[0].size@ == 8192);
    snapshot! { lem_al(4096) };
    // [0, 4096) is FREE: dealloc_pre is violated (the run is not allocated, used < 4096)
    proof_assert!(!dealloc_pre(fl, 0, 4096));
    fl_deallocate(&mut fl, 0, 4096u32);
    // nothing refused it: the bytes-used total wrapped, and the free list silently
    // LOST [4096, 8192) (the run at offset 0 was overwritten with size 4096)
    let n = fl.regions.len();
    let sz = if n == 1 { fl.regions[0].size } else { 0 };
    (fl.used, n, sz)
}

/// Anti-vacuity twin: MUST FAIL (claims the violating release left the list intact).
#[ensures(result@ == 8192)]
pub fn verify_mt_freelist_deallocate_pre__mutant() -> usize {
    let mut fl = fl_new(8192);
    snapshot! { lem_al(4096) };
    fl_deallocate(&mut fl, 0, 4096u32);
    if fl.regions.len() == 1 { fl.regions[0].size } else { 0 }
}

/// **MT-FREELIST-DEALLOCATE-UNDERFLOW** (error-case, code-only, 6 attachments)
///
/// "The release operation subtracts from the bytes-used total as its very first
/// action, before it looks at the free runs at all and without ever checking that
/// the run being released was actually allocated or that the total is large
/// enough, so an unmatched release makes the bytes-used total wrap round to an
/// enormous number."
#[requires(sorted_by_offset(fl.regions@))]
#[requires(in_cap(fl.regions@, fl.capacity@))]
#[requires(size@ > 0)]
#[requires(offset@ + align_up_l(size@) <= fl.capacity@)]
#[requires(fl.used@ < align_up_l(size@))]
// for EVERY free-run state (the outcome on `used` does not depend on the runs at all):
#[ensures((^fl).used@ == fl.used@ - align_up_l(size@) + usize::MAX@ + 1)]
#[ensures((^fl).used@ >= usize::MAX@ + 1 - fl.capacity@)]
#[ensures((^fl).used@ > (^fl).capacity@ || fl.capacity@ * 2 > usize::MAX@)]
pub fn verify_mt_freelist_deallocate_underflow(fl: &mut FreeListModel, offset: usize, size: u32) {
    fl_deallocate(fl, offset, size);
}

/// Anti-vacuity twin: MUST FAIL (claims an unmatched release leaves `used` small).
#[requires(sorted_by_offset(fl.regions@))]
#[requires(in_cap(fl.regions@, fl.capacity@))]
#[requires(size@ > 0)]
#[requires(offset@ + align_up_l(size@) <= fl.capacity@)]
#[requires(fl.used@ < align_up_l(size@))]
#[ensures((^fl).used@ <= (^fl).capacity@)]
pub fn verify_mt_freelist_deallocate_underflow__mutant(fl: &mut FreeListModel, offset: usize, size: u32) {
    fl_deallocate(fl, offset, size);
}

/// **MT-FREELIST-DEALLOCATE-NO-DOUBLE-FREE-CHECK** (error-case, code-only, 6 attachments)
///
/// "Nothing detects the same run being released twice: the second release
/// decrements the bytes-used total again and merges an already-free region into
/// the free runs, after which the same bytes can be handed out to two different
/// callers at once."
#[ensures(result.0@ == usize::MAX@ + 1 - 4096)]
#[ensures(result.1 == Some(0usize) && result.2 == Some(0usize))]
#[ensures(result.3@ == 0)]
pub fn verify_mt_freelist_deallocate_no_double_free_check() -> (usize, Option<usize>, Option<usize>, usize) {
    snapshot! { lem_al(4096) };
    // --- W1: a literal double release of one run (one-page free list) ---
    let mut f1 = fl_new(4096);
    let a = fl_allocate(&mut f1, 4096u32);
    proof_assert!(a == Some(0usize));
    snapshot! { lem_fs_bound(f1.regions@) };
    proof_assert!(f1.regions@.len() == 0);
    fl_deallocate(&mut f1, 0, 4096u32);          // first release: legitimate
    proof_assert!(f1.used@ == 0);
    let r1 = snapshot! { f1.regions@ };
    proof_assert!(exists<j: Int> 0 <= j && j < r1.len() && r1[j].offset@ == 0 && r1[j].size@ == 4096);
    fl_deallocate(&mut f1, 0, 4096u32);          // second release of the SAME run
    // nothing detected it: the already-free run was re-inserted over itself ...
    proof_assert!(f1.regions@ == *r1);
    // ... and the bytes-used total was decremented AGAIN (wrapped, release build)
    let used_w1 = f1.used;

    // --- W2: the same run released twice with a reuse in between ---
    let mut f2 = fl_new(4096);
    let x = fl_allocate(&mut f2, 4096u32);       // caller A gets [0, 4096)
    proof_assert!(x == Some(0usize));
    snapshot! { lem_fs_bound(f2.regions@) };
    fl_deallocate(&mut f2, 0, 4096u32);          // A releases it
    let c = fl_allocate(&mut f2, 4096u32);       // caller C now owns [0, 4096)
    proof_assert!(c == Some(0usize));
    snapshot! { lem_fs_bound(f2.regions@) };
    fl_deallocate(&mut f2, 0, 4096u32);          // A releases its run AGAIN -- undetected
    // the bytes-used total says 0 although C still holds 4096 bytes
    let used_while_c_live = f2.used;
    let d = fl_allocate(&mut f2, 4096u32);       // caller D is handed [0, 4096) too
    (used_w1, c, d, used_while_c_live)
}

/// Anti-vacuity twin: MUST FAIL (claims the second caller gets different bytes).
#[ensures(result.0 != result.1)]
pub fn verify_mt_freelist_deallocate_no_double_free_check__mutant() -> (Option<usize>, Option<usize>) {
    snapshot! { lem_al(4096) };
    let mut f2 = fl_new(4096);
    let _ = fl_allocate(&mut f2, 4096u32);
    snapshot! { lem_fs_bound(f2.regions@) };
    fl_deallocate(&mut f2, 0, 4096u32);
    let c = fl_allocate(&mut f2, 4096u32);
    snapshot! { lem_fs_bound(f2.regions@) };
    fl_deallocate(&mut f2, 0, 4096u32);
    let d = fl_allocate(&mut f2, 4096u32);
    (c, d)
}

/// **MT-FREELIST-NEW-POST** (postcondition, code-only, 6 attachments) -- REFUTED.
///
/// Statement: "A newly created free list records the requested capacity, reports
/// zero bytes used, and holds exactly one free run starting at offset zero and
/// spanning the whole capacity, which is why a freshly set-up or freshly wiped
/// pool can satisfy a single request as large as its entire capacity."
///
/// NEGATION, proved: (a) a free list created with a capacity that is not a
/// multiple of 4096 (initialize accepts any pool_size > 0, lib.rs:266) does hold
/// one run [0, capacity) -- but a single request as large as that capacity is
/// REFUSED, because allocate rounds the request UP to 4096 (allocator.rs:42) while
/// the run is not rounded; (b) `FreeList::new(0)` (what `Default` builds,
/// lib.rs:109) holds NO run at all (allocator.rs:18 `if capacity > 0`).
#[ensures(result.0 == None)]
#[ensures(result.1@ == 5000 && result.2@ == 1)]
#[ensures(result.3@ == 0)]
pub fn refute_mt_freelist_new_post() -> (Option<usize>, usize, usize, usize) {
    snapshot! { lem_al(5000) };
    let mut fl = fl_new(5000);
    let cap = fl.capacity;
    let runs = fl.regions.len();
    proof_assert!(fl.regions@[0].offset@ == 0 && fl.regions@[0].size@ == 5000 && fl.used@ == 0);
    proof_assert!(align_up_l(5000) == 8192);
    proof_assert!(no_region_fits(fl.regions@, align_up_l(5000)));
    // a single request as large as the entire capacity
    let r = fl_allocate(&mut fl, 5000u32);
    let empty = fl_new(0);
    (r, cap, runs, empty.regions.len())
}

/// Anti-vacuity twin: MUST FAIL (claims the full-capacity request succeeds).
#[ensures(result != None)]
pub fn refute_mt_freelist_new_post__mutant() -> Option<usize> {
    snapshot! { lem_al(5000) };
    let mut fl = fl_new(5000);
    fl_allocate(&mut fl, 5000u32)
}

/// Companion (NOT a scored module): the statement DOES hold for every non-zero
/// capacity that is a multiple of 4096 and fits a u32 request.
#[requires(c@ > 0 && c@ % 4096 == 0)]
#[ensures(result == Some(0usize))]
pub fn witness_mt_freelist_new_post_aligned(c: u32) -> Option<usize> {
    snapshot! { lem_al(c@) };
    let mut fl = fl_new(c as usize);
    proof_assert!(fl.regions@.len() == 1 && fl.capacity@ == c@ && fl.used@ == 0);
    proof_assert!(some_region_fits(fl.regions@, align_up_l(c@)));
    let r = fl_allocate(&mut fl, c);
    proof_assert!(forall<o: usize> r == Some(o) ==> o@ + c@ <= c@);
    r
}

/// **MT-CLEAR-POST-EMPTY** (postcondition, 7 attachments) -- REFUTED.
///
/// Statement: "After the cache is wiped no key is reported as present, every
/// previously cached key looks up as absent, the reported number of bytes in use
/// is zero, and all the memory is back in one single unbroken free run so that a
/// single insertion as large as the entire pool capacity can succeed."
///
/// NEGATION, proved from the real entry point: set the pool up with a size that
/// is not a multiple of 4096 (initialize accepts any pool_size > 0), wipe it, and
/// a single insertion as large as the entire capacity FAILS with PoolFull -- the
/// one unbroken free run is [0, 5000) but the request is rounded to 8192. Every
/// OTHER clause holds and is asserted here (no key present, lookups absent,
/// used == 0, exactly one run [0, pool_size)).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ + 1 < u64::MAX@)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => match result.1 { Err(MtError::PoolFull) => true, _ => false }, Err(_) => true })]
#[ensures(match result.0 { Ok(_) => result.2@ == 5000, Err(_) => true })]
pub fn refute_mt_clear_post_empty(st: &mut StateModel, k: Key) -> (Result<(), MtError>, Result<usize, MtError>, usize) {
    snapshot! { lem_al(5000) };
    let r = mt_initialize(st, 5000);
    let mut ins: Result<usize, MtError> = Err(MtError::InvalidSize);
    let mut cap = 0usize;
    match r {
        Ok(()) => {
            let _ = mt_clear(st);
            // every other clause of the statement holds after the wipe:
            proof_assert!(st.slots@.len() == 0 && st.allocator.used@ == 0);
            proof_assert!(st.allocator.regions@.len() == 1 && st.allocator.regions@[0].offset@ == 0
                          && st.allocator.regions@[0].size@ == 5000);
            proof_assert!(forall<k2: Key> !slot_at(st.slots@, k2));
            cap = mt_capacity(st);
            proof_assert!(no_region_fits(st.allocator.regions@, align_up_l(5000)));
            // ... but a single insertion as large as the entire capacity is refused
            ins = mt_insert(st, k, 5000u32);
        }
        Err(_) => {}
    }
    (r, ins, cap)
}

/// Anti-vacuity twin: MUST FAIL (claims that full-capacity insertion succeeds).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ + 1 < u64::MAX@)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => match result.1 { Ok(_) => true, _ => false }, Err(_) => true })]
pub fn refute_mt_clear_post_empty__mutant(st: &mut StateModel, k: Key) -> (Result<(), MtError>, Result<usize, MtError>) {
    snapshot! { lem_al(5000) };
    let r = mt_initialize(st, 5000);
    let mut ins: Result<usize, MtError> = Err(MtError::InvalidSize);
    match r {
        Ok(()) => {
            let _ = mt_clear(st);
            ins = mt_insert(st, k, 5000u32);
        }
        Err(_) => {}
    }
    (r, ins)
}

/// Companion (NOT a scored module): every clause holds for a 4 KiB-multiple pool
/// size that fits a u32 request -- after clear, no key is present, lookups are
/// absent, used() == 0, one run [0, pool_size), and inserting pool_size bytes succeeds.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.pool_size@ % 4096 == 0 && st.pool_size@ <= 4294967295)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(!result.0 && result.1 == None && result.2@ == 0)]
#[ensures(match result.3 { Ok(_) => true, Err(_) => false })]
pub fn witness_mt_clear_post_empty_aligned(st: &mut StateModel, k: Key) -> (bool, Option<(usize, u32)>, usize, Result<usize, MtError>) {
    let _ = mt_clear(st);
    let c = mt_contains(st, k);
    let g = mt_peek(st, k);
    let u = mt_used(st);
    let ps = st.pool_size as u32;
    snapshot! { lem_al(ps@) };
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(ps@)));
    let ins = mt_insert(st, k, ps);
    (c, g, u, ins)
}

/// Mirror of `Default for MemoryTierState` (lib.rs:102-118). The bound policy is
/// a SEPARATE component, passed in as it is; an un-set-up memory-tier has no
/// policy pool of its own, so nothing is tracked for it.
#[requires(p.tracked@.len() == 0)]
#[ensures(result.pool_base@ == 0 && result.pool_size@ == 0 && result.pool_id@ == 0)]
#[ensures(!result.initialized && !result.spdk_allocated && !result.poisoned)]
#[ensures(result.allocator.capacity@ == 0 && result.allocator.used@ == 0)]
#[ensures(result.allocator.regions@.len() == 0 && result.slots@.len() == 0)]
#[ensures(result.policy == p)]
#[ensures(mt_inv(result))]
pub fn mt_default(p: PolicyModel) -> StateModel {
    StateModel {
        pool_base: 0,          // std::ptr::null_mut()
        pool_size: 0,
        pool_id: 0,
        allocator: fl_new(0),  // FreeList::new(0)
        slots: Vec::new(),     // HashMap::new()
        initialized: false,    // AtomicBool::new(false)
        spdk_allocated: false,
        poisoned: false,
        policy: p,
    }
}

/// **MT-DEFAULT-STATE-POST** (postcondition, code-only, 6 attachments)
///
/// "A freshly created component starts with a null base address, a pool size of
/// zero, a zero-capacity allocator holding no entries, the set-up flag clear and
/// the direct-transfer capability flag clear, so every operation either refuses or
/// reports emptiness until the pool is set up."
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ < u64::MAX@)]
#[ensures(result.0@ == 0 && result.1@ == 0 && !result.2 && result.3 == None && !result.4)]
#[ensures(result.5 == None && result.6 == None)]
#[ensures(match result.7 { Err(_) => true, Ok(_) => false })]
#[ensures(match result.8 { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(match result.9 { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(result.10 == None && result.11@.len() == 0)]
pub fn verify_mt_default_state_post(
    p: PolicyModel,
    k: Key,
    size: u32,
) -> (usize, usize, bool, Option<(usize, usize)>, bool, Option<(usize, u32)>, Option<(usize, u32)>,
      Result<usize, MtError>, Result<(), MtError>, Result<usize, MtError>, Option<Key>, Vec<Key>) {
    let mut st = mt_default(p);
    proof_assert!(st.pool_base@ == 0 && st.pool_size@ == 0 && !st.initialized && !st.spdk_allocated);
    proof_assert!(st.allocator.capacity@ == 0 && st.slots@.len() == 0);
    let cap = mt_capacity(&st);
    let used = mt_used(&st);
    let has = mt_contains(&st, k);
    let info = mt_pool_info(&st);
    let dma = mt_is_dma_capable(&st);
    let g = mt_get(&mut st, k);
    let pk = mt_peek(&st, k);
    let ins = mt_insert(&mut st, k, size);
    let rm = mt_remove(&mut st, k);
    let cl = mt_clear(&mut st);
    let ev = mt_evict_next(&mut st);
    let old = mt_oldest_keys(&mut st, 8);
    (cap, used, has, info, dma, g, pk, ins, rm, cl, ev, old)
}

/// Anti-vacuity twin: MUST FAIL (claims a fresh component already reports capacity).
#[requires(p.tracked@.len() == 0)]
#[ensures(result@ > 0)]
pub fn verify_mt_default_state_post__mutant(p: PolicyModel) -> usize {
    let st = mt_default(p);
    mt_capacity(&st)
}

/// **MT-DEFAULT-POOL-ID-AMBIGUOUS** (invariant, code-only, 6 attachments)
///
/// "An uninitialized component stores the policy pool identifier zero, which is
/// not a reserved sentinel but a perfectly valid identifier the eviction policy may
/// have handed to some other pool, so the stored identifier means nothing until
/// the pool has actually been set up and every operation must rely on the set-up
/// flag instead."
#[ensures(result.0@ == 0 && result.1@ == 0)]
#[ensures(result.2)]
pub fn verify_mt_default_pool_id_ambiguous(k: Key, keys: &[Key]) -> (u64, u64, bool) {
    // a shared policy in which ANOTHER client created pools 0 and 1 first
    // (eviction-policy-lru::create_pool hands out ids 0, 1, 2, ...)
    let mut other = PolicyModel { tracked: Vec::new(), next_handle: 0, pools: 0, last_pool: 0 };
    let theirs = policy_create_pool(&mut other);
    let _ = policy_create_pool(&mut other);
    proof_assert!(other.last_pool@ == 1);
    let mut st = mt_default(other);
    // the un-set-up component's stored id is 0 == the other client's live pool
    proof_assert!(st.pool_id == theirs);
    let s0 = snapshot! { st };
    // every operation that would use the stored id relies on the flag instead:
    // none of them reaches the policy (a call with id 0 would set last_pool = 0)
    let _ = mt_evict_next(&mut st);
    let _ = mt_evict_next_for_key(&mut st, k);
    let _ = mt_oldest_keys(&mut st, 4);
    mt_touch(&mut st, k);
    mt_batch_touch(&mut st, keys);
    let _ = mt_clear(&mut st);
    let _ = mt_insert(&mut st, k, 4096);
    let _ = mt_remove(&mut st, k);
    let _ = mt_get(&mut st, k);
    proof_assert!(st == *s0);
    let untouched = st.policy.last_pool == 1;
    (st.pool_id, theirs, untouched)
}

/// Anti-vacuity twin: MUST FAIL (claims an operation used the stale id 0).
#[ensures(result@ == 0)]
pub fn verify_mt_default_pool_id_ambiguous__mutant(k: Key) -> u64 {
    let mut other = PolicyModel { tracked: Vec::new(), next_handle: 0, pools: 0, last_pool: 0 };
    let _ = policy_create_pool(&mut other);
    let _ = policy_create_pool(&mut other);
    let mut st = mt_default(other);
    let _ = mt_evict_next(&mut st);
    let _ = mt_oldest_keys(&mut st, 4);
    mt_touch(&mut st, k);
    let _ = mt_clear(&mut st);
    st.policy.last_pool
}

/// **MT-BATCH-TOUCH-FRAME** (frame, 6 attachments)
///
/// "Refreshing many entries at once changes only their positions in the eviction
/// order: no entry is added or removed, no address or size changes, and the bytes
/// in use and total capacity stay the same."
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).allocator.used == st.allocator.used && (^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).allocator.regions@ == st.allocator.regions@)]
#[ensures(forall<x: Key> tracks((^st).policy.tracked@, x) == tracks(st.policy.tracked@, x))]
#[ensures((^st).policy.tracked@.len() == st.policy.tracked@.len())]
#[ensures(result.0 == result.2 && result.1 == result.3)]
pub fn verify_mt_batch_touch_frame(st: &mut StateModel, keys: &[Key], k: Key)
    -> (usize, usize, usize, usize) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let p0 = mt_peek(st, k);
    mt_batch_touch(st, keys);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    let p1 = mt_peek(st, k);
    proof_assert!(p0 == p1);
    (u0, c0, u1, c1)
}

/// Anti-vacuity twin: MUST FAIL (claims batch_touch can drop an entry from the policy).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 0)]
#[ensures((^st).policy.tracked@.len() < st.policy.tracked@.len())]
pub fn verify_mt_batch_touch_frame__mutant(st: &mut StateModel, keys: &[Key]) {
    mt_batch_touch(st, keys);
}

/// **MT-INIT-POST-POLICY-POOL** (postcondition, code-only, 10 attachments)
///
/// "Setting up the pool asks the bound eviction policy to create one fresh policy
/// pool and stores the identifier it hands back, and every later eviction-related
/// call uses that one stored identifier rather than creating another pool."
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[ensures(match result { Ok(_) => (^st).pool_id@ == st.policy.pools@
                                  && (^st).policy.pools@ == st.policy.pools@ + 1
                                  && (^st).policy.last_pool == (^st).pool_id,
                         Err(_) => (^st).policy.pools == st.policy.pools })]
pub fn verify_mt_init_post_policy_pool(st: &mut StateModel, psz: usize, ops: &[(u8, Key, u32)]) -> Result<(), MtError> {
    let r = mt_initialize(st, psz);
    match r {
        Ok(()) => {
            let id = snapshot! { st.pool_id };
            let pools1 = snapshot! { st.policy.pools };
            // ANY later sequence of the 17 calls (incl. a second initialize):
            mt_run(st, ops, psz);
            // ... never creates another policy pool, never changes the stored id,
            // and every policy call it made carried that stored id
            proof_assert!(st.policy.pools == *pools1 && st.pool_id == *id);
            proof_assert!(st.policy.last_pool == *id);
        }
        Err(_) => {}
    }
    r
}

/// Anti-vacuity twin: MUST FAIL (claims a later call created a second pool).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[ensures(match result { Ok(_) => (^st).policy.pools@ == st.policy.pools@ + 2, Err(_) => true })]
pub fn verify_mt_init_post_policy_pool__mutant(st: &mut StateModel, psz: usize, ops: &[(u8, Key, u32)]) -> Result<(), MtError> {
    let r = mt_initialize(st, psz);
    if r.is_ok() {
        mt_run(st, ops, psz);
    }
    r
}

/// **MT-INV-POINTER-STABLE-UNTIL-FREED** (invariant, divergent, 7 attachments)
///
/// "The address given out for an entry remains valid and keeps pointing at that
/// entry's own data until that entry is explicitly deleted or evicted; until then
/// the component never moves the entry and never re-uses its memory for a
/// different entry."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned && st.initialized)]
#[requires(slot_at(st.slots@, k))]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// as long as the entry is still there, the address (and size) handed out is unchanged ...
#[ensures(result.2 ==> result.1 == result.0 && result.0 != None)]
// ... the pool mapping itself has not moved ...
#[ensures((^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size)]
// ... and no OTHER live entry shares any byte of its run
#[ensures(result.2 ==> forall<i: Int, j: Int> 0 <= i && i < (^st).slots@.len() && 0 <= j && j < (^st).slots@.len()
          && (^st).slots@[i].key == k && (^st).slots@[j].key != k
          ==> disj((^st).slots@[i].offset@, sl_end((^st).slots@[i]),
                   (^st).slots@[j].offset@, sl_end((^st).slots@[j])))]
pub fn verify_mt_inv_pointer_stable_until_freed(
    st: &mut StateModel,
    k: Key,
    ops: &[(u8, Key, u32)],
    psz: usize,
) -> (Option<(usize, u32)>, Option<(usize, u32)>, bool) {
    let p0 = mt_peek(st, k);
    let s0 = snapshot! { st.slots@ };
    let st0 = snapshot! { *st };
    let n = ops.len();
    let mut i: usize = 0;
    // run ANY calls, stopping as soon as the entry has been deleted / evicted / wiped
    #[invariant(i@ <= n@)]
    #[invariant(mt_inv(*st))]
    #[invariant(!st.poisoned && st.initialized)]
    #[invariant(st.pool_base == st0.pool_base && st.pool_size == st0.pool_size)]
    #[invariant(st.policy.next_handle@ <= st0.policy.next_handle@ + i@)]
    #[invariant(st.policy.pools@ <= st0.policy.pools@ + i@)]
    #[invariant(forall<a: Int, b: Int> 0 <= a && a < s0.len() && 0 <= b && b < st.slots@.len()
                && s0[a].key == k && st.slots@[b].key == k ==> s0[a] == st.slots@[b])]
    while i < n && slots_contains(&st.slots, k) {
        let (op, key, size) = ops[i];
        let before = snapshot! { st.slots@ };
        proof_assert!(slot_at(*before, k));
        let _ = mt_call(st, op, key, size, psz);
        proof_assert!(slot_stable(*before, st.slots@));
        i += 1;
    }
    let still = mt_contains(st, k);
    let p1 = mt_peek(st, k);
    (p0, p1, still)
}

/// Anti-vacuity twin: MUST FAIL (claims a live entry's address can change).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned && st.initialized)]
#[requires(slot_at(st.slots@, k))]
#[requires(st.policy.next_handle@ + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[ensures(result.2 ==> result.1 != result.0)]
pub fn verify_mt_inv_pointer_stable_until_freed__mutant(st: &mut StateModel, k: Key, op: u8, key: Key, size: u32)
    -> (Option<(usize, u32)>, Option<(usize, u32)>, bool) {
    let p0 = mt_peek(st, k);
    let _ = mt_call(st, op, key, size, 0);
    let still = mt_contains(st, k);
    let p1 = mt_peek(st, k);
    (p0, p1, still)
}

/// **MT-INV-LOGGER-OPTIONAL** (invariant, 17 attachments)
///
/// "Every operation returns the same result and leaves the cache in the same state
/// whether or not the optional logging component is attached, because a message is
/// silently skipped when no logger is connected, so logging is never required for
/// correct operation."
///
/// A 2-run (non-interference) proof over `init_logged`, the only code path that
/// logs: two identical components, the same allocation outcome, one WITH a logger
/// and one WITHOUT -> identical results and field-wise identical states; the
/// detached sink is untouched. The other 16 methods contain no logging call at all
/// (the only `log_info`/`log_warn` call sites are lib.rs:249, 251, 300, 324, all
/// inside `initialize` / `alloc_mmap`).
#[requires(mt_inv(*s1) && mt_inv(*s2))]
#[requires(same_state(*s1, *s2))]
#[requires(!s1.initialized)]
#[requires(s1.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 && pool_size@ + 4096 <= usize::MAX@)]
#[requires(forall<b: usize> a == Some(b) ==> b@ % 4096 == 0 && b@ > 0 && b@ + pool_size@ <= usize::MAX@)]
#[ensures(result.0 == result.1)]
#[ensures(same_state(^s1, ^s2))]
#[ensures(^without == *without)]
pub fn verify_mt_inv_logger_optional(
    s1: &mut StateModel,
    s2: &mut StateModel,
    pool_size: usize,
    a: Option<usize>,
    with: &mut LogSink,
    without: &mut LogSink,
) -> (Result<(), MtError>, Result<(), MtError>) {
    let r1 = init_logged(s1, pool_size, a, with, true);
    let r2 = init_logged(s2, pool_size, a, without, false);
    (r1, r2)
}

/// Anti-vacuity twin: MUST FAIL (claims the logger-less run ends in a different state).
#[requires(mt_inv(*s1) && mt_inv(*s2))]
#[requires(same_state(*s1, *s2))]
#[requires(!s1.initialized)]
#[requires(s1.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 && pool_size@ + 4096 <= usize::MAX@)]
#[requires(forall<b: usize> a == Some(b) ==> b@ % 4096 == 0 && b@ > 0 && b@ + pool_size@ <= usize::MAX@)]
#[ensures((^s1).initialized != (^s2).initialized)]
pub fn verify_mt_inv_logger_optional__mutant(
    s1: &mut StateModel,
    s2: &mut StateModel,
    pool_size: usize,
    a: Option<usize>,
    with: &mut LogSink,
    without: &mut LogSink,
) {
    let _ = init_logged(s1, pool_size, a, with, true);
    let _ = init_logged(s2, pool_size, a, without, false);
}

/// **MT-INIT-FLAG-ORDERING-MIX** (invariant, code-only, 15 attachments) -- REFUTED.
///
/// Statement: "... the double-set-up guard inside the set-up operation itself
/// reads the same flag with the weakest possible ordering, so two threads calling
/// set-up at the same time are not reliably serialised by that check."
///
/// NEGATION, proved: two concurrent `initialize` calls ARE serialised. The
/// Relaxed load (lib.rs:275) executes while `self.state.write()` (lib.rs:274) is
/// held, and the guard stays held through the Release store (lib.rs:322), so the
/// two critical sections are mutually exclusive and every interleaving is one of
/// the two sequential orders (`t1_first`). In both orders at most one call
/// succeeds, the other returns `AllocationFailed("already initialized")`, and the
/// stored pool is exactly the winner's. Modelling assumption (not a #[trusted]
/// item): `RwLock::write` is an exclusive lock whose acquire/release order the
/// flag accesses -- std's documented contract.
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(sz1@ > 0 && sz1@ + 4096 <= usize::MAX@ && sz2@ > 0 && sz2@ + 4096 <= usize::MAX@)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[ensures(!(result.0 == Ok(()) && result.1 == Ok(())))]
#[ensures(result.0 == Ok(()) ==> result.1 == Err(MtError::AllocationFailed) && (^st).pool_size == sz1)]
#[ensures(result.1 == Ok(()) ==> result.0 == Err(MtError::AllocationFailed) && (^st).pool_size == sz2)]
#[ensures(result.0 == Ok(()) || result.1 == Ok(()) ==> (^st).initialized && (^st).policy.pools@ == st.policy.pools@ + 1)]
pub fn refute_mt_init_flag_ordering_mix(
    st: &mut StateModel,
    sz1: usize,
    sz2: usize,
    t1_first: bool,
) -> (Result<(), MtError>, Result<(), MtError>) {
    if t1_first {
        let r1 = mt_initialize(st, sz1);
        let r2 = mt_initialize(st, sz2);
        (r1, r2)
    } else {
        let r2 = mt_initialize(st, sz2);
        let r1 = mt_initialize(st, sz1);
        (r1, r2)
    }
}

/// Anti-vacuity twin: MUST FAIL (claims both concurrent set-ups can succeed).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(sz1@ > 0 && sz1@ + 4096 <= usize::MAX@ && sz2@ > 0 && sz2@ + 4096 <= usize::MAX@)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[ensures(result.0 == Ok(()) ==> result.1 == Ok(()))]
pub fn refute_mt_init_flag_ordering_mix__mutant(st: &mut StateModel, sz1: usize, sz2: usize)
    -> (Result<(), MtError>, Result<(), MtError>) {
    let r1 = mt_initialize(st, sz1);
    let r2 = mt_initialize(st, sz2);
    (r1, r2)
}

/// `self.eviction_policy.get()` on the receptacle: `Ok` iff a provider is
/// connected (component-framework receptacle semantics).
#[ensures(result == connected)]
pub fn ep_get(connected: bool) -> bool {
    connected
}

/// Outcome of a call that contains a receptacle lookup.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum RxOutcome {
    /// `.unwrap()` on `Err` -- the call aborts.
    Panicked,
    /// Returned normally (including a clean error return).
    Returned,
}

/// One IMemoryTier call with the eviction-policy receptacle lookup placed exactly
/// where each real method performs it (after that method's own early returns):
/// insert lib.rs:344 (after :333 size, :338 flag), get :391 (after :387),
/// oldest_keys :434 (after :430 flag / n==0), evict_next :444 (after :440),
/// remove :484 (after :478), touch :515 (after :511), clear :605 (after :600),
/// all `.unwrap()`; batch_touch :532-535 `Err(_) => return` (after :525 empty,
/// :529 flag). peek, contains, capacity, used, pool_info, is_dma_capable and
/// telemetry_snapshot never look it up. (initialize: see `rx_initialize`.)
#[requires(mt_inv(*st) && arith_ok(*st) && !st.poisoned)]
#[ensures(mt_inv(^st))]
#[ensures(!connected && match result { RxOutcome::Panicked => true, _ => false } ==> ^st == *st)]
#[ensures(!connected && st.initialized ==> match result {
    RxOutcome::Panicked => op@ == 1 && size@ > 0 || op@ == 2 || op@ == 4 || op@ == 5
                           || op@ == 6 && size@ > 0 || op@ == 7 || op@ == 8 || op@ == 14,
    RxOutcome::Returned => !(op@ == 1 && size@ > 0 || op@ == 2 || op@ == 4 || op@ == 5
                           || op@ == 6 && size@ > 0 || op@ == 7 || op@ == 8 || op@ == 14) })]
#[ensures(!connected && op@ == 9 ==> ^st == *st && match result { RxOutcome::Returned => true, _ => false })]
#[ensures(connected ==> match result { RxOutcome::Returned => true, _ => false })]
#[ensures(op@ == 3 || (op@ >= 10 && op@ != 14) ==> ^st == *st)]
pub fn rx_call(st: &mut StateModel, op: u8, key: Key, size: u32, connected: bool) -> RxOutcome {
    if op == 1 {
        if size == 0 || !st.initialized {
            let _ = mt_insert(st, key, size);
            return RxOutcome::Returned;
        }
        if !ep_get(connected) {
            return RxOutcome::Panicked;
        }
        let _ = mt_insert(st, key, size);
    } else if op == 2 || op == 4 || op == 5 || op == 7 || op == 8 || op == 14 {
        if !st.initialized {
            return RxOutcome::Returned; // each of these returns before the lookup
        }
        if !ep_get(connected) {
            return RxOutcome::Panicked;
        }
        if op == 2 {
            let _ = mt_get(st, key);
        } else if op == 4 {
            let _ = mt_evict_next(st);
        } else if op == 5 {
            let _ = mt_evict_next_for_key(st, key);
        } else if op == 7 {
            let _ = mt_remove(st, key);
        } else if op == 8 {
            mt_touch(st, key);
        } else {
            let _ = mt_clear(st);
        }
    } else if op == 6 {
        if !st.initialized || size == 0 {
            return RxOutcome::Returned;
        }
        if !ep_get(connected) {
            return RxOutcome::Panicked;
        }
        let _ = mt_oldest_keys(st, size as usize);
    } else if op == 9 {
        // batch_touch(&[key]): non-empty; flag; `match self.eviction_policy.get() { Err(_) => return }`
        if !st.initialized {
            return RxOutcome::Returned;
        }
        if !ep_get(connected) {
            return RxOutcome::Returned;
        }
        mt_batch_touch(st, &[key]);
    } else if op == 3 {
        let _ = mt_peek(st, key);
    } else if op == 10 {
        let _ = mt_contains(st, key);
    } else if op == 11 {
        let _ = mt_capacity(st);
    } else if op == 12 {
        let _ = mt_used(st);
    } else if op == 13 {
        let _ = mt_pool_info(st);
    } else if op == 15 {
        let _ = mt_is_dma_capable(st);
    } else {
        let _ = mt_telemetry_snapshot(st);
    }
    RxOutcome::Returned
}

/// `initialize` with its receptacle lookup (lib.rs:270-272): after the size check,
/// before the lock -- `map_err(|_| NotInitialized(..))?`, a CLEAN refusal.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 ==> pool_size@ + 4096 <= usize::MAX@)]
#[ensures(!connected && pool_size@ > 0 ==> result == Err(MtError::NotInitialized) && ^st == *st)]
#[ensures(mt_inv(^st))]
// --- batch 2 ---
#[ensures(match result { Ok(_) => true, Err(_) => ^st == *st })]
#[ensures(match result { Ok(_) => (^st).initialized && (^st).slots@ == Seq::empty()
                                  && (^st).allocator.capacity@ == pool_size@ && (^st).allocator.used@ == 0,
                         Err(_) => true })]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
pub fn rx_initialize(st: &mut StateModel, pool_size: usize, connected: bool) -> Result<(), MtError> {
    if pool_size == 0 {
        return Err(MtError::InvalidSize);
    }
    if !ep_get(connected) {
        return Err(MtError::NotInitialized);
    }
    mt_initialize(st, pool_size)
}

/// **MT-INV-RECEPTACLE-LOOKUP-PANICS** (invariant, code-only, 10 attachments)
///
/// "Set-up refuses cleanly when the eviction-policy receptacle is not connected and
/// the batch refresh is the one operation that quietly returns in that case, but
/// seven other places fetch the same receptacle and unwrap the result, so if the
/// policy is ever disconnected after set-up those operations abort instead of
/// reporting an error."
#[requires(mt_inv(*st) && arith_ok(*st) && !st.poisoned)]
#[requires(st.initialized)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(size@ > 0)]
// set-up refuses cleanly
#[ensures(result.0 == Err(MtError::NotInitialized))]
// batch refresh quietly returns
#[ensures(match result.1 { RxOutcome::Returned => true, _ => false })]
// the seven unwrap sites abort: insert, get, oldest_keys, evict_next, remove, touch, clear
// (plus evict_next_for_key, which delegates to evict_next)
#[ensures(match result.2 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.3 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.4 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.5 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.6 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.7 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.8 { RxOutcome::Panicked => true, _ => false })]
#[ensures(match result.9 { RxOutcome::Panicked => true, _ => false })]
// the lookup-free methods are unaffected
#[ensures(match result.10 { RxOutcome::Returned => true, _ => false })]
// and nothing changed state on the way (no error is ever REPORTED by the seven)
#[ensures(^st == *st)]
pub fn verify_mt_inv_receptacle_lookup_panics(st: &mut StateModel, key: Key, size: u32, psz: usize)
    -> (Result<(), MtError>, RxOutcome, RxOutcome, RxOutcome, RxOutcome, RxOutcome,
        RxOutcome, RxOutcome, RxOutcome, RxOutcome, RxOutcome) {
    let init = rx_initialize(st, psz, false);
    let bt = rx_call(st, 9, key, size, false);
    let ins = rx_call(st, 1, key, size, false);
    let g = rx_call(st, 2, key, size, false);
    let ok = rx_call(st, 6, key, size, false);
    let ev = rx_call(st, 4, key, size, false);
    let evk = rx_call(st, 5, key, size, false);
    let rm = rx_call(st, 7, key, size, false);
    let t = rx_call(st, 8, key, size, false);
    let cl = rx_call(st, 14, key, size, false);
    let pk = rx_call(st, 3, key, size, false);
    (init, bt, ins, g, ok, ev, evk, rm, t, cl, pk)
}

/// Anti-vacuity twin: MUST FAIL (claims a disconnected insert reports an error instead of aborting).
#[requires(mt_inv(*st) && arith_ok(*st) && !st.poisoned)]
#[requires(st.initialized)]
#[requires(size@ > 0)]
#[ensures(match result { RxOutcome::Returned => true, _ => false })]
pub fn verify_mt_inv_receptacle_lookup_panics__mutant(st: &mut StateModel, key: Key, size: u32) -> RxOutcome {
    rx_call(st, 1, key, size, false)
}

/// Lock / policy events of one call, in program order (lock-skeleton mirror).
/// `PoolRead` = `state.pool.read()` (or `try_read` with the `telemetry` fallback
/// to `read()`), `PoolWrite` = `state.pool.write()` / `try_write`, `PoolRelease`
/// = the guard dropped (`drop(pool)` or end of scope), `PolicyRefresh` =
/// `ep.touch` / `ep.batch_touch`, `PolicyOther` = any other policy call.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum LkEv {
    StateRead,
    PoolRead,
    PoolWrite,
    PoolRelease,
    PolicyRefresh,
    PolicyOther,
}

#[logic(open)]
pub fn no_pool_write(t: Seq<LkEv>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < t.len() ==> t[i] != LkEv::PoolWrite }
}

#[logic(open)]
pub fn no_pool_read(t: Seq<LkEv>) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < t.len() ==> t[i] != LkEv::PoolRead }
}

/// Every eviction-order refresh comes after the pool guard taken before it was released.
#[logic(open)]
pub fn refresh_after_release(t: Seq<LkEv>) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= j && j < i && i < t.len() && t[i] == LkEv::PolicyRefresh
            && (t[j] == LkEv::PoolRead || t[j] == LkEv::PoolWrite)
            ==> exists<m: Int> j < m && m < i && t[m] == LkEv::PoolRelease
    }
}

/// get (lib.rs:385-414): state.read; flag; pool.read; slots.get?; drop(pool); ep.touch.
#[ensures(no_pool_write(result@) && refresh_after_release(result@))]
// batch 3: the exact trace of a successful get
#[ensures(init && present ==> result@.len() == 4 && result@[0] == LkEv::StateRead && result@[1] == LkEv::PoolRead
          && result@[2] == LkEv::PoolRelease && result@[3] == LkEv::PolicyRefresh)]
pub fn lk_get(init: bool, present: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PoolRead);
    if !present { t.push(LkEv::PoolRelease); return t; }
    t.push(LkEv::PoolRelease); // drop(pool) lib.rs:411
    t.push(LkEv::PolicyRefresh); // ep.touch lib.rs:412
    t
}

/// peek (lib.rs:416-426) and contains (lib.rs:561-569): shared pool lock, no policy call.
#[ensures(no_pool_write(result@) && refresh_after_release(result@))]
// batch 3: no policy call at all
#[ensures(forall<i: Int> 0 <= i && i < result@.len()
          ==> result@[i] != LkEv::PolicyRefresh && result@[i] != LkEv::PolicyOther)]
pub fn lk_peek_or_contains(init: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PoolRead);
    t.push(LkEv::PoolRelease);
    t
}

/// batch_touch (lib.rs:524-559): pool.read, collect handles, drop(pool), ep.batch_touch.
#[ensures(no_pool_write(result@) && refresh_after_release(result@))]
#[ensures(nonempty && init && connected ==> result@.len() == 4 && result@[0] == LkEv::StateRead
          && result@[1] == LkEv::PoolRead && result@[2] == LkEv::PoolRelease && result@[3] == LkEv::PolicyRefresh)]
// --- batch 4: the two early returns (MT-BATCH-TOUCH-EMPTY-NOOP) ---
#[ensures(!nonempty ==> result@.len() == 0)]
#[ensures(nonempty && !init ==> result@.len() == 1 && result@[0] == LkEv::StateRead)]
pub fn lk_batch_touch(nonempty: bool, init: bool, connected: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    if !nonempty { return t; }
    t.push(LkEv::StateRead);
    if !init || !connected { return t; }
    t.push(LkEv::PoolRead);
    t.push(LkEv::PoolRelease); // drop(pool) lib.rs:557
    t.push(LkEv::PolicyRefresh); // ep.batch_touch lib.rs:558
    t
}

/// touch (lib.rs:509-522): pool.read; if present { drop(pool); ep.touch }.
#[ensures(no_pool_write(result@) && refresh_after_release(result@))]
// batch 3: the exact trace of a touch of a present key
#[ensures(init && present ==> result@.len() == 4 && result@[0] == LkEv::StateRead && result@[1] == LkEv::PoolRead
          && result@[2] == LkEv::PoolRelease && result@[3] == LkEv::PolicyRefresh)]
// --- batch 6: MT-TOUCH-SILENT-NOOP -- an absent key / un-set-up pool: no policy call at all ---
#[ensures(!init || !present ==> forall<i: Int> 0 <= i && i < result@.len()
          ==> result@[i] != LkEv::PolicyRefresh && result@[i] != LkEv::PolicyOther)]
pub fn lk_touch(init: bool, present: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PoolRead);
    t.push(LkEv::PoolRelease); // drop(pool) lib.rs:519 / end of scope
    if present {
        t.push(LkEv::PolicyRefresh); // ep.touch lib.rs:520
    }
    t
}

/// insert (lib.rs:332-383): pool.write; contains?; allocate?; ep.track (under the lock); insert slot.
#[ensures(no_pool_read(result@) && refresh_after_release(result@))]
#[ensures(size_ok && init ==> exists<i: Int> 0 <= i && i < result@.len() && result@[i] == LkEv::PoolWrite)]
// --- batch 5: the two early returns (MT-INSERT-ERR-INVALID-SIZE / -NOT-INITIALIZED) ---
#[ensures(!size_ok ==> result@ == Seq::empty())]
#[ensures(size_ok && !init ==> result@ == Seq::singleton(LkEv::StateRead))]
pub fn lk_insert(size_ok: bool, init: bool, absent: bool, alloc_ok: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    if !size_ok { return t; }
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PoolWrite);
    if absent && alloc_ok {
        t.push(LkEv::PolicyOther); // ep.track -- registration, not a refresh
    }
    t.push(LkEv::PoolRelease);
    t
}

/// remove (lib.rs:476-507): pool.write; slots.remove?; ep.remove (under the lock); deallocate.
#[ensures(no_pool_read(result@) && refresh_after_release(result@))]
#[ensures(init ==> exists<i: Int> 0 <= i && i < result@.len() && result@[i] == LkEv::PoolWrite)]
// --- batch 6: MT-REMOVE-ERR-NOT-INITIALIZED / -KEY-NOT-FOUND ---
#[ensures(!init ==> result@ == Seq::singleton(LkEv::StateRead))]
#[ensures(!present ==> forall<i: Int> 0 <= i && i < result@.len()
          ==> result@[i] != LkEv::PolicyRefresh && result@[i] != LkEv::PolicyOther)]
pub fn lk_remove(init: bool, present: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PoolWrite);
    if present {
        t.push(LkEv::PolicyOther); // ep.remove
    }
    t.push(LkEv::PoolRelease);
    t
}

/// evict_next (lib.rs:438-470): ep.identify_next_to_evict BEFORE the pool lock; then pool.write.
#[ensures(no_pool_read(result@) && refresh_after_release(result@))]
#[ensures(init && victim ==> exists<i: Int> 0 <= i && i < result@.len() && result@[i] == LkEv::PoolWrite)]
pub fn lk_evict_next(init: bool, victim: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init { return t; }
    t.push(LkEv::PolicyOther); // identify_next_to_evict
    if victim {
        t.push(LkEv::PoolWrite);
        t.push(LkEv::PoolRelease);
    }
    t
}

/// **MT-INV-MODULE-DOC-LOCKING-CLAIM** (invariant, code-only, 8 attachments)
///
/// "The operations that only read -- the two lookups, the batch refresh and the
/// presence check -- take only a shared lock on the pool while the operations that
/// change it -- insertions, deletions and evictions -- take an exclusive one, and
/// eviction-order refreshes happen after the pool lock has been released, exactly
/// as the module documentation claims."
#[ensures(no_pool_write(result.0@) && no_pool_write(result.1@) && no_pool_write(result.2@) && no_pool_write(result.3@))]
#[ensures(no_pool_read(result.4@) && no_pool_read(result.5@) && no_pool_read(result.6@))]
#[ensures(init && size_ok ==> exists<i: Int> 0 <= i && i < result.4@.len() && result.4@[i] == LkEv::PoolWrite)]
#[ensures(init ==> exists<i: Int> 0 <= i && i < result.5@.len() && result.5@[i] == LkEv::PoolWrite)]
#[ensures(init && b2 ==> exists<i: Int> 0 <= i && i < result.6@.len() && result.6@[i] == LkEv::PoolWrite)]
#[ensures(refresh_after_release(result.0@) && refresh_after_release(result.2@) && refresh_after_release(result.7@))]
pub fn verify_mt_inv_module_doc_locking_claim(init: bool, b1: bool, b2: bool, b3: bool, size_ok: bool)
    -> (Vec<LkEv>, Vec<LkEv>, Vec<LkEv>, Vec<LkEv>, Vec<LkEv>, Vec<LkEv>, Vec<LkEv>, Vec<LkEv>) {
    let g = lk_get(init, b1);           // lookup
    let p = lk_peek_or_contains(init);  // lookup (peek)
    let bt = lk_batch_touch(b2, init, b3); // batch refresh
    let c = lk_peek_or_contains(init);  // presence check
    let ins = lk_insert(size_ok, init, b1, b2);
    let rm = lk_remove(init, b1);
    let ev = lk_evict_next(init, b2);
    let t = lk_touch(init, b1);
    (g, p, bt, c, ins, rm, ev, t)
}

/// Anti-vacuity twin: MUST FAIL (claims get refreshes the eviction order while holding the pool lock).
#[ensures(no_pool_write(result@))]
#[ensures(init && present ==> exists<i: Int> 0 <= i && i < result@.len() && result@[i] == LkEv::PolicyRefresh
          && !refresh_after_release(result@))]
pub fn verify_mt_inv_module_doc_locking_claim__mutant(init: bool, present: bool) -> Vec<LkEv> {
    lk_get(init, present)
}

/// The pool's structural rules, over the two objects the pool write lock guards
/// (`Pool { allocator, slots }`, lib.rs:80-83) and NOTHING else -- in particular
/// not the policy, which is a separately synchronised component that under
/// concurrency may disagree with the pool (stale handles, raced victims).
#[logic(open)]
pub fn pool_inv(fl: FreeListModel, s: Seq<SlotModel>) -> bool {
    pearlite! {
        fl_inv(fl.regions@, fl.capacity@)
        && slot_inv(s, fl.capacity@)
        && slot_keys_unique(s)
        && fl.used@ == slot_sum(s)
        && free_sum(fl.regions@) + fl.used@ == fl.capacity@
        && slot_ivl_ok(s, fl.capacity@)
        && slots_disj(s)
        && slots_clear(fl.regions@, s)
    }
}

/// insert's pool critical section (lib.rs:358-379) for ANY key/size and ANY handle
/// the policy hands back (`ep.track` touches only the policy object).
#[requires(pool_inv(*fl, slots@))]
#[ensures(pool_inv(^fl, (^slots)@))]
#[ensures((^fl).capacity == fl.capacity)]
// --- batch 3 ---
#[ensures(size@ > 0 && !slot_at(slots@, key) && some_region_fits(fl.regions@, align_up_l(size@))
          ==> match result { Ok(_) => true, Err(_) => false })]
#[ensures(match result { Ok(_) => slot_at((^slots)@, key), Err(_) => (^slots)@ == slots@ })]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^slots)@, k2) == slot_at(slots@, k2))]
pub fn cs_insert(fl: &mut FreeListModel, slots: &mut Vec<SlotModel>, key: Key, size: u32, handle: u64)
    -> Result<usize, MtError> {
    if size == 0 {
        return Err(MtError::InvalidSize); // lib.rs:333, before any lock
    }
    if slots_contains(slots, key) {
        return Err(MtError::AlreadyExists);
    }
    let s0 = snapshot! { slots@ };
    let offset = match fl_allocate(fl, size) {
        Some(o) => o,
        None => return Err(MtError::PoolFull),
    };
    let ns = SlotModel { key, offset, size, handle };
    snapshot! { lem_ss_push(*s0, ns) };
    snapshot! { lem_al(size@) };
    slots.push(ns);
    proof_assert!(slots@ == s0.push_back(ns));
    proof_assert!(forall<j: Int> 0 <= j && j < s0.len() ==> slots@[j] == s0[j]);
    proof_assert!(slots@[s0.len()] == ns);
    proof_assert!(forall<j: Int> 0 <= j && j < s0.len()
                  ==> disj(s0[j].offset@, sl_end(s0[j]), offset@, offset@ + align_up_l(size@)));
    proof_assert!(forall<k2: Key> slot_at(slots@, k2) == (slot_at(*s0, k2) || k2 == key));
    proof_assert!(slot_keys_unique(slots@));
    let fin = snapshot! { slots@ };
    proof_assert!(forall<i: Int, j: Int> 0 <= i && i < s0.len() && 0 <= j && j < s0.len() && fin[i].key != fin[j].key
        ==> disj(fin[i].offset@, sl_end(fin[i]), fin[j].offset@, sl_end(fin[j])));
    proof_assert!(forall<i: Int> 0 <= i && i < s0.len()
        ==> disj(fin[i].offset@, sl_end(fin[i]), fin[s0.len()].offset@, sl_end(fin[s0.len()])));
    proof_assert!(forall<i: Int> 0 <= i && i < s0.len()
        ==> disj(fin[s0.len()].offset@, sl_end(fin[s0.len()]), fin[i].offset@, sl_end(fin[i])));
    proof_assert!(slots_disj(slots@));
    proof_assert!(slots_clear(fl.regions@, slots@));
    proof_assert!(slot_inv(slots@, fl.capacity@) && slot_ivl_ok(slots@, fl.capacity@));
    Ok(offset)
}

/// evict_next's pool critical section (lib.rs:458-462) for ANY key -- whatever
/// `identify_next_to_evict` returned earlier OUTSIDE the lock, stale or not.
#[requires(pool_inv(*fl, slots@))]
#[ensures(pool_inv(^fl, (^slots)@))]
#[ensures((^fl).capacity == fl.capacity)]
// --- batch 3: a stale victim (no entry) leaves the pool untouched ---
#[ensures(!slot_at(slots@, key) ==> (^slots)@ == slots@ && (^fl).regions@ == fl.regions@ && (^fl).used == fl.used)]
#[ensures(!slot_at((^slots)@, key))]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^slots)@, k2) == slot_at(slots@, k2))]
pub fn cs_evict(fl: &mut FreeListModel, slots: &mut Vec<SlotModel>, key: Key) {
    match slots_remove(slots, key) {
        Some(slot) => {
            snapshot! { lem_al(slot.size@) };
            proof_assert!(dealloc_pre(*fl, slot.offset@, align_up_l(slot.size@)));
            fl_deallocate(fl, slot.offset, slot.size);
            proof_assert!(forall<j: Int> 0 <= j && j < slots@.len()
                ==> disj(slots@[j].offset@, sl_end(slots@[j]), slot.offset@, sl_end(slot)));
            proof_assert!(slots_clear(fl.regions@, slots@));
        }
        None => {}
    }
}

/// remove's pool critical section (lib.rs:498-506) for ANY key (`ep.remove` with a
/// possibly stale handle touches only the policy object).
#[requires(pool_inv(*fl, slots@))]
#[ensures(pool_inv(^fl, (^slots)@))]
#[ensures((^fl).capacity == fl.capacity)]
// --- batch 3 ---
#[ensures(slot_at(slots@, key) ==> result == Ok(()))]
#[ensures(!slot_at((^slots)@, key))]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^slots)@, k2) == slot_at(slots@, k2))]
#[ensures(forall<i: Int> 0 <= i && i < slots@.len() && slots@[i].key == key
          ==> some_region_fits((^fl).regions@, align_up_l(slots@[i].size@)))]
#[ensures(result != Ok(()) ==> (^slots)@ == slots@ && (^fl).regions@ == fl.regions@ && (^fl).used == fl.used)]
pub fn cs_remove(fl: &mut FreeListModel, slots: &mut Vec<SlotModel>, key: Key) -> Result<(), MtError> {
    let sl0 = snapshot! { slots@ };
    match slots_remove(slots, key) {
        None => Err(MtError::KeyNotFound),
        Some(slot) => {
            snapshot! { lem_al(slot.size@) };
            proof_assert!(forall<i: Int> 0 <= i && i < sl0.len() && sl0[i].key == key ==> sl0[i] == slot);
            proof_assert!(dealloc_pre(*fl, slot.offset@, align_up_l(slot.size@)));
            fl_deallocate(fl, slot.offset, slot.size);
            proof_assert!(some_region_fits(fl.regions@, align_up_l(slot.size@)));
            proof_assert!(forall<j: Int> 0 <= j && j < slots@.len()
                ==> disj(slots@[j].offset@, sl_end(slots@[j]), slot.offset@, sl_end(slot)));
            proof_assert!(slots_clear(fl.regions@, slots@));
            Ok(())
        }
    }
}

/// clear's pool critical section (lib.rs:606-610).
#[requires(pool_inv(*fl, slots@))]
#[requires(fl.capacity == pool_size)]
#[ensures(pool_inv(^fl, (^slots)@))]
#[ensures((^fl).capacity == fl.capacity)]
pub fn cs_clear(fl: &mut FreeListModel, slots: &mut Vec<SlotModel>, pool_size: usize) {
    *slots = Vec::new();
    *fl = fl_new(pool_size);
}

/// **MT-INV-CONCURRENT-STRUCTURAL-INTEGRITY** (invariant, divergent, 17 attachments)
///
/// "When many threads use the component at the same time the cache always ends up
/// in a state that satisfies all of its structural rules -- entries never overlap,
/// no key appears twice, no space is lost or double-booked and the bytes-in-use
/// figure still matches the entries held -- which the code secures by always taking
/// the component-state lock first and the inner pool lock second, never the other
/// way round, and by making every call out to the eviction policy after the pool
/// lock has been released."
///
/// Interleaving model at LOCK granularity: every mutation of `Pool` happens inside
/// a `pool.write()` critical section, so a concurrent execution is an arbitrary
/// interleaving of whole critical sections (insert / evict / remove / clear) with
/// read-only sections and policy-only steps (identify, touch, batch_touch,
/// candidates), which do not write the pool. Keys flowing from the policy into a
/// later critical section are ARBITRARY here (stale handles / raced victims
/// included), and NO agreement between policy and pool is assumed.
#[requires(pool_inv(*fl, slots@))]
#[ensures((^fl).capacity == fl.capacity)]
// entries never overlap
#[ensures(slots_disj((^slots)@))]
// no key appears twice
#[ensures(slot_keys_unique((^slots)@))]
// no space is lost: free runs + in-use == capacity; nor double-booked
#[ensures(free_sum((^fl).regions@) + (^fl).used@ == (^fl).capacity@)]
#[ensures(slots_clear((^fl).regions@, (^slots)@))]
// the bytes-in-use figure matches the entries held
#[ensures((^fl).used@ == slot_sum((^slots)@))]
#[ensures(pool_inv(^fl, (^slots)@))]
pub fn verify_mt_inv_concurrent_structural_integrity(
    fl: &mut FreeListModel,
    slots: &mut Vec<SlotModel>,
    steps: &[(u8, Key, u32, u64)],
) {
    let cap = fl.capacity;
    let n = steps.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(pool_inv(*fl, slots@))]
    #[invariant(fl.capacity == cap)]
    while i < n {
        let (op, key, size, h) = steps[i];
        if op == 0 {
            let _ = cs_insert(fl, slots, key, size, h);
        } else if op == 1 {
            cs_evict(fl, slots, key);
        } else if op == 2 {
            let _ = cs_remove(fl, slots, key);
        } else if op == 3 {
            cs_clear(fl, slots, cap);
        } else {
            // a shared-lock reader or a policy-only step: no write to the pool
        }
        i += 1;
    }
}

/// Anti-vacuity twin: MUST FAIL -- a critical section that releases an entry's run
/// but forgets to drop the entry (the double-booking bug the invariant excludes).
#[requires(pool_inv(*fl, slots@))]
#[ensures(pool_inv(^fl, (^slots)@))]
pub fn verify_mt_inv_concurrent_structural_integrity__mutant(fl: &mut FreeListModel, slots: &mut Vec<SlotModel>) {
    if slots.len() > 0 {
        let sl = slots[0];
        snapshot! { lem_al(sl.size@) };
        fl_deallocate(fl, sl.offset, sl.size);
    }
}

// ===========================================================================
// 11. Batch 2 — shared logic and lemmas
// ===========================================================================

/// Sum of the REQUESTED (unrounded) sizes of the entries held.
#[logic(open)]
#[variant(s.len())]
pub fn raw_sum(s: Seq<SlotModel>) -> Int {
    pearlite! {
        if s.len() <= 0 { 0 }
        else { raw_sum(s.subsequence(0, s.len() - 1)) + s[s.len() - 1].size@ }
    }
}

/// The rounded total is a whole number of 4 KiB units.
#[logic]
#[variant(s.len())]
#[ensures(slot_sum(s) % 4096 == 0)]
pub fn lem_ss_aligned(s: Seq<SlotModel>) {
    pearlite! {
        if s.len() <= 0 { () } else {
            lem_ss_aligned(s.subsequence(0, s.len() - 1));
            proof_assert!(align_up_l(s[s.len() - 1].size@) % 4096 == 0);
            ()
        }
    }
}

/// Requested total vs rounded total: `<=`, `==` iff every size is a 4 KiB multiple.
#[logic]
#[variant(s.len())]
#[requires(forall<j: Int> 0 <= j && j < s.len() ==> s[j].size@ > 0)]
#[ensures(raw_sum(s) <= slot_sum(s))]
#[ensures((forall<j: Int> 0 <= j && j < s.len() ==> s[j].size@ % 4096 == 0) ==> raw_sum(s) == slot_sum(s))]
#[ensures((exists<j: Int> 0 <= j && j < s.len() && s[j].size@ % 4096 != 0) ==> raw_sum(s) < slot_sum(s))]
pub fn lem_raw(s: Seq<SlotModel>) {
    pearlite! {
        if s.len() <= 0 { () } else {
            lem_al(s[s.len() - 1].size@);
            proof_assert!(forall<j: Int> 0 <= j && j < s.len() - 1 ==> s.subsequence(0, s.len() - 1)[j] == s[j]);
            lem_raw(s.subsequence(0, s.len() - 1))
        }
    }
}

/// A 4 KiB-multiple that fits under `c` fits under `c` rounded DOWN.
#[logic]
#[requires(x % 4096 == 0 && 0 <= x && x <= c)]
#[ensures(x <= c - c % 4096)]
pub fn lem_floor(x: Int, c: Int) {}

/// A `u32` request rounds up to at most 4 GiB.
#[logic]
#[requires(0 < x && x <= 4294967295)]
#[ensures(align_up_l(x) <= 4294967296)]
pub fn lem_al_u32(x: Int) {
    pearlite! { lem_al(x) }
}

/// Every observable of two states agrees except the GHOST `last_pool`.
#[logic(open)]
pub fn same_obs(a: StateModel, b: StateModel) -> bool {
    pearlite! {
        a.pool_base == b.pool_base && a.pool_size == b.pool_size && a.pool_id == b.pool_id
        && a.allocator.regions@ == b.allocator.regions@ && a.allocator.capacity == b.allocator.capacity
        && a.allocator.used == b.allocator.used && a.slots@ == b.slots@
        && a.initialized == b.initialized && a.spdk_allocated == b.spdk_allocated
        && a.poisoned == b.poisoned && a.policy.tracked@ == b.policy.tracked@
        && a.policy.next_handle == b.policy.next_handle && a.policy.pools == b.policy.pools
    }
}

// ===========================================================================
// 12. Batch 2 — property drivers
// ===========================================================================

/// **MT-EVICT-NEXT-FRAME-OTHER-ENTRIES** (frame, 6 attachments)
///
/// "Eviction removes exactly one entry: every other key stays present with the same
/// address, the same size and the same contents, and the pool's total capacity is
/// unchanged."
#[requires(mt_inv(*st))]
#[ensures(result.3 == result.4)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> slot_at(st.slots@, k) && !slot_at((^st).slots@, k))]
#[ensures(forall<k: Key, k3: Key> result.0 == Some(k) && k3 != k
          ==> slot_at((^st).slots@, k3) == slot_at(st.slots@, k3))]
#[ensures(forall<k: Key> result.0 == Some(k) && k2 != k ==> result.1 == result.2)]
#[ensures(result.0 == None ==> result.1 == result.2 && (^st).slots@ == st.slots@)]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures((^st).pool_base == st.pool_base)]
pub fn verify_mt_evict_next_frame_other_entries(st: &mut StateModel, k2: Key)
    -> (Option<Key>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_evict_next(st);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// Anti-vacuity twin: MUST FAIL (claims the EVICTED key's lookup is unchanged too).
#[requires(mt_inv(*st))]
#[ensures(result.3 == result.4)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> result.1 == result.2)]
pub fn verify_mt_evict_next_frame_other_entries__mutant(st: &mut StateModel, k2: Key)
    -> (Option<Key>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_evict_next(st);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// **MT-INV-UNIQUE-KEY** (invariant, 6 attachments)
///
/// "The cache holds at most one entry for any given key at any time, enforced both
/// by the duplicate check in the insertion path and by the keyed map the entries
/// are stored in, so a lookup can never be ambiguous and a deletion can never
/// leave a second copy behind."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// the duplicate check: inserting a key that is present is refused
#[ensures(result.0 && size@ > 0 ==> result.1 == Err(MtError::AlreadyExists))]
// at most one entry per key, on both sides of the key bookkeeping
#[ensures(forall<i: Int, j: Int> 0 <= i && i < (^st).slots@.len() && 0 <= j && j < (^st).slots@.len()
          && (^st).slots@[i].key == (^st).slots@[j].key ==> i == j)]
#[ensures(policy_keys_unique((^st).policy.tracked@))]
// a deletion leaves no second copy behind
#[ensures((^st).initialized ==> !slot_at((^st).slots@, k) && !tracks((^st).policy.tracked@, k))]
pub fn verify_mt_inv_unique_key(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, k: Key, size: u32)
    -> (bool, Result<usize, MtError>, Result<(), MtError>) {
    mt_run(st, ops, psz);
    proof_assert!(slot_keys_unique(st.slots@));
    let c = mt_contains(st, k);
    let ins = mt_insert(st, k, size);
    proof_assert!(slot_keys_unique(st.slots@));
    let rm = mt_remove(st, k);
    (c, ins, rm)
}

/// Anti-vacuity twin: MUST FAIL (claims a second copy survives the deletion).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(policy_keys_unique((^st).policy.tracked@))]
#[ensures((^st).initialized ==> slot_at((^st).slots@, k))]
pub fn verify_mt_inv_unique_key__mutant(st: &mut StateModel, k: Key, size: u32) {
    let _ = mt_insert(st, k, size);
    let _ = mt_remove(st, k);
}

/// **MT-INV-ENTRY-SIZE-NARROW-FIELD** (invariant, code-only, 6 attachments)
///
/// "An entry's size is stored in a thirty-two-bit field while the pool's capacity
/// and usage are counted in machine-word-sized numbers, so no single entry can ever
/// be larger than four gigabytes even in a much larger pool, and the conversions
/// between the two widths are unchecked."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// the stored size is a u32 and the space it reserves is at most 4 GiB, whatever the pool size
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> (^st).slots@[i].size@ <= 4294967295 && align_up_l((^st).slots@[i].size@) <= 4294967296)]
// a successful insertion charges at most 4 GiB to the machine-word `used` counter
#[ensures(match result.0 { Ok(_) => (^st).allocator.used@ - result.2@ <= 4294967296, _ => true })]
// the unchecked widening `size as usize` (lib.rs:362) and its use at lib.rs:457/501 lose nothing
#[ensures(result.1 == size)]
pub fn verify_mt_inv_entry_size_narrow_field(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, key: Key, size: u32)
    -> (Result<usize, MtError>, u32, usize) {
    mt_run(st, ops, psz);
    let u0 = st.allocator.used;
    let r = mt_insert(st, key, size);
    proof_assert!(forall<i: Int> 0 <= i && i < st.slots@.len() ==> st.slots@[i].size@ > 0);
    proof_assert!(forall<x: Int> 0 < x && x <= 4294967295 ==> align_up_l(x) <= 4294967296);
    let w = size as usize;
    let back = w as u32;
    proof_assert!(w@ == size@);
    (r, back, u0)
}

/// Anti-vacuity twin: MUST FAIL (claims the reserved span always fits the 32-bit field;
/// a request above 4 GiB - 4 KiB rounds up to exactly 2^32 > u32::MAX).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures(match result { Ok(_) => (^st).allocator.used@ - st.allocator.used@ <= 4294967295, _ => true })]
pub fn verify_mt_inv_entry_size_narrow_field__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-INV-NO-INTERNAL-VICTIM-SELECTION-STATE** (invariant, spec-only, 6 attachments)
///
/// "All decisions about which entry to evict come from the external eviction-policy
/// component; the memory tier itself stores no ordering, rotation or
/// region-selection information, so its eviction behaviour is fully determined by
/// that policy's state."
///
/// Two set-up memory tiers whose bound policies hold the SAME order make the same
/// eviction choice and report the same oldest keys, whatever else differs (their
/// entries' offsets, sizes, free lists, pool bases ...).
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(a.initialized && b.initialized)]
#[requires(a.policy.tracked@ == b.policy.tracked@)]
#[ensures(result.0 == result.1)]
#[ensures(result.2@ == result.3@)]
pub fn verify_mt_inv_no_internal_victim_selection_state(a: &mut StateModel, b: &mut StateModel, n: usize)
    -> (Option<Key>, Option<Key>, Vec<Key>, Vec<Key>) {
    let oa = mt_oldest_keys(a, n);
    let ob = mt_oldest_keys(b, n);
    proof_assert!(oa@.len() == ob@.len());
    proof_assert!(forall<i: Int> 0 <= i && i < oa@.len() ==> oa@[i] == ob@[i]);
    proof_assert!(a.policy.tracked@ == b.policy.tracked@);
    proof_assert!(a.policy.tracked@.len() > 0 ==> tracks(a.policy.tracked@, (a.policy.tracked@[0]).0));
    proof_assert!(a.policy.tracked@.len() > 0 ==> slot_at(a.slots@, (a.policy.tracked@[0]).0)
                  && slot_at(b.slots@, (b.policy.tracked@[0]).0));
    proof_assert!(a.policy.tracked@.len() > 0 ==> a.slots@.len() > 0 && b.slots@.len() > 0);
    let ra = mt_evict_next(a);
    let rb = mt_evict_next(b);
    (ra, rb, oa, ob)
}

/// Anti-vacuity twin: MUST FAIL (claims the memory tier's OWN state -- identical
/// entries -- determines the victim, with the policy order left free).
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(a.initialized && b.initialized)]
#[requires(a.slots@ == b.slots@)]
#[ensures(a.slots@ == b.slots@)]
#[ensures(result.0 == result.1)]
pub fn verify_mt_inv_no_internal_victim_selection_state__mutant(a: &mut StateModel, b: &mut StateModel)
    -> (Option<Key>, Option<Key>) {
    let ra = mt_evict_next(a);
    let rb = mt_evict_next(b);
    (ra, rb)
}

/// **MT-INV-NEVER-NOT-EVICTABLE** (invariant, 6 attachments)
///
/// "No operation of the component ever returns the not-evictable error: the
/// component declares seven kinds of error but only ever produces six ..."
///
/// The four `Result`-returning methods (initialize -- with its receptacle lookup --
/// insert, remove, clear) never return `NotEvictable`; evict_next /
/// evict_next_for_key return `Option<CacheKey>` and cannot carry an error at all;
/// the other eleven methods return no `MemoryTierError`.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ + 1 < u64::MAX@)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures(match result.1 { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures(match result.2 { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures(match result.3 { Err(MtError::NotEvictable) => false, _ => true })]
pub fn verify_mt_inv_never_not_evictable(st: &mut StateModel, psz: usize, connected: bool, key: Key, size: u32)
    -> (Result<(), MtError>, Result<usize, MtError>, Result<(), MtError>, Result<usize, MtError>) {
    let r0 = rx_initialize(st, psz, connected);
    let r1 = mt_insert(st, key, size);
    let _ = mt_evict_next(st);
    let _ = mt_evict_next_for_key(st, key);
    let r2 = mt_remove(st, key);
    let r3 = mt_clear(st);
    (r0, r1, r2, r3)
}

/// Anti-vacuity twin: MUST FAIL (claims insert never reports PoolFull either).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures(match result { Err(MtError::PoolFull) => false, _ => true })]
pub fn verify_mt_inv_never_not_evictable__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-INIT-FRAME-ON-FAILURE** (frame, 6 attachments)
///
/// "When setting up the pool fails for any reason the component is left exactly as
/// it was before the call: it still reports itself as not set up, reports no
/// capacity and no bytes in use, holds no entries, and retains no partially built
/// pool that a later call could stumble over."
///
/// Every failure path of `initialize` from the not-set-up state: zero size
/// (lib.rs:262), receptacle missing (lib.rs:266-268), pool allocation failed
/// (lib.rs:221 / :291-295 -- `alloc_pool` returns None).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) =>
    ^st == *st && !(^st).initialized && result.1@ == 0 && result.2@ == 0 && !result.3
    && result.4 == None && (^st).slots@.len() == 0 && (^st).policy.tracked@.len() == 0
    && (^st).allocator.regions@ == st.allocator.regions@ && result.5 == st.spdk_allocated })]
pub fn verify_mt_init_frame_on_failure(st: &mut StateModel, psz: usize, connected: bool, k: Key)
    -> (Result<(), MtError>, usize, usize, bool, Option<(usize, usize)>, bool) {
    let r = rx_initialize(st, psz, connected);
    let c = mt_capacity(st);
    let u = mt_used(st);
    let h = mt_contains(st, k);
    let pi = mt_pool_info(st);
    let d = mt_is_dma_capable(st);
    (r, c, u, h, pi, d)
}

/// Anti-vacuity twin: MUST FAIL (claims a failed set-up leaves the component set up).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result { Ok(_) => true, Err(_) => (^st).slots@.len() == 0 })]
#[ensures(match result { Ok(_) => true, Err(_) => (^st).initialized })]
pub fn verify_mt_init_frame_on_failure__mutant(st: &mut StateModel, psz: usize, connected: bool) -> Result<(), MtError> {
    rx_initialize(st, psz, connected)
}

/// **MT-INIT-POST-READY** (postcondition, 6 attachments)
///
/// "When the pool is set up successfully the component reports a total capacity
/// exactly equal to the number of bytes the caller requested, reports zero bytes in
/// use, and holds no cached entries at all, so no key is reported as present."
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1@ == psz@ && result.2@ == 0 && !result.3
          && result.4 == None && result.5 == None && (^st).slots@.len() == 0, Err(_) => true })]
pub fn verify_mt_init_post_ready(st: &mut StateModel, psz: usize, k: Key)
    -> (Result<(), MtError>, usize, usize, bool, Option<(usize, u32)>, Option<(usize, u32)>) {
    let r = mt_initialize(st, psz);
    let c = mt_capacity(st);
    let u = mt_used(st);
    let h = mt_contains(st, k);
    let p = mt_peek(st, k);
    let g = mt_get(st, k);
    (r, c, u, h, p, g)
}

/// Anti-vacuity twin: MUST FAIL (claims the whole pool is in use after set-up).
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1@ == psz@, Err(_) => true })]
#[ensures(match result.0 { Ok(_) => result.2@ == psz@, Err(_) => true })]
pub fn verify_mt_init_post_ready__mutant(st: &mut StateModel, psz: usize) -> (Result<(), MtError>, usize, usize) {
    let r = mt_initialize(st, psz);
    let c = mt_capacity(st);
    let u = mt_used(st);
    (r, c, u)
}

/// **MT-INSERT-POST-TRACKED** (postcondition, 6 attachments)
///
/// "After adding a cache entry succeeds that entry is registered with the external
/// eviction-policy component, so it immediately becomes a candidate that a later
/// eviction can select, that a later recency refresh can promote, and that can
/// appear in the list of oldest keys."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => tracks((^st).policy.tracked@, key), _ => true })]
// it appears among the oldest keys (ask for as many as are tracked)
#[ensures(match result.0 { Ok(_) => exists<i: Int> 0 <= i && i < result.1@.len() && result.1@[i] == key, _ => true })]
// a refresh promotes it to most-recently-used
#[ensures(match result.0 { Ok(_) => (^st).policy.tracked@.len() > 0
          && ((^st).policy.tracked@[(^st).policy.tracked@.len() - 1]).0 == key, _ => true })]
pub fn verify_mt_insert_post_tracked(st: &mut StateModel, key: Key, size: u32) -> (Result<usize, MtError>, Vec<Key>) {
    let r = mt_insert(st, key, size);
    proof_assert!(match r { Ok(_) => st.initialized && slot_at(st.slots@, key) && tracks(st.policy.tracked@, key), _ => true });
    let n = st.policy.tracked.len();
    let o = mt_oldest_keys(st, n);
    mt_touch(st, key);
    (r, o)
}

/// Anti-vacuity twin: MUST FAIL (claims the new entry is NOT handed to the policy).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result { Ok(_) => slot_at((^st).slots@, key), _ => true })]
#[ensures(match result { Ok(_) => !tracks((^st).policy.tracked@, key), _ => true })]
pub fn verify_mt_insert_post_tracked__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-TOUCH-FRAME** (frame, 6 attachments)
///
/// "Refreshing an entry changes only its position in the eviction order: the set
/// of keys present, each entry's address and size, the bytes in use and the total
/// capacity all stay the same."
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).pool_base == st.pool_base)]
#[ensures((^st).allocator.regions@ == st.allocator.regions@)]
#[ensures(forall<x: Key> tracks((^st).policy.tracked@, x) == tracks(st.policy.tracked@, x))]
#[ensures((^st).policy.tracked@.len() == st.policy.tracked@.len())]
#[ensures(forall<x: Key> mt_contains_l(^st, x) == mt_contains_l(*st, x))]
#[ensures(result.0 == result.2 && result.1 == result.3 && result.4 == result.5)]
pub fn verify_mt_touch_frame(st: &mut StateModel, key: Key, k: Key)
    -> (usize, usize, usize, usize, Option<(usize, u32)>, Option<(usize, u32)>) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let p0 = mt_peek(st, k);
    mt_touch(st, key);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    let p1 = mt_peek(st, k);
    (u0, c0, u1, c1, p0, p1)
}

/// Logical `contains` (the observable presence of `k`).
#[logic(open)]
pub fn mt_contains_l(st: StateModel, k: Key) -> bool {
    pearlite! { st.initialized && slot_at(st.slots@, k) }
}

/// Anti-vacuity twin: MUST FAIL (claims a refresh leaves the eviction ORDER unchanged).
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
pub fn verify_mt_touch_frame__mutant(st: &mut StateModel, key: Key) {
    mt_touch(st, key);
}

/// **MT-USED-POST-SUM** (postcondition, 6 attachments)
///
/// "The bytes-in-use query reports the total space occupied by all entries
/// currently in the cache, where each entry counts as its requested size rounded
/// up to the next multiple of four kibibytes."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result@ == slot_sum((^st).slots@))]
#[ensures(!(^st).initialized ==> result@ == 0)]
pub fn verify_mt_used_post_sum(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) -> usize {
    mt_run(st, ops, psz);
    proof_assert!(!st.initialized ==> st.slots@.len() == 0 && slot_sum(st.slots@) == 0);
    mt_used(st)
}

/// Anti-vacuity twin: MUST FAIL (claims in-use is the UNROUNDED sum of the sizes).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(!(^st).initialized ==> result@ == 0)]
#[ensures(result@ == raw_sum((^st).slots@))]
pub fn verify_mt_used_post_sum__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) -> usize {
    mt_run(st, ops, psz);
    mt_used(st)
}

/// **MT-FREELIST-CAPACITY-CONSTANT** (invariant, code-only, 5 attachments)
///
/// "The capacity a free list reports is fixed when it is created and is never
/// changed by any allocation or release, so the only way the component's capacity
/// can change is by building a whole new free list."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// allocate (any outcome) and release (ANY arguments the shape allows, valid or not)
#[ensures((^fl).capacity == fl.capacity)]
// component level: once set up, no sequence of calls changes the capacity, and it
// always equals the stored pool size `clear` rebuilds from
#[ensures(st.initialized ==> (^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).initialized ==> (^st).allocator.capacity@ == (^st).pool_size@)]
pub fn verify_mt_freelist_capacity_constant(
    fl: &mut FreeListModel, size: u32, off: usize, sz2: u32,
    st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize,
) -> Option<usize> {
    let r = fl_allocate(fl, size);
    if sz2 > 0 && off <= fl.capacity {
        let a = align_up(sz2 as usize);
        if a <= fl.capacity - off {
            fl_deallocate(fl, off, sz2);
        }
    }
    mt_run(st, ops, psz);
    r
}

/// Anti-vacuity twin: MUST FAIL (claims allocation consumes capacity).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(result == None ==> (^fl).capacity == fl.capacity)]
#[ensures(result != None ==> (^fl).capacity@ == fl.capacity@ - align_up_l(size@))]
pub fn verify_mt_freelist_capacity_constant__mutant(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// **MT-CONTAINS-POST-EXACT** (postcondition, 5 attachments)
///
/// "The presence check answers yes exactly when a live cache entry exists for that
/// key, so it answers yes after a successful insertion and no after the entry has
/// been deleted, evicted or wiped by a full clear."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(result.0 == (st.initialized && slot_at(st.slots@, k)))]
#[ensures(match result.1 { Ok(_) => result.2, _ => true })]
#[ensures(!result.3)]
#[ensures(!result.5)]
#[ensures(!result.6)]
pub fn verify_mt_contains_post_exact(st: &mut StateModel, k: Key, size: u32)
    -> (bool, Result<usize, MtError>, bool, bool, Option<Key>, bool, bool) {
    let c0 = mt_contains(st, k);
    let ins = mt_insert(st, k, size);
    let c1 = mt_contains(st, k);
    let _ = mt_remove(st, k);
    let c2 = mt_contains(st, k);
    let ev = mt_evict_next(st);
    let c3 = match ev {
        Some(kk) => mt_contains(st, kk),
        None => false,
    };
    let _ = mt_clear(st);
    let c4 = mt_contains(st, k);
    (c0, ins, c1, c2, ev, c3, c4)
}

/// Anti-vacuity twin: MUST FAIL (claims the key is still reported present after its deletion).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1, _ => true })]
#[ensures(match result.0 { Ok(_) => result.2, _ => true })]
pub fn verify_mt_contains_post_exact__mutant(st: &mut StateModel, k: Key, size: u32)
    -> (Result<usize, MtError>, bool, bool) {
    let ins = mt_insert(st, k, size);
    let c1 = mt_contains(st, k);
    let _ = mt_remove(st, k);
    let c2 = mt_contains(st, k);
    (ins, c1, c2)
}

/// **MT-EVICT-NEXT-POST-LRU-ORDER** (postcondition, spec-only, divergent, 5 attachments)
///
/// "With the least-recently-used policy that the system currently binds, an entry
/// that has been read or explicitly refreshed more recently is never evicted before
/// an entry that has not been used since."
///
/// After `get(a)` (`via_get`) or `touch(a)`, with another entry `b` present, the
/// next eviction is not `a`: memory-tier forwards the refresh to the bound policy
/// (lib.rs:410 / :516) and takes the victim from it (lib.rs:441); the LRU order
/// itself is the policy mirror (`policy_touch_key` = `LruList::move_to_back`,
/// `policy_identify_next_to_evict` = `pop_front`).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(slot_at(st.slots@, a) && slot_at(st.slots@, b) && a != b)]
#[ensures(result != None)]
#[ensures(result != Some(a))]
pub fn verify_mt_evict_next_post_lru_order(st: &mut StateModel, a: Key, b: Key, via_get: bool) -> Option<Key> {
    if via_get {
        let _ = mt_get(st, a);
    } else {
        mt_touch(st, a);
    }
    let t = snapshot! { st.policy.tracked@ };
    proof_assert!(tracks(*t, a) && tracks(*t, b));
    proof_assert!(t.len() > 0 && (t[t.len() - 1]).0 == a);
    proof_assert!(t.len() >= 2);
    proof_assert!((t[0]).0 != a);
    proof_assert!(slot_at(st.slots@, b) && st.slots@.len() > 0);
    mt_evict_next(st)
}

/// Anti-vacuity twin: MUST FAIL (the same claim WITHOUT the refresh: an un-refreshed
/// entry may well be the least recently used one).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(slot_at(st.slots@, a) && slot_at(st.slots@, b) && a != b)]
#[ensures(result != None)]
#[ensures(result != Some(a))]
pub fn verify_mt_evict_next_post_lru_order__mutant(st: &mut StateModel, a: Key, b: Key) -> Option<Key> {
    proof_assert!(st.slots@.len() > 0);
    mt_evict_next(st)
}

/// **MT-INV-UNALIGNED-CAPACITY-TAIL** (invariant, code-only, 5 attachments)
///
/// "Nothing requires the requested pool size to be a whole number of
/// four-kibibyte units, and when it is not, the leftover bytes at the end of the
/// pool can never be allocated to anyone, so the component reports a capacity it
/// can never fully hand out."
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(psz@ % 4096 != 0)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
// set-up accepts the unaligned size
#[ensures(result == Ok(()))]
#[ensures((^st).initialized && (^st).allocator.capacity@ == psz@)]
// after ANY sequence of calls, no entry reaches into the tail ...
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> sl_end((^st).slots@[i]) <= psz@ - psz@ % 4096)]
// ... and the reported capacity is never fully in use
#[ensures((^st).allocator.used@ <= psz@ - psz@ % 4096)]
#[ensures((^st).allocator.used@ < (^st).allocator.capacity@)]
pub fn verify_mt_inv_unaligned_capacity_tail(st: &mut StateModel, psz: usize, b: usize, ops: &[(u8, Key, u32)])
    -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    let r = init_logged(st, psz, Some(b), &mut lg, false);
    mt_run(st, ops, psz);
    snapshot! { lem_ss_aligned(st.slots@) };
    proof_assert!(st.allocator.used@ % 4096 == 0);
    proof_assert!(st.allocator.used@ <= st.allocator.capacity@ || st.allocator.used@ >= 0);
    proof_assert!(forall<j: Int> 0 <= j && j < st.allocator.regions@.len() ==> st.allocator.regions@[j].size@ > 0);
    snapshot! { lem_fs_bound(st.allocator.regions@) };
    proof_assert!(st.allocator.used@ <= psz@);
    snapshot! { lem_floor(st.allocator.used@, psz@) };
    proof_assert!(forall<i: Int> 0 <= i && i < st.slots@.len() ==> sl_end(st.slots@[i]) % 4096 == 0
                  && 0 <= sl_end(st.slots@[i]) && sl_end(st.slots@[i]) <= psz@);
    proof_assert!(forall<x: Int> x % 4096 == 0 && 0 <= x && x <= psz@ ==> x <= psz@ - psz@ % 4096);
    r
}

/// Anti-vacuity twin: MUST FAIL (the same claim with an ALIGNED pool size allowed:
/// then the pool can be filled completely).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[ensures(result == Ok(()))]
#[ensures((^st).allocator.used@ < (^st).allocator.capacity@)]
pub fn verify_mt_inv_unaligned_capacity_tail__mutant(st: &mut StateModel, psz: usize, b: usize, ops: &[(u8, Key, u32)])
    -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    let r = init_logged(st, psz, Some(b), &mut lg, false);
    mt_run(st, ops, psz);
    r
}

/// **MT-INV-OBSERVER-CONSISTENCY** (invariant, spec-only, 5 attachments)
///
/// "The presence check, the two lookup operations, the count returned when the
/// cache is wiped and the bytes-in-use figure all agree about which entries exist:
/// a key reported as present is exactly a key whose lookups succeed and whose space
/// is counted in the bytes in use."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// presence check == peek succeeds == get succeeds
#[ensures(result.0 == (result.1 != None))]
#[ensures(result.0 == (result.2 != None))]
// a present key's (rounded) space is counted in bytes-in-use, and in-use is
// exactly the space of the present keys (`result.5` = the entries at observation time)
#[ensures(result.0 ==> exists<i: Int> 0 <= i && i < result.5.len() && result.5[i].key == k
          && align_up_l(result.5[i].size@) <= result.3@)]
#[ensures(result.3@ == slot_sum(*result.5))]
#[ensures(result.0 == slot_at(*result.5, k))]
// the wipe count is the number of present keys (one slot per key)
#[ensures(match result.4 { Ok(cnt) => cnt@ == result.5.len() && slot_keys_unique(*result.5),
                           Err(_) => result.5.len() == 0 })]
pub fn verify_mt_inv_observer_consistency(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, k: Key)
    -> (bool, Option<(usize, u32)>, Option<(usize, u32)>, usize, Result<usize, MtError>, Snapshot<Seq<SlotModel>>) {
    mt_run(st, ops, psz);
    let c = mt_contains(st, k);
    let p = mt_peek(st, k);
    let u = mt_used(st);
    let g = mt_get(st, k);
    proof_assert!(!st.initialized ==> st.slots@.len() == 0 && slot_sum(st.slots@) == 0);
    proof_assert!(forall<j: Int> 0 <= j && j < st.slots@.len() ==> st.slots@[j].size@ > 0);
    snapshot! { lem_ss_bound(st.slots@) };
    let mid = snapshot! { st.slots@ };
    let cl = mt_clear(st);
    (c, p, g, u, cl, mid)
}

/// Anti-vacuity twin: MUST FAIL (claims presence check and peek DISAGREE).
#[requires(mt_inv(*st))]
#[ensures(result.0 == (result.1 == None))]
pub fn verify_mt_inv_observer_consistency__mutant(st: &mut StateModel, k: Key) -> (bool, Option<(usize, u32)>) {
    let c = mt_contains(st, k);
    let p = mt_peek(st, k);
    (c, p)
}

/// GHOST trace of the eviction-order refresh calls memory-tier issues
/// (`ep.touch(handle)` lib.rs:410 / :516, `ep.batch_touch(&handles)` lib.rs:558).
pub struct RefreshLog {
    pub handles: Vec<u64>,
}

/// The refresh call itself. Only the CALL is modelled -- what the bound policy
/// does with a handle whose entry is gone is the policy's business and is NOT
/// claimed here (`IEvictionPolicy::touch` documents no stale-handle behaviour).
#[ensures((^log).handles@ == log.handles@.push_back(h))]
pub fn issue_refresh(log: &mut RefreshLog, h: u64) {
    log.handles.push(h);
}

/// Phase 1 of `get` (lib.rs:403-408) / `touch` (lib.rs:513-514) / one key of
/// `batch_touch` (lib.rs:550-552): under the pool READ lock, copy the slot's
/// address, size and eviction handle OUT of the map.
#[requires(mt_inv(*st))]
#[ensures(match result {
    Some((p, s, h)) => st.initialized && exists<i: Int> 0 <= i && i < st.slots@.len()
        && st.slots@[i].key == key && p@ == st.pool_base@ + st.slots@[i].offset@
        && s == st.slots@[i].size && h == st.slots@[i].handle,
    None => !(st.initialized && slot_at(st.slots@, key)),
})]
pub fn lookup_handle(st: &StateModel, key: Key) -> Option<(usize, u32, u64)> {
    if !st.initialized {
        return None;
    }
    let n = st.slots.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == st.slots@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> st.slots@[j].key != key)]
    while i < n {
        let sl = st.slots[i];
        if sl.key == key {
            return Some((st.pool_base + sl.offset, sl.size, sl.handle));
        }
        i += 1;
    }
    None
}

/// **MT-INV-HANDLE-CAN-OUTLIVE-ENTRY** (invariant, code-only, 5 attachments) -- HAZARD
///
/// "An entry's eviction handle is a freely copyable value, and three operations copy
/// it out of the cache, release the pool lock and only then use it to refresh the
/// eviction order, so in that window another thread may delete or evict the entry
/// and the refresh is then applied to a handle whose entry no longer exists;
/// nothing detects or prevents this."
///
/// The interleaving at lock granularity: thread A's `get(key)` critical section
/// (copy out), A releases the pool lock (`drop(pool)`, lib.rs:409), thread B's
/// whole `remove(key)` critical section runs, then A's `ep.touch(handle)`
/// (lib.rs:410) -- with no re-check.
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
// A's get reports success ...
#[ensures(result.0 != None)]
// ... B's deletion succeeded in the window ...
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
// ... so when the refresh is issued the entry is gone from the pool AND the policy ...
#[ensures(!slot_at((^st).slots@, key) && !tracks((^st).policy.tracked@, key))]
// ... yet the refresh IS issued, with exactly the deleted entry's handle
#[ensures(exists<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == key
          && (^log).handles@ == log.handles@.push_back(st.slots@[i].handle))]
pub fn verify_mt_inv_handle_can_outlive_entry(st: &mut StateModel, key: Key, log: &mut RefreshLog)
    -> (Option<(usize, u32)>, Result<(), MtError>) {
    let ph = lookup_handle(st, key); // A: pool.read(); slots.get(&key); copy handle
    // A: drop(pool) -- B: remove(key) runs to completion
    let rm = mt_remove(st, key);
    // A: `let _ = ep.touch(handle);` -- no re-validation of the slot
    let out = match ph {
        Some((p, s, h)) => {
            issue_refresh(log, h);
            Some((p, s))
        }
        None => None,
    };
    (out, rm)
}

/// Anti-vacuity twin: MUST FAIL (claims the entry is still live when the refresh is issued).
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
#[ensures(result.0 != None)]
#[ensures(slot_at((^st).slots@, key))]
pub fn verify_mt_inv_handle_can_outlive_entry__mutant(st: &mut StateModel, key: Key, log: &mut RefreshLog)
    -> (Option<(usize, u32)>, Result<(), MtError>) {
    let ph = lookup_handle(st, key);
    let rm = mt_remove(st, key);
    let out = match ph {
        Some((p, s, h)) => {
            issue_refresh(log, h);
            Some((p, s))
        }
        None => None,
    };
    (out, rm)
}

/// **MT-INV-FOUR-KIB-ALIGNMENT** (invariant, divergent, 5 attachments)
///
/// "Every entry's position within the pool and every entry's reserved length are
/// whole multiples of four kibibytes, which is what makes the addresses handed out
/// four-kibibyte aligned and keeps the bytes-in-use total a whole number of
/// four-kibibyte units."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> (^st).slots@[i].offset@ % 4096 == 0 && align_up_l((^st).slots@[i].size@) % 4096 == 0)]
#[ensures((^st).initialized ==> forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> ((^st).pool_base@ + (^st).slots@[i].offset@) % 4096 == 0)]
#[ensures((^st).allocator.used@ % 4096 == 0)]
#[ensures(forall<a: usize, s: u32> result == Some((a, s)) ==> a@ % 4096 == 0)]
pub fn verify_mt_inv_four_kib_alignment(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, k: Key)
    -> Option<(usize, u32)> {
    mt_run(st, ops, psz);
    snapshot! { lem_ss_aligned(st.slots@) };
    proof_assert!(st.allocator.used@ == slot_sum(st.slots@));
    mt_peek(st, k)
}

/// Anti-vacuity twin: MUST FAIL (claims the REQUESTED size is a 4 KiB multiple too;
/// only the reserved length is).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> (^st).slots@[i].offset@ % 4096 == 0)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> (^st).slots@[i].size@ % 4096 == 0)]
pub fn verify_mt_inv_four_kib_alignment__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
}

/// **MT-INV-SIZE-ACCOUNTING** -- REFUTATION of the wording (divergent, 5 attachments)
///
/// Statement: "... adding up the sizes of all entries always gives a SMALLER number
/// than the reported bytes in use." Negation proved: from a fresh pool, (a) with no
/// entries the two figures are equal (0 == 0), and (b) after inserting one entry of
/// 4096 bytes the requested sizes add up to EXACTLY the bytes in use.
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ >= 4096 && psz@ + 4096 <= usize::MAX@)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(st.policy.pools@ + 1 < u64::MAX@ && st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(result.0 == Ok(()))]
#[ensures(result.1@ == 0)]
#[ensures(match result.2 { Ok(_) => true, Err(_) => false })]
#[ensures((^st).slots@.len() == 1 && raw_sum((^st).slots@) == (^st).allocator.used@)]
#[ensures(!(raw_sum((^st).slots@) < (^st).allocator.used@))]
pub fn refute_mt_inv_size_accounting(st: &mut StateModel, psz: usize, b: usize, k: Key)
    -> (Result<(), MtError>, usize, Result<usize, MtError>) {
    let mut lg = LogSink { lines: 0 };
    let r = init_logged(st, psz, Some(b), &mut lg, false);
    let u0 = mt_used(st);
    proof_assert!(st.slots@ == Seq::empty() && raw_sum(st.slots@) == 0);
    proof_assert!(st.allocator.regions@[0].size@ >= 4096);
    proof_assert!(align_up_l(4096) == 4096);
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(4096)));
    proof_assert!(!slot_at(st.slots@, k));
    let ins = mt_insert(st, k, 4096);
    proof_assert!(st.slots@.len() == 1 && st.slots@[0].size@ == 4096);
    proof_assert!(st.slots@.subsequence(0, 0) == Seq::empty());
    proof_assert!(raw_sum(st.slots@) == 4096);
    (r, u0, ins)
}

/// Anti-vacuity twin: MUST FAIL (asserts the WORDING -- "smaller" -- in that scenario).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ >= 4096 && psz@ + 4096 <= usize::MAX@)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(st.policy.pools@ + 1 < u64::MAX@ && st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(result.0 == Ok(()))]
#[ensures(raw_sum((^st).slots@) < (^st).allocator.used@)]
pub fn refute_mt_inv_size_accounting__mutant(st: &mut StateModel, psz: usize, b: usize, k: Key)
    -> (Result<(), MtError>, Result<usize, MtError>) {
    let mut lg = LogSink { lines: 0 };
    let r = init_logged(st, psz, Some(b), &mut lg, false);
    let ins = mt_insert(st, k, 4096);
    (r, ins)
}

/// UNSCORED WITNESS for MT-INV-SIZE-ACCOUNTING -- the intended NON-STRICT obligation,
/// proved for every reachable state: the size reported on lookup is the size asked
/// for; the sum of the requested sizes is <= bytes in use, with EQUALITY exactly
/// when every requested size is a 4 KiB multiple (strictly smaller otherwise).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(raw_sum((^st).slots@) <= (^st).allocator.used@)]
#[ensures((forall<j: Int> 0 <= j && j < (^st).slots@.len() ==> (^st).slots@[j].size@ % 4096 == 0)
          ==> raw_sum((^st).slots@) == (^st).allocator.used@)]
#[ensures((exists<j: Int> 0 <= j && j < (^st).slots@.len() && (^st).slots@[j].size@ % 4096 != 0)
          ==> raw_sum((^st).slots@) < (^st).allocator.used@)]
#[ensures(match result.0 { Ok(_) => exists<p: usize> result.1 == Some((p, sz)), _ => true })]
pub fn witness_mt_inv_size_accounting_nonstrict(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, k: Key, sz: u32)
    -> (Result<usize, MtError>, Option<(usize, u32)>) {
    mt_run(st, ops, psz);
    let ins = mt_insert(st, k, sz);
    let pk = mt_peek(st, k);
    proof_assert!(forall<j: Int> 0 <= j && j < st.slots@.len() ==> st.slots@[j].size@ > 0);
    snapshot! { lem_raw(st.slots@) };
    proof_assert!(st.allocator.used@ == slot_sum(st.slots@));
    (ins, pk)
}

/// **MT-INSERT-FRAME-OTHER-ENTRIES** (frame, spec-only, 5 attachments)
///
/// "Adding a cache entry leaves every other entry untouched: their addresses, their
/// sizes and the bytes they hold are unchanged, none of them is removed, and the
/// pool's total capacity stays the same."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[requires(k2 != key)]
#[ensures(result.1 == result.2)]
#[ensures(result.3 == result.4)]
#[ensures(forall<x: Key> x != key ==> slot_at((^st).slots@, x) == slot_at(st.slots@, x))]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures((^st).pool_base == st.pool_base)]
pub fn verify_mt_insert_frame_other_entries(st: &mut StateModel, key: Key, size: u32, k2: Key)
    -> (Result<usize, MtError>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_insert(st, key, size);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// Anti-vacuity twin: MUST FAIL (drops `k2 != key`: the inserted key's own lookup changes).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(result.3 == result.4)]
#[ensures(result.1 == result.2)]
pub fn verify_mt_insert_frame_other_entries__mutant(st: &mut StateModel, key: Key, size: u32, k2: Key)
    -> (Result<usize, MtError>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_insert(st, key, size);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// **MT-REMOVE-FRAME-OTHER-ENTRIES** (frame, 5 attachments)
///
/// "Deleting one entry leaves all other entries with the same addresses, sizes and
/// contents, keeps them all present, and does not change the pool's total capacity."
#[requires(mt_inv(*st))]
#[requires(k2 != key)]
#[ensures(result.1 == result.2)]
#[ensures(result.3 == result.4)]
#[ensures(forall<x: Key> x != key ==> slot_at((^st).slots@, x) == slot_at(st.slots@, x))]
#[ensures(slot_stable(st.slots@, (^st).slots@))]
#[ensures((^st).pool_base == st.pool_base)]
pub fn verify_mt_remove_frame_other_entries(st: &mut StateModel, key: Key, k2: Key)
    -> (Result<(), MtError>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_remove(st, key);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// Anti-vacuity twin: MUST FAIL (drops `k2 != key`: the deleted key's lookup changes).
#[requires(mt_inv(*st))]
#[ensures(result.3 == result.4)]
#[ensures(result.1 == result.2)]
pub fn verify_mt_remove_frame_other_entries__mutant(st: &mut StateModel, key: Key, k2: Key)
    -> (Result<(), MtError>, Option<(usize, u32)>, Option<(usize, u32)>, usize, usize) {
    let p0 = mt_peek(st, k2);
    let c0 = mt_capacity(st);
    let r = mt_remove(st, key);
    let p1 = mt_peek(st, k2);
    let c1 = mt_capacity(st);
    (r, p0, p1, c0, c1)
}

/// Mirror of `MemoryTierTelemetry` (lib.rs:42-47, `telemetry` feature): three
/// `AtomicU64` counters, modelled by value (each is a single atomic RMW target;
/// `Relaxed` ordering is irrelevant to one counter's own modification order).
pub struct TelemetryModel {
    pub evictions: u64,
    pub write_lock_contentions: u64,
    pub read_lock_contentions: u64,
}

/// `AtomicU64::fetch_add(1, Ordering::Relaxed)` (lib.rs:348, 396, 460, ...):
/// std documents "This operation wraps around on overflow" -- no panic, in any
/// build profile.
#[ensures(c@ < u64::MAX@ ==> result@ == c@ + 1)]
#[ensures(c@ == u64::MAX@ ==> result@ == 0)]
pub fn fetch_add_one(c: u64) -> u64 {
    if c == u64::MAX { 0 } else { c + 1 }
}

/// One counted event: 0 = eviction (lib.rs:460), 1 = write-lock contention
/// (lib.rs:345-349, 449-453, 488-492), otherwise read-lock contention (lib.rs:393-397, 538-542).
#[ensures(which@ == 0 ==> (^t).evictions@ == fetch_l(t.evictions@)
          && (^t).write_lock_contentions == t.write_lock_contentions
          && (^t).read_lock_contentions == t.read_lock_contentions)]
#[ensures(which@ == 1 ==> (^t).write_lock_contentions@ == fetch_l(t.write_lock_contentions@)
          && (^t).evictions == t.evictions && (^t).read_lock_contentions == t.read_lock_contentions)]
#[ensures(which@ >= 2 ==> (^t).read_lock_contentions@ == fetch_l(t.read_lock_contentions@)
          && (^t).evictions == t.evictions && (^t).write_lock_contentions == t.write_lock_contentions)]
pub fn tel_event(t: &mut TelemetryModel, which: u8) {
    if which == 0 {
        t.evictions = fetch_add_one(t.evictions);
    } else if which == 1 {
        t.write_lock_contentions = fetch_add_one(t.write_lock_contentions);
    } else {
        t.read_lock_contentions = fetch_add_one(t.read_lock_contentions);
    }
}

/// The wrapping successor.
#[logic(open)]
pub fn fetch_l(c: Int) -> Int {
    pearlite! { if c < 18446744073709551615 { c + 1 } else { 0 } }
}

/// `MemoryTierTelemetry::reset` (lib.rs:58-62) / `reset_telemetry` (lib.rs:177-181).
#[ensures((^t).evictions@ == 0 && (^t).write_lock_contentions@ == 0 && (^t).read_lock_contentions@ == 0)]
pub fn tel_reset(t: &mut TelemetryModel) {
    t.evictions = 0;
    t.write_lock_contentions = 0;
    t.read_lock_contentions = 0;
}

/// `telemetry_snapshot` (lib.rs:612-625): three `load(Relaxed)`s.
#[ensures(result == (t.evictions, t.write_lock_contentions, t.read_lock_contentions))]
pub fn tel_snapshot(t: &TelemetryModel) -> (u64, u64, u64) {
    (t.evictions, t.write_lock_contentions, t.read_lock_contentions)
}

/// **MT-TELEMETRY-COUNTERS-MONOTONIC** -- REFUTATION (divergent, 5 attachments)
///
/// Statement: "Every telemetry counter only ever grows, one step at a time, ...
/// and never decreases, except when the dedicated reset operation is deliberately
/// called." Negation proved: a counter at `u64::MAX` that records one more event
/// (no reset involved) reads 0 at the next snapshot -- it DECREASED.
#[requires(t.evictions@ == u64::MAX@)]
#[ensures(result.1@ < result.0@)]
#[ensures(result.1@ == 0)]
pub fn refute_mt_telemetry_counters_monotonic(t: &mut TelemetryModel) -> (u64, u64) {
    let (e0, _w0, _r0) = tel_snapshot(t);
    tel_event(t, 0); // one more eviction, lib.rs:460 -- no reset anywhere
    let (e1, _w1, _r1) = tel_snapshot(t);
    (e0, e1)
}

/// Anti-vacuity twin: MUST FAIL (asserts the unqualified monotonicity at the wrap).
#[requires(t.evictions@ == u64::MAX@)]
#[ensures(result.3 == result.2)]
#[ensures(result.1@ >= result.0@)]
pub fn refute_mt_telemetry_counters_monotonic__mutant(t: &mut TelemetryModel) -> (u64, u64, u64, u64) {
    let (e0, w0, _r0) = tel_snapshot(t);
    tel_event(t, 0);
    let (e1, w1, _r1) = tel_snapshot(t);
    (e0, e1, w0, w1)
}

/// UNSCORED WITNESS for MT-TELEMETRY-COUNTERS-MONOTONIC -- the bounded version that
/// DOES hold: below the wrap point, every event raises exactly one counter by exactly
/// one, so over any event sequence each counter is non-decreasing and grows by at
/// most the number of events; reset sets all three to zero.
#[requires(t.evictions@ + evs@.len() < u64::MAX@)]
#[requires(t.write_lock_contentions@ + evs@.len() < u64::MAX@)]
#[requires(t.read_lock_contentions@ + evs@.len() < u64::MAX@)]
#[ensures(t.evictions@ <= (^t).evictions@ && (^t).evictions@ <= t.evictions@ + evs@.len())]
#[ensures(t.write_lock_contentions@ <= (^t).write_lock_contentions@
          && (^t).write_lock_contentions@ <= t.write_lock_contentions@ + evs@.len())]
#[ensures(t.read_lock_contentions@ <= (^t).read_lock_contentions@
          && (^t).read_lock_contentions@ <= t.read_lock_contentions@ + evs@.len())]
#[ensures((^t).evictions@ + (^t).write_lock_contentions@ + (^t).read_lock_contentions@
          == t.evictions@ + t.write_lock_contentions@ + t.read_lock_contentions@ + evs@.len())]
pub fn witness_mt_telemetry_counters_monotonic_bounded(t: &mut TelemetryModel, evs: &[u8]) {
    let t0 = snapshot! { *t };
    let n = evs.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(t0.evictions@ <= t.evictions@ && t.evictions@ <= t0.evictions@ + i@)]
    #[invariant(t0.write_lock_contentions@ <= t.write_lock_contentions@
                && t.write_lock_contentions@ <= t0.write_lock_contentions@ + i@)]
    #[invariant(t0.read_lock_contentions@ <= t.read_lock_contentions@
                && t.read_lock_contentions@ <= t0.read_lock_contentions@ + i@)]
    #[invariant(t.evictions@ + t.write_lock_contentions@ + t.read_lock_contentions@
                == t0.evictions@ + t0.write_lock_contentions@ + t0.read_lock_contentions@ + i@)]
    while i < n {
        tel_event(t, evs[i]);
        i += 1;
    }
}

/// **MT-FREELIST-ALLOCATE-FRAME** (frame, code-only, 4 attachments)
///
/// "Allocation never changes the total capacity and never moves or shrinks any free
/// run other than the single run it takes space from."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures((^fl).capacity == fl.capacity)]
#[ensures(result == None ==> (^fl).regions@ == fl.regions@)]
// every run other than the one taken survives, unmoved and unshrunk ...
#[ensures(forall<o: usize, j: Int> result == Some(o) && 0 <= j && j < fl.regions@.len() && fl.regions@[j].offset != o
          ==> exists<k: Int> 0 <= k && k < (^fl).regions@.len() && (^fl).regions@[k] == fl.regions@[j])]
// ... and every run afterwards is such a survivor or the remainder of the taken run
#[ensures(forall<o: usize, k: Int> result == Some(o) && 0 <= k && k < (^fl).regions@.len()
          ==> (exists<j: Int> 0 <= j && j < fl.regions@.len() && fl.regions@[j].offset != o
                  && (^fl).regions@[k] == fl.regions@[j])
              || (^fl).regions@[k].offset@ == o@ + align_up_l(size@))]
// the taken run is the one starting at the returned offset
#[ensures(forall<o: usize> result == Some(o) ==> exists<i: Int> 0 <= i && i < fl.regions@.len()
          && fl.regions@[i].offset == o && alloc_step(fl.regions@, (^fl).regions@, i, align_up_l(size@)))]
pub fn verify_mt_freelist_allocate_frame(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// Anti-vacuity twin: MUST FAIL (claims a successful allocation leaves the free runs as they were).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures((^fl).capacity == fl.capacity)]
#[ensures((^fl).regions@ == fl.regions@)]
pub fn verify_mt_freelist_allocate_frame__mutant(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// **MT-FREELIST-NEW-ZERO-CAPACITY** (error-case, code-only, 4 attachments)
///
/// "A free list created with zero capacity records no free run at all, so every
/// later allocation request from it fails; this is exactly the state the component
/// sits in before its pool has been set up."
#[requires(p.tracked@.len() == 0)]
#[ensures(result.0 == None && result.1 == None)]
#[ensures(result.2@.len() == 0)]
#[ensures(result.3.allocator.regions@.len() == 0 && result.3.allocator.capacity@ == 0 && !result.3.initialized)]
pub fn verify_mt_freelist_new_zero_capacity(s1: u32, s2: u32, p: PolicyModel)
    -> (Option<usize>, Option<usize>, Vec<Region>, StateModel) {
    let mut fl = fl_new(0);
    proof_assert!(free_sum(fl.regions@) == 0);
    let r1 = fl_allocate(&mut fl, s1);
    let r2 = fl_allocate(&mut fl, s2); // ... and every later one
    let d = mt_default(p);             // lib.rs:105 `FreeList::new(0)` in Default
    (r1, r2, fl.regions, d)
}

/// Anti-vacuity twin: MUST FAIL (claims a non-zero request is served from it).
#[ensures(s1@ > 0 ==> result != None)]
pub fn verify_mt_freelist_new_zero_capacity__mutant(s1: u32) -> Option<usize> {
    let mut fl = fl_new(0);
    fl_allocate(&mut fl, s1)
}

/// `touch(k)` for each key of `keys`, one call per key (the "individually" side of
/// MT-BATCH-TOUCH-POST-ALL-PRESENT).
#[requires(mt_inv(*st))]
#[ensures(mt_inv(^st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(!st.initialized ==> ^st == *st)]
#[ensures(st.initialized ==> (^st).policy.tracked@ == touch_all_l(st.policy.tracked@, filter_l(keys@, st.slots@)))]
pub fn touch_each(st: &mut StateModel, keys: &[Key]) {
    let st0 = snapshot! { *st };
    let n = keys.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(n@ == keys@.len())]
    #[invariant(mt_inv(*st))]
    #[invariant(refresh_frame(*st0, *st))]
    #[invariant(!st0.initialized ==> *st == *st0)]
    #[invariant(st0.initialized ==> st.policy.tracked@
                == touch_all_l(st0.policy.tracked@, filter_l(keys@.subsequence(0, i@), st0.slots@)))]
    while i < n {
        snapshot! { lem_filter_step(keys@, st0.slots@, i@) };
        snapshot! { lem_ta_push(st0.policy.tracked@, filter_l(keys@.subsequence(0, i@), st0.slots@), keys@[i@]) };
        mt_touch(st, keys[i]);
        i += 1;
    }
    proof_assert!(keys@.subsequence(0, n@) == keys@);
}

/// **MT-BATCH-TOUCH-POST-ALL-PRESENT** (postcondition, 4 attachments)
///
/// "Refreshing many entries at once updates the recency of every key in the list
/// that is present in the cache, giving the same result as refreshing each of those
/// keys individually, and it gathers all their eviction handles under a single
/// acquisition of the pool's shared lock."
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(same_state(*a, *b))]
// the same result as refreshing each key individually
#[ensures(same_obs(^a, ^b))]
// = refresh, in list order, exactly the listed keys that are present
#[ensures(a.initialized ==> (^a).policy.tracked@ == touch_all_l(a.policy.tracked@, filter_l(keys@, a.slots@)))]
// one shared pool-lock acquisition for the whole batch
#[ensures(result@.len() == 4 && result@[1] == LkEv::PoolRead
          && (forall<i: Int> 0 <= i && i < 4 && i != 1 ==> result@[i] != LkEv::PoolRead))]
pub fn verify_mt_batch_touch_post_all_present(a: &mut StateModel, b: &mut StateModel, keys: &[Key]) -> Vec<LkEv> {
    mt_batch_touch(a, keys);
    touch_each(b, keys);
    proof_assert!(a.initialized ==> keys@.len() == 0 ==> filter_l(keys@, a.slots@) == Seq::empty());
    lk_batch_touch(true, true, true)
}

/// Anti-vacuity twin: MUST FAIL (claims a batch refresh leaves the order unchanged).
#[requires(mt_inv(*a))]
#[ensures(refresh_frame(*a, ^a))]
#[ensures((^a).policy.tracked@ == a.policy.tracked@)]
pub fn verify_mt_batch_touch_post_all_present__mutant(a: &mut StateModel, keys: &[Key]) {
    mt_batch_touch(a, keys);
}

/// **MT-CLEAR-FRAME-POOL-MEMORY** (frame, divergent, 4 attachments)
///
/// "Wiping the cache removes the entries but keeps the pool itself: the total
/// capacity, the pool's base address and whether the memory is directly usable by
/// storage hardware are all unchanged, and the component remains set up and ready
/// for further use."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(result.0 == result.1)]
#[ensures(result.2 == result.3)]
#[ensures(result.4 == result.5)]
#[ensures((^st).initialized && (^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size)]
#[ensures(match result.6 { Ok(_) => true, Err(_) => false })]
// the divergence: the rebuilt capacity comes from `pool_size`, which equals the old capacity
#[ensures(result.1@ == st.pool_size@ && st.allocator.capacity@ == st.pool_size@)]
#[ensures(mt_inv(^st))]
pub fn verify_mt_clear_frame_pool_memory(st: &mut StateModel)
    -> (usize, usize, Option<(usize, usize)>, Option<(usize, usize)>, bool, bool, Result<usize, MtError>) {
    let c0 = mt_capacity(st);
    let pi0 = mt_pool_info(st);
    let d0 = mt_is_dma_capable(st);
    let r = mt_clear(st);
    let c1 = mt_capacity(st);
    let pi1 = mt_pool_info(st);
    let d1 = mt_is_dma_capable(st);
    (c0, c1, pi0, pi1, d0, d1, r)
}

/// Anti-vacuity twin: MUST FAIL (claims the wipe leaves bytes-in-use unchanged too).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).allocator.used == st.allocator.used)]
pub fn verify_mt_clear_frame_pool_memory__mutant(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// **MT-INV-FREE-LIST-COALESCING** (invariant, 5 attachments)
///
/// "Whenever a run is released it is merged with any free neighbour on either side,
/// so no two free runs overlap, none has zero size and no two are immediately
/// adjacent, and the space of two entries that sat next to each other becomes one
/// larger free run that can satisfy an insertion as large as the two of them
/// together."
///
/// Part 1 (the pair): entries `a = slots[ia]` and `b = slots[ib]` with `a` ending
/// exactly where `b` starts are both deleted (in EITHER order); afterwards ONE free
/// run covers `[a.offset, b.offset + rounded(b))`, so an insertion of their combined
/// rounded size succeeds (when it fits the `u32` size parameter).
/// Part 2 (the invariant): after ANY further sequence of calls, the free runs are
/// pairwise disjoint, non-empty, and no two touch (`no_adj`, part of `fl_inv`).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.initialized)]
#[requires(ia@ < st.slots@.len() && ib@ < st.slots@.len())]
#[requires(sl_end(st.slots@[ia@]) == st.slots@[ib@].offset@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => false })]
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
// one run now covers both entries' space ...
#[ensures(covered(*result.3, st.slots@[ia@].offset@, sl_end(st.slots@[ib@])))]
#[ensures(some_region_fits(*result.3,
          align_up_l(st.slots@[ia@].size@) + align_up_l(st.slots@[ib@].size@)))]
// ... so an insertion as large as both together succeeds
#[ensures(align_up_l(st.slots@[ia@].size@) + align_up_l(st.slots@[ib@].size@) <= 4294967295
          ==> match result.2 { Ok(_) => true, Err(_) => false })]
// the maintained shape of the free list, at every later point
#[ensures(no_adj((^st).allocator.regions@))]
#[ensures(disj_all((^st).allocator.regions@))]
#[ensures(forall<i: Int> 0 <= i && i < (^st).allocator.regions@.len() ==> (^st).allocator.regions@[i].size@ > 0)]
pub fn verify_mt_inv_free_list_coalescing(
    st: &mut StateModel, ia: usize, ib: usize, lower_first: bool, ops: &[(u8, Key, u32)], psz: usize,
) -> (Result<(), MtError>, Result<(), MtError>, Result<usize, MtError>, Snapshot<Seq<Region>>) {
    let a = st.slots[ia];
    let b = st.slots[ib];
    let s0 = snapshot! { st.slots@ };
    proof_assert!(a.size@ > 0 && b.size@ > 0);
    snapshot! { lem_al(a.size@) };
    snapshot! { lem_al(b.size@) };
    proof_assert!(ia != ib && a.key != b.key);
    let (r1, r2);
    if lower_first {
        r1 = mt_remove(st, a.key);
        let g1 = snapshot! { st.allocator.regions@ };
        proof_assert!(forall<i: Int> 0 <= i && i < s0.len() && s0[i].key == a.key ==> s0[i] == a);
        proof_assert!(covered(*g1, a.offset@, sl_end(a)));
        proof_assert!(slot_at(st.slots@, b.key));
        proof_assert!(forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == b.key ==> st.slots@[i] == b);
        proof_assert!(clear_of(*g1, b.offset@, sl_end(b)));
        proof_assert!(exists<k: Int> 0 <= k && k < g1.len() && g1[k].offset@ <= a.offset@
                      && g1[k].offset@ + g1[k].size@ == b.offset@);
        r2 = mt_remove(st, b.key);
    } else {
        r1 = mt_remove(st, b.key);
        let g1 = snapshot! { st.allocator.regions@ };
        proof_assert!(forall<i: Int> 0 <= i && i < s0.len() && s0[i].key == b.key ==> s0[i] == b);
        proof_assert!(covered(*g1, b.offset@, sl_end(b)));
        proof_assert!(slot_at(st.slots@, a.key));
        proof_assert!(forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == a.key ==> st.slots@[i] == a);
        proof_assert!(clear_of(*g1, a.offset@, sl_end(a)));
        proof_assert!(exists<k: Int> 0 <= k && k < g1.len() && g1[k].offset@ == sl_end(a)
                      && g1[k].offset@ + g1[k].size@ >= sl_end(b));
        r2 = mt_remove(st, a.key);
    }
    let fr = snapshot! { st.allocator.regions@ };
    proof_assert!(covered(*fr, a.offset@, sl_end(b)));
    proof_assert!(sl_end(b) - a.offset@ == align_up_l(a.size@) + align_up_l(b.size@));
    proof_assert!(some_region_fits(*fr, align_up_l(a.size@) + align_up_l(b.size@)));
    proof_assert!(!slot_at(st.slots@, a.key));
    let total = align_up(a.size as usize) + align_up(b.size as usize);
    let mut ins: Result<usize, MtError> = Err(MtError::PoolFull);
    if total <= u32::MAX as usize {
        let t32 = total as u32;
        proof_assert!(t32@ == total@ && t32@ % 4096 == 0 && t32@ > 0);
        snapshot! { lem_al(t32@) };
        proof_assert!(align_up_l(t32@) == t32@);
        ins = mt_insert(st, a.key, t32);
    }
    mt_run(st, ops, psz);
    (r1, r2, ins, fr)
}

/// Anti-vacuity twin: MUST FAIL (deletes only the LOWER entry and claims one run
/// already covers both -- the upper entry is still allocated).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(ia@ < st.slots@.len() && ib@ < st.slots@.len())]
#[requires(sl_end(st.slots@[ia@]) == st.slots@[ib@].offset@)]
#[ensures(match result { Ok(_) => true, Err(_) => false })]
#[ensures(covered((^st).allocator.regions@, st.slots@[ia@].offset@, sl_end(st.slots@[ib@])))]
pub fn verify_mt_inv_free_list_coalescing__mutant(st: &mut StateModel, ia: usize, ib: usize) -> Result<(), MtError> {
    let a = st.slots[ia];
    mt_remove(st, a.key)
}

// ===========================================================================
// 13. Batch 3 — phase-split evict_next / remove / insert for interleavings,
//     free_capacity, and the batch-3 property drivers.
// ===========================================================================

/// `MemoryTierComponent::free_capacity` (lib.rs:183-191, an INHERENT helper -- not on
/// `IMemoryTier`): `capacity() - used()` under the pool read lock, `0` when not set up.
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result@ == 0)]
#[ensures(st.initialized ==> result@ == st.allocator.capacity@ - st.allocator.used@)]
pub fn mt_free_capacity(st: &StateModel) -> usize {
    if !st.initialized {
        return 0;
    }
    proof_assert!(forall<j: Int> 0 <= j && j < st.allocator.regions@.len() ==> st.allocator.regions@[j].size@ > 0);
    snapshot! { lem_fs_bound(st.allocator.regions@) };
    proof_assert!(st.allocator.used@ <= st.allocator.capacity@);
    st.allocator.capacity - st.allocator.used
}

/// evict_next PHASE 1 (lib.rs:439-445): `state.read()`, the flag, then
/// `ep.identify_next_to_evict(state.pool_id)` -- taken WITHOUT the pool lock, so any
/// whole pool critical section of another thread may run before phase 2. Touches only
/// the policy object. Policy modelled per its interface (ieviction_policy.rs:101-104):
/// it returns a key it TRACKS (the LRU head) and stops tracking it.
#[requires(policy_keys_unique(st.policy.tracked@))]
#[ensures(match result {
    Some(k) => st.initialized && st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0
        && !tracks((^st).policy.tracked@, k),
    None => (^st).policy.tracked@ == st.policy.tracked@,
})]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result != None)]
#[ensures(policy_keys_unique((^st).policy.tracked@))]
#[ensures(forall<k: Key, k2: Key> result == Some(k) && k2 != k
          ==> tracks((^st).policy.tracked@, k2) == tracks(st.policy.tracked@, k2))]
#[ensures((^st).slots@ == st.slots@ && (^st).allocator.regions@ == st.allocator.regions@
          && (^st).allocator.used == st.allocator.used && (^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).initialized == st.initialized && (^st).pool_base == st.pool_base
          && (^st).pool_size == st.pool_size && (^st).pool_id == st.pool_id)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
pub fn ev_identify(st: &mut StateModel) -> Option<Key> {
    if !st.initialized {
        return None;
    }
    let pid = st.pool_id;
    policy_identify_next_to_evict(&mut st.policy, pid)
}

/// evict_next PHASE 2 (lib.rs:446-466): `pool.write()`; `if let Some(slot) =
/// slots.remove(&key) { deallocate }`; `telemetry.evictions.fetch_add(1)` -- NOT inside
/// the `if let` -- and `Some(key)`, for ANY key phase 1 produced (stale or not).
#[requires(pool_inv(st.allocator, st.slots@))]
#[ensures(pool_inv((^st).allocator, (^st).slots@))]
#[ensures(result == Some(key))]
#[ensures((^tel).evictions@ == fetch_l(tel.evictions@))]
// batch 4: the other two counters are untouched by an eviction
#[ensures((^tel).write_lock_contentions == tel.write_lock_contentions
          && (^tel).read_lock_contentions == tel.read_lock_contentions)]
#[ensures(!slot_at(st.slots@, key) ==> (^st).slots@ == st.slots@
          && (^st).allocator.regions@ == st.allocator.regions@ && (^st).allocator.used == st.allocator.used)]
#[ensures(!slot_at((^st).slots@, key))]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
#[ensures((^st).policy.tracked@ == st.policy.tracked@ && (^st).policy.next_handle == st.policy.next_handle)]
#[ensures((^st).initialized == st.initialized && (^st).pool_base == st.pool_base
          && (^st).pool_size == st.pool_size && (^st).pool_id == st.pool_id)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
pub fn ev_commit(st: &mut StateModel, key: Key, tel: &mut TelemetryModel) -> Option<Key> {
    cs_evict(&mut st.allocator, &mut st.slots, key);
    tel_event(tel, 0); // lib.rs:463-464 `state.telemetry.evictions.fetch_add(1, Relaxed)`
    Some(key)
}

/// remove (lib.rs:476-507) when the policy may DISAGREE with the pool: the pool
/// critical section, `ep.remove(slot.eviction_handle)` with its result DISCARDED
/// (`let _ =`, lib.rs:504). Policy per its interface (ieviction_policy.rs:98-99, :65):
/// untracking a handle that is no longer tracked fails with `InvalidHandle` and changes
/// nothing -- mirrored by key as "remove k if tracked, else no-op".
#[requires(pool_inv(st.allocator, st.slots@))]
#[requires(policy_keys_unique(st.policy.tracked@))]
#[ensures(pool_inv((^st).allocator, (^st).slots@))]
#[ensures(policy_keys_unique((^st).policy.tracked@))]
#[ensures(slot_at(st.slots@, key) ==> result == Ok(()))]
#[ensures(!slot_at((^st).slots@, key))]
#[ensures(slot_at(st.slots@, key) ==> !tracks((^st).policy.tracked@, key))]
#[ensures(forall<k2: Key> k2 != key ==> tracks((^st).policy.tracked@, k2) == tracks(st.policy.tracked@, k2))]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
#[ensures(forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == key
          ==> some_region_fits((^st).allocator.regions@, align_up_l(st.slots@[i].size@)))]
#[ensures((^st).initialized == st.initialized && (^st).pool_base == st.pool_base
          && (^st).pool_size == st.pool_size && (^st).pool_id == st.pool_id)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
#[ensures((^st).policy.next_handle == st.policy.next_handle)]
pub fn rm_concurrent(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    let r = cs_remove(&mut st.allocator, &mut st.slots, key);
    match r {
        Ok(()) => {
            let pid = st.pool_id;
            policy_remove_key(&mut st.policy, pid, key); // `let _ = ep.remove(handle)`
        }
        Err(_) => {}
    }
    r
}

/// insert (lib.rs:332-383) when the policy may disagree with the pool: the pool
/// critical section; `ep.track` only after `allocate` succeeded, under the pool lock.
#[requires(pool_inv(st.allocator, st.slots@))]
#[requires(policy_keys_unique(st.policy.tracked@))]
#[requires(!tracks(st.policy.tracked@, key))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(pool_inv((^st).allocator, (^st).slots@))]
#[ensures(policy_keys_unique((^st).policy.tracked@))]
#[ensures(size@ > 0 && !slot_at(st.slots@, key) && some_region_fits(st.allocator.regions@, align_up_l(size@))
          ==> match result { Ok(_) => true, Err(_) => false })]
#[ensures(match result { Ok(_) => slot_at((^st).slots@, key) && tracks((^st).policy.tracked@, key), Err(_) => true })]
#[ensures(forall<k2: Key> k2 != key ==> tracks((^st).policy.tracked@, k2) == tracks(st.policy.tracked@, k2))]
#[ensures(forall<k2: Key> k2 != key ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
#[ensures((^st).initialized == st.initialized && (^st).pool_base == st.pool_base
          && (^st).pool_size == st.pool_size && (^st).pool_id == st.pool_id)]
#[ensures((^st).allocator.capacity == st.allocator.capacity)]
pub fn ins_concurrent(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    let h = st.policy.next_handle;
    let r = cs_insert(&mut st.allocator, &mut st.slots, key, size, h);
    match r {
        Ok(_) => {
            let pid = st.pool_id;
            let _ = policy_track(&mut st.policy, pid, key);
        }
        Err(_) => {}
    }
    r
}

/// oldest_keys (lib.rs:428-436): it never takes the pool lock at all -- one policy call.
#[ensures((^st).policy.tracked == st.policy.tracked)]
#[ensures((^st).slots@ == st.slots@)]
#[ensures(st.initialized && n@ > 0 ==> result@.len() == if n@ < st.policy.tracked@.len() { n@ } else { st.policy.tracked@.len() })]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i] == (st.policy.tracked@[i]).0)]
pub fn ok_concurrent(st: &mut StateModel, n: usize) -> Vec<Key> {
    if !st.initialized || n == 0 {
        return Vec::new();
    }
    let pid = st.pool_id;
    policy_candidates(&mut st.policy, pid, n)
}

/// `mt_inv` contains every conjunct of `pool_inv`.
#[logic]
#[requires(mt_inv(st))]
#[ensures(pool_inv(st.allocator, st.slots@))]
pub fn lem_mt_pool(st: StateModel) {}

// ---------------------------------------------------------------------------

/// **MT-CLEAR-POST-UNTRACKED** (postcondition, 4 attachments)
///
/// "After the cache is wiped the external eviction-policy component no longer tracks
/// any of the removed keys, so the oldest-keys query returns an empty list and an
/// eviction attempt reports that there is nothing to evict."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => false })]
#[ensures(forall<k: Key> slot_at(st.slots@, k) ==> !tracks((^st).policy.tracked@, k))]
#[ensures(forall<k: Key> !tracks((^st).policy.tracked@, k))]
#[ensures(result.1@.len() == 0)]
#[ensures(result.2 == None)]
#[ensures(result.3 == None)]
pub fn verify_mt_clear_post_untracked(st: &mut StateModel, n: usize, k: Key)
    -> (Result<usize, MtError>, Vec<Key>, Option<Key>, Option<Key>) {
    let c = mt_clear(st);
    let ok = mt_oldest_keys(st, n);
    let e1 = mt_evict_next(st);
    let e2 = mt_evict_next_for_key(st, k);
    (c, ok, e1, e2)
}

/// Anti-vacuity twin: MUST FAIL (no wipe: a set-up, non-empty cache has something to evict).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(result.1@.len() <= n@)]
#[ensures(result.0 == None)]
pub fn verify_mt_clear_post_untracked__mutant(st: &mut StateModel, n: usize) -> (Option<Key>, Vec<Key>) {
    let ok = mt_oldest_keys(st, n);
    let e1 = mt_evict_next(st);
    (e1, ok)
}

/// **MT-EVICT-NEXT-REPORTS-SUCCESS-WITHOUT-FREEING** (error-case, code-only) -- HAZARD
///
/// "If the eviction policy names a key for which the pool holds no entry, eviction
/// silently does nothing to the pool yet still reports that key as evicted and still
/// counts one more eviction in its telemetry ..."
///
/// CONFIRMED with a policy that OBEYS its contract: the policy nominates a key it
/// really tracks and that really has storage (`k` = LRU head, `slot_at(k)`); but
/// `identify_next_to_evict` runs before the pool lock (lib.rs:445 vs :447-458), so
/// thread B's whole `remove(k)` (lib.rs:476-507; its `ep.remove` fails harmlessly with
/// `InvalidHandle`, discarded) fits in the window. When A then takes the pool lock the
/// pool holds no entry for `k`: A's critical section changes NOTHING in the pool,
/// yet A returns `Some(k)` and bumps `evictions`.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.slots@.len() > 0)]
// the nomination is contract-conforming: a tracked key, the LRU head, with storage
#[ensures(match result.0 { Some(k) => st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0
          && slot_at(st.slots@, k), None => false })]
// thread B's deletion succeeded in the window
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
// when A takes the pool lock the pool holds no entry for the nominated key ...
#[ensures(forall<k: Key> result.0 == Some(k) ==> !slot_at(*result.2, k))]
// ... A's commit changes nothing in the pool: same entries, same free runs, same bytes in use ...
#[ensures((^st).slots@ == *result.2)]
#[ensures((^st).allocator.regions@ == *result.3)]
#[ensures((^st).allocator.used@ == result.4@)]
// ... yet A still reports the key evicted (result.0 above) and counts one more eviction
#[ensures((^tel).evictions@ == fetch_l(tel.evictions@))]
pub fn verify_mt_evict_next_reports_success_without_freeing(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Result<(), MtError>, Snapshot<Seq<SlotModel>>, Snapshot<Seq<Region>>, usize) {
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    proof_assert!(st.policy.tracked@.len() > 0);
    snapshot! { lem_mt_pool(*st) };
    let s0 = snapshot! { st.slots@ };
    let t0 = snapshot! { st.policy.tracked@ };
    // thread A, phase 1: identify_next_to_evict (no pool lock)
    let v = ev_identify(st);
    let k = match v {
        Some(k) => k,
        None => {
            return (None, Err(MtError::KeyNotFound), snapshot! { st.slots@ },
                    snapshot! { st.allocator.regions@ }, st.allocator.used);
        }
    };
    proof_assert!(tracks(*t0, k));
    proof_assert!(slot_at(*s0, k));
    // thread B: remove(k), whole critical section
    let rm = rm_concurrent(st, k);
    let s_mid = snapshot! { st.slots@ };
    let r_mid = snapshot! { st.allocator.regions@ };
    let u_mid = st.allocator.used;
    // thread A, phase 2
    let ev = ev_commit(st, k, tel);
    (ev, rm, s_mid, r_mid, u_mid)
}

/// Anti-vacuity twin: MUST FAIL (claims A's eviction released bytes).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.slots@.len() > 0)]
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
#[ensures((^st).allocator.used@ < result.2@)]
pub fn verify_mt_evict_next_reports_success_without_freeing__mutant(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Result<(), MtError>, usize) {
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    snapshot! { lem_mt_pool(*st) };
    let s0 = snapshot! { st.slots@ };
    let t0 = snapshot! { st.policy.tracked@ };
    let v = ev_identify(st);
    let k = match v {
        Some(k) => k,
        None => return (None, Err(MtError::KeyNotFound), st.allocator.used),
    };
    proof_assert!(tracks(*t0, k));
    proof_assert!(slot_at(*s0, k));
    let rm = rm_concurrent(st, k);
    let u_mid = st.allocator.used;
    let ev = ev_commit(st, k, tel);
    (ev, rm, u_mid)
}

/// **MT-GET-FRAME** (frame, 4 attachments)
///
/// "The recency-updating lookup never changes the stored data: no entry is added or
/// removed, no block is moved or freed, the reported bytes in use and total capacity
/// are unchanged, and the contents of the returned block are not modified; its only
/// side effect is on the eviction order."
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).allocator.regions@ == st.allocator.regions@)]
#[ensures((^st).pool_base == st.pool_base && (^st).pool_size == st.pool_size)]
#[ensures(forall<x: Key> mt_contains_l(^st, x) == mt_contains_l(*st, x))]
#[ensures(result.0 == result.2 && result.1 == result.3)]
#[ensures(forall<x: Key> tracks((^st).policy.tracked@, x) == tracks(st.policy.tracked@, x))]
#[ensures((^st).policy.tracked@.len() == st.policy.tracked@.len())]
#[ensures((^st).policy.next_handle == st.policy.next_handle && (^st).policy.pools == st.policy.pools)]
pub fn verify_mt_get_frame(st: &mut StateModel, key: Key)
    -> (usize, usize, usize, usize, Option<(usize, u32)>) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let g = mt_get(st, key);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    (u0, c0, u1, c1, g)
}

/// Anti-vacuity twin: MUST FAIL (claims get leaves the eviction ORDER unchanged too).
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
pub fn verify_mt_get_frame__mutant(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// **MT-GET-POST-TOUCH** (postcondition, 4 attachments)
///
/// "A successful recency-updating lookup marks the key as the most recently used
/// entry, so it will be chosen for eviction only after entries that have not been used
/// since, and the refresh is applied only after the pool lock has been released."
#[requires(mt_inv(*st))]
#[ensures(result.0 != None ==> (^st).policy.tracked@.len() > 0
          && ((^st).policy.tracked@[(^st).policy.tracked@.len() - 1]).0 == key)]
#[ensures(result.0 != None ==> forall<i: Int> 0 <= i && i < (^st).policy.tracked@.len() - 1
          ==> ((^st).policy.tracked@[i]).0 != key)]
#[ensures(result.0 != None ==> (^st).policy.tracked@ == touch_l(st.policy.tracked@, key))]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> result.0 != None)]
// lock skeleton of a successful get: the refresh comes after the pool guard is dropped
#[ensures(refresh_after_release(result.1@) && no_pool_write(result.1@))]
#[ensures(result.1@.len() == 4 && result.1@[2] == LkEv::PoolRelease && result.1@[3] == LkEv::PolicyRefresh)]
pub fn verify_mt_get_post_touch(st: &mut StateModel, key: Key) -> (Option<(usize, u32)>, Vec<LkEv>) {
    let g = mt_get(st, key);
    let t = lk_get(true, true);
    proof_assert!(t@.len() == 4);
    (g, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the looked-up key becomes the OLDEST entry).
#[requires(mt_inv(*st))]
#[ensures(result != None ==> (^st).policy.tracked@.len() > 0)]
#[ensures(result != None ==> ((^st).policy.tracked@[0]).0 == key)]
pub fn verify_mt_get_post_touch__mutant(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// **MT-INV-POINTERS-INSIDE-POOL** (invariant, 4 attachments)
///
/// "Every memory address the component gives out for an entry, together with the
/// whole block of bytes that entry is said to occupy, lies inside the pool's own memory
/// region, because an entry's starting offset plus its rounded size never exceeds the
/// pool size."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
// every entry held: offset + rounded size <= pool size
#[ensures((^st).initialized ==> forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> sl_end((^st).slots@[i]) <= (^st).pool_size@)]
// every address handed out, with its whole rounded block, lies in [base, base + pool_size)
#[ensures(forall<p: usize> result.0 == Ok(p) ==> (^st).pool_base@ <= p@
          && p@ + align_up_l(sz@) <= (^st).pool_base@ + (^st).pool_size@)]
#[ensures(forall<p: usize, s: u32> result.1 == Some((p, s)) ==> (^st).pool_base@ <= p@
          && p@ + align_up_l(s@) <= (^st).pool_base@ + (^st).pool_size@)]
#[ensures(forall<p: usize, s: u32> result.2 == Some((p, s)) ==> (^st).pool_base@ <= p@
          && p@ + align_up_l(s@) <= (^st).pool_base@ + (^st).pool_size@)]
#[ensures(forall<b: usize, s: usize> result.3 == Some((b, s)) ==> b == (^st).pool_base && s == (^st).pool_size)]
pub fn verify_mt_inv_pointers_inside_pool(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize,
    k: Key, sz: u32, k2: Key, k3: Key)
    -> (Result<usize, MtError>, Option<(usize, u32)>, Option<(usize, u32)>, Option<(usize, usize)>) {
    mt_run(st, ops, psz);
    let s0 = snapshot! { st.slots@ };
    let ins = mt_insert(st, k, sz);
    proof_assert!(forall<p: usize> ins == Ok(p) ==> st.initialized && slot_ivl_ok(st.slots@, st.allocator.capacity@)
                  && st.allocator.capacity@ == st.pool_size@);
    proof_assert!(forall<p: usize> ins == Ok(p) ==> sl_end(st.slots@[s0.len()]) <= st.pool_size@);
    let s1 = snapshot! { st.slots@ };
    let g = mt_get(st, k2);
    proof_assert!(st.slots@ == *s1);
    let pk = mt_peek(st, k3);
    let pi = mt_pool_info(st);
    (ins, g, pk, pi)
}

/// Anti-vacuity twin: MUST FAIL (claims every entry ends strictly before the pool end).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures((^st).initialized ==> forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> sl_end((^st).slots@[i]) <= (^st).pool_size@)]
#[ensures((^st).initialized ==> forall<i: Int> 0 <= i && i < (^st).slots@.len()
          ==> sl_end((^st).slots@[i]) < (^st).pool_size@)]
pub fn verify_mt_inv_pointers_inside_pool__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
}

/// **MT-INV-ENTRY-SIZE-POSITIVE** (invariant, code-only, 4 attachments)
///
/// "Every entry the cache holds has a size of at least one byte, because the insertion
/// path rejects a size of zero and no other operation ever creates an entry."
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> (^st).slots@[i].size@ >= 1)]
#[ensures(match result.0 { Err(MtError::InvalidSize) => true, _ => false })]
#[ensures(forall<p: usize, s: u32> result.1 == Some((p, s)) ==> s@ >= 1)]
#[ensures(forall<p: usize, s: u32> result.2 == Some((p, s)) ==> s@ >= 1)]
pub fn verify_mt_inv_entry_size_positive(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, k: Key, k2: Key)
    -> (Result<usize, MtError>, Option<(usize, u32)>, Option<(usize, u32)>) {
    mt_run(st, ops, psz);
    let z = mt_insert(st, k, 0);
    let g = mt_get(st, k2);
    let p = mt_peek(st, k2);
    (z, g, p)
}

/// Anti-vacuity twin: MUST FAIL (claims every entry is at least TWO bytes).
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> (^st).slots@[i].size@ >= 1)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).slots@.len() ==> (^st).slots@[i].size@ >= 2)]
pub fn verify_mt_inv_entry_size_positive__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize) {
    mt_run(st, ops, psz);
}

/// **MT-INSERT-POST-LOOKUP** (postcondition, 4 attachments)
///
/// "After adding a cache entry succeeds the key is reported as present, and both the
/// recency-updating lookup and the non-recency-updating lookup return the very same
/// address and the very same size that the insertion returned."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.1)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.2 == Some((p, size)))]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.3 == Some((p, size)))]
pub fn verify_mt_insert_post_lookup(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, bool, Option<(usize, u32)>, Option<(usize, u32)>) {
    let s0 = snapshot! { st.slots@ };
    let r = mt_insert(st, key, size);
    proof_assert!(forall<p: usize> r == Ok(p) ==> forall<i: Int> 0 <= i && i < st.slots@.len()
                  && st.slots@[i].key == key ==> i == s0.len());
    let c = mt_contains(st, key);
    let pk = mt_peek(st, key);
    let g = mt_get(st, key);
    (r, c, pk, g)
}

/// Anti-vacuity twin: MUST FAIL (claims the lookups report the ROUNDED size).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.1 != None)]
#[ensures(forall<p: usize, q: usize, s: u32> result.0 == Ok(p) && result.1 == Some((q, s)) ==> s@ == align_up_l(size@))]
pub fn verify_mt_insert_post_lookup__mutant(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, Option<(usize, u32)>) {
    let r = mt_insert(st, key, size);
    let pk = mt_peek(st, key);
    (r, pk)
}

/// **MT-OLDEST-KEYS-FRAME** (frame, 4 attachments)
///
/// "Looking at the oldest keys is purely observational: it removes no entry, frees or
/// reserves no byte of the pool, leaves the bytes in use and total capacity unchanged,
/// and does not alter how soon any of the reported keys will be evicted."
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).allocator.regions@ == st.allocator.regions@)]
#[ensures(result.1 == result.3 && result.2 == result.4)]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
#[ensures(forall<i: Int> 0 <= i && i < result.0@.len() ==> ((^st).policy.tracked@[i]).0 == result.0@[i])]
pub fn verify_mt_oldest_keys_frame(st: &mut StateModel, n: usize) -> (Vec<Key>, usize, usize, usize, usize) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let ks = mt_oldest_keys(st, n);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    (ks, u0, c0, u1, c1)
}

/// Anti-vacuity twin: MUST FAIL (claims the reported keys are taken OUT of the order).
#[requires(mt_inv(*st))]
#[ensures((^st).slots@ == st.slots@)]
#[ensures(result@.len() > 0 ==> (^st).policy.tracked@.len() < st.policy.tracked@.len())]
pub fn verify_mt_oldest_keys_frame__mutant(st: &mut StateModel, n: usize) -> Vec<Key> {
    mt_oldest_keys(st, n)
}

/// **MT-PEEK-FRAME-EVICTION-ORDER** (frame, 4 attachments)
///
/// "The non-recency-updating lookup leaves the eviction order completely unchanged,
/// never calling the eviction policy at all, so an entry that was next in line to be
/// evicted is still next in line after being read this way ..."
///
/// `peek` (lib.rs:416-426) reads under the pool READ lock and makes no call on `ep` --
/// mirrored as `mt_peek(&StateModel)` (the policy object is not even reachable for
/// writing), and its lock skeleton has no policy event. The entry next in line before
/// the peek is the one the following eviction takes.
#[requires(mt_inv(*st))]
#[ensures(forall<i: Int> 0 <= i && i < result.2@.len()
          ==> result.2@[i] != LkEv::PolicyRefresh && result.2@[i] != LkEv::PolicyOther)]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result.1 == Some((st.policy.tracked@[0]).0))]
pub fn verify_mt_peek_frame_eviction_order(st: &mut StateModel, k: Key)
    -> (Option<(usize, u32)>, Option<Key>, Vec<LkEv>) {
    let p = mt_peek(st, k);
    let t = lk_peek_or_contains(st.initialized);
    proof_assert!(st.policy.tracked@.len() > 0 ==> tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.policy.tracked@.len() > 0 ==> st.slots@.len() > 0);
    let e = mt_evict_next(st);
    (p, e, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the eviction after the peek takes the NEWEST entry).
#[requires(mt_inv(*st))]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result.1 != None)]
#[ensures(st.initialized && st.policy.tracked@.len() > 0
          ==> result.1 == Some((st.policy.tracked@[st.policy.tracked@.len() - 1]).0))]
pub fn verify_mt_peek_frame_eviction_order__mutant(st: &mut StateModel, k: Key) -> (Option<(usize, u32)>, Option<Key>) {
    let p = mt_peek(st, k);
    proof_assert!(st.policy.tracked@.len() > 0 ==> tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.policy.tracked@.len() > 0 ==> st.slots@.len() > 0);
    let e = mt_evict_next(st);
    (p, e)
}

/// **MT-PEEK-FRAME-POOL-STATE** (frame, spec-only, 4 attachments)
///
/// "The non-recency-updating lookup changes no stored data: the set of keys present,
/// every entry's address and size, the bytes in use and the total capacity are all the
/// same afterwards."
#[requires(mt_inv(*st))]
#[ensures(result.0 == result.3 && result.1 == result.4 && result.2 == result.5)]
#[ensures(result.6 == result.7)]
pub fn verify_mt_peek_frame_pool_state(st: &StateModel, k: Key, k2: Key)
    -> (usize, usize, Option<(usize, u32)>, usize, usize, Option<(usize, u32)>, bool, bool) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let q0 = mt_peek(st, k2);
    let b0 = mt_contains(st, k2);
    let _p = mt_peek(st, k);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    let q1 = mt_peek(st, k2);
    let b1 = mt_contains(st, k2);
    (u0, c0, q0, u1, c1, q1, b0, b1)
}

/// Anti-vacuity twin: MUST FAIL (the same observations around an INSERT instead of a peek).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(result.0 == result.0)]
#[ensures(result.0 == result.1)]
pub fn verify_mt_peek_frame_pool_state__mutant(st: &mut StateModel, k: Key, sz: u32) -> (usize, usize) {
    let u0 = mt_used(st);
    let _p = mt_insert(st, k, sz);
    let u1 = mt_used(st);
    (u0, u1)
}

/// **MT-REMOVE-INVALIDATES-POINTER-SILENTLY** (postcondition, code-only) -- HAZARD
///
/// "Any address previously handed out for a deleted key becomes stale the instant the
/// deletion returns, and nothing in the component marks it as stale: the freed byte
/// range goes straight back into the free space and the very next insertion can hand
/// the same bytes to a different caller."
///
/// CONFIRMED on a witness run: set up a pool, insert `k1`, delete `k1`, insert `k2 !=
/// k1` of the same size -- the second caller receives the IDENTICAL address. Between
/// the deletion and the second insertion the old block is part of a free run, and the
/// only thing that changed for `k1` is that a key lookup now misses.
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ + 1 < u64::MAX@ && st.policy.next_handle@ + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(k1 != k2)]
#[requires(sz@ > 0 && align_up_l(sz@) <= psz@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => false })]
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
#[ensures(result.2 == None)]
// the deleted block went straight back into one free run
#[ensures(forall<p: usize> result.0 == Ok(p) ==> covered(*result.4, p@ - b@, p@ - b@ + align_up_l(sz@)))]
// the very next insertion, for a DIFFERENT key, hands out the very same address
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.3 == Ok(p))]
pub fn verify_mt_remove_invalidates_pointer_silently(st: &mut StateModel, psz: usize, b: usize, k1: Key, k2: Key, sz: u32)
    -> (Result<usize, MtError>, Result<(), MtError>, Option<(usize, u32)>, Result<usize, MtError>, Snapshot<Seq<Region>>) {
    let mut lg = LogSink { lines: 0 };
    let _ = init_logged(st, psz, Some(b), &mut lg, false);
    proof_assert!(st.allocator.regions@.len() == 1 && st.allocator.regions@[0].offset@ == 0
                  && st.allocator.regions@[0].size@ == psz@ && st.pool_base == b);
    snapshot! { lem_al(sz@) };
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(sz@)));
    let p1 = mt_insert(st, k1, sz);
    proof_assert!(forall<p: usize> p1 == Ok(p) ==> p@ == b@);
    proof_assert!(forall<p: usize> p1 == Ok(p) ==> st.slots@.len() == 1 && st.slots@[0].key == k1
                  && st.slots@[0].offset@ == 0 && st.slots@[0].size == sz);
    let r1 = snapshot! { st.allocator.regions@ };
    proof_assert!(forall<j: Int> 0 <= j && j < r1.len() ==> r1[j].offset@ >= align_up_l(sz@));
    let rm = mt_remove(st, k1);
    let fr = snapshot! { st.allocator.regions@ };
    proof_assert!(covered(*fr, 0, align_up_l(sz@)));
    proof_assert!(fr.len() > 0 && fr[0].offset@ == 0 && fr[0].size@ >= align_up_l(sz@));
    let pk = mt_peek(st, k1);
    proof_assert!(!slot_at(st.slots@, k2));
    let p2 = mt_insert(st, k2, sz);
    proof_assert!(forall<p: usize> p2 == Ok(p) ==> p@ == b@);
    (p1, rm, pk, p2, fr)
}

/// Anti-vacuity twin: MUST FAIL (claims the second caller gets a DIFFERENT address).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ + 1 < u64::MAX@ && st.policy.next_handle@ + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(b@ % 4096 == 0 && b@ > 0 && b@ + psz@ <= usize::MAX@)]
#[requires(k1 != k2)]
#[requires(sz@ > 0 && align_up_l(sz@) <= psz@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => false })]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> result.1 != Ok(p))]
pub fn verify_mt_remove_invalidates_pointer_silently__mutant(st: &mut StateModel, psz: usize, b: usize, k1: Key, k2: Key, sz: u32)
    -> (Result<usize, MtError>, Result<usize, MtError>) {
    let mut lg = LogSink { lines: 0 };
    let _ = init_logged(st, psz, Some(b), &mut lg, false);
    snapshot! { lem_al(sz@) };
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(sz@)));
    let p1 = mt_insert(st, k1, sz);
    let _ = mt_remove(st, k1);
    let p2 = mt_insert(st, k2, sz);
    (p1, p2)
}

/// **MT-REMOVE-POST-UNTRACKED** (postcondition, 4 attachments)
///
/// "A deleted key is also dropped from the external eviction-policy component's
/// bookkeeping, so it can no longer be reported among the oldest keys nor nominated
/// as a victim."
#[requires(mt_inv(*st))]
#[ensures(st.initialized ==> !tracks((^st).policy.tracked@, key))]
#[ensures(forall<i: Int> 0 <= i && i < result.1@.len() ==> result.1@[i] != key)]
#[ensures(result.2 != Some(key))]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> match result.0 { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_remove_post_untracked(st: &mut StateModel, key: Key, n: usize)
    -> (Result<(), MtError>, Vec<Key>, Option<Key>) {
    let r = mt_remove(st, key);
    proof_assert!(!st.initialized ==> !tracks(st.policy.tracked@, key));
    let ks = mt_oldest_keys(st, n);
    let e = mt_evict_next(st);
    (r, ks, e)
}

/// Anti-vacuity twin: MUST FAIL (the same claims WITHOUT the deletion).
#[requires(mt_inv(*st))]
#[ensures(result.1 != Some(key))]
#[ensures(forall<i: Int> 0 <= i && i < result.0@.len() ==> result.0@[i] != key)]
pub fn verify_mt_remove_post_untracked__mutant(st: &mut StateModel, key: Key, n: usize) -> (Vec<Key>, Option<Key>) {
    let ks = mt_oldest_keys(st, n);
    let e = mt_evict_next(st);
    (ks, e)
}

/// **MT-TOUCH-POST-ORDER-REFRESH** (postcondition, 4 attachments)
///
/// "Refreshing a key that is present makes it the most recently used entry, so it will
/// be evicted only after entries that have not been used since, no data is returned to
/// the caller, and the refresh is applied only after the pool lock has been released."
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
#[ensures((^st).policy.tracked@.len() > 0 && ((^st).policy.tracked@[(^st).policy.tracked@.len() - 1]).0 == key)]
#[ensures(forall<i: Int> 0 <= i && i < (^st).policy.tracked@.len() - 1 ==> ((^st).policy.tracked@[i]).0 != key)]
#[ensures((^st).policy.tracked@ == touch_l(st.policy.tracked@, key))]
#[ensures(refresh_after_release(result@) && no_pool_write(result@))]
#[ensures(result@.len() == 4 && result@[2] == LkEv::PoolRelease && result@[3] == LkEv::PolicyRefresh)]
pub fn verify_mt_touch_post_order_refresh(st: &mut StateModel, key: Key) -> Vec<LkEv> {
    let () = mt_touch(st, key); // returns `()` -- no data to the caller
    let t = lk_touch(true, true);
    proof_assert!(t@.len() == 4);
    t
}

/// Anti-vacuity twin: MUST FAIL (claims the refreshed key becomes the OLDEST entry).
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
#[ensures((^st).policy.tracked@.len() > 0)]
#[ensures(((^st).policy.tracked@[0]).0 == key)]
pub fn verify_mt_touch_post_order_refresh__mutant(st: &mut StateModel, key: Key) {
    mt_touch(st, key);
}

/// **MT-FREELIST-ALLOCATE-ERR-NO-FIT** (error-case, code-only, 3 attachments)
///
/// "When no single free run is long enough for the rounded request the allocation
/// fails and, because the search happens before any modification, the free runs and
/// the bytes-used total are left exactly as they were."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[requires(size@ > 0 && no_region_fits(fl.regions@, align_up_l(size@)))]
#[ensures(result == None)]
#[ensures((^fl).regions@ == fl.regions@)]
#[ensures((^fl).used == fl.used && (^fl).capacity == fl.capacity)]
pub fn verify_mt_freelist_allocate_err_no_fit(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// Anti-vacuity twin: MUST FAIL (claims failure whenever the TOTAL free space is short
/// of nothing -- i.e. whenever some run fits, it still fails).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[requires(size@ > 0 && some_region_fits(fl.regions@, align_up_l(size@)))]
#[ensures((^fl).capacity == fl.capacity)]
#[ensures(result == None)]
pub fn verify_mt_freelist_allocate_err_no_fit__mutant(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// **MT-FREELIST-ALLOCATE-ERR-ZERO** (error-case, code-only, 3 attachments)
///
/// "A request for zero bytes fails without reserving anything and without changing
/// the free runs or the bytes-used total."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(result == None)]
#[ensures((^fl).regions@ == fl.regions@)]
#[ensures((^fl).used == fl.used && (^fl).capacity == fl.capacity)]
pub fn verify_mt_freelist_allocate_err_zero(fl: &mut FreeListModel) -> Option<usize> {
    fl_allocate(fl, 0)
}

/// Anti-vacuity twin: MUST FAIL (claims a zero request reserves one 4 KiB unit).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(result == None)]
#[ensures((^fl).used@ == fl.used@ + 4096)]
pub fn verify_mt_freelist_allocate_err_zero__mutant(fl: &mut FreeListModel) -> Option<usize> {
    fl_allocate(fl, 0)
}

/// **MT-FREELIST-ALLOCATE-FIRST-FIT** (postcondition, code-only, 3 attachments)
///
/// "The allocator scans its free runs in increasing order of position and takes the
/// first one that is large enough, so it always returns the lowest starting offset that
/// can satisfy the request."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(forall<o: usize> result == Some(o) ==> exists<i: Int> 0 <= i && i < fl.regions@.len()
          && fl.regions@[i].offset == o && fl.regions@[i].size@ >= align_up_l(size@)
          && (forall<j: Int> 0 <= j && j < i ==> fl.regions@[j].size@ < align_up_l(size@)))]
#[ensures(forall<o: usize, j: Int> result == Some(o) && 0 <= j && j < fl.regions@.len()
          && fl.regions@[j].size@ >= align_up_l(size@) ==> o@ <= fl.regions@[j].offset@)]
pub fn verify_mt_freelist_allocate_first_fit(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    let r0 = snapshot! { fl.regions@ };
    let r = fl_allocate(fl, size);
    proof_assert!(sorted_by_offset(*r0));
    r
}

/// Anti-vacuity twin: MUST FAIL (claims the returned offset is strictly below EVERY fitting run).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(forall<o: usize, j: Int> result == Some(o) && 0 <= j && j < fl.regions@.len()
          && fl.regions@[j].size@ >= align_up_l(size@) ==> o@ <= fl.regions@[j].offset@)]
#[ensures(forall<o: usize, j: Int> result == Some(o) && 0 <= j && j < fl.regions@.len()
          && fl.regions@[j].size@ >= align_up_l(size@) ==> o@ < fl.regions@[j].offset@)]
pub fn verify_mt_freelist_allocate_first_fit__mutant(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    let r0 = snapshot! { fl.regions@ };
    let r = fl_allocate(fl, size);
    proof_assert!(sorted_by_offset(*r0));
    r
}

/// **MT-FREELIST-ALLOCATE-POST** (postcondition, code-only, 3 attachments)
///
/// "A successful allocation returns the starting offset of a run of free bytes at least
/// as long as the requested size rounded up to the next four-kibibyte boundary, marks
/// exactly that rounded amount as used, and leaves whatever is left over of the chosen
/// run still free."
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(forall<o: usize> result == Some(o) ==> covered(fl.regions@, o@, o@ + align_up_l(size@)))]
#[ensures(forall<o: usize> result == Some(o) ==> (^fl).used@ == fl.used@ + align_up_l(size@))]
#[ensures(forall<o: usize> result == Some(o) ==> exists<i: Int> 0 <= i && i < fl.regions@.len()
          && fl.regions@[i].offset == o && fl.regions@[i].size@ >= align_up_l(size@)
          && (fl.regions@[i].size@ > align_up_l(size@) ==> exists<k: Int> 0 <= k && k < (^fl).regions@.len()
              && (^fl).regions@[k].offset@ == o@ + align_up_l(size@)
              && (^fl).regions@[k].size@ == fl.regions@[i].size@ - align_up_l(size@)))]
#[ensures(forall<o: usize> result == Some(o) ==> clear_of((^fl).regions@, o@, o@ + align_up_l(size@)))]
pub fn verify_mt_freelist_allocate_post(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    let r0 = snapshot! { fl.regions@ };
    let r = fl_allocate(fl, size);
    proof_assert!(forall<o: usize, i: Int> r == Some(o) && 0 <= i && i < r0.len() && r0[i].offset == o
                  && alloc_step(*r0, fl.regions@, i, align_up_l(size@)) && r0[i].size@ > align_up_l(size@)
                  ==> fl.regions@[i].offset@ == o@ + align_up_l(size@)
                      && fl.regions@[i].size@ == r0[i].size@ - align_up_l(size@));
    r
}

/// Anti-vacuity twin: MUST FAIL (claims only the UNROUNDED request is marked used).
#[requires(fl_inv(fl.regions@, fl.capacity@))]
#[requires(fl.used@ + free_sum(fl.regions@) <= fl.capacity@)]
#[ensures(forall<o: usize> result == Some(o) ==> covered(fl.regions@, o@, o@ + align_up_l(size@)))]
#[ensures(forall<o: usize> result == Some(o) ==> (^fl).used@ == fl.used@ + size@)]
pub fn verify_mt_freelist_allocate_post__mutant(fl: &mut FreeListModel, size: u32) -> Option<usize> {
    fl_allocate(fl, size)
}

/// **MT-FREELIST-ALLOCATE-ROUNDING-OVERFLOW** -- REFUTATION (error-case, code-only) -- HAZARD
///
/// Statement: "The allocator rounds the requested size up ... using unchecked
/// arithmetic, so a request within a few thousand bytes of the largest representable
/// size aborts the process instead of failing with the pool-full error."
///
/// Negation proved for the shipped component: the ONLY caller of `FreeList::allocate`
/// is `insert(key, size: u32)` (lib.rs:364-367, `size as usize`), so on the 64-bit
/// target the rounding `next_multiple_of(4096)` of EVERY reachable request -- up to
/// `u32::MAX` -- is representable (the exact `align_up` mirror runs with its no-overflow
/// precondition discharged), and a request larger than the pool fails with `PoolFull`.
/// (`mt_insert` itself is proved panic-free for every `u32`.) The latent overflow for a
/// direct `usize` caller is the UNSCORED witness below.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(!slot_at(st.slots@, key))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[requires(size@ > st.allocator.capacity@)]
#[ensures(align_up_l(size@) <= usize::MAX@)]
#[ensures(result.0@ == align_up_l(size@))]
#[ensures(match result.1 { Err(MtError::PoolFull) => true, _ => false })]
pub fn refute_mt_freelist_allocate_rounding_overflow(st: &mut StateModel, key: Key, size: u32)
    -> (usize, Result<usize, MtError>) {
    // allocator.rs:42 `size.next_multiple_of(ALIGNMENT)` on the value insert passes
    let a = align_up(size as usize);
    proof_assert!(forall<j: Int> 0 <= j && j < st.allocator.regions@.len()
                  ==> st.allocator.regions@[j].size@ <= st.allocator.capacity@);
    snapshot! { lem_al(size@) };
    proof_assert!(no_region_fits(st.allocator.regions@, align_up_l(size@)));
    let r = mt_insert(st, key, size);
    (a, r)
}

/// Anti-vacuity twin: MUST FAIL (claims the too-large request SUCCEEDS).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(!slot_at(st.slots@, key))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[requires(size@ > st.allocator.capacity@)]
#[ensures(align_up_l(size@) <= usize::MAX@)]
#[ensures(match result { Ok(_) => true, _ => false })]
pub fn refute_mt_freelist_allocate_rounding_overflow__mutant(st: &mut StateModel, key: Key, size: u32)
    -> Result<usize, MtError> {
    snapshot! { lem_al(size@) };
    mt_insert(st, key, size)
}

/// UNSCORED witness (MT-FREELIST-ALLOCATE-ROUNDING-OVERFLOW): the latent defect is real
/// at the `FreeList` level -- for a `usize` request in `(usize::MAX - 4095, usize::MAX]`
/// the exact 4 KiB round-up exceeds `usize::MAX`, so `next_multiple_of` would overflow
/// (debug: panic; release: wrap). No such request is reachable through `insert`.
#[logic]
#[requires(s > 18446744073709551615 - 4095 && s <= 18446744073709551615)]
#[ensures(align_up_l(s) > 18446744073709551615)]
pub fn witness_mt_freelist_allocate_rounding_overflow_latent(s: Int) {
    pearlite! { lem_al(s) }
}

/// **MT-CLEAR-DISCARDS-HANDLES** -- REFUTATION (error-case, code-only) -- HAZARD
///
/// Statement: "Wiping the cache throws away every entry's eviction handle in one go
/// without individually untracking any of them, relying entirely on the eviction
/// policy's bulk pool-clearing call ..., so if that bulk call does not fully untrack the
/// keys they remain as candidates with no storage behind them."
///
/// Negation proved with the policy modelled per its interface
/// (`clear_pool`: "Remove all entries from the pool, resetting it to empty",
/// ieviction_policy.rs:113-114): after `clear()` on a set-up cache, EVERY key that had
/// storage before is no longer a candidate, no key at all is tracked, the oldest-keys
/// query is empty for every `n`, and both eviction entry points report nothing to
/// evict. The leftover-candidate outcome needs a policy that breaks its own contract.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(match result.0 { Ok(c) => c@ == st.slots@.len(), Err(_) => false })]
#[ensures((^st).slots@.len() == 0)]
#[ensures(forall<k: Key> slot_at(st.slots@, k) ==> !tracks((^st).policy.tracked@, k) && !slot_at((^st).slots@, k))]
#[ensures(forall<k: Key> !tracks((^st).policy.tracked@, k))]
#[ensures(result.1@.len() == 0)]
#[ensures(result.2 == None && result.3 == None)]
pub fn refute_mt_clear_discards_handles(st: &mut StateModel, n: usize, k: Key)
    -> (Result<usize, MtError>, Vec<Key>, Option<Key>, Option<Key>) {
    let c = mt_clear(st);
    let ks = mt_oldest_keys(st, n);
    let e1 = mt_evict_next(st);
    let e2 = mt_evict_next_for_key(st, k);
    (c, ks, e1, e2)
}

/// Anti-vacuity twin: MUST FAIL (claims a key that had storage is STILL a candidate after the wipe).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.slots@.len() > 0)]
#[ensures(match result { Ok(_) => true, Err(_) => false })]
#[ensures(tracks((^st).policy.tracked@, st.slots@[0].key))]
pub fn refute_mt_clear_discards_handles__mutant(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// **MT-CONTAINS-FRAME** (frame, 3 attachments)
///
/// "The presence check changes nothing whatsoever, and in particular it does not count
/// as a use of the entry, so checking for an entry never affects which entry is
/// evicted next."
///
/// `contains` (lib.rs:561-569) reads under the pool READ lock and makes no `ep` call:
/// `mt_contains(&StateModel)`, whose lock skeleton has no policy event; the entry
/// next in line before the check is the one the following eviction takes.
#[requires(mt_inv(*st))]
#[ensures(result.1 == result.4 && result.2 == result.5)]
#[ensures(forall<i: Int> 0 <= i && i < result.6@.len()
          ==> result.6@[i] != LkEv::PolicyRefresh && result.6@[i] != LkEv::PolicyOther)]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result.3 == Some((st.policy.tracked@[0]).0))]
pub fn verify_mt_contains_frame(st: &mut StateModel, k: Key)
    -> (bool, usize, usize, Option<Key>, usize, usize, Vec<LkEv>) {
    let u0 = mt_used(st);
    let c0 = mt_capacity(st);
    let b = mt_contains(st, k);
    let t = lk_peek_or_contains(st.initialized);
    let u1 = mt_used(st);
    let c1 = mt_capacity(st);
    proof_assert!(st.policy.tracked@.len() > 0 ==> tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.policy.tracked@.len() > 0 ==> st.slots@.len() > 0);
    let e = mt_evict_next(st);
    (b, u0, c0, e, u1, c1, t)
}

/// Anti-vacuity twin: MUST FAIL (claims a present key that was checked is protected from eviction).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 0)]
#[ensures(result.0)]
#[ensures(result.1 != Some(result.2))]
pub fn verify_mt_contains_frame__mutant(st: &mut StateModel) -> (bool, Option<Key>, Key) {
    proof_assert!(tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    let k0 = st.policy.tracked[0].0;
    let b = mt_contains(st, k0);
    proof_assert!(st.slots@.len() > 0);
    let e = mt_evict_next(st);
    (b, e, k0)
}

/// **MT-FREE-CAPACITY-POST** (postcondition, divergent, 3 attachments)
///
/// "The free-space query reports the total pool capacity minus the bytes currently in
/// use, and reports zero for a component that has not been set up, so a caller watching
/// for memory pressure can see exactly how much room is left."
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result.0@ == 0)]
#[ensures(result.0@ == result.1@ - result.2@)]
#[ensures(st.initialized ==> result.0@ == st.allocator.capacity@ - st.allocator.used@ && result.0@ >= 0)]
pub fn verify_mt_free_capacity_post(st: &StateModel) -> (usize, usize, usize) {
    let f = mt_free_capacity(st);
    let c = mt_capacity(st);
    let u = mt_used(st);
    (f, c, u)
}

/// Anti-vacuity twin: MUST FAIL (claims free space is the whole capacity, ignoring use).
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result.0@ == 0)]
#[ensures(result.0@ == result.1@)]
pub fn verify_mt_free_capacity_post__mutant(st: &StateModel) -> (usize, usize) {
    let f = mt_free_capacity(st);
    let c = mt_capacity(st);
    (f, c)
}

/// **MT-INSERT-FRAME-ON-ERROR** (frame, 3 attachments)
///
/// "When adding a cache entry fails for any reason nothing about the cache changes: no
/// entry is added or removed, no existing entry's address or size changes, and the
/// reported number of bytes in use is exactly what it was before the call, so no
/// partial reservation is left behind."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => same_state(*st, ^st) })]
#[ensures(match result.0 { Ok(_) => true, Err(_) => result.1 == result.3 && result.2 == result.4 && result.5 == result.6 })]
pub fn verify_mt_insert_frame_on_error(st: &mut StateModel, key: Key, size: u32, k2: Key)
    -> (Result<usize, MtError>, usize, Option<(usize, u32)>, usize, Option<(usize, u32)>, bool, bool) {
    let u0 = mt_used(st);
    let q0 = mt_peek(st, k2);
    let b0 = mt_contains(st, k2);
    let s0 = snapshot! { st.slots@ };
    let r = mt_insert(st, key, size);
    let u1 = mt_used(st);
    let q1 = mt_peek(st, k2);
    let b1 = mt_contains(st, k2);
    proof_assert!(match r { Ok(_) => true, Err(_) => st.slots@ == *s0 && slot_keys_unique(st.slots@) });
    proof_assert!(match r { Ok(_) => true, Err(_) => q0 == q1 });
    (r, u0, q0, u1, q1, b0, b1)
}

/// Anti-vacuity twin: MUST FAIL (claims bytes in use are unchanged on SUCCESS too).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => true, Err(_) => result.1 == result.2 })]
#[ensures(result.1 == result.2)]
pub fn verify_mt_insert_frame_on_error__mutant(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, usize, usize) {
    let u0 = mt_used(st);
    let r = mt_insert(st, key, size);
    let u1 = mt_used(st);
    (r, u0, u1)
}

/// **MT-INSERT-POST-POINTER** (postcondition, 3 attachments)
///
/// "When adding a cache entry succeeds the caller receives a non-null address, computed
/// as the pool's base address plus the byte offset the allocator reserved, that lies
/// inside the pool's own memory range and refers to a block at least as large as the
/// number of bytes requested."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> p@ > 0)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> (^st).slots@.len() > 0
          && p@ == st.pool_base@ + (^st).slots@[(^st).slots@.len() - 1].offset@
          && (^st).slots@[(^st).slots@.len() - 1].key == key)]
#[ensures(forall<p: usize> result.0 == Ok(p) ==> match result.1 { Some((b, s)) =>
          b@ <= p@ && p@ + size@ <= p@ + align_up_l(size@) && p@ + align_up_l(size@) <= b@ + s@, None => false })]
pub fn verify_mt_insert_post_pointer(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, Option<(usize, usize)>) {
    let s0 = snapshot! { st.slots@ };
    let r = mt_insert(st, key, size);
    snapshot! { lem_al(size@) };
    proof_assert!(forall<p: usize> r == Ok(p) ==> st.initialized && st.slots@.len() == s0.len() + 1
                  && sl_end(st.slots@[s0.len()]) <= st.allocator.capacity@
                  && st.allocator.capacity@ == st.pool_size@ && st.pool_base@ > 0);
    let pi = mt_pool_info(st);
    (r, pi)
}

/// Anti-vacuity twin: MUST FAIL (claims the address is always the pool base itself).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(forall<p: usize> result == Ok(p) ==> p@ > 0)]
#[ensures(forall<p: usize> result == Ok(p) ==> p == st.pool_base)]
pub fn verify_mt_insert_post_pointer__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-INSERT-POST-USED** (postcondition, 3 attachments)
///
/// "After adding a cache entry succeeds the reported number of bytes in use grows by
/// exactly the requested size rounded up to the next multiple of four kibibytes and by
/// nothing more, so a one-byte entry consumes four kibibytes of the pool's budget."
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.2@ == result.1@ + align_up_l(size@), Err(_) => true })]
#[ensures(match result.3 { Ok(_) => result.4@ == result.2@ + 4096, Err(_) => true })]
pub fn verify_mt_insert_post_used(st: &mut StateModel, key: Key, size: u32, k1: Key)
    -> (Result<usize, MtError>, usize, usize, Result<usize, MtError>, usize) {
    let u0 = mt_used(st);
    let r = mt_insert(st, key, size);
    let u1 = mt_used(st);
    let r1 = mt_insert(st, k1, 1);
    let u2 = mt_used(st);
    (r, u0, u1, r1, u2)
}

/// Anti-vacuity twin: MUST FAIL (claims a one-byte entry consumes one byte).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.2@ >= result.1@, Err(_) => true })]
#[ensures(match result.0 { Ok(_) => result.2@ == result.1@ + 1, Err(_) => true })]
pub fn verify_mt_insert_post_used__mutant(st: &mut StateModel, k1: Key) -> (Result<usize, MtError>, usize, usize) {
    let u0 = mt_used(st);
    let r = mt_insert(st, k1, 1);
    let u1 = mt_used(st);
    (r, u0, u1)
}

/// **MT-OLDEST-KEYS-POST-LIVE-MEMBERSHIP** -- REFUTATION (postcondition, divergent)
///
/// Statement: "Every key returned by the oldest-keys query is a key that is actually
/// still present in the cache at the time of the call, so a caller can look each one up
/// without getting a miss."
///
/// Negation proved, with the policy obeying its interface throughout: a reachable
/// QUIESCENT state in which `oldest_keys` returns a key that has no entry. Interleaving
/// at lock granularity (every pool mutation is one whole critical section):
///   A: evict_next phase 1 -- `identify_next_to_evict` pops the LRU head `k` (no pool lock);
///   B: remove(k)  -- entry removed + freed; `ep.remove` fails (handle consumed), discarded;
///   C: insert(k)  -- a fresh entry for `k`; the policy tracks `k` again;
///   A: phase 2    -- `slots.remove(&k)` finds C's FRESH entry and frees it.
/// Now the policy tracks `k`, the pool has no entry for `k`, and every later
/// `oldest_keys(n >= #tracked)` returns `k`. (The sequential form holds -- unscored
/// witness below; the divergence note's mechanism, "eviction never untracks its victim",
/// is NOT how it happens: `identify_next_to_evict` does untrack per the interface.)
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(i@ < st.slots@.len())]
#[requires(st.policy.tracked@.len() > 0 && st.slots@[i@].key == (st.policy.tracked@[0]).0)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
#[ensures(match result.3 { Ok(_) => true, Err(_) => false })]
#[ensures(!slot_at((^st).slots@, st.slots@[i@].key))]
#[ensures(!mt_contains_l(^st, st.slots@[i@].key))]
#[ensures(exists<j: Int> 0 <= j && j < result.0@.len() && result.0@[j] == st.slots@[i@].key)]
pub fn refute_mt_oldest_keys_post_live_membership(st: &mut StateModel, i: usize, tel: &mut TelemetryModel)
    -> (Vec<Key>, Result<usize, MtError>, Option<Key>, Result<(), MtError>) {
    let sl = st.slots[i];
    let k = sl.key;
    snapshot! { lem_mt_pool(*st) };
    proof_assert!(sl.size@ > 0);
    // A: phase 1
    let v = ev_identify(st);
    proof_assert!(v == Some(k));
    proof_assert!(!tracks(st.policy.tracked@, k));
    // B: remove(k)
    proof_assert!(slot_at(st.slots@, k) && st.slots@[i@] == sl);
    let rm = rm_concurrent(st, k);
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(sl.size@)));
    // C: insert(k, size)
    let ins = ins_concurrent(st, k, sl.size);
    proof_assert!(tracks(st.policy.tracked@, k));
    // A: phase 2 -- removes C's fresh entry
    let ev = ev_commit(st, k, tel);
    proof_assert!(tracks(st.policy.tracked@, k) && !slot_at(st.slots@, k));
    proof_assert!(st.policy.tracked@.len() > 0);
    // any later oldest_keys
    let n = st.policy.tracked.len();
    let ks = ok_concurrent(st, n);
    (ks, ins, ev, rm)
}

/// Anti-vacuity twin: MUST FAIL (claims, after the same interleaving, the key IS present).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(i@ < st.slots@.len())]
#[requires(st.policy.tracked@.len() > 0 && st.slots@[i@].key == (st.policy.tracked@[0]).0)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result { Ok(_) => true, Err(_) => false })]
#[ensures(slot_at((^st).slots@, st.slots@[i@].key))]
pub fn refute_mt_oldest_keys_post_live_membership__mutant(st: &mut StateModel, i: usize, tel: &mut TelemetryModel)
    -> Result<usize, MtError> {
    let sl = st.slots[i];
    let k = sl.key;
    snapshot! { lem_mt_pool(*st) };
    proof_assert!(sl.size@ > 0);
    let _v = ev_identify(st);
    proof_assert!(slot_at(st.slots@, k) && st.slots@[i@] == sl);
    let _rm = rm_concurrent(st, k);
    let ins = ins_concurrent(st, k, sl.size);
    let _ev = ev_commit(st, k, tel);
    ins
}

/// UNSCORED witness (MT-OLDEST-KEYS-POST-LIVE-MEMBERSHIP): the SEQUENTIAL form holds --
/// after any sequence of whole calls, every key oldest_keys returns is present.
#[requires(mt_inv(*st))]
#[requires(!st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 1 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(forall<j: Int> 0 <= j && j < result@.len() ==> mt_contains_l(^st, result@[j]))]
pub fn witness_mt_oldest_keys_post_live_membership_sequential(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, n: usize)
    -> Vec<Key> {
    mt_run(st, ops, psz);
    mt_oldest_keys(st, n)
}

/// **MT-REMOVE-IGNORES-POLICY-ERROR** -- REFUTATION (error-case, code-only) -- HAZARD
///
/// Statement: "Deleting an entry removes it from the pool first and only then asks the
/// eviction policy to stop tracking it, discarding whatever the policy answers, so if
/// the untracking fails the caller is still told the deletion succeeded while the
/// policy keeps offering a key whose storage is gone."
///
/// Negation proved, policy per its interface (`remove(handle)` fails only with
/// `InvalidHandle` = "invalid or already removed", ieviction_policy.rs:57-58/98-99; the
/// pool id is valid from initialize on, so `InvalidPool` cannot occur):
///  (a) sequentially the untrack always succeeds: after `remove(k) == Ok` the policy
///      does not track `k`, the oldest-keys query never offers it, the next eviction
///      never nominates it;
///  (b) the one way `ep.remove` CAN fail -- a racing `evict_next` already consumed the
///      handle -- leaves the key untracked too: the failed call has nothing to keep.
/// So a success report never coexists with the policy still offering THAT deletion's key.
#[requires(mt_inv(*st))]
#[requires(mt_inv(*st2))]
#[requires(st2.initialized && st2.slots@.len() > 0)]
#[ensures(match result.0 { Ok(_) => !tracks((^st).policy.tracked@, key) && !slot_at((^st).slots@, key), Err(_) => true })]
#[ensures(forall<i: Int> 0 <= i && i < result.1@.len() ==> result.1@[i] != key)]
#[ensures(result.2 != Some(key))]
#[ensures(match result.3 { Some(k) => match result.4 { Ok(_) => !tracks((^st2).policy.tracked@, k)
          && !slot_at((^st2).slots@, k), Err(_) => false }, None => false })]
pub fn refute_mt_remove_ignores_policy_error(st: &mut StateModel, key: Key, n: usize, st2: &mut StateModel)
    -> (Result<(), MtError>, Vec<Key>, Option<Key>, Option<Key>, Result<(), MtError>) {
    // (a) sequential
    let r = mt_remove(st, key);
    proof_assert!(!st.initialized ==> !tracks(st.policy.tracked@, key));
    let ks = mt_oldest_keys(st, n);
    let e = mt_evict_next(st);
    // (b) the failing-untrack interleaving: evict_next phase 1, then remove of that key
    proof_assert!(slot_at(st2.slots@, st2.slots@[0].key));
    proof_assert!(st2.policy.tracked@.len() > 0);
    snapshot! { lem_mt_pool(*st2) };
    let s0 = snapshot! { st2.slots@ };
    let t0 = snapshot! { st2.policy.tracked@ };
    let v = ev_identify(st2);
    let r2 = match v {
        Some(k) => {
            proof_assert!(tracks(*t0, k) && slot_at(*s0, k));
            rm_concurrent(st2, k)
        }
        None => Err(MtError::KeyNotFound),
    };
    (r, ks, e, v, r2)
}

/// Anti-vacuity twin: MUST FAIL (claims the deleted key is still tracked after success).
#[requires(mt_inv(*st))]
#[ensures(match result { Ok(_) => !slot_at((^st).slots@, key), Err(_) => true })]
#[ensures(match result { Ok(_) => tracks((^st).policy.tracked@, key), Err(_) => true })]
pub fn refute_mt_remove_ignores_policy_error__mutant(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    mt_remove(st, key)
}

// ===========================================================================
// 14. Batch 4 — telemetry-carrying evict_next, alloc_mmap / spdk initialize
//     mirrors with the OS results as INPUTS, Drop, and the batch-4 drivers.
// ===========================================================================

/// `evict_next` WITH its eviction counter (lib.rs:438-468, `telemetry` feature),
/// sequential composition of phase 1 (`ev_identify`, no pool lock) and phase 2
/// (`ev_commit`, pool write lock + `evictions.fetch_add(1)` -- outside the `if let`).
#[requires(mt_inv(*st))]
#[ensures(match result { Some(_) => (^tel).evictions@ == fetch_l(tel.evictions@), None => ^tel == *tel })]
#[ensures(match result { Some(k) => st.initialized && st.policy.tracked@.len() > 0
          && k == (st.policy.tracked@[0]).0 && slot_at(st.slots@, k) && !slot_at((^st).slots@, k),
          None => (^st).slots@ == st.slots@ })]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result != None)]
#[ensures(forall<k2: Key> result != Some(k2) ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
#[ensures((^tel).write_lock_contentions == tel.write_lock_contentions
          && (^tel).read_lock_contentions == tel.read_lock_contentions)]
pub fn mt_evict_next_tel(st: &mut StateModel, tel: &mut TelemetryModel) -> Option<Key> {
    snapshot! { lem_mt_pool(*st) };
    let s0 = snapshot! { st.slots@ };
    let t0 = snapshot! { st.policy.tracked@ };
    match ev_identify(st) {
        None => None,
        Some(k) => {
            proof_assert!(tracks(*t0, k));
            proof_assert!(slot_at(*s0, k));
            ev_commit(st, k, tel)
        }
    }
}

/// The three counters as one value.
#[logic(open)]
pub fn trip(t: TelemetryModel) -> (u64, u64, u64) {
    pearlite! { (t.evictions, t.write_lock_contentions, t.read_lock_contentions) }
}

/// `MemoryTierComponent::telemetry` (lib.rs:167-171) -> `MemoryTierTelemetry::snapshot`
/// (lib.rs:50-56): three SEPARATE `load(Relaxed)`s, under `state.read()` only.
#[ensures(result == trip(*t))]
pub fn mt_telemetry_inherent(t: &TelemetryModel) -> (u64, u64, u64) {
    let e = t.evictions;
    let w = t.write_lock_contentions;
    let r = t.read_lock_contentions;
    (e, w, r)
}

// ---------------------------------------------------------------------------
// alloc_mmap (lib.rs:194-258) with the OS results as universally-quantified INPUTS
// ---------------------------------------------------------------------------

/// `libc::MAP_FAILED` == `(void*)-1`.
pub const MAP_FAILED: usize = usize::MAX;
/// `size_of::<libc::c_ulong>() * 8` on the supported (64-bit Linux) targets.
pub const C_ULONG_BITS: usize = 64;
/// `DEFAULT_POOL_SIZE` (lib.rs:37): 256 MiB.
pub const DEFAULT_POOL_SIZE: usize = 256 * 1024 * 1024;

/// ASSUMPTION (stated as a PRECONDITION, not `#[trusted]`): a non-`MAP_FAILED` result of
/// `mmap(NULL, size, ..)` is page aligned, non-null, and the mapping fits the address space
/// (POSIX/Linux mmap(2); non-null holds because the hint is NULL and MAP_FIXED is not used).
#[logic(open)]
pub fn mmap_ok(r: usize, size: usize) -> bool {
    pearlite! { r != MAP_FAILED ==> r@ % 4096 == 0 && r@ > 0 && r@ + size@ <= usize::MAX@ }
}

/// ASSUMPTION (precondition): a non-NULL `spdk_zmalloc(size, 4096, ..)` result is 4096-aligned
/// and the region fits the address space (SPDK env.h contract).
#[logic(open)]
pub fn zmalloc_ok(r: usize, size: usize) -> bool {
    pearlite! { r != 0usize ==> r@ % 4096 == 0 && r@ + size@ <= usize::MAX@ }
}

/// GHOST: the OS calls `alloc_mmap` issued (recorded, not executed).
pub struct MmapLog {
    pub huge_calls: u64,
    pub plain_calls: u64,
    pub mbind_issued: bool,
    pub mbind_mask: u64,
    pub mbind_maxnode: usize,
    pub mbind_node: usize,
}

/// `MemoryTierComponent::alloc_mmap` (lib.rs:194-258), line by line. `r_huge` is what
/// `mmap(.., MAP_HUGETLB, ..)` returned, `r_plain` what the fallback `mmap` returned (only
/// consulted if it is called), `rc` what `syscall(SYS_mbind, ..)` returned: all arbitrary.
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures((^ml).huge_calls@ == ml.huge_calls@ + 1)]
#[ensures((^ml).plain_calls@ == ml.plain_calls@ + if r_huge == MAP_FAILED { 1 } else { 0 })]
#[ensures(r_huge != MAP_FAILED ==> result == Ok(r_huge))]
#[ensures(r_huge == MAP_FAILED && r_plain != MAP_FAILED ==> result == Ok(r_plain))]
#[ensures(r_huge == MAP_FAILED && r_plain == MAP_FAILED ==> result == Err(MtError::AllocationFailed))]
#[ensures(match result { Ok(p) => p != MAP_FAILED, Err(e) => e == MtError::AllocationFailed })]
#[ensures(match result { Ok(_) => true, Err(_) => (^ml).mbind_issued == ml.mbind_issued
          && (^ml).mbind_mask == ml.mbind_mask && (^ml).mbind_maxnode == ml.mbind_maxnode
          && (^ml).mbind_node == ml.mbind_node })]
// NUMA binding (lib.rs:226-256)
#[ensures(forall<n: i32> numa == Some(n) && n@ >= 0 && result != Err(MtError::AllocationFailed)
          ==> (^ml).mbind_issued && (^ml).mbind_node@ == n@ && (^ml).mbind_maxnode@ == n@ + 2
              && (n@ >= C_ULONG_BITS@ ==> (^ml).mbind_mask@ == 0))]
#[ensures(forall<n: i32> (numa == None || (numa == Some(n) && n@ < 0))
          ==> (^ml).mbind_issued == ml.mbind_issued && (^ml).mbind_mask == ml.mbind_mask
              && (^ml).mbind_maxnode == ml.mbind_maxnode && (^ml).mbind_node == ml.mbind_node)]
#[ensures(!connected ==> ^lg == *lg)]
// --- batch 5: MT-INIT-NUMA-NEGATIVE-IGNORED -- no NUMA request / a negative one logs NOTHING ---
#[ensures(numa == None ==> ^lg == *lg)]
#[ensures(forall<n: i32> numa == Some(n) && n@ < 0 ==> ^lg == *lg)]
pub fn alloc_mmap_m(
    pool_size: usize,
    numa: Option<i32>,
    r_huge: usize,
    r_plain: usize,
    rc: i64,
    ml: &mut MmapLog,
    lg: &mut LogSink,
    connected: bool,
) -> Result<usize, MtError> {
    let _ = pool_size;
    ml.huge_calls += 1; // lib.rs:199-208 mmap(.., MAP_HUGETLB, ..)
    let ptr = r_huge;
    let ptr = if ptr == MAP_FAILED {
        ml.plain_calls += 1; // lib.rs:211-220 mmap(.., MAP_PRIVATE | MAP_ANONYMOUS, ..)
        let p = r_plain;
        if p == MAP_FAILED {
            return Err(MtError::AllocationFailed); // lib.rs:221-222
        }
        p
    } else {
        ptr
    };
    match numa {
        Some(node) => {
            if node >= 0 {
                let node_id = node as usize; // lib.rs:228
                let mut nodemask: u64 = 0; // lib.rs:229
                if node_id < C_ULONG_BITS {
                    nodemask = 1u64 << node_id; // lib.rs:230-232
                }
                // lib.rs:234-243: syscall(SYS_mbind, ptr, pool_size, MPOL_BIND, &nodemask, node_id + 2, 0)
                let maxnode = node_id + 2;
                ml.mbind_issued = true;
                ml.mbind_mask = nodemask;
                ml.mbind_maxnode = maxnode;
                ml.mbind_node = node_id;
                if rc == 0 {
                    log_msg(lg, connected); // lib.rs:245
                } else {
                    log_msg(lg, connected); // lib.rs:247-251 (warning; NOT an error)
                }
            }
        }
        None => {}
    }
    Ok(ptr)
}

/// The two set-up outcomes that a successful mmap-path initialize stores.
#[logic(open)]
pub fn mmap_base(r_huge: usize, r_plain: usize) -> usize {
    pearlite! { if r_huge != MAP_FAILED { r_huge } else { r_plain } }
}

/// `IMemoryTier::initialize` (lib.rs:261-330) on the mmap path (non-`spdk` build, or `spdk`
/// build with the SPDK env inactive): guards, `alloc_mmap(pool_size, numa_node)?`, then the
/// same tail as `mt_initialize` (`init_logged`). The receptacle lookup (lib.rs:266-268) is
/// modelled separately by `rx_initialize`.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 ==> pool_size@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, pool_size) && mmap_ok(r_plain, pool_size))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(mt_inv(^st))]
#[ensures(pool_size@ == 0 ==> result == Err(MtError::InvalidSize) && ^st == *st && ^ml == *ml)]
#[ensures(st.initialized && pool_size@ > 0 ==> result == Err(MtError::AllocationFailed) && ^st == *st && ^ml == *ml)]
#[ensures(!st.initialized && pool_size@ > 0 && (r_huge != MAP_FAILED || r_plain != MAP_FAILED) ==> result == Ok(()))]
#[ensures(!st.initialized && pool_size@ > 0 && r_huge == MAP_FAILED && r_plain == MAP_FAILED
          ==> result == Err(MtError::AllocationFailed))]
#[ensures(match result { Ok(_) => true, Err(_) => ^st == *st })]
#[ensures(match result { Ok(_) => (^st).initialized && (^st).pool_size == pool_size
          && (^st).pool_base == mmap_base(r_huge, r_plain) && (^st).pool_base@ > 0
          && (^st).allocator.capacity == pool_size && (^st).allocator.used@ == 0
          && (^st).allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: pool_size })
          && (^st).slots@ == Seq::empty() && !(^st).spdk_allocated
          && (^st).pool_id == st.policy.pools && (^st).policy.pools@ == st.policy.pools@ + 1
          && (^st).policy.tracked@ == st.policy.tracked@ && (^st).policy.next_handle == st.policy.next_handle
          && (^st).poisoned == st.poisoned
          && (^ml).huge_calls@ == ml.huge_calls@ + 1, Err(_) => true })]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
#[ensures(!st.initialized && pool_size@ > 0 && r_huge != MAP_FAILED ==> (^ml).plain_calls == ml.plain_calls)]
#[ensures(!st.initialized && pool_size@ > 0 && r_huge == MAP_FAILED ==> (^ml).plain_calls@ == ml.plain_calls@ + 1)]
#[ensures(forall<n: i32> !st.initialized && pool_size@ > 0 && numa == Some(n) && n@ >= 0 && result == Ok(())
          ==> (^ml).mbind_issued && (^ml).mbind_node@ == n@ && (^ml).mbind_maxnode@ == n@ + 2
              && (n@ >= C_ULONG_BITS@ ==> (^ml).mbind_mask@ == 0))]
pub fn mt_initialize_mmap(
    st: &mut StateModel,
    pool_size: usize,
    numa: Option<i32>,
    r_huge: usize,
    r_plain: usize,
    rc: i64,
    ml: &mut MmapLog,
    lg: &mut LogSink,
    connected: bool,
) -> Result<(), MtError> {
    if pool_size == 0 {
        return Err(MtError::InvalidSize);
    }
    if st.initialized {
        return Err(MtError::AllocationFailed);
    }
    let a = match alloc_mmap_m(pool_size, numa, r_huge, r_plain, rc, ml, lg, connected) {
        Ok(p) => Some(p),
        Err(_) => None,
    };
    init_logged(st, pool_size, a, lg, connected)
}

/// `IMemoryTier::initialize` in an `spdk` build (lib.rs:276-306): `env_active` =
/// `interfaces::is_spdk_env_active()`; `r_spdk` = what `spdk_zmalloc(pool_size, 4096, NULL,
/// node, DMA)` returned (0 == NULL). Active env -> SPDK pool, `spdk_allocated = true`;
/// otherwise the mmap path. DISCLOSED: on the SPDK branch the `spdk_allocated = true` store is
/// mirrored AFTER the flag store (lib.rs:321 is before :322); sequential properties only.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 ==> pool_size@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, pool_size) && mmap_ok(r_plain, pool_size) && zmalloc_ok(r_spdk, pool_size))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(zl.calls@ < u64::MAX@)]
#[ensures(mt_inv(^st))]
#[ensures(match result { Ok(_) => true, Err(_) => ^st == *st })]
#[ensures(env_active ==> ^ml == *ml)]
#[ensures(!st.initialized && pool_size@ > 0 && env_active && r_spdk@ != 0 ==> result == Ok(()))]
#[ensures(env_active && r_spdk@ == 0 ==> match result { Err(MtError::AllocationFailed) => true, Err(MtError::InvalidSize) => true, _ => false })]
#[ensures(match result { Ok(_) => (^st).initialized && (^st).spdk_allocated == env_active
          && (^st).pool_size == pool_size && (^st).allocator.capacity == pool_size
          && (env_active ==> (^st).pool_base == r_spdk)
          && (!env_active ==> (^st).pool_base == mmap_base(r_huge, r_plain))
          && (^st).pool_base@ > 0 && (^st).policy.tracked@ == st.policy.tracked@
          && (^st).policy.pools@ == st.policy.pools@ + 1 && (^st).policy.next_handle == st.policy.next_handle
          && (^st).poisoned == st.poisoned, Err(_) => true })]
// --- batch 5: the zmalloc call (ZmLog), and the error outcomes, exposed ---
#[ensures(match result { Ok(_) => (^st).slots@ == Seq::empty() && (^st).allocator.used@ == 0
          && (^st).allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: pool_size }), Err(_) => true })]
#[ensures(!env_active ==> ^zl == *zl)]
#[ensures(pool_size@ == 0 ==> result == Err(MtError::InvalidSize) && ^st == *st && ^ml == *ml && ^zl == *zl)]
#[ensures(st.initialized && pool_size@ > 0 ==> result == Err(MtError::AllocationFailed)
          && ^st == *st && ^ml == *ml && ^zl == *zl)]
#[ensures(!st.initialized && pool_size@ > 0 && env_active ==> (^zl).calls@ == zl.calls@ + 1
          && (^zl).numa_id == numa_or_any(numa) && (^zl).size == pool_size)]
#[ensures(!st.initialized && pool_size@ > 0 && env_active && r_spdk@ == 0 ==> result == Err(MtError::AllocationFailed))]
#[ensures(!st.initialized && pool_size@ > 0 && !env_active && (r_huge != MAP_FAILED || r_plain != MAP_FAILED)
          ==> result == Ok(()))]
#[ensures(!st.initialized && pool_size@ > 0 && !env_active && r_huge == MAP_FAILED && r_plain == MAP_FAILED
          ==> result == Err(MtError::AllocationFailed))]
#[ensures(match result { Err(MtError::NotEvictable) => false, _ => true })]
// --- batch 6: MT-IS-DMA-CAPABLE-HUGEPAGE-MMAP-UNDERSTATED -- huge-page mmap success, no fallback mmap ---
#[ensures(!st.initialized && pool_size@ > 0 && !env_active && r_huge != MAP_FAILED
          ==> (^ml).plain_calls == ml.plain_calls)]
pub fn mt_initialize_spdk(
    st: &mut StateModel,
    pool_size: usize,
    numa: Option<i32>,
    env_active: bool,
    r_spdk: usize,
    r_huge: usize,
    r_plain: usize,
    rc: i64,
    ml: &mut MmapLog,
    zl: &mut ZmLog,
    lg: &mut LogSink,
    connected: bool,
) -> Result<(), MtError> {
    if pool_size == 0 {
        return Err(MtError::InvalidSize);
    }
    if st.initialized {
        return Err(MtError::AllocationFailed);
    }
    if env_active {
        // lib.rs:280 (pin) `let node_id = numa_node.unwrap_or(-1);`
        let node_id = match numa {
            Some(n) => n,
            None => -1,
        };
        // lib.rs:282-290 (pin) spdk_zmalloc(pool_size, 4096, NULL, node_id, SPDK_MALLOC_DMA)
        let r_spdk = spdk_zmalloc_m(pool_size, node_id, r_spdk, zl);
        if r_spdk == 0 {
            return Err(MtError::AllocationFailed); // lib.rs:290-294
        }
        log_msg(lg, connected); // lib.rs:295-298
        let r = init_logged(st, pool_size, Some(r_spdk), lg, connected);
        st.spdk_allocated = true; // lib.rs:321 (spdk_allocated = true)
        r
    } else {
        let a = match alloc_mmap_m(pool_size, numa, r_huge, r_plain, rc, ml, lg, connected) {
            Ok(p) => Some(p),
            Err(_) => None,
        };
        init_logged(st, pool_size, a, lg, connected)
    }
}

/// GHOST (batch 5): the `spdk_zmalloc` call `initialize` issued on the SPDK path (recorded,
/// not executed). `numa_id` is its 4th argument -- per `spdk/env.h:105-120` "NUMA node ID to
/// allocate memory on, or SPDK_ENV_NUMA_ID_ANY (-1) for any NUMA node".
pub struct ZmLog {
    pub calls: u64,
    pub numa_id: i32,
    pub size: usize,
}

/// `numa_node.unwrap_or(-1)` (lib.rs:280, pin): `None` becomes SPDK_ENV_NUMA_ID_ANY.
#[logic(open)]
pub fn numa_or_any(numa: Option<i32>) -> i32 {
    pearlite! { match numa { Some(n) => n, None => -1i32 } }
}

/// `spdk_sys::spdk_zmalloc(size, 4096, NULL, numa_id, DMA)`: records the call, returns the
/// (arbitrary) result `r` it is handed -- the SPDK allocator's answer is an INPUT.
#[ensures((^zl).calls@ == if zl.calls@ < u64::MAX@ { zl.calls@ + 1 } else { zl.calls@ })]
#[ensures((^zl).numa_id == numa_id && (^zl).size == size)]
#[ensures(result == r)]
pub fn spdk_zmalloc_m(size: usize, numa_id: i32, r: usize, zl: &mut ZmLog) -> usize {
    if zl.calls < u64::MAX {
        zl.calls += 1;
    }
    zl.numa_id = numa_id;
    zl.size = size;
    r
}

// ---------------------------------------------------------------------------
// Drop for MemoryTierState (lib.rs:120-141)
// ---------------------------------------------------------------------------

/// GHOST: the release calls the destructor issued (recorded, not executed).
pub struct ReleaseLog {
    pub munmaps: u64,
    pub spdk_frees: u64,
    pub last_base: usize,
    pub last_len: usize,
}

/// `impl Drop for MemoryTierState` (lib.rs:120-141). `spdk_build` = the `spdk` cargo feature
/// (whether the `#[cfg(feature = "spdk")]` block exists); `env_active` =
/// `interfaces::is_spdk_env_active()`, a process-global AtomicBool (spdk_types.rs:202-215)
/// cleared by spdk-env's `do_fini` BEFORE `spdk_env_fini()` (spdk-env/src/env.rs:187-199).
/// `MemoryTierState` holds NO handle to the eviction policy, so Drop cannot call it: the
/// mirror touches only `pool_base` (and the ghost log).
#[requires(rl.munmaps@ < u64::MAX@ && rl.spdk_frees@ < u64::MAX@)]
#[ensures((^st).pool_base@ == 0)]
#[ensures(st.pool_base@ == 0 ==> ^rl == *rl)]
#[ensures(st.pool_base@ != 0 && !(spdk_build && st.spdk_allocated)
          ==> (^rl).munmaps@ == rl.munmaps@ + 1 && (^rl).spdk_frees == rl.spdk_frees
              && (^rl).last_base == st.pool_base && (^rl).last_len == st.pool_size)]
#[ensures(st.pool_base@ != 0 && spdk_build && st.spdk_allocated && env_active
          ==> (^rl).spdk_frees@ == rl.spdk_frees@ + 1 && (^rl).munmaps == rl.munmaps
              && (^rl).last_base == st.pool_base)]
#[ensures(spdk_build && st.spdk_allocated && !env_active ==> ^rl == *rl)]
#[ensures((^st).pool_size == st.pool_size && (^st).pool_id == st.pool_id
          && (^st).allocator.regions@ == st.allocator.regions@ && (^st).allocator.capacity == st.allocator.capacity
          && (^st).allocator.used == st.allocator.used && (^st).slots@ == st.slots@
          && (^st).initialized == st.initialized && (^st).spdk_allocated == st.spdk_allocated
          && (^st).poisoned == st.poisoned)]
#[ensures((^st).policy.tracked@ == st.policy.tracked@ && (^st).policy.next_handle == st.policy.next_handle
          && (^st).policy.pools == st.policy.pools && (^st).policy.last_pool == st.policy.last_pool)]
pub fn mt_drop(st: &mut StateModel, spdk_build: bool, env_active: bool, rl: &mut ReleaseLog) {
    if st.pool_base == 0 {
        return; // lib.rs:122-124
    }
    if spdk_build && st.spdk_allocated {
        if env_active {
            rl.spdk_frees += 1; // lib.rs:128-130 spdk_free(pool_ptr)
            rl.last_base = st.pool_base;
            rl.last_len = st.pool_size;
        }
        st.pool_base = 0; // lib.rs:132
        return; // lib.rs:133
    }
    rl.munmaps += 1; // lib.rs:135-137 munmap(pool_ptr, pool_size) -- return value ignored
    rl.last_base = st.pool_base;
    rl.last_len = st.pool_size;
    st.pool_base = 0; // lib.rs:138
}

// ---------------------------------------------------------------------------
// Batch 4 -- property drivers
// ---------------------------------------------------------------------------

/// **MT-TELEMETRY-EVICTION-COUNT** -- REFUTATION (postcondition, divergent)
///
/// Statement: "... the eviction counter ... grows by exactly one for each eviction call in
/// which the policy named a victim and by nothing for calls in which no victim was named, so
/// it equals the total number of entries evicted since the counters were last reset."
/// Negation proved: after a reset, ONE eviction call in which the (contract-obeying) policy
/// named a victim leaves the counter at 1 while that call evicted NO entry (thread B's
/// `remove` of the nominee landed in the lock gap between identify and the pool lock). The
/// per-call growth half HOLDS (see `witness_mt_telemetry_eviction_count_sequential`); the
/// "equals the number of entries evicted" conclusion does not.
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.slots@.len() > 0)]
// counters were reset, then exactly one eviction call ran, and its policy named a victim ...
#[ensures(match result.0 { Some(k) => st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0
          && slot_at(st.slots@, k), None => false })]
#[ensures((^tel).evictions@ == 1)]
// ... the entry left through B's deletion, not through the eviction ...
#[ensures(match result.2 { Ok(_) => true, Err(_) => false })]
// ... and the eviction call itself removed no entry at all: 0 entries evicted, counter 1
#[ensures((^st).slots@ == *result.1)]
pub fn refute_mt_telemetry_eviction_count(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Snapshot<Seq<SlotModel>>, Result<(), MtError>) {
    tel_reset(tel); // reset_telemetry (lib.rs:174-176)
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    proof_assert!(st.policy.tracked@.len() > 0);
    snapshot! { lem_mt_pool(*st) };
    let s0 = snapshot! { st.slots@ };
    let t0 = snapshot! { st.policy.tracked@ };
    let v = ev_identify(st); // A: phase 1, no pool lock
    let k = match v {
        Some(k) => k,
        None => return (None, snapshot! { st.slots@ }, Err(MtError::KeyNotFound)),
    };
    proof_assert!(tracks(*t0, k));
    proof_assert!(slot_at(*s0, k));
    let rm = rm_concurrent(st, k); // B: remove(k), whole critical section
    let s_mid = snapshot! { st.slots@ };
    let ev = ev_commit(st, k, tel); // A: phase 2 -- counts the eviction
    (ev, s_mid, rm)
}

/// Anti-vacuity twin: MUST FAIL (claims the counted eviction removed an entry).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[requires(st.slots@.len() > 0)]
#[ensures((^tel).evictions@ == 1)]
#[ensures((^st).slots@ != *result.1)]
pub fn refute_mt_telemetry_eviction_count__mutant(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Snapshot<Seq<SlotModel>>) {
    tel_reset(tel);
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    snapshot! { lem_mt_pool(*st) };
    let s0 = snapshot! { st.slots@ };
    let t0 = snapshot! { st.policy.tracked@ };
    let v = ev_identify(st);
    let k = match v {
        Some(k) => k,
        None => { tel_event(tel, 0); return (None, snapshot! { st.slots@ }); }
    };
    proof_assert!(tracks(*t0, k));
    proof_assert!(slot_at(*s0, k));
    let _rm = rm_concurrent(st, k);
    let s_mid = snapshot! { st.slots@ };
    let ev = ev_commit(st, k, tel);
    (ev, s_mid)
}

/// UNSCORED WITNESS for MT-TELEMETRY-EVICTION-COUNT: the per-call growth half, and the
/// sequential (no interleaving) form of the conclusion -- every counted eviction removes
/// exactly the named entry; an uncounted call removes nothing.
#[requires(mt_inv(*st))]
#[ensures(match result { Some(k) => (^tel).evictions@ == fetch_l(tel.evictions@)
          && slot_at(st.slots@, k) && !slot_at((^st).slots@, k), None => ^tel == *tel && (^st).slots@ == st.slots@ })]
#[ensures(forall<k2: Key> result != Some(k2) ==> slot_at((^st).slots@, k2) == slot_at(st.slots@, k2))]
pub fn witness_mt_telemetry_eviction_count_sequential(st: &mut StateModel, tel: &mut TelemetryModel) -> Option<Key> {
    mt_evict_next_tel(st, tel)
}

/// **MT-CAPACITY-POST** (postcondition)
///
/// "The total-capacity query reports the full size of the pool in bytes as it was requested
/// when the pool was set up, and that number does not change as entries are added, deleted,
/// evicted or wiped."
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1@ == psz@, Err(_) => result.1@ == 0 })]
#[ensures(match result.0 { Ok(_) => result.2@ == psz@, Err(_) => true })]
pub fn verify_mt_capacity_post(st: &mut StateModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (Result<(), MtError>, usize, usize) {
    let r = mt_initialize(st, psz);
    let c0 = mt_capacity(st);
    // any sequence of insert / remove / evict_next / evict_next_for_key / clear / ... calls
    mt_run(st, ops, psz);
    let c1 = mt_capacity(st);
    (r, c0, c1)
}

/// Anti-vacuity twin: MUST FAIL (claims the capacity shrinks after the calls).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && !st.poisoned)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1@ == psz@, Err(_) => result.1@ == 0 })]
#[ensures(match result.0 { Ok(_) => result.2@ < psz@, Err(_) => true })]
pub fn verify_mt_capacity_post__mutant(st: &mut StateModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (Result<(), MtError>, usize, usize) {
    let r = mt_initialize(st, psz);
    let c0 = mt_capacity(st);
    mt_run(st, ops, psz);
    let c1 = mt_capacity(st);
    (r, c0, c1)
}

/// **MT-EVICT-NEXT-ERR-NO-VICTIM** (error-case, divergent)
///
/// "Asking to evict reports that nothing was evicted, and changes nothing at all, both when
/// the pool has not been set up and when the eviction policy nominates no victim."
/// `a` goes through `evict_next`, `b` through `evict_next_for_key(k)`.
#[requires(mt_inv(*a) && mt_inv(*b))]
#[ensures(!a.initialized ==> result.0 == None && ^a == *a)]
#[ensures(!b.initialized ==> result.1 == None && ^b == *b)]
#[ensures(a.initialized && a.policy.tracked@.len() == 0 ==> result.0 == None)]
#[ensures(b.initialized && b.policy.tracked@.len() == 0 ==> result.1 == None)]
#[ensures(result.0 == None ==> same_obs(*a, ^a))]
#[ensures(result.1 == None ==> same_obs(*b, ^b))]
pub fn verify_mt_evict_next_err_no_victim(a: &mut StateModel, b: &mut StateModel, k: Key)
    -> (Option<Key>, Option<Key>) {
    let ra = mt_evict_next(a);
    let rb = mt_evict_next_for_key(b, k);
    (ra, rb)
}

/// Anti-vacuity twin: MUST FAIL (claims a set-up pool with a nominee also answers `None`).
#[requires(mt_inv(*a))]
#[ensures(!a.initialized ==> result == None)]
#[ensures(a.initialized ==> result == None)]
pub fn verify_mt_evict_next_err_no_victim__mutant(a: &mut StateModel) -> Option<Key> {
    mt_evict_next(a)
}

/// UNSCORED WITNESS for the divergence of MT-EVICT-NEXT-ERR-NO-VICTIM: the `None` answer does
/// NOT mean the cache is empty. Two concurrent evictions A and B with ONE tracked entry: A's
/// phase 1 pops it (no pool lock), B's phase 1 then finds no victim -- B answers "nothing
/// evicted" while the pool still holds the entry.
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() == 1)]
#[ensures(result.0 != None)]
#[ensures(result.1 == None)]
#[ensures((^st).slots@.len() > 0)]
pub fn witness_mt_evict_next_err_no_victim_concurrent(st: &mut StateModel) -> (Option<Key>, Option<Key>) {
    proof_assert!(tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.slots@.len() > 0);
    let a = ev_identify(st); // A phase 1
    proof_assert!(st.policy.tracked@.len() == 0 || (forall<x: Key> !tracks(st.policy.tracked@, x)));
    let b = ev_identify(st); // B phase 1: the policy now names no victim
    (a, b)
}

/// **MT-EVICT-NEXT-FRAME-NO-ROTATION-STATE** (frame, spec-only)
///
/// "Eviction keeps no hidden position or rotation counter of its own, so two runs that start
/// from the same cache contents and the same policy state evict the same key and the victim
/// never depends on how many evictions happened before."
/// Two runs with the same contents and policy order but ARBITRARY eviction histories (their
/// eviction counters `ta`, `tb` are unconstrained) evict the same key; that key is a function
/// of the policy order alone.
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(a.initialized == b.initialized)]
#[requires(a.slots@ == b.slots@)]
#[requires(a.policy.tracked@ == b.policy.tracked@)]
#[ensures(result.0 == result.1)]
#[ensures(result.0 == if a.initialized && a.policy.tracked@.len() > 0 { Some((a.policy.tracked@[0]).0) } else { None })]
pub fn verify_mt_evict_next_frame_no_rotation_state(
    a: &mut StateModel, b: &mut StateModel, ta: &mut TelemetryModel, tb: &mut TelemetryModel,
) -> (Option<Key>, Option<Key>) {
    let ra = mt_evict_next_tel(a, ta);
    let rb = mt_evict_next_tel(b, tb);
    (ra, rb)
}

/// Anti-vacuity twin: MUST FAIL (claims different histories give different victims).
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(a.initialized && b.initialized)]
#[requires(a.slots@ == b.slots@ && a.policy.tracked@ == b.policy.tracked@)]
#[requires(a.policy.tracked@.len() > 0)]
#[requires(ta.evictions@ != tb.evictions@)]
#[ensures(result.0 != None)]
#[ensures(result.0 != result.1)]
pub fn verify_mt_evict_next_frame_no_rotation_state__mutant(
    a: &mut StateModel, b: &mut StateModel, ta: &mut TelemetryModel, tb: &mut TelemetryModel,
) -> (Option<Key>, Option<Key>) {
    let ra = mt_evict_next_tel(a, ta);
    let rb = mt_evict_next_tel(b, tb);
    (ra, rb)
}

/// **MT-EVICT-NEXT-POST-POLICY-VICTIM** (postcondition)
///
/// "The key that eviction removes and returns is exactly the key that the external
/// eviction-policy component nominates as the next victim for this pool; the memory tier makes
/// no choice of its own."
/// `result.1` is the policy's answer to `identify_next_to_evict(pool_id)`; `result.0` what
/// evict_next returns. Phase 2 (`ev_commit`) is proved for ANY key phase 1 produced.
#[requires(mt_inv(*st))]
#[ensures(result.0 == result.1)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> !slot_at((^st).slots@, k) && !tracks((^st).policy.tracked@, k))]
#[ensures(match result.1 { Some(k) => st.policy.tracked@.len() > 0 && k == (st.policy.tracked@[0]).0, None => true })]
#[ensures(st.initialized && st.policy.tracked@.len() > 0 ==> result.1 != None)]
pub fn verify_mt_evict_next_post_policy_victim(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Option<Key>) {
    snapshot! { lem_mt_pool(*st) };
    let nominee = ev_identify(st); // ep.identify_next_to_evict(state.pool_id), lib.rs:445
    let r = match nominee {
        None => None,
        Some(k) => ev_commit(st, k, tel), // lib.rs:447-466: Some(key)
    };
    (r, nominee)
}

/// Anti-vacuity twin: MUST FAIL (claims eviction returns something other than the nominee).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 0)]
#[ensures(result.1 != None)]
#[ensures(result.0 != result.1)]
pub fn verify_mt_evict_next_post_policy_victim__mutant(st: &mut StateModel, tel: &mut TelemetryModel)
    -> (Option<Key>, Option<Key>) {
    snapshot! { lem_mt_pool(*st) };
    let nominee = ev_identify(st);
    let r = match nominee {
        None => None,
        Some(k) => ev_commit(st, k, tel),
    };
    (r, nominee)
}

/// **MT-EVICT-FOR-KEY-POST-ALIAS** (postcondition)
///
/// "Freeing space on behalf of a named key behaves exactly like a plain eviction for every
/// possible key argument: it removes the same victim and leaves the cache in the same state
/// as a plain eviction would."
/// Two identical states; `a` gets `evict_next_for_key(key)` for an ARBITRARY key, `b` gets
/// `evict_next()`. Same victim; same entries, same bytes in use, same capacity, same policy
/// membership afterwards.
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(same_obs(*a, *b))]
#[ensures(result.0 == result.1)]
#[ensures(forall<x: Key> slot_at((^a).slots@, x) == slot_at((^b).slots@, x))]
#[ensures(forall<x: Key> tracks((^a).policy.tracked@, x) == tracks((^b).policy.tracked@, x))]
#[ensures((^a).allocator.used == (^b).allocator.used)]
#[ensures((^a).allocator.capacity == (^b).allocator.capacity && (^a).initialized == (^b).initialized
          && (^a).pool_base == (^b).pool_base && (^a).pool_size == (^b).pool_size)]
#[ensures(result.0 == None ==> same_obs(^a, ^b))]
pub fn verify_mt_evict_for_key_post_alias(a: &mut StateModel, b: &mut StateModel, key: Key)
    -> (Option<Key>, Option<Key>) {
    proof_assert!(a.policy.tracked@.len() > 0 ==> tracks(a.policy.tracked@, (a.policy.tracked@[0]).0));
    proof_assert!(a.policy.tracked@.len() > 0 ==> a.slots@.len() > 0 && b.slots@.len() > 0);
    let ra = mt_evict_next_for_key(a, key);
    let rb = mt_evict_next(b);
    proof_assert!(ra == rb);
    proof_assert!(forall<x: Key> slot_at(a.slots@, x) == slot_at(b.slots@, x));
    proof_assert!(forall<x: Key> tracks(a.policy.tracked@, x) == slot_at(a.slots@, x));
    proof_assert!(forall<x: Key> tracks(b.policy.tracked@, x) == slot_at(b.slots@, x));
    (ra, rb)
}

/// Anti-vacuity twin: MUST FAIL (claims the named key changes the victim).
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(same_obs(*a, *b))]
#[requires(a.initialized && a.policy.tracked@.len() > 0)]
#[ensures(result.0 != None)]
#[ensures(result.0 != result.1)]
pub fn verify_mt_evict_for_key_post_alias__mutant(a: &mut StateModel, b: &mut StateModel, key: Key)
    -> (Option<Key>, Option<Key>) {
    proof_assert!(tracks(a.policy.tracked@, (a.policy.tracked@[0]).0));
    proof_assert!(a.slots@.len() > 0 && b.slots@.len() > 0);
    let ra = mt_evict_next_for_key(a, key);
    let rb = mt_evict_next(b);
    (ra, rb)
}

/// **MT-EVICT-FOR-KEY-POST-SPACE-GLOBAL** (postcondition, spec-only)
///
/// "The space released by this operation is usable by any subsequent insertion, not only by
/// an insertion of the key that was named, because the pool is one undivided region of memory."
/// Evict on behalf of `named`; then insert an UNRELATED key `k2` (any key absent before,
/// `k2 != named` allowed) of any size that fits the victim's rounded run: it succeeds.
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 0)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[requires(!slot_at(st.slots@, k2))]
#[requires(sz@ > 0)]
#[requires(forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == (st.policy.tracked@[0]).0
           ==> align_up_l(sz@) <= align_up_l(st.slots@[i].size@))]
#[ensures(result.0 == Some((st.policy.tracked@[0]).0))]
#[ensures(match result.1 { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_evict_for_key_post_space_global(st: &mut StateModel, named: Key, k2: Key, sz: u32)
    -> (Option<Key>, Result<usize, MtError>) {
    proof_assert!(tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.slots@.len() > 0);
    let s0 = snapshot! { st.slots@ };
    let v = mt_evict_next_for_key(st, named);
    proof_assert!(!slot_at(st.slots@, k2));
    proof_assert!(forall<i: Int, j: Int> 0 <= i && i < s0.len() && 0 <= j && j < s0.len()
                  && s0[i].key == s0[j].key ==> i == j);
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(sz@)));
    let r = mt_insert(st, k2, sz);
    (v, r)
}

/// Anti-vacuity twin: MUST FAIL (claims the freed run serves only the named key).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 0)]
#[requires(st.policy.next_handle@ + 1 < u64::MAX@)]
#[requires(!slot_at(st.slots@, k2) && k2 != named)]
#[requires(sz@ > 0)]
#[requires(forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == (st.policy.tracked@[0]).0
           ==> align_up_l(sz@) <= align_up_l(st.slots@[i].size@))]
#[ensures(result.0 == Some((st.policy.tracked@[0]).0))]
#[ensures(match result.1 { Ok(_) => false, Err(_) => true })]
pub fn verify_mt_evict_for_key_post_space_global__mutant(st: &mut StateModel, named: Key, k2: Key, sz: u32)
    -> (Option<Key>, Result<usize, MtError>) {
    proof_assert!(tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    proof_assert!(st.slots@.len() > 0);
    let v = mt_evict_next_for_key(st, named);
    let r = mt_insert(st, k2, sz);
    (v, r)
}

/// **MT-INV-DEFAULT-POOL-SIZE-NEVER-APPLIED** (invariant, spec-only, divergent)
///
/// "The component never silently uses its published default pool size of 256 mebibytes; the
/// size of a pool is always exactly the size the caller passed when setting it up, because
/// every set-up call requires an explicit size."
/// From the `Default` state: capacity 0 (not 256 MiB); a zero request is refused (no fallback
/// to the constant); after a successful set-up with `psz` and ANY later calls, capacity and
/// pool size are exactly `psz` -- 256 MiB only if the caller asked for 256 MiB.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0@ == 0 && result.0@ != DEFAULT_POOL_SIZE@)]
#[ensures(psz@ == 0 ==> result.1 == Err(MtError::InvalidSize) && result.2@ == 0)]
#[ensures(match result.1 { Ok(_) => result.2@ == psz@ && result.3@ == psz@, Err(_) => true })]
#[ensures(match result.1 { Ok(_) => (result.2@ == DEFAULT_POOL_SIZE@) == (psz@ == DEFAULT_POOL_SIZE@), Err(_) => true })]
pub fn verify_mt_inv_default_pool_size_never_applied(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (usize, Result<(), MtError>, usize, usize) {
    let mut st = mt_default(p);
    let c0 = mt_capacity(&st);
    let r = mt_initialize(&mut st, psz);
    let c_init = mt_capacity(&st);
    if psz == 0 {
        return (c0, r, c_init, st.pool_size);
    }
    let ok = match r { Ok(_) => true, Err(_) => false };
    proof_assert!(ok ==> st.initialized && st.allocator.capacity@ == psz@ && st.pool_size@ == psz@);
    mt_run(&mut st, ops, psz);
    proof_assert!(ok ==> st.initialized && st.allocator.capacity@ == psz@ && st.pool_size@ == psz@);
    let c = mt_capacity(&st);
    proof_assert!(ok ==> c@ == psz@);
    (c0, r, c, st.pool_size)
}

/// Anti-vacuity twin: MUST FAIL (claims the published default is what gets applied).
#[requires(p.tracked@.len() == 0)]
#[requires(p.pools@ + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0@ == 0)]
#[ensures(match result.1 { Ok(_) => result.2@ == DEFAULT_POOL_SIZE@, Err(_) => true })]
pub fn verify_mt_inv_default_pool_size_never_applied__mutant(p: PolicyModel, psz: usize)
    -> (usize, Result<(), MtError>, usize) {
    let mut st = mt_default(p);
    let c0 = mt_capacity(&st);
    let r = mt_initialize(&mut st, psz);
    let c = mt_capacity(&st);
    (c0, r, c)
}

/// **MT-INIT-POST-HUGEPAGE-FALLBACK** (postcondition)
///
/// "Setting up the pool prefers large memory pages but still succeeds with the full requested
/// capacity available when large pages cannot be obtained, because the allocator
/// transparently retries with an ordinary anonymous mapping."
/// OS outcomes are inputs: huge-page mmap FAILED, ordinary mmap succeeded => Ok, base = the
/// ordinary mapping, capacity = requested, all of it free. Huge pages available => that
/// mapping is used and the ordinary mmap is never issued (preference). Any NUMA request and
/// any mbind outcome.
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(r_huge == MAP_FAILED && r_plain != MAP_FAILED ==> result.0 == Ok(())
          && (^st).pool_base == r_plain && (^ml).plain_calls@ == ml.plain_calls@ + 1)]
#[ensures(r_huge != MAP_FAILED ==> result.0 == Ok(()) && (^st).pool_base == r_huge
          && (^ml).plain_calls == ml.plain_calls)]
#[ensures(r_huge == MAP_FAILED && r_plain != MAP_FAILED ==> result.1@ == psz@
          && (^st).allocator.used@ == 0 && free_sum((^st).allocator.regions@) == psz@)]
pub fn verify_mt_init_post_hugepage_fallback(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> (Result<(), MtError>, usize) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_mmap(st, psz, numa, r_huge, r_plain, rc, ml, &mut lg, false);
    let c = mt_capacity(st);
    proof_assert!(r == Ok(()) ==> st.allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: psz }));
    proof_assert!(r == Ok(()) ==> free_sum(st.allocator.regions@) == psz@);
    (r, c)
}

/// Anti-vacuity twin: MUST FAIL (claims set-up fails when only huge pages are unavailable).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(r_huge != MAP_FAILED ==> result == Ok(()))]
#[ensures(r_huge == MAP_FAILED ==> result == Err(MtError::AllocationFailed))]
pub fn verify_mt_init_post_hugepage_fallback__mutant(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    mt_initialize_mmap(st, psz, numa, r_huge, r_plain, rc, ml, &mut lg, false)
}

/// **MT-INIT-POST-POOLINFO** (postcondition)
///
/// "After the pool is set up successfully the query that reports the pool's base address and
/// total size returns a real, non-null address together with exactly the size that was
/// requested, so an outside party such as a GPU driver can register the whole region."
/// On the mmap path the address is the one mmap returned (non-null under `mmap_ok`).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1 == Some((mmap_base(r_huge, r_plain), psz))
          && mmap_base(r_huge, r_plain)@ != 0 && mmap_base(r_huge, r_plain) != MAP_FAILED,
          Err(_) => result.1 == None })]
#[ensures(match result.0 { Ok(_) => result.2@ == psz@, Err(_) => true })]
pub fn verify_mt_init_post_poolinfo(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> (Result<(), MtError>, Option<(usize, usize)>, usize) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_mmap(st, psz, numa, r_huge, r_plain, rc, ml, &mut lg, false);
    let pi = mt_pool_info(st);
    let c = mt_capacity(st);
    (r, pi, c)
}

/// Anti-vacuity twin: MUST FAIL (claims the reported size is the size minus one page).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1 != None, Err(_) => result.1 == None })]
#[ensures(forall<b: usize, s: usize> result.1 == Some((b, s)) ==> s@ < psz@)]
pub fn verify_mt_init_post_poolinfo__mutant(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> (Result<(), MtError>, Option<(usize, usize)>) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_mmap(st, psz, numa, r_huge, r_plain, rc, ml, &mut lg, false);
    let pi = mt_pool_info(st);
    (r, pi)
}

/// **MT-PEEK-POST-HIT** (postcondition)
///
/// "The non-recency-updating lookup of a key that is present returns exactly the same address
/// and the same size that the recency-updating lookup would return for that key."
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
#[ensures(result.0 != None)]
#[ensures(result.0 == result.1)]
pub fn verify_mt_peek_post_hit(st: &mut StateModel, key: Key) -> (Option<(usize, u32)>, Option<(usize, u32)>) {
    let s0 = snapshot! { st.slots@ };
    let p = mt_peek(st, key);
    let g = mt_get(st, key);
    proof_assert!(forall<i: Int, j: Int> 0 <= i && i < s0.len() && 0 <= j && j < s0.len()
                  && s0[i].key == s0[j].key ==> i == j);
    (p, g)
}

/// Anti-vacuity twin: MUST FAIL (claims peek reports a different size than get).
#[requires(mt_inv(*st))]
#[requires(st.initialized && slot_at(st.slots@, key))]
#[ensures(result.0 != None)]
#[ensures(forall<p1: usize, s1: u32, p2: usize, s2: u32> result.0 == Some((p1, s1)) && result.1 == Some((p2, s2))
          ==> s1 != s2)]
pub fn verify_mt_peek_post_hit__mutant(st: &mut StateModel, key: Key) -> (Option<(usize, u32)>, Option<(usize, u32)>) {
    let p = mt_peek(st, key);
    let g = mt_get(st, key);
    (p, g)
}

/// **MT-POOL-INFO-POST** (postcondition)
///
/// "Once the pool is set up the query reports the starting address of the single block of
/// memory that backs the whole cache together with its total size in bytes, and that size
/// equals the pool's reported capacity, which is what a caller needs in order to register the
/// region with a device."
/// Any set-up state (reachable states satisfy `mt_inv`): the base and size reported; size ==
/// capacity(); every entry's block lies inside [base, base + size).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(result.0 == Some((st.pool_base, st.pool_size)))]
#[ensures(st.pool_base@ != 0)]
#[ensures(st.pool_size == result.1)]
#[ensures(forall<i: Int> 0 <= i && i < st.slots@.len()
          ==> st.pool_base@ <= st.pool_base@ + st.slots@[i].offset@
              && st.pool_base@ + st.slots@[i].offset@ + st.slots@[i].size@ <= st.pool_base@ + result.1@)]
pub fn verify_mt_pool_info_post(st: &StateModel) -> (Option<(usize, usize)>, usize) {
    let pi = mt_pool_info(st);
    let c = mt_capacity(st);
    (pi, c)
}

/// Anti-vacuity twin: MUST FAIL (claims the reported size differs from capacity()).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(result.0 != None)]
#[ensures(forall<b: usize, s: usize> result.0 == Some((b, s)) ==> s != result.1)]
pub fn verify_mt_pool_info_post__mutant(st: &StateModel) -> (Option<(usize, usize)>, usize) {
    let pi = mt_pool_info(st);
    let c = mt_capacity(st);
    (pi, c)
}

/// **MT-RESET-TELEMETRY-POST** -- REFUTATION (postcondition, divergent)
///
/// Statement: "After the telemetry counters are explicitly reset, a fresh snapshot reports
/// zero for every one of the three counters, while the cache contents, the capacity and the
/// bytes in use are all left untouched; this is the only way any counter ever decreases."
/// Negation proved: a counter DECREASES with no reset at all -- `fetch_add(1)` wraps at
/// `u64::MAX` (std: "wraps around on overflow"). The reset postcondition and its frame HOLD
/// (`witness_mt_reset_telemetry_post`); only the trailing "only way" clause is false (same
/// root cause as the batch-2 refutation of MT-TELEMETRY-COUNTERS-MONOTONIC).
#[requires(mt_inv(*st))]
#[requires(t.evictions@ == u64::MAX@)]
#[ensures(result.1@ < result.0@)]
#[ensures(result.2 == result.3)]
pub fn refute_mt_reset_telemetry_post(st: &mut StateModel, t: &mut TelemetryModel) -> (u64, u64, usize, usize) {
    let c0 = mt_capacity(st);
    let (e0, _w0, _r0) = tel_snapshot(t);
    tel_event(t, 0); // one more eviction (lib.rs:463-464) -- reset_telemetry is never called
    let (e1, _w1, _r1) = tel_snapshot(t);
    let c1 = mt_capacity(st);
    (e0, e1, c0, c1)
}

/// Anti-vacuity twin: MUST FAIL (asserts the unreset counter did not decrease).
#[requires(mt_inv(*st))]
#[requires(t.evictions@ == u64::MAX@)]
#[ensures(result.2 == result.3)]
#[ensures(result.1@ >= result.0@)]
pub fn refute_mt_reset_telemetry_post__mutant(st: &mut StateModel, t: &mut TelemetryModel) -> (u64, u64, usize, usize) {
    let c0 = mt_capacity(st);
    let (e0, _w0, _r0) = tel_snapshot(t);
    tel_event(t, 0);
    let (e1, _w1, _r1) = tel_snapshot(t);
    let c1 = mt_capacity(st);
    (e0, e1, c0, c1)
}

/// UNSCORED WITNESS for MT-RESET-TELEMETRY-POST: the reset postcondition and its frame.
/// `reset_telemetry` (lib.rs:174-176) takes `state.read()` and touches only the atomics.
#[requires(mt_inv(*st))]
#[ensures(result.0 .0@ == 0 && result.0 .1@ == 0 && result.0 .2@ == 0)]
#[ensures(result.1 == result.2 && result.3 == result.4)]
#[ensures(^st == *st)]
pub fn witness_mt_reset_telemetry_post(st: &mut StateModel, t: &mut TelemetryModel)
    -> ((u64, u64, u64), usize, usize, usize, usize) {
    let c0 = mt_capacity(st);
    let u0 = mt_used(st);
    tel_reset(t);
    let s = tel_snapshot(t);
    let c1 = mt_capacity(st);
    let u1 = mt_used(st);
    (s, c0, c1, u0, u1)
}

/// UNSCORED WITNESS for the divergence of MT-RESET-TELEMETRY-POST: the reset is three
/// separate stores under a SHARED lock (lib.rs:58-62, :174), so a concurrent snapshot
/// (also under the shared lock) can observe a half-reset triple.
#[requires(t.write_lock_contentions@ > 0)]
#[ensures(result.0@ == 0 && result.1@ > 0)]
#[ensures(trip(^t) == (0u64, 0u64, 0u64))]
pub fn witness_mt_reset_telemetry_torn(t: &mut TelemetryModel) -> (u64, u64, u64) {
    t.evictions = 0; // reset store 1 (lib.rs:59)
    let s = (t.evictions, t.write_lock_contentions, t.read_lock_contentions); // concurrent snapshot
    t.write_lock_contentions = 0; // reset store 2 (lib.rs:60)
    t.read_lock_contentions = 0; // reset store 3 (lib.rs:61)
    s
}

/// **MT-TELEMETRY-INHERENT-POST** (postcondition, code-only) -- HAZARD
///
/// "The inherent telemetry accessor returns the same three counter values as the
/// interface-level snapshot, but reads them one at a time rather than as one atomic group, so
/// the three numbers it returns need not correspond to any single instant."
/// (a) quiescent: `telemetry()` == `telemetry_snapshot()`. (b) CONFIRMED: `telemetry()` holds
/// only `state.read()` (lib.rs:168); an eviction's counter bump (lib.rs:463-464) and an
/// insert's write-contention bump (lib.rs:345-349) run under `state.read()` too, so both can
/// land between its first and second load. The returned triple equals NONE of the three
/// states the counters passed through during the read. (Each atomic op is one interleaving
/// step; a sequentially-consistent interleaving is one of the behaviours `Relaxed` permits.)
#[requires(t.evictions@ + 1 < u64::MAX@ && t.write_lock_contentions@ + 1 < u64::MAX@)]
#[ensures(result.0 == result.1)]
#[ensures(result.0 == trip(*t))]
#[ensures(result.2 != trip(*t))]
#[ensures(result.2 != *result.3)]
#[ensures(result.2 != trip(^t))]
pub fn verify_mt_telemetry_inherent_post(t: &mut TelemetryModel)
    -> ((u64, u64, u64), (u64, u64, u64), (u64, u64, u64), Snapshot<(u64, u64, u64)>) {
    let a = mt_telemetry_inherent(t); // telemetry()
    let b = tel_snapshot(t); // telemetry_snapshot()
    // telemetry() racing two writers:
    let e = t.evictions; // load 1 (lib.rs:52)
    tel_event(t, 0); // writer 1: evict_next's evictions.fetch_add(1)
    let mid = snapshot! { trip(*t) };
    tel_event(t, 1); // writer 2: insert's write_lock_contentions.fetch_add(1)
    let w = t.write_lock_contentions; // load 2 (lib.rs:53)
    let r = t.read_lock_contentions; // load 3 (lib.rs:54)
    (a, b, (e, w, r), mid)
}

/// Anti-vacuity twin: MUST FAIL (claims the racing read returns the starting instant).
#[requires(t.evictions@ + 1 < u64::MAX@ && t.write_lock_contentions@ + 1 < u64::MAX@)]
#[ensures((^t).evictions@ == t.evictions@ + 1)]
#[ensures(result.1 == trip(*t))]
pub fn verify_mt_telemetry_inherent_post__mutant(t: &mut TelemetryModel) -> ((u64, u64, u64), (u64, u64, u64)) {
    let a = mt_telemetry_inherent(t);
    let e = t.evictions;
    tel_event(t, 0);
    tel_event(t, 1);
    let w = t.write_lock_contentions;
    let r = t.read_lock_contentions;
    (a, (e, w, r))
}

/// **MT-DROP-DOES-NOT-UNTRACK** (error-case, code-only) -- HAZARD
///
/// "Destroying the component releases its memory but never tells the eviction policy that the
/// pool is gone, so the policy keeps the pool identifier and every entry it was tracking, and
/// a policy shared with other components accumulates candidates that can never be served."
/// CONFIRMED, by code structure: `MemoryTierState` has no field referring to the policy, Drop
/// makes no `ep` call. After the destructor (any build, any SPDK-env state) the policy holds
/// every tracked entry and its pool counter, and its next nomination for the destroyed pool's
/// id is a key of the destroyed tier.
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.slots@.len() > 0)]
#[requires(rl.munmaps@ < u64::MAX@ && rl.spdk_frees@ < u64::MAX@)]
#[ensures((^st).pool_base@ == 0)]
#[ensures(forall<k: Key> slot_at(st.slots@, k) ==> tracks(*result.1, k))]
#[ensures(match result.0 { Some(k) => slot_at(st.slots@, k), None => false })]
pub fn verify_mt_drop_does_not_untrack(st: &mut StateModel, spdk_build: bool, env_active: bool, rl: &mut ReleaseLog)
    -> (Option<Key>, Snapshot<Seq<(Key, u64)>>) {
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    mt_drop(st, spdk_build, env_active, rl);
    let after = snapshot! { st.policy.tracked@ };
    proof_assert!(st.policy.tracked@.len() > 0);
    proof_assert!(tracks(st.policy.tracked@, (st.policy.tracked@[0]).0));
    let pid = st.pool_id;
    let v = policy_identify_next_to_evict(&mut st.policy, pid); // the shared policy, later
    (v, after)
}

/// Anti-vacuity twin: MUST FAIL (claims Drop untracked the entries).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.slots@.len() > 0)]
#[requires(rl.munmaps@ < u64::MAX@ && rl.spdk_frees@ < u64::MAX@)]
#[ensures((^st).pool_base@ == 0)]
#[ensures((^st).policy.tracked@.len() == 0)]
pub fn verify_mt_drop_does_not_untrack__mutant(st: &mut StateModel, spdk_build: bool, env_active: bool, rl: &mut ReleaseLog) {
    proof_assert!(slot_at(st.slots@, st.slots@[0].key));
    mt_drop(st, spdk_build, env_active, rl);
}

/// **MT-DROP-POST-UNMAP** (postcondition, code-only, divergent)
///
/// "When the component is destroyed, a pool that came from the ordinary memory-mapping path is
/// handed back to the operating system and the stored base address is set to null, so the
/// region is released exactly once."
/// Set up on the mmap path, ANY later calls, destroy (any build / env state): exactly one
/// `munmap` is issued, with exactly the address mmap returned and the length mmap was asked
/// for; no `spdk_free`; the stored base is null, so a repeated destructor issues nothing.
#[requires(mt_inv(*st))]
#[requires(!st.initialized && st.pool_base@ == 0 && !st.poisoned)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(rl.munmaps@ + 2 < u64::MAX@ && rl.spdk_frees@ + 2 < u64::MAX@)]
#[ensures(match result { Ok(_) => (^rl).munmaps@ == rl.munmaps@ + 1 && (^rl).spdk_frees == rl.spdk_frees
          && (^rl).last_base == mmap_base(r_huge, r_plain) && (^rl).last_len == psz,
          Err(_) => ^rl == *rl })]
#[ensures((^st).pool_base@ == 0)]
pub fn verify_mt_drop_post_unmap(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64,
    ops: &[(u8, Key, u32)], spdk_build: bool, env_active: bool, ml: &mut MmapLog, rl: &mut ReleaseLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_mmap(st, psz, numa, r_huge, r_plain, rc, ml, &mut lg, false);
    match r {
        Ok(_) => mt_run(st, ops, psz),
        Err(_) => {}
    }
    mt_drop(st, spdk_build, env_active, rl); // the destructor
    mt_drop(st, spdk_build, env_active, rl); // a repeated release attempt: issues nothing
    r
}

/// Anti-vacuity twin: MUST FAIL (claims the region is unmapped twice).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && st.pool_base@ == 0)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(rl.munmaps@ + 2 < u64::MAX@ && rl.spdk_frees@ + 2 < u64::MAX@)]
#[ensures((^st).pool_base@ == 0)]
#[ensures(match result { Ok(_) => (^rl).munmaps@ == rl.munmaps@ + 2, Err(_) => true })]
pub fn verify_mt_drop_post_unmap__mutant(
    st: &mut StateModel, psz: usize, r_huge: usize, r_plain: usize, ml: &mut MmapLog, rl: &mut ReleaseLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_mmap(st, psz, None, r_huge, r_plain, 0, ml, &mut lg, false);
    mt_drop(st, false, false, rl);
    mt_drop(st, false, false, rl);
    r
}

/// **MT-DROP-SPDK-LEAK** (error-case, code-only) -- HAZARD
///
/// "For a pool obtained from the storage runtime's allocator the destructor releases it only
/// while that runtime is still active; if the runtime has already been shut down the
/// destructor nulls the stored address and returns without freeing anything, so the memory is
/// leaked for the remaining life of the process."
/// MECHANISM CONFIRMED (code): set up from `spdk_zmalloc` while the env is active, any later
/// calls, env shut down (`is_spdk_env_active()` false), destroy: no `spdk_free`, no `munmap`
/// is issued, the stored base -- memory-tier's only record of the region -- is nulled, and a
/// repeated destructor issues nothing either. Whether the bytes then stay unreclaimed for the
/// rest of the process is SPDK/DPDK env-teardown behaviour, NOT modelled (see advisory note).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && st.pool_base@ == 0 && !st.poisoned)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(r_spdk@ != 0 && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(rl.munmaps@ + 2 < u64::MAX@ && rl.spdk_frees@ + 2 < u64::MAX@)]
#[ensures(result.0 == Ok(()))]
#[ensures(result.1)]
#[ensures(^rl == *rl)]
#[ensures((^st).pool_base@ == 0)]
pub fn verify_mt_drop_spdk_leak(
    st: &mut StateModel, psz: usize, numa: Option<i32>, r_spdk: usize, ops: &[(u8, Key, u32)],
    ml: &mut MmapLog, rl: &mut ReleaseLog,
) -> (Result<(), MtError>, bool) {
    let mut lg = LogSink { lines: 0 };
    // set-up while the SPDK env is active: spdk_zmalloc succeeds
    let mut zl = ZmLog { calls: 0, numa_id: 0, size: 0 };
    let r = mt_initialize_spdk(st, psz, numa, true, r_spdk, MAP_FAILED, MAP_FAILED, 0, ml, &mut zl, &mut lg, false);
    mt_run(st, ops, psz);
    let dma = st.spdk_allocated;
    // spdk-env do_fini: set_spdk_env_active(false), spdk_env_fini() -- then the component drops
    mt_drop(st, true, false, rl);
    mt_drop(st, true, false, rl);
    (r, dma)
}

/// Anti-vacuity twin: MUST FAIL (claims the SPDK pool is freed after the env is gone).
#[requires(mt_inv(*st))]
#[requires(!st.initialized && st.pool_base@ == 0)]
#[requires(st.policy.pools@ + 2 < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(r_spdk@ != 0 && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(rl.munmaps@ + 2 < u64::MAX@ && rl.spdk_frees@ + 2 < u64::MAX@)]
#[ensures(result == Ok(()))]
#[ensures((^rl).spdk_frees@ == rl.spdk_frees@ + 1)]
pub fn verify_mt_drop_spdk_leak__mutant(
    st: &mut StateModel, psz: usize, r_spdk: usize, ml: &mut MmapLog, rl: &mut ReleaseLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    let mut zl = ZmLog { calls: 0, numa_id: 0, size: 0 };
    let r = mt_initialize_spdk(st, psz, None, true, r_spdk, MAP_FAILED, MAP_FAILED, 0, ml, &mut zl, &mut lg, false);
    mt_drop(st, true, false, rl);
    r
}

/// **MT-ALLOC-MMAP-ERR-BOTH-FAIL** (error-case, code-only)
///
/// "Only when both the huge-page mapping and the ordinary mapping fail does the fallback
/// allocator report an allocation failure, and it never returns a null or failed mapping
/// address to its caller."
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[ensures(match result { Err(_) => r_huge == MAP_FAILED && r_plain == MAP_FAILED, Ok(_) => true })]
#[ensures(r_huge == MAP_FAILED && r_plain == MAP_FAILED ==> result == Err(MtError::AllocationFailed))]
#[ensures(forall<p: usize> result == Ok(p) ==> p != MAP_FAILED && p@ != 0
          && (p == r_huge || (r_huge == MAP_FAILED && p == r_plain)))]
pub fn verify_mt_alloc_mmap_err_both_fail(
    psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, numa, r_huge, r_plain, rc, ml, &mut lg, false)
}

/// Anti-vacuity twin: MUST FAIL (claims a huge-page failure alone is an allocation failure).
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[ensures(r_huge != MAP_FAILED ==> result == Ok(r_huge))]
#[ensures(r_huge == MAP_FAILED ==> result == Err(MtError::AllocationFailed))]
pub fn verify_mt_alloc_mmap_err_both_fail__mutant(
    psz: usize, r_huge: usize, r_plain: usize, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, None, r_huge, r_plain, 0, ml, &mut lg, false)
}

/// **MT-ALLOC-MMAP-MAXNODE-ARITHMETIC** (invariant, code-only)
///
/// "The node-count argument handed to the kernel's memory-binding call is computed as the
/// requested node number plus two using unchecked arithmetic on a value converted from a signed
/// integer, so it is correct only because the conversion is guarded by a non-negativity check
/// and the node number is far from the largest representable value."
/// The `node as usize` / `node_id + 2` of `alloc_mmap_m` carry Creusot's overflow VCs, which
/// prove; the argument is exactly node + 2 for EVERY non-negative i32 (up to i32::MAX); a
/// negative node never reaches the conversion.
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(r_huge != MAP_FAILED || r_plain != MAP_FAILED)]
#[ensures(forall<n: i32> numa == Some(n) && n@ >= 0 ==> (^ml).mbind_issued
          && (^ml).mbind_maxnode@ == n@ + 2 && (^ml).mbind_node@ == n@
          && (^ml).mbind_maxnode@ <= i32::MAX@ + 2)]
#[ensures(forall<n: i32> numa == Some(n) && n@ < 0 ==> (^ml).mbind_issued == ml.mbind_issued
          && (^ml).mbind_maxnode == ml.mbind_maxnode)]
pub fn verify_mt_alloc_mmap_maxnode_arithmetic(
    psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, numa, r_huge, r_plain, rc, ml, &mut lg, false)
}

/// Anti-vacuity twin: MUST FAIL (claims the kernel gets the node number plus one).
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(r_huge != MAP_FAILED)]
#[ensures(result == Ok(r_huge))]
#[ensures(forall<n: i32> numa == Some(n) && n@ >= 0 ==> (^ml).mbind_maxnode@ == n@ + 1)]
pub fn verify_mt_alloc_mmap_maxnode_arithmetic__mutant(
    psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, numa, r_huge, r_plain, 0, ml, &mut lg, false)
}

/// **MT-ALLOC-MMAP-NODEMASK-GUARD** (error-case, code-only) -- HAZARD
///
/// "The bitmask naming the requested processor memory node is only filled in when the node
/// number is small enough to fit in one machine word, but when the node number is too large
/// the code does not stop: it goes on to ask the kernel to bind the pool using an all-zero
/// mask, which cannot name the requested node."
/// CONFIRMED (code): for every node >= 64 the mbind call is still issued, with mask 0 and
/// maxnode = node + 2; the allocation still succeeds. Also: the maxnode handed over then
/// exceeds the one-word mask buffer (maxnode - 1 > 64 bits; see note on what the kernel reads).
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(r_huge != MAP_FAILED || r_plain != MAP_FAILED)]
#[requires(node@ >= C_ULONG_BITS@)]
#[ensures(result == Ok(mmap_base(r_huge, r_plain)))]
#[ensures((^ml).mbind_issued)]
#[ensures((^ml).mbind_mask@ == 0)]
#[ensures((^ml).mbind_node@ == node@ && (^ml).mbind_maxnode@ == node@ + 2)]
#[ensures((^ml).mbind_maxnode@ - 1 > C_ULONG_BITS@)]
pub fn verify_mt_alloc_mmap_nodemask_guard(
    psz: usize, node: i32, r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, Some(node), r_huge, r_plain, rc, ml, &mut lg, false)
}

/// Anti-vacuity twin: MUST FAIL (claims the too-large node stops before the mbind call).
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(!ml.mbind_issued)]
#[requires(r_huge != MAP_FAILED)]
#[requires(node@ >= C_ULONG_BITS@)]
#[ensures(result == Ok(r_huge))]
#[ensures(!(^ml).mbind_issued)]
pub fn verify_mt_alloc_mmap_nodemask_guard__mutant(
    psz: usize, node: i32, r_huge: usize, r_plain: usize, ml: &mut MmapLog,
) -> Result<usize, MtError> {
    let mut lg = LogSink { lines: 0 };
    alloc_mmap_m(psz, Some(node), r_huge, r_plain, 0, ml, &mut lg, false)
}

/// Membership in `filter_l`: exactly the listed keys that have a slot.
#[logic]
#[variant(ks.len())]
#[ensures(forall<k: Key> (exists<i: Int> 0 <= i && i < filter_l(ks, s).len() && filter_l(ks, s)[i] == k)
          == (slot_at(s, k) && exists<j: Int> 0 <= j && j < ks.len() && ks[j] == k))]
pub fn lem_filter_mem(ks: Seq<Key>, s: Seq<SlotModel>) {
    pearlite! {
        if ks.len() <= 0 { () } else {
            let p = ks.subsequence(0, ks.len() - 1);
            lem_filter_mem(p, s);
            proof_assert!(forall<j: Int> 0 <= j && j < p.len() ==> p[j] == ks[j]);
            proof_assert!(forall<k: Key> (exists<j: Int> 0 <= j && j < ks.len() && ks[j] == k)
                          == ((exists<j: Int> 0 <= j && j < p.len() && p[j] == k) || ks[ks.len() - 1] == k));
            let x = ks[ks.len() - 1];
            let f = filter_l(p, s);
            if slot_at(s, x) {
                proof_assert!(filter_l(ks, s) == f.push_back(x));
                proof_assert!(forall<i: Int> 0 <= i && i < f.len() ==> f.push_back(x)[i] == f[i]);
                proof_assert!(f.push_back(x)[f.len()] == x);
                proof_assert!(forall<k: Key> (exists<i: Int> 0 <= i && i < f.push_back(x).len() && f.push_back(x)[i] == k)
                              == ((exists<i: Int> 0 <= i && i < f.len() && f[i] == k) || x == k));
                ()
            } else {
                proof_assert!(filter_l(ks, s) == f);
                ()
            }
        }
    }
}

/// `filter_l` is idempotent: filtering an already-filtered batch changes nothing.
#[logic]
#[variant(ks.len())]
#[ensures(filter_l(filter_l(ks, s), s) == filter_l(ks, s))]
pub fn lem_filter_idem(ks: Seq<Key>, s: Seq<SlotModel>) {
    pearlite! {
        if ks.len() <= 0 { () } else {
            let p = ks.subsequence(0, ks.len() - 1);
            let x = ks[ks.len() - 1];
            lem_filter_idem(p, s);
            if slot_at(s, x) {
                proof_assert!(filter_l(ks, s) == filter_l(p, s).push_back(x));
                proof_assert!(filter_l(p, s).push_back(x).subsequence(0, filter_l(p, s).len()) == filter_l(p, s));
                ()
            } else {
                ()
            }
        }
    }
}

/// **MT-BATCH-TOUCH-EMPTY-NOOP** -- REFUTATION (error-case)
///
/// Statement: "Refreshing an empty list of keys, or refreshing before the pool has been set
/// up, returns immediately without taking any lock and without calling the eviction policy,
/// and has no observable effect of any kind on the cache or on the eviction order."
/// Negation proved: a non-empty batch before set-up DOES take a lock -- `self.state.read()`
/// (lib.rs:528) precedes the set-up check (lib.rs:529). The empty-batch half, "no policy
/// call" and "no observable effect" all HOLD (`witness_mt_batch_touch_empty_noop_intended`).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(keys@.len() > 0)]
#[ensures(^st == *st)]
#[ensures(result@.len() > 0 && result@[0] == LkEv::StateRead)]
pub fn refute_mt_batch_touch_empty_noop(st: &mut StateModel, keys: &[Key], connected: bool) -> Vec<LkEv> {
    mt_batch_touch(st, keys);
    lk_batch_touch(keys.len() > 0, st.initialized, connected)
}

/// Anti-vacuity twin: MUST FAIL (asserts the literal "no lock" reading).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[requires(keys@.len() > 0)]
#[ensures(^st == *st)]
#[ensures(result@.len() == 0)]
pub fn refute_mt_batch_touch_empty_noop__mutant(st: &mut StateModel, keys: &[Key], connected: bool) -> Vec<LkEv> {
    mt_batch_touch(st, keys);
    lk_batch_touch(keys.len() > 0, st.initialized, connected)
}

/// UNSCORED WITNESS for MT-BATCH-TOUCH-EMPTY-NOOP: what the statement intends. Empty batch:
/// no lock at all, nothing changes. Not set up: no POOL lock, no policy call, nothing changes.
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(!b.initialized)]
#[requires(empty@.len() == 0)]
#[ensures(^a == *a && ^b == *b)]
#[ensures(result.0@.len() == 0)]
#[ensures(no_pool_read(result.1@) && no_pool_write(result.1@))]
#[ensures(forall<i: Int> 0 <= i && i < result.1@.len()
          ==> result.1@[i] != LkEv::PolicyRefresh && result.1@[i] != LkEv::PolicyOther)]
pub fn witness_mt_batch_touch_empty_noop_intended(a: &mut StateModel, b: &mut StateModel, empty: &[Key], keys: &[Key], c: bool)
    -> (Vec<LkEv>, Vec<LkEv>) {
    mt_batch_touch(a, empty);
    let t0 = lk_batch_touch(false, a.initialized, c);
    mt_batch_touch(b, keys);
    let t1 = lk_batch_touch(keys.len() > 0, false, c);
    (t0, t1)
}

/// **MT-BATCH-TOUCH-PRE** (precondition, spec-only)
///
/// "Refreshing many entries at once may be requested with any list of keys, including an empty
/// list or keys that are not in the cache, and it never fails outright."
/// No requirement on `keys` (any length, any contents, absent keys, duplicates) beyond the
/// reachable-state invariant; the mirror is panic-free (Creusot proves absence of panics and
/// overflow) and returns `()` -- there is no error channel. SCOPE: unpoisoned locks.
#[requires(mt_inv(*st))]
#[ensures(mt_inv(^st))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(forall<k: Key> tracks((^st).policy.tracked@, k) == tracks(st.policy.tracked@, k))]
pub fn verify_mt_batch_touch_pre(st: &mut StateModel, keys: &[Key]) {
    mt_batch_touch(st, keys);
}

/// Anti-vacuity twin: MUST FAIL (claims a batch must leave the order untouched).
#[requires(mt_inv(*st))]
#[ensures(mt_inv(^st))]
#[ensures((^st).policy.tracked@ == st.policy.tracked@)]
pub fn verify_mt_batch_touch_pre__mutant(st: &mut StateModel, keys: &[Key]) {
    mt_batch_touch(st, keys);
}

/// **MT-BATCH-TOUCH-SKIPS-ABSENT** (error-case)
///
/// "Keys in the batch that are not in the cache are quietly skipped, no error is reported, the
/// keys in the same batch that are present are still refreshed, and the caller is never told
/// how many of its keys were skipped."
/// `a` refreshes the batch as given; `b` (identical) refreshes only its present keys
/// (`present == filter_l(keys, slots)`): the outcomes are identical, so the absent keys had no
/// effect; the refreshed keys are exactly the listed keys that are present; return type `()`.
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(same_state(*a, *b))]
#[requires(a.initialized)]
#[requires(present@ == filter_l(keys@, a.slots@))]
#[ensures(same_obs(^a, ^b))]
#[ensures((^a).policy.tracked@ == touch_all_l(a.policy.tracked@, present@))]
#[ensures(forall<k: Key> (exists<i: Int> 0 <= i && i < present@.len() && present@[i] == k)
          == (slot_at(a.slots@, k) && exists<j: Int> 0 <= j && j < keys@.len() && keys@[j] == k))]
pub fn verify_mt_batch_touch_skips_absent(a: &mut StateModel, b: &mut StateModel, keys: &[Key], present: &[Key]) {
    snapshot! { lem_filter_mem(keys@, a.slots@) };
    snapshot! { lem_filter_idem(keys@, a.slots@) };
    let a0 = snapshot! { *a };
    let b0 = snapshot! { *b };
    proof_assert!(filter_l(present@, b0.slots@) == present@);
    mt_batch_touch(a, keys);
    mt_batch_touch(b, present);
    proof_assert!(a.policy.tracked@ == touch_all_l(a0.policy.tracked@, present@));
    proof_assert!(b.policy.tracked@ == touch_all_l(b0.policy.tracked@, present@));
    proof_assert!(a.policy.tracked@ == b.policy.tracked@);
    proof_assert!(a.slots@ == b.slots@ && a.allocator.regions@ == b.allocator.regions@);
}

/// Anti-vacuity twin: MUST FAIL (claims an absent key in the batch changes the outcome).
#[requires(mt_inv(*a) && mt_inv(*b))]
#[requires(same_state(*a, *b))]
#[requires(a.initialized)]
#[requires(present@ == filter_l(keys@, a.slots@))]
#[ensures((^a).initialized)]
#[ensures((^a).policy.tracked@ != (^b).policy.tracked@)]
pub fn verify_mt_batch_touch_skips_absent__mutant(a: &mut StateModel, b: &mut StateModel, keys: &[Key], present: &[Key]) {
    snapshot! { lem_filter_idem(keys@, a.slots@) };
    mt_batch_touch(a, keys);
    mt_batch_touch(b, present);
}

/// **MT-CAPACITY-ERR-NOT-INITIALIZED** (error-case, divergent)
///
/// "Before the pool has been set up the total-capacity query reports zero, so a reported
/// capacity of zero always means there is no usable pool."
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result@ == 0)]
#[ensures(result@ == 0 ==> !st.initialized)]
pub fn verify_mt_capacity_err_not_initialized(st: &StateModel) -> usize {
    mt_capacity(st)
}

/// Anti-vacuity twin: MUST FAIL (claims zero can also come from a set-up pool).
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result@ == 0)]
#[ensures(st.initialized ==> result@ == 0)]
pub fn verify_mt_capacity_err_not_initialized__mutant(st: &StateModel) -> usize {
    mt_capacity(st)
}

/// **MT-CAPACITY-FRAME** (frame)
///
/// "Asking for the total capacity changes nothing about the cache or the eviction order."
/// The call takes the state by shared reference (lib.rs:567-574 reads under `state.read()` /
/// `pool.read()` and makes no `ep` call -- the same lock skeleton as peek/contains); the
/// complete component state, including the policy order, is unchanged, and the next eviction
/// still takes the same head.
#[requires(mt_inv(*st))]
#[ensures(^st == *st)]
#[ensures(result.0 == result.1)]
#[ensures(forall<i: Int> 0 <= i && i < result.2@.len()
          ==> result.2@[i] != LkEv::PolicyRefresh && result.2@[i] != LkEv::PolicyOther)]
#[ensures(no_pool_write(result.2@))]
pub fn verify_mt_capacity_frame(st: &mut StateModel) -> (usize, usize, Vec<LkEv>) {
    let c0 = mt_capacity(st);
    let c1 = mt_capacity(st);
    let t = lk_peek_or_contains(st.initialized);
    (c0, c1, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the query refreshes the eviction order).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 1)]
#[ensures(result@ == st.allocator.capacity@)]
#[ensures((^st).policy.tracked@ != st.policy.tracked@)]
pub fn verify_mt_capacity_frame__mutant(st: &mut StateModel) -> usize {
    mt_capacity(st)
}

/// **MT-CAPACITY-PRE** (precondition, spec-only)
///
/// "The total-capacity query may be called at any time, before or after the pool is set up,
/// and always returns a number rather than failing."
/// Before set-up (the `Default` state), right after a set-up attempt (successful or not), and
/// after ANY later call sequence: the query returns a number (0 when not set up, the pool size
/// otherwise); no precondition beyond the reachable-state invariant. SCOPE: unpoisoned locks.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0@ == 0)]
#[ensures(match result.1 { Ok(_) => result.2@ == psz@ && result.3@ == psz@, Err(_) => result.2@ == 0 })]
#[ensures(result.3@ == 0 || result.3@ == result.4@)]
pub fn verify_mt_capacity_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (usize, Result<(), MtError>, usize, usize, usize) {
    let mut st = mt_default(p);
    let c0 = mt_capacity(&st); // before set-up
    let r = mt_initialize(&mut st, psz);
    let c1 = mt_capacity(&st); // after the set-up attempt
    mt_run(&mut st, ops, psz);
    let c2 = mt_capacity(&st); // at any later point
    (c0, r, c1, c2, st.pool_size)
}

/// Anti-vacuity twin: MUST FAIL (claims the query before set-up reports a usable size).
#[requires(p.tracked@.len() == 0)]
#[ensures(result.1@ == 0)]
#[ensures(result.0@ > 0)]
pub fn verify_mt_capacity_pre__mutant(p: PolicyModel) -> (usize, usize) {
    let st = mt_default(p);
    let c0 = mt_capacity(&st);
    (c0, st.pool_size)
}

// ===========================================================================
// Batch 5 — supporting mirrors
// ===========================================================================

/// `IMemoryTier::initialize` (lib.rs:261-326, pin) IN FULL, `spdk` build: the size check
/// (:262), the receptacle lookup `self.eviction_policy.get().map_err(..)?` (:266-268), then
/// lock + flag check (:270-271) and the allocation (SPDK :277-300 / mmap :302) --
/// `mt_initialize_spdk`. A non-`spdk` build is the `env_active == false` instance.
/// OS/SPDK results (`r_huge`, `r_plain`, `rc`, `r_spdk`) are arbitrary INPUTS.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(pool_size@ > 0 ==> pool_size@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, pool_size) && mmap_ok(r_plain, pool_size) && zmalloc_ok(r_spdk, pool_size))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@)]
#[requires(zl.calls@ < u64::MAX@)]
#[ensures(mt_inv(^st))]
#[ensures(match result { Ok(_) => true, Err(_) => ^st == *st })]
#[ensures(pool_size@ == 0 ==> result == Err(MtError::InvalidSize) && ^ml == *ml && ^zl == *zl)]
#[ensures(!connected && pool_size@ > 0 ==> result == Err(MtError::NotInitialized) && ^ml == *ml && ^zl == *zl)]
#[ensures(connected && st.initialized && pool_size@ > 0 ==> result == Err(MtError::AllocationFailed)
          && ^ml == *ml && ^zl == *zl)]
#[ensures(st.initialized ==> match result { Ok(_) => false, Err(_) => ^ml == *ml && ^zl == *zl })]
#[ensures(connected && !st.initialized && pool_size@ > 0 && env_active ==> (^zl).calls@ == zl.calls@ + 1
          && (^zl).numa_id == numa_or_any(numa) && (^zl).size == pool_size)]
#[ensures(!env_active ==> ^zl == *zl)]
#[ensures(match result { Ok(_) => pool_size@ > 0 && connected && !st.initialized && (^st).initialized
          && (^st).pool_size == pool_size && (^st).allocator.capacity == pool_size
          && (^st).allocator.used@ == 0 && (^st).slots@ == Seq::empty()
          && (^st).spdk_allocated == env_active && (env_active ==> (^st).pool_base == r_spdk), Err(_) => true })]
#[ensures(connected && !st.initialized && pool_size@ > 0
          && (env_active && r_spdk@ != 0 || !env_active && (r_huge != MAP_FAILED || r_plain != MAP_FAILED))
          ==> result == Ok(()))]
#[ensures(connected && !st.initialized && pool_size@ > 0
          && (env_active && r_spdk@ == 0 || !env_active && r_huge == MAP_FAILED && r_plain == MAP_FAILED)
          ==> result == Err(MtError::AllocationFailed))]
pub fn rx_initialize_spdk(
    st: &mut StateModel,
    pool_size: usize,
    numa: Option<i32>,
    connected: bool,
    env_active: bool,
    r_spdk: usize,
    r_huge: usize,
    r_plain: usize,
    rc: i64,
    ml: &mut MmapLog,
    zl: &mut ZmLog,
) -> Result<(), MtError> {
    if pool_size == 0 {
        return Err(MtError::InvalidSize); // lib.rs:262-264 (pin)
    }
    if !ep_get(connected) {
        return Err(MtError::NotInitialized); // lib.rs:266-268 (pin)
    }
    let mut lg = LogSink { lines: 0 };
    mt_initialize_spdk(st, pool_size, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false)
}

/// GHOST (batch 5): the CONTENT of the pool's memory region, absolute address -> byte.
/// SCOPE of this model: memory-tier never writes through `pool_ptr` (pin: insert :377,
/// get :404 and peek :420 only compute `pool_ptr.add(offset)`; remove :496-501, evict_next
/// :456-457 and clear :603-606 only edit `slots` / the `FreeList` metadata, allocator.rs keeps
/// no pointer), so NO mirrored memory-tier function takes a `PoolMem`: by construction its
/// bytes change only through `mem_write`, i.e. a caller storing through a returned pointer.
/// Not modelled: the initial zero fill (MAP_ANONYMOUS / spdk_zmalloc), lifetimes, other threads.
pub struct PoolMem {
    pub bytes: Snapshot<creusot_std::logic::Mapping<Int, u8>>,
}

/// A caller writes byte `v` at address `addr` through a pointer it was handed.
#[ensures(*(^m).bytes == (*m.bytes).set(addr@, v))]
pub fn mem_write(m: &mut PoolMem, addr: usize, v: u8) {
    m.bytes = snapshot! { (*m.bytes).set(addr@, v) };
}

// ===========================================================================
// Batch 5 — property drivers
// ===========================================================================

/// **MT-CLEAR-ERR-NOT-INITIALIZED** (error-case)
/// "Attempting to wipe the cache before the pool has been set up fails with the
/// not-initialized error carrying the message that the pool is not initialized."
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[ensures(^st == *st)]
#[ensures(match result { Err(MtError::NotInitialized) => true, _ => false })]
pub fn verify_mt_clear_err_not_initialized(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// Anti-vacuity twin: MUST FAIL (claims an un-set-up wipe reports zero entries removed).
#[requires(mt_inv(*st))]
#[requires(!st.initialized)]
#[ensures(^st == *st)]
#[ensures(match result { Ok(c) => c@ == 0, _ => false })]
pub fn verify_mt_clear_err_not_initialized__mutant(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// **MT-CLEAR-POST-COUNT** (postcondition)
/// "Wiping the cache reports how many entries were removed, which is exactly the number of
/// entries that were present at the moment of the call, and therefore reports zero when the
/// cache already held nothing."
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(match result { Ok(c) => c@ == st.slots@.len(), Err(_) => false })]
#[ensures(st.slots@.len() == 0 ==> result == Ok(0usize))]
#[ensures((^st).slots@.len() == 0)]
pub fn verify_mt_clear_post_count(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// Anti-vacuity twin: MUST FAIL (claims a wipe always reports zero).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures((^st).slots@.len() == 0)]
#[ensures(match result { Ok(c) => c@ == 0, Err(_) => false })]
pub fn verify_mt_clear_post_count__mutant(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// **MT-CLEAR-PRE** (precondition)
/// "To wipe the whole cache the pool must have been set up, and no condition at all is
/// placed on how many entries it currently holds, so wiping an already empty pool is
/// legitimate."
#[requires(mt_inv(*st))]
#[ensures(st.initialized ==> match result { Ok(_) => true, Err(_) => false })]
#[ensures(st.initialized && st.slots@.len() == 0 ==> result == Ok(0usize))]
#[ensures(!st.initialized ==> match result { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(mt_inv(^st))]
pub fn verify_mt_clear_pre(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// Anti-vacuity twin: MUST FAIL (claims an empty pool can be wiped whether or not it is set up).
#[requires(mt_inv(*st))]
#[ensures(mt_inv(^st))]
#[ensures(st.slots@.len() == 0 ==> match result { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_clear_pre__mutant(st: &mut StateModel) -> Result<usize, MtError> {
    mt_clear(st)
}

/// **MT-CONTAINS-ERR-NOT-INITIALIZED** (error-case)
/// "The presence check answers no for every key when the pool has not been set up yet, which
/// a caller cannot distinguish from the key genuinely being absent from a working pool."
#[requires(mt_inv(*st) && !st.initialized)]
#[requires(mt_inv(*live) && live.initialized && !slot_at(live.slots@, key))]
#[ensures(!result.0)]
#[ensures(!result.1)]
#[ensures(result.0 == result.1)]
pub fn verify_mt_contains_err_not_initialized(st: &StateModel, live: &StateModel, key: Key) -> (bool, bool) {
    (mt_contains(st, key), mt_contains(live, key))
}

/// Anti-vacuity twin: MUST FAIL (claims the two answers are distinguishable).
#[requires(mt_inv(*st) && !st.initialized)]
#[requires(mt_inv(*live) && live.initialized && !slot_at(live.slots@, key))]
#[ensures(!result.0)]
#[ensures(result.0 != result.1)]
pub fn verify_mt_contains_err_not_initialized__mutant(st: &StateModel, live: &StateModel, key: Key) -> (bool, bool) {
    (mt_contains(st, key), mt_contains(live, key))
}

/// **MT-CONTAINS-PRE** (precondition, spec-only)
/// "The presence check may be called with any key at any time and always answers yes or no
/// rather than failing."
/// Before set-up (Default), after a set-up attempt and after ANY later call sequence: a
/// (panic-free) boolean, exactly `initialized && key present`. SCOPE: unpoisoned locks.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(!result.0)]
#[ensures(result.1 == (result.3 && result.4))]
pub fn verify_mt_contains_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)], key: Key)
    -> (bool, bool, Result<(), MtError>, bool, bool) {
    let mut st = mt_default(p);
    let c0 = mt_contains(&st, key); // before set-up
    let r = mt_initialize(&mut st, psz);
    mt_run(&mut st, ops, psz);
    let c1 = mt_contains(&st, key); // any later point
    let init = st.initialized;
    let present = slots_contains(&st.slots, key);
    (c0, c1, r, init, present)
}

/// Anti-vacuity twin: MUST FAIL (claims a set-up pool answers yes for every key).
#[requires(mt_inv(*st))]
#[requires(st.initialized)]
#[ensures(result == (st.initialized && slot_at(st.slots@, key)))]
#[ensures(result)]
pub fn verify_mt_contains_pre__mutant(st: &StateModel, key: Key) -> bool {
    mt_contains(st, key)
}

/// **MT-EVICT-NEXT-PRE** (precondition)
/// "To evict an entry the pool must have been set up and the eviction policy must be
/// connected; the caller needs no knowledge of which entry will be chosen, and an empty cache
/// is reported through the return value rather than by raising an error."
/// `st`: any reachable state, no key argument -- not set up => None, set up + empty => None
/// (an Option, no error channel), set up + non-empty => some victim. `st2`: the receptacle
/// lookup (lib.rs:440, pin) -- disconnected after set-up aborts (`.unwrap()`), connected returns.
#[requires(mt_inv(*st))]
#[requires(mt_inv(*st2) && arith_ok(*st2) && !st2.poisoned && st2.initialized)]
#[ensures(!st.initialized ==> result.0 == None && ^st == *st)]
#[ensures(st.initialized && st.slots@.len() == 0 ==> result.0 == None && (^st).slots@ == st.slots@)]
#[ensures(st.initialized && st.slots@.len() > 0 ==> result.0 != None)]
#[ensures(connected ==> match result.1 { RxOutcome::Returned => true, _ => false })]
#[ensures(!connected ==> match result.1 { RxOutcome::Panicked => true, _ => false })]
pub fn verify_mt_evict_next_pre(st: &mut StateModel, st2: &mut StateModel, key: Key, connected: bool)
    -> (Option<Key>, RxOutcome) {
    let r = mt_evict_next(st);
    let o = rx_call(st2, 4, key, 0, connected);
    (r, o)
}

/// Anti-vacuity twin: MUST FAIL (claims a set-up EMPTY cache still yields a victim).
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result == None)]
#[ensures(st.initialized && st.slots@.len() == 0 ==> result != None)]
pub fn verify_mt_evict_next_pre__mutant(st: &mut StateModel) -> Option<Key> {
    mt_evict_next(st)
}

/// **MT-EVICT-FOR-KEY-ERR-NO-VICTIM** (error-case, spec-only)
/// "Asking to free space on behalf of a key reports that nothing was evicted, and changes
/// nothing, both when the cache is empty and when the pool has not been set up."
#[requires(mt_inv(*st))]
#[requires(!st.initialized || st.slots@.len() == 0)]
#[ensures(result == None)]
#[ensures(same_obs(*st, ^st))]
#[ensures(!st.initialized ==> ^st == *st)]
pub fn verify_mt_evict_for_key_err_no_victim(st: &mut StateModel, key: Key) -> Option<Key> {
    mt_evict_next_for_key(st, key)
}

/// Anti-vacuity twin: MUST FAIL (claims something is evicted).
#[requires(mt_inv(*st))]
#[requires(!st.initialized || st.slots@.len() == 0)]
#[ensures(same_obs(*st, ^st))]
#[ensures(result != None)]
pub fn verify_mt_evict_for_key_err_no_victim__mutant(st: &mut StateModel, key: Key) -> Option<Key> {
    mt_evict_next_for_key(st, key)
}

/// **MT-EVICT-FOR-KEY-FRAME-KEY-IGNORED** (frame, divergent)
/// "The key passed to this operation has no effect on what happens: it does not narrow the
/// choice of victim, it is not given any protection, and it may itself be the entry that gets
/// evicted and returned."
/// Two identical states, two DIFFERENT key arguments: the same victim, the same surviving key
/// set, the same bytes in use; and when the argument is the LRU head it is itself evicted.
#[requires(mt_inv(*a) && mt_inv(*b) && same_state(*a, *b))]
#[ensures(result.0 == result.1)]
#[ensures(forall<k: Key> slot_at((^a).slots@, k) == slot_at((^b).slots@, k))]
#[ensures((^a).allocator.used == (^b).allocator.used)]
#[ensures(forall<k: Key> tracks((^a).policy.tracked@, k) == tracks((^b).policy.tracked@, k))]
#[ensures(a.initialized && a.policy.tracked@.len() > 0 && (a.policy.tracked@[0]).0 == k1
          ==> result.0 == Some(k1) && !slot_at((^a).slots@, k1))]
pub fn verify_mt_evict_for_key_frame_key_ignored(a: &mut StateModel, b: &mut StateModel, k1: Key, k2: Key)
    -> (Option<Key>, Option<Key>) {
    let r1 = mt_evict_next_for_key(a, k1);
    let r2 = mt_evict_next_for_key(b, k2);
    proof_assert!(forall<k: Key> slot_at(a.slots@, k) == tracks(a.policy.tracked@, k));
    (r1, r2)
}

/// Anti-vacuity twin: MUST FAIL (claims the named key is protected from eviction).
#[requires(mt_inv(*a))]
#[requires(a.initialized && a.policy.tracked@.len() > 0 && (a.policy.tracked@[0]).0 == k1)]
#[ensures(result != None)]
#[ensures(slot_at((^a).slots@, k1))]
pub fn verify_mt_evict_for_key_frame_key_ignored__mutant(a: &mut StateModel, k1: Key) -> Option<Key> {
    mt_evict_next_for_key(a, k1)
}

/// **MT-EVICT-FOR-KEY-PRE** (precondition, spec-only)
/// "To free space on behalf of a particular key the pool must have been set up and the
/// eviction policy must be connected, but the named key itself need not be present in the
/// cache and no other condition applies."
#[requires(mt_inv(*st) && st.initialized && st.slots@.len() > 0 && !slot_at(st.slots@, key))]
#[requires(mt_inv(*st2) && arith_ok(*st2) && !st2.poisoned && st2.initialized)]
#[ensures(result.0 != None)]
#[ensures(forall<k: Key> result.0 == Some(k) ==> k != key && slot_at(st.slots@, k) && !slot_at((^st).slots@, k))]
#[ensures(mt_inv(^st))]
#[ensures(connected ==> match result.1 { RxOutcome::Returned => true, _ => false })]
#[ensures(!connected ==> match result.1 { RxOutcome::Panicked => true, _ => false })]
pub fn verify_mt_evict_for_key_pre(st: &mut StateModel, st2: &mut StateModel, key: Key, connected: bool)
    -> (Option<Key>, RxOutcome) {
    let r = mt_evict_next_for_key(st, key);
    let o = rx_call(st2, 5, key, 0, connected);
    (r, o)
}

/// Anti-vacuity twin: MUST FAIL (claims an absent key means nothing can be evicted).
#[requires(mt_inv(*st) && st.initialized && st.slots@.len() > 0 && !slot_at(st.slots@, key))]
#[ensures(mt_inv(^st))]
#[ensures(result == None)]
pub fn verify_mt_evict_for_key_pre__mutant(st: &mut StateModel, key: Key) -> Option<Key> {
    mt_evict_next_for_key(st, key)
}

/// **MT-FREE-CAPACITY-SUBTRACTION-SAFE** (invariant, code-only)
/// "The free-space query subtracts the bytes in use from the capacity with unchecked
/// arithmetic on unsigned numbers, so its correctness depends entirely on the bytes in use
/// never exceeding the capacity."
/// From the Default state, through a set-up attempt and ANY call sequence (`mt_run`): `used <=
/// capacity` holds (part of `acct_ok`), so `capacity - used` (lib.rs:186, pin) never wraps
/// (Creusot checks the `usize` subtraction for underflow) and equals the true difference.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.1 ==> result.2@ <= result.3@ && result.0@ == result.3@ - result.2@)]
#[ensures(!result.1 ==> result.0@ == 0)]
pub fn verify_mt_free_capacity_subtraction_safe(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (usize, bool, usize, usize) {
    let mut st = mt_default(p);
    let _ = mt_initialize(&mut st, psz);
    mt_run(&mut st, ops, psz);
    let f = mt_free_capacity(&st);
    snapshot! { lem_fs_bound(st.allocator.regions@) };
    proof_assert!(st.initialized ==> st.allocator.used@ <= st.allocator.capacity@);
    (f, st.initialized, st.allocator.used, st.allocator.capacity)
}

/// Anti-vacuity twin: MUST FAIL -- the same unchecked subtraction WITHOUT the invariant
/// (an arbitrary set-up state): the `usize` underflow check fails.
#[requires(st.initialized)]
#[ensures(result@ <= st.allocator.capacity@)]
pub fn verify_mt_free_capacity_subtraction_safe__mutant(st: &StateModel) -> usize {
    st.allocator.capacity - st.allocator.used
}

/// **MT-GET-ERR-MISS** (error-case, divergent)
/// "The recency-updating lookup reports that nothing was found both when the cache holds no
/// entry for the key and when the pool has never been set up, rather than returning a stale
/// or invalid address."
#[requires(mt_inv(*st))]
#[requires(!st.initialized || !slot_at(st.slots@, key))]
#[ensures(result == None)]
#[ensures(refresh_frame(*st, ^st) && (^st).policy.tracked@ == st.policy.tracked@)]
#[ensures(!st.initialized ==> ^st == *st)]
pub fn verify_mt_get_err_miss(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// Anti-vacuity twin: MUST FAIL (claims a miss hands back an address).
#[requires(mt_inv(*st))]
#[requires(!st.initialized || !slot_at(st.slots@, key))]
#[ensures(refresh_frame(*st, ^st))]
#[ensures(result != None)]
pub fn verify_mt_get_err_miss__mutant(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// **MT-GET-POST-HIT** (postcondition)
/// "Looking up a key that is present returns that entry's address, computed as the pool base
/// plus the entry's stored offset, together with the size that was originally requested for
/// it, so the caller reads the identical block of cached data that was handed out at insertion."
/// `st`: any set-up state holding `key` at index i -> exactly (base + slots[i].offset,
/// slots[i].size). `st2`: insert(key2, size) = Ok(p) then get(key2) = Some((p, size)).
#[requires(mt_inv(*st) && st.initialized && slot_at(st.slots@, key))]
#[requires(mt_inv(*st2) && st2.initialized && st2.policy.next_handle@ + 1 < u64::MAX@)]
#[ensures(match result.0 { Some((p, s)) => forall<i: Int> 0 <= i && i < st.slots@.len() && st.slots@[i].key == key
          ==> p@ == st.pool_base@ + st.slots@[i].offset@ && s == st.slots@[i].size, None => false })]
#[ensures(forall<p: usize> result.1 == Ok(p) ==> result.2 == Some((p, size)))]
pub fn verify_mt_get_post_hit(st: &mut StateModel, key: Key, st2: &mut StateModel, key2: Key, size: u32)
    -> (Option<(usize, u32)>, Result<usize, MtError>, Option<(usize, u32)>) {
    let h = mt_get(st, key);
    let ins = mt_insert(st2, key2, size);
    let g = mt_get(st2, key2);
    proof_assert!(slot_keys_unique(st2.slots@));
    (h, ins, g)
}

/// Anti-vacuity twin: MUST FAIL (claims the address ignores the entry's offset).
#[requires(mt_inv(*st) && st.initialized && slot_at(st.slots@, key))]
#[ensures(result != None)]
#[ensures(forall<p: usize, s: u32> result == Some((p, s)) ==> p == st.pool_base)]
pub fn verify_mt_get_post_hit__mutant(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// **MT-GET-PRE** (precondition)
/// "The recency-updating lookup imposes no precondition at all: it may be called with any key
/// at any time, including on a component that was never set up, and it reports a missing
/// entry through its return value rather than by raising an error."
#[requires(mt_inv(*st))]
#[ensures(!st.initialized ==> result == None && ^st == *st)]
#[ensures(st.initialized ==> (result == None) == !slot_at(st.slots@, key))]
#[ensures(mt_inv(^st))]
pub fn verify_mt_get_pre(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// Anti-vacuity twin: MUST FAIL (claims a never-set-up component returns an address).
#[requires(mt_inv(*st))]
#[ensures(mt_inv(^st))]
#[ensures(!st.initialized ==> result != None)]
pub fn verify_mt_get_pre__mutant(st: &mut StateModel, key: Key) -> Option<(usize, u32)> {
    mt_get(st, key)
}

/// **MT-INIT-ERR-ALLOC-FAILED** (error-case)
/// "If the underlying request for memory from the operating system or from the storage
/// runtime cannot be satisfied, setting up the pool fails with the allocation-failed error
/// carrying a description of what went wrong, and the component stays not set up rather than
/// returning an unusable pool."
/// Both sources: SPDK env active and `spdk_zmalloc` NULL, or mmap path with both mmaps failing.
#[requires(mt_inv(*st) && !st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[requires(env_active && r_spdk@ == 0 || !env_active && r_huge == MAP_FAILED && r_plain == MAP_FAILED)]
#[ensures(^st == *st)]
#[ensures(!(^st).initialized)]
#[ensures(result == Err(MtError::AllocationFailed))]
pub fn verify_mt_init_err_alloc_failed(
    st: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    mt_initialize_spdk(st, psz, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false)
}

/// Anti-vacuity twin: MUST FAIL (claims a failed allocation still yields a set-up pool).
#[requires(mt_inv(*st) && !st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[requires(env_active && r_spdk@ == 0 || !env_active && r_huge == MAP_FAILED && r_plain == MAP_FAILED)]
#[ensures(^st == *st)]
#[ensures((^st).initialized)]
pub fn verify_mt_init_err_alloc_failed__mutant(
    st: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    let mut lg = LogSink { lines: 0 };
    mt_initialize_spdk(st, psz, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false)
}

/// **MT-INIT-ERR-DOUBLE-INIT** (error-case, divergent)
/// "A second attempt to set up the memory pool on a component that is already set up fails
/// with an error and leaves the existing pool completely untouched instead of replacing it."
/// Full initialize (receptacle connected or not, either build, any OS/SPDK result).
#[requires(mt_inv(*st) && st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(match result { Ok(_) => false, Err(_) => true })]
#[ensures(^st == *st)]
#[ensures(^ml == *ml && ^zl == *zl)]
#[ensures(connected && psz@ > 0 ==> result == Err(MtError::AllocationFailed))]
pub fn verify_mt_init_err_double_init(
    st: &mut StateModel, psz: usize, numa: Option<i32>, connected: bool, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, numa, connected, env_active, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// Anti-vacuity twin: MUST FAIL (claims a second set-up with good inputs succeeds).
#[requires(mt_inv(*st) && st.initialized)]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[requires(r_huge != MAP_FAILED)]
#[ensures(^ml == *ml)]
#[ensures(result == Ok(()))]
pub fn verify_mt_init_err_double_init__mutant(
    st: &mut StateModel, psz: usize, r_spdk: usize,
    r_huge: usize, r_plain: usize, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, None, true, false, r_spdk, r_huge, r_plain, 0, ml, zl)
}

/// **MT-INIT-ERR-NO-EVICTION-POLICY** (error-case, divergent)
/// "If the external eviction-policy receptacle has not been connected when the pool is set
/// up, the set-up call fails before any memory is allocated rather than producing a pool that
/// could never choose a victim."
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(result == Err(MtError::NotInitialized))]
#[ensures(^st == *st)]
#[ensures(^ml == *ml && ^zl == *zl)]
pub fn verify_mt_init_err_no_eviction_policy(
    st: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, numa, false, env_active, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// Anti-vacuity twin: MUST FAIL (claims the pool memory is requested before the check).
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(^st == *st)]
#[ensures((^ml).huge_calls@ == ml.huge_calls@ + 1)]
pub fn verify_mt_init_err_no_eviction_policy__mutant(
    st: &mut StateModel, psz: usize, r_spdk: usize,
    r_huge: usize, r_plain: usize, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, None, false, false, r_spdk, r_huge, r_plain, 0, ml, zl)
}

/// **MT-INIT-ERR-ZERO-SIZE** (error-case)
/// "Asking to set up a pool of zero bytes fails with the invalid-size error before any memory
/// is requested from the operating system, and leaves the component not set up so that no
/// memory is reserved."
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(mmap_ok(r_huge, 0usize) && mmap_ok(r_plain, 0usize) && zmalloc_ok(r_spdk, 0usize))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(result == Err(MtError::InvalidSize))]
#[ensures(^st == *st)]
#[ensures(!st.initialized ==> !(^st).initialized)]
#[ensures(^ml == *ml && ^zl == *zl)]
pub fn verify_mt_init_err_zero_size(
    st: &mut StateModel, numa: Option<i32>, connected: bool, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, 0, numa, connected, env_active, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// Anti-vacuity twin: MUST FAIL (claims the receptacle is checked before the size).
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(mmap_ok(r_huge, 0usize) && mmap_ok(r_plain, 0usize) && zmalloc_ok(r_spdk, 0usize))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(^st == *st)]
#[ensures(result == Err(MtError::NotInitialized))]
pub fn verify_mt_init_err_zero_size__mutant(
    st: &mut StateModel, r_spdk: usize, r_huge: usize, r_plain: usize, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, 0, None, false, false, r_spdk, r_huge, r_plain, 0, ml, zl)
}

/// **MT-INIT-NUMA-NEGATIVE-IGNORED** (postcondition, code-only) -- HAZARD
/// "A requested processor memory node given as a negative number is silently ignored and the
/// pool is created with default memory placement, with no error and no warning telling the
/// caller that the request was dropped."
/// CONFIRMED (mmap path, alloc_mmap lib.rs:225-226 pin): `alloc_mmap(psz, Some(n<0))` and
/// `alloc_mmap(psz, None)` from identical logs are indistinguishable -- same result, no mbind,
/// and NOT ONE log line (no warning, no info); the whole initialize then succeeds.
#[requires(n@ < 0)]
#[requires(ma.huge_calls@ < u64::MAX@ && ma.plain_calls@ < u64::MAX@)]
#[requires(*ma == *mb)]
#[requires(r_huge != MAP_FAILED || r_plain != MAP_FAILED)]
#[requires(mt_inv(*st) && !st.initialized && st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(mc.huge_calls@ < u64::MAX@ && mc.plain_calls@ < u64::MAX@)]
#[ensures(result.0 == result.1)]
#[ensures(result.0 == Ok(mmap_base(r_huge, r_plain)))]
#[ensures((^ma).mbind_issued == ma.mbind_issued && (^ma).mbind_mask == ma.mbind_mask
          && (^ma).mbind_maxnode == ma.mbind_maxnode && (^ma).mbind_node == ma.mbind_node)]
#[ensures((^ma).huge_calls == (^mb).huge_calls && (^ma).plain_calls == (^mb).plain_calls
          && (^ma).mbind_issued == (^mb).mbind_issued && (^ma).mbind_mask == (^mb).mbind_mask
          && (^ma).mbind_maxnode == (^mb).mbind_maxnode && (^ma).mbind_node == (^mb).mbind_node)]
#[ensures(^la == *la)]
#[ensures(result.2 == Ok(()) && (^st).initialized)]
pub fn verify_mt_init_numa_negative_ignored(
    psz: usize, n: i32, r_huge: usize, r_plain: usize, rc: i64,
    ma: &mut MmapLog, mb: &mut MmapLog, la: &mut LogSink, lb: &mut LogSink, connected: bool,
    st: &mut StateModel, mc: &mut MmapLog,
) -> (Result<usize, MtError>, Result<usize, MtError>, Result<(), MtError>) {
    let ra = alloc_mmap_m(psz, Some(n), r_huge, r_plain, rc, ma, la, connected);
    let rb = alloc_mmap_m(psz, None, r_huge, r_plain, rc, mb, lb, connected);
    let mut lc = LogSink { lines: 0 };
    let ri = mt_initialize_mmap(st, psz, Some(n), r_huge, r_plain, rc, mc, &mut lc, false);
    (ra, rb, ri)
}

/// Anti-vacuity twin: MUST FAIL (claims a negative node draws a warning line).
#[requires(n@ < 0)]
#[requires(ma.huge_calls@ < u64::MAX@ && ma.plain_calls@ < u64::MAX@)]
#[requires(r_huge != MAP_FAILED)]
#[requires(la.lines@ < u64::MAX@)]
#[ensures(result == Ok(r_huge))]
#[ensures((^la).lines@ == la.lines@ + 1)]
pub fn verify_mt_init_numa_negative_ignored__mutant(
    psz: usize, n: i32, r_huge: usize, r_plain: usize, rc: i64, ma: &mut MmapLog, la: &mut LogSink,
) -> Result<usize, MtError> {
    alloc_mmap_m(psz, Some(n), r_huge, r_plain, rc, ma, la, true)
}

/// **MT-INIT-NUMA-ONLY-ON-MMAP-PATH** (postcondition, code-only) -- HAZARD
/// "A requested processor memory node is only honoured when the pool happens to be taken from
/// the ordinary memory-mapping path; when the storage runtime's allocator supplies the pool
/// the requested node is dropped entirely with no error and no warning."
/// REFUTED (the "dropped entirely" claim): with the SPDK env active the requested node is
/// handed to `spdk_zmalloc` as its `numa_id` argument (lib.rs:280-289 pin), which spdk/env.h
/// documents as "NUMA node ID to allocate memory on"; a successful call stores the SPDK
/// region. The NEGATION proved: on the SPDK path the request is forwarded, not dropped.
#[requires(mt_inv(*st) && !st.initialized && st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(n@ >= 0)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures((^zl).calls@ == zl.calls@ + 1)]
#[ensures((^zl).numa_id == n && (^zl).size == psz)]
#[ensures(r_spdk@ != 0 ==> result == Ok(()) && (^st).spdk_allocated && (^st).pool_base == r_spdk)]
pub fn refute_mt_init_numa_only_on_mmap_path(
    st: &mut StateModel, psz: usize, n: i32, r_spdk: usize, r_huge: usize, r_plain: usize, rc: i64,
    ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, Some(n), true, true, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// Anti-vacuity twin: MUST FAIL (the hazard's reading: the node never reaches the allocator --
/// the SPDK allocator is asked for "any node").
#[requires(mt_inv(*st) && !st.initialized && st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(n@ >= 0)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures((^zl).calls@ == zl.calls@ + 1)]
#[ensures((^zl).numa_id@ == -1)]
pub fn refute_mt_init_numa_only_on_mmap_path__mutant(
    st: &mut StateModel, psz: usize, n: i32, r_spdk: usize, r_huge: usize, r_plain: usize,
    ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, Some(n), true, true, r_spdk, r_huge, r_plain, 0, ml, zl)
}

/// **MT-INIT-POST-NUMA-FALLBACK** (postcondition)
/// "If the caller asks for the pool memory to be pinned to a particular processor memory node
/// and that pinning cannot be carried out, setting up the pool still succeeds and the pool is
/// fully usable under whatever memory placement the operating system chose instead."
/// mmap path (lib.rs:244-256 pin): `a` gets an mbind FAILURE (rc != 0), `b` -- an identical
/// copy -- a success (rc == 0). Both Ok, and the two set-up pools are the same pool: base,
/// size, a single free run covering it, nothing in use, the full invariant.
#[requires(mt_inv(*a) && !a.initialized && a.policy.pools@ < u64::MAX@ && same_state(*a, *b) && mt_inv(*b))]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(n@ >= 0 && rc != 0i64)]
#[requires(r_huge != MAP_FAILED || r_plain != MAP_FAILED)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ma.huge_calls@ < u64::MAX@ && ma.plain_calls@ < u64::MAX@)]
#[requires(mb.huge_calls@ < u64::MAX@ && mb.plain_calls@ < u64::MAX@)]
#[ensures(result.0 == Ok(()) && result.1 == Ok(()))]
#[ensures(mt_inv(^a) && (^a).initialized && (^a).allocator.used@ == 0 && (^a).slots@ == Seq::empty()
          && (^a).allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: psz }))]
#[ensures((^ma).mbind_issued && (^ma).mbind_node@ == n@)]
#[ensures((^a).pool_base == (^b).pool_base && (^a).pool_size == (^b).pool_size
          && (^a).allocator.regions@ == (^b).allocator.regions@ && (^a).allocator.capacity == (^b).allocator.capacity
          && (^a).allocator.used == (^b).allocator.used && (^a).slots@ == (^b).slots@
          && (^a).pool_id == (^b).pool_id && (^a).spdk_allocated == (^b).spdk_allocated)]
pub fn verify_mt_init_post_numa_fallback(
    a: &mut StateModel, b: &mut StateModel, psz: usize, n: i32, rc: i64, r_huge: usize, r_plain: usize,
    ma: &mut MmapLog, mb: &mut MmapLog,
) -> (Result<(), MtError>, Result<(), MtError>) {
    let mut la = LogSink { lines: 0 };
    let mut lb = LogSink { lines: 0 };
    let ra = mt_initialize_mmap(a, psz, Some(n), r_huge, r_plain, rc, ma, &mut la, false);
    let rb = mt_initialize_mmap(b, psz, Some(n), r_huge, r_plain, 0, mb, &mut lb, false);
    (ra, rb)
}

/// Anti-vacuity twin: MUST FAIL (claims a failed binding aborts the set-up).
#[requires(mt_inv(*a) && !a.initialized && a.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(n@ >= 0 && rc != 0i64)]
#[requires(r_huge != MAP_FAILED)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ma.huge_calls@ < u64::MAX@ && ma.plain_calls@ < u64::MAX@)]
#[ensures(mt_inv(^a))]
#[ensures(result == Err(MtError::AllocationFailed))]
pub fn verify_mt_init_post_numa_fallback__mutant(
    a: &mut StateModel, psz: usize, n: i32, rc: i64, r_huge: usize, r_plain: usize, ma: &mut MmapLog,
) -> Result<(), MtError> {
    let mut la = LogSink { lines: 0 };
    mt_initialize_mmap(a, psz, Some(n), r_huge, r_plain, rc, ma, &mut la, false)
}

/// **MT-INIT-PRE** (precondition)
/// "Before the pool-setup operation may be called successfully the component must not already
/// have been initialized, the requested pool size must be greater than zero bytes, and the
/// external eviction-policy receptacle must already be connected."
/// Necessity (Ok ==> all three) and sufficiency (all three + an allocation the OS/SPDK grants
/// ==> Ok) over the FULL initialize, both builds.
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(result == Ok(()) ==> !st.initialized && psz@ > 0 && connected)]
#[ensures(!st.initialized && psz@ > 0 && connected
          && (env_active && r_spdk@ != 0 || !env_active && (r_huge != MAP_FAILED || r_plain != MAP_FAILED))
          ==> result == Ok(()))]
pub fn verify_mt_init_pre(
    st: &mut StateModel, psz: usize, numa: Option<i32>, connected: bool, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, numa, connected, env_active, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// Anti-vacuity twin: MUST FAIL (drops the allocation conjunct: claims the three conditions
/// alone guarantee success).
#[requires(mt_inv(*st))]
#[requires(st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(result == Ok(()) ==> !st.initialized && psz@ > 0 && connected)]
#[ensures(!st.initialized && psz@ > 0 && connected ==> result == Ok(()))]
pub fn verify_mt_init_pre__mutant(
    st: &mut StateModel, psz: usize, numa: Option<i32>, connected: bool, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> Result<(), MtError> {
    rx_initialize_spdk(st, psz, numa, connected, env_active, r_spdk, r_huge, r_plain, rc, ml, zl)
}

/// **MT-INSERT-ERR-ALREADY-EXISTS** (error-case)
/// "Asking to add a cache entry under a key that already has an entry fails with the
/// already-exists error naming that key, and because the duplicate check runs before any
/// space is reserved the entry that was already there keeps its original address, size and
/// contents and no pool space is wasted."
#[requires(mt_inv(*st) && st.initialized && slot_at(st.slots@, key))]
#[requires(size@ > 0 && st.policy.next_handle@ < u64::MAX@)]
#[ensures(result == Err(MtError::AlreadyExists))]
#[ensures(same_state(*st, ^st))]
#[ensures(forall<i: Int> 0 <= i && i < st.slots@.len() ==> (^st).slots@[i] == st.slots@[i])]
#[ensures((^st).allocator.used == st.allocator.used && (^st).allocator.regions@ == st.allocator.regions@)]
pub fn verify_mt_insert_err_already_exists(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// Anti-vacuity twin: MUST FAIL (claims the duplicate attempt reserved space).
#[requires(mt_inv(*st) && st.initialized && slot_at(st.slots@, key))]
#[requires(size@ > 0 && st.policy.next_handle@ < u64::MAX@)]
#[ensures(result == Err(MtError::AlreadyExists))]
#[ensures((^st).allocator.used@ == st.allocator.used@ + align_up_l(size@))]
pub fn verify_mt_insert_err_already_exists__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// **MT-INSERT-ERR-INVALID-SIZE** (error-case)
/// "Asking to add a cache entry of zero bytes fails with the invalid-size error before any
/// lock is taken, so no key is created and no memory is reserved."
#[requires(mt_inv(*st) && st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Err(MtError::InvalidSize) => true, _ => false })]
#[ensures(same_state(*st, ^st))]
#[ensures(result.1@ == Seq::empty())]
pub fn verify_mt_insert_err_invalid_size(st: &mut StateModel, key: Key) -> (Result<usize, MtError>, Vec<LkEv>) {
    let init = st.initialized;
    let absent = !slots_contains(&st.slots, key);
    let r = mt_insert(st, key, 0);
    let t = lk_insert(false, init, absent, false);
    (r, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the state lock is taken first).
#[requires(mt_inv(*st) && st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Err(MtError::InvalidSize) => true, _ => false })]
#[ensures(result.1@.len() > 0 && result.1@[0] == LkEv::StateRead)]
pub fn verify_mt_insert_err_invalid_size__mutant(st: &mut StateModel, key: Key) -> (Result<usize, MtError>, Vec<LkEv>) {
    let init = st.initialized;
    let r = mt_insert(st, key, 0);
    let t = lk_insert(false, init, true, false);
    (r, t)
}

/// **MT-INSERT-ERR-NOT-INITIALIZED** (error-case)
/// "Attempting to add a cache entry before the pool has been set up fails with the
/// not-initialized error, carrying the message that the pool is not initialized, rather than
/// reading or writing any memory."
/// The state is untouched and the lock trace is just `state.read()` -- no pool lock, so no
/// pool bytes or bookkeeping are read or written (lib.rs:333-337 pin).
#[requires(mt_inv(*st) && !st.initialized && size@ > 0 && st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result.0 { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(^st == *st)]
#[ensures(result.1@ == Seq::singleton(LkEv::StateRead))]
#[ensures(no_pool_read(result.1@) && no_pool_write(result.1@))]
pub fn verify_mt_insert_err_not_initialized(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, Vec<LkEv>) {
    let absent = !slots_contains(&st.slots, key);
    let r = mt_insert(st, key, size);
    let t = lk_insert(true, false, absent, false);
    (r, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the pool write lock is taken).
#[requires(mt_inv(*st) && !st.initialized && size@ > 0 && st.policy.next_handle@ < u64::MAX@)]
#[ensures(^st == *st)]
#[ensures(exists<i: Int> 0 <= i && i < result.1@.len() && result.1@[i] == LkEv::PoolWrite)]
pub fn verify_mt_insert_err_not_initialized__mutant(st: &mut StateModel, key: Key, size: u32)
    -> (Result<usize, MtError>, Vec<LkEv>) {
    let r = mt_insert(st, key, size);
    let t = lk_insert(true, false, true, false);
    (r, t)
}

/// **MT-INSERT-MEMORY-NOT-CLEARED** (postcondition, code-only) -- HAZARD
/// "Adding a cache entry hands back a region of the pool without clearing it, and because the
/// allocator reuses the byte offsets of evicted or removed entries, the bytes a caller
/// receives for a brand-new key can still hold data written by a previous occupant of that
/// space."
/// CONFIRMED (witness scenario): a freshly set-up one-page pool; insert(k1) -> p1; the caller
/// writes `v` at p1; remove(k1); insert(k2 != k1) -> p2. Proved: p2 == p1 (first fit reuses
/// the freed offset) and the byte at p2 is still `v` (insert never writes the region).
#[requires(mt_inv(*st) && st.initialized && st.slots@ == Seq::empty())]
#[requires(st.allocator.capacity@ == 4096
           && st.allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: 4096usize }))]
#[requires(st.policy.next_handle@ + 2 < u64::MAX@)]
#[requires(k1 != k2)]
#[ensures(match result { Ok((p1, p2)) => p1 == p2 && (*(^mem).bytes).get(p2@) == v, Err(_) => false })]
pub fn verify_mt_insert_memory_not_cleared(st: &mut StateModel, mem: &mut PoolMem, k1: Key, k2: Key, v: u8)
    -> Result<(usize, usize), MtError> {
    snapshot! { lem_al(4096) };
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(4096)));
    let p1 = match mt_insert(st, k1, 4096) {
        Ok(p) => p,
        Err(e) => return Err(e),
    };
    proof_assert!(p1 == st.pool_base);
    mem_write(mem, p1, v); // the previous occupant's data
    match mt_remove(st, k1) {
        Ok(_) => (),
        Err(e) => return Err(e),
    }
    proof_assert!(!slot_at(st.slots@, k2));
    let p2 = match mt_insert(st, k2, 4096) {
        Ok(p) => p,
        Err(e) => return Err(e),
    };
    Ok((p1, p2))
}

/// Anti-vacuity twin: MUST FAIL (claims the new key's bytes come back cleared).
#[requires(mt_inv(*st) && st.initialized && st.slots@ == Seq::empty())]
#[requires(st.allocator.capacity@ == 4096
           && st.allocator.regions@ == Seq::singleton(Region { offset: 0usize, size: 4096usize }))]
#[requires(st.policy.next_handle@ + 2 < u64::MAX@)]
#[requires(k1 != k2 && v@ != 0)]
#[ensures(match result { Ok((p1, p2)) => p1 == p2 && (*(^mem).bytes).get(p2@)@ == 0, Err(_) => false })]
pub fn verify_mt_insert_memory_not_cleared__mutant(st: &mut StateModel, mem: &mut PoolMem, k1: Key, k2: Key, v: u8)
    -> Result<(usize, usize), MtError> {
    snapshot! { lem_al(4096) };
    proof_assert!(some_region_fits(st.allocator.regions@, align_up_l(4096)));
    let p1 = match mt_insert(st, k1, 4096) {
        Ok(p) => p,
        Err(e) => return Err(e),
    };
    proof_assert!(p1 == st.pool_base);
    mem_write(mem, p1, v);
    match mt_remove(st, k1) {
        Ok(_) => (),
        Err(e) => return Err(e),
    }
    proof_assert!(!slot_at(st.slots@, k2));
    let p2 = match mt_insert(st, k2, 4096) {
        Ok(p) => p,
        Err(e) => return Err(e),
    };
    Ok((p1, p2))
}

/// **MT-INSERT-PRE** (precondition)
/// "To add a new cache entry successfully the pool must already have been set up, the
/// requested size must be at least one byte, the key must not already have an entry, and
/// there must be a single free run of pool memory at least as large as the requested size
/// rounded up to the next four-kibibyte boundary."
/// Necessity (Ok ==> all four) and sufficiency (all four ==> Ok).
#[requires(mt_inv(*st) && st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result { Ok(_) => st.initialized && size@ >= 1 && !slot_at(st.slots@, key)
          && some_region_fits(st.allocator.regions@, align_up_l(size@)), Err(_) => true })]
#[ensures(st.initialized && size@ >= 1 && !slot_at(st.slots@, key)
          && some_region_fits(st.allocator.regions@, align_up_l(size@))
          ==> match result { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_insert_pre(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

/// Anti-vacuity twin: MUST FAIL (drops the free-run conjunct).
#[requires(mt_inv(*st) && st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result { Ok(_) => st.initialized && size@ >= 1, Err(_) => true })]
#[ensures(st.initialized && size@ >= 1 && !slot_at(st.slots@, key)
          ==> match result { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_insert_pre__mutant(st: &mut StateModel, key: Key, size: u32) -> Result<usize, MtError> {
    mt_insert(st, key, size)
}

// ===========================================================================
// Batch 6 — supporting mirrors
// ===========================================================================

/// `IEvictionPolicy::track(pool, key, semantics) -> Result<EvictionHandle, EvictionPolicyError>`
/// (lib.rs:365-367 pin) modelled per its INTERFACE CONTRACT (ieviction_policy.rs:52-59, 77-87):
/// the only error kinds are `InvalidPool(id)` = "the specified pool does not exist" and
/// `InvalidHandle` (track takes no handle). Pool ids are handed out by `create_pool` as
/// `0, 1, 2, ..` (`policy_create_pool`), so a pool the policy created is `pool < pools`: there
/// track MUST succeed. For an id the policy never created it MAY refuse -- `refuse` is the
/// policy's free choice. `None` = `Err(InvalidPool)`.
#[requires(p.next_handle@ < u64::MAX@)]
#[requires(policy_keys_unique(p.tracked@))]
#[requires(!tracks(p.tracked@, key))]
#[ensures((result == None) == (pool@ >= p.pools@ && refuse))]
#[ensures(match result {
    Some(h) => (^p).tracked@ == p.tracked@.push_back((key, h)) && policy_keys_unique((^p).tracked@)
        && tracks((^p).tracked@, key) && (^p).next_handle@ == p.next_handle@ + 1,
    None => ^p == *p,
})]
#[ensures((^p).pools == p.pools)]
pub fn policy_track_r(p: &mut PolicyModel, pool: u64, key: Key, refuse: bool) -> Option<u64> {
    if pool >= p.pools && refuse {
        return None; // Err(EvictionPolicyError::InvalidPool(pool))
    }
    Some(policy_track(p, pool, key))
}

/// The outcome of one `insert` call: it returned `r`, or it PANICKED.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum InsOutcome {
    Done(Result<usize, MtError>),
    Panicked,
}

/// `IMemoryTier::insert` (lib.rs:328-379 pin) WITH the fallible `ep.track(..)` and its
/// `.unwrap()` (lib.rs:365-367 pin). Program order: size check (:329), flag (:334), pool write
/// lock (:354), duplicate check (:356), `allocator.allocate(..)?` (:360-363) -- the run is now
/// RESERVED -- then `track(..).unwrap()`: on `Err` the unwrap panics with the pool write guard
/// live, which poisons the pool `RwLock` (std: a panic while a write guard is held poisons the
/// lock); the slot is never recorded. Whether track refuses depends only on the pool id and the
/// policy (not on the allocator), so the refusing case is split out first; the non-refusing case
/// IS `mt_insert` (whose `policy_track` is the succeeding track).
#[requires(mt_inv(*st))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result {
    InsOutcome::Panicked => st.initialized && size@ > 0 && !slot_at(st.slots@, key)
        && st.pool_id@ >= st.policy.pools@ && refuse
        && (^st).allocator.used@ == st.allocator.used@ + align_up_l(size@)
        && (^st).slots@ == st.slots@
        && (^st).policy.tracked@ == st.policy.tracked@
        && !tracks((^st).policy.tracked@, key)
        && (^st).poisoned,
    InsOutcome::Done(Ok(_)) => slot_at((^st).slots@, key) && tracks((^st).policy.tracked@, key)
        && (^st).allocator.used@ == st.allocator.used@ + align_up_l(size@) && mt_inv(^st),
    InsOutcome::Done(Err(_)) => same_state(*st, ^st),
})]
#[ensures(st.initialized ==> st.pool_id@ < st.policy.pools@
          ==> match result { InsOutcome::Panicked => false, _ => true })]
#[ensures(st.initialized && size@ > 0 && !slot_at(st.slots@, key) && st.pool_id@ >= st.policy.pools@ && refuse
          && some_region_fits(st.allocator.regions@, align_up_l(size@))
          ==> match result { InsOutcome::Panicked => true, _ => false })]
pub fn mt_insert_r(st: &mut StateModel, key: Key, size: u32, refuse: bool) -> InsOutcome {
    if st.initialized && size > 0 && st.pool_id >= st.policy.pools && refuse {
        if slots_contains(&st.slots, key) {
            return InsOutcome::Done(Err(MtError::AlreadyExists)); // lib.rs:356-358
        }
        proof_assert!(!tracks(st.policy.tracked@, key));
        match fl_allocate(&mut st.allocator, size) {
            // lib.rs:360-363
            None => InsOutcome::Done(Err(MtError::PoolFull)),
            Some(_offset) => {
                let pid = st.pool_id;
                match policy_track_r(&mut st.policy, pid, key, refuse) {
                    // lib.rs:365-367: `.unwrap()` on Err -> panic, pool write guard live
                    None => {
                        st.poisoned = true;
                        InsOutcome::Panicked
                    }
                    Some(_) => {
                        proof_assert!(false);
                        InsOutcome::Panicked
                    }
                }
            }
        }
    } else {
        InsOutcome::Done(mt_insert(st, key, size))
    }
}

/// `oldest_keys` (lib.rs:424-432 pin): `state.read()`; flag / `n == 0` early return; ONE
/// `ep.get_eviction_candidates(pool_id, n)` call; no pool lock at all.
#[ensures(init && npos ==> result@ == Seq::empty().push_back(LkEv::StateRead).push_back(LkEv::PolicyOther))]
#[ensures(!(init && npos) ==> result@ == Seq::singleton(LkEv::StateRead))]
#[ensures(no_pool_read(result@) && no_pool_write(result@))]
pub fn lk_oldest_keys(init: bool, npos: bool) -> Vec<LkEv> {
    let mut t = Vec::new();
    t.push(LkEv::StateRead);
    if !init || !npos {
        return t;
    }
    t.push(LkEv::PolicyOther); // get_eviction_candidates
    t
}

/// The pool `RwLock` of `MemoryTierState::pool` (lib.rs:85 pin): live guards + poison flag.
pub struct PoolLockModel {
    pub readers: u64,
    pub writer: bool,
    pub poisoned: bool,
}

/// No guard live and not poisoned: what a single-threaded caller sees at every call boundary
/// (each method drops its pool guard before returning).
#[logic(open)]
pub fn lk_free(l: PoolLockModel) -> bool {
    pearlite! { l.readers@ == 0 && !l.writer && !l.poisoned }
}

/// `RwLock::try_write` (std): `Ok` iff no reader and no writer is live and the lock is not
/// poisoned (a poisoned lock yields `Err(TryLockError::Poisoned)`).
#[ensures(result == lk_free(*l))]
#[ensures(result ==> (^l).writer && (^l).readers == l.readers && !(^l).poisoned)]
#[ensures(!result ==> ^l == *l)]
pub fn try_write_m(l: &mut PoolLockModel) -> bool {
    if l.readers == 0 && !l.writer && !l.poisoned {
        l.writer = true;
        true
    } else {
        false
    }
}

/// `RwLock::try_read` (std): `Ok` iff no writer is live and the lock is not poisoned.
#[requires(l.readers@ < u64::MAX@)]
#[ensures(result == (!l.writer && !l.poisoned))]
#[ensures(result ==> (^l).readers@ == l.readers@ + 1 && (^l).writer == l.writer && (^l).poisoned == l.poisoned)]
#[ensures(!result ==> ^l == *l)]
pub fn try_read_m(l: &mut PoolLockModel) -> bool {
    if !l.writer && !l.poisoned {
        l.readers += 1;
        true
    } else {
        false
    }
}

/// One pool-lock acquisition site WITH its contention counter (telemetry build), then the
/// guard dropped at the end of the method. `kind`: 0 = `try_write` site (insert :343-352,
/// remove :483-492), 4 = evict_next with a victim (:443-452 + `evictions.fetch_add` :460),
/// 1 = `try_read` site (get :390-399, batch_touch :534-543), anything else = an uncounted
/// site (`read()`/`write()` in peek/touch/contains/capacity/used/clear, or no pool lock).
/// On a failed `try_*` the counter is bumped and the blocking `read()/write()` follows -- the
/// mirror stops there (that call blocks on another thread's guard, or panics on poison).
#[requires(l.readers@ < u64::MAX@)]
#[ensures(lk_free(*l) ==> lk_free(^l)
          && (^t).write_lock_contentions == t.write_lock_contentions
          && (^t).read_lock_contentions == t.read_lock_contentions)]
#[ensures((kind@ == 0 || kind@ == 4) && !lk_free(*l)
          ==> (^t).write_lock_contentions@ == fetch_l(t.write_lock_contentions@))]
#[ensures(kind@ == 0 || kind@ == 4 ==> (^t).read_lock_contentions == t.read_lock_contentions)]
#[ensures(kind@ == 1 && (l.writer || l.poisoned)
          ==> (^t).read_lock_contentions@ == fetch_l(t.read_lock_contentions@))]
pub fn tel_pool_site(l: &mut PoolLockModel, t: &mut TelemetryModel, kind: u8) {
    if kind == 0 || kind == 4 {
        if !try_write_m(l) {
            tel_event(t, 1); // write_lock_contentions.fetch_add(1)
            return; // then state.pool.write().unwrap()
        }
        if kind == 4 {
            tel_event(t, 0); // evictions.fetch_add(1) (lib.rs:460)
        }
        l.writer = false; // guard dropped
    } else if kind == 1 {
        if !try_read_m(l) {
            tel_event(t, 2); // read_lock_contentions.fetch_add(1)
            return; // then state.pool.read().unwrap()
        }
        l.readers -= 1; // guard dropped
    }
}

/// `IMemoryTier::telemetry_snapshot` (lib.rs:615-635 pin). `feature` = the `telemetry` cargo
/// feature: on, `state.read()` + three `load(Relaxed)`s (:618-628); off,
/// `MemoryTierTelemetrySnapshot::default()` (:633) -- all zero. Touches nothing (shared refs).
#[ensures(feature ==> result == trip(*t))]
#[ensures(!feature ==> result == (0u64, 0u64, 0u64))]
pub fn mt_telemetry_snapshot_m(t: &TelemetryModel, feature: bool) -> (u64, u64, u64) {
    if feature {
        let e = t.evictions;
        let w = t.write_lock_contentions;
        let r = t.read_lock_contentions;
        (e, w, r)
    } else {
        (0, 0, 0)
    }
}

// ===========================================================================
// Batch 6 — property drivers
// ===========================================================================

/// **MT-INSERT-TRACK-FAILURE-PANICS** (error-case, code-only) -- HAZARD -- REFUTATION
///
/// "Adding a cache entry reserves the pool space first and only afterwards asks the eviction
/// policy to track the new key, and it unwraps that result instead of handling it, so a
/// refusal by the policy aborts the operation after space has already been reserved instead of
/// failing cleanly."
/// Negation proved: in EVERY state reachable from an un-set-up component by ANY call sequence
/// (incl. the set-up call), with the bound policy obeying its `track` contract (it may refuse
/// only a pool it never created), insert never panics on the track result; a failing insert
/// leaves the state exactly as it was (no partial reservation); a successful one is tracked.
#[requires(mt_inv(*st) && !st.initialized && !st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(match result.0 { InsOutcome::Panicked => false, _ => true })]
#[ensures(match result.0 { InsOutcome::Done(Err(_)) => same_state(*result.1, ^st), _ => true })]
#[ensures(match result.0 { InsOutcome::Done(Ok(_)) => tracks((^st).policy.tracked@, key)
          && slot_at((^st).slots@, key), _ => true })]
pub fn refute_mt_insert_track_failure_panics(
    st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize, key: Key, size: u32, refuse: bool,
) -> (InsOutcome, Snapshot<StateModel>) {
    proof_assert!(pid_ok(*st));
    mt_run(st, ops, psz);
    proof_assert!(pid_ok(*st));
    let before = snapshot! { *st };
    let o = mt_insert_r(st, key, size, refuse);
    proof_assert!(match o { InsOutcome::Panicked => false, _ => true });
    (o, before)
}

/// Anti-vacuity twin: MUST FAIL -- the same claim WITHOUT the reachability argument (an
/// arbitrary set-up state, whose pool id need not name a pool the policy created).
#[requires(mt_inv(*st) && st.initialized && !st.poisoned)]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result { InsOutcome::Done(Err(_)) => same_state(*st, ^st), _ => true })]
#[ensures(match result { InsOutcome::Panicked => false, _ => true })]
pub fn refute_mt_insert_track_failure_panics__mutant(st: &mut StateModel, key: Key, size: u32, refuse: bool)
    -> InsOutcome {
    mt_insert_r(st, key, size, refuse)
}

/// UNSCORED witness -- the MECHANISM the hazard describes is real: if the bound policy does
/// not know the stored pool id (only possible outside a fixed binding, e.g. the
/// `eviction_policy` receptacle disconnected and re-connected to another policy instance after
/// `initialize`, receptacle.rs:84-117), a contract-obeying refusal panics AFTER the run was
/// reserved: used grew by the rounded size, no slot was recorded, the key is untracked, the pool
/// lock is poisoned.
#[requires(mt_inv(*st) && st.initialized && !st.poisoned)]
#[requires(st.pool_id@ >= st.policy.pools@)]
#[requires(size@ > 0 && !slot_at(st.slots@, key) && some_region_fits(st.allocator.regions@, align_up_l(size@)))]
#[requires(st.policy.next_handle@ < u64::MAX@)]
#[ensures(match result { InsOutcome::Panicked => true, _ => false })]
#[ensures((^st).allocator.used@ == st.allocator.used@ + align_up_l(size@) && (^st).slots@ == st.slots@)]
#[ensures(!tracks((^st).policy.tracked@, key) && (^st).poisoned)]
pub fn witness_mt_insert_track_failure_panics_mechanism(st: &mut StateModel, key: Key, size: u32) -> InsOutcome {
    mt_insert_r(st, key, size, true)
}

/// **MT-IS-DMA-CAPABLE-FRAME-STABLE** (frame)
///
/// "Asking whether the memory is directly usable by storage hardware changes nothing, and the
/// answer is decided once while the pool is being created and stays the same for the whole
/// life of that pool no matter how many entries are added, deleted, evicted or wiped."
/// The query takes the state by shared reference (lib.rs:610-613 pin: `state.read()`, one field
/// read). The answer is fixed by the set-up call (`spdk_allocated == env_active`, either build,
/// every OS/SPDK outcome as an input) and is identical after ANY later call sequence.
#[requires(mt_inv(*st) && !st.initialized && !st.poisoned)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1 == env_active && result.2 == result.1 && result.3 == result.1,
          Err(_) => true })]
pub fn verify_mt_is_dma_capable_frame_stable(
    st: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize, r_huge: usize,
    r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog, ops: &[(u8, Key, u32)],
) -> (Result<(), MtError>, bool, bool, bool) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_spdk(st, psz, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false);
    let d1 = mt_is_dma_capable(st); // right after set-up
    let d2 = mt_is_dma_capable(st); // asked again: the query changed nothing
    match r {
        Ok(_) => {
            mt_run(st, ops, psz); // any inserts / removes / evictions / clears / ...
        }
        Err(_) => {}
    }
    let d3 = mt_is_dma_capable(st);
    (r, d1, d2, d3)
}

/// Anti-vacuity twin: MUST FAIL (claims the answer can change after set-up).
#[requires(mt_inv(*st) && st.initialized && !st.poisoned)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0 == st.spdk_allocated)]
#[ensures(result.1 != result.0)]
pub fn verify_mt_is_dma_capable_frame_stable__mutant(st: &mut StateModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (bool, bool) {
    let d1 = mt_is_dma_capable(st);
    mt_run(st, ops, psz);
    let d2 = mt_is_dma_capable(st);
    (d1, d2)
}

/// **MT-IS-DMA-CAPABLE-HUGEPAGE-MMAP-UNDERSTATED** (postcondition, code-only) -- HAZARD
///
/// "A pool that the fallback path successfully mapped from huge pages still reports that it is
/// not directly usable by storage hardware, because the stored flag records only whether the
/// storage runtime's allocator was used and not whether the memory happens to be suitable."
/// CONFIRMED (mechanism): non-`spdk` build (`mt_initialize_mmap`) and `spdk` build with the SPDK
/// env inactive (`mt_initialize_spdk`, env_active = false): the MAP_HUGETLB mmap succeeds
/// (`r_huge != MAP_FAILED`), set-up is Ok, the pool IS that huge-page mapping (base == r_huge,
/// no plain fallback mmap issued), and `is_dma_capable()` answers false.
#[requires(mt_inv(*a) && !a.initialized && a.policy.pools@ < u64::MAX@)]
#[requires(mt_inv(*b) && !b.initialized && b.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(r_huge != MAP_FAILED && mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[requires(ml2.huge_calls@ < u64::MAX@ && ml2.plain_calls@ < u64::MAX@)]
#[ensures(result.0 == Ok(()) && (^a).pool_base == r_huge && (^ml2).plain_calls == ml2.plain_calls && !result.1)]
#[ensures(result.2 == Ok(()) && (^b).pool_base == r_huge && (^ml).plain_calls == ml.plain_calls && !result.3)]
pub fn verify_mt_is_dma_capable_hugepage_mmap_understated(
    a: &mut StateModel, b: &mut StateModel, psz: usize, numa: Option<i32>, r_spdk: usize, r_huge: usize,
    r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog, ml2: &mut MmapLog,
) -> (Result<(), MtError>, bool, Result<(), MtError>, bool) {
    let mut lg = LogSink { lines: 0 };
    // non-`spdk` build
    let ra = mt_initialize_mmap(a, psz, numa, r_huge, r_plain, rc, ml2, &mut lg, false);
    let da = mt_is_dma_capable(a);
    // `spdk` build, env inactive
    let rb = mt_initialize_spdk(b, psz, numa, false, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false);
    let db = mt_is_dma_capable(b);
    (ra, da, rb, db)
}

/// Anti-vacuity twin: MUST FAIL (claims the huge-page pool reports DMA capability).
#[requires(mt_inv(*a) && !a.initialized && a.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(r_huge != MAP_FAILED && mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz))]
#[requires(ml2.huge_calls@ < u64::MAX@ && ml2.plain_calls@ < u64::MAX@)]
#[ensures(result.0 == Ok(()))]
#[ensures(result.1)]
pub fn verify_mt_is_dma_capable_hugepage_mmap_understated__mutant(
    a: &mut StateModel, psz: usize, numa: Option<i32>, r_huge: usize, r_plain: usize, rc: i64, ml2: &mut MmapLog,
) -> (Result<(), MtError>, bool) {
    let mut lg = LogSink { lines: 0 };
    let ra = mt_initialize_mmap(a, psz, numa, r_huge, r_plain, rc, ml2, &mut lg, false);
    let da = mt_is_dma_capable(a);
    (ra, da)
}

/// **MT-IS-DMA-CAPABLE-NO-INIT-GUARD** (error-case, divergent)
///
/// "Before the pool is set up the component answers no to being directly usable by storage
/// hardware, because there is no pool memory yet."
/// Proved over every un-set-up state REACHABLE from `Default` (lib.rs:98-114 pin) by any call
/// sequence (failed set-up attempts included): the answer is no. DIVERGENCE CONFIRMED in the
/// same module: the query has no set-up check (lib.rs:610-613 pin) -- it returns the stored flag
/// for ANY state, so an un-set-up state whose flag were true would answer yes (`st2`).
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(!result.0)]
#[ensures(!result.2 ==> !result.1)]
#[ensures(result.3 == st2.spdk_allocated)]
#[ensures(!st2.initialized && st2.spdk_allocated ==> result.3)]
pub fn verify_mt_is_dma_capable_no_init_guard(p: PolicyModel, ops: &[(u8, Key, u32)], psz: usize, st2: &StateModel)
    -> (bool, bool, bool, bool) {
    let mut st = mt_default(p);
    let d0 = mt_is_dma_capable(&st); // Default
    mt_run(&mut st, ops, psz);
    let d1 = mt_is_dma_capable(&st); // after any call sequence
    let d2 = mt_is_dma_capable(st2);
    (d0, d1, st.initialized, d2)
}

/// Anti-vacuity twin: MUST FAIL (claims the answer is guarded by the set-up flag).
#[requires(mt_inv(*st2))]
#[ensures(result == st2.spdk_allocated)]
#[ensures(!st2.initialized ==> !result)]
pub fn verify_mt_is_dma_capable_no_init_guard__mutant(st2: &StateModel) -> bool {
    proof_assert!(init_inv(*st2));
    mt_is_dma_capable(st2)
}

/// **MT-IS-DMA-CAPABLE-POST-IFF-SPDK** (postcondition)
///
/// "The component answers yes to being directly usable by storage hardware exactly when the
/// pool's memory was obtained from the storage runtime's own allocator, and answers no whenever
/// the ordinary memory-mapping fallback was used, so a yes answer is a reliable licence to hand
/// cache addresses straight to a device."
/// After a successful set-up in an `spdk` build (`mt_initialize_spdk`, all OS/SPDK results as
/// inputs): yes <==> the pool base is the `spdk_zmalloc(.., SPDK_MALLOC_DMA)` result (the zmalloc
/// call was issued for exactly this pool size), no <==> it is the mmap result. A non-`spdk`
/// build always answers no (`mt_initialize_mmap`).
#[requires(mt_inv(*st) && !st.initialized && st.policy.pools@ < u64::MAX@)]
#[requires(mt_inv(*st2) && !st2.initialized && st2.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[requires(ml2.huge_calls@ < u64::MAX@ && ml2.plain_calls@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => (result.1 == env_active)
          && (result.1 ==> (^st).pool_base == r_spdk && (^zl).calls@ == zl.calls@ + 1 && (^zl).size == psz)
          && (!result.1 ==> (^st).pool_base == mmap_base(r_huge, r_plain)), Err(_) => true })]
#[ensures(match result.2 { Ok(_) => !result.3 && (^st2).pool_base == mmap_base(r_huge, r_plain), Err(_) => true })]
pub fn verify_mt_is_dma_capable_post_iff_spdk(
    st: &mut StateModel, st2: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize,
    r_huge: usize, r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog, ml2: &mut MmapLog,
) -> (Result<(), MtError>, bool, Result<(), MtError>, bool) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_spdk(st, psz, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false);
    let d = mt_is_dma_capable(st);
    let r2 = mt_initialize_mmap(st2, psz, numa, r_huge, r_plain, rc, ml2, &mut lg, false);
    let d2 = mt_is_dma_capable(st2);
    (r, d, r2, d2)
}

/// Anti-vacuity twin: MUST FAIL (claims an mmap-backed pool answers yes).
#[requires(mt_inv(*st) && !st.initialized && st.policy.pools@ < u64::MAX@)]
#[requires(psz@ > 0 && psz@ + 4096 <= usize::MAX@)]
#[requires(mmap_ok(r_huge, psz) && mmap_ok(r_plain, psz) && zmalloc_ok(r_spdk, psz))]
#[requires(ml.huge_calls@ < u64::MAX@ && ml.plain_calls@ < u64::MAX@ && zl.calls@ < u64::MAX@)]
#[ensures(match result.0 { Ok(_) => result.1 == env_active, Err(_) => true })]
#[ensures(match result.0 { Ok(_) => (^st).pool_base == mmap_base(r_huge, r_plain) ==> result.1, Err(_) => true })]
pub fn verify_mt_is_dma_capable_post_iff_spdk__mutant(
    st: &mut StateModel, psz: usize, numa: Option<i32>, env_active: bool, r_spdk: usize, r_huge: usize,
    r_plain: usize, rc: i64, ml: &mut MmapLog, zl: &mut ZmLog,
) -> (Result<(), MtError>, bool) {
    let mut lg = LogSink { lines: 0 };
    let r = mt_initialize_spdk(st, psz, numa, env_active, r_spdk, r_huge, r_plain, rc, ml, zl, &mut lg, false);
    let d = mt_is_dma_capable(st);
    (r, d)
}

/// **MT-IS-DMA-CAPABLE-PRE** (precondition, spec-only)
///
/// "The question of whether the pool's memory can be used directly by storage hardware may be
/// asked at any time and always answers yes or no rather than failing."
/// Before set-up (`Default`), right after a set-up attempt, and after ANY later call sequence the
/// query returns a bool with no precondition at all (no set-up check, lib.rs:610-613 pin).
/// SCOPE: unpoisoned `state` lock (`state.read().unwrap()`).
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(!result.0)]
#[ensures(match result.1 { Ok(_) => true, Err(_) => !result.2 })]
#[ensures(result.3 == result.4)]
pub fn verify_mt_is_dma_capable_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (bool, Result<(), MtError>, bool, bool, bool) {
    let mut st = mt_default(p);
    let d0 = mt_is_dma_capable(&st); // before set-up
    let r = mt_initialize(&mut st, psz);
    let d1 = mt_is_dma_capable(&st); // after the set-up attempt
    mt_run(&mut st, ops, psz);
    let d2 = mt_is_dma_capable(&st); // at any later point
    (d0, r, d1, d2, st.spdk_allocated)
}

/// Anti-vacuity twin: MUST FAIL (claims the query answers yes before set-up).
#[requires(p.tracked@.len() == 0)]
#[ensures(result.1 == false)]
#[ensures(result.0)]
pub fn verify_mt_is_dma_capable_pre__mutant(p: PolicyModel) -> (bool, bool) {
    let st = mt_default(p);
    let d0 = mt_is_dma_capable(&st);
    (d0, st.initialized)
}

/// **MT-OLDEST-KEYS-ERR-EMPTY** (error-case)
///
/// "Asking for zero oldest keys, or asking at all before the pool has been set up, returns an
/// empty list and changes nothing, rather than being treated as a request for all keys or as an
/// error." The early return (lib.rs:426-427 pin) precedes the policy call: the whole state,
/// policy included (ghost `last_pool` too), is identical, and the trace holds no policy call.
#[requires(mt_inv(*st))]
#[requires(n@ == 0 || !st.initialized)]
#[ensures(result.0@ == Seq::empty())]
#[ensures(^st == *st)]
#[ensures(result.1@ == Seq::singleton(LkEv::StateRead))]
pub fn verify_mt_oldest_keys_err_empty(st: &mut StateModel, n: usize) -> (Vec<Key>, Vec<LkEv>) {
    let init = st.initialized;
    let r = mt_oldest_keys(st, n);
    let t = lk_oldest_keys(init, n > 0);
    (r, t)
}

/// Anti-vacuity twin: MUST FAIL (claims n == 0 is read as "all keys").
#[requires(mt_inv(*st) && st.initialized && st.policy.tracked@.len() > 0)]
#[ensures(^st == *st)]
#[ensures(result@.len() == st.policy.tracked@.len())]
pub fn verify_mt_oldest_keys_err_empty__mutant(st: &mut StateModel) -> Vec<Key> {
    mt_oldest_keys(st, 0)
}

/// **MT-OLDEST-KEYS-POST-SHAPE** (postcondition)
///
/// "The oldest-keys query obtains its whole answer from a single request to the external
/// eviction-policy component for this pool, returning no more keys than the caller asked for and
/// no more than the policy is currently tracking, ordered with the entry that would be evicted
/// first at the front and progressively newer entries after it."
/// One policy call (trace = [state.read, get_eviction_candidates]) for THIS pool (`last_pool ==
/// pool_id`); length == min(n, tracked); the answer is the policy order's prefix (index 0 = LRU
/// head, increasing index = more recently used); and the front IS what the next eviction takes.
#[requires(mt_inv(*st) && st.initialized && n@ > 0)]
#[ensures(result.1@ == Seq::empty().push_back(LkEv::StateRead).push_back(LkEv::PolicyOther))]
#[ensures(*result.3 == st.pool_id)]
#[ensures(result.0@.len() <= n@ && result.0@.len() <= st.policy.tracked@.len())]
#[ensures(result.0@.len() == if n@ < st.policy.tracked@.len() { n@ } else { st.policy.tracked@.len() })]
#[ensures(forall<i: Int> 0 <= i && i < result.0@.len() ==> result.0@[i] == (st.policy.tracked@[i]).0)]
#[ensures(result.0@.len() > 0 ==> result.2 == Some(result.0@[0]))]
pub fn verify_mt_oldest_keys_post_shape(st: &mut StateModel, n: usize)
    -> (Vec<Key>, Vec<LkEv>, Option<Key>, Snapshot<u64>) {
    let r = mt_oldest_keys(st, n);
    let lp = snapshot! { st.policy.last_pool };
    let t = lk_oldest_keys(true, true);
    proof_assert!(r@.len() > 0 ==> tracks(st.policy.tracked@, r@[0]));
    proof_assert!(r@.len() > 0 ==> slot_at(st.slots@, r@[0]));
    let e = mt_evict_next(st); // what the next eviction takes
    (r, t, e, lp)
}

/// Anti-vacuity twin: MUST FAIL (claims the answer always has n entries).
#[requires(mt_inv(*st) && st.initialized && n@ > 0)]
#[ensures(result@.len() <= n@)]
#[ensures(result@.len() == n@)]
pub fn verify_mt_oldest_keys_post_shape__mutant(st: &mut StateModel, n: usize) -> Vec<Key> {
    mt_oldest_keys(st, n)
}

/// **MT-OLDEST-KEYS-PRE** (precondition, spec-only)
///
/// "The query for the oldest keys may be called with any requested count, including zero or a
/// count larger than the number of entries held, and it never fails outright."
/// Any `n` (no precondition on it), before set-up and after a set-up attempt plus ANY call
/// sequence: a list of exactly min(n, tracked) keys when set up, empty otherwise. SCOPE:
/// unpoisoned locks, eviction_policy receptacle connected (lib.rs:430 pin `.unwrap()`).
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0@ == Seq::empty())]
#[ensures(result.1@.len() <= n@ && result.1@.len() <= result.2@)]
#[ensures(result.3 && n@ > 0 ==> result.1@.len() == if n@ < result.2@ { n@ } else { result.2@ })]
#[ensures(!result.3 || n@ == 0 ==> result.1@ == Seq::empty())]
pub fn verify_mt_oldest_keys_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)], n: usize)
    -> (Vec<Key>, Vec<Key>, usize, bool) {
    let mut st = mt_default(p);
    let a = mt_oldest_keys(&mut st, n); // before set-up
    let _ = mt_initialize(&mut st, psz);
    mt_run(&mut st, ops, psz);
    let tl = st.policy.tracked.len();
    let init = st.initialized;
    let b = mt_oldest_keys(&mut st, n); // any later point, any n
    (a, b, tl, init)
}

/// Anti-vacuity twin: MUST FAIL (claims the query before set-up hands back n keys).
#[requires(p.tracked@.len() == 0 && n@ > 0)]
#[ensures(result@.len() <= n@)]
#[ensures(result@.len() == n@)]
pub fn verify_mt_oldest_keys_pre__mutant(p: PolicyModel, n: usize) -> Vec<Key> {
    let mut st = mt_default(p);
    mt_oldest_keys(&mut st, n)
}

/// **MT-PEEK-ERR-MISS** (error-case, divergent)
///
/// "The non-recency-updating lookup reports that nothing was found both when the cache holds no
/// entry for the key and when the pool has not been set up, rather than returning an address."
/// Proved: either miss case ==> None. DIVERGENCE CONFIRMED: the two cases give the SAME answer
/// (`a`: not set up, `b`: set up without the key), so a caller cannot tell them apart.
#[requires(mt_inv(*st) && (!st.initialized || !slot_at(st.slots@, key)))]
#[requires(mt_inv(*a) && !a.initialized)]
#[requires(mt_inv(*b) && b.initialized && !slot_at(b.slots@, kb))]
#[ensures(result.0 == None)]
#[ensures(result.1 == result.2)]
pub fn verify_mt_peek_err_miss(st: &StateModel, key: Key, a: &StateModel, ka: Key, b: &StateModel, kb: Key)
    -> (Option<(usize, u32)>, Option<(usize, u32)>, Option<(usize, u32)>) {
    (mt_peek(st, key), mt_peek(a, ka), mt_peek(b, kb))
}

/// Anti-vacuity twin: MUST FAIL (claims a miss hands back an address).
#[requires(mt_inv(*st) && (!st.initialized || !slot_at(st.slots@, key)))]
#[ensures(result != None)]
pub fn verify_mt_peek_err_miss__mutant(st: &StateModel, key: Key) -> Option<(usize, u32)> {
    mt_peek(st, key)
}

/// **MT-PEEK-PRE** (precondition, spec-only)
///
/// "The non-recency-updating lookup may be called with any key at any time, including before the
/// pool has been set up, and never fails outright; a missing entry is reported through the
/// return value." Any key; before set-up -> None; after a set-up attempt and ANY call sequence
/// -> Some exactly when the pool is set up and holds the key. SCOPE: unpoisoned locks.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0 == None)]
#[ensures((result.1 != None) == *result.2)]
pub fn verify_mt_peek_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)], key: Key)
    -> (Option<(usize, u32)>, Option<(usize, u32)>, Snapshot<bool>) {
    let mut st = mt_default(p);
    let a = mt_peek(&st, key);
    let _ = mt_initialize(&mut st, psz);
    mt_run(&mut st, ops, psz);
    let b = mt_peek(&st, key);
    let hit = snapshot! { st.initialized && slot_at(st.slots@, key) };
    (a, b, hit)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up lookup finds something).
#[requires(p.tracked@.len() == 0)]
#[ensures(result != None)]
pub fn verify_mt_peek_pre__mutant(p: PolicyModel, key: Key) -> Option<(usize, u32)> {
    let st = mt_default(p);
    mt_peek(&st, key)
}

/// **MT-POOL-INFO-ERR-NOT-INITIALIZED** (error-case)
///
/// "Before the pool is set up, or whenever the stored base address is null, the query reports
/// that there is no pool address to give out rather than returning a null or meaningless
/// address." (lib.rs:585-591 pin) Not set up or null base ==> None; any Some carries a
/// non-null, page-aligned base. (In reachable states a set-up pool's base is never null:
/// `init_inv`.)
#[requires(mt_inv(*st))]
#[ensures(!st.initialized || st.pool_base@ == 0 ==> result == None)]
#[ensures(forall<b: usize, s: usize> result == Some((b, s)) ==> b@ > 0 && b@ % 4096 == 0)]
pub fn verify_mt_pool_info_err_not_initialized(st: &StateModel) -> Option<(usize, usize)> {
    mt_pool_info(st)
}

/// Anti-vacuity twin: MUST FAIL (claims an un-set-up pool reports an address).
#[requires(mt_inv(*st))]
#[ensures(forall<b: usize, s: usize> result == Some((b, s)) ==> b@ > 0)]
#[ensures(!st.initialized ==> result != None)]
pub fn verify_mt_pool_info_err_not_initialized__mutant(st: &StateModel) -> Option<(usize, usize)> {
    mt_pool_info(st)
}

/// **MT-POOL-INFO-FRAME** (frame)
///
/// "Asking for the pool's base address and size changes nothing about the cache, the eviction
/// order or the memory itself, and the address and size it reports never change after the pool
/// has been set up." The query takes the state by shared reference (lib.rs:585-591 pin: only
/// `state.read()`, no pool lock, no policy call); two back-to-back answers agree, and after ANY
/// call sequence on the set-up pool the answer is the same (base, size).
#[requires(mt_inv(*st) && st.initialized && !st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0 == result.1 && result.1 == result.2)]
#[ensures(result.0 == Some((st.pool_base, st.pool_size)))]
pub fn verify_mt_pool_info_frame(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize)
    -> (Option<(usize, usize)>, Option<(usize, usize)>, Option<(usize, usize)>) {
    let a = mt_pool_info(st);
    let b = mt_pool_info(st);
    mt_run(st, ops, psz);
    let c = mt_pool_info(st);
    (a, b, c)
}

/// Anti-vacuity twin: MUST FAIL (claims the reported base moves after later calls).
#[requires(mt_inv(*st) && st.initialized && !st.poisoned)]
#[requires(st.policy.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(st.policy.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0 == Some((st.pool_base, st.pool_size)))]
#[ensures(result.1 != result.0)]
pub fn verify_mt_pool_info_frame__mutant(st: &mut StateModel, ops: &[(u8, Key, u32)], psz: usize)
    -> (Option<(usize, usize)>, Option<(usize, usize)>) {
    let a = mt_pool_info(st);
    mt_run(st, ops, psz);
    let c = mt_pool_info(st);
    (a, c)
}

/// **MT-POOL-INFO-PRE** (precondition, spec-only)
///
/// "The query for the pool's base address and size may be called at any time and reports the
/// absence of a pool through its return value instead of failing." Before set-up -> None; after
/// a set-up attempt and ANY call sequence -> Some((base, size)) exactly when set up.
/// SCOPE: unpoisoned `state` lock.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0 == None)]
#[ensures(match result.1 { Ok(_) => result.2 == Some((*result.4, psz)) && *result.5 == *result.4,
          Err(_) => result.2 == None })]
#[ensures(*result.6 ==> result.3 == Some((*result.5, *result.7)))]
#[ensures(!*result.6 ==> result.3 == None)]
pub fn verify_mt_pool_info_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)])
    -> (Option<(usize, usize)>, Result<(), MtError>, Option<(usize, usize)>, Option<(usize, usize)>,
        Snapshot<usize>, Snapshot<usize>, Snapshot<bool>, Snapshot<usize>) {
    let mut st = mt_default(p);
    let a = mt_pool_info(&st);
    let r = mt_initialize(&mut st, psz);
    let b = mt_pool_info(&st);
    let base1 = snapshot! { st.pool_base };
    mt_run(&mut st, ops, psz);
    let c = mt_pool_info(&st);
    let base2 = snapshot! { st.pool_base };
    let init2 = snapshot! { st.initialized };
    let size2 = snapshot! { st.pool_size };
    (a, r, b, c, base1, base2, init2, size2)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up query reports an address).
#[requires(p.tracked@.len() == 0)]
#[ensures(result != None)]
pub fn verify_mt_pool_info_pre__mutant(p: PolicyModel) -> Option<(usize, usize)> {
    let st = mt_default(p);
    mt_pool_info(&st)
}

/// **MT-REMOVE-ERR-KEY-NOT-FOUND** (error-case)
///
/// "Attempting to delete a key that is not in the cache fails with the key-not-found error
/// naming that key, and nothing about the cache is freed or changed."
/// `slots.remove(&key).ok_or(KeyNotFound(key))?` (lib.rs:496-499 pin) returns before
/// `ep.remove` / `deallocate`: Err(KeyNotFound), the full state (allocator, slots, policy) is
/// unchanged, and no policy call is made. (The error's key payload is not modelled.)
#[requires(mt_inv(*st) && st.initialized && !slot_at(st.slots@, key))]
#[ensures(result.0 == Err(MtError::KeyNotFound))]
#[ensures(same_state(*st, ^st))]
#[ensures(forall<i: Int> 0 <= i && i < result.1@.len()
          ==> result.1@[i] != LkEv::PolicyRefresh && result.1@[i] != LkEv::PolicyOther)]
pub fn verify_mt_remove_err_key_not_found(st: &mut StateModel, key: Key) -> (Result<(), MtError>, Vec<LkEv>) {
    let r = mt_remove(st, key);
    let t = lk_remove(true, false);
    (r, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the failed delete freed space).
#[requires(mt_inv(*st) && st.initialized && !slot_at(st.slots@, key))]
#[ensures(result == Err(MtError::KeyNotFound))]
#[ensures((^st).allocator.used@ < st.allocator.used@)]
pub fn verify_mt_remove_err_key_not_found__mutant(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    mt_remove(st, key)
}

/// **MT-REMOVE-ERR-NOT-INITIALIZED** (error-case)
///
/// "Attempting to delete an entry before the pool has been set up fails with the
/// not-initialized error carrying the message that the pool is not initialized."
/// (lib.rs:473-478 pin) Err(NotInitialized), state identical, trace = [state.read()] only.
/// The message text is not modelled.
#[requires(mt_inv(*st) && !st.initialized)]
#[ensures(match result.0 { Err(MtError::NotInitialized) => true, _ => false })]
#[ensures(^st == *st)]
#[ensures(result.1@ == Seq::singleton(LkEv::StateRead))]
pub fn verify_mt_remove_err_not_initialized(st: &mut StateModel, key: Key) -> (Result<(), MtError>, Vec<LkEv>) {
    let present = slots_contains(&st.slots, key);
    let r = mt_remove(st, key);
    let t = lk_remove(false, present);
    (r, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up delete reports key-not-found).
#[requires(mt_inv(*st) && !st.initialized)]
#[ensures(^st == *st)]
#[ensures(result == Err(MtError::KeyNotFound))]
pub fn verify_mt_remove_err_not_initialized__mutant(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    mt_remove(st, key)
}

/// **MT-REMOVE-PRE** (precondition)
///
/// "To delete a named entry successfully the pool must already have been set up and the cache
/// must currently hold an entry for that key." Necessity (Ok ==> set up and present) and
/// sufficiency (set up and present ==> Ok).
#[requires(mt_inv(*st))]
#[ensures(match result { Ok(_) => st.initialized && slot_at(st.slots@, key), Err(_) => true })]
#[ensures(st.initialized && slot_at(st.slots@, key) ==> match result { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_remove_pre(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    mt_remove(st, key)
}

/// Anti-vacuity twin: MUST FAIL (claims a set-up pool deletes any key).
#[requires(mt_inv(*st))]
#[ensures(match result { Ok(_) => st.initialized, Err(_) => true })]
#[ensures(st.initialized ==> match result { Ok(_) => true, Err(_) => false })]
pub fn verify_mt_remove_pre__mutant(st: &mut StateModel, key: Key) -> Result<(), MtError> {
    mt_remove(st, key)
}

/// **MT-TELEMETRY-NO-CONTENTION-WHEN-UNCONTENDED** (postcondition, spec-only)
///
/// "When only one thread has ever used the component, both lock-contention counters in the
/// snapshot remain zero, because a contention is recorded only when an operation fails to take
/// the lock on its first attempt."
/// Single-threaded use = every call starts with no pool guard live (each method drops its guard
/// before returning) and, with no prior panic, an unpoisoned pool lock. ANY sequence of calls,
/// each running its pool-lock site (`try_write` / `try_read` with the counter on failure, or an
/// uncounted `read()`/`write()`): both contention counters stay 0 and the snapshot reports 0, 0.
#[requires(lk_free(*l))]
#[requires(t.write_lock_contentions@ == 0 && t.read_lock_contentions@ == 0)]
#[ensures((^t).write_lock_contentions@ == 0 && (^t).read_lock_contentions@ == 0)]
#[ensures(result.1@ == 0 && result.2@ == 0)]
#[ensures(lk_free(^l))]
pub fn verify_mt_telemetry_no_contention_when_uncontended(l: &mut PoolLockModel, t: &mut TelemetryModel, ops: &[u8])
    -> (u64, u64, u64) {
    let n = ops.len();
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(lk_free(*l))]
    #[invariant(t.write_lock_contentions@ == 0 && t.read_lock_contentions@ == 0)]
    while i < n {
        tel_pool_site(l, t, ops[i]);
        i += 1;
    }
    mt_telemetry_snapshot_m(t, true)
}

/// Anti-vacuity twin: MUST FAIL -- the same claim with ONE guard live at a call boundary (a
/// second thread holding the pool read lock): an insert's try_write fails and is counted.
#[requires(l.readers@ == 1 && !l.writer && !l.poisoned)]
#[requires(t.write_lock_contentions@ == 0 && t.read_lock_contentions@ == 0)]
#[ensures((^t).read_lock_contentions@ == 0)]
#[ensures((^t).write_lock_contentions@ == 0)]
pub fn verify_mt_telemetry_no_contention_when_uncontended__mutant(l: &mut PoolLockModel, t: &mut TelemetryModel) {
    tel_pool_site(l, t, 0);
}

/// UNSCORED witness: a POISONED pool lock (after a panic inside a pool guard, e.g. the
/// insert/track unwrap) makes a single thread bump a contention counter -- try_write returns
/// `Err(Poisoned)`, which the code counts as contention before `write().unwrap()` panics.
#[requires(!l.writer && l.readers@ == 0 && l.poisoned)]
#[requires(t.write_lock_contentions@ == 0)]
#[ensures((^t).write_lock_contentions@ == 1)]
pub fn witness_mt_telemetry_no_contention_poisoned(l: &mut PoolLockModel, t: &mut TelemetryModel) {
    tel_pool_site(l, t, 0);
}

/// **MT-TELEMETRY-SNAPSHOT-FRAME** (frame)
///
/// "Reading the telemetry snapshot neither resets nor otherwise alters the counters, does not
/// touch the cache contents and does not affect the eviction order, so two readings taken with
/// no activity in between are identical." (lib.rs:615-635 pin: three `load`s under
/// `state.read()`, no pool lock, no policy call.) Either build.
#[requires(mt_inv(*st))]
#[ensures(result.0 == result.1)]
#[ensures(^t == *t)]
#[ensures(^st == *st)]
pub fn verify_mt_telemetry_snapshot_frame(st: &mut StateModel, t: &mut TelemetryModel, feature: bool)
    -> ((u64, u64, u64), (u64, u64, u64)) {
    let a = mt_telemetry_snapshot_m(t, feature);
    let b = mt_telemetry_snapshot_m(t, feature);
    let _ = st;
    (a, b)
}

/// Anti-vacuity twin: MUST FAIL (claims a reading resets the counters).
#[requires(t.evictions@ > 0)]
#[ensures(result.0 == trip(*t))]
#[ensures(result.1 == (0u64, 0u64, 0u64))]
pub fn verify_mt_telemetry_snapshot_frame__mutant(t: &mut TelemetryModel) -> ((u64, u64, u64), (u64, u64, u64)) {
    proof_assert!(t.evictions@ > 0);
    let a = mt_telemetry_snapshot_m(t, true);
    let b = mt_telemetry_snapshot_m(t, true);
    (a, b)
}

/// **MT-TELEMETRY-SNAPSHOT-POST** (postcondition)
///
/// "The telemetry snapshot reports the current cumulative counts of evictions and of write- and
/// read-lock contentions when the optional counter-tracking option was built in, and reports
/// zero for every counter when it was not, so callers always see a well-defined reading rather
/// than stale or invented numbers."
/// Feature on: exactly the three current counter values; across an `evict_next` the eviction
/// reading grows by exactly the eviction it counted (cumulative, no wrap below u64::MAX).
/// Feature off: (0, 0, 0) always. SEQUENTIAL-ONLY (three separate loads -- see
/// MT-TELEMETRY-INHERENT-POST for the torn read).
#[requires(mt_inv(*st) && t.evictions@ + 1 < u64::MAX@)]
#[ensures(feature ==> result.0 == trip(*t) && result.2 == trip(^t))]
#[ensures(feature ==> (result.2).0@ == (result.0).0@ + match result.1 { Some(_) => 1, None => 0 })]
#[ensures(feature ==> (result.2).1 == (result.0).1 && (result.2).2 == (result.0).2)]
#[ensures(!feature ==> result.0 == (0u64, 0u64, 0u64) && result.2 == (0u64, 0u64, 0u64))]
pub fn verify_mt_telemetry_snapshot_post(st: &mut StateModel, t: &mut TelemetryModel, feature: bool)
    -> ((u64, u64, u64), Option<Key>, (u64, u64, u64)) {
    let a = mt_telemetry_snapshot_m(t, feature);
    let v = mt_evict_next_tel(st, t);
    let b = mt_telemetry_snapshot_m(t, feature);
    (a, v, b)
}

/// Anti-vacuity twin: MUST FAIL (claims a feature-less build reports the counters).
#[requires(t.evictions@ > 0)]
#[ensures(result == (0u64, 0u64, 0u64))]
#[ensures(result == trip(*t))]
pub fn verify_mt_telemetry_snapshot_post__mutant(t: &TelemetryModel) -> (u64, u64, u64) {
    proof_assert!(t.evictions@ > 0);
    mt_telemetry_snapshot_m(t, false)
}

/// **MT-TELEMETRY-SNAPSHOT-PRE** (precondition, spec-only)
///
/// "The telemetry snapshot may be requested at any time, including before the pool has been set
/// up and regardless of whether the optional counter-tracking build option was enabled, and it
/// always returns a set of numbers rather than failing."
/// No precondition at all, either build: before set-up (`Default`) and after a set-up attempt
/// the call returns a triple (the counters, or zeros). SCOPE: unpoisoned `state` lock (feature
/// build; the feature-less build takes no lock).
#[requires(p.tracked@.len() == 0)]
#[requires(p.pools@ + 1 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(feature ==> result.0 == trip(*t) && result.2 == trip(*t))]
#[ensures(!feature ==> result.0 == (0u64, 0u64, 0u64) && result.2 == (0u64, 0u64, 0u64))]
#[ensures(!result.3)]
pub fn verify_mt_telemetry_snapshot_pre(p: PolicyModel, t: &TelemetryModel, feature: bool, psz: usize)
    -> ((u64, u64, u64), Result<(), MtError>, (u64, u64, u64), bool) {
    let mut st = mt_default(p);
    let init0 = st.initialized;
    let a = mt_telemetry_snapshot_m(t, feature); // before set-up
    let r = mt_initialize(&mut st, psz);
    let b = mt_telemetry_snapshot_m(t, feature); // after the set-up attempt
    (a, r, b, init0)
}

/// Anti-vacuity twin: MUST FAIL (claims the feature-less reading echoes live counters).
#[requires(p.tracked@.len() == 0 && t.write_lock_contentions@ > 0)]
#[ensures(!result.1)]
#[ensures(result.0 == trip(*t))]
pub fn verify_mt_telemetry_snapshot_pre__mutant(p: PolicyModel, t: &TelemetryModel) -> ((u64, u64, u64), bool) {
    let st = mt_default(p);
    let a = mt_telemetry_snapshot_m(t, false);
    (a, st.initialized)
}

/// **MT-TOUCH-PRE** (precondition, spec-only)
///
/// "Refreshing an entry's recency may be requested for any key at any time; it returns nothing
/// and reports no error, so the caller needs no prior check that the key exists."
/// Any key, no precondition beyond the reachable-state invariant: before set-up the call leaves
/// the `Default` state untouched; after a set-up attempt and ANY call sequence it returns
/// (unit) and the invariant still holds. SCOPE: unpoisoned locks, receptacle connected
/// (lib.rs:511 pin `.unwrap()`).
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(*result.0 == *result.1)]
#[ensures(mt_inv(*result.2))]
pub fn verify_mt_touch_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)], key: Key)
    -> (Snapshot<StateModel>, Snapshot<StateModel>, Snapshot<StateModel>) {
    let mut st = mt_default(p);
    let s0 = snapshot! { st };
    mt_touch(&mut st, key); // before set-up
    let s1 = snapshot! { st };
    let _ = mt_initialize(&mut st, psz);
    mt_run(&mut st, ops, psz);
    mt_touch(&mut st, key); // any later point, any key
    let s2 = snapshot! { st };
    (s0, s1, s2)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up refresh changed something).
#[requires(p.tracked@.len() == 0)]
#[ensures(result.0.initialized == result.1.initialized)]
#[ensures(*result.0 != *result.1)]
pub fn verify_mt_touch_pre__mutant(p: PolicyModel, key: Key) -> (Snapshot<StateModel>, Snapshot<StateModel>) {
    let mut st = mt_default(p);
    let s0 = snapshot! { st };
    mt_touch(&mut st, key);
    let s1 = snapshot! { st };
    (s0, s1)
}

/// **MT-TOUCH-SILENT-NOOP** (error-case, divergent)
///
/// "Refreshing a key that is not in the cache, or refreshing before the pool has been set up,
/// does nothing at all and reports nothing, so a caller has no way to discover that its refresh
/// was dropped." (lib.rs:505-518 pin) Either case: the complete state (policy order and ghost
/// `last_pool` included) is identical, the trace holds no policy call, and the call returns unit
/// -- the same (only) answer a successful refresh gives.
#[requires(mt_inv(*st) && (!st.initialized || !slot_at(st.slots@, key)))]
#[ensures(^st == *st)]
#[ensures(forall<i: Int> 0 <= i && i < result.1@.len()
          ==> result.1@[i] != LkEv::PolicyRefresh && result.1@[i] != LkEv::PolicyOther)]
#[ensures(result.0 == ())]
pub fn verify_mt_touch_silent_noop(st: &mut StateModel, key: Key) -> ((), Vec<LkEv>) {
    let init = st.initialized;
    let present = slots_contains(&st.slots, key);
    let u = mt_touch(st, key);
    let t = lk_touch(init, present);
    (u, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the dropped refresh reordered the policy).
#[requires(mt_inv(*st) && st.initialized && !slot_at(st.slots@, key) && st.policy.tracked@.len() > 1)]
#[ensures((^st).slots@ == st.slots@)]
#[ensures((^st).policy.tracked@ != st.policy.tracked@)]
pub fn verify_mt_touch_silent_noop__mutant(st: &mut StateModel, key: Key) {
    mt_touch(st, key)
}

/// **MT-USED-ERR-NOT-INITIALIZED** (error-case)
///
/// "Before the pool has been set up the bytes-in-use query reports zero." (lib.rs:576-583 pin)
#[requires(mt_inv(*st) && !st.initialized)]
#[ensures(result@ == 0)]
pub fn verify_mt_used_err_not_initialized(st: &StateModel) -> usize {
    mt_used(st)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up query reports bytes in use).
#[requires(mt_inv(*st) && !st.initialized)]
#[ensures(result@ <= 0)]
#[ensures(result@ > 0)]
pub fn verify_mt_used_err_not_initialized__mutant(st: &StateModel) -> usize {
    mt_used(st)
}

/// **MT-USED-FRAME** (frame)
///
/// "Asking how many bytes are in use changes nothing about the cache or the eviction order."
/// The call takes the state by shared reference (lib.rs:576-583 pin reads under `state.read()` /
/// `pool.read()` and makes no `ep` call -- the same lock skeleton as peek/contains/capacity):
/// the complete state, policy order included, is unchanged; two answers agree.
#[requires(mt_inv(*st))]
#[ensures(^st == *st)]
#[ensures(result.0 == result.1)]
#[ensures(forall<i: Int> 0 <= i && i < result.2@.len()
          ==> result.2@[i] != LkEv::PolicyRefresh && result.2@[i] != LkEv::PolicyOther)]
#[ensures(no_pool_write(result.2@))]
pub fn verify_mt_used_frame(st: &mut StateModel) -> (usize, usize, Vec<LkEv>) {
    let u0 = mt_used(st);
    let u1 = mt_used(st);
    let t = lk_peek_or_contains(st.initialized);
    (u0, u1, t)
}

/// Anti-vacuity twin: MUST FAIL (claims the query refreshes the eviction order).
#[requires(mt_inv(*st))]
#[requires(st.initialized && st.policy.tracked@.len() > 1)]
#[ensures(result@ == st.allocator.used@)]
#[ensures((^st).policy.tracked@ != st.policy.tracked@)]
pub fn verify_mt_used_frame__mutant(st: &mut StateModel) -> usize {
    mt_used(st)
}

/// **MT-USED-PRE** (precondition, spec-only)
///
/// "The bytes-in-use query may be called at any time and always returns a number rather than
/// failing." Before set-up (0), right after a set-up attempt (0 either way: a fresh pool holds
/// nothing), and after ANY later call sequence (the allocator's used bytes, <= capacity).
/// SCOPE: unpoisoned locks.
#[requires(p.tracked@.len() == 0)]
#[requires(p.next_handle@ + ops@.len() + 2 < u64::MAX@)]
#[requires(p.pools@ + ops@.len() + 2 < u64::MAX@)]
#[requires(psz@ > 0 ==> psz@ + 4096 <= usize::MAX@)]
#[ensures(result.0@ == 0 && result.1@ == 0)]
#[ensures(result.3 ==> result.2 == result.4)]
#[ensures(!result.3 ==> result.2@ == 0)]
pub fn verify_mt_used_pre(p: PolicyModel, psz: usize, ops: &[(u8, Key, u32)]) -> (usize, usize, usize, bool, usize) {
    let mut st = mt_default(p);
    let u0 = mt_used(&st);
    let _ = mt_initialize(&mut st, psz);
    let u1 = mt_used(&st);
    mt_run(&mut st, ops, psz);
    let u2 = mt_used(&st);
    (u0, u1, u2, st.initialized, st.allocator.used)
}

/// Anti-vacuity twin: MUST FAIL (claims the un-set-up query reports a non-zero number).
#[requires(p.tracked@.len() == 0)]
#[ensures(result.1 == false)]
#[ensures(result.0@ > 0)]
pub fn verify_mt_used_pre__mutant(p: PolicyModel) -> (usize, bool) {
    let st = mt_default(p);
    let u0 = mt_used(&st);
    (u0, st.initialized)
}

//! Kani verification harnesses for the `memory-tier` component.
//!
//! ADDITIVE, `#[cfg(kani)]`-ONLY. This module is compiled only under `cargo kani`
//! (`lib.rs` gates it with `#[cfg(kani)] mod verification;`). No production line is
//! altered by its presence.
//!
//! Each harness is named `verify_<property-id>` with the id lowercased and `-`→`_`,
//! so the shipped scorer finds it by convention. Anti-vacuity twins carry the
//! `__mutant` suffix and MUST FAIL.
#![allow(clippy::all)]
#![allow(dead_code)]

use std::collections::hash_map::RandomState;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering as AOrd};
use std::sync::{Arc, RwLock};

use interfaces::{
    BlockSemantics, CacheKey, EvictionHandle, EvictionPolicyError, IEvictionPolicy, IMemoryTier,
    MemoryTierError, PoolId,
};

use crate::allocator::FreeList;
use crate::{MemoryTierComponent, MemoryTierState};
use std::sync::atomic::Ordering;

/// The 4 KiB alignment the allocator and the DMA contract are stated in terms of
/// (`allocator.rs:5`, spec FR-004 / NFR-010).
const ALIGN: usize = 4096;

// =====================================================================================
// Stubs. Each is disclosed in the advisory `note` of every property that depends on it.
// =====================================================================================

/// Deterministic `RandomState` so `HashMap::new()` (`lib.rs:104`, `lib.rs:301`, and the
/// `InterfaceMap` inside `define_component!`) does not reach `getrandom`/`syscall`.
/// Sound for map *semantics* only — nothing here asserts anything about hash order.
fn concrete_random_state() -> RandomState {
    let keys: [u64; 2] = [0, 0];
    assert!(std::mem::size_of::<[u64; 2]>() == std::mem::size_of::<RandomState>());
    // SAFETY: size-checked above; RandomState is two u64 SipHash keys.
    unsafe { std::mem::transmute(keys) }
}

// `Drop for MemoryTierState` (lib.rs:112-131) calls `libc::munmap`, a foreign function.
// Every component harness ends in `std::mem::forget(c)` so that path is never entered --
// cheaper and more faithful than stubbing munmap.

// =====================================================================================
// A minimal in-crate `IEvictionPolicy` provider.
//
// `define_interface!` expands to a plain `trait: Send + Sync + 'static`, so no component
// machinery is needed to supply one. This one is INSTRUMENTED: it counts the policy calls
// memory-tier makes, which is what lets a harness observe which calls the component actually
// performs rather than inferring it.
//
// CONFORMANCE: `StubPolicy::new(true)` satisfies the `IEvictionPolicy` contract clauses the
// proofs rely on -- in particular `identify_next_to_evict` removes its own victim from
// tracking (interfaces/src/ieviction_policy.rs:101-103), as the shipped `eviction-policy-lru`
// does. That conformance is pinned by `check_stub_conforms_ieviction_policy` and EVERY proof
// here uses it. `new(false)` is deliberately NON-conforming and exists ONLY as the negative
// control inside that conformance harness; it must never be used to establish a property of
// memory-tier, because a violation produced by a contract-breaking stub is a property of the
// stub, not of the component.
// =====================================================================================

const STUB_SLOTS: usize = 4;

pub(crate) struct StubPolicy {
    track_calls: AtomicUsize,
    remove_calls: AtomicUsize,
    touch_calls: AtomicUsize,
    clear_calls: AtomicUsize,
    keys: [AtomicU64; STUB_SLOTS],
    live: [AtomicBool; STUB_SLOTS],
    next_idx: AtomicU32,
    /// When true the policy untracks its own victim inside `identify_next_to_evict`,
    /// as `IEvictionPolicy`'s doc comment (`ieviction_policy.rs:101-103`) requires.
    /// When false it does not — the case the divergence note calls out.
    untrack_own_victim: bool,
}

impl StubPolicy {
    pub(crate) fn new(untrack_own_victim: bool) -> Self {
        Self {
            track_calls: AtomicUsize::new(0),
            remove_calls: AtomicUsize::new(0),
            touch_calls: AtomicUsize::new(0),
            clear_calls: AtomicUsize::new(0),
            keys: [
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
                AtomicU64::new(0),
            ],
            live: [
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            ],
            next_idx: AtomicU32::new(0),
            untrack_own_victim,
        }
    }

    pub(crate) fn track_calls(&self) -> usize {
        self.track_calls.load(AOrd::SeqCst)
    }
    pub(crate) fn remove_calls(&self) -> usize {
        self.remove_calls.load(AOrd::SeqCst)
    }
    pub(crate) fn touch_calls(&self) -> usize {
        self.touch_calls.load(AOrd::SeqCst)
    }
    /// Is `key` still in the policy's bookkeeping?
    pub(crate) fn is_tracked(&self, key: CacheKey) -> bool {
        let mut i = 0usize;
        while i < STUB_SLOTS {
            if self.live[i].load(AOrd::SeqCst) && self.keys[i].load(AOrd::SeqCst) == key {
                return true;
            }
            i += 1;
        }
        false
    }
    fn slot_of(&self, key: CacheKey) -> Option<usize> {
        let mut i = 0usize;
        while i < STUB_SLOTS {
            if self.live[i].load(AOrd::SeqCst) && self.keys[i].load(AOrd::SeqCst) == key {
                return Some(i);
            }
            i += 1;
        }
        None
    }
    /// Oldest live slot (FIFO == LRU for a policy that is never touched).
    fn oldest(&self) -> Option<usize> {
        let mut i = 0usize;
        while i < STUB_SLOTS {
            if self.live[i].load(AOrd::SeqCst) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

impl IEvictionPolicy for StubPolicy {
    fn create_pool(&self) -> PoolId {
        0
    }

    fn track(
        &self,
        pool: PoolId,
        key: CacheKey,
        _semantics: BlockSemantics,
    ) -> Result<EvictionHandle, EvictionPolicyError> {
        self.track_calls.fetch_add(1, AOrd::SeqCst);
        if let Some(i) = self.slot_of(key) {
            return Ok(EvictionHandle::new(pool, i as u32));
        }
        let i = self.next_idx.fetch_add(1, AOrd::SeqCst) as usize;
        if i >= STUB_SLOTS {
            return Err(EvictionPolicyError::InvalidPool(pool));
        }
        self.keys[i].store(key, AOrd::SeqCst);
        self.live[i].store(true, AOrd::SeqCst);
        Ok(EvictionHandle::new(pool, i as u32))
    }

    fn touch(&self, _handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        self.touch_calls.fetch_add(1, AOrd::SeqCst);
        Ok(())
    }

    fn batch_touch(&self, handles: &[EvictionHandle]) -> Result<(), EvictionPolicyError> {
        self.touch_calls.fetch_add(handles.len(), AOrd::SeqCst);
        Ok(())
    }

    fn remove(&self, handle: EvictionHandle) -> Result<(), EvictionPolicyError> {
        self.remove_calls.fetch_add(1, AOrd::SeqCst);
        let i = handle.index() as usize;
        if i >= STUB_SLOTS {
            return Err(EvictionPolicyError::InvalidHandle);
        }
        self.live[i].store(false, AOrd::SeqCst);
        Ok(())
    }

    fn identify_next_to_evict(&self, _pool: PoolId) -> Option<CacheKey> {
        let i = self.oldest()?;
        let k = self.keys[i].load(AOrd::SeqCst);
        if self.untrack_own_victim {
            self.live[i].store(false, AOrd::SeqCst);
        }
        Some(k)
    }

    fn get_eviction_candidates(&self, _pool: PoolId, n: usize) -> Vec<CacheKey> {
        let mut out = Vec::new();
        let mut i = 0usize;
        while i < STUB_SLOTS && out.len() < n {
            if self.live[i].load(AOrd::SeqCst) {
                out.push(self.keys[i].load(AOrd::SeqCst));
            }
            i += 1;
        }
        out
    }

    fn len(&self, _pool: PoolId) -> usize {
        let mut c = 0usize;
        let mut i = 0usize;
        while i < STUB_SLOTS {
            if self.live[i].load(AOrd::SeqCst) {
                c += 1;
            }
            i += 1;
        }
        c
    }

    fn clear_pool(&self, _pool: PoolId) {
        self.clear_calls.fetch_add(1, AOrd::SeqCst);
        let mut i = 0usize;
        while i < STUB_SLOTS {
            self.live[i].store(false, AOrd::SeqCst);
            i += 1;
        }
    }
}

/// Build a real `MemoryTierComponent` with a real receptacle binding to `StubPolicy`.
fn wire(untrack_own_victim: bool) -> (Arc<MemoryTierComponent>, Arc<StubPolicy>) {
    let pol = Arc::new(StubPolicy::new(untrack_own_victim));
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> = pol.clone();
    let c = MemoryTierComponent::new(RwLock::new(MemoryTierState::default()));
    c.eviction_policy.connect(ep).unwrap();
    (c, pol)
}

fn is_init(c: &MemoryTierComponent) -> bool {
    c.state.read().unwrap().initialized.load(Ordering::Acquire)
}

// #####################################################################################
// A real, page-aligned stand-in for the pool. `#[repr(align(4096))]` reproduces exactly
// what `mmap` / `spdk_zmalloc(_, 4096, ..)` promise, and -- unlike a bare integer cast --
// it is a REAL allocation, so `state.pool_ptr.add(offset)` (lib.rs:383) stays inside an
// object and Kani's pointer-bounds checks are exercised rather than tripped.
// #####################################################################################

#[repr(align(4096))]
struct AlignedPool([u8; 8 * ALIGN]);
static mut FAKE_POOL: AlignedPool = AlignedPool([0u8; 8 * ALIGN]);

fn fake_pool_base() -> *mut u8 {
    #[allow(unused_unsafe)]
    unsafe {
        std::ptr::addr_of_mut!(FAKE_POOL) as *mut u8
    }
}

/// Replaces `MemoryTierComponent::alloc_mmap` (lib.rs:187-253), whose body is
/// `libc::mmap` plus `libc::syscall(SYS_mbind)` -- foreign functions Kani cannot model.
/// Hands back the page-aligned base of a real 3-page object, which is precisely
/// `mmap`'s documented postcondition.
fn stub_alloc_mmap2(
    _c: &MemoryTierComponent,
    _pool_size: usize,
    _numa_node: Option<i32>,
) -> Result<*mut u8, MemoryTierError> {
    Ok(fake_pool_base())
}

// #####################################################################################
// A bump-allocator model for `FreeList::allocate` / `FreeList::deallocate`.
//
// WHY: `FreeList` is `BTreeMap`-backed, and MEASURED on this component, CBMC's symbolic
// execution of std's BTreeMap costs ~9 s / 370 MB for ONE mutation and blows past 400 s /
// 5 GB by the sixth. P1 (initialize-once-only) and P3 (evict-untrack) are not ABOUT
// allocation at all -- they are about the initialized flag and about which policy calls
// the eviction path makes -- so paying BTreeMap's price there buys nothing.
//
// The model is a 4 KiB bump allocator: it reproduces exactly the two facts the two
// properties depend on (distinct, 4 KiB-aligned, non-overlapping offsets inside the pool;
// `None` once the pool is exhausted) and nothing else. Disclosed in both properties'
// `note` as a trusted stub => fidelity `representative`, symbol ★.
//
// Allocation itself is proved WITHOUT this stub, on the real BTreeMap-backed FreeList,
// by verify_mt_insert_post_alignment / verify_mt_insert_err_pool_full.
//
// The stub cannot own typed state (Kani rejects a monomorphised stub), so the bump
// cursor lives in a type-erased static -- the pattern the skill prescribes.
// #####################################################################################

/// Replaces `alloc::fmt::format`, i.e. the body of every `format!` in the reachable code.
/// `initialize` builds a log string on its SUCCESS path (lib.rs:313-316) and the argument is
/// evaluated at the call site, so it runs even with the logger receptacle unconnected.
/// MEASURED: real `format!` turns `initialize` into a >200 s / 2.2 GB timeout; stubbed it is
/// 15-18 s / 177 MB. Faithful for every property here -- none of them mentions log text.
fn stub_fmt_format(_args: std::fmt::Arguments<'_>) -> String {
    String::new()
}

static BUMP: AtomicUsize = AtomicUsize::new(0);
static BUMP_CAP: AtomicUsize = AtomicUsize::new(0);

fn bump_reset(cap: usize) {
    BUMP.store(0, AOrd::SeqCst);
    BUMP_CAP.store(cap, AOrd::SeqCst);
}

fn stub_fl_allocate(_fl: &mut FreeList, size: usize) -> Option<usize> {
    if size == 0 {
        return None; // same guard as allocator.rs:39-41
    }
    let aligned = size.next_multiple_of(ALIGN);
    let off = BUMP.load(AOrd::SeqCst);
    if off + aligned > BUMP_CAP.load(AOrd::SeqCst) {
        return None; // same PoolFull outcome as allocator.rs:44
    }
    BUMP.store(off + aligned, AOrd::SeqCst);
    Some(off)
}

fn stub_fl_deallocate(_fl: &mut FreeList, _offset: usize, _size: usize) {
    // A bump allocator does not reclaim. Neither P1 nor P3 asserts anything about reuse.
}

/// REQUIRED stub-reachability diagnostic (the analogue of `check_assumes_actually_bind`).
/// A stub that blinds the code makes every downstream implication vacuously true AND makes
/// its mutant twin pass, so anti-vacuity cannot see it. This proves the stubbed paths are
/// still REACHED and still return something usable, and that the REAL `HashMap` slot table
/// and the REAL policy receptacle are still doing their work.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn check_stubs_actually_reached() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(true);
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    // the mmap stub was REACHED: a real, page-aligned pool of the right size is reported
    let (ptr, sz) = c.pool_info().unwrap();
    assert!(!ptr.is_null());
    assert!((ptr as usize) & (ALIGN - 1) == 0);
    assert!(sz == 3 * ALIGN);
    // the allocator stub was REACHED and returned a usable in-pool aligned address, and
    // the REAL HashMap slot table + REAL policy receptacle both recorded the entry.
    let p1 = c.insert(1u64, ALIGN as u32).unwrap();
    assert!(!p1.is_null());
    assert!((p1 as usize) & (ALIGN - 1) == 0);
    assert!((p1 as usize) >= ptr as usize);
    assert!(c.contains(1u64));
    assert!(pol.track_calls() == 1);
    assert!(pol.is_tracked(1u64));
    std::mem::forget(c);
}

/// Second half of the diagnostic: the stubbed allocator still returns DISTINCT,
/// non-overlapping offsets and still reports pool exhaustion, so nothing downstream can be
/// vacuously true because every allocation collapsed onto one address or onto `None`.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn check_stub_allocator_distinct_and_exhausts() {
    bump_reset(2 * ALIGN);
    let mut fl = FreeList::new(2 * ALIGN);
    let a = fl.allocate(ALIGN).unwrap();
    let b = fl.allocate(ALIGN).unwrap();
    assert!(a != b);
    assert!(a & (ALIGN - 1) == 0 && b & (ALIGN - 1) == 0);
    assert!(a + ALIGN <= b || b + ALIGN <= a);
    assert!(fl.allocate(ALIGN).is_none()); // exhaustion still reported
    assert!(fl.allocate(0).is_none()); // the size == 0 guard survives
}

// #####################################################################################
// P2  MT-INSERT-POST-ALIGNMENT   (postcondition, spec-only, global, attachments 3)
//
// "Every address handed back when adding a cache entry starts on a four-kibibyte boundary,
//  so the memory can be used directly as the source or destination of a disk transfer
//  without any extra copying or realignment."
//
// `insert` returns `state.pool_ptr.add(offset)` (lib.rs:383-384), so the obligation is
//   (i)   `size.next_multiple_of(4096)` is a multiple of 4096;
//   (ii)  every free-region start the allocator can return is a multiple of 4096 -- an
//         invariant of the BTreeMap bookkeeping, established by starting at offset 0 and
//         preserved by `offset + aligned_size` on allocate and by coalescing on deallocate;
//   (iii) the pool base is at least page-aligned (an FFI postcondition of mmap /
//         spdk_zmalloc(_, 4096, ..)).
//
// SCORED CLAIM (iteration 3) = the ARITHMETIC CORE, (i) + the region-start recurrence + (iii).
// It is the geometry whose proof AND whose anti-vacuity twin are both affordable, so the twin
// vouches for the harness that is actually scored. Fidelity `arithmetic-core`.
// NOT CLAIMED: that the real BTreeMap bookkeeping maintains the 4 KiB lattice across arbitrary
// allocate/deallocate interleavings (clause ii as an invariant of `FreeList`). MEASURED: a single
// CONCRETE `FreeList::allocate` on the real BTreeMap reaches 28 GB at 170 s (`BTreeMap::remove`
// is the cliff) and the old composition-level mutant ran >900 s. Route clause (ii) to Creusot.
// #####################################################################################

/// SCORED. The arithmetic core over the REAL `usize::next_multiple_of` and REAL address
/// arithmetic, for a FULLY SYMBOLIC u32 request size across its entire range and a symbolic
/// 4 KiB-lattice region start:
///   * the rounded size is always a whole number of pages, >= the request, < request + 4 KiB;
///   * the allocator's region-start recurrence (0, then prev + aligned_size) stays on the lattice;
///   * a page-aligned base (PROVED from `#[repr(align(4096))]`, not assumed) plus a lattice
///     offset -- which is exactly `pool_ptr.add(offset)` -- is page-aligned.
#[kani::proof]
#[kani::solver(minisat)]
fn verify_mt_insert_post_alignment() {
    let base = fake_pool_base() as usize;
    assert!(base & (ALIGN - 1) == 0); // proved, not assumed

    // (i) rounding is to a whole number of pages, for every legal request size.
    let size: u32 = kani::any();
    kani::assume(size >= 1); // the production guard at lib.rs:329-331 (`size == 0` -> InvalidSize)
    let aligned = (size as usize).next_multiple_of(ALIGN);
    assert!(aligned & (ALIGN - 1) == 0);
    assert!(aligned >= size as usize);
    assert!(aligned < size as usize + ALIGN);

    // (ii-core) the region-start recurrence stays on the 4 KiB lattice.
    let start: usize = kani::any();
    kani::assume(start & (ALIGN - 1) == 0);
    kani::assume(start <= (1usize << 40)); // keep the sum in range (pool sizes are << 2^40)
    let next = start + aligned;
    assert!(next & (ALIGN - 1) == 0);

    // (iii) and therefore the address `insert` hands back is 4 KiB-aligned.
    assert!((base + start) & (ALIGN - 1) == 0);
    assert!((base + next) & (ALIGN - 1) == 0);

    kani::cover!(aligned == ALIGN);
    kani::cover!(aligned == 2 * ALIGN);
}

/// Anti-vacuity twin: the SAME harness, same flags, with exactly ONE assertion flipped -- the
/// returned-address alignment `(base + start)`. MUST FAIL, on that assertion.
#[kani::proof]
#[kani::solver(minisat)]
fn verify_mt_insert_post_alignment__mutant() {
    let base = fake_pool_base() as usize;
    assert!(base & (ALIGN - 1) == 0);

    let size: u32 = kani::any();
    kani::assume(size >= 1);
    let aligned = (size as usize).next_multiple_of(ALIGN);
    assert!(aligned & (ALIGN - 1) == 0);
    assert!(aligned >= size as usize);
    assert!(aligned < size as usize + ALIGN);

    let start: usize = kani::any();
    kani::assume(start & (ALIGN - 1) == 0);
    kani::assume(start <= (1usize << 40));
    let next = start + aligned;
    assert!(next & (ALIGN - 1) == 0);

    assert!((base + start) & (ALIGN - 1) != 0); // FLIPPED -- must FAIL
    assert!((base + next) & (ALIGN - 1) == 0);

    kani::cover!(aligned == ALIGN);
    kani::cover!(aligned == 2 * ALIGN);
}

// ---- Supporting / boundary harnesses for P2. NOT scored, and deliberately NOT named with the
// `verify_mt_insert_post_alignment` prefix: `cargo kani --harness` substring-matches, so any such
// name would be executed inside the SCORED run and bill its cost (the iteration-2 scored run
// executed eight variants, two of them >200 s, plus a >900 s twin). ----

/// SUPPORTING (not scored): the component-level composition -- real `insert`, real `Pool`, real
/// `HashMap` slot table, real policy; only the BTreeMap allocator replaced by the disclosed 4 KiB
/// bump model. MEASURED iteration 1: 125 s / 3.2 GB / 0 undetermined, SUCCESSFUL.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn support_insert_alignment_composition() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);
    c.initialize(3 * ALIGN, None).unwrap();
    let (base, _sz) = c.pool_info().unwrap();
    assert!((base as usize) & (ALIGN - 1) == 0);
    let size: u32 = kani::any();
    kani::assume(size >= 1 && size <= ALIGN as u32);
    let p = c.insert(1u64, size).unwrap();
    assert!(!p.is_null());
    assert!((p as usize) & (ALIGN - 1) == 0);
    assert!(p as usize >= base as usize);
    std::mem::forget(c);
}

/// BOUNDARY SIGNATURE (not scored): the real, unstubbed BTreeMap `FreeList`, one concrete
/// allocate. MEASURED: >170 s, 28 GB and climbing (iteration 1); kept to reproduce the wall.
#[kani::proof]
#[kani::solver(minisat)]
fn boundary_insert_alignment_real_freelist() {
    let base = fake_pool_base() as usize;
    let mut fl = FreeList::new(3 * ALIGN);
    let off = fl.allocate(ALIGN).unwrap();
    assert!(off & (ALIGN - 1) == 0);
    assert!((base + off) & (ALIGN - 1) == 0);
    assert!(fl.used() == ALIGN);
}

// #####################################################################################
// P4  MT-INSERT-ERR-POOL-FULL   (error-case, spec+code, loop-bearing)
//
// "When no single free run of pool memory is large enough for the rounded request, adding a
//  cache entry fails with the pool-full error rather than returning a short or overlapping
//  block, and this can happen even when the total amount of free space would have been
//  sufficient had it not been fragmented."
//
// SCORED CLAIM (iteration 3) = the ERROR CONTRACT end to end on the real component: when the
// allocator reports no run large enough, `insert` (lib.rs:360-363, `.ok_or(PoolFull)?`) returns
// exactly `MemoryTierError::PoolFull`, hands back no pointer, creates no slot, never tells the
// policy, and consumes nothing. Real `initialize`, real `insert`, real `RwLock`s, real `Pool`
// and `HashMap` slot table, real policy receptacle. The free-run SELECTION is behind the
// disclosed 4 KiB bump model (`stub_fl_allocate`), whose reachability and exhaustion behaviour is
// pinned by `check_stubs_actually_reached` / `check_stub_allocator_distinct_and_exhausts`.
// Fidelity `representative` (disclosed stub).
// NOT CLAIMED: the FRAGMENTATION clause ("can happen even when total free space would suffice")
// -- that is a property of the real BTreeMap first-fit scan (allocator.rs:44) and is unreachable
// for Kani here: the fully concrete 3-allocate/1-deallocate witness times out (>400 s), as does a
// 2-call concrete version, with and without --no-unwinding-checks. Route it to Creusot.
// #####################################################################################

/// SCORED. A two-page request into a one-page pool on the real component.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_insert_err_pool_full() {
    bump_reset(ALIGN); // a one-page pool
    let (c, pol) = wire_lean(true);
    c.initialize(ALIGN, None).unwrap();
    let r = c.insert(1u64, (2 * ALIGN) as u32); // two pages into a one-page pool
    assert!(matches!(r, Err(MemoryTierError::PoolFull))); // exactly PoolFull, no pointer
    assert!(!c.contains(1u64)); // no slot was created
    assert!(pol.track_calls() == 0); // the policy was never told about it
    assert!(c.used() == 0); // and nothing was consumed
    std::mem::forget(c);
}

/// Anti-vacuity twin: the SAME harness, same flags, with exactly ONE assertion flipped -- the
/// PoolFull outcome itself. MUST FAIL, on that assertion.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_insert_err_pool_full__mutant() {
    bump_reset(ALIGN);
    let (c, pol) = wire_lean(true);
    c.initialize(ALIGN, None).unwrap();
    let r = c.insert(1u64, (2 * ALIGN) as u32);
    assert!(!matches!(r, Err(MemoryTierError::PoolFull))); // FLIPPED -- must FAIL
    assert!(!c.contains(1u64));
    assert!(pol.track_calls() == 0);
    assert!(c.used() == 0);
    std::mem::forget(c);
}

// ---- Boundary harnesses for P4. NOT scored; renamed out of the `verify_mt_insert_err_pool_full`
// substring space so the scored run does not execute them. ----

/// BOUNDARY SIGNATURE (not scored): the fragmentation witness on the real BTreeMap `FreeList`.
/// 3-page pool, allocate two pages one at a time, free the FIRST: 8192 bytes free in total but
/// the largest run is 4096, so a 2-page request must fail. MEASURED: >400 s TIMEOUT.
#[kani::proof]
#[kani::solver(minisat)]
fn boundary_pool_full_fragmentation_real_freelist() {
    let mut fl = FreeList::new(3 * ALIGN);
    let a = fl.allocate(ALIGN).unwrap();
    let _b = fl.allocate(ALIGN).unwrap();
    fl.deallocate(a, ALIGN);
    assert!(fl.capacity() - fl.used() == 2 * ALIGN);
    let used_before = fl.used();
    assert!(fl.allocate(2 * ALIGN).is_none());
    assert!(fl.used() == used_before);
}

/// BOUNDARY SIGNATURE (not scored): the smallest real-FreeList exhaustion witness, two allocator
/// calls. MEASURED: >120 s TIMEOUT; >200 s with --no-unwinding-checks.
#[kani::proof]
#[kani::solver(minisat)]
fn boundary_pool_full_exhaustion_real_freelist() {
    let mut fl = FreeList::new(ALIGN);
    assert!(fl.allocate(ALIGN).is_some());
    assert!(fl.allocate(ALIGN).is_none());
    assert!(fl.used() == ALIGN);
}

// #####################################################################################
// P1  MT-INV-INITIALIZE-ONCE-ONLY   (invariant, spec+code, global, ATTACHMENTS 17)
//
// "Setting up the pool is a one-way step: the component has exactly two states, not set
//  up and set up, the only legal transition is from the first to the second exactly once,
//  nothing ever clears the flag, and its capacity and base address never change
//  afterwards."
//
// Proved on the REAL component: real `define_component!` instance, real `Receptacle`
// binding, real `RwLock<MemoryTierState>`, real `AtomicBool`, real `IMemoryTier::initialize`.
// Both halves in one harness: establishment (un-initialized -> initialized) and
// preservation (a SECOND call at an arbitrary size and arbitrary NUMA node is rejected and
// changes neither the flag, the base address, nor the capacity).
// #####################################################################################

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_inv_initialize_once_only() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);

    // ---- state 1: NOT set up. Every accessor must report the empty pool. -----------
    assert!(!is_init_lean(&c));
    assert!(c.capacity() == 0);
    assert!(c.used() == 0);
    assert!(c.pool_info().is_none());

    // ---- the one legal transition ---------------------------------------------------
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    assert!(is_init_lean(&c));
    let (base1, size1) = c.pool_info().unwrap();
    assert!(size1 == 3 * ALIGN);
    assert!(c.capacity() == 3 * ALIGN);

    // ---- preservation: a second attempt, for ANY size and ANY NUMA node -------------
    let sz2: usize = kani::any();
    let numa: Option<i32> = kani::any();
    let r2 = c.initialize(sz2, numa);

    assert!(r2.is_err()); // never a second successful transition
    assert!(is_init_lean(&c)); // the flag is never cleared
    let (base2, size2) = c.pool_info().unwrap();
    assert!(base2 == base1); // base address never changes
    assert!(size2 == size1); // capacity never changes
    assert!(c.capacity() == 3 * ALIGN);

    // The two rejection paths are both reachable (sz2 == 0 -> InvalidSize,
    // sz2 != 0 -> AllocationFailed("already initialized")).
    kani::cover!(sz2 == 0);
    kani::cover!(sz2 == 3 * ALIGN);

    std::mem::forget(c);
}

/// Anti-vacuity twin: claims the second `initialize` SUCCEEDS. MUST FAIL.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_inv_initialize_once_only__mutant() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    let sz2: usize = kani::any();
    kani::assume(sz2 >= 1 && sz2 <= 3 * ALIGN);
    let r2 = c.initialize(sz2, None);
    assert!(r2.is_ok()); // the OPPOSITE -- must FAIL
    std::mem::forget(c);
}

// #####################################################################################
// CONFORMANCE RAIL for the `IEvictionPolicy` mock.
//
// WHY THIS EXISTS. The pipeline has `__mutant` twins to catch a FALSE PROOF, but nothing to
// catch a FALSE REFUTATION -- and a stub that violates the contract it stands in for is
// exactly how one gets manufactured. An earlier version of this file "refuted"
// MT-EVICT-NEXT-POST-UNTRACKED using `StubPolicy::new(false)`, a mock that does NOT untrack
// its own victim. `IEvictionPolicy` (interfaces/src/ieviction_policy.rs:101-103) specifies
// `identify_next_to_evict` as "Select the next key the policy would evict, REMOVE IT FROM
// TRACKING, and return it", and the shipped `eviction-policy-lru` honours it
// (lib.rs:133-138 -> `lru.pop_front()` -> lru_list.rs:99-104 -> `self.remove(head_idx)`).
// So that harness established only "a non-conforming policy leaves the victim tracked",
// which is not a property of memory-tier. It has been WITHDRAWN.
//
// This harness makes the mock's conformance EXPLICIT AND CHECKABLE, for every contract
// clause the proofs below rely on, and includes a negative control proving that the
// non-conforming configuration really is non-conforming (so the distinction cannot silently
// collapse again).
// #####################################################################################

#[kani::proof]
#[kani::solver(minisat)]
#[kani::unwind(6)]
fn check_stub_conforms_ieviction_policy() {
    let k: CacheKey = 7;
    let j: CacheKey = 11;

    // ---- the CONFORMING configuration, used by every proof below -------------------
    let pol = StubPolicy::new(true);
    assert!(pol.len(0) == 0);
    assert!(pol.identify_next_to_evict(0).is_none()); // "None if the pool is empty"

    let h = pol.track(0, k, BlockSemantics::default()).unwrap();
    assert!(pol.is_tracked(k));
    assert!(pol.len(0) == 1);

    // Re-registering a tracked key is idempotent and returns the existing handle
    // (ieviction_policy.rs:84-86).
    let h2 = pol.track(0, k, BlockSemantics::default()).unwrap();
    assert!(h2 == h);
    assert!(pol.len(0) == 1);

    // THE CLAUSE THE PROOFS RELY ON: identify_next_to_evict removes its victim from
    // tracking (ieviction_policy.rs:101-103), exactly as eviction-policy-lru's
    // `pop_front()` -> `remove(head_idx)` does.
    let v = pol.identify_next_to_evict(0);
    assert!(v == Some(k));
    assert!(!pol.is_tracked(k)); // <- untracked by the policy itself
    assert!(pol.len(0) == 0);
    assert!(!pol.get_eviction_candidates(0, 2).contains(&k));
    assert!(pol.identify_next_to_evict(0).is_none()); // and not nominable again

    // `remove` also untracks (ieviction_policy.rs:98-99).
    let hj = pol.track(0, j, BlockSemantics::default()).unwrap();
    assert!(pol.is_tracked(j));
    pol.remove(hj).unwrap();
    assert!(!pol.is_tracked(j));
    assert!(pol.len(0) == 0);

    // `clear_pool` empties it (ieviction_policy.rs:113-114).
    pol.track(0, j, BlockSemantics::default()).unwrap();
    pol.clear_pool(0);
    assert!(pol.len(0) == 0);

    // ---- NEGATIVE CONTROL ----------------------------------------------------------
    // The non-conforming configuration really does violate the clause above. This is why it
    // may never be used to establish a property of memory-tier, and asserting it here keeps
    // the two configurations from being confused again.
    let bad = StubPolicy::new(false);
    bad.track(0, k, BlockSemantics::default()).unwrap();
    assert!(bad.identify_next_to_evict(0) == Some(k));
    assert!(bad.is_tracked(k)); // contract VIOLATED on purpose -- not a memory-tier defect
}

// #####################################################################################
// P3  MT-EVICT-NEXT-POST-UNTRACKED   (postcondition, inventory origin: divergent)
//
// "An evicted key is also removed from the external eviction-policy component's bookkeeping,
//  so it can no longer appear in the list of oldest keys and cannot be nominated as a victim
//  a second time."
//
// HOW IT HOLDS. `evict_next` (lib.rs:434-464) does not itself call the policy's untrack entry
// point; it delegates, by calling `identify_next_to_evict`, whose own contract
// (ieviction_policy.rs:101-103) is to remove the victim from tracking. Composed with a
// CONFORMING policy -- which `eviction-policy-lru` is -- the postcondition holds. That is what
// the harness below proves, against the real `evict_next` and the real `Pool`/`HashMap` slot
// table, with the conforming mock whose relevant clauses are pinned by
// `check_stub_conforms_ieviction_policy`.
//
// SCOPE OF THE CLAIM, stated honestly: this is a COMPOSITIONAL proof. It proves the
// postcondition given the callee contract; it does not re-prove that contract (that belongs to
// eviction-policy-lru, verified from source: lib.rs:133-138 -> lru_list.rs:99-104). The
// separate fact that `evict_next` drops the Slot's `eviction_handle` without returning it to
// the policy is a HANDLE-LIFETIME observation, recorded under MT-INV-HANDLE-CAN-OUTLIVE-ENTRY,
// not a violation of this postcondition -- see `witness_remove_does_untrack`.
// #####################################################################################

/// SUPPORTING (not scored): the same obligation with the entry inserted through the tier's real
/// `insert`, so the real `HashMap` is mutated twice. MEASURED: TIMEOUT at 400 s / 4.2 GB -- the
/// `Pool.slots` hashbrown wall. Renamed out of the `verify_mt_evict_next_post_untracked`
/// substring space in iteration 3: under its old `__fullstack` name the scorer's base run
/// (`--harness` substring match) executed it inside the scored run.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn support_evict_untracked_fullstack() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(true); // CONFORMING, per ieviction_policy.rs:101-103
    c.initialize(3 * ALIGN, None).unwrap();
    let k: CacheKey = 7;
    c.insert(k, ALIGN as u32).unwrap();
    assert!(pol.is_tracked(k));
    assert!(c.contains(k));
    let victim = c.evict_next();
    assert!(victim == Some(k));
    assert!(!pol.is_tracked(k));
    assert!(pol.len(0) == 0);
    assert!(!c.contains(k));
    assert!(c.evict_next().is_none());
    std::mem::forget(c);
}

/// SCORED. The spec postcondition against the REAL `initialize` and REAL `evict_next`
/// (lib.rs:434-464), the real `RwLock<MemoryTierState>`, the real `Pool`, and a CONFORMING
/// `IEvictionPolicy` whose relevant contract clauses are pinned by
/// `check_stub_conforms_ieviction_policy`.
///
/// The tracked key is primed on the policy directly rather than through the tier's `insert`.
/// That costs nothing in fidelity for THIS property -- `evict_next` consults `pool.slots` only
/// to decide whether to reclaim memory (`if let Some(slot) = pool.slots.remove(&key)`), never to
/// decide whether the policy is untracked -- and it avoids the `Pool.slots` traffic that puts
/// `support_evict_untracked_fullstack` beyond the cap. COMPOSITIONAL: proves the postcondition
/// GIVEN the callee contract; that contract is eviction-policy-lru's (lib.rs:133-138 ->
/// lru_list.rs:99-104).
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_evict_next_post_untracked() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(true);
    c.initialize(3 * ALIGN, None).unwrap();
    let k: CacheKey = 7;
    pol.track(0, k, BlockSemantics::default()).unwrap();

    let victim = c.evict_next();
    assert!(victim == Some(k));
    assert!(!pol.is_tracked(k)); // removed from the policy's bookkeeping
    assert!(pol.len(0) == 0); // ... so it is not among the oldest keys
    assert!(c.evict_next().is_none()); // and cannot be nominated a second time
    kani::cover!(victim == Some(k));
    std::mem::forget(c);
}

/// Anti-vacuity twin: the SAME harness, same flags, with exactly ONE assertion flipped -- the
/// untracking postcondition itself. MUST FAIL, on that assertion.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_evict_next_post_untracked__mutant() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(true);
    c.initialize(3 * ALIGN, None).unwrap();
    let k: CacheKey = 7;
    pol.track(0, k, BlockSemantics::default()).unwrap();

    let victim = c.evict_next();
    assert!(victim == Some(k));
    assert!(pol.is_tracked(k)); // FLIPPED -- must FAIL
    assert!(pol.len(0) == 0);
    assert!(c.evict_next().is_none());
    kani::cover!(victim == Some(k));
    std::mem::forget(c);
}

/// SUPPORTING EVIDENCE for a DIFFERENT obligation -- MT-INV-HANDLE-CAN-OUTLIVE-ENTRY, not the
/// untracking postcondition above.
///
/// `remove` (lib.rs:500) hands the Slot's `eviction_handle` back to the policy
/// (`let _ = ep.remove(slot.eviction_handle);`) before deallocating. `evict_next`
/// (lib.rs:455-458) does not: it drops the whole `Slot`, handle included, and relies on the
/// policy having untracked its own victim. This harness proves the asymmetry -- exactly one
/// policy `remove` call on the `remove` path, zero on the eviction path.
///
/// That is a verifiable fact about handle lifetime, and it is NOT a defect in this
/// postcondition: with a conforming policy the victim is already untracked, so no handle needs
/// returning. It matters for the stale-handle window recorded under
/// MT-INV-HANDLE-CAN-OUTLIVE-ENTRY (inventory line 5895), where a handle copied out of the slot
/// table can outlive its entry. Uses the CONFORMING policy; `identify_next_to_evict` is never
/// called here, so the setting is immaterial to the result.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn witness_remove_does_untrack() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(true);
    c.initialize(3 * ALIGN, None).unwrap();
    let k: CacheKey = 9;
    c.insert(k, ALIGN as u32).unwrap();
    assert!(pol.remove_calls() == 0);
    c.remove(k).unwrap();
    assert!(pol.remove_calls() == 1); // the eviction path makes ZERO such calls
    assert!(!pol.is_tracked(k));
    std::mem::forget(c);
}

// #####################################################################################
// P5  MT-INV-LOCK-POISONING-CASCADE   (invariant, CODE-ONLY, global, attachments 17)
//
// "Every lock acquisition in the component unwraps its result, so once any thread panics
//  while holding either the component-state lock or the inner pool lock, every later call
//  on the component panics too and there is no way to recover the pool."
//
// WHY THE CASCADE ITSELF IS NOT EXPRESSIBLE IN KANI (measured iteration 3; supersedes the
// iteration-1 diagnosis "catch_unwind is unsupported", which was only the first symptom):
// Kani compiles and links std with `-C panic=abort` (`cfg!(panic = "unwind")` is FALSE inside a
// harness -- see `boundary_lock_poison_model_is_panic_abort`). In that build of std:
//   * `sync::poison::Flag` has NO `failed` field; `Flag::get()` is the constant `false` and
//     `Flag::done()` (called from every write guard's Drop) is a no-op;
//   * `PoisonError<T>` carries a `_never: !` field, i.e. it is UNINHABITED, so
//     `LockResult::Err` cannot exist and every `.unwrap()` on a lock result is infallible.
// So both the antecedent (a lock becomes poisoned) and the consequent (a later acquisition
// returns Err and the unwrap panics) are absent from the model -- not merely hard to reach.
// `boundary_lock_poison_simulated_unwind_does_not_poison` proves it: a write guard dropped while
// `std::thread::panicking()` reports an in-flight panic leaves the lock UNPOISONED. No stub can
// restore it: constructing the Err would require a value of an uninhabited type.
//
// SCORED CLAIM (iteration 3) = the half that IS expressible, on the real component: with no
// poisoning, every acquisition of the component-state lock and of the inner pool lock succeeds,
// before and after the one-way `initialize`, and the real methods whose bodies `.unwrap()` those
// acquisitions return normally with the expected values. I.e. the lock-unwraps are not a panic
// source on an unpoisoned component, so poisoning is the ONLY trigger of the cascade.
// NOT CLAIMED: the cascade (poisoned -> every later call panics, no recovery). Route: a native
// `#[test]` under the default panic=unwind build (poison via a panicking thread holding the
// guard, then `catch_unwind` around each later call), or Creusot.
// #####################################################################################

/// SCORED. The unpoisoned half of MT-INV-LOCK-POISONING-CASCADE.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn support_mt_inv_lock_poisoning_unpoisoned_half() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);

    // Before set-up: the component-state lock acquires cleanly both ways.
    assert!(!c.state.is_poisoned());
    assert!(c.state.read().is_ok());
    assert!(c.state.write().is_ok());

    // `initialize` itself holds `state.write().unwrap()` (lib.rs:270) and returns normally.
    assert!(c.initialize(3 * ALIGN, None).is_ok());

    // After set-up: the outer lock still acquires cleanly both ways ...
    assert!(!c.state.is_poisoned());
    assert!(c.state.read().is_ok());
    assert!(c.state.write().is_ok());
    // ... and so does the inner pool lock, the one `initialize` replaced (lib.rs:305-308).
    {
        let st = c.state.read().unwrap();
        assert!(!st.pool.is_poisoned());
        assert!(st.pool.read().is_ok());
        assert!(st.pool.write().is_ok());
    }

    // Real methods whose bodies `.unwrap()` those acquisitions return normally.
    assert!(c.capacity() == 3 * ALIGN); // state.read().unwrap() + pool.read().unwrap()
    assert!(c.used() == 0); // state.read().unwrap() + pool.read().unwrap()
    assert!(!c.contains(1u64)); // state.read().unwrap() + pool.read().unwrap()
    assert!(c.pool_info().is_some()); // state.read().unwrap()
    assert!(!c.is_dma_capable()); // state.read().unwrap()
    std::mem::forget(c);
}

/// Anti-vacuity twin: the SAME harness, same flags, with exactly ONE assertion flipped -- the
/// post-set-up outer write acquisition. MUST FAIL, on that assertion.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn support_mt_inv_lock_poisoning_unpoisoned_half__mutant() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);

    assert!(!c.state.is_poisoned());
    assert!(c.state.read().is_ok());
    assert!(c.state.write().is_ok());

    assert!(c.initialize(3 * ALIGN, None).is_ok());

    assert!(!c.state.is_poisoned());
    assert!(c.state.read().is_ok());
    assert!(c.state.write().is_err()); // FLIPPED -- must FAIL
    {
        let st = c.state.read().unwrap();
        assert!(!st.pool.is_poisoned());
        assert!(st.pool.read().is_ok());
        assert!(st.pool.write().is_ok());
    }

    assert!(c.capacity() == 3 * ALIGN);
    assert!(c.used() == 0);
    assert!(!c.contains(1u64));
    assert!(c.pool_info().is_some());
    assert!(!c.is_dma_capable());
    std::mem::forget(c);
}

// ---- Boundary harnesses for P5. NOT scored; named out of the
// `verify_mt_inv_lock_poisoning_cascade` substring space. ----

/// Simulated unwinding: while this is true, the `std::thread::panicking` stub reports a panic in
/// flight, which is exactly the condition under which a guard's Drop poisons its lock in an
/// unwind build of std.
static SIM_PANICKING: AtomicBool = AtomicBool::new(false);
fn stub_thread_panicking() -> bool {
    SIM_PANICKING.load(AOrd::SeqCst)
}

/// BOUNDARY, PROVED (SUCCESSFUL is the expected verdict): Kani's std is the panic=abort build.
/// The stub IS applied (first assertion), yet `cfg!(panic = "unwind")` is false.
#[kani::proof]
#[kani::stub(std::thread::panicking, stub_thread_panicking)]
#[kani::solver(minisat)]
fn boundary_lock_poison_model_is_panic_abort() {
    SIM_PANICKING.store(true, AOrd::SeqCst);
    assert!(std::thread::panicking()); // the stub is live
    assert!(!cfg!(panic = "unwind")); // ... and std was built panic=abort
    SIM_PANICKING.store(false, AOrd::SeqCst);
}

/// BOUNDARY, PROVED (SUCCESSFUL is the expected verdict): on the REAL component-state lock, a
/// write guard dropped while a panic is (simulated as) in flight -- the cascade's antecedent --
/// does NOT poison the lock in Kani's model, and later acquisitions still succeed. So the
/// antecedent is unrepresentable here, which is why the scored harness is the unpoisoned half.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(std::thread::panicking, stub_thread_panicking)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn boundary_lock_poison_simulated_unwind_does_not_poison() {
    let (c, _pol) = wire_lean(true);
    {
        let _g = c.state.write().unwrap();
        SIM_PANICKING.store(true, AOrd::SeqCst); // the guard now drops "during unwinding"
    }
    SIM_PANICKING.store(false, AOrd::SeqCst);
    assert!(std::thread::panicking() == false);
    assert!(!c.state.is_poisoned()); // an unwind build would report TRUE here
    assert!(c.state.read().is_ok()); // ... and Err here
    std::mem::forget(c);
}

/// BOUNDARY SIGNATURE (not scored): the iteration-1 base harness, `catch_unwind` antecedent.
/// MEASURED: FAILED, `catch_unwind is not currently supported by Kani` (kani#267), 6034
/// undetermined, 19 s. This is the run the gate called "unclassifiable".
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn boundary_lock_poison_catch_unwind() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);
    c.initialize(3 * ALIGN, None).unwrap();
    let cc = &c;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _g = cc.state.write().unwrap();
        panic!("poison the state lock");
    }));
    assert!(r.is_err());
    assert!(c.state.read().is_err());
    assert!(c.state.write().is_err());
    std::mem::forget(c);
}

/// CAPACITY-SCALING check (the skill's second, independent test that a proof is real: the
/// mutant twin catches a proof with no content, this catches a proof whose bound merely
/// happened to be generous enough). Same source, same flags, pool capacity 3 pages -> 6.
/// Every verdict must hold and `undetermined` must stay 0.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_inv_initialize_once_only__scale2x() {
    bump_reset(6 * ALIGN);
    let (c, _pol) = wire_lean(true);
    assert!(!is_init_lean(&c));
    assert!(c.capacity() == 0);
    assert!(c.pool_info().is_none());
    assert!(c.initialize(6 * ALIGN, None).is_ok());
    assert!(is_init_lean(&c));
    let (base1, size1) = c.pool_info().unwrap();
    assert!(size1 == 6 * ALIGN);
    let sz2: usize = kani::any();
    let numa: Option<i32> = kani::any();
    let r2 = c.initialize(sz2, numa);
    assert!(r2.is_err());
    assert!(is_init_lean(&c));
    let (base2, size2) = c.pool_info().unwrap();
    assert!(base2 == base1 && size2 == size1);
    assert!(c.capacity() == 6 * ALIGN);
    kani::cover!(sz2 == 0);
    kani::cover!(sz2 == 6 * ALIGN);
    std::mem::forget(c);
}

/// Its mutant, so the scaling check covers the anti-vacuity verdict too. MUST FAIL.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::unwind(6)]
#[kani::solver(minisat)]
fn verify_mt_inv_initialize_once_only__scale2x_mutant() {
    bump_reset(6 * ALIGN);
    let (c, _pol) = wire_lean(true);
    assert!(c.initialize(6 * ALIGN, None).is_ok());
    let sz2: usize = kani::any();
    kani::assume(sz2 >= 1 && sz2 <= 6 * ALIGN);
    assert!(c.initialize(sz2, None).is_ok()); // the OPPOSITE -- must FAIL
    std::mem::forget(c);
}

// ---- layered cost probes for the COMPONENT machinery --------------------------------
// Which layer costs what: the define_component! InterfaceMap (2 hashbrown inserts over
// TypeId -> Box<dyn Any>), `initialize`, or `Pool.slots` (a real HashMap<u64, Slot>)?

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::solver(minisat)]
fn probe_comp_1_wire_only() {
    let (c, _pol) = wire(true);
    assert!(!is_init(&c));
    assert!(c.capacity() == 0);
    std::mem::forget(c);
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::solver(minisat)]
fn probe_comp_2_initialize() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire(true);
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    assert!(is_init(&c));
    assert!(c.pool_info().is_some());
    std::mem::forget(c);
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::solver(minisat)]
fn probe_comp_3_one_insert() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire(true);
    c.initialize(3 * ALIGN, None).unwrap();
    assert!(c.insert(1u64, ALIGN as u32).is_ok());
    assert!(c.contains(1u64));
    std::mem::forget(c);
}

// #####################################################################################
// `wire_lean` -- the same component WITHOUT `define_component!`'s interface-discovery map.
//
// MEASURED: `MemoryTierComponent::new()` alone (no initialize, no insert) costs >180 s /
// 2 GB under CBMC, because `__init_interfaces_*` does two `hashbrown` inserts into
// `InterfaceMap = HashMap<TypeId, Box<dyn Any + Send + Sync>>` and the keys are vtable-
// carrying trait objects. That map is pure *interface discovery*: `query_interface` reads
// it, and NOTHING in `initialize`, `insert`, `get`, `evict_next`, `remove`, `contains`,
// `capacity`, `used` or `pool_info` ever touches it.
//
// So we build the real struct with an EMPTY discovery map and everything else real: the
// real `RwLock<MemoryTierState>`, the real `AtomicBool`, the real `Pool` with its real
// `HashMap<CacheKey, Slot>`, the real `FreeList`, and real `Receptacle`s carrying a real
// `Arc<dyn IEvictionPolicy>`. Disclosed in every affected property's `note`.
// #####################################################################################

fn wire_lean(untrack_own_victim: bool) -> (MemoryTierComponent, Arc<StubPolicy>) {
    let pol = Arc::new(StubPolicy::new(untrack_own_victim));
    let ep: Arc<dyn IEvictionPolicy + Send + Sync> = pol.clone();
    let c = MemoryTierComponent {
        __interface_map: component_core::component::InterfaceMap::new(),
        __interface_info: Vec::new(),
        __receptacle_info: Vec::new(),
        __version: "0.3.0",
        state: RwLock::new(MemoryTierState::default()),
        logger: component_core::receptacle::Receptacle::new(),
        eviction_policy: component_core::receptacle::Receptacle::new(),
    };
    c.eviction_policy.connect(ep).unwrap();
    (c, pol)
}

fn is_init_lean(c: &MemoryTierComponent) -> bool {
    c.state.read().unwrap().initialized.load(Ordering::Acquire)
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::solver(minisat)]
fn probe_lean_1_wire_only() {
    let (c, _pol) = wire_lean(true);
    assert!(!is_init_lean(&c));
    assert!(c.capacity() == 0);
    std::mem::forget(c);
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::solver(minisat)]
fn probe_lean_2_initialize() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    assert!(is_init_lean(&c));
    assert!(c.pool_info().is_some());
    std::mem::forget(c);
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::solver(minisat)]
fn probe_lean_3_insert_evict() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(false);
    c.initialize(3 * ALIGN, None).unwrap();
    c.insert(7u64, ALIGN as u32).unwrap();
    let v = c.evict_next();
    assert!(v == Some(7u64));
    assert!(pol.remove_calls() == 0);
    std::mem::forget(c);
}

// ---- probe: is `format!` inside `initialize` the remaining cost? ---------------------
// `initialize` ends with `self.log_info(&format!("... {} MiB pool", pool_size/(1024*1024)))`
// (lib.rs:313-316). The `format!` runs even though the logger receptacle is unconnected,
// because the argument is evaluated at the call site. None of the five sample properties
// says anything about log text, so a constant-String stub for `alloc::fmt::format` is
// faithful for all of them.
#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::solver(minisat)]
fn probe_lean_2b_initialize_nofmt() {
    bump_reset(3 * ALIGN);
    let (c, _pol) = wire_lean(true);
    assert!(c.initialize(3 * ALIGN, None).is_ok());
    assert!(is_init_lean(&c));
    assert!(c.pool_info().is_some());
    std::mem::forget(c);
}

#[kani::proof]
#[kani::stub(RandomState::new, concrete_random_state)]
#[kani::stub(MemoryTierComponent::alloc_mmap, stub_alloc_mmap2)]
#[kani::stub(FreeList::allocate, stub_fl_allocate)]
#[kani::stub(FreeList::deallocate, stub_fl_deallocate)]
#[kani::stub(std::fmt::format, stub_fmt_format)]
#[kani::solver(minisat)]
fn probe_lean_3b_insert_evict_nofmt() {
    bump_reset(3 * ALIGN);
    let (c, pol) = wire_lean(false);
    c.initialize(3 * ALIGN, None).unwrap();
    c.insert(7u64, ALIGN as u32).unwrap();
    let v = c.evict_next();
    assert!(v == Some(7u64));
    assert!(pol.remove_calls() == 0);
    std::mem::forget(c);
}

// ---- geometry probes for the real (unstubbed) BTreeMap FreeList ----------------------

/// ONE concrete allocate from a fresh pool: BTreeMap new+insert, then find+remove+insert.
#[kani::proof]
#[kani::solver(minisat)]
fn probe_fl_1_concrete_alloc() {
    let mut fl = FreeList::new(3 * ALIGN);
    let o = fl.allocate(ALIGN);
    assert!(o == Some(0));
    assert!(o.unwrap() & (ALIGN - 1) == 0);
    assert!(fl.used() == ALIGN);
}

/// ONE allocate whose size is a symbolic choice among representative residues mod 4096.
#[kani::proof]
#[kani::solver(minisat)]
fn probe_fl_2_representative_alloc() {
    let mut fl = FreeList::new(3 * ALIGN);
    let k: u8 = kani::any();
    kani::assume(k < 7);
    let s: usize = match k {
        0 => 1,             // minimum legal request
        1 => ALIGN - 1,     // just under one page
        2 => ALIGN,         // exactly one page
        3 => ALIGN + 1,     // just over one page
        4 => 2 * ALIGN,     // exactly two pages
        5 => 3 * ALIGN,     // exactly the whole pool
        _ => 3 * ALIGN + 1, // one byte more than the pool: must fail
    };
    match fl.allocate(s) {
        Some(off) => {
            assert!(off & (ALIGN - 1) == 0);
            assert!(off + s.next_multiple_of(ALIGN) <= 3 * ALIGN);
            assert!(fl.used() == s.next_multiple_of(ALIGN));
        }
        None => assert!(fl.used() == 0),
    }
    kani::cover!(fl.used() == 0);
    kani::cover!(fl.used() == ALIGN);
    kani::cover!(fl.used() == 3 * ALIGN);
}

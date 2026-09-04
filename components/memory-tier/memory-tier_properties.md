# Verified properties — memory-tier (Kani)

Verified from spec `specs/001-memory-tier/spec.md` against code `src/allocator.rs`.
Harnesses live in the `#[cfg(kani)] mod verification` block in `src/allocator.rs`.
Toolchain: `cargo kani` (Kani 0.67.0). Run with `cargo kani -p memory-tier`.

**Result: 3 harnesses, 3 SUCCESSFUL, 0 failed.**

Each harness calls the **real** production function `allocator::align_up` under the production
input domain and asserts a spec-derived postcondition. Kani proves each property over the
**entire input domain** (`size` symbolic in `[1, u32::MAX]`, matching the `IMemoryTier::insert(key,
size: u32)` type bound), symbolically — not by sampling. The `verify_align_up_contract` harness was
anti-vacuity checked by fault injection (see bottom).

## Why the Kani yield here is small (the headline finding)

The memory-tier's allocatable, deterministic core is the `FreeList` allocator, but `FreeList`
stores its free regions in a **`std::collections::BTreeMap`** (spec NFR-005: "BTreeMap for O(log n)
first-fit search"). **Any harness that calls `FreeList::new` / `allocate` / `deallocate` is
intractable for Kani/CBMC** — CBMC must symbolically execute the full generic BTreeMap machinery
(node search, split, merge, rebalance over `NonNull` node pointers), and this does not terminate at
any geometry we can construct. This is an intrinsic CBMC limit on std associative containers, not a
bound we can shrink (measured evidence below). The one BTreeMap-free, pointer-free piece is the
alignment primitive `align_up` (FR-004), and that is what the three harnesses prove.

To make that primitive verifiable while still exercising **real production code** (per the skill's
"call the real function" rule), the inline `size.next_multiple_of(ALIGNMENT)` at the two call sites
in `allocate`/`deallocate` was factored into a `pub(crate) fn align_up`. This is a behaviour-
preserving refactor: all 21 existing `memory-tier` unit tests still pass.

## `align_up` — the 4 KiB alignment primitive (`src/allocator.rs`)

`align_up(size) = size.next_multiple_of(ALIGNMENT)`, `ALIGNMENT = 4096`. It is the single rounding
step on the sizing path of every `allocate` (offset granularity, FR-004) and every `deallocate`
(aligned `used`-accounting). Proving its contract proves the alignment invariant the whole allocator
rests on, independently of the intractable container.

- **[Postcondition]** For every request `size` in `[1, u32::MAX]`, `align_up(size)` is a multiple of
  4096, is never below `size`, and rounds up by strictly less than one page (`a - size < 4096`);
  Kani additionally proves the arithmetic never overflows/panics over that whole domain. — spec
  FR-004 / SC-4 — harness `verify_align_up_contract` (SUCCESSFUL, CBMC Verification Time ≈ 0.046 s).
- **[Invariant]** `align_up` is idempotent — an already-aligned value is a fixpoint
  (`align_up(align_up(size)) == align_up(size)`). This is why re-aligning `slot.size` inside
  `deallocate` recovers exactly the aligned block `allocate` charged to `used`, giving symmetric
  accounting. — spec FR-004 / FR-026 (accounting side) — harness `verify_align_up_idempotent`
  (SUCCESSFUL, ≈ 0.039 s).
- **[Invariant]** `align_up` is monotonic non-decreasing (`s1 <= s2 ⟹ align_up(s1) <= align_up(s2)`),
  so the first-fit `region_size >= aligned_size` test behaves consistently across request sizes. —
  spec FR-004 — harness `verify_align_up_monotonic` (SUCCESSFUL, ≈ 0.049 s).

## Assumptions / bounds (the assume audit)

- `kani::assume(size > 0)` mirrors the real production guard: `insert()` returns `InvalidSize` for
  `size == 0` (FR-008) and `FreeList::allocate` early-returns `None` for `size == 0`, so `align_up`
  is never reached with 0 in production.
- `kani::assume(size <= u32::MAX as usize)` mirrors the interface type bound: `insert(key, size:
  u32)`, and `deallocate` is called with `slot.size as usize` where `slot.size: u32`. So the full
  production domain of `align_up` is exactly `[1, u32::MAX]`, and Kani proves over all of it.
- No `#[kani::unwind(N)]` is needed: `align_up` is straight-line 64-bit arithmetic with no loops.

## IMemoryTier method coverage (17 public methods)

| # | Method | Status | Notes |
|---|--------|--------|-------|
| 1 | `initialize` | Not | `mmap`/`spdk_zmalloc`/`mbind` FFI + `BTreeMap`/`HashMap` init + external `create_pool`. Out of scope for BMC (FFI, container, receptacle). |
| 2 | `insert` | Partial | FR-004 alignment of the allocation size **proven** via `align_up`. The `BTreeMap` allocation (FR-010 PoolFull), `HashMap` dedup (FR-009 AlreadyExists), and raw-`*mut u8` return are TOOL-blocked / out of scope. |
| 3 | `get` | Not | Raw `*mut u8` + `HashMap` lookup + external eviction `touch`. CBMC weak on raw pointers; container intractable. |
| 4 | `peek` | Not | Raw `*mut u8` + `HashMap` lookup. Same as `get` minus the touch. |
| 5 | `evict_next` | Partial | Aligned-size accounting on the `deallocate` path is the proven `align_up`. Victim choice is delegated to external `IEvictionPolicy`; `HashMap`/`BTreeMap` mutation intractable. |
| 6 | `evict_next_for_key` | Partial | Alias for `evict_next` (FR-014, key ignored — single unsharded pool). Same coverage. |
| 7 | `oldest_keys` | Not | Pure delegation to `IEvictionPolicy::get_eviction_candidates`. No local logic to prove. |
| 8 | `remove` | Partial | Aligned free-size accounting is the proven `align_up`. `HashMap` remove + `KeyNotFound` (FR-015) + `BTreeMap` deallocate intractable. |
| 9 | `touch` | Not | `HashMap` lookup + external eviction `touch`. |
| 10 | `batch_touch` | Not | `HashMap` lookups + `Vec` build + external `batch_touch`. |
| 11 | `contains` | Not | Thin `HashMap::contains_key` behind `RwLock`; no arithmetic to prove. |
| 12 | `capacity` | Not | Thin accessor (`allocator.capacity()`), lock-guarded; no logic. |
| 13 | `used` | Not | Thin accessor (`allocator.used()`), lock-guarded; the aligned-unit invariant on the value it returns is what `align_up` proves. |
| 14 | `pool_info` | Not | Returns raw base `*mut u8` + size `Option`. CBMC weak on raw pointers. |
| 15 | `is_dma_capable` | Not | Thin bool-field accessor; set only by the FFI/SPDK init path. |
| 16 | `clear` | Not | `HashMap::clear` + `FreeList::new` (BTreeMap) + external `clear_pool`. |
| 17 | `telemetry_snapshot` | Not | Thin atomic-load accessor, feature-gated; returns zeros without `telemetry`. |

Summary: **0 Covered, 4 Partial (`insert`, `evict_next`, `evict_next_for_key`, `remove` — all via
the one shared, proven `align_up` alignment primitive), 13 Not.**

## Not verified by Kani (and why) — TOOL vs AGENT gap labels

- **`FreeList::allocate` / `deallocate` / `new` — offset-in-range (FR-004/FR-007), alloc-shrinks-free,
  alloc/dealloc round-trip, coalesce correctness (FR-026), PoolFull (FR-010).** *(TOOL — intrinsic)*
  These are exactly the properties the extent-manager campaign proved on its **array/bitmap-backed**
  buddy & slab allocators, and are the intended best-yield target here — but memory-tier's `FreeList`
  is **`BTreeMap`-backed**, and CBMC cannot symbolically execute std `BTreeMap` tractably. Measured:
  - `verify_allocate_poolfull` (symbolic capacity): CBMC produced **222,675,100 SAT variables /
    1,278,290,265 clauses**; `Runtime Solver > 1300 s` with no result; killed. (Even this harness's
    "logic" just returns `None`, so the entire cost is BTreeMap modeling.)
  - `verify_coalesce_adjacent` (**fully concrete** capacity `3*4096` and concrete sizes): never
    finished **symbolic execution** — stuck unwinding `alloc::collections::btree::search::find_key_index`
    / node rebalancing — at the 500 s timeout, before even reaching the solver.
  - Conclusion: this is a container-modeling wall independent of geometry or symbolic-vs-concrete
    inputs. Not an AGENT gap — no bounded geometry rescues it. (Wanting these proved is a reason to
    reach for **Creusot** — an inductive/logic-level proof over the map — not Kani.)
- **`HashMap<CacheKey, Slot>` slot-map properties — dedup on insert (FR-009), presence tracking
  (`contains`), remove/`KeyNotFound` (FR-015), clear (FR-018).** *(TOOL — intrinsic, worse than
  BTreeMap.)* `std::collections::HashMap` adds SipHash + `RandomState` on top of the same
  heap/rehash machinery; strictly more intractable for CBMC than the BTreeMap above. Not attempted
  beyond noting the wall.
- **Raw-pointer / DMA return values — `insert`→`*mut u8`, `get`/`peek`→`(*mut u8, u32)`,
  `pool_info`→`(*mut u8, usize)` (FR-011/FR-012/FR-022, NFR-010).** *(TOOL — intrinsic.)* The pointer
  is `pool_ptr.add(offset)` into an `mmap`/SPDK region; CBMC has no model of the underlying mapped
  allocation, so pointer-validity/DMA-suitability cannot be expressed. Integration-test territory.
- **Concurrency — single `RwLock<Pool>` serialization, lock-contention counters, out-of-lock eviction
  touches (FR-005/FR-006, NFR-002, SC-3).** *(TOOL — out of scope for BMC.)* Interleaving/atomicity
  properties are **Loom**'s job, not Kani's.
- **FFI / OS — `mmap`+`MAP_HUGETLB` fallback (FR-001/FR-002), `mbind` NUMA binding + graceful
  fallback (FR-019), `spdk_zmalloc`/`spdk_free` (FR-003), `is_dma_capable` (FR-020), Drop/`munmap`
  (NFR-006).** *(TOOL — out of scope.)* Syscalls and SPDK FFI are unmodelled; a Kani harness cannot
  reach or stub them meaningfully.
- **Eviction-policy delegation — victim selection (FR-013), `oldest_keys` candidates (FR-021),
  `touch`/`batch_touch` recency (FR-016/FR-017).** *(TOOL — out of scope; cross-component.)* These
  are behaviours of the external `IEvictionPolicy` component reached through a receptacle; they are
  not memory-tier logic and are not present in this crate to harness.
- **Telemetry counters (FR-027/FR-028, NFR-011).** *(AGENT — feasible, low value.)* `telemetry_snapshot`
  under the `telemetry` feature is a few `AtomicU64::load`s; a harness on the counter struct's
  snapshot/reset arithmetic is buildable but exercises trivial straight-line atomic loads with no
  spec-meaningful invariant. Deprioritised, not blocked.
- **`initialize` zero-size rejection (US-3, `InvalidSize`) and double-init rejection.** *(AGENT —
  feasible but not isolable cheaply.)* The `pool_size == 0 → InvalidSize` and `initialized →
  AllocationFailed` guards are simple, but they sit inside `initialize`, which then proceeds into the
  FFI + container init path; there is no pure sub-function to call, so a Kani harness would drag in
  the TOOL-blocked machinery. Covered instead by unit tests `initialize_twice_fails` /
  `insert_zero_size_fails`.

## Anti-vacuity validation

`verify_align_up_contract` was fault-injection tested to confirm it is bound to the real code (the
change was reverted after): replacing `align_up`'s body with `size` (no rounding) made the harness
**FAIL** with `Failed Checks: "FR-004: result is 4 KiB-aligned"` (VERIFICATION: FAILED, 1 failure).
`verify_align_up_idempotent` and `verify_align_up_monotonic` are trivially satisfied by the identity
function, so they were not separately fault-checked; they add distinct facts (fixpoint, monotonicity)
on top of the anti-vacuity-anchored contract harness.

## Performance (project rule: wall-clock + peak RSS)

- **Tractable suite (3 `align_up` harnesses):** cold end-to-end `cargo kani -p memory-tier`
  (after `cargo clean`) ≈ **13 s wall clock**; warm re-run ≈ 1 s. Per-harness **CBMC Verification
  Time 0.039–0.049 s**; solver runtimes sub-millisecond. Peak RSS of the `cargo`/kani-driver parent
  ≈ **63–178 MB** (CBMC child is negligible here given <0.05 s solves).
  *Measurement caveat:* `cargo kani` spawns detached children, so `/usr/bin/time -v` on the parent
  under-attributes wall/RSS; the end-to-end figure above is measured with `date`, and the per-harness
  CBMC times are Kani's own reported `Verification Time`.
- **Intractable BTreeMap harnesses (documented, not compiled):** `verify_allocate_poolfull` reached
  222.7 M SAT variables / 1.28 B clauses, solver >1300 s, no result (killed ~590 s wall);
  `verify_coalesce_adjacent` did not finish symbolic execution at 500 s. Peak RSS for these CBMC runs
  was many GB (not precisely captured before kill).

## Environment note

`cargo kani` builds the whole crate, which pulls `interfaces` (spdk feature) → `spdk-sys`, whose
build script needs a prebuilt SPDK tree. This worktree satisfies it with gitignored symlinks
`deps/spdk -> /opt/spdk` and `deps/spdk-build -> /opt/spdk-build` (same as the extent-manager kani
worktree). No SPDK/FFI code is reached by any harness; the symlinks only let the crate compile
under Kani.
</content>
</invoke>

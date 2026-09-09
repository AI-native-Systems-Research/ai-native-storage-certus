# Verified properties — memory-tier (Kani)

Clean-slate re-run on branch `verif/kani/memory-tier-rerun` (baseline `verif/kani/memory-tier@dfde3983`).
Verified from spec `specs/001-memory-tier/spec.md` against `src/allocator.rs` + `src/lib.rs`, consuming
the id-keyed inventory `verif_memory-tier_property_inventory.md` (N=17 methods, M≈54 properties, 11
globals). Harnesses: `#[cfg(kani)] mod verification` in `src/allocator.rs` (native `align_up` + BTreeMap
walls) and in `src/lib.rs` (HashMap construction wall). Toolchain: Kani 0.67.0 / CBMC 6.8.0.
Run: `cargo kani -p memory-tier`.

**Headline (per-method, three-end-states rule): strict 0/17, relaxed 3/17.**
Every one of the 17 `IMemoryTier` methods is reachable only through data structures CBMC cannot execute
tractably — a `std::collections::HashMap<CacheKey,Slot>` slot map (getrandom + SipHash) and a
`BTreeMap<usize,usize>`-backed `FreeList` allocator (node navigation) — plus `mmap`/SPDK FFI and raw DMA
pointers. The verifiable core that survives BMC is (1) the alignment primitive `align_up` (ALIGN-4K) and
(2) `FreeList::new` establishment (a single BTreeMap insert is tractable). Everything else is an
evidence-backed tool wall (⊘, signature captured by an actual run) or a genuine cross-component delegation
(⤴ to `IEvictionPolicy`).

## Proved (native, this tree)

### `align_up` — the 4 KiB alignment primitive (ALIGN-4K / FR-004, `src/allocator.rs`)
`align_up(size) = size.next_multiple_of(4096)`; every `allocate`/`deallocate` routes its size through it.
Kani proves each fact over the **full production domain** `size ∈ [1, u32::MAX]` (the `insert(key, size:
u32)` type bound), symbolically — not by sampling. No unwind needed (straight-line 64-bit arithmetic).

- `ALIGN-4K` **[Postcondition]** `align_up(size)` is a multiple of 4096, never below `size`, and rounds
  up by strictly less than one page; the arithmetic never overflows/panics. — FR-004 / SC-4 — harness
  `verify_align_up_contract` (SUCCESSFUL, Verification Time 0.052 s).
- `ALIGN-4K` **[Invariant]** `align_up` is idempotent (already-aligned ⇒ fixpoint); this is why
  re-aligning `slot.size` in `deallocate` recovers exactly the block `allocate` charged (USED-CONSERVE
  symmetry). — FR-004 — harness `verify_align_up_idempotent` (SUCCESSFUL, 0.040 s).
- `ALIGN-4K` **[Invariant]** `align_up` is monotonic non-decreasing, so first-fit's `region_size >=
  aligned_size` test is order-consistent. — FR-004 — harness `verify_align_up_monotonic` (SUCCESSFUL,
  0.046 s).

### `FreeList::new` — allocator establishment (`src/allocator.rs`)
A single BTreeMap insert (`free_regions.insert(0, capacity)`) IS tractable at `#[kani::unwind(4)]` — a
finding this re-run adds over the baseline, which declared *all* FreeList harnesses intractable.

- `INIT-EMPTY` (allocator half) **[Postcondition]** a fresh `FreeList` has `used()==0`. Implements
  INIT-EMPTY (for `initialize`) and the allocator half of `clear` (which rebuilds `FreeList::new`). —
  FR-018 / US-3.
- `CAP-CONST` (establishment) **[Postcondition]** `capacity()` equals the size passed to `new`. —
  FR-018 / US-5.
- Harness `verify_freelist_new_establishes` (SUCCESSFUL, Verification Time 2.855 s, unwind=4). Anti-vacuity
  confirmed (see below).

## Assumptions / bounds (assume audit)
- `kani::assume(size > 0)` mirrors the real guard: `insert()` → `InvalidSize` for `size==0` (FR-008) and
  `FreeList::allocate` early-returns `None` for 0, so `align_up` is never reached with 0 in production.
- `kani::assume(size <= u32::MAX as usize)` mirrors the interface type bound `insert(key, size: u32)` and
  `deallocate(offset, slot.size as usize)` with `slot.size: u32`. Full `align_up` domain = `[1,u32::MAX]`.
- The `wall_*` allocate/deallocate harnesses use a deliberately tiny geometry (`capacity = 2 * 4096`, one
  page request) at `#[kani::unwind(4)]` to give CBMC its best chance — establishment is tractable,
  allocate/deallocate are not (below).

## Not verified — attempted, tool boundary hit (⊘, signature captured by an actual run)

- **USED-LE-CAP / USED-CONSERVE — `FreeList::allocate`.** *(TOOL — intrinsic, std BTreeMap.)* Harness
  `wall_freelist_used_le_cap` (unwind=4, capacity 2 pages, symbolic `size ∈ (0,4096]`) calls the real
  BTreeMap-backed allocator. CBMC stalls unwinding BTreeMap node navigation — `btree::search::
  find_key_index`, `btree::navigate::first_leaf_edge`/`next_kv`, and `node::correct_childrens_parent_links`
  — from the `free_regions.iter().find(...)` first-fit search. It hits the unwind cap ("Not unwinding
  loop … iteration 4") and produces **no verdict** at the 8-min timebox: `timeout 480 → rc=124, wall
  8:00.00`. Route: **Creusot** (logic-level FMap over the map).
- **SPACE-REUSE / COALESCE — `FreeList::deallocate`.** *(TOOL — intrinsic, std BTreeMap.)* Harness
  `wall_freelist_space_reuse` (allocate 1 page → deallocate → assert `used()==0`) exercises the same
  BTreeMap navigation plus the `range(..offset)`/`get(&next)` coalesce probes and `deallocating_next`/
  `deallocating_end`. Outcome: `timeout 480 → rc=124, wall 8:00.00` (same navigation wall, no verdict).
  Route: **Creusot**.
- **CONTAINS-REFLECTS, INSERT-DEDUP, REMOVE-NOTFOUND, CLEAR-COUNT — `HashMap<CacheKey,Slot>` slot map.**
  *(TOOL — intrinsic, std HashMap, strictly worse than BTreeMap.)* Reached only via a
  `MemoryTierComponent`, whose `MemoryTierState::default()` builds a `HashMap` whose `RandomState` seeds
  through `std::sys::random::linux::getrandom` (foreign function) + `core::hash::sip::Hasher` (SipHash).
  See INIT-GATE wall.
- **INIT-GATE (all 16 non-initialize methods), INIT-ZERO, INSERT-ZERO, INIT-EP-NOT-CONNECTED.** *(TOOL —
  construction wall.)* Every method is a member of `MemoryTierComponent`; merely *constructing* one — even
  to call the cheapest gated accessor `capacity()`, which returns 0 before touching the pool — drags in
  `HashMap::new()` → getrandom/SipHash. Harness `wall_init_gate_capacity_uninit` RUN captured: CBMC warns
  "Found the following unsupported constructs", unwinds `core::hash::sip::Hasher::write` (259 iterations)
  and `std::sys::random::linux::getrandom` (64+ iterations), and does not terminate (rc=124 at the
  timebox). The private std path `std::sys::random::*` is not `kani::stub`-nameable without patching
  std/shared code, so no faithful stub was available. Route: **Creusot** (models the gate over abstract
  state, no getrandom).
- **PTR-IN-BOUNDS, INSERT-SUCCESS (writable ptr), GET-ROUNDTRIP, POOLINFO-RETURNS — raw `*mut u8`.**
  *(TOOL — no model.)* Pointers are `pool_ptr.add(offset)` into an `mmap`/SPDK region; CBMC has no model
  of the mapped allocation, so pointer-validity/DMA-suitability is not expressible. Route: integration.
- **INIT-MONOTONIC, INIT-DOUBLE, INIT-CAPACITY (method-level).** *(TOOL — FFI.)* Require a *successful*
  `initialize`, which enters `mmap`(+`MAP_HUGETLB`)/`spdk_zmalloc`/`mbind` FFI and `ep.create_pool()`.

## Not verified — delegated across a real interface boundary (⤴ IEvictionPolicy)
Behaviours of the bound `IEvictionPolicy` component (e.g. `eviction-policy-lru`), reached through a
receptacle — not memory-tier logic. (Confirmed by inventory reconciliation flag #2.)
- `OLDEST-ORDER`, `OLDEST-BOUNDED` (oldest-first, ≤ n) → `IEvictionPolicy::get_eviction_candidates`.
- victim selection in `EVICT-FREES` (which key) → `IEvictionPolicy::identify_next_to_evict`.
- `GET-TOUCHES`, `TOUCH-UPDATES`, `BATCHTOUCH-ALL/SKIP` (recency effect) → `IEvictionPolicy::touch` /
  `batch_touch`.

## Not verified — not expressible for a bounded checker
- **Concurrency**: single `RwLock<Pool>` serialization, lock-contention counters, out-of-lock eviction
  touches (FR-005/006, NFR-002). Route: **Loom**.
- **FFI/OS/hardware**: `mmap`+`MAP_HUGETLB` fallback, `mbind` NUMA binding, `spdk_zmalloc`/`spdk_free`,
  `is_dma_capable`, Drop/`munmap` (FR-001/002/003/019/020, NFR-006). Route: integration/e2e on real hw.

## IMemoryTier method coverage (17 methods) — strict vs relaxed
Strict = every inventory row for the method is a green native proof. Relaxed = ≥1 green row **and** every
remaining row at a legitimate end-state (Proved / evidence-⊘ / ⤴ / not-expressible), no open row.

| # | Method | Native green row(s) | Verdict |
|---|--------|---------------------|---------|
| 1 | initialize | INIT-EMPTY + CAP-CONST establishment (`FreeList::new`) | relaxed ✓ (strict ✗: FFI/double-init/gate ⊘) |
| 2 | insert | ALIGN-4K (allocation size via `align_up`) | relaxed ✓ (strict ✗: dedup/poolfull/ptr ⊘) |
| 3 | get | — | ✗ (all rows ⊘/⤴) |
| 4 | peek | — | ✗ |
| 5 | evict_next | — | ✗ (accounting ⊘, victim ⤴) |
| 6 | evict_next_for_key | — | ✗ (alias of evict_next) |
| 7 | oldest_keys | — | ✗ (guard ⊘, order/bound ⤴) |
| 8 | remove | — | ✗ (HashMap/BTreeMap ⊘) |
| 9 | touch | — | ✗ (recency ⤴, gate ⊘) |
| 10 | batch_touch | — | ✗ |
| 11 | contains | — | ✗ (CONTAINS-REFLECTS ⊘) |
| 12 | capacity | — | ✗ (allocator-level CAP-CONST proved, but method construction-walled) |
| 13 | used | — | ✗ |
| 14 | pool_info | — | ✗ (raw ptr ⊘) |
| 15 | is_dma_capable | — | ✗ (FFI-set bool, construction-walled) |
| 16 | clear | CLEAR-EMPTIES allocator half (`FreeList::new`) | relaxed ✓ (strict ✗: HashMap clear + count ⊘) |
| 17 | telemetry_snapshot | — | ✗ (atomics; concurrency semantics not-expressible) |

**Strict: 0/17. Relaxed: 3/17 (initialize, insert, clear).**

## Anti-vacuity validation
Each native green harness was fault-injected (a contract-violating mutation), confirmed to FAIL, then
reverted via `git checkout`. See the run report for the mutation/result matrix. The `wall_*` harnesses are
not green and need no vacuity check.

## Environment note
`cargo kani` builds the whole crate, which pulls `interfaces` (spdk feature) → `spdk-sys`, whose build
script needs a prebuilt SPDK tree. Satisfied via gitignored symlinks `deps/spdk` and `deps/spdk-build`. No
SPDK/FFI code is reached by any harness; the symlinks only let the crate compile under Kani.

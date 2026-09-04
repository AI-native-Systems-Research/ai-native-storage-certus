# Verified properties — memory-tier (Creusot)

Proven from spec `specs/001-memory-tier/spec.md` (FR / User Story / Success
Criteria) against code in `components/memory-tier/src/allocator.rs` (the
`FreeList` first-fit allocator) and `src/lib.rs` (the `IMemoryTier` accounting
paths). Artifacts: this `verif/` crate.

Coverage is aggregated onto the public interface **`IMemoryTier`**
(`components/interfaces/src/imemory_tier.rs`, 17 methods). This run **extends**
the prior 8-proof allocator core with **4 new proofs** (`free_capacity`,
`clear_reset`, `deallocate_merge`, `align_up_idempotent`) under the
coverage-discipline policy: attempt every property the tool can express, and
label every miss **TOOL** (intrinsic/ecosystem limit) vs **AGENT** (provable,
not yet done).

**Method.** Each proved function is a **faithful whole-function mirror** of a
shipped arithmetic fragment. The memory-tier crate cannot be built under Creusot
(it depends on `interfaces`, the component framework, `std::collections::{HashMap,
BTreeMap}`, `RwLock`/atomics, `libc`/SPDK FFI and `*mut u8` pointers), so each
mirror reproduces the source statements and attaches the spec-derived contract
(`#[requires]` / `#[ensures]`). Every mirror cites the source line range it
tracks. `align_up_idempotent` is the one exception: it is not a single source
statement but a **derived structural property** of the shipped `align_up` path
(idempotence), proved by composing the mirror twice.

**What is in scope.** The **arithmetic and accounting core** of the allocator
and of `IMemoryTier`'s accounting accessors: 4 KiB alignment (and its
idempotence), the allocate split, `used`/`capacity`/`free_capacity` accounting,
the coalescing offset math (each neighbour and the full both-neighbour merge),
and the `clear` accounting reset. **Out of scope for Creusot** (see the
TOOL-labelled sections): the `BTreeMap`/`HashMap` container operations, raw
`*mut u8` DMA pointers, mmap/hugepage/NUMA/SPDK FFI, and all `RwLock`/atomic
concurrency (the latter is Loom's job).

**Honesty caveat.** A green proof here covers the **mirror**, not the shipped
function. The residual gap is the body-equality obligation between mirror and
source, discharged **by inspection** against the cited line ranges — there is no
automated equality check binding a mirror to `allocator.rs`/`lib.rs`. There are
**no `#[trusted]` items inside this crate** — every VC is discharged by the
prover portfolio.

**Result.** `cargo creusot` → `Proved (12 files) ✔`.
**48 VCs discharged across 12 `.coma` files** (prior run: 41 VCs / 8 files),
all closed by the **alt-ergo** prover from the configured portfolio
(`why3find.json`: alt-ergo / z3 / cvc5 / cvc4; tactics `split_vc` +
`compute_specified`). Every one of the 4 newly proved functions was validated
**anti-vacuously** by fault injection (see "Anti-vacuity" below).

Build (creusot-std is not vendored on this branch; `Cargo.toml` points the
dependency at the sibling toolchain checkout that ships it — a read-only path
reference, never modified):

```
cd components/memory-tier/verif && cargo creusot
```

Per-file VC counts (leaf prover goals): `align_up` 15, `align_up_idempotent` 2,
`alloc_admits` 2, `allocate_split` 4, `clear_reset` 1, `coalesce_next` 3,
`coalesce_prev` 7, `deallocate_merge` 3, `deallocate_used` 1, `free_capacity` 1,
`leftover_offset_aligned` 4, `lifecycle_alloc_free` 5 = **48**.

---

## align_up — mirrors `allocator.rs:42` (`size.next_multiple_of(ALIGNMENT)`)

Spec: **FR-004**, **SC-4** (all allocations 4 KiB-aligned for NVMe DMA).

- **[Postcondition]** The rounded size is a multiple of 4096. — FR-004, SC-4 —
  proved: `align_up.coma` (15/15 VCs)
- **[Postcondition]** `result >= size` — never returns fewer bytes than requested.
- **[Postcondition]** `result < size + 4096` — minimal: internal fragmentation is
  under one page (matches the spec's "waste up to 4095 bytes" note).
- **[Precondition]** `size > 0` and `size + 4095 <= usize::MAX` (overflow-free).

  Needed two SMT bridges (`proof_assert!`): the ComputerDivision identity
  `n == q*4096 + r`, `r < 4096`, and `Div_mult` (`(q*4096)/4096 == q`) to
  discharge `result % 4096 == 0`.

## align_up_idempotent — derived structural property of `align_up` (FR-004) **[NEW]**

- **[Postcondition]** Applying `align_up` twice equals applying it once
  (`result.1 == result.0`): an already-aligned size is a **fixed point** of
  `align_up`. Proved purely from `align_up`'s contract — the only multiple of
  4096 in `[size, size+4096)` is `size` itself. This is what makes the alignment
  invariant stable under repeated allocation (the leftover offset is a fixed
  point). — FR-004 — proved: `align_up_idempotent.coma` (2/2 VCs)
- **[Precondition]** `size > 0`, `size + 8191 <= usize::MAX` (two align_up calls).

## alloc_admits — mirrors `allocator.rs:39-41` (`if size == 0 { return None; }`)

Spec: **FR-008** (insert rejects zero size with InvalidSize).

- **[Postcondition]** A zero-size request is always rejected; any positive size is
  admitted. — FR-008 — proved: `alloc_admits.coma` (2/2 VCs)

## allocate_split — mirrors `allocator.rs:44-54` (split of the found free region)

Runs after first-fit has selected a region `(offset, region_size)` with
`region_size >= aligned_size`. Spec: accounting (User Story 5), **FR-010**.

- **[Postcondition]** `used` grows by exactly `aligned_size`. — proved:
  `allocate_split.coma` (4/4 VCs)
- **[Postcondition]** Remaining free bytes = `region_size - aligned_size`
  (no underflow, given the first-fit precondition).
- **[Postcondition]** The leftover region starts at `offset + aligned_size` and
  stays within the pool (`leftover_offset + remaining <= capacity`); `used` never
  exceeds `capacity`. — FR-010.
- **[Precondition]** `aligned_size` a positive multiple of 4096, `region_size >=
  aligned_size`, region within pool, `used + aligned_size <= capacity`.

## deallocate_used — mirrors `allocator.rs:60-61` (`self.used -= aligned_size`)

- **[Postcondition]** `used` decreases by exactly `aligned_size`. — accounting —
  proved: `deallocate_used.coma` (1/1 VC)
- **[Precondition]** `used >= aligned_size` — a free never underflows `used`.

## coalesce_prev — mirrors `allocator.rs:66-73` (coalesce with preceding region)

Spec: **FR-026** (coalesce adjacent free regions on deallocation).

- **[Postcondition]** When the preceding region is exactly adjacent
  (`prev_offset + prev_size == offset`) the two merge into
  `(prev_offset, prev_size + size)`; otherwise the freed region is unchanged. —
  FR-026 — proved: `coalesce_prev.coma` (7/7 VCs)
- **[Invariant]** Preserves the right endpoint (`new_offset + new_size ==
  offset + size`) and only extends leftward (`new_offset <= offset`).
- **[Precondition]** No overlap (`prev_offset + prev_size <= offset`); merged size
  overflow-free.

## coalesce_next — mirrors `allocator.rs:76-80` (coalesce with following region)

Spec: **FR-026**.

- **[Postcondition]** The region grows by exactly the following region's size
  (`result == new_size + next_size`) and never shrinks. — FR-026 — proved:
  `coalesce_next.coma` (3/3 VCs)
- **[Precondition]** Merged size overflow-free.

## deallocate_merge — the FULL deallocate coalescing path (FR-026, FR-015) **[NEW]**

Composes `coalesce_prev` (P5) then `coalesce_next` (P6) exactly as `deallocate()`
runs them (`allocator.rs:63-82`). `next_present` models the trusted
`BTreeMap::get(&next_offset).is_some()` lookup (`allocator.rs:77`); `next_size` is
that region's size. Backs `remove()`/`evict_next()`.

- **[Postcondition — extend-only]** Coalescing across both neighbours only ever
  extends the region leftward: adjacent prev ⟹ `result.0 == prev_offset`; gap ⟹
  `result.0 == offset`; always `result.0 <= offset`. — FR-026 — proved:
  `deallocate_merge.coma` (3/3 VCs)
- **[Postcondition — freed span preserved]** `result.0 + result.1 >= offset +
  size` — the freed bytes are always still covered by the merged region.
- **[Postcondition — byte conservation]** The right endpoint grows by `next_size`
  **iff** a following region merged: `next_present ⟹ result.0 + result.1 ==
  offset + size + next_size`; `!next_present ⟹ result.0 + result.1 == offset +
  size`. Together with the extend-only clauses this pins the full merge exactly.
  — FR-026, FR-015.
- **[Precondition]** `prev_offset + prev_size <= offset`; both the prev-merge and
  the full-merge sums are overflow-free.

## free_capacity — mirrors `lib.rs:186` (`capacity() - used()`), FR-029 **[NEW]**

Spec: **FR-029** (`free_capacity()` returns `capacity() - used()` for
proactive-eviction triggers). Backing accessors: `capacity()`/`used()`.

- **[Postcondition]** `result == capacity - used` — exact complement of `used`. —
  FR-029 — proved: `free_capacity.coma` (1/1 VC)
- **[Postcondition]** `result <= capacity` — free bytes never exceed capacity.
- **[Precondition]** `used <= capacity` — the accounting invariant that
  `allocate_split` (P3) establishes and preserves.

## clear_reset — mirrors `lib.rs:605` / `allocator.rs:16-26` (`FreeList::new`), FR-018 **[NEW]**

Spec: **FR-018** / User Story 4 (`clear()` removes all entries, resets the
allocator, returns count). `clear()` replaces the allocator with
`FreeList::new(pool_size)`, which sets `used = 0`, `capacity = pool_size`.

- **[Postcondition]** After clear, `used == 0`. — FR-018 — proved:
  `clear_reset.coma` (1/1 VC)
- **[Postcondition]** After clear, `capacity == pool_size` (full pool restored).

  The returned entry count is `HashMap::len()` — a trusted container op, **not**
  modeled here. What is proved is the allocator's arithmetic reset.

## lifecycle_alloc_free — composes allocate_split + deallocate_used

- **[Postcondition]** Allocating `aligned_size` out of a region and then freeing
  the same amount restores `used` to its original value (`result == used`) — the
  allocator's **accounting-conservation** invariant across an alloc→free cycle,
  behind `remove_and_reuse` / `capacity_and_used`. — proved:
  `lifecycle_alloc_free.coma` (5/5 VCs)

## leftover_offset_aligned — inductive step for offset alignment

- **[Postcondition]** An aligned region start plus an aligned carve yields an
  aligned leftover start (`result % 4096 == 0`, `result >= offset`). With the
  offset-0 base case (`allocator.rs:19`) this establishes by induction that
  **every free-region start — hence every returned allocation offset — is 4 KiB
  aligned**, so returned pointers are directly usable for NVMe DMA. — FR-004,
  NFR-010, SC-4 — proved: `leftover_offset_aligned.coma` (4/4 VCs)

---

## Anti-vacuity (fault injection, this run)

All 4 newly proved functions were fault-injected and confirmed to turn **red
(✘)**, then reverted; the 8 prior proofs stayed green throughout:

- `free_capacity` — body `capacity - used` → `capacity + used`: `vc_free_capacity ✘`.
- `clear_reset` — body `new_used = 0` → `1`: `vc_clear_reset ✘`.
- `deallocate_merge` — added false clause `result.0 > offset`: `vc_deallocate_merge ✘ (14/16)`.
- `align_up_idempotent` — postcond `result.1 == result.0` → `== result.0 + 4096`:
  `vc_align_up_idempotent ✘ (3/4)`.

(The prior 8 proofs were fault-injection–validated in the 2026-08-20 run.)

---

## IMemoryTier — 17-method coverage map

Legend: **Covered** = the method's spec-relevant *computational core* is proved;
**Partial** = some provable arithmetic proved, but material behavior stays
unproved (container/FFI/pointer/concurrency); **Not** = nothing in the method is
expressible in Creusot (pure container/FFI/pointer/concurrency).

| # | IMemoryTier method | Status | What is proved / why not |
|---|--------------------|--------|--------------------------|
| 1 | `initialize` | Partial | Allocator accounting init (`used=0`, `capacity=pool_size` via `FreeList::new`, = `clear_reset` P10); zero-size→InvalidSize is P2-shaped. NOT: mmap/hugepage/NUMA `mbind`/SPDK FFI, double-init atomic flag, eviction-policy `create_pool`. |
| 2 | `insert` | Partial | Core allocation math proved: P1 `align_up`, P2 zero-size reject, P3 `allocate_split`, P8 leftover alignment. NOT: duplicate-key detection (`HashMap`), `PoolFull` under fragmentation (`BTreeMap` first-fit), raw `*mut u8` return. |
| 3 | `get` | Not | `HashMap` lookup + raw `*mut u8` return + eviction-policy `touch` (interior mutability / concurrency). |
| 4 | `peek` | Not | `HashMap` lookup + raw `*mut u8` return. |
| 5 | `evict_next` (`evict_lru`) | Partial | Post-victim free-slot math proved: P4 `used` decrement, P5/P6 coalescing, P11 full `deallocate_merge`, P7 conservation. NOT: victim selection (eviction policy, concurrency), `HashMap::remove`. |
| 6 | `evict_next_for_key` (`evict_lru_for_key`) | Partial | Alias of `evict_next` (FR-014, `key` ignored — single unsharded pool). Same dealloc math; same NOTs. |
| 7 | `oldest_keys` | Not | Delegates to `IEvictionPolicy::get_eviction_candidates`; returned `Vec` is the policy's. Only the `n==0 ⇒ empty` guard is trivially modelable (AGENT, low value). |
| 8 | `remove` | Partial | Free-slot math proved: P4/P5/P6/P7/P11. NOT: `HashMap::remove` + `KeyNotFound` (container), eviction-policy `remove`. |
| 9 | `touch` | Not | `HashMap` lookup + eviction-policy `touch` over `&self` interior mutability → concurrency (Loom). |
| 10 | `batch_touch` | Not | As `touch` over a slice; container + concurrency (Loom). |
| 11 | `contains` | Not | `HashMap::contains_key` + initialized flag — thin accessor over a container. |
| 12 | `capacity` | Covered | Returns `allocator.capacity()`; its accounting meaning (constant; `used <= capacity`) is proved by P3/P9. (Method body is a `RwLock` read of a field — the lock is concurrency, out of scope.) |
| 13 | `used` | Covered | Best-covered accessor: P3 (grows by `aligned_size`, `<= capacity`), P4 (shrinks), P7 (conservation), P9 (`free_capacity` complement). |
| 14 | `pool_info` | Not | Returns raw base `*mut u8` + size; pointer reasoning is weak in Creusot. Only the `initialized && !null ⇒ Some` guard is trivial. |
| 15 | `is_dma_capable` | Not | Returns the `spdk_allocated` bool set by the FFI init path; no arithmetic/structural content. |
| 16 | `clear` | Partial | P10 `clear_reset` proves the accounting reset (`used→0`, `capacity→pool_size`). NOT: `HashMap::clear` + count (`len`), eviction-policy `clear_pool`. |
| 17 | `telemetry_snapshot` | Not | Reads relaxed atomics (concurrency) or returns zeros; no arithmetic/structural content. |

**Tally: Covered 2 (`capacity`, `used`) · Partial 6 (`initialize`, `insert`,
`evict_next`, `evict_next_for_key`, `remove`, `clear`) · Not 9 (`get`, `peek`,
`oldest_keys`, `touch`, `batch_touch`, `contains`, `pool_info`,
`is_dma_capable`, `telemetry_snapshot`).**

Interpretation: Creusot's realizable yield on this component is the **allocator
arithmetic + accounting core** shared by the 6 mutating/accounting methods, plus
the 2 accounting accessors. The other 9 methods are container lookups, raw DMA
pointers, FFI, or shared-cache recency/concurrency — none expressible as a
sequential arithmetic contract here.

---

## Measurement (hard project rule — `/usr/bin/time -v`)

| Run | Wall clock (Elapsed) | Peak RSS (Maximum resident set size) | Outcome |
|-----|----------------------|--------------------------------------|---------|
| Baseline, 8 files, clean (compile creusot-std + prove) | 0:27.20 | 817,400 KB (~799 MiB) | Proved (8 files) ✔ |
| Extended, 12 files, clean (compile + prove) | 0:24.61 | 818,024 KB (~799 MiB) | Proved (12 files) ✔ |
| Extended, 12 files, cached proof replay only | 0:00.94 | 122,220 KB (~119 MiB) | Proved (12 files) ✔ |

Peak RSS is dominated by compiling `creusot-std` and the Creusot translation, not
by the SMT replay (the 4 added arithmetic proofs are cheap: alt-ergo closes each
new VC in < 0.05 s). Host: this worktree's build node.

---

## Assumptions / trusted boundaries

- **`BTreeMap` / `HashMap` container operations are trusted / unmodeled.** The
  first-fit scan (`allocator.rs:44`), preceding-region lookup (`:67`), following-
  region lookup (`:77`), and every `slots` `HashMap` op (`insert`/`get`/`remove`/
  `contains_key`/`clear`/`len`) are not modeled. The proofs assume these lookups
  behave as their preconditions state (e.g. first-fit returns a region with
  `region_size >= aligned_size`; `range(..offset).next_back()` returns a region
  with `prev_offset + prev_size <= offset`; `get(&next_offset)` — here the
  `next_present`/`next_size` inputs to `deallocate_merge` — returns a region
  starting exactly at `new_offset + new_size`). Properties that depend on the
  **global** free-region set (no two live free regions overlap; every byte is free
  or allocated exactly once; the free set stays sorted) are therefore **not**
  proved.
- **These are mirrors, not the shipped functions.** `allocate()`/`deallocate()`
  and the `IMemoryTier` bodies cannot build under Creusot (BTreeMap/HashMap,
  `*mut u8`, `RwLock`/atomics, `libc`/SPDK FFI, the component framework). Mirror
  bodies transcribe the arithmetic statements and cite source lines; a green proof
  covers the mirror. The residual mirror-vs-source drift is checked by eye against
  the citations — there is no automated equality check.
- **`next_multiple_of` is modeled, not called.** `align_up` implements
  `ceil(size/4096)*4096` and proves it equivalent over `size > 0`; creusot-std
  provides no contract for the real `next_multiple_of`.
- **`creusot-std`** (the `creusot-contracts` runtime, `Int`/`Seq` models, division
  lemmas) is trusted, as in every Creusot proof.
- **Prover portfolio** alt-ergo / z3 / cvc5 / cvc4 is trusted (all 48 VCs happened
  to be closed by alt-ergo).
- **Overflow / range preconditions are assumptions, not proofs** — e.g.
  `size + 4095 <= usize::MAX` (align_up), `used <= capacity` (free_capacity), the
  overflow-free merged-size bounds (deallocate_merge). These bound inputs the
  shipped code also assumes implicitly (a 256 MiB default pool is far from
  `usize::MAX`).
- **No `#[trusted]` items exist inside this crate.**

## Mirror-vs-source drift caveat

This doc and its `.coma` proof artifacts live on branch
`verif/creusot/memory-tier`. They are **not** re-derived from `allocator.rs` /
`lib.rs` on every source edit — a change to the shipped allocator or the
`IMemoryTier` accounting paths that is not mirrored here will silently drift, and
the line-range citations above are the only tie binding mirror to source. Treat a
green `Proved (12 files) ✔` as evidence about the mirrors as of the cited line
ranges, not a live invariant of `main`. (Same caveat the extent-manager doc
carries.)

## Not proved — attempted / expressible-but-deferred (AGENT)

- **`PoolFull` as a faithful decision (FR-010).** The accounting *necessary*
  condition — when `used == capacity`, no positive request fits — is trivially
  provable, but adding it as a mirror would insert a byte-count check the shipped
  `allocate()` does **not** perform (it returns `PoolFull` from the first-fit
  *search* failing under fragmentation). A faithful proof of `PoolFull` requires
  the free-region set (TOOL, below). Deliberately **not** added to avoid a
  non-faithful mirror; the `used <= capacity` bound is already proved (P3).
- **`initialize` zero-size → InvalidSize** and **`oldest_keys` `n==0 ⇒ empty` /
  `len <= n` clamp.** Trivially P2-shaped guards; not mirrored because their yield
  is negligible next to the substantive behavior (FFI init; eviction-policy
  delegation), which is TOOL.
- **Global free-list invariants over a Vec reimplementation.** Non-overlap, full
  coverage, and sortedness of the free set are AGENT-reachable by reimplementing
  the allocator over a Creusot-supported `Vec<(usize,usize)>` with loop
  invariants — but that is a large, explicitly non-shipped mirror (the shipped
  code is `BTreeMap`-backed). Deferred; same call the extent-manager buddy-merge
  conservation proof made.

## Not proved — not expressible in Creusot (TOOL)

- **`BTreeMap`/`HashMap` semantics** — no creusot-contracts extern specs for the
  ordered-map range/scan or the hash-map ops this component relies on; the global
  free-region invariants they carry are unreachable without a modeled container.
- **Raw `*mut u8` DMA pointers** — `insert`/`get`/`peek`/`pool_info` return/derive
  `state.pool_ptr.add(offset)`; Creusot's pointer/aliasing model does not let us
  state the DMA-validity or provenance properties these methods are about.
- **FFI init / teardown** — mmap + `MAP_HUGETLB` fallback, `mbind(MPOL_BIND)` NUMA
  binding with graceful fallback (FR-002/019), `spdk_zmalloc`/`spdk_free`
  (FR-003/020), `munmap` on Drop (NFR-006). External effects, out of scope.
- **Concurrency** — the single `RwLock<Pool>` discipline (FR-005/006, NFR-002),
  data-path touches applied outside the pool lock, relaxed telemetry atomics
  (FR-027/028), and SC-3 (no deadlock/corruption under 16+ threads). Creusot
  reasons about sequential bodies, not lock protocols or interleavings — **this is
  Loom's job**, not Creusot's.
- **Eviction-policy delegation** — victim selection (`identify_next_to_evict`),
  `touch`/`batch_touch`, `get_eviction_candidates`, `create_pool`/`clear_pool` are
  calls into an external `IEvictionPolicy` component with its own interior
  mutability; recency/LRU correctness lives there (and under Loom), not in this
  crate.

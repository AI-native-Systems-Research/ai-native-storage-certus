# Verified properties — memory-tier (Creusot)

Proven from spec `specs/001-memory-tier/spec.md` (FR / User Story / Success
Criteria) against code in `components/memory-tier/src/allocator.rs` (the
`FreeList` first-fit allocator) and `src/lib.rs` (the `IMemoryTier` accounting
paths). Artifacts: this `verif/` crate.

Coverage is aggregated onto the public interface **`IMemoryTier`**
(`components/interfaces/src/imemory_tier.rs`, 17 methods). This run consumes the
**property inventory** (`memory-tier_property_inventory.md`, Role 1) and works
the **global-invariant ledger top-down by attachment count**. It **extends** the
prior 12-proof allocator/accounting core with **15 new proof functions**
covering the ledger's highest-leverage items: `INIT-GATE` (rank 1, +16 methods),
`INIT-MONOTONIC` + `CAP-CONST` (the init latch and fixed capacity),
`PTR-IN-BOUNDS` (rank 8), a `SPACE-REUSE` extension (rank 7), and the
`CONTAINS-REFLECTS` (rank 4) `FMap` slot-map campaign. Every proved / not-proved
line below cites its inventory `id`.

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

**Result.** `cargo creusot` → `Proved (30 files) ✔`.
**27 authored proof functions** (prior run: 12) plus 3 auto-derived `Clone`
goals = **30 `.coma` / `proof.json` files**. Under the current `proof.json`
metric these carry **48 top-level `vc_*` goals** that expand to **107 leaf
prover goals** after `split_vc`, all closed by the **alt-ergo** prover from the
configured portfolio (`why3find.json`: alt-ergo / z3 / cvc5 / cvc4; tactics
`split_vc` + `compute_specified`).

> **Metric note.** The prior doc reported "48 VCs / 12 files" using a *per-file
> leaf-goal* count. This run counts from `proof.json`: 48 = top-level `vc_*`
> goals (one or more per function), 107 = prover-invocation leaves after
> `split_vc`. The change is a **measurement convention**, not a regression — no
> previously green goal went red. Prior 12 functions ⇒ 18 of the 48 top-level
> goals; the 15 new functions ⇒ the remaining 30 (+ 3 trivial derived `Clone`).

Every one of the **15 newly proved functions** was validated **anti-vacuously**
by fault injection — all 15 turned red (✘) then reverted (see "Anti-vacuity").

Build (creusot-std is not vendored on this branch; `Cargo.toml` points the
dependency at the sibling toolchain checkout that ships it — a read-only path
reference, never modified):

```
cd components/memory-tier/verif && cargo creusot
```

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

# Global-invariant ledger items (this run)

Worked top-down by attachment count from `memory-tier_property_inventory.md`.
Each item cites its inventory `id`.

## INIT-GATE — `id: INIT-GATE` (rank 1, 16 method attachments) **[NEW]**

Every data-path method returns a *gated* sentinel and performs **no state
mutation** while `!initialized`. The gate manifests in four shapes; one mirror
per shape, each `#[requires(!state.initialized)]` proving both the sentinel and
frame (`result_state == state`), plus a representative *initialized-path*
mutation to show the guard, not vacuity, produces the sentinel:

- `gate_mutator_result` — `insert` (lib.rs:334), `remove` (:474), `clear` (:595)
  ⇒ `Err(NotInitialized)`, state unchanged. Proved: `gate_mutator_result.coma`.
- `gate_mutator_option` — `evict_next` (:436), `evict_next_for_key` ⇒ `None`,
  state unchanged. Proved: `gate_mutator_option.coma`.
- `gate_readonly_option` — `get` (:383), `peek` (:414), `pool_info` (:587) ⇒
  `None`. Proved: `gate_readonly_option.coma`.
- `gate_readonly_scalar` — `contains` (:559)→false, `capacity` (:569)/`used`
  (:578)→0, `oldest_keys`, `is_dma_capable` (:610), `telemetry_snapshot` (:615)
  → default/0. Proved: `gate_readonly_scalar.coma`.
- `gate_noop` — `touch` (:507), `batch_touch` (:525) ⇒ silent return, state
  unchanged. Proved: `gate_noop.coma`.

Proving each shape once discharges the gated-behavior bundle for all 16 methods
(the shape is identical; only the sentinel type differs). — `id: INIT-GATE`.

## INIT-MONOTONIC + CAP-CONST — `id: INIT-MONOTONIC`, `id: CAP-CONST` **[NEW]**

- `initialize_state` — establishes the latch: from `!initialized`, after init
  `initialized == true`, `capacity == pool_size`, `used == 0`; a **double-init**
  frame shows a second call does not change the accounting. — `id: INIT-MONOTONIC`,
  `id: CAP-CONST` — proved: `initialize_state.coma`.
- `mutator_preserves_init` — `#[requires(state.initialized)]`,
  `#[ensures(result.initialized)]`: any accounting mutation preserves the latch
  (once true, never reset) and holds `capacity` fixed while `used` moves. —
  `id: INIT-MONOTONIC`, `id: CAP-CONST` — proved: `mutator_preserves_init.coma`.

## PTR-IN-BOUNDS — `id: PTR-IN-BOUNDS` (rank 8, 4 attachments) **[NEW]**

- `ptr_in_bounds` — a returned allocation at `offset` with `aligned_size` carved
  from a region of `region_size` satisfies `offset + aligned_size <= pool_size`:
  the pointer `base.add(offset)` and its span stay inside the mapped pool. —
  `id: PTR-IN-BOUNDS` — proved: `ptr_in_bounds.coma`. (This is the *arithmetic*
  in-bounds fact; the raw-pointer provenance/DMA-validity remains TOOL, below.)

## SPACE-REUSE — `id: SPACE-REUSE` (rank 7) extension **[NEW]**

- `space_reuse_free_grows` — freeing `aligned_size` grows free capacity by
  exactly that amount (`capacity - new_used`, with `new_used = used -
  aligned_size`). — `id: SPACE-REUSE`.
- `space_reuse_two_cycles` — two alloc→free round-trips return `used` to its
  start (free space is fully reusable, no accounting leak across cycles). —
  `id: SPACE-REUSE` — proved: `space_reuse_free_grows.coma`,
  `space_reuse_two_cycles.coma`. Extends `lifecycle_alloc_free` (P7) to the
  reuse claim.

## CONTAINS-REFLECTS — `id: CONTAINS-REFLECTS` (rank 4, 6 attachments) **[NEW — LANDED]**

Models the `slots` map as a logic-level `creusot_std::logic::FMap<u64,
SlotMirror>` and proves that `contains(k)` reflects membership, maintained by
every mutator. Four universally-quantified logic lemmas + one operational ghost
trace:

- `contains_reflects_insert` — `#[ensures(m.insert(k,v).contains(k))]` plus the
  frame (`j != k ==> insert preserves contains(j)`) and `get(k) == v`. — proved:
  `contains_reflects_insert.coma`.
- `contains_reflects_insert_len` — a **fresh** key grows `len` by exactly 1
  (`!m.contains(k) ==> m.insert(k,v).len() == m.len() + 1`); a present key does
  not grow it. — proved: `contains_reflects_insert_len.coma`.
- `contains_reflects_remove` — `#[ensures(!m.remove(k).contains(k))]` plus the
  frame; models both `remove()` and `evict_next()` deleting exactly the victim
  key. — proved: `contains_reflects_remove.coma`.
- `contains_reflects_clear` — `forall<k> !empty().contains(k)`: after `clear()`
  no key is present. — proved: `contains_reflects_clear.coma`.
- `contains_reflects_trace` — an **operational** `ghost!` trace exercising
  empty→insert→insert→remove→evict→clear with `proof_assert!` checks on
  `contains` at each step (the program-level `insert_ghost`/`remove_ghost`/
  `clear_ghost` API). — proved: `contains_reflects_trace.coma`.

This lands the `contains`/`insert`/`remove`/`evict`/`clear` membership
invariant that was previously listed as a container gap. **Boundary:** the model
is an `FMap`, not the shipped `HashMap` — the lemmas prove the *membership
algebra* the real `slots` map must satisfy; binding it to `std::HashMap`'s ops
remains a trusted mirror step (see boundaries).

---

## Anti-vacuity (fault injection, this run)

All **15 newly proved functions** were fault-injected (one contract- or
body-perturbation each that compiles but violates the property) and confirmed to
turn **red (✘)**, then reverted; the prior proofs stayed green throughout. Each
fault named its VC goal `Coma.vc_<fn>: ✘`:

- `initialize_state` — latch `initialized: true` → `false`: `vc_initialize_state ✘`.
- `mutator_preserves_init` — `initialized: state.initialized` → `false`: `vc_mutator_preserves_init ✘`.
- `gate_mutator_result` — drop the `!initialized` guard (`if false`): `vc_gate_mutator_result ✘`.
- `gate_mutator_option` — drop the guard: `vc_gate_mutator_option ✘`.
- `gate_readonly_option` — gated `None` → `Some(0)`: `vc_gate_readonly_option ✘`.
- `gate_readonly_scalar` — gated `0` → `1`: `vc_gate_readonly_scalar ✘`.
- `gate_noop` — gated no-op mutates `used` → `0`: `vc_gate_noop ✘`.
- `ptr_in_bounds` — `offset + aligned_size` → `offset + region_size`: `vc_ptr_in_bounds ✘`.
- `space_reuse_free_grows` — `capacity - new_used` → `capacity - used`: `vc_space_reuse_free_grows ✘`.
- `space_reuse_two_cycles` — skip the free (`u2 = u1`): `vc_space_reuse_two_cycles ✘`.
- `contains_reflects_insert` — `contains(k)` → `!contains(k)`: `vc_contains_reflects_insert ✘`.
- `contains_reflects_insert_len` — fresh `+1` → `+2`: `vc_contains_reflects_insert_len ✘`.
- `contains_reflects_remove` — `!contains(k)` → `contains(k)`: `vc_contains_reflects_remove ✘`.
- `contains_reflects_clear` — `!empty().contains` → `empty().contains`: `vc_contains_reflects_clear ✘`.
- `contains_reflects_trace` — assert `contains(1)` → `!contains(1)`: `vc_contains_reflects_trace ✘`.

(The 12 prior proofs were fault-injection–validated in earlier runs.)

---

## IMemoryTier — 17-method coverage map

Legend: **Covered** = the method's spec-relevant *computational core* is proved;
**Partial** = some provable arithmetic/structural behavior proved, but material
behavior stays unproved (container/FFI/pointer/concurrency); **Not** = nothing in
the method is expressible in Creusot (pure container/FFI/pointer/concurrency).
Every data-path method now additionally carries the **INIT-GATE** gated-behavior
proof (`id: INIT-GATE`) — while uninitialized the method returns its sentinel and
mutates nothing — noted as "+INIT-GATE" below.

| # | IMemoryTier method | Status | What is proved / why not |
|---|--------------------|--------|--------------------------|
| 1 | `initialize` | Partial | INIT-MONOTONIC latch + CAP-CONST proved (`initialize_state`: `used=0`, `capacity=pool_size`, double-init frame); accounting init = `clear_reset` P10; zero-size→InvalidSize is P2-shaped. NOT: mmap/hugepage/NUMA `mbind`/SPDK FFI, eviction-policy `create_pool`. |
| 2 | `insert` | Partial | +INIT-GATE. Core allocation math proved: P1 `align_up`, P2 zero-size reject, P3 `allocate_split`, P8 leftover alignment, PTR-IN-BOUNDS (arithmetic), CONTAINS-REFLECTS insert (membership+len). NOT: `PoolFull` under fragmentation (`BTreeMap` first-fit), raw `*mut u8` return semantics. |
| 3 | `get` | Partial | +INIT-GATE (gated `None`, no mutation). NOT: `HashMap` lookup value + raw `*mut u8` return + eviction-policy `touch` (interior mutability / concurrency). |
| 4 | `peek` | Partial | +INIT-GATE (gated `None`). NOT: `HashMap` lookup value + raw `*mut u8` return. |
| 5 | `evict_next` (`evict_lru`) | Partial | +INIT-GATE. Post-victim free-slot math proved: P4 `used` decrement, P5/P6 coalescing, P11 full `deallocate_merge`, P7 conservation, SPACE-REUSE, CONTAINS-REFLECTS remove (victim deleted). NOT: victim selection (eviction policy, concurrency). |
| 6 | `evict_next_for_key` (`evict_lru_for_key`) | Partial | Alias of `evict_next` (FR-014, `key` ignored — single unsharded pool). +INIT-GATE. Same dealloc math + CONTAINS-REFLECTS; same NOTs. |
| 7 | `oldest_keys` | Partial | +INIT-GATE (gated empty). NOT: delegates to `IEvictionPolicy::get_eviction_candidates`; returned `Vec` is the policy's. |
| 8 | `remove` | Partial | +INIT-GATE. Free-slot math proved: P4/P5/P6/P7/P11 + SPACE-REUSE + CONTAINS-REFLECTS remove (`!contains(k)` after). NOT: `HashMap::remove` value + `KeyNotFound` detection, eviction-policy `remove`. |
| 9 | `touch` | Partial | +INIT-GATE (gated no-op, no mutation). NOT: `HashMap` lookup + eviction-policy `touch` over `&self` interior mutability → concurrency (Loom). |
| 10 | `batch_touch` | Partial | +INIT-GATE (gated no-op). NOT: as `touch` over a slice; container + concurrency (Loom). |
| 11 | `contains` | Partial | +INIT-GATE (gated `false`). CONTAINS-REFLECTS proves the membership algebra (`contains(k)` iff `k` in map, maintained by insert/remove/clear). NOT: binding the `FMap` model to the shipped `HashMap::contains_key`. |
| 12 | `capacity` | Covered | +INIT-GATE (gated `0`). Returns `allocator.capacity()`; accounting meaning (constant via CAP-CONST; `used <= capacity`) proved by P3/P9. (Body is a `RwLock` read — concurrency out of scope.) |
| 13 | `used` | Covered | +INIT-GATE (gated `0`). Best-covered accessor: P3 (grows by `aligned_size`, `<= capacity`), P4 (shrinks), P7 (conservation), P9 (`free_capacity` complement), SPACE-REUSE. |
| 14 | `pool_info` | Partial | +INIT-GATE (gated `None`). PTR-IN-BOUNDS gives the arithmetic in-bounds fact for the base+span. NOT: raw `*mut u8` provenance/DMA-validity. |
| 15 | `is_dma_capable` | Not | +INIT-GATE (gated `false`). Otherwise returns the `spdk_allocated` bool set by the FFI init path; no arithmetic/structural content. |
| 16 | `clear` | Partial | +INIT-GATE. P10 `clear_reset` proves accounting reset (`used→0`, `capacity→pool_size`); CONTAINS-REFLECTS clear (no key present after). NOT: `HashMap::clear` value + count (`len`), eviction-policy `clear_pool`. |
| 17 | `telemetry_snapshot` | Not | +INIT-GATE (gated default). Otherwise reads relaxed atomics (concurrency) or returns zeros; no arithmetic/structural content. |

**Tally (spec-core status): Covered 2 (`capacity`, `used`) · Partial 13
(`initialize`, `insert`, `get`, `peek`, `evict_next`, `evict_next_for_key`,
`oldest_keys`, `remove`, `touch`, `batch_touch`, `contains`, `pool_info`,
`clear`) · Not 2 (`is_dma_capable`, `telemetry_snapshot`).**

The Partial count rose from 6 to 13 because **INIT-GATE now discharges the
gated-behavior obligation for all 16 data-path methods** and CONTAINS-REFLECTS
adds the membership algebra for the 5 slot-touching methods. This is a real but
**modest** gain per method: for the 7 methods that moved Not→Partial the *only*
newly proved content is the uninitialized-gate (and, where applicable, the
`FMap` membership model) — their initialized-path value (container lookups, raw
pointers, FFI, concurrency) is still TOOL and unproved. The 2 remaining Not
methods have no structural content even in their gate beyond the sentinel.

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
| Prior, 12 files, clean (compile + prove) | 0:24.61 | 818,024 KB (~799 MiB) | Proved (12 files) ✔ |
| **This run, 30 files, clean (compile + prove)** | **0:24.73** | **819,080 KB (~800 MiB)** | **Proved (30 files) ✔** |
| This run, 30 files, cached proof replay only | 0:01.14 | 150,208 KB (~147 MiB) | Proved (30 files) ✔ |

Peak RSS is dominated by compiling `creusot-std` and the Creusot translation, not
by the SMT replay: adding 15 proof functions (12→30 files) barely moved clean
wall-clock (24.61→24.73 s) or peak RSS (~800 MiB flat), because the new goals are
cheap — alt-ergo closes each new arithmetic/FMap VC in well under 0.05 s. Host:
this worktree's build node (`/usr/bin/time -v`).

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
- **The `TierState` mirror is a hand-written model of `MemoryTierState`.** The
  gate/latch proofs (INIT-GATE, INIT-MONOTONIC, CAP-CONST) run over a
  `TierState { initialized, capacity, used, spdk_allocated }` struct that mirrors
  the shipped state's *accounting* fields. The real `initialized: AtomicBool` is
  modeled as a plain `bool` — the atomic/`RwLock` protocol around it is **not**
  modeled (that is concurrency, Loom's job). What is proved is the sequential
  gate/latch logic assuming a single-threaded read of the flag.
- **The `FMap` slot model is not the shipped `HashMap`.** CONTAINS-REFLECTS proves
  the membership algebra over `creusot_std::logic::FMap<u64, SlotMirror>`; the real
  `slots: HashMap<CacheKey, Slot>` has no creusot-contracts extern spec. The lemmas
  establish the invariant the shipped map *must* satisfy, bound to `HashMap`'s ops
  only by inspection — not by an extern-spec'd container.
- **`creusot-std`** (the `creusot-contracts` runtime, `Int`/`Seq`/`FMap` models,
  division lemmas) is trusted, as in every Creusot proof.
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
green `Proved (30 files) ✔` as evidence about the mirrors as of the cited line
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
  **Exception (now landed):** the `slots` *membership algebra* (CONTAINS-REFLECTS)
  is proved over a logic-level `FMap` model — see the ledger section. What remains
  TOOL is binding that model to the concrete `HashMap` and the ordered `BTreeMap`
  free-region scan/range semantics.
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

# Verified properties — memory-tier (Creusot)

Proven from spec `specs/001-memory-tier/spec.md` against code `src/allocator.rs`
(the `FreeList` first-fit allocator). Artifacts: `verif/` (run `cargo creusot`
from `components/memory-tier/verif/`; all 8 goals discharge — `Proved (8 files) ✔`).

**What these proofs cover.** The real `FreeList` stores its free regions in a
`BTreeMap<usize, usize>`. Creusot cannot model `BTreeMap`, so the container
operations (first-fit iteration `.iter().find(...)`, the preceding-region lookup
`.range(..offset).next_back()`, and the following-region lookup `.get(&next_offset)`)
are **trusted boundaries**. What is proved is the **arithmetic and accounting core**
that executes once those lookups have chosen a region: 4 KiB alignment, the
allocate split, `used` accounting, and the coalescing offset math. Each proved
function is a standalone **mirror** whose body transcribes the corresponding
statements of `allocate()` / `deallocate()`; the line correspondence is recorded
below for the drift check. A green proof of a mirror covers the mirror, not the
shipped `BTreeMap`-backed function — see "Assumptions / trusted boundaries".

All 8 proofs were validated by **fault injection** (2026-08-20): a
contract-violating edit to each function was confirmed to turn its goal red
(`✘`), then reverted. No proof is vacuous.

## align_up  — mirrors `allocator.rs:42` (`size.next_multiple_of(ALIGNMENT)`)

- **[Postcondition]** The rounded size is a multiple of 4096 — every allocation
  size is 4 KiB-aligned. — spec **FR-004**, **SC-4** — proved: `align_up.coma` (15/15 VCs)
- **[Postcondition]** The rounded size is never smaller than the request
  (`result >= size`). — FR-004 — proved: `align_up.coma`
- **[Postcondition]** The rounding is minimal: `result < size + 4096`, so internal
  fragmentation per allocation is under one page (matches the "waste up to 4095
  bytes" note in the spec's Implementation Notes). — proved: `align_up.coma`
- **[Precondition]** `size > 0` and `size + 4095 <= usize::MAX` (overflow-free). — proved: `align_up.coma`

## alloc_admits  — mirrors `allocator.rs:39-41` (`if size == 0 { return None; }`)

- **[Postcondition]** A zero-size request is always rejected; any positive size is
  admitted to the search. — spec **FR-008** — proved: `alloc_admits.coma` (1/1 VC)

## allocate_split  — mirrors `allocator.rs:44-54` (split of the found free region)

Runs after first-fit has selected a free region `(offset, region_size)` with
`region_size >= aligned_size`.

- **[Postcondition]** `used` grows by exactly `aligned_size`. — accounting (spec
  User Story 5 / `capacity_and_used`) — proved: `allocate_split.coma` (1/1 VC)
- **[Postcondition]** The remaining free bytes in the region are
  `region_size - aligned_size` (no underflow, given the first-fit precondition). — proved: `allocate_split.coma`
- **[Postcondition]** The leftover free region starts at `offset + aligned_size`
  and stays within the pool (`leftover_offset + remaining <= capacity`); `used`
  never exceeds `capacity`. — spec **FR-010** (allocation stays within the pool) — proved: `allocate_split.coma`
- **[Precondition]** `aligned_size` is a positive multiple of 4096, the region is
  big enough (`region_size >= aligned_size`, the first-fit guarantee), the region
  lies within the pool, and the pool has room (`used + aligned_size <= capacity`). — proved: `allocate_split.coma`

## deallocate_used  — mirrors `allocator.rs:60-61` (`self.used -= aligned_size`)

- **[Postcondition]** `used` decreases by exactly `aligned_size`. — accounting — proved: `deallocate_used.coma` (1/1 VC)
- **[Precondition]** `used >= aligned_size` — a free never underflows the `used`
  counter. — proved: `deallocate_used.coma`

## coalesce_prev  — mirrors `allocator.rs:66-73` (coalesce with preceding region)

- **[Postcondition]** When the preceding free region is exactly adjacent
  (`prev_offset + prev_size == offset`) the two merge into
  `(prev_offset, prev_size + size)`; otherwise the freed region is unchanged. — spec **FR-026** — proved: `coalesce_prev.coma` (1/1 VC)
- **[Invariant]** Coalescing preserves the region's right endpoint
  (`new_offset + new_size == offset + size`) and only ever extends the region
  leftward (`new_offset <= offset`) — no bytes are gained or lost by the merge. — FR-026 — proved: `coalesce_prev.coma`
- **[Precondition]** The preceding region does not overlap the freed region
  (`prev_offset + prev_size <= offset`); the merged size is overflow-free. — proved: `coalesce_prev.coma`

## coalesce_next  — mirrors `allocator.rs:76-80` (coalesce with following region)

- **[Postcondition]** The region grows by exactly the following region's size
  (`result == new_size + next_size`), and coalescing never shrinks the region. — spec **FR-026** — proved: `coalesce_next.coma` (1/1 VC)
- **[Precondition]** The merged size is overflow-free. — proved: `coalesce_next.coma`

## lifecycle_alloc_free  (lifecycle) — composes allocate_split + deallocate_used

- **[Postcondition]** Allocating `aligned_size` bytes out of a region and then
  freeing the same amount restores `used` to its original value
  (`result == used`). This is the allocator's **accounting-conservation**
  invariant across an alloc→free cycle — the core property behind
  `remove_and_reuse` / `capacity_and_used`. — proved: `lifecycle_alloc_free.coma` (1/1 VC)

## leftover_offset_aligned  (lifecycle) — inductive step for offset alignment

The pool begins as one region at offset 0 (aligned; `allocator.rs:19`) and each
allocate carves an aligned chunk, so the leftover region starts at
`offset + aligned_size` (`allocator.rs:50`).

- **[Postcondition]** An aligned region start plus an aligned carve yields an
  aligned leftover start (`result % 4096 == 0`, `result >= offset`). Together with
  the offset-0 base case this establishes by induction that **every free-region
  start — and therefore every returned allocation offset — is 4 KiB-aligned**, so
  returned pointers are directly usable for NVMe DMA. — spec **FR-004**,
  **NFR-010**, **SC-4** — proved: `leftover_offset_aligned.coma` (1/1 VC)

## Assumptions / trusted boundaries

- **`BTreeMap` container operations are trusted.** The first-fit scan
  (`allocator.rs:44`), the preceding-region lookup (`:67`), and the
  following-region lookup (`:77`) are not modeled. The proofs assume these lookups
  behave as their preconditions state — e.g. that the first-fit scan returns a
  region with `region_size >= aligned_size`, that `range(..offset).next_back()`
  returns a region with `prev_offset + prev_size <= offset`, and that
  `get(&next_offset)` returns a region starting exactly at `new_offset + new_size`.
  Properties that depend on the *global* free-region set (no two live free regions
  overlap; every byte is either free or allocated exactly once; the free set stays
  sorted) are therefore **not** proved here.
- **These are mirrors, not the shipped functions.** `allocate()` / `deallocate()`
  cannot be built under Creusot (BTreeMap, `*mut u8`, the component framework). The
  mirror bodies transcribe the arithmetic statements byte-faithfully and each cites
  its source line, but a green proof covers the mirror. The residual gap between
  mirror and shipped code is the drift the line-correspondence citations above are
  meant to let a reviewer check by eye; there is no automated equality check
  binding the mirror to `allocator.rs`.
- **`next_multiple_of` is modeled, not called.** `align_up` implements
  `ceil(size/4096)*4096` and proves it equivalent in behavior to
  `size.next_multiple_of(4096)` over the `size > 0` domain; creusot-std provides no
  contract for the real `next_multiple_of`.
- **`align_up` alignment needed two SMT bridges** (`proof_assert!`): the
  ComputerDivision identity `n == q*4096 + r` with `r < 4096`, and `Div_mult`
  (`(q*4096)/4096 == q`) to discharge `result % 4096 == 0`. These are standard
  integer-division facts, asserted so alt-ergo/z3/cvc5 need not rediscover them.

## Attempted but not proven

- **Global free-list invariants** (non-overlap of free regions, full coverage of
  the pool, sortedness). These are properties of the whole `BTreeMap`, which is
  outside Creusot's modeling scope; proving them would require reimplementing the
  allocator over a Creusot-supported sequence type (e.g. `Vec<(usize,usize)>`) with
  loop invariants — a much larger effort that would still be a mirror, not the
  shipped `BTreeMap` code.
- **Concurrency properties** (SC-3: no deadlock/corruption under 16+ threads;
  FR-005/FR-006 `RwLock<Pool>` discipline). Creusot reasons about sequential
  function bodies, not lock protocols; these remain covered by tests/architecture,
  as the spec's "Verified" column already records.

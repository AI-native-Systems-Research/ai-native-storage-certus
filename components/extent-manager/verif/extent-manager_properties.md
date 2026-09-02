# Verified properties — extent-manager (Creusot)

Proven from spec `specs/001-extent-manager-v2/spec.md` against code in
`components/extent-manager/src/*.rs`. Artifacts: this `verif/` crate.

**Method.** Each function below is a *faithful whole-function mirror* of a
shipped function: the extent-manager crate cannot be built under Creusot (it
depends on the `interfaces` crate, `parking_lot`, `std::collections::{HashMap,
BTreeMap}`, SPDK/DMA FFI and async), so the mirror reproduces the source body
and attaches the spec-derived contract (`#[requires]`/`#[ensures]`). Every
mirror cites the exact source line range it tracks.

**Honesty caveat.** A green proof here covers the **mirror**, not the shipped
function. The residual gap is exactly the body-equality obligation between
mirror and source, which is discharged **by inspection** against the cited line
ranges (see each mirror's doc comment). There are **no `#[trusted]` boundaries
inside this crate** — every VC is discharged by the prover portfolio.

**Result.** `cargo creusot` → `Proved (13 files) ✔`.
**42 VCs discharged across 13 `.coma` files**, portfolio alt-ergo / z3 / cvc5 /
cvc4, tactics `split_vc` + `compute_specified`.

Build (creusot-std is not vendored on this branch; `Cargo.toml` patches it to
the toolchain checkout that ships it — a read-only path reference):

```
cd components/extent-manager/verif && cargo creusot
```

---

## Sector alignment — `align_to_sector_size` (src/region.rs:37-39)

Spec: **FR-005** (`reserve_extent` returns a *sector-aligned* slot), **FR-002**
(`sector_size > 0`).

- **[Precondition]** `sector_size > 0`, and `size + sector_size` does not
  overflow `u32` (a latent requirement the source shares). — spec FR-002
- **[Postcondition]** The result is a multiple of `sector_size`
  (`result % sector_size == 0`). — spec FR-005 — proved
- **[Postcondition]** The result covers the request (`result >= size`) and is
  minimal — at most one sector of slack (`result < size + sector_size`).
  Together these pin the result to *the* sector-aligned size for `size`. —
  spec FR-005
- Evidence: `align/align_to_sector_size.coma` — **8/8 VCs**. Includes the
  nonlinear `q*sector_size` overflow and divisibility goals, discharged with
  the division-identity + remainder-bound `proof_assert!` bridges (no trusted
  lemma needed once the addition-overflow precondition is stated).

## Slab geometry — slot ⇄ byte-offset (src/slab.rs:58-88)

Spec: **FR-005** (offset handed to WriteHandle), **FR-008** (`remove_extent(offset)`
locates the owning slot), **FR-012** (offset addressing).

- **[Postcondition]** `slot_offset(start, elem, i) == start + i*elem` and
  `>= start`. — FR-005 — proved: `slab_geom/slot_offset.coma` (**3/3 VCs**)
- **[Postcondition]** `slot_for_offset` only ever returns a slot index that is
  in range (`i < num_slots`) and round-trips through `slot_offset`
  (`byte_offset == start + i*elem`); anything below `start` returns `None`; and
  any in-range, element-aligned offset maps to its exact slot. — FR-008, FR-012
  — proved: `slab_geom/slot_for_offset.coma` (**1/1 VC**)
- **[Postcondition]** `contains_offset(start, slab_size, off)` is exactly
  `start <= off < start + slab_size`. — FR-008 — proved:
  `slab_geom/contains_offset.coma` (**1/1 VC**)
- **[Lifecycle / round-trip]** For a valid slot `i` (`i < num_slots`,
  `elem > 0`), the disk offset produced by `slot_offset` maps back through
  `slot_for_offset` to exactly `Some(i)`. This is the property that makes
  `remove_extent(offset)` locate the correct slot. — FR-008, FR-012 — proved:
  `slab_geom/offset_roundtrip.coma` (**8/8 VCs**)

## Bitmap allocator liveness — `AllocationBitmap` (src/bitmap.rs:1-64)

Spec: **FR-020** (each slab uses a bitmap allocator). `allocated_count` is the
liveness counter behind `Slab::is_empty` (slab.rs:50-52) and hence the
deferred-free "return an emptied slab to the buddy allocator" path (**FR-008**).

- **[Invariant]** Well-formedness `wf`: the word vector holds exactly
  `ceil(num_slots / 64)` words, established by `new` and preserved by
  `set`/`clear`. — proved: `new.coma` (**2/2**), carried through `set`/`clear`.
- **[Postcondition + memory-safety]** `set(idx)` increments `allocated_count`
  by 1 and the derived word index `idx/64` is provably in bounds of `words`
  (the bit access cannot go out of range); `num_slots` and `wf` are preserved.
  — FR-020 — proved: `set.coma` (**10/10 VCs**)
- **[Postcondition + memory-safety]** `clear(idx)` decrements `allocated_count`
  by 1, with the same in-bounds and `wf` guarantees. — FR-020 — proved:
  `clear.coma` (**2/2 VCs**)
- **[Postcondition]** `is_all_free() == (allocated_count == 0)`;
  `count_set() == allocated_count`; `num_slots()` returns the field. — FR-020 —
  proved: `is_all_free.coma`, `count_set.coma`, `num_slots.coma` (**1/1** each)
- **[Lifecycle]** A fresh bitmap reports all-free; after one `set` the count is
  1. — FR-020 — proved: `lifecycle_new_set.coma` (**3/3 VCs**)

Precondition on `set`: `idx < num_slots` (source `debug_assert!`) and
`allocated_count < num_slots` (at most `num_slots` slots can be live; keeps the
counter from overflowing). Precondition on `clear`: `allocated_count > 0`.

## Region sharding — `region_for_key` (src/lib.rs:219)

Spec: **FR-022** ("Keys MUST be sharded by `key & (region_count - 1)`"),
**FR-002** (`region_count` is a positive power of two, validated at format,
lib.rs:397).

- **[Precondition]** `region_count != 0` and `region_count & (region_count-1) == 0`
  (i.e. a positive power of two — the exact bit pattern `format()` enforces via
  `is_power_of_two()`). — FR-002
- **[Postcondition]** The shard index is always in range: `result < region_count`,
  so the subsequent `regions[idx]` access (lib.rs:220) is in bounds. — FR-022 —
  proved: `shard/region_for_key.coma` (**1/1 VC**, `#[bitwise_proof]` mode for
  the bit-vector mask reasoning).

---

## Assumptions / trusted boundaries

- **Mirror-vs-source body equality (per function).** The proofs run against
  mirrors, not the shipped functions. Equality is asserted by inspection against
  the cited source line ranges. `align_to_sector_size` is the only mirror whose
  text differs from the source: the single expression
  `(size + sector_size - 1) / sector_size * sector_size` is split into named
  `let` steps (`n`, `q`) so intermediate facts can be named — semantically
  identical, no logic change.
- **`creusot-std`** (the `creusot-contracts` runtime, `Vec`/`Int`/`Option`
  models, `#[bitwise_proof]`) is trusted, as in every Creusot proof.
- **Prover portfolio** alt-ergo / z3 / cvc5 / cvc4 is trusted (standard for
  SMT-backed verification).
- **Overflow preconditions are assumptions, not proofs.** `align_to_sector_size`
  requires `size + sector_size <= u32::MAX`; `slot_offset` requires
  `start + i*elem <= u64::MAX`. These bound inputs the shipped code also assumes
  implicitly; they are not established here.
- **No `#[trusted]` items exist inside this crate.**

## Not proved by Creusot (and why)

- **Bit-level `set`/`is_set` round-trip** (bitmap.rs:17-40). We proved the
  `allocated_count` liveness invariant and the memory-safety of the word index,
  but **not** the bit-vector property "`is_set(idx)` is true immediately after
  `set(idx)` and false after `clear(idx)`". That needs symbolic bit-vector
  reasoning over `words[idx/64] & (1 << idx%64)` across a `Vec<u64>`; tractable
  in principle with `#[bitwise_proof]` but not attempted here to stay within the
  high-value budget.
- **Buddy allocator order math** — `alloc`/`free`/`mark_allocated` (buddy.rs:57-157).
  The order computation is `63 - blocks.leading_zeros()` / `64 - (b-1).leading_zeros()`,
  i.e. a `log2`. Creusot models `leading_zeros` only relationally, so proving
  "the chosen order yields `2^order >= blocks_needed`" requires `leading_zeros ↔
  log2` reasoning the SMT backends do not discharge. Compounded by the
  `Vec<Vec<u64>>` free-list buddy-merge mutation. Documented as intractable at
  this budget.
- **Checkpoint / recovery / superblock** (checkpoint.rs, recovery.rs,
  superblock.rs) — byte-exact serialization, CRC32, dual-copy fallback (FR-013,
  FR-017, FR-018, SC-002/003). These are I/O- and layout-bound (block-device
  reads/writes, `&[u8]` (de)serialization); out of Creusot's arithmetic/protocol
  sweet spot. Candidates for a future CRC/coverage arithmetic proof only.
- **Region orchestration** — `alloc_extent`, `free_slot`,
  `remove_extent_by_offset`, `flush_pending_frees` (region.rs:41-160). These
  coordinate `BTreeMap<u64, Slab>` + `SizeClassManager` (HashMap) + buddy +
  `pending_frees`. The *deferred-free* crash-safety property (FR-025: a removed
  slot is not reused until after a checkpoint) is a multi-object temporal
  invariant over container state — beyond a single-function contract; would need
  a modelled-container lifecycle proof.
- **Concurrency / coalescing** (FR-015, FR-023, SC-006) — `parking_lot::RwLock`
  sharding and single-writer checkpoint coalescing are concurrency properties,
  outside Creusot's sequential scope.

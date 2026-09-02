# Verified properties — extent-manager (Creusot)

Proven from spec `specs/001-extent-manager-v2/spec.md` against code in
`components/extent-manager/src/*.rs`. Artifacts: this `verif/` crate.

**Re-run under the coverage-discipline policy.** This run attempts *every
property expressible as a contract*, including two the prior run had punted on
budget/tractability grounds:

- **bitmap bit-level round-trip** (`set`/`clear`/`is_set` over `Vec<u64>`) — now
  **proved** with `#[bitwise_proof]` (prior run: "not attempted, to stay within
  budget");
- **buddy allocator order math** (`ceil(log2)` sizing) — now **proved** with the
  `pow2` bit-vector bridge (prior run: "documented as intractable").

**Method.** Each function below is a *faithful whole-function mirror* of a
shipped function: the extent-manager crate cannot be built under Creusot (it
depends on the `interfaces` crate, `parking_lot`, `std::collections::{HashMap,
BTreeMap}`, SPDK/DMA FFI and async), so the mirror reproduces the source body
and attaches the spec-derived contract (`#[requires]`/`#[ensures]`). Every
mirror cites the exact source line range it tracks.

**Honesty caveat.** A green proof here covers the **mirror**, not the shipped
function. The residual gap is the body-equality obligation between mirror and
source, discharged **by inspection** against the cited line ranges. There are
**no `#[trusted]` items inside this crate** — every VC is discharged by the
prover portfolio.

**Result.** `cargo creusot` → `Proved (17 files) ✔`.
**72 VCs discharged across 17 `.coma` files** (prior run: 42 VCs / 13 files),
portfolio alt-ergo / z3 / cvc5 / cvc4, tactics `split_vc` + `compute_specified`.
Every newly proved function was validated anti-vacuously by fault injection (a
contract-violating edit turns a VC red; reverted).

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
  overflow `u32`. — spec FR-002
- **[Postcondition]** The result is a multiple of `sector_size`
  (`result % sector_size == 0`). — spec FR-005 — proved:
  `align/align_to_sector_size.coma` (**8/8 VCs**)
- **[Postcondition]** The result covers the request (`result >= size`) and is
  minimal — at most one sector of slack (`result < size + sector_size`). —
  spec FR-005 — same file.

## Slab geometry — slot ⇄ byte-offset (src/slab.rs:58-88)

Spec: **FR-005** (offset handed to WriteHandle), **FR-008** (`remove_extent(offset)`
locates the owning slot), **FR-012** (offset addressing).

- **[Postcondition]** `slot_offset(start, elem, i) == start + i*elem` and
  `>= start`. — FR-005 — proved: `slab_geom/slot_offset.coma` (**3/3 VCs**)
- **[Postcondition]** `slot_for_offset` returns only an in-range slot index
  (`i < num_slots`) that round-trips through `slot_offset`; anything below
  `start` returns `None`; any in-range, element-aligned offset maps to its exact
  slot. — FR-008, FR-012 — proved: `slab_geom/slot_for_offset.coma` (**1/1 VC**)
- **[Postcondition]** `contains_offset(start, slab_size, off)` is exactly
  `start <= off < start + slab_size`. — FR-008 — proved:
  `slab_geom/contains_offset.coma` (**1/1 VC**)
- **[Lifecycle / round-trip]** For a valid slot `i` (`i < num_slots`, `elem > 0`),
  the disk offset from `slot_offset` maps back through `slot_for_offset` to
  exactly `Some(i)` — the property that makes `remove_extent(offset)` locate the
  correct slot. — FR-008, FR-012 — proved: `slab_geom/offset_roundtrip.coma`
  (**8/8 VCs**)

## Bitmap allocator — `AllocationBitmap` (src/bitmap.rs:1-64)

Spec: **FR-020** (each slab uses a bitmap allocator to pack same-size extents).
Two families of property.

### Liveness counter (`allocated_count`) — memory-safety + count

- **[Invariant]** Well-formedness `wf`: the word vector holds exactly
  `ceil(num_slots / 64)` words, established by `new` and preserved by
  `set`/`clear`. — proved: `new.coma` (**2/2**), carried through `set`/`clear`.
- **[Postcondition + memory-safety]** `set(idx)` increments `allocated_count`
  by 1; the derived word index `idx/64` is provably in bounds of `words`;
  `num_slots` and `wf` preserved. — FR-020 — proved: `set.coma` (**11/11 VCs**)
- **[Postcondition + memory-safety]** `clear(idx)` decrements `allocated_count`
  by 1, with the same in-bounds and `wf` guarantees. — FR-020 — proved:
  `clear.coma` (**8/8 VCs**)
- **[Postcondition]** `is_all_free() == (allocated_count == 0)`;
  `count_set() == allocated_count`; `num_slots()` returns the field. — FR-020 —
  proved: `is_all_free.coma`, `count_set.coma`, `num_slots.coma` (**1/1** each)
- **[Lifecycle]** A fresh bitmap reports all-free; after one `set` the count is
  1. — FR-020 — proved: `lifecycle_new_set.coma` (**3/3 VCs**)

### Bit-level round-trip (NEW — coverage-discipline priority)

The shared predicate `slot_bit(bm, idx) := bm.words[idx/64].nth_bit(idx%64)`
reads exactly the bit the source `is_set` reads (`(words[idx/64] >> idx%64) & 1`).
All three functions carry `#[bitwise_proof]`, so the SMT portfolio reasons over
`&`/`|`/`<<`/`>>` on the `Vec<u64>` words as genuine bit vectors (via the
builtin `nth_bit` / `UInt64$BW$.nth`).

- **[Postcondition]** After `set(idx)`, the slot's bit is set:
  `slot_bit(&^self, idx@)`. — FR-020 — proved inside `set.coma`.
- **[Postcondition]** After `clear(idx)`, the slot's bit is clear:
  `!slot_bit(&^self, idx@)`. — FR-020 — proved inside `clear.coma`.
- **[Postcondition]** `is_set(idx) == slot_bit(self, idx@)` — the read is
  exactly the bit. — FR-020 — proved: `is_set.coma` (**7/7 VCs**)
- **[Round-trip]** `is_set(idx)` is **true** immediately after `set(idx)`. —
  FR-020 — proved: `roundtrip_set_then_is_set.coma` (**5/5 VCs**)
- **[Round-trip]** `is_set(idx)` is **false** immediately after `clear(idx)`. —
  FR-020 — proved: `roundtrip_clear_then_is_set.coma` (**3/3 VCs**)

  Anti-vacuity: flipping `set` to clear the bit reddens `vc_set`; negating the
  `is_set` read reddens `vc_is_set`; flipping the round-trip's expected result
  reddens `vc_roundtrip_set_then_is_set`.

## Buddy allocator order math — `order_for_blocks` (src/buddy.rs:59-63, 87-91, 120-124)

Spec: **FR-019** (each region uses a buddy allocator for coarse-grained
allocation). `alloc`, `free` and `mark_allocated` all turn a size into a buddy
*order* via the same computation
`order = if blocks <= 1 { 0 } else { 64 - (blocks-1).leading_zeros() }`, i.e.
`ceil(log2(blocks))`. This mirror proves the *sizing correctness* of that shared
order computation with `#[bitwise_proof]`.

- **[Postcondition — coverage]** The chosen order is large enough for the
  request: `2^result >= blocks`. — FR-019 — proved: `buddy/order_for_blocks.coma`
  (**8/8 VCs**)
- **[Postcondition — minimality]** It is the *smallest* such order:
  `result == 0 || 2^(result-1) < blocks`. Together with coverage this pins
  `result` to exactly `ceil(log2(blocks))`. — FR-019 — same file.

  The `leading_zeros → 2^k` step (the exact gap the prior run cited as
  intractable) is bridged by the `Int::pow2` builtin (`bv.Pow2int.pow2`): from
  the relational `leading_zeros` model `x >> (63 - lz) == 1`, two
  `proof_assert!` bounds `2^(63-lz) <= x < 2^(64-lz)` let the portfolio derive
  both postconditions. Anti-vacuity: shrinking the order to `63 - lz` reddens
  the coverage VC.

---

## Assumptions / trusted boundaries

- **Mirror-vs-source body equality (per function).** The proofs run against
  mirrors, not the shipped functions; equality is asserted by inspection against
  the cited source line ranges. `align_to_sector_size` is the only mirror whose
  text differs from the source (the single expression is split into named `let`
  steps so intermediate facts can be named — semantically identical).
- **`creusot-std`** (the `creusot-contracts` runtime, `Vec`/`Int`/`Seq` models,
  `#[bitwise_proof]`, `nth_bit`, `pow2`) is trusted, as in every Creusot proof.
- **Prover portfolio** alt-ergo / z3 / cvc5 / cvc4 is trusted (standard for
  SMT-backed verification).
- **Overflow / range preconditions are assumptions, not proofs.**
  `align_to_sector_size` requires `size + sector_size <= u32::MAX`; `slot_offset`
  requires `start + i*elem <= u64::MAX`; `set` requires
  `allocated_count < num_slots` (no counter overflow). These bound inputs the
  shipped code also assumes implicitly.
- **No `#[trusted]` items exist inside this crate.**

## Not proved — attempted, tool boundary hit

- **Buddy free-list *merge* conservation** — `free` / `mark_allocated`
  (buddy.rs:84-157). The claim is that the buddy-merge loop conserves total free
  bytes (`total_free` invariant) while coalescing the `Vec<Vec<u64>>` free lists.
  **Attempted** as a faithful mirror; blocked at the container-API level:
  `free` uses `self.free_lists[order].iter().position(|&o| o == buddy_offset)`
  followed by `.swap_remove(pos)`, and **neither `Iterator::position` nor
  `Vec::swap_remove` has a `creusot-contracts` extern spec** (only `Vec::push`
  is specified in `creusot-std/src/std/vec.rs`). Without a logical model of the
  removal's effect on the `Seq`, no postcondition about the mutated free list is
  discharged. Compounded by the multi-object sum-over-nested-`Vec` invariant the
  conservation claim needs. Route: rewrite the merge with only specified ops
  (`Seq::remove` model + an index loop) as an explicitly *non-faithful* mirror,
  or add extern specs for `swap_remove`/`position` upstream, then re-attempt.
  (The *order arithmetic* those methods sit on top of **is** proved above — see
  `order_for_blocks`.)

## Not proved — not expressible in Creusot (shape)

- **Checkpoint / recovery / superblock byte layout** (checkpoint.rs, recovery.rs,
  superblock.rs) — byte-exact serialization, CRC32, dual-copy fallback
  (FR-013/017/018, SC-002/003). Depends on effects Creusot cannot model: real
  block-device I/O and `&[u8]` (de)serialization layout. Route: fault-injection /
  round-trip serialization tests; a standalone CRC arithmetic proof is the only
  Creusot-shaped fragment.
- **Deferred-free crash safety** (FR-025) — "a removed slot is not reused until
  after a checkpoint" is a multi-object *temporal* invariant over
  `BTreeMap<u64, Slab>` + `SizeClassManager` + `pending_frees`, not a
  single-function contract. Route: a modelled-container lifecycle proof, or a
  Loom/property test over the removal→checkpoint→alloc sequence.
- **Concurrency / coalescing** (FR-015, FR-023, SC-006) — `parking_lot::RwLock`
  sharding and single-writer checkpoint coalescing are concurrency/interleaving
  properties, outside Creusot's sequential logic. Route: Loom.

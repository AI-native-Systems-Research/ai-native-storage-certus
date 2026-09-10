# Verified properties — extent-manager (Kani, clean-slate re-run 2026-09-10)

Verified from spec `specs/001-extent-manager-v2/spec.md` against code
`src/{bitmap,buddy,slab,region,superblock}.rs`, keyed to the tool-independent property inventory
`verif/extent-manager_property_inventory.md` (N=14 public methods, M=47 properties, 6 named globals).
Harnesses live in `#[cfg(kani)] mod verification` blocks in each source file.
Toolchain: Kani 0.67.0 / CBMC 6.8.0. Run with
`cargo kani -Z unstable-options --output-format=terse --harness-timeout 90s -j 24 --default-unwind 3`.

**Result: 19 harnesses — 17 SUCCESSFUL, 2 tool-boundary walls (⊘, CBMC timed out, captured on purpose).**

Each harness follows `kani::assume(<precondition mirroring the production guard>)` → **call the real
function** → `assert!(<postcondition>)`. Kani proves each property over the **entire input domain within
the stated `#[kani::unwind(N)]` bound**, symbolically. Structures are exercised at deliberately small,
bounded geometries (noted per section) so the SAT problem stays tractable; the proof is exhaustive over
all inputs *at that geometry*. **Every one of the 17 SUCCESSFUL harnesses is anti-vacuity-anchored** — a
fault injected into the code it exercises flips it to FAILED (matrix at bottom).

This is a clean-slate re-run: the baseline suite (`b4ace468`, 14 harnesses) was stripped and re-authored
from the inventory by id. New vs baseline: `verify_total_free_le_usable` (EM-USED-LE-CAP),
`verify_slot_for_offset_rejects_invalid` (remove-routing reject half), `verify_align_to_sector_size`
(EM-SECTOR-ALIGN global — baseline had **no** region harness), and the two wall probes below (baseline
only *documented* region.rs/lib.rs as unverifiable; this run **executes** them to capture the signature).

## Global-invariant coverage (inventory ledger, by attachment count)

| rank | global id | attachments | Kani verdict |
|---|---|---|---|
| 1 | EM-INIT-GATE | 8 | ⊘ construction wall (ExtentManager driver — HashMap/thread/FFI); see `wall_data_base_lba_roundtrip` |
| 2 | EM-KEYVEC-MEMBERSHIP | 4 | **partial ✓** — slab-level membership (set/get/free_slot, FREE_KEY sentinel) proved in slab.rs + bitmap.rs; the region-level BTreeMap composition is ⊘ (`wall_region_alloc_extent`) |
| 3 | EM-SECTOR-ALIGN | 3 | **✓** proved (`verify_align_to_sector_size`, symbolic size; + slab `slot_offset` math) |
| 4 | EM-DEFERRED-FREE | 3 | ⊘ region BTreeMap composition wall (`wall_region_alloc_extent`); buddy merge core proved (`verify_alloc_free_round_trip_1block`) |
| 5 | EM-DATABASE-ROUNDTRIP | 2 | ⊘ construction wall (`wall_data_base_lba_roundtrip`) |
| 6 | EM-USED-LE-CAP | 2 | **✓** proved at the buddy level (`verify_total_free_le_usable`, symbolic alloc) |

## Allocation bitmap (`src/bitmap.rs` — `AllocationBitmap`)

Bounded scope: `num_slots = 8` (one sub-`u64` word, exercises the `< 64` packing path), `#[kani::unwind(9)]`.
Implements EM-KEYVEC-MEMBERSHIP (slot allocated iff bit set) and EM-RESERVE-SIZECLASS (rover free search).

- **[Postcondition]** `set` on a free slot marks exactly that slot and increments the count by one. —
  `verify_set_marks_slot` (0 of 434).
- **[Invariant]** set-then-clear returns the bitmap to empty (slot free, count 0, `is_all_free`). —
  `verify_set_clear_round_trip` (0 of 472).
- **[Invariant]** setting one slot never marks a distinct slot; slots are independent. —
  `verify_set_independent` (0 of 433).
- **[Postcondition]** on an all-free bitmap, `find_free_from(start)` returns a valid, in-bounds, free slot
  for every `start`. — `verify_find_free_from_valid` (0 of 398).
- **[Postcondition]** on a full bitmap, `find_free_from` returns `None` for every `start`, count ==
  num_slots. — `verify_find_free_from_full` (0 of 450).

## Buddy allocator (`src/buddy.rs` — `BuddyAllocator`)

Bounded scope: `sector_size = 1`, `total_usable = 2` blocks (`max_order = 1`), `#[kani::unwind(6)]`.
Implements EM-RESERVE-SIZECLASS, EM-USED-LE-CAP, and the offset-range guarantee. Every harness calls the
real `new`/`alloc`/`free`/`total_free`, so Kani also checks every internal shift/subtract/add/cast.

- **[Postcondition]** `new` publishes the whole usable range as free (`total_free == total_usable_size ==
  total`). — `verify_new_total_free` (0 of 549).
- **[Invariant · EM-USED-LE-CAP]** `total_free() <= total_usable_size()` holds at construction and after
  an arbitrary (symbolic) `alloc`. Capacity is concrete because a symbolic `total` makes
  `max_order = 63 - leading_zeros(total)` a symbolic loop bound on the free-list build; the invariant is
  instead exercised over a symbolic allocation. — `verify_total_free_le_usable` (0 of 696).
- **[Postcondition]** any successful `alloc(size)` returns an offset in `[base_offset, base_offset+total)`,
  for symbolic `base_offset` (≤ 2^40) and symbolic `size` in `[1, total]`. — `verify_alloc_offset_in_range`
  (0 of 614).
- **[Invariant]** a successful `alloc(size)` never increases free space (symbolic `size`). —
  `verify_alloc_shrinks_free` (0 of 690).
- **[Invariant]** allocating one block then freeing it restores the original free space (buddy merge
  round-trip; strict shrink then exact restore). — `verify_alloc_free_round_trip_1block` (0 of 851).

## Slab (`src/slab.rs` — `Slab`)

Bounded scope: `start_offset = 8192`, `element_size = 4096`, `slab_size = 16384` ⇒ 4 slots,
`#[kani::unwind(6)]`. Implements EM-KEYVEC-MEMBERSHIP, EM-SECTOR-ALIGN (slot offset math),
EM-REMOVE-OFFSET-ROUTING (slot_for_offset/contains_offset), EM-RESERVE-INVISIBLE-UNTIL-PUBLISH.

- **[Postcondition]** a fresh slab has every key == `FREE_KEY` (nothing enumerable until published). —
  `verify_new_keys_all_free` (0 of 382).
- **[Invariant]** `slot_offset` and `slot_for_offset` are exact inverses over the slot domain, and
  `slot_offset(idx) = start + idx*element_size` lies in the slab range — this is what lets
  `remove_extent(offset)` recover the slot. — `verify_slot_offset_roundtrip` (0 of 462).
- **[Precondition→reject]** `slot_for_offset` REJECTS any below-start / misaligned / past-end byte offset,
  over a fully symbolic offset (the OffsetNotFound reject half of remove-routing). —
  `verify_slot_for_offset_rejects_invalid` (0 of 429).
- **[Postcondition]** `alloc_slot` yields an in-bounds slot whose offset matches `slot_offset(idx)`, lies
  in the slab range, and whose key is still `FREE_KEY` (reserved, not yet published). —
  `verify_alloc_slot_offset_valid` (0 of 488).
- **[Invariant]** publishing a non-sentinel key then reading it back is consistent, and `free_slot` resets
  the key to `FREE_KEY`. — `verify_set_key_then_free` (0 of 529).

## Region (`src/region.rs` — `RegionState`)

- **[Invariant · EM-SECTOR-ALIGN, global rank 3]** `align_to_sector_size(size, ss)` rounds up to a whole
  number of sectors: result is `ss`-aligned, ≥ size, within one sector of size, identity when already
  aligned. Proved over a **symbolic size** at the production sector size (4096, a fixed power-of-two
  divisor — a symbolic divisor makes `/ss*ss` a SAT blow-up, not the property under test). Factored to an
  associated fn so it verifies without RegionState construction. — `verify_align_to_sector_size` (0 of 16).

## Superblock (`src/superblock.rs` — `Superblock`)

Bounded scope: `#[kani::unwind(2)]`.

- **[Precondition→Error · truncation guard]** `deserialize()` rejects a buffer shorter than
  `SUPERBLOCK_SIZE` rather than reading past the end. The test buffer carries a **valid magic** so the
  reject is attributable to the length guard alone (an all-zero short buffer would also be caught by the
  magic check — the baseline harness was anti-vacuity-blind to the guard it named). —
  `verify_deserialize_rejects_short_buffer` (0 of 3095).

## Tool-boundary walls (⊘ — executed to capture the signature, not proved)

Both were run at `--harness-timeout 90s`; both return `VERIFICATION:- FAILED / CBMC timed out` — the
reproducible ⊘ signature. Route: **Creusot** (logic-level models over the maps / abstract state).

- **`region::wall_region_alloc_extent`** (`#[kani::unwind(4)]`) — `alloc_extent` composes
  `BTreeMap<u64,Slab>::insert`, the `HashMap`-backed `SizeClassManager`, and `buddy.alloc`. CBMC stalls
  unwinding std BTreeMap/HashMap navigation (the same container-modeling wall memory-tier hit). Covers the
  region-level halves of EM-KEYVEC-MEMBERSHIP / EM-DEFERRED-FREE.
- **`lib::wall_data_base_lba_roundtrip`** (`#[kani::unwind(3)]`) — constructs `ExtentManager::new_default()`,
  whose `InterfaceMap = HashMap<TypeId, Box<dyn Any>>` drags in `getrandom`/SipHash `RandomState` seeding
  (foreign function) plus thread/atomic constructs. Construction wall gating EM-INIT-GATE and
  EM-DATABASE-ROUNDTRIP.

## Assumptions / bounds (the assume audit)

- `assume(idx < num_slots)` (bitmap) mirrors the `debug_assert!((idx as u32) < self.num_slots)` guard.
- `assume(size in [1, total])` (buddy) bounds the request to capacity; the out-of-capacity `None` path is
  allowed by the `if let Some(..)` shape, not asserted away.
- `assume(key != FREE_KEY)` (slab) mirrors FR-011: `FREE_KEY = u64::MAX` is the reserved sentinel.
- `assume(size <= 1<<28)` (align) keeps the `size + ss - 1` sum within u32 so the ALIGNMENT property is
  tested, not the (unguarded) production overflow.
- **Bounded geometries** (bitmap 8 slots; buddy 2 blocks; slab 4 slots): exhaustive over all inputs *at
  these sizes*. Production-scale (millions of extents) is an inductive argument — Creusot territory.
- **Concrete divisors** (buddy capacity, align sector size): a *symbolic* divisor/capacity turns integer
  division / `leading_zeros`-derived loop bounds into SAT blow-ups; fixing the divisor keeps the
  *interesting* operand (size / allocation) symbolic. This is a tractability choice, not a soundness one.

## Not verified by Kani (and why) — beyond the two executed walls

- **Superblock CRC / `serialize` / full round-trip** — `crc32fast::hash` runtime SIMD detection lowers to
  `_xgetbv` (`unsupported_construct`). Blocks SC-002/FR-003 serialize→deserialize.
- **Invalid-magic rejection** — the error path builds `format!("...{magic:#x}")` over the symbolic magic;
  symbolic hex-string formatting blows CBMC up. US3-AS3 documented, not proven; the sibling
  truncation-reject path IS proven above.
- **`buddy::free` / merge over a symbolic request, and `buddy::mark_allocated`** — linear
  `iter().position` search + XOR buddy math over a symbolic offset → SAT blow-up. Concrete single-block
  round-trip is proved instead; general merge correctness needs induction (Creusot).
- **`region.rs` composition, `checkpoint.rs`, `recovery.rs`, `lib.rs` driver** — BTreeMap/HashMap
  containers, I/O, variable-length key-vector serialization, `parking_lot::RwLock`, background threads,
  `Condvar` checkpoint coalescing, SPDK/DMA FFI. Concurrency (US4/FR-023/024, SC-004), durability/recovery
  (US2/US3, FR-013–018), and the two-phase-commit / deferred-free lifecycle (US1/US6) are temporal /
  round-trip properties for Creusot + integration tests.

## Anti-vacuity validation (mutation matrix — 13 mutations, all 17 SUCCESSFUL harnesses anchored)

Each mutation was applied to production code, the target harness(es) re-run (expecting FAILED), then the
tree reverted via `git checkout`. Every SUCCESSFUL harness flips to FAILED under at least one fault.

| # | Mutation (production code) | Harness(es) → verdict |
|---|---|---|
| A | `bitmap::set`: `\|= 1u64<<bit` → `\|= 0u64<<bit` (no-op) | set_marks_slot, set_clear_round_trip, set_independent, find_free_from_full → **FAILED** |
| B | `bitmap::find_free_from`: `if !is_set` → `if is_set` | find_free_from_valid → **FAILED** |
| C | `buddy::new`: double-push each block into the free list | new_total_free, total_free_le_usable → **FAILED** |
| D | `buddy::alloc`: return offset `+ total_usable_size` (out of range) | alloc_offset_in_range → **FAILED** |
| E | `buddy::alloc`: push the popped block back twice (inflate free) | alloc_shrinks_free, alloc_free_round_trip_1block → **FAILED** |
| F | `buddy::free`: drop the final `push(current_offset)` (no restore) | alloc_free_round_trip_1block → **FAILED** |
| G | `slab::new`: `keys: vec![FREE_KEY;..]` → `vec![0u64;..]` | new_keys_all_free → **FAILED** |
| H | `slab::slot_offset`: drop the `* element_size` factor | slot_offset_roundtrip → **FAILED** |
| I | `slab::slot_for_offset`: disable the misalignment reject | slot_for_offset_rejects_invalid → **FAILED** |
| J | `slab::alloc_slot`: return offset `+ slab_size` | alloc_slot_offset_valid → **FAILED** |
| K | `slab::free_slot`: set key to `0u64` instead of `FREE_KEY` | set_key_then_free → **FAILED** |
| L | `region::align_to_sector_size`: body → `size` (identity, no rounding) | align_to_sector_size → **FAILED** |
| M | `superblock::deserialize`: `if buf.len() < SUPERBLOCK_SIZE` → `if false` | deserialize_rejects_short_buffer → **FAILED** |

**Disclosure (one-sided invariants).** `verify_alloc_shrinks_free` and the post-alloc half of
`verify_total_free_le_usable` are `≤`-shaped: a *deflating* fault leaves `≤` true, so only an *inflating*
fault (E, C) kills them — the same fixpoint/monotonic caveat noted in the memory-tier run. Both are
anchored here by inflating mutations; the strict-shrink direction is additionally pinned by
`verify_alloc_free_round_trip_1block`'s `total_free() < before`.

## Environment note

`cargo kani` builds the whole crate, which pulls `interfaces/spdk` → `spdk-sys`, whose build script needs
a prebuilt SPDK tree. This worktree satisfies it with gitignored symlinks `deps/spdk -> /opt/spdk` and
`deps/spdk-build -> /opt/spdk-build`. No SPDK/FFI code is reached by any SUCCESSFUL harness; the symlinks
only let the crate compile under Kani. **No modification to the shared `components/interfaces/` crate.**

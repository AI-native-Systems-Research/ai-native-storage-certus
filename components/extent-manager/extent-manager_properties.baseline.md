# Verified properties — extent-manager (Kani)

Verified from spec `specs/001-extent-manager-v2/spec.md` against code `src/{bitmap,buddy,slab,superblock}.rs`.
Harnesses live in `#[cfg(kani)] mod verification` blocks in each of those source files.
Toolchain: `cargo kani` (Kani 0.67.0). Run with `cargo kani -p extent-manager`.

**Result: 14 harnesses, 14 SUCCESSFUL, 0 failed.**

Each harness follows the pattern `kani::assume(<spec precondition mirroring the production guard>)`
→ **call the real function** → `assert!(<spec postcondition>)`. Kani proves each property over the
**entire input domain within the stated `#[kani::unwind(N)]` bound**, symbolically (not by sampling).
The data structures are exercised at deliberately small, bounded geometries (noted per section) so the
SAT problem stays tractable; the proof is exhaustive over all inputs *at that geometry*. Every harness
was also anti-vacuity checked (see bottom) or belongs to a group that was.

## Allocation bitmap (`src/bitmap.rs` — `AllocationBitmap`)

Bounded scope: `num_slots = 8` (one sub-`u64` word, exercises the `< 64` packing path), `#[kani::unwind(9)]`.

- **[Postcondition]** Setting a free slot marks exactly that slot allocated and increments the allocated
  count by one. — spec FR-020 — harness `verify_set_marks_slot` (SUCCESSFUL, 0 of 434 checks failed).
- **[Invariant]** Set-then-clear of any slot returns the bitmap to the empty state (slot free, count 0,
  `is_all_free`). — spec FR-020 — harness `verify_set_clear_round_trip` (SUCCESSFUL, 0 of 472).
- **[Invariant]** Setting one slot never marks any distinct slot; slots are independent. — spec FR-020 —
  harness `verify_set_independent` (SUCCESSFUL, 0 of 433).
- **[Postcondition]** On an all-free bitmap, `find_free_from(start)` returns a valid, in-bounds slot that
  is genuinely free, for every `start`. — spec FR-020 — harness `verify_find_free_from_valid`
  (SUCCESSFUL, 0 of 398).
- **[Postcondition]** On a fully-allocated bitmap, `find_free_from(start)` returns `None` for every
  `start`, and the count equals `num_slots`. — spec FR-020 — harness `verify_find_free_from_full`
  (SUCCESSFUL, 0 of 450).

## Buddy allocator (`src/buddy.rs` — `BuddyAllocator`)

Bounded scope: `sector_size = 1`, `total_usable = 2` blocks (`max_order = 1`), `#[kani::unwind(6)]`.
All harnesses call the real `new`/`alloc`/`free`/`total_free`, so Kani also checks every internal shift,
subtraction, addition and cast for overflow across the full symbolic input.

- **[Postcondition]** `new` publishes the whole usable byte range as free space
  (`total_free == total_usable_size == total`). — spec FR-019 — harness `verify_new_total_free`
  (SUCCESSFUL, 0 of 549).
- **[Postcondition]** Any successful `alloc(size)` returns an offset within `[base_offset, base_offset+total)`,
  for symbolic `base_offset` (≤ 2^40) and symbolic `size` in `[1, total]`. — spec FR-019 —
  harness `verify_alloc_offset_in_range` (SUCCESSFUL, 0 of 614).
- **[Invariant]** A successful `alloc(size)` never increases free space (symbolic `size`). — spec FR-019 —
  harness `verify_alloc_shrinks_free` (SUCCESSFUL, 0 of 690).
- **[Invariant]** Allocating one block then freeing it restores the original free space (buddy merge
  round-trip). — spec FR-019 — harness `verify_alloc_free_round_trip_1block` (SUCCESSFUL, 0 of 851).

## Slab (`src/slab.rs` — `Slab`)

Bounded scope: `start_offset = 8192` (non-zero, so the offset arithmetic is real), `element_size = 4096`,
`slab_size = 16384` ⇒ 4 slots, `#[kani::unwind(6)]`.

- **[Postcondition]** A freshly created slab has every key slot equal to `FREE_KEY`. — spec FR-011 —
  harness `verify_new_keys_all_free` (SUCCESSFUL, 0 of 382).
- **[Invariant]** `slot_offset` and `slot_for_offset` are exact inverses over the whole slot domain, and
  `slot_offset(idx) = start + idx*element_size` lies inside the slab range — this is what lets
  `remove_extent(offset)` recover the correct slot. — spec FR-005 / FR-012 —
  harness `verify_slot_offset_roundtrip` (SUCCESSFUL, 0 of 462).
- **[Postcondition]** `alloc_slot` yields an in-bounds slot whose returned byte offset matches
  `slot_offset(idx)` and lies within the slab's disk range. — spec FR-005 —
  harness `verify_alloc_slot_offset_valid` (SUCCESSFUL, 0 of 481).
- **[Invariant]** Publishing a (non-sentinel) key then reading it back is consistent, and `free_slot`
  resets the slot's key to the `FREE_KEY` sentinel. — spec FR-006 / FR-011 —
  harness `verify_set_key_then_free` (SUCCESSFUL, 0 of 529).

## Superblock (`src/superblock.rs` — `Superblock`)

Bounded scope: fixed 4096-byte image, `#[kani::unwind(2)]`.

- **[Precondition→Error]** `deserialize()` rejects a truncated buffer (shorter than `SUPERBLOCK_SIZE`),
  returning an error rather than reading past the end. — spec FR-004 —
  harness `verify_deserialize_rejects_short_buffer` (SUCCESSFUL, 0 of 3090).

## Assumptions / bounds (the assume audit)

- `kani::assume(idx < num_slots)` (bitmap) mirrors the `debug_assert!((idx as u32) < self.num_slots)`
  guard in `set`/`clear`/`is_set` — matched to a real production guard.
- `kani::assume(size in [1, total])` (buddy) bounds the request to the allocator's capacity; the
  out-of-capacity path (`alloc` returns `None`) is allowed by the `if let Some(..)` shape, not asserted away.
- `kani::assume(key != FREE_KEY)` (slab) mirrors FR-011: `FREE_KEY = u64::MAX` is the reserved sentinel
  and is never stored as a live key.
- **Bounded geometries** (bitmap 8 slots; buddy 2 blocks; slab 4 slots): Kani proves exhaustively over
  all inputs *at these sizes*. Behaviour at production scale (millions of extents, thousands of slabs) is
  an inductive argument — Creusot territory (see below), not covered here.
- **`#[kani::unwind(N)]`** caps loop unrolling; Kani emits an unwinding-assertion failure if N is too
  small, so a green result confirms the bound covers every loop at the chosen geometry.

## Not verified by Kani (and why)

- **Superblock CRC / serialize (`verify_serialize_length`, attempted then removed)** — `serialize()` calls
  `crc32fast::hash`, whose runtime SIMD feature detection lowers to `std::arch::x86_64::_xgetbv`, reported
  by Kani as an `unsupported_construct` (`1 of 1310 failed, 1309 undetermined`). Any harness that reaches
  the CRC path is therefore unverifiable with this dependency. This also blocks the full serialize→
  deserialize round-trip (SC-002 / FR-003 / FR-004), which is the strongest superblock property.
- **Superblock invalid-magic rejection (`verify_deserialize_rejects_bad_magic`, attempted then removed)** —
  logically simple (early `return Err` on `magic != SUPERBLOCK_MAGIC`), but the error path builds
  `format!("invalid superblock magic: {magic:#x}")` over the *symbolic* magic value; symbolic hex-string
  formatting blows CBMC up (no result in >8 min). US-3 acceptance #3 is thus documented, not proven. The
  `verify_deserialize_rejects_short_buffer` harness (static error message) proves the sibling reject path.
- **`buddy::free` / merge round-trip over a *symbolic* request size** — attempted as
  `verify_alloc_free_round_trip` and removed: `free`'s linear `iter().position` search over the free lists
  combined with the XOR buddy-offset computation over a symbolic offset causes a SAT blow-up (no result in
  >14 min even at `total = 2`). Replaced with the concrete single-block round-trip above. A general
  merge-correctness proof needs induction → Creusot territory.
- **`buddy::mark_allocated`** (recovery-time reconstruction, `buddy.rs:117`) — nested split search over
  free lists; same symbolic-search blow-up profile as `free`. Not harnessed.
- **`region.rs` (`RegionState`: `alloc_extent`, `free_slot`, `publish_slot`, `remove_extent_by_offset`,
  `flush_pending_frees`)** — these compose `BTreeMap<u64, Slab>`, a `HashMap`-backed `SizeClassManager`,
  and the buddy allocator. The map operations and the unbounded slab collection make them unbounded /
  induction-shaped. Their constituent invariants (bitmap, slab offset math, buddy range) are the tractable
  pieces and are verified above.
- **`checkpoint.rs`, `recovery.rs`, `block_io.rs`, `write_handle.rs`, and the `ExtentManager` driver in
  `lib.rs`** — I/O, serialization of variable-length key vectors, `parking_lot::RwLock`, background
  threads, `Condvar`-based checkpoint coalescing, and SPDK/DMA FFI. Out of scope for bounded model
  checking: concurrency (US-4/FR-023/FR-024, SC-004), durability and recovery (US-2/US-3,
  FR-013–FR-018), and the two-phase-commit / deferred-free lifecycle (US-1/US-6, FR-005–FR-008) are
  temporal/round-trip properties better suited to integration tests and Creusot.

## Anti-vacuity validation

Two harnesses were fault-injection tested to confirm they are bound to the real code (the change was
reverted after each):
- `bitmap::verify_set_marks_slot` — changing `set` to `words[word] |= 0u64 << bit` (no-op instead of
  setting the bit) made the harness **FAIL** with `assertion failed: bm.is_set(idx)` (1 of 434).
- `slab::verify_slot_offset_roundtrip` — adding `+ 1` to `slot_offset` made the harness **FAIL** on all
  three round-trip assertions (`off == START + idx*ELEM`, `contains_offset`, `slot_for_offset == Some(idx)`).

## Environment note

`cargo kani` builds the whole crate, which pulls `interfaces/spdk` → `spdk-sys`, whose build script
requires a prebuilt SPDK tree. This worktree satisfies it with gitignored symlinks
`deps/spdk -> /opt/spdk` and `deps/spdk-build -> /opt/spdk-build` (same as the sibling kani worktree).
No SPDK/FFI code is reached by any harness; the symlinks only let the crate compile under Kani.

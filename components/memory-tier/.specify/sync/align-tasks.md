# Alignment Tasks

Generated: 2026-07-22

These tasks were deferred from spec-sync AUTO-BACKFILL rather than resolved, because
each requires a judgment call or code investigation beyond documentation drift. Do not
apply either "Option" below without maintainer sign-off.

## Task: Align 001-memory-tier/sharding-not-implemented

**Severity**: High

**Spec Requirement**: FR-005 ("Pool is divided into 16 independent shards"), FR-006
("Shard selection uses key modulo 16"), FR-007 ("Each shard has its own Mutex-protected
allocator and slot map"), NFR-002 ("Per-shard locking minimizes contention, 16-way
parallelism"), FR-013 ("evict_lru() cycles through shards via atomic counter,
round-robin"), FR-021 ("oldest_keys(n) peeks at N oldest keys across shards, sampling
(n / NUM_SHARDS).max(1) per shard"), SC-3 ("Concurrent access from 16+ threads does not
deadlock or corrupt state" — implicitly relies on 16 independent shard locks for
parallelism, not just correctness).

**Current Code**: `components/memory-tier/src/lib.rs` implements a single, unsharded
`Pool` (one `FreeList` allocator + one `HashMap<CacheKey, Slot>`) behind one
`RwLock<Pool>` (lines ~75-89). There is no `Shard` struct, no `NUM_SHARDS` constant, no
`shard_for_key()` function, and no `evict_counter` atomic anywhere in `src/`.
`evict_lru()` (lines 431-463) calls `ep.pop_oldest(state.pool_id)` once against the
single global pool. `oldest_keys()` (lines 421-429) calls `ep.peek_oldest(state.pool_id,
n)` once, with no per-shard sampling. All writers across all keys contend for the one
`RwLock`, not one of 16 shard-scoped locks.

**Required Change**: This is a judgment call — two options, pick one with maintainer
input:
- **Option A (implement)**: Build the 16-way shard architecture as specified in
  `plan.md` (per-shard `FreeList` + `HashMap`, `Mutex<Shard>` per shard,
  `key % NUM_SHARDS` selection, `AtomicUsize` round-robin counter for `evict_lru()`,
  per-shard sampling in `oldest_keys()`). This is the larger effort but matches the
  documented design intent (plan.md Memory Layout / Concurrency Model / Key Design
  Decision #1 and #6).
- **Option B (backfill down)**: If sharding was intentionally dropped in favor of the
  simpler single-pool design (e.g., because the external `IEvictionPolicy` receptacle
  already provides its own internal synchronization, making per-shard locks
  redundant), rewrite `spec.md` FR-005/006/007, NFR-002, FR-013, FR-021, SC-3 and
  `plan.md`'s Memory Layout / Pointer Arithmetic / Concurrency Model / Key Design
  Decision sections to describe the actual single-pool design, and update
  `components/interfaces/src/imemory_tier.rs` doc comments (which still claim
  `P4 (shard-bounded)` / `P5 (shard-deterministic)` / `P10 (evict-round-robin)`) to
  match.

Do NOT silently pick Option B without maintainer confirmation — this drift report
deliberately left the spec's sharded description intact rather than erasing it.

**Files to Modify** (once a decision is made): `components/memory-tier/src/lib.rs`
(Option A) or `components/memory-tier/specs/001-memory-tier/spec.md`,
`components/memory-tier/specs/001-memory-tier/plan.md`,
`components/interfaces/src/imemory_tier.rs` (Option B).

**Estimated Effort**: large (Option A) / medium (Option B)

---

## Task: Align 001-memory-tier/evict-lru-for-key-ignores-key

**Severity**: High

**Spec Requirement**: FR-014 — "`evict_lru_for_key()` evicts from the same shard as the
target key."

**Current Code**: `components/memory-tier/src/lib.rs:465-467` — the `key` parameter is
bound as `_key` and never read; the function body is identical to `evict_lru()`. It is a
dead alias with no key-scoped behavior.

**Required Change**: Coupled to the sharding decision above:
- If Option A (implement sharding) is chosen: make `evict_lru_for_key()` compute the
  target key's shard and evict only from that shard's eviction pool.
- If Option B (backfill down) is chosen: either (i) remove `evict_lru_for_key()` from
  the public `IMemoryTier` interface and spec as a redundant alias for `evict_lru()`,
  or (ii) rename/redocument it as an intentionally-global eviction call and update
  FR-014's wording to drop the "same shard" claim.

**Files to Modify**: `components/memory-tier/src/lib.rs`,
`components/interfaces/src/imemory_tier.rs`,
`components/memory-tier/specs/001-memory-tier/spec.md` (FR-014).

**Estimated Effort**: small (once the sharding decision is made)

---

## Task: Align 001-memory-tier/creusot-proofs-absent

**Severity**: Medium

**Spec Requirement**: SC-8 — "10 formal properties verified with Creusot (21
verification conditions)"; plan.md's "Formal Verification (Creusot)" section lists
P1-P10 by name.

**Current Code**: No `verif/`, `creusot/`, or any Creusot-related directory or file
exists anywhere under `components/memory-tier/`. `components/interfaces/src/
imemory_tier.rs` doc comments (lines ~53-68, and per-method annotations at lines
~107-166) assert `# Verified: P4 (shard-bounded)`, `P5 (shard-deterministic)`,
`P10 (evict-round-robin)` and restate all 10 properties as "formally proved," despite
three of them (P4, P5, P10) describing sharded behavior that does not exist in code —
so they could not have been discharged against the current implementation even if the
proof artifacts existed elsewhere.

**Required Change**: Judgment call — two options:
- **Option A**: Produce the actual Creusot proof artifacts (requires `tools-creusot-*`
  skills: init, annotate-interfaces, extract-spec-prop) for the properties that are
  implementable against the current (or future sharded) code, and remove/soften claims
  for any property that depends on an architecture decision not yet made (P4, P5, P10
  depend on the sharding outcome above).
  ```
  cargo/creusot toolchain: tools-creusot-install, tools-creusot-init,
  tools-creusot-annotate-interfaces (available for other components; not yet run here)
  ```
- **Option B**: Remove or soften the SC-8 claim and the interface doc comments' "Verified"
  annotations until proof artifacts exist, replacing with "planned" language.

**Files to Modify**: `components/interfaces/src/imemory_tier.rs` (doc comments),
`components/memory-tier/specs/001-memory-tier/spec.md` (SC-8),
`components/memory-tier/specs/001-memory-tier/plan.md` (Formal Verification section),
plus new proof artifacts if Option A is chosen.

**Estimated Effort**: large (Option A) / small (Option B)

---

## Task: Align 001-memory-tier/version-mismatch

**Severity**: Medium

**Spec Requirement**: NFR-008 — "Component version is 0.2.0."

**Current Code**: Three different version strings exist, none of which is `0.2.0`:
- `components/memory-tier/Cargo.toml:3` — package `version = "0.1.0"`
- `components/memory-tier/src/lib.rs:139` — `define_component!` macro `version:
  "0.3.0"` field

**Required Change**: Do not guess which value is authoritative. Reconcile all three to a
single version number, driven by:
1. Whether a `0.1.0` → `0.2.0`/`0.3.0` release actually happened (check CHANGELOG / git
   tags / release history for `memory-tier`), and
2. Whether the sharding decision above (Option A vs B) itself warrants a version bump
   (e.g., landing sharding = minor/major bump; backfilling the spec down = patch bump
   to align docs with the already-released `0.1.0`/`0.3.0` code).

Once decided, update `Cargo.toml` and the `define_component!` macro's `version:` field
to match each other, and update `spec.md` NFR-008 to match both.

**Files to Modify**: `components/memory-tier/Cargo.toml`,
`components/memory-tier/src/lib.rs` (`define_component!` macro),
`components/memory-tier/specs/001-memory-tier/spec.md` (NFR-008).

**Estimated Effort**: small (mechanical fix once the target version is decided)

---

## Task: Align 001-memory-tier/readme-source-layout-drift

**Severity**: Low

**Spec Requirement**: Not a `spec.md` FR, but flagged by drift analysis:
`components/memory-tier/README.md`'s Architecture / Source Layout sections describe an
`LruList`/`lru.rs` module ("index-based doubly-linked list") for eviction.

**Current Code**: No `lru.rs` module exists in `components/memory-tier/src/`. Eviction
is fully delegated to the external `IEvictionPolicy` receptacle (see FR-024); the only
source files are `src/lib.rs` and `src/allocator.rs`.

**Required Change**: Update `README.md`'s Architecture and Source Layout sections to
remove the `lru.rs`/`LruList` description and describe eviction as delegated to the
`IEvictionPolicy` receptacle instead. `tasks.md` already has an open checklist item for
this ("Verify README.md matches current source layout (lru.rs removed, eviction
delegated)").

**Files to Modify**: `components/memory-tier/README.md` (out of scope for this
spec-sync pass — README.md is not under `specs/**`, so it was not edited here).

**Estimated Effort**: small

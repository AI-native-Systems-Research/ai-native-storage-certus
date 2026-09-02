---
spec_sync_component: memory-tier
spec_sync_drift_status: drift
spec_sync_synced_at: 2026-09-02T21:40:18Z
spec_sync_git_commit: 2fc1cd3c
spec_sync_inputs_sha256: b714aa50e56f8418efdf2507d8dcf1f01bab9f765be901564fff8acf01060d7d
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — memory-tier

**Generated**: 2026-09-02 (spec-sync re-run)

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 (+ plan.md, tasks.md supporting artifacts) |
| Requirements Checked | 47 (29 FR + 11 NFR + 7 SC) |
| Aligned | 46 |
| Drifted (spec-numbered) | 1 (NFR-008) |
| Drifted (supporting artifacts, resolved this pass by BACKFILL) | 2 (plan.md, tasks.md) |
| Not Implemented | 0 |
| Unspecced | 0 |

**Context**: `spec.md` was already backfilled to the single-`RwLock<Pool>` reality
in the 2026-08-20 Phase B pass and is fully aligned with `src/`. The previously
committed drift-report described the *pre-backfill* sharded spec and was stale.
This re-run re-verifies every requirement against the current spec + code, and
extends the backfill to the two supporting spec artifacts (`plan.md`, `tasks.md`)
that still described the never-built 16-way sharding and Creusot P1–P10 material.

`drift_status: drift` because two actionable items remain **unresolved and out of
this pass's edit scope** (NFR-008 version mismatch, and a stale interface doc
comment in `components/interfaces/`). They are not stamped `clean`.

## Spec: 001-memory-tier — Memory Tier (DRAM Cache Pool)

### Aligned ✓ — spec.md matches code

All 29 FRs, 10 of 11 NFRs, and all 7 Success Criteria are aligned. Representative
evidence (verified this pass):

| Req | Evidence |
|-----|----------|
| FR-001 single contiguous mmap region | `src/lib.rs:190-224` (`alloc_mmap`) |
| FR-002 hugepage `MAP_HUGETLB` + fallback | `src/lib.rs:200`, fallback `src/lib.rs:206-223` |
| FR-003 `spdk_zmalloc` when SPDK active | `src/lib.rs:277-303` (feature-gated) |
| FR-004 4 KiB alignment | `ALIGNMENT=4096`, `next_multiple_of` `src/allocator.rs:5,42,60` |
| FR-005 single unsharded `RwLock<Pool>` | `Pool{allocator,slots}` `src/lib.rs:76-79`; `pool: RwLock<Pool>` `src/lib.rs:85` |
| FR-006 one reader-writer lock; readers shared / mutators exclusive | reads `get/peek/contains/batch_touch/capacity/used` take `.read()`; mutators `insert/remove/evict/clear` take `.write()` — `src/lib.rs:334-608` |
| FR-007 single `FreeList` + single `HashMap<CacheKey,Slot>` | `src/lib.rs:76-79` |
| FR-008 insert zero size → InvalidSize | `src/lib.rs:329-331` |
| FR-009 insert duplicate → AlreadyExists | `src/lib.rs:356-358` |
| FR-010 insert PoolFull | `src/lib.rs:360-363` |
| FR-011 get returns ptr+size, updates order | `src/lib.rs:381-410` (touch applied after `drop(pool)`) |
| FR-012 peek without order update | `src/lib.rs:412-422` |
| FR-013 evict_next delegates to `identify_next_to_evict`; no round-robin | `src/lib.rs:434-466` |
| FR-014 evict_next_for_key is alias, `_key` ignored | `src/lib.rs:468-470` |
| FR-015 remove frees; KeyNotFound absent | `src/lib.rs:472-503` |
| FR-016 touch updates order | `src/lib.rs:505-518` |
| FR-017 batch_touch amortizes lock | `src/lib.rs:520-555` |
| FR-018 clear resets, returns count | `src/lib.rs:594-608` |
| FR-019 NUMA mbind w/ fallback | `src/lib.rs:225-254` |
| FR-020 is_dma_capable true only for SPDK | `src/lib.rs:610-613` |
| FR-021 oldest_keys single `get_eviction_candidates` call | `src/lib.rs:424-432` |
| FR-022 pool_info base ptr + size | `src/lib.rs:585-592` |
| FR-023 initialized flag guard | `initialized.load(Acquire)` on all data-path ops `src/lib.rs` |
| FR-024 IEvictionPolicy receptacle | `src/lib.rs:143-144` |
| FR-025 ILogger optional receptacle | `src/lib.rs:143` |
| FR-026 free-list coalescing | `src/allocator.rs:59-83` |
| FR-027 telemetry counters (feature) | `src/lib.rs:37-60` |
| FR-028 telemetry_snapshot / telemetry / reset | `src/lib.rs:166-177,615-635` |
| FR-029 free_capacity() | `src/lib.rs:180-187` |
| NFR-001/002/003/004/005/006/007/009/010/011 | RwLock<Pool>, touches outside lock, BTreeMap first-fit, Send/Sync `src/lib.rs:95-96`, Drop `src/lib.rs:116-136`, `DEFAULT_POOL_SIZE` `src/lib.rs:34`, spdk feature `Cargo.toml:9` |
| SC-1..SC-7 | unit tests pass; Drop frees; RwLock concurrency; 4 KiB invariant; evict→reuse; NUMA fallback; SPDK DMA path |

Minor observation (not drift): `is_dma_capable()` (`src/lib.rs:610-613`) and
`telemetry_snapshot()` (`src/lib.rs:615-635`) do not check the `initialized`
flag, unlike the FR-023 data-path operations. Both are harmless read-only status
queries (they return `spdk_allocated` / cumulative counters), so FR-023 stays
Aligned.

### Drifted ⚠️

- **NFR-008 (component version) — UNRESOLVED (HUMAN_DECISION, out of scope).**
  Three-way mismatch with no authoritative value: `Cargo.toml:3` = `0.1.0`,
  `define_component!` macro `version:` = `0.3.0` (`src/lib.rs:140`), `spec.md`
  NFR-008 = `0.2.0`. Reconciling requires editing `Cargo.toml` and `src/lib.rs`
  (both outside spec-sync edit scope) plus a maintainer choosing the real
  version. **minor severity, but actionable** → keeps `drift_status: drift`.
  Tracked in `.specify/sync/align-tasks.md`.

- **plan.md architecture sections — RESOLVED THIS PASS (BACKFILL).** `plan.md`
  still described a 16-way sharded pool (Memory Layout, Pointer Arithmetic,
  Concurrency Model with per-shard Mutex, Key Design Decisions #1/#6) and a
  "Formal Verification (Creusot)" section listing P1–P10 (21 VCs). Neither the
  sharding nor the proofs were ever built (`grep` for `shard|NUM_SHARDS|creusot`
  in `src/` returns nothing; no `components/memory-tier/verif/` directory).
  Rewritten to the single-`RwLock<Pool>` design, matching `spec.md`. **moderate**
  (doc-vs-code contradiction) → resolved.

- **tasks.md stale references — RESOLVED THIS PASS (BACKFILL).** Removed
  "Confirm formal verification properties (P1-P10)", "Verify Creusot verification
  conditions still discharge", and "test for evict_next_for_key targeting correct
  shard"; rewrote the "shard layout" diagram task and "configurable shard count"
  backlog item to reflect the single-pool reality. **minor** → resolved.

### Not Implemented ✗

None. The current `spec.md` no longer mandates any unbuilt structure (the
never-built 16-way sharding, round-robin `evict_counter`, and Creusot SC-8 were
already removed from `spec.md` in the 2026-08-20 backfill).

## Out-of-scope drift (noted, not editable by this pass)

- **`components/interfaces/src/imemory_tier.rs:87-91` — stale `evict_next_for_key`
  doc comment.** Still reads "Evict the eviction policy's next victim from the
  **same shard as `key`**" and "Returns … `None` if the target shard is empty",
  describing sharding that does not exist. The `P4/P5/P10 "Verified"` overclaiming
  was already removed from this file on the main thread; only the "same shard"
  phrasing remains. **Cannot be edited** — `components/interfaces/**` is out of
  this component's edit scope. HUMAN_DECISION / cross-cutting interface fix.

- **`components/memory-tier/README.md:23-30` — nonexistent `src/lru.rs`.** The
  "Source Layout" block lists `lru.rs — Index-based doubly-linked list for O(1)
  LRU operations`, but no such file exists; eviction is delegated to the
  `IEvictionPolicy` receptacle (FR-024). Doc-only and **outside spec-sync scope**
  (`README.md` is not under `specs/**`). Tracked in `tasks.md` Documentation.

## Recommendations

1. **Reconcile NFR-008 version** across `Cargo.toml` (0.1.0), `define_component!`
   (0.3.0), and `spec.md` (0.2.0) to a single source of truth. This is the only
   actionable spec-numbered drift and requires a maintainer edit to `Cargo.toml` +
   `src/lib.rs`. (Blocks `clean`.)
2. **Fix the `evict_next_for_key` doc comment** in
   `components/interfaces/src/imemory_tier.rs` to drop the "same shard" language
   (the pool is unsharded; `key` is ignored). Requires an interface-crate edit.
3. **Fix README source layout** — remove the `lru.rs` entry; note eviction is
   delegated to `IEvictionPolicy`.

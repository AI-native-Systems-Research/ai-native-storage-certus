# Spec ↔ Implementation Drift Report — memory-tier

**Generated**: pending

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 40 (29 FR + 11 NFR) + 8 SC |
| Aligned | 33 |
| Drifted | 8 |
| Not Implemented | 3 |
| Unspecced | 0 |

The dominant drift is the **16-way sharding architecture** described throughout
the spec that does not exist in code (single `RwLock<Pool>`). The spec's own
"Spec-Sync Notes (2026-07-22)" already enumerate most of these as intentionally
deferred; this report confirms they still hold and adds the stale
verification-claim finding.

## Spec: 001-memory-tier — Memory Tier (DRAM Cache Pool)

### Aligned ✓

| Req | Evidence |
|-----|----------|
| FR-001 single contiguous mmap region | `src/lib.rs:190-224` (`alloc_mmap`) |
| FR-002 hugepage w/ fallback | `MAP_HUGETLB` then plain mmap `src/lib.rs:195-223` |
| FR-003 spdk_zmalloc when SPDK active | `src/lib.rs:277-303` |
| FR-004 4 KiB alignment | `ALIGNMENT=4096`, `next_multiple_of` `src/allocator.rs:5,42,60` |
| FR-008 insert zero size → InvalidSize | `src/lib.rs:329-331` |
| FR-009 insert duplicate → AlreadyExists | `src/lib.rs:356-358` |
| FR-010 insert PoolFull | `src/lib.rs:360-363` |
| FR-011 get returns ptr+size, updates order | `src/lib.rs:381-410` |
| FR-012 peek without order update | `src/lib.rs:412-422` |
| FR-015 remove frees; KeyNotFound absent | `src/lib.rs:472-503` |
| FR-016 touch updates order | `src/lib.rs:505-518` |
| FR-017 batch_touch amortizes lock | `src/lib.rs:520-555` |
| FR-018 clear resets, returns count | `src/lib.rs:594-608` |
| FR-019 NUMA mbind w/ fallback | `src/lib.rs:225-254` |
| FR-020 is_dma_capable true only for SPDK | `src/lib.rs:610-613` |
| FR-022 pool_info base ptr + size | `src/lib.rs:585-592` |
| FR-023 initialized flag guard on all ops | `initialized.load(Acquire)` throughout `src/lib.rs` |
| FR-024 IEvictionPolicy receptacle | `define_component!` `src/lib.rs:143-144` |
| FR-025 ILogger optional receptacle | `src/lib.rs:143` |
| FR-026 free-list coalescing | `src/allocator.rs:59-83` |
| FR-027 telemetry counters (feature) | `src/lib.rs:37-60` |
| FR-028 telemetry_snapshot / telemetry / reset | `src/lib.rs:166-177,615-635` |
| FR-029 free_capacity() | `src/lib.rs:180-187` |
| NFR-001/003/004/005/006/007/009/010/011 | free-list BTreeMap, Drop, Send/Sync, spdk feature — `src/lib.rs`, `src/allocator.rs` |

### Drifted ⚠️

- **FR-005 / FR-007 / NFR-002 (16 independent shards, per-shard lock)** — spec: pool
  divided into 16 Mutex-protected shards for 16-way parallelism. actual: single
  `RwLock<Pool>` with one `FreeList` + one `HashMap`. `src/lib.rs:76-85,313-316`.
  **major** (spec-acknowledged deferred; align-task "sharding-not-implemented").
- **FR-006 (shard = key modulo 16)** — no shard selection exists in code.
  `src/lib.rs` has no `shard_for_key`. **major** (same root cause as above).
- **FR-013 (evict_next round-robin via atomic counter)** — spec: cycles shards via
  atomic. actual: `evict_next` delegates to `ep.identify_next_to_evict`; no
  `evict_counter` field exists. `src/lib.rs:434-466`. **moderate**.
- **FR-014 (evict_next_for_key targets key's shard)** — actual: `_key` ignored; pure
  alias for `evict_next`. `src/lib.rs:468-470`. **moderate** (align-task
  "evict-lru-for-key-ignores-key").
- **FR-021 (oldest_keys per-shard `(n/NUM_SHARDS)` sampling)** — actual: single call
  `ep.get_eviction_candidates(pool_id, n)`, no per-shard sampling. `src/lib.rs:424-432`.
  **minor** (current behavior is simpler and correct; only the sampling mechanism drifted).
- **NFR-008 (version 0.2.0)** — three-way mismatch: `Cargo.toml` = `0.1.0`,
  `define_component!` `version:` = `0.3.0` (`src/lib.rs:140`), spec says `0.2.0`.
  **minor** (align-task "version-mismatch").
- **SC-003 / SC (16-thread concurrency via shard locks)** — concurrency real but
  serialized through a single `RwLock`, not 16 shard locks as claimed. **moderate**.
- **IMemoryTier doc comments assert stale "Verified P1–P10 / 16 shards"** — 
  `components/interfaces/src/imemory_tier.rs:53-119` claims Creusot-verified
  properties and "index < 16 / cycles through all 16 shards" that describe the
  non-existent sharded design and non-existent proofs. **major** (misleading
  contract documentation; align-task "creusot-proofs-absent").

### Not Implemented ✗

- **SC-8 (10 Creusot properties, 21 verification conditions)** — no proof artifacts
  under `components/memory-tier/` (`verif/` absent). Referenced by FR-006, FR-014,
  FR-013, FR-023 "Verified" columns.
- **16-way sharded allocator/slot-map** (FR-005/006/007) — architecture absent.
- **Round-robin eviction counter** (FR-013) — field/logic absent.

## Unspecced Features

None. All implemented surface (telemetry, free_capacity, DEFAULT_POOL_SIZE) is
now captured in the spec (backfilled 2026-07-22). `DEFAULT_POOL_SIZE` is a
declared-but-unused constant — documented in Implementation Notes.

## Recommendations

1. **Decide sharding fate** (highest priority): either implement the 16-way
   sharded pool (FR-005/006/007/013/014/021, NFR-002) or rewrite the spec to
   describe the shipped single-`RwLock<Pool>` design. This decision blocks
   several FRs and SC-003/SC-8.
2. **Fix stale verification claims** in `components/interfaces/src/imemory_tier.rs:53-119`
   — remove or requalify the "Verified P1–P10" and "16 shards" doc comments until
   proofs and sharding actually exist.
3. **Reconcile version** across `Cargo.toml` (0.1.0), `define_component!` (0.3.0),
   and spec NFR-008 (0.2.0) to a single source of truth.
4. Either honor `key` in `evict_next_for_key` (FR-014) or respec it as an alias.

# Drift Report: memory-tier

Generated: 2026-08-07T15:28:26Z

Spec: `specs/001-memory-tier/spec.md` (Status: Backfilled)
Implementation: `src/lib.rs`, `src/allocator.rs`
Interface: `components/interfaces/src/imemory_tier.rs`
Cargo: `Cargo.toml` (features: `spdk`, `telemetry`)

## Summary

| Class | Count |
|-------|-------|
| Aligned | 30 |
| Drifted | 9 |
| Not Implemented | 1 (SC-008 Creusot proofs) |
| Unspecced | 0 (new); 2 reserved-for-future items already documented |

Most drift is **already known and intentionally deferred** — see the spec's
"Spec-Sync Notes (2026-07-22)" section and `.specify/sync/align-tasks.md`. The
central divergence is that the spec describes a 16-way sharded pool that the
implementation does not have (single `RwLock<Pool>`). No new drift or unspecced
public surface was introduced since the 2026-07-22 analysis; this report refreshes
timestamps and re-confirms line locations against current `src/lib.rs`.

## Detailed Findings

### Functional Requirements

| ID | Status | Location / Note |
|----|--------|-----------------|
| FR-001 single contiguous mmap region | Aligned | `src/lib.rs:190-257` (`alloc_mmap`) |
| FR-002 hugepage (MAP_HUGETLB) preferred, fallback | Aligned | `src/lib.rs:195-223` |
| FR-003 SPDK spdk_zmalloc when SPDK active | Aligned | `src/lib.rs:277-303` (feature `spdk`) |
| FR-004 4 KiB-aligned allocations | Aligned | `src/allocator.rs:5,42,60` |
| FR-005 16 independent shards | **Drifted (HIGH)** | No shards; single `Pool` in `RwLock<Pool>` `src/lib.rs:76-90`. Known / deferred. |
| FR-006 shard = key mod 16 | **Drifted (HIGH)** | No sharding; "Verified: Creusot P4,P5" claim also stale (no proofs). |
| FR-007 per-shard Mutex allocator+slot map | **Drifted (HIGH)** | Single pool, single lock. Known / deferred. |
| FR-008 insert rejects zero → InvalidSize | Aligned | `src/lib.rs:329-331` |
| FR-009 insert rejects duplicate → AlreadyExists | Aligned | `src/lib.rs:356-358` |
| FR-010 insert → PoolFull when allocator can't satisfy | Aligned | `src/lib.rs:360-363` |
| FR-011 get returns ptr+size, updates eviction order | Aligned | `src/lib.rs:381-410` |
| FR-012 peek returns ptr+size, no eviction update | Aligned | `src/lib.rs:412-422` |
| FR-013 evict_next round-robin via atomic counter | **Drifted (HIGH)** | No `evict_counter`; `evict_next` just calls `ep.identify_next_to_evict` `src/lib.rs:434-466`. No round-robin. Stale "Verified P10". |
| FR-014 evict_next_for_key targets key's shard | **Drifted (HIGH)** | `key` ignored; pure alias for `evict_next` `src/lib.rs:468-470`. Known / deferred. |
| FR-015 remove frees slot; KeyNotFound for absent | Aligned | `src/lib.rs:472-503` |
| FR-016 touch updates eviction-order | Aligned | `src/lib.rs:505-518` |
| FR-017 batch_touch amortizes lock | Aligned | `src/lib.rs:520-555` |
| FR-018 clear removes all, resets, returns count | Aligned | `src/lib.rs:594-608` |
| FR-019 NUMA mbind with graceful fallback | Aligned | `src/lib.rs:225-254` |
| FR-020 is_dma_capable true only for SPDK pools | Aligned | `src/lib.rs:610-613` |
| FR-021 oldest_keys(n) peeks N oldest across shards | **Drifted (MEDIUM)** | Functionally returns N oldest via `ep.get_eviction_candidates` `src/lib.rs:424-432`; but "across shards / per-shard sampling" description is false (no shards). |
| FR-022 pool_info base ptr+size | Aligned | `src/lib.rs:585-592` |
| FR-023 all ops check initialized flag | Aligned | `src/lib.rs:334,383,414,426,436,474,507,525,559,569,578,587,596` |
| FR-024 eviction policy external receptacle | Aligned | `src/lib.rs:142-145` |
| FR-025 logger optional receptacle | Aligned | `src/lib.rs:142-145,153-163` |
| FR-026 free-list coalesces adjacent regions | Aligned | `src/allocator.rs:59-83` |
| FR-027 telemetry counters (feature-gated) | Aligned | `src/lib.rs:37-68` (feature `telemetry`) |
| FR-028 telemetry_snapshot + telemetry()/reset_telemetry() | Aligned | `src/lib.rs:166-177,615-635` |
| FR-029 free_capacity() = capacity − used | Aligned | `src/lib.rs:180-187` |

### Non-Functional Requirements

| ID | Status | Location / Note |
|----|--------|-----------------|
| NFR-001 O(1) LRU ops via policy | Aligned | delegated to `IEvictionPolicy` |
| NFR-002 per-shard 16-way parallelism | **Drifted (HIGH)** | Single `RwLock<Pool>`; no shard-level parallelism. Known / deferred. |
| NFR-003 no syscalls on data path | Aligned | data path is pure memory ops |
| NFR-004 thread-safe Send+Sync | Aligned | `src/lib.rs:92-96` (`unsafe impl` with SAFETY note) |
| NFR-005 BTreeMap O(log n) first-fit | Aligned | `src/allocator.rs:3,10` |
| NFR-006 freed on Drop | Aligned | `src/lib.rs:116-136` |
| NFR-007 default pool size 256 MiB | Aligned (unused) | `DEFAULT_POOL_SIZE` `src/lib.rs:34`; declared but no call site (reserved-for-future, documented). |
| NFR-008 component version 0.2.0 | **Drifted (MEDIUM)** | Three-way mismatch: `Cargo.toml` = `0.1.0`, `define_component!` `version:` = `0.3.0` (`src/lib.rs:140`), spec says `0.2.0`. Known / deferred. |
| NFR-009 SPDK optional compile-time feature | Aligned | `Cargo.toml:9,18` |
| NFR-010 pointers DMA-suitable | Aligned | page-aligned contiguous pool |
| NFR-011 telemetry zero-cost when disabled | Aligned | all counters `#[cfg(feature = "telemetry")]` |

### Success Criteria

| ID | Status | Note |
|----|--------|------|
| SC-1 unit tests pass | Aligned | 13 tests in `src/lib.rs`, 9 in `src/allocator.rs` |
| SC-2 no leaks (Drop) | Aligned | `Drop for MemoryTierState` `src/lib.rs:116-136` |
| SC-3 16+ thread concurrency via shard locks | **Drifted (MEDIUM)** | Thread-safe, but via single `RwLock`, not shard locks. Known / deferred. |
| SC-4 4 KiB alignment invariant | Aligned | `src/allocator.rs` tests |
| SC-5 eviction frees + re-insert | Aligned | tests `evict_next_returns_some`, `remove_and_reuse` |
| SC-6 NUMA binding or fallback | Aligned | `src/lib.rs:225-254` |
| SC-7 SPDK DMA-capable pointers | Aligned | `src/lib.rs:277-303` (feature) |
| SC-8 10 Creusot properties / 21 VCs | **Not Implemented (HIGH)** | No `components/memory-tier/verif/` dir or proof artifacts exist. Interface doc comments (`imemory_tier.rs:53-68` and per-method "# Verified: P.." lines) assert proofs that are absent/stale. Spec references a nonexistent directory. |

## Spec References to Nonexistent Files/Proofs

- `components/interfaces/src/imemory_tier.rs:53` references
  `components/memory-tier/verif/` — the directory does not exist.
- SC-008 and every "# Verified: P1..P10" doc annotation in `imemory_tier.rs`
  claim Creusot proofs with no backing artifacts anywhere under the component.

## Unspecced Code

| Item | Location | Severity | Notes |
|------|----------|----------|-------|
| `MemoryTierError::NotEvictable` variant (defined, never constructed) | `components/interfaces/src/imemory_tier.rs:32` | Low | Reserved-for-future; already documented in spec Implementation Notes. |
| `DEFAULT_POOL_SIZE` public constant, no call site | `src/lib.rs:34` | Low | Reserved-for-future default-constructor path; documented in spec. |

No new unspecced public surface: `free_capacity`, `telemetry`/`reset_telemetry`,
`telemetry_snapshot` are all backfilled (FR-027..029, NFR-011).

## Recommendations

Resolve via `.specify/sync/align-tasks.md` (options already recorded there):
1. **sharding-not-implemented** (FR-005/006/007, NFR-002, SC-3): decide whether
   16-way sharding is intended future work or should be removed from the spec, then
   either implement shards or rewrite the spec to the single-pool design. Highest impact.
2. **evict-lru-for-key-ignores-key** (FR-013, FR-014): implement round-robin /
   key-targeted eviction, or respec both as policy-delegated single-pool eviction.
3. **creusot-proofs-absent** (SC-008): either add the `verif/` proofs or strip the
   stale "# Verified" annotations from `imemory_tier.rs` and downgrade SC-008.
4. **version-mismatch** (NFR-008): reconcile `Cargo.toml` (0.1.0), macro (0.3.0),
   and spec (0.2.0) to a single version.
5. FR-021 wording: drop "per-shard sampling / across shards" to match the
   delegated single-pool implementation.

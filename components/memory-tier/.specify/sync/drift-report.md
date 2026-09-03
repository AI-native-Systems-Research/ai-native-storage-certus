---
spec_sync_component: memory-tier
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-03T18:54:21Z
spec_sync_git_commit: b220a1c8
spec_sync_inputs_sha256: d55cba7b408172a4f5bee951b6c01a7f34924f889bcf2b223c74eacb8614d04f
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec ↔ Implementation Drift Report — memory-tier

> **Digest refreshed 2026-09-03 (spec-sync gate fix).** The earlier
> `spec_sync_inputs_sha256` was computed in a working tree that contained an
> untracked `components/interfaces/src/iipc.rs` — a local file not part of this
> branch — which `scripts/spec-sync-hash.sh` folds into every component's
> interface hash, so CI's clean checkout recomputed a different digest. The
> stray file was removed and the digest recomputed in a clean tree. No `src/`,
> `specs/`, or report content changed; drift status remains `clean`. (This
> component's own sync — including the `imemory_tier.rs` doc-comment fix — is
> unchanged; the interface edit is already reflected in this digest.)

**Generated**: 2026-09-03
**Mode**: Read-only drift analysis, then ALIGN (doc→reality) + version reconciliation applied.

## Summary

| Metric | Count |
|--------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 40 (29 FR + 11 NFR) + 7 SC |
| Aligned | 40 |
| Drifted (this sweep) | 2 → both resolved |
| Not Implemented | 0 |
| Unspecced | 0 |

The 2026-08-20 Phase B backfill already rewrote the spec to describe the shipped
**single-`RwLock<Pool>`** design (retiring the never-built "16-way sharded pool +
Creusot-verified properties" narrative). This sweep confirms code and spec agree
on that reality and resolves the two items Phase B had left open:

1. the residual sharding language in the shared `IMemoryTier` doc comment, and
2. the three-way component-version mismatch (NFR-008).

Both are now fixed; no actionable drift remains.

## Spec: 001-memory-tier — Memory Tier (DRAM Cache Pool)

### Aligned ✓ (verified this sweep)

| Req | Evidence |
|-----|----------|
| FR-001 single contiguous mmap region | `src/lib.rs:190-224` (`alloc_mmap`) |
| FR-002 hugepage w/ fallback | `MAP_HUGETLB` then plain mmap `src/lib.rs:200-223` |
| FR-003 spdk_zmalloc when SPDK active | `src/lib.rs:277-303` |
| FR-004 4 KiB alignment | `ALIGNMENT`/`next_multiple_of` `src/allocator.rs` |
| FR-005 single unsharded pool behind one `RwLock<Pool>` | `struct Pool` + `pool: RwLock<Pool>` `src/lib.rs:76-85`; re-created in `initialize` `:313-316` |
| FR-006 read ops shared lock / mutations exclusive lock | reads `state.pool.read()` (`get :401`, `peek :418`, `contains :563`, `batch_touch :545`, `capacity :572`, `used :582`); mutations `state.pool.write()` (`insert :354`, `remove :494`, `evict_next :454`, `clear :602`) |
| FR-007 one first-fit `FreeList` + one `HashMap<CacheKey,Slot>` | `struct Pool { allocator: FreeList, slots: HashMap<..> }` `src/lib.rs:76-79` |
| FR-008 insert zero size → InvalidSize | `src/lib.rs:329-331` |
| FR-009 insert duplicate → AlreadyExists | `src/lib.rs:356-358` |
| FR-010 insert PoolFull | `src/lib.rs:360-363` |
| FR-011 get returns ptr+size, updates order | `src/lib.rs:381-410` |
| FR-012 peek without order update | `src/lib.rs:412-422` |
| FR-013 evict_next delegates to `identify_next_to_evict`; no shard counter | `src/lib.rs:434-466` |
| FR-014 evict_next_for_key is an alias for evict_next; `_key` ignored | `src/lib.rs:468-470` |
| FR-015 remove frees; KeyNotFound absent | `src/lib.rs:472-503` |
| FR-016 touch updates order | `src/lib.rs:505-518` |
| FR-017 batch_touch amortizes lock | `src/lib.rs:520-555` |
| FR-018 clear resets, returns count | `src/lib.rs:594-608` |
| FR-019 NUMA mbind w/ fallback | `src/lib.rs:225-254` |
| FR-020 is_dma_capable true only for SPDK | `src/lib.rs:610-613` |
| FR-021 oldest_keys = single `get_eviction_candidates(pool_id, n)` call | `src/lib.rs:424-432` |
| FR-022 pool_info base ptr + size | `src/lib.rs:585-592` |
| FR-023 initialized flag guard on all ops | `initialized.load(Acquire)` throughout `src/lib.rs` |
| FR-024 IEvictionPolicy receptacle | `define_component!` `src/lib.rs:142-145` |
| FR-025 ILogger optional receptacle | `src/lib.rs:143` |
| FR-026 free-list coalescing | `src/allocator.rs:59-83` (tests `coalesce_adjacent`, `coalesce_with_following`) |
| FR-027 telemetry counters (feature) | `src/lib.rs:37-60` |
| FR-028 telemetry_snapshot / telemetry / reset | `src/lib.rs:166-177,615-635` |
| FR-029 free_capacity() | `src/lib.rs:180-187` |
| NFR-001/002 RwLock serializes mutations, touches outside pool lock | `get`/`touch`/`batch_touch` drop pool guard before `ep.touch` `src/lib.rs:407-408,515-516,553-554` |
| NFR-003/004/005/006/007/009/010 | mmap data path, `unsafe impl Send/Sync` `:95-96`, BTreeMap free-list, Drop `:116-136`, DEFAULT_POOL_SIZE 256 MiB `:34`, spdk feature-gated |
| **NFR-008 component version = 0.3.0** | `Cargo.toml:3` = `0.3.0`, `define_component!` `version:` = `0.3.0` `src/lib.rs:140`, spec NFR-008 = `0.3.0` — **all three agree** |
| NFR-011 telemetry zero-cost when disabled | `#[cfg(feature = "telemetry")]` gating throughout `src/lib.rs` |
| SC-1 all unit tests pass | `cargo test -p memory-tier` → 21 passed, 0 failed |
| SC-2..SC-7 | Drop frees pool, RwLock concurrency, 4 KiB invariant, evict→reinsert, NUMA fallback, SPDK DMA path — all present |

### Drifted ⚠️ → resolved this sweep

- **Interface doc: `evict_next_for_key` described sharding** (ALIGN, doc→reality) —
  `components/interfaces/src/imemory_tier.rs:87-91` said "evict … from the same
  shard as `key`" / "target shard is empty", contradicting FR-014 and the code
  (`src/lib.rs:468-470` ignores `_key`). The interface tree is folded into this
  component's sync hash, so it is in scope. **Resolved**: doc comment rewritten to
  state the method is an alias for `evict_next` and that `key` is ignored because
  the pool is a single unsharded region. Severity: minor (documentation-only, no
  behavior change).
- **NFR-008 version three-way mismatch** (version reconciliation) — `Cargo.toml` =
  `0.1.0`, `define_component!` macro = `0.3.0` (`src/lib.rs:140`), spec = `0.2.0`.
  **Resolved by maintainer decision**: reconciled to **0.3.0** (the runtime-reported
  `define_component!` value is authoritative). `Cargo.toml` bumped `0.1.0`→`0.3.0`
  and spec NFR-008 updated `0.2.0`→`0.3.0`; the macro was already `0.3.0`. Severity:
  minor.

### Not Implemented ✗

None. The spec no longer asserts a 16-way sharded allocator, a round-robin
eviction counter, or Creusot-verified properties, so there are no unimplemented
requirements. `verif/` is correctly absent (SC-8 and the "Verified P#" columns
were removed in Phase B and are intentionally not re-added).

## Unspecced Features

None. All implemented surface (telemetry, `free_capacity`, `DEFAULT_POOL_SIZE`) is
captured in the spec. `DEFAULT_POOL_SIZE` remains a declared-but-unused constant,
documented in Implementation Notes.

## Recommendations

None outstanding. Commit this stamped `drift-report.md` together with the code and
spec changes (`Cargo.toml`, `spec.md`, `components/interfaces/src/imemory_tier.rs`)
so the CI Spec-Sync Gate sees a fresh report whose input hash matches the tree.

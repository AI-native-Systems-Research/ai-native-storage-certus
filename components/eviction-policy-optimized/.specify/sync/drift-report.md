---
spec_sync_component: eviction-policy-optimized
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-23T18:26:18Z
spec_sync_git_commit: 6cbc57cb
spec_sync_inputs_sha256: 27f5e798e0c205a4bfb46e2338f5344836f1429e871319586dc688ac46bf088f
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec Drift Report

Generated: 2026-09-23
Project: eviction-policy-optimized

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 19 (13 FR + 6 NFR) |
| ✓ Aligned | 19 (100%) |
| ⚠️ Drifted | 0 (0%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 0 |

> Note: `specs/001-optimized-eviction-policy/spec.md` was **backfilled from the
> implementation** in this same sync (status: Backfilled). By construction the
> spec documents current behavior, so this first analysis is expected to be
> aligned. The value here is the FR→code traceability recorded below; the
> behavioral review questions live in `tasks.md`, not as drift.

## Detailed Findings

### Spec: 001-optimized-eviction-policy — Optimized (TinyLFU) Eviction Policy Component

#### Aligned ✓

- **FR-001** create_pool → `src/lib.rs:94-116` (sequential IDs from 0; per-pool
  `lru`/`sketch`/`access_count`/`max_len` at `:104-111`).
- **FR-002** track: sketch increment, aging, admission, handle return;
  `_semantics` ignored; non-idempotent re-registration → `src/lib.rs:118-157`.
- **FR-003** touch → MRU, no sketch update → `src/lib.rs:159-173`.
- **FR-004** batch_touch, one lock per pool-run, empty no-op → `src/lib.rs:175-201`.
- **FR-005** remove unlinks in O(1) → `src/lib.rs:203-217`.
- **FR-006** identify_next_to_evict → `pop_front`/`None` → `src/lib.rs:219-224`.
- **FR-007** get_eviction_candidates → `peek_front_n`, empty for bad pool → `src/lib.rs:226-235`.
- **FR-008** len → `0` for bad pool → `src/lib.rs:237-246`.
- **FR-009** clear_pool clears list, preserves sketch/counters → `src/lib.rs:248-254`.
- **FR-010** InvalidPool on Result methods; graceful degradation elsewhere → `src/lib.rs:125,161,182-195,205,221,228,239,250`.
- **FR-011** admission gate `new_est >= 3 && new_est > victim_est` → push_back
  else push_front; empty → push_back → `src/lib.rs:144-154`.
- **FR-012** idempotent touch/remove on stale handle → guarded by
  `active` flag in `src/lru_list.rs:108-110` (move_to_back) and `:159-161` (remove).
- **FR-013** aging: halve every `max_len*2` accesses → `src/lib.rs:136-142`.
- **NFR-001** O(1) single-entry ops; sketch bounded by `CMS_ROWS`/`CMS_COLS`
  (`src/lib.rs:18-19,38-60`).
- **NFR-002/003** `RwLock<EvictionState>` + per-pool `Mutex` → `src/lib.rs:76-78,104,124,133`.
- **NFR-004** bounded sketch `[[u8; CMS_COLS]; CMS_ROWS]` → `src/lib.rs:27-29`.
- **NFR-005** Certus component model, provides IEvictionPolicy, single `logger`
  receptacle → `src/lib.rs:80-91`; drop-in via `full-optimized` profile.
- **NFR-006** one-time startup banner guarded by `Once` → `src/lib.rs:66,95-103`.

#### Drifted ⚠️

None.

#### Not Implemented ✗

None.

### Unspecced Code 🆕

None. The full public surface (`IEvictionPolicy` impl + `CountMinSketch` +
`LruList`) is covered by FR-001..FR-013 and the Key Entities section.

## Inter-Spec Conflicts

None. This component shares the `IEvictionPolicy` interface with
`eviction-policy-lru` and `eviction-policy-session-lists`; the deliberate
non-idempotent re-registration divergence is documented in FR-002 (consistent
with the LRU sibling's spec).

## Recommendations

1. Treat the backfilled spec as **Draft** and walk the `tasks.md` review
   checklist — in particular confirm the admission threshold (`>= 3`) and aging
   period (`max_len*2`) are the intended tuning, not incidental constants.
2. Add a direct admission-gate test (a hot key protected against a flood of cold
   keys); the current suite exercises the gate only implicitly.
3. Decide whether `clear_pool` should reset the frequency sketch (currently it
   does not — FR-009).

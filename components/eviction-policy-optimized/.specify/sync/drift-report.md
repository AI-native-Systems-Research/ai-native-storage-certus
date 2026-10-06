---
spec_sync_component: eviction-policy-optimized
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-10-06T23:46:10Z
spec_sync_git_commit: 64184293
spec_sync_inputs_sha256: 7635e51de06bf35575f1b1dab6b7ceb6176e7ffe7ae0158cd1b4bbe004f96b5c
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---

> **Re-stamp 2026-10-06:** inputs changed only through other components' interfaces (`interfaces/src/idispatcher.rs`, `interfaces/src/iremote_lookup.rs`, and the interfaces drift report), none of which eviction-policy-optimized uses; its own `src/`, `specs/` and `ieviction_policy.rs` are unchanged since the previous sync, so that analysis stands.
> **Sync 2026-10-05 (PR #505, FR-012 stale-handle fix).** Scope: the only `src/` change since the
> last `clean` stamp (2026-10-03, `2c86a131`) is the fix in `src/lru_list.rs`: `move_to_back` and
> `remove` now range-check the slot index before reading it, so a handle whose slot no longer exists
> (e.g. issued before `clear_pool`) is a silent no-op instead of a panic. **Re-checked** every
> requirement that code touches: FR-012 (now satisfied as written: no panic, no effect, `Ok(())`) and
> FR-009 (`clear`, unchanged behaviour). Spec edits: FR-012's guard description and citations, and
> FR-009's citation; every cited `lru_list.rs` line was re-mapped from the diff and re-read. No other
> requirement cites the changed code; this stamp asserts nothing beyond what was checked.
>
> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

> **Re-stamp 2026-10-05 (transitive: `components/interfaces` changed, branch `fix/poller-cpu-placement-logging`).** The only change to this component's hashed inputs is a doc-comment correction on `DispatcherConfig::poller_base_cpu` in `components/interfaces/src/idispatcher.rs` (no signature, type, or behaviour change). This component's own `src/**` and `specs/**` are unchanged, and it does not reference `poller_base_cpu` (verified by grep over its src/ and specs/). Its previous stamp was re-verified to match `git archive origin/unstable` before re-stamping. **No re-analysis was performed for this component**; this stamp asserts only what was checked.

> **Re-stamp 2026-10-01 (transitive: `components/interfaces` changed).** The spec-sync digest
> folds `components/interfaces/{src,specs}` into **every** component's hash, so an interface
> change invalidates all of them at once -- which is the design, not a defect. The interface
> change is the `served_by` attribution work: new `ServedBy` and `LookupOutcome` types, and
> `IDispatcher::batch_lookup` widened to return `Vec<LookupOutcome>`.
>
> **No re-analysis was performed for this component, and the digest bump asserts only what was
> actually checked**: the interface change is additive except for `batch_lookup`'s return type,
> which the compiler enforces across all implementors, and this component's own `src/**` and
> `specs/**` are unchanged. Components that implement `IDispatcher` (`dispatcher`,
> `dispatcher-p2p`) had their specs updated substantively; this one did not need it. If a later
> sweep finds drift here, this stamp is not evidence against it.

# Spec Drift Report

Generated: 2026-09-23
Project: eviction-policy-optimized

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Requirements Checked | 18 (13 FR + 5 NFR) |
| ✓ Aligned | 18 (100%) |
| ⚠️ Drifted | 0 (0%) |
| ✗ Not Implemented | 0 (0%) |
| 🆕 Unspecced Code | 0 |

> Note: `specs/001-optimized-eviction-policy/spec.md` was **backfilled from the
> implementation** in this same sync (status: Backfilled). By construction the
> spec documents current behavior, so the FR/NFR traceability below is expected
> to be aligned. The value here is (a) the FR→code mapping recorded below and
> (b) the two success-criterion violations found and fixed during analysis (see
> "Fixes applied during this sync"). Behavioral review questions live in
> `tasks.md`, not as drift.

## Fixes applied during this sync

Analysis verified each success criterion against a live build. Two SC-003
violations were found in the committed code and fixed in the same sync (both
behavior-preserving; all 27 tests still pass):

1. **`cargo clippy -- -D warnings` failed** — `needless_range_loop` in
   `CountMinSketch::increment` and `estimate` (`for row in 0..CMS_ROWS` indexing
   `CMS_PRIMES[row]`). Rewritten to `for (row, &prime) in
   CMS_PRIMES.iter().enumerate()` (`src/lib.rs:38-52`). `CMS_ROWS ==
   CMS_PRIMES.len() == 4`, so iteration count and hashing are unchanged.
2. **`cargo fmt --check` failed** — the aging guard `if` condition was split
   across two lines. Reformatted to a single line (`src/lib.rs:124`).

Post-fix: `cargo test -p eviction-policy-optimized` (27 passed),
`cargo clippy -p eviction-policy-optimized -- -D warnings` (clean),
`cargo fmt -p eviction-policy-optimized --check` (clean). SC-001..SC-004 hold.

## Detailed Findings

### Spec: 001-optimized-eviction-policy — Optimized (TinyLFU) Eviction Policy Component

#### Aligned ✓

- **FR-001** create_pool → sequential IDs from 0; per-pool
  `lru`/`sketch`/`access_count`/`max_len` → `src/lib.rs:89-102`.
- **FR-002** track: sketch increment, aging, admission, handle return;
  `_semantics` ignored (`:108`); non-idempotent re-registration → `src/lib.rs:104-140`.
- **FR-003** touch → MRU (`move_to_back`), no sketch update → `src/lib.rs:142-156`.
- **FR-004** batch_touch, one lock per pool-run, empty no-op → `src/lib.rs:158-184`.
- **FR-005** remove unlinks in O(1) → `src/lib.rs:186-200`.
- **FR-006** identify_next_to_evict → `pop_front`/`None` → `src/lib.rs:202-207`.
- **FR-007** get_eviction_candidates → `peek_front_n`, empty for bad pool → `src/lib.rs:209-218`.
- **FR-008** len → `0` for bad pool → `src/lib.rs:220-229`.
- **FR-009** clear_pool clears list, preserves sketch/counters → `src/lib.rs:231-237`
  (list `clear` only, `src/lru_list.rs:197-203`).
- **FR-010** InvalidPool on Result methods
  (`src/lib.rs:111-118,144-152,164-167,175-178,188-196`); graceful degradation
  elsewhere (`:204,211,222,233`).
- **FR-011** admission gate: non-empty pool → `push_front` when
  `estimate(key) <= estimate(head_key)` else `push_back`; empty pool →
  `push_back`; no absolute threshold → `src/lib.rs:129-137`.
- **FR-012** idempotent touch/remove on stale handle → guarded by a range check
  on the slot index, then the `active` flag, in `src/lru_list.rs:113-118`
  (move_to_back) and `:168-173` (remove).
- **FR-013** aging: halve every `max_len * 10` accesses → `src/lib.rs:121-127`
  (`halve` at `src/lib.rs:54-60`).
- **NFR-001** O(1) single-entry ops; sketch bounded by `CMS_ROWS`/`CMS_COLS`
  (`src/lib.rs:18-19,38-60`).
- **NFR-002** `RwLock<EvictionState>` + per-pool `Mutex` → `src/lib.rs:71-73,83,90,110,119`.
- **NFR-003** per-pool `Mutex` isolates pools; only `create_pool` write-locks
  state → `src/lib.rs:90,119`.
- **NFR-004** bounded sketch `[[u8; CMS_COLS]; CMS_ROWS]` (4×1024) → `src/lib.rs:27-29`.
- **NFR-005** Certus component model, provides IEvictionPolicy, single `logger`
  receptacle → `src/lib.rs:75-86`; drop-in via `full-optimized` profile
  (`apps/certus-server-yaml/profiles/full-optimized.yaml`).

#### Drifted ⚠️

None remaining after the fixes above.

#### Not Implemented ✗

None.

#### Unspecced Code 🆕

None. All public `IEvictionPolicy` methods, the `CountMinSketch` helper, and the
`LruList` internals are covered by FR-001..FR-013 / NFR-001..NFR-005.

## Divergence note vs. sibling branch

The sibling branch `…-all5` carries a backfilled spec for a **different** tuning
of this component (CMS `4×8192`, bucket `>> 51`, aging every `max_len*2`,
admission `new_est >= 3 && new_est > victim_est`, plus a `Once` startup banner).
This report and its spec describe **this** branch's implementation only: CMS
`4×1024`, bucket `>> 54`, aging every `max_len*10`, admission
`estimate(key) <= estimate(head_key) → push_front else push_back`, and no
startup banner. The two specs are not interchangeable.

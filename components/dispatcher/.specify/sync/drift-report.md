---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-02T20:45:00Z
spec_sync_git_commit: 787b8263
spec_sync_inputs_sha256: 2886506afcb1d6dbaa9505fa7e3c01405c045fbf588dfd2003be84865ea63deb
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec Drift Report — dispatcher

Generated: 2026-09-02
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis, then BACKFILL apply to `spec.md` (code authoritative).
Branch: `evolve-dispatcher-dw`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Drift findings this sweep | 4 |
| ⚠️ Drifted → resolved by backfill | 4 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 |

Scope of this sweep: the only hashed input that changed since the last clean
sync (`f3967e82`) is `components/dispatcher/src/lib.rs`. The net diff comes from
two bug-fix commits on this branch — `9b83ebe1` (populate stream) and `1d55b9c2`
(graceful degrade under the Check→Pin eviction race) — plus a cosmetic comment
touch from the `8f494f8d` sharding revert. All four behavioral findings are
**code-authoritative** (shipped fixes; aligning code→spec would reintroduce
vLLM-crashing failures), so each was resolved by backfilling `spec.md`.

## Detailed Findings (all resolved by BACKFILL)

### 1. Eviction of unpinned-but-unpersisted victims: drop → skip — severity: major
- Commit: `1d55b9c2`. Location: `src/lib.rs` `evict_one_clean` (~945-965).
- Spec had said (FR-024, User Story eviction narrative, acceptance scenario 3,
  edge cases, Session Q&A): an unpinned candidate with incomplete write-through
  is **dropped entirely** via `dm.remove` + `mt.remove` (data loss accepted).
- Code now **skips** such a candidate and tries the next; if nothing is
  demotable the scan returns `AllocationFailed`, which degrades the caller to an
  uncached serve. Rationale: a key a connector has just Checked resident but not
  yet Pinned has `read_ref == 0`, so `dm.remove` succeeds and silently drops it;
  the connector's ensuing load then misses → remote-forward → fatal `IoError`
  (`EngineDeadError`) in the vLLM connector. Demotion keeps the key resolvable
  (BlockDevice); a full remove does not.
- Backfill: FR-024 (§Functional Requirements), User Story eviction narrative,
  acceptance scenario 3, edge-case bullet, and Session Q&A entry all rewritten to
  "skip, never drop" with the Check→Pin rationale.
- Locked in by regression test `evict_never_drops_unpersisted_unpinned_victim`
  (commit `787b8263`): filling the tier with unpersisted, unpinned entries and
  forcing eviction must surface `AllocationFailed` and leave every key
  resolvable (never `NotExist`); reverting the fix fails the test.

### 2. `batch_lookup` dm.lookup Err retried, not misclassified — severity: minor
- Commit: `1d55b9c2`. Location: `src/lib.rs` `batch_lookup` entry classification (~2181).
- Spec (FR-039 step 1) said only "classifies all entries by dispatch-map state."
- Code now retries a `dm.lookup` `Err` locally up to 5 times — an `Err` is a
  transient `write_ref` timeout (concurrent store-commit/promote), not a miss —
  instead of collapsing it into `KeyNotFound` (which would forward a live key to
  remote-lookup and fail fatally). Defensive: the comment notes this does not
  fire under the current workload (`write_ref` windows are sub-millisecond); the
  observed degrade is restored by findings 1 and 3.
- Backfill: FR-039 step (1) extended to describe the Err = transient-timeout
  retry and why misclassification would be fatal.

### 3. Single-key cold inline `promote_and_serve` fast path removed — severity: moderate
- Commit: `1d55b9c2`. Location: `src/lib.rs` `batch_lookup` cold dispatch (~2291).
- Spec (FR-039 step 3, User Story 11 narrative) documented a single-entry cold
  inline bypass that skipped the cold-pool thread hop.
- Code now routes **all** cold entries, including a lone single-key load, through
  the pooled path. The inline fast path had no staging fallback (FR-053), so
  under memory-tier pressure it turned a survivable cold miss into a fatal load
  failure; the pooled path defers `AllocationFailed` to the staging post-pass.
- Backfill: FR-039 step (3) rewritten (single-key not special-cased; fast path
  removed) — steps (4)-(6) kept in place so the `step (5)` cross-references at
  User Story 11 remain valid — and the User Story 11 narrative updated.

### 4. `populate_from_gpu` D2H stream: warm → store — severity: moderate
- Commit: `9b83ebe1`. Location: `src/lib.rs` `populate` (~2941).
- Spec (FR-037, FR-056, Assumptions) said `populate_from_gpu` uses the `warm`
  stream for its D2H copy (rationale: it syncs before returning, so no concurrent
  H2D to overlap with).
- Code now uses the dedicated `store` stream so the D2H does not serialize behind
  concurrent H2D lookups on `warm` (PCIe full-duplex via the GPU's two copy
  engines). The prior "no concurrent H2D" rationale held only in isolation.
- Backfill: FR-037, FR-056, and the Assumptions/Implementation-Notes bullet all
  updated so both `populate_from_gpu` and `batch_populate` resolve `store`.

## Not Implemented
None.

## Unspecced Code
None. (The previously-unspecced single-key inline bypass was removed in code this
sweep; the previously-unspecced `batch_populate` remains documented as FR-059.)

## Recommendations
1. Commit this `drift-report.md` (with the freshness stamp above) together with
   the `spec.md` backfill so the CI Spec-Sync Gate sees a fresh report.
2. Follow-up (out of this sweep's scope): the source still carries two "gRPC
   handler" comments in `src/lib.rs`; a source-comment cleanup remains pending.

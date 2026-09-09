---
spec_sync_component: dispatcher
spec_sync_drift_status: clean
spec_sync_synced_at: 2026-09-09T21:33:18Z
spec_sync_git_commit: 5bb71702
spec_sync_inputs_sha256: af0388391aea2f22dc103ed59e028b2b2dd20eff09db928bdd0867811d96ac0e
spec_sync_hash_tool: scripts/spec-sync-hash.sh
---
# Spec Drift Report — dispatcher

Generated: 2026-09-09
Project: dispatcher (spec: specs/001-dispatcher-cache-interface/spec.md)
Mode: Read-only drift analysis, then BACKFILL apply to `spec.md` (code authoritative).
Branch: `fix-dispatcher-store-backpressure`

## Summary

| Category | Count |
|----------|-------|
| Specs Analyzed | 1 |
| Drift findings this sweep | 1 (two-layer store-backpressure feature) |
| ⚠️ Drifted → resolved by backfill | 1 |
| ✗ Not Implemented | 0 |
| 🆕 Unspecced Code | 0 |

Scope of this sweep (2026-09-09): since the last clean sync (spec commit
`787b8263`, 2026-09-02, inputs `2886506a…`) the hashed inputs changed in two
places, both introducing the **same feature**:

- `components/dispatcher/src/lib.rs` — `reserve_memory` now applies bounded
  store backpressure, and `populate`/`batch_populate` best-effort **drop on
  full** after the backpressure budget is spent (commits `39ffee9b`,
  `c400ced8`).
- `components/interfaces/src/idispatcher.rs` — `DispatcherConfig` gained
  `store_backpressure_ms: u64` (default 5000); `TierEventStats` gained two `u64`
  counters `store_backpressure_events` and `store_drops_on_full`.

The single behavioral finding is **code-authoritative** (shipped fix; aligning
code→spec would reintroduce the fatal vLLM `assert transfer_result.success` →
`EngineDeadError` on a full tier), so it was resolved by backfilling `spec.md`.

## Detailed Findings (resolved by BACKFILL)

### 1. Two-layer store backpressure + best-effort drop-on-full — severity: major
- Commits: `39ffee9b` (bounded store backpressure), `c400ced8` (best-effort
  drop on full after budget).
- Locations:
  - `reserve_memory` `src/lib.rs:3115` — retry loop: on `evict_and_insert`
    returning `AllocationFailed`, sleep 20 ms and retry until the
    `store_backpressure_ms` budget is exhausted, then fail fast with a final
    `evict_and_insert(...)?`. `budget.is_zero()` (config `= 0`) restores the
    original single-shot fail-fast path.
  - `populate` `src/lib.rs:2950` and `batch_populate` `src/lib.rs:3017` — after
    `reserve_memory` ultimately fails with `AllocationFailed`, both callers
    **unconditionally** treat the store as a best-effort drop: they log, bump
    `store_drops_on_full`, and return `Ok(())` **without caching**, rather than
    propagating `AllocationFailed`. This prevents the connector's fatal
    `assert transfer_result.success`.
  - `DispatcherConfig.store_backpressure_ms` and the two `TierEventStats`
    counters: `components/interfaces/src/idispatcher.rs`.
- Spec (pre-sync) documented only the read-path staging deferral (FR-053) and a
  four-field `TierEventStats` (FR-058); the store path had no backpressure or
  drop semantics, and `populate`/`batch_populate` were specified to surface
  `AllocationFailed` on a full tier.
- Why code is authoritative: dropping (not erroring) on a genuinely full tier is
  the load-bearing behavior that keeps the vLLM connector alive under store
  pressure; the bounded backpressure gives the async evictor a window to free
  space before the drop. Reverting to spec (return `AllocationFailed`) reintroduces
  the crash. See memory `certus-async-full-tier-crash` for the failure this fix
  addresses.
- **Accuracy note (verified against `src/lib.rs:3150-3202` this sweep):** the
  drop-on-full in `populate`/`batch_populate` is *unconditional* — it does not
  depend on `store_backpressure_ms`. Setting `store_backpressure_ms = 0` disables
  only the **retry** inside `reserve_memory` (fail fast on first
  `AllocationFailed`); the caller still catches that failure and drops. The spec
  text (FR-033, FR-056, FR-060, edge case) was worded to state this precisely and
  does **not** claim that `store_backpressure_ms = 0` restores an
  `AllocationFailed`-returning `populate`.
- Backfill applied to `spec.md` this sweep:
  - **New FR-060** — the two-layer store-backpressure requirement:
    (1) bounded store backpressure in `reserve_memory`; (2) best-effort
    drop-on-full in `populate`/`batch_populate` (unconditional). Complements the
    FR-053 read-path staging deferral.
  - **FR-003** (populate) — appended backpressure/drop note referencing FR-060.
  - **FR-033** (config) — added `store_backpressure_ms` (u64, default 5000) to
    the field list plus disable semantics (0 = `reserve_memory` fails fast, but
    drop-on-full per FR-060 still applies).
  - **FR-056** (`reserve_memory`) — bounded store-backpressure description,
    deadlock-free rationale, `store_backpressure_ms = 0` restores fail-fast.
  - **FR-058** — "four `u64` fields" → "six `u64` fields"; added
    `store_backpressure_events` and `store_drops_on_full`.
  - **FR-059** (`batch_populate`) — appended the drop-on-full note (`c400ced8`).
  - **Edge case** bullet — populate applies bounded backpressure then
    best-effort drops (returns `Ok(())`), does NOT return `AllocationFailed`.
  - **User Story 1** — new acceptance scenario 5 (drop-on-full returns success,
    no dispatch-map entry, increments `store_drops_on_full`).
  - New **Last Synced: 2026-09-09** metadata line summarizing the backfill.

## Not Implemented
None.

## Unspecced Code
None. The store-backpressure config field, both new counters, and the
drop-on-full behavior are now covered by FR-033/FR-056/FR-058/FR-059/FR-060.

## Recommendations
1. Commit this `drift-report.md` (with the freshness stamp above) together with
   the `spec.md` backfill and the `src/lib.rs` + `idispatcher.rs` inputs it
   certifies so the CI Spec-Sync Gate sees a fresh, matching report.
2. Follow-up (out of this sweep's scope, carried over): the source still carries
   two "gRPC handler" comments in `src/lib.rs`; a source-comment cleanup remains
   pending.

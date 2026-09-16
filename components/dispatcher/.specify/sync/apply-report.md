# Spec Sync Apply Report — dispatcher
Applied: 2026-09-09
Project: dispatcher
Spec: 001-dispatcher-cache-interface
Branch: `fix-dispatcher-store-backpressure`
Source proposals: `.specify/sync/proposals.json` (1 APPROVED)
Backup: `.specify/sync/backups/spec-001-20260909T212936Z.md.bak`

## Applied Edits (specs/** only — no source changes)

| # | Requirement | Direction | Result |
|---|-------------|-----------|--------|
| 1 | FR-060 (new) | BACKFILL | ✅ added — two-layer store backpressure: (1) bounded backpressure in `reserve_memory`; (2) unconditional best-effort drop-on-full in `populate`/`batch_populate`. Complements FR-053 read-path staging. |
| 2 | FR-003 (populate) | BACKFILL | ✅ appended backpressure/drop note referencing FR-060 |
| 3 | FR-033 (config) | BACKFILL | ✅ added `store_backpressure_ms` (u64, default 5000) + disable semantics (0 = `reserve_memory` fails fast; drop-on-full FR-060 still applies) |
| 4 | FR-056 (`reserve_memory`) | BACKFILL | ✅ bounded store backpressure description + deadlock-free rationale; `store_backpressure_ms = 0` restores fail-fast |
| 5 | FR-058 (`TierEventStats`) | BACKFILL | ✅ "four `u64` fields" → "six"; added `store_backpressure_events`, `store_drops_on_full` |
| 6 | FR-059 (`batch_populate`) | BACKFILL | ✅ appended drop-on-full note (`c400ced8`) |
| 7 | Edge case bullet | BACKFILL | ✅ populate applies bounded backpressure then best-effort drops (returns `Ok(())`), does NOT return `AllocationFailed`; `store_backpressure_ms = 0` disables only the retry |
| 8 | User Story 1 acceptance scenario 5 (new) | BACKFILL | ✅ drop-on-full returns success, no dispatch-map entry, increments `store_drops_on_full`, does NOT return `AllocationFailed` |
| 9 | Last-Synced header | (bookkeeping) | ✅ 2026-09-09 sync note added summarizing the backfill |

## Verification

- `grep -n store_backpressure_ms spec.md` → FR-033, FR-056 (and FR-060) — config field + disable semantics documented.
- `grep -n "six .u64. fields" spec.md` → FR-058 counter count updated.
- `grep -n "FR-060" spec.md` → new requirement present, cross-referenced from FR-003/FR-033/FR-056/FR-059 and the edge case.
- **Accuracy check (against `src/lib.rs:3150-3202`)**: confirmed the drop-on-full in `populate`/`batch_populate` is *unconditional*; `store_backpressure_ms = 0` disables only the `reserve_memory` retry. All spec wording (FR-033/FR-056/FR-060/edge case) states this precisely and does NOT claim `store_backpressure_ms = 0` makes `populate` return `AllocationFailed`.
- No `src/**`, `interfaces/**`, or `CLAUDE.md` files modified. No cargo build/test run (sync is analysis + doc only).

## Deferred (out of editing scope)

- Two `src/lib.rs` "gRPC handler" **source comments** still reference gRPC. Recorded as a code-side follow-up; NOT changed here (source is outside the `.specify/sync/**` + `specs/**` editing scope). Carried over from the prior sweep.

## Drift status after apply

**clean** — the single actionable spec↔code drift identified this cycle (the
two-layer store-backpressure feature) was resolved by the approved `specs/**`
backfill. The only residual item is a documentation-only source-comment cleanup,
out of this sync's editable scope; it does not represent unresolved spec drift.

# Spec Sync Apply Report — dispatcher
Applied: 2026-08-31
Project: dispatcher
Spec: 001-dispatcher-cache-interface
Branch: `sync-tmp`
Source proposals: `.specify/sync/proposals.json` (6 APPROVED)
Backup: `.specify/sync/backups/specs/001-dispatcher-cache-interface/spec.md.bak`

## Applied Edits (specs/** only — no source changes)

| # | Requirement | Direction | Result |
|---|-------------|-----------|--------|
| 1 | FR-006 | BACKFILL | ✅ H2D warm-path copy → `memcpy_batch_async` on `warm_load` (scatter to N regions) |
| 2 | FR-037 | BACKFILL | ✅ single warm stream → per-device `warm_load` (H2D) + `warm_store` (D2H), split for PCIe overlap |
| 3 | FR-039 step (2) | BACKFILL | ✅ MemoryTier-hit H2D copies → batched `memcpy_batch_async` on `warm_load` |
| 4 | FR-040 | BACKFILL | ✅ "gRPC handler" → "shm-queue control transport" |
| 5 | FR-042 | BACKFILL | ✅ "gRPC TakeEvents stream" → "shm-queue TakeEvents stream" |
| 6 | FR-052 | BACKFILL | ✅ stream inventory → two warm streams + pipeline pair per device |
| 7 | FR-056 (`copy_gpu_to_memory_async`) | BACKFILL | ✅ → batched `memcpy_batch_async` gather on `warm_store` |
| 8 | Implementation Notes | BACKFILL | ✅ warm-path APIs → `memcpy_batch_async` scatter/gather; noted `warm_load`/`warm_store` split |
| 9 | FR-058 (new) | BACKFILL-UNSPECCED | ✅ added — `tier_event_stats()` + `TierEventCounters` |
| 10 | SC-017 (new) | BACKFILL-UNSPECCED | ✅ added — counters zero at startup, monotonic, delta = tier events |
| 11 | Last-Synced header | (bookkeeping) | ✅ 2026-08-31 sync note appended |

## Verification

- `grep memcpy_h2d_async spec.md` → only the Last-Synced rationale note (intentional). No requirement still names the removed production API.
- `grep gRPC spec.md` → only the Last-Synced note and FR-040's "since gRPC was removed" clause (both intentional rationale).
- Active requirement set now FR-001…058 (minus removed FR-020/021/022/026/027) + SC-001…017 (minus SC-008).
- No `src/**`, `interfaces/**`, or `CLAUDE.md` files modified. No cargo build/test run (sync is analysis + doc only).

## Deferred (out of editing scope)

- Two `src/lib.rs` "gRPC handler" **source comments** (near the null-stream branch of
  `copy_gpu_to_memory_async`, ~`src/lib.rs:2983`) still reference gRPC. Recorded in
  `align-tasks.md` as a code-side follow-up; NOT changed here (source is outside the
  `.specify/sync/**` + `specs/**` editing scope).

## Drift status after apply

**clean** — every actionable spec↔code drift identified this cycle was resolved by an
approved `specs/**` backfill. The only residual item is a documentation-only source-comment
cleanup, tracked in `align-tasks.md` and out of this sync's editable scope; it does not
represent unresolved spec drift.

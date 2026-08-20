# Spec-Sync Proposals — gpu-services

**Generated**: 2026-08-20
**Based on**: `.specify/sync/drift-report.{json,md}` (generated 2026-08-20)
**Component**: gpu-services

## Summary

| Direction | Count |
|---|---|
| BACKFILL | 1 |
| ALIGN (task) | 0 |
| RESOLVED | 0 |
| BACKFILL-UNSPECCED | 0 |
| HUMAN_DECISION | 0 |

The 2026-08-20 drift report found 77/78 requirements aligned across the three
specs (all heavily backfilled in prior sync rounds). A single minor drift
remained: spec 003 FR-012.

Prior-round canonical deliverables were archived alongside this file
(`proposals-20260721.{md,json}`, `apply-report-20260721.{md,json}`,
`proposals-20260807.json`) before regenerating the current-run outputs.

---

## Proposal 1 — 003-gpu-p2p-server / FR-012 — BACKFILL

**Severity**: minor
**Direction**: BACKFILL (spec → matches code)

**Rationale**: The code correctly performs chunked NVMe reads in
`--chunk-size` increments (`do_chunked_read`, `p2p_server.rs:273-323`: one
`ReadAsync` per chunk with `sectors_per_chunk = chunk_size / sector_size`,
submitted together via `BatchSubmit`, all completions awaited). The spec's
parenthetical "(not exceeding the NVMe controller's MDTS)" asserts a *runtime*
guarantee the code never implemented and was never designed to: the MDTS
ceiling is conveyed only through the `--chunk-size` CLI help text ("must not
exceed MDTS, typically 128KB", `p2p_server.rs:54`) and the 131072-byte
default — there is no runtime query, validation, or clamp of `--chunk-size`
against the device MDTS. This is intentional operator-configured behavior, not
a behavioral bug: an oversized chunk would surface as an NVMe read error
through the block-device layer (returned to the client as `ERROR: ...`), not
as silent corruption. Per the Phase-B decision rule (stale/overclaiming spec +
correct, intentional code → BACKFILL; correct spec + buggy code → ALIGN), this
is a BACKFILL, not an ALIGN.

**Before**:
> **FR-012**: All transfer handlers MUST perform NVMe reads in `--chunk-size`
> increments (not exceeding the NVMe controller's MDTS).

**After**:
> **FR-012**: All transfer handlers MUST perform NVMe reads in `--chunk-size`
> increments — `do_chunked_read` issues one async `ReadAsync` per chunk
> (`sectors_per_chunk = chunk_size / sector_size`, successive LBAs) and submits
> them together via `BatchSubmit`, awaiting all completions. Keeping
> `--chunk-size` at or below the NVMe controller's Maximum Data Transfer Size
> (MDTS) is an **operator responsibility**, communicated via the `--chunk-size`
> CLI help text ("must not exceed MDTS, typically 128KB") and the 131072-byte
> default; the server does NOT query the controller's MDTS or validate/clamp
> `--chunk-size` against it at runtime. A `--chunk-size` larger than the device
> MDTS surfaces as an NVMe read error through the block-device layer (returned
> to the client as `ERROR: <message>`), not as silent corruption.

**Additional spec edits applied**:
- US1 Acceptance Scenario 4 added — chunked reads split into
  `ceil(size / chunk-size)` chunk-sized async reads with the chunk count
  reported as `<n>` in the `OK <size> bytes (<mode>, <n> chunks)` response.
- Assumptions bullet added — MDTS chunk sizing is the operator's
  responsibility (cross-references FR-012).
- `Last-Synced: 2026-08-20` metadata line added.

**Location**: `components/gpu-services/src/bin/p2p_server.rs:54,273`
**Confidence**: HIGH

---

## ALIGN tasks

None this run (0 real behavioral-bug drifts).

## Unspecced features

None this run (drift report reported 0 unspecced; auxiliary `dma.rs`/
`gdrcopy_ffi.rs` items were backfilled into spec 002 in prior rounds).

# Spec-Sync Proposals — `remote-lookup-rdma-responder`

**Generated**: 2026-09-02 (fresh verification pass)
**Source**: `.specify/sync/drift-report.json` (regenerated 2026-09-02 — 21/24
aligned, **3 drifted**, 0 not_implemented, 2 unspecced, 0 conflicts).
**Policy**: correct spec + non-compliant code → ALIGN (fix code, never weaken spec);
spec-lag → BACKFILL. This run finds only ALIGN items + already-applied unspecced
backfills.

## Outcome summary

| Direction | Count |
|-----------|-------|
| BACKFILL (drifted req → spec matches code) | 0 |
| ALIGN (task, no code change this pass) | 3 |
| BACKFILL-UNSPECCED | 2 (both already applied in prior passes) |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

---

## 1. FR-008 — best-effort teardown-failure logging → **ALIGN** (approved)

- **Requirement**: FR-008 — "Freeing the queue pair (`rdma_destroy_qp`) is
  best-effort cleanup performed after the ERROR transition; its failure MUST be
  logged, not fatal."
- **Location read**: `src/rdma.rs:154-169` (`RealCmConn::drop`).
- **Actual**: The load-bearing safety step is aligned — `to_error()` asserts the
  QP→ERROR transition (fail-stop) *before* the ack (`src/rdma.rs:144-152`,
  `src/connection.rs:181-195`), and unknown/dead nodes are acked idempotently. But
  `Drop` calls `rdma_disconnect`/`rdma_destroy_qp`/`rdma_destroy_id` and discards
  every return code, logging nothing on failure — the "its failure MUST be logged"
  clause is unmet.
- **Direction & rationale**: FR-008 is correct and load-bearing; the gap is a
  non-compliant code path (missing diagnostic), so this is **ALIGN**, not BACKFILL.
  Do not weaken the spec to drop the logging clause. Already tracked as align-task
  Task 6 (2026-08-07). Only reachable under `--features rdma`.
- **before / after**: n/a (no spec text change).
- **Resolution**: ALIGN task re-affirmed in `align-tasks.md` (2026-09-02 section)
  with current line refs (`src/rdma.rs:154-169`). No `.rs` edit this pass.
- **approved**: true

## 2. FR-010 — `ibv_reg_mr` failure error-variant mapping → **ALIGN** (approved)

- **Requirement**: FR-010 — "If … `ibv_reg_mr` fails, `initialize()` MUST return
  `Registration`."
- **Location read**: `src/rdma.rs:300-309` (reg_mr `Err`), `src/lib.rs:203` (uniform
  `Bind` map), `src/lib.rs:191-200` (precondition `Registration` paths — correct).
- **Actual**: Register-once/expose/dereg and the unbound-receptacle /
  uninitialized-pool precondition paths correctly return `Registration`. But
  `RealCmSeam::bind` returns `Err(String)` for *all* real-CM failures, mapped
  uniformly via `.map_err(RemoteLookupRdmaResponderError::Bind)`, so a genuine
  `ibv_reg_mr` failure surfaces to the caller as `Bind`, not `Registration`.
- **Direction & rationale**: FR-010 is correct; the code returns the wrong error
  variant on a real failure path → **ALIGN**. Split `bind`'s error channel so the
  registration failure routes to `Registration`. Already tracked as align-task
  Task 5 (2026-08-07). Medium severity; only reachable under `--features rdma`.
- **before / after**: n/a (no spec text change).
- **Resolution**: ALIGN task re-affirmed in `align-tasks.md` (2026-09-02 section)
  with current line refs (`src/rdma.rs:300-309`, `src/lib.rs:203`). No `.rs` edit
  this pass.
- **approved**: true

## 3. FR-014 — ILogger diagnostics routing → **ALIGN** (approved)

- **Requirement**: FR-014 — "Diagnostics MUST route through an optional `ILogger`
  receptacle; a missing logger MUST NOT turn any operation into an error."
- **Location read**: `src/rdma.rs:459-464` (`eprintln!` bypass); `src/lib.rs:116-120`
  (aligned `log_debug` path).
- **Actual**: The primary diagnostics (`initialize`/`shutdown`) route through the
  `ILogger` receptacle and correctly tolerate a missing logger. The device
  async-event instrumentation in `drain_async_events` prints via `eprintln!`,
  bypassing `ILogger`; the accept-loop closure captures no logger handle.
- **Direction & rationale**: correct spec + non-compliant code → **ALIGN**. Do not
  weaken FR-014 to bless `eprintln!`. Already flagged in spec Known Limitations and
  the standing FR-014 align-task; re-affirmed. Low severity; `--features rdma` only.
- **before / after**: n/a (no spec text change).
- **Resolution**: ALIGN task re-affirmed in `align-tasks.md` (2026-09-02 section).
  No source edit this pass.
- **approved**: true

## 4. Unspecced — device async-event instrumentation → **BACKFILL-UNSPECCED** (already present; approved)

- **Feature**: `TAG_ASYNC` epoll fd + `drain_async_events`/`async_event_name` + FFI
  `responder_async_fd`/`responder_drain_async_event` (`src/rdma.rs:41,44-70,351-356,440-466`;
  `src/ffi.rs:294-302`; `src/wrapper.c`).
- **Direction & rationale**: best-effort operator diagnostics, no FR needed. Already
  backfilled into `spec.md` Known Limitations (2026-08-07). Verified present and
  faithful this run; no additional spec edit required.
- **before**: existing Known-Limitations bullet (2026-08-07).
- **after**: unchanged (the `eprintln!` gap it documents is carried as the FR-014
  ALIGN item above).
- **approved**: true (no-op)

## 5. Unspecced — command-bridge thread (`rdma-responder-cmd-bridge`) → **BACKFILL-UNSPECCED** (already applied; approved)

- **Feature**: dedicated thread draining the fd-less SPSC command inbox onto the
  command eventfd (`TAG_CMD`) so the accept loop's `epoll` can service commands
  (`src/rdma.rs:358-373`).
- **Direction & rationale**: FR-004 implementation mechanism. Already backfilled into
  `spec.md` FR-004 note + `data-model.md` internal entity (2026-08-20). Verified
  present and faithful this run; no additional spec edit required.
- **before / after**: unchanged from the 2026-08-20 backfill.
- **approved**: true (no-op)

# Spec-Sync Phase B — Proposals — `remote-lookup-rdma-responder`

**Generated**: 2026-08-20
**Policy**: `.specify/sync/PHASE_B_POLICY.md` (no per-component note — each item
classified by reading its `location` code: spec-lag → BACKFILL, real bug → ALIGN).
**Source**: `.specify/sync/drift-report.json` (regenerated 2026-08-20 —
23/24 aligned, **1 drifted**, 0 not_implemented, **2 unspecced**, 0 conflicts).

## Outcome summary

| Direction | Count |
|-----------|-------|
| BACKFILL (drifted req → spec matches code) | 0 |
| ALIGN (task, no code change) | 1 |
| BACKFILL-UNSPECCED | 2 |
| RESOLVED | 0 |
| HUMAN_DECISION | 0 |

---

## 1. FR-014 — ILogger diagnostics routing → **ALIGN**

- **Requirement**: FR-014 — "Diagnostics MUST route through an optional `ILogger`
  receptacle; a missing logger MUST NOT turn any operation into an error."
- **Location read**: `src/rdma.rs:459-464` (`eprintln!` bypass) and
  `src/lib.rs:116-120` (aligned `log_debug` path).
- **Actual**: The primary diagnostics (`initialize`/`shutdown`) route through the
  `ILogger` receptacle via `log_debug` and correctly tolerate a missing logger — the
  "missing logger is never an error" half of FR-014 is satisfied. However the device
  async-event instrumentation in `drain_async_events` prints via `eprintln!`
  (`src/rdma.rs:459-464`), bypassing `ILogger`.
- **Direction & rationale**: FR-014 is a **correct, agreed** requirement (route
  diagnostics through the `ILogger` receptacle, mirroring the initiator) and the
  receptacle is wired and used on the primary path — the spec is **not** stale/behind
  the code. The `eprintln!` async-event path is a genuine (Low-severity) code
  deviation from a correct spec. Per the policy decision rule (*correct spec + buggy
  code → ALIGN*), this is an **ALIGN** item, **not** a BACKFILL — the spec must not be
  weakened to bless `eprintln!`. The deviation is already flagged in the spec's Known
  Limitations and matches the standing FR-014 align-task; re-affirmed here for the
  2026-08-20 run.
- **before / after**: n/a (no spec text change — ALIGN).
- **Resolution**: ALIGN task added to `align-tasks.md` (2026-08-20 section). No `.rs`
  edit in this Markdown-only Phase-B pass.

## 2. Unspecced — device async-event instrumentation → **BACKFILL-UNSPECCED** (already present)

- **Feature**: `TAG_ASYNC` epoll fd + `drain_async_events`/`async_event_name` +
  FFI `responder_async_fd`/`responder_drain_async_event`
  (`src/rdma.rs:41,47-70,351-356,440-466`; `src/ffi.rs:297-302`; `src/wrapper.c`).
- **Direction & rationale**: drift-report `suggested_spec` = best-effort operator
  diagnostics, no FR needed unless it becomes load-bearing. This was **already
  backfilled** into `spec.md` Known Limitations on 2026-08-07 ("Device async-event
  instrumentation" bullet), which also records the `eprintln!`-vs-`ILogger` gap that
  ties to the FR-014 ALIGN item above. **Verified present** in the current spec.
- **before**: existing Known-Limitations bullet (2026-08-07).
- **after**: unchanged — faithful to current code; not promoted to an FR (not
  load-bearing).

## 3. Unspecced — command-bridge thread (`rdma-responder-cmd-bridge`) → **BACKFILL-UNSPECCED** (applied)

- **Feature**: dedicated thread draining the fd-less SPSC command inbox onto the
  command `eventfd` (`TAG_CMD`) so the accept loop's `epoll` can service commands
  (`src/rdma.rs:358-373`).
- **Direction & rationale**: drift-report `suggested_spec` = note the FR-004
  SPSC→eventfd bridge mechanism in plan.md/data-model. Behavior is implied by FR-004,
  but the bridge thread itself was an undocumented internal detail. Not a new
  user-facing FR — backfilled as an **implementation note under FR-004** (spec.md) and
  as an **internal entity** (data-model.md).
- **before**: FR-004 described the `epoll` wait over `{cm fd, command inbox, stop}`
  with no note on how the fd-less SPSC command inbox is made pollable; the data-model
  CM-seam section did not mention the bridge thread.
- **after**: `spec.md` FR-004 gains a "Command-inbox bridge note (backfilled
  2026-08-20)"; `data-model.md` gains a "Command-inbox bridge thread" internal-entity
  subsection (role / lifecycle / mock-vs-real). See `apply-report.md`.

# Sync Apply Report — `remote-lookup-rdma-responder`

**Mode**: AUTO-BACKFILL
**Applied**: 2026-07-22T23:21:11Z
**Source**: `.specify/sync/drift-report.{json,md}` (generated 2026-07-22T22:39:05Z)
**Backups**: `.specify/sync/backups/20260722T232111Z/{spec.md,plan.md}`

No `proposals.json` gate existed for this component (drift report only); per
AUTO-BACKFILL mode, resolutions below were derived and applied directly from
the drift report's findings and recommendations, respecting the hard rule that
only Markdown under `specs/**` and `.specify/sync/**` may be edited.

## Changes Made

### Specs Updated

| Spec | Section | Change Type | Reason |
|------|---------|--------------|--------|
| 001-rdma-lookup-responder/spec.md | Key Entities → `Endpoint` | Modified | Backfill/doc-fix: text said `ip` is "supplied by `set_bind_ip()` ... (never auto-detected)", contradicting FR-002a (auto-detect fallback) and its own Clarifications session in the same file. Rewritten to match FR-002a's resolution precedence. Found during this pass (not separately flagged in drift-report.json, but same root cause as the interfaces-crate conflict it does flag). |
| 001-rdma-lookup-responder/spec.md | New "Build & Feature Flags" section (before "Known Limitations") | Backfill | Drift-report unspecced-feature item + recommendation #4: names the `rdma` Cargo feature explicitly, documents the build-configuration `Bind` error as distinct from the FR-002/FR-010 runtime `Bind`/`Registration` failure modes. Also cross-referenced the pre-existing `telemetry` feature (FR-016) for completeness. |
| 001-rdma-lookup-responder/plan.md | "Testing" / "Target Platform" | Backfill (supporting) | Named `--features rdma` explicitly in the test-command list and cross-referenced the new spec.md section, per recommendation #4's "spec/plan/contract docs" latitude. |

### New Specs Created

None (NEW_SPEC: n/a per task scope).

### Superseded

None (SUPERSEDE: n/a per task scope).

### Implementation Tasks Generated

4 tasks appended to `.specify/sync/align-tasks.md` (file did not previously
exist — created fresh):

1. **FR-016** — wire `TelemetryCollector::record_accept_loop_error()` into a real failure branch (currently dead code; counter permanently 0). Severity: medium.
2. **Contract §3** — construct `ResponderEvent::Error` on the QP-creation-failure branch (`src/rdma.rs:373-382`, `accept_child`); currently the failure is silently `rdma_reject`ed with no signal to `remote-lookup`. Severity: medium.
3. **FR-014** — thread a logger handle into the accept-loop closure so `RealCmSeam` failure branches can emit `ILogger` diagnostics; today only `initialize()`/`shutdown()` log. Severity: low-medium.
4. **Interfaces doc comment** — `components/interfaces/src/iremote_lookup_rdma_responder.rs:256-262` (`set_bind_ip`/`initialize` on `IRemoteLookupRdmaResponderAdmin`) states the pre-clarification "never auto-detects" behavior, contradicting FR-002a and the shipped code. Flagged as an align-task rather than fixed directly — the file is a `.rs` source file outside this pass's writable scope (`components/interfaces/**`, not `specs/**`/`.specify/sync/**`).

Tasks 1–3 are code DEFECTS per the assignment framing (spec is correct/
load-bearing; code declares the surface but never exercises it on the real
failure path) — resolved by future code changes, not spec weakening. Task 4 is
a doc defect in a shared crate outside this component's spec-sync write scope.

### Not Applied / Deferred

| Item | Reason |
|------|--------|
| Editing `components/interfaces/src/iremote_lookup_rdma_responder.rs` directly | Out of scope for this spec-sync apply pass (source `.rs` file, not under `specs/**` or `.specify/sync/**`); filed as align-task #4 instead. |
| Wiring `record_accept_loop_error()`, `ResponderEvent::Error`, or `ILogger` calls into `src/rdma.rs`/`src/lib.rs`/`src/connection.rs` | Source-code changes are explicitly out of scope for spec-sync apply; filed as align-tasks #1-3. |

No proposal was rejected outright — every drift item in the report is
accounted for by either a spec/doc backfill (the unspecced `rdma` feature +
the same-doc stale Endpoint description) or an align-task (the three
error/diagnostics-path defects + the cross-doc stale comment). Nothing was
DEFERRED as "unsure" — all four remaining items had clear, unambiguous
resolutions per the hard rules given for this pass.

## Next Steps

1. Review the updated `spec.md` / `plan.md` sections above.
2. Implement the four tasks in `.specify/sync/align-tasks.md` in a follow-up code PR (tasks 1 and 2 should land together — both consume the same QP-creation-failure signal; task 4 touches the shared `components/interfaces` crate and should be checked for other stale copies of the same claim while there).
3. Commit spec changes: `git add components/remote-lookup-rdma-responder/specs/ components/remote-lookup-rdma-responder/.specify/sync/ && git commit -m "spec-sync: backfill rdma feature docs, align-tasks for responder error path"`.

---

# 2026-08-07 Sweep

Applied: 2026-08-07 on branch `sync/spec-drift-sweep-20260807`
Mode: Interactive drift sweep (`component-sync-specs`, all specced components)
Source: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-07T15:28:11Z).

Drift headline: 19/23 aligned. Four Drifted (FR-008 Low, FR-010 Medium, FR-014
Low, FR-016 Low), 2 unspecced items, 1 stale interface-doc conflict. The July
pass already filed align-tasks for FR-016 wiring, `ResponderEvent::Error`,
FR-014 logger routing, and the interfaces-doc conflict; those still stand. This
sweep adds two new ALIGN items (FR-008, FR-010) and two spec BACKFILLs, and
raises the one genuine HUMAN_DECISION fork (FR-016) with the maintainer.

## Changes Made (branch only, nothing committed to `unstable`)

### Specs Updated (BACKFILL — document reality; applied)

| Spec | Requirement | Change |
|------|-------------|--------|
| 001-rdma-lookup-responder | FR-010 | Added an access-flags note: code registers `LOCAL_WRITE | REMOTE_WRITE | REMOTE_READ` (`src/rdma.rs:297-299`); `REMOTE_READ` exceeds the stated write-only minimum and is retained deliberately under the trusted-fabric assumption, with an explicit security caveat. **No code change** (deliberate keep per maintainer decision). |
| 001-rdma-lookup-responder | Known Limitations | Backfilled the device async-event instrumentation (`TAG_ASYNC` epoll + `drain_async_events`) as best-effort operator diagnostics, noting the `eprintln!`-vs-`ILogger` gap ties to the FR-014 align-task |
| 001-rdma-lookup-responder | Known Limitations | Backfilled a `REMOTE_READ`-narrowing follow-up bullet (low-risk hardening if no remote-read consumer appears) |

### Code

**FR-016 wire-up DRAFTED on branch** (per the maintainer's fork decision — see
"Human Decision Resolved" below): `src/connection.rs`, `src/rdma.rs`,
`src/lib.rs` (+ new test). Verified: 22 mock tests / 24 with `telemetry`,
`--features rdma` type-checks, fmt + clippy clean. Staged for review, **not**
committed to `unstable`.

The two other new drift items (FR-008, FR-010) are Low/Medium ALIGN and remain
**queued, not drafted** (sweep pacing: only HIGH code bugs — or an explicit
maintainer fork decision, as with FR-016 — get a drafted fix). Both are
reachable only under `--features rdma` on real hardware, so they are invisible
to the default-members/CI build.

### Implementation Tasks (see align-tasks.md 2026-08-07 section)

- **Task 5 — FR-010** error-mapping (`ibv_reg_mr` failure → `Registration`, not
  `Bind`; `src/lib.rs:195-196`): **queued** (Medium).
- **Task 6 — FR-008** log best-effort `rdma_destroy_qp`/`rdma_disconnect`
  failures in `RealCmConn::drop` (`src/rdma.rs:144-169`): **queued** (Low).
- July Tasks 1–4 (FR-016 wiring, `ResponderEvent::Error`, FR-014 logger,
  interfaces-doc): unchanged, still open.

## Human Decision Resolved — FR-016 → (A) Wire it up (DRAFTED on branch)

The one genuine fork in the responder was raised with the maintainer, who chose
**(A) wire it up**. `record_accept_loop_error()` was defined and unit-tested but
**never called** in production, and `ResponderEvent::Error` was **never
constructed** anywhere (`src/telemetry.rs`, `src/connection.rs:148-158,184`;
drift-report FR-016 + Recommendation 5). The fix is **drafted on branch
`sync/spec-drift-sweep-20260807`** (source changes, staged for review, NOT
committed to `unstable`):

| File | Change |
|------|--------|
| `src/connection.rs` | New `CmEvent::AcceptError { message }` variant (non-fatal accept-loop error). New `ConnectionTable::record_accept_loop_error()` forwarding to telemetry (keeps the collector encapsulated in the table). `MockCmSeam` now carries `VecDeque<CmEvent>` and gained `inject_accept_error(msg)` for tests. |
| `src/rdma.rs` | `accept_child` now returns `Result<_, String>` with meaningful messages (`rdma_create_qp`/`rdma_accept` failure). `drain_cm_events`'s reject branch pushes `CmEvent::AcceptError { message }` after `rdma_reject` (the connect is still rejected first). |
| `src/lib.rs` | `run_accept_loop` handles `CmEvent::AcceptError` → `table.record_accept_loop_error()` + `send_event(ResponderEvent::Error { message })` (lossless per FR-011a). New test `accept_error_surfaces_a_responder_error_event` asserts exactly one `Error` event, then channel close, and (under `telemetry`) the counter == 1. |

**Behavioral change**: `remote-lookup` will now receive `ResponderEvent::Error`
events on non-fatal accept-loop failures it never received before — delivered
losslessly on the same channel as `ConnectionEstablished`/`DisconnectAck`
(FR-011a). Fatal HCA/programming faults still fail-stop and are **not** rerouted
through `Error` (only the `accept_child` QP-formation reject path emits it).

**Verification** (branch, not committed): `cargo test -p
remote-lookup-rdma-responder` → 22 pass (mock); `--features telemetry` → 24 pass
(counter assertion active); `cargo check --features rdma` clean; `cargo fmt
--check` + `cargo clippy` (default and `telemetry,rdma`) clean.

This closes July align-tasks 1 (FR-016 wiring) and 2 (`ResponderEvent::Error`)
under option (A) — they are now implemented, not merely queued.

### Not Applied / Deferred

- FR-008, FR-010 code fixes: queued as align-tasks (not drafted — non-HIGH).
- All source-code and `components/interfaces/**` edits remain out of scope for
  this Markdown-only pass (consistent with the July pass).

## Backups

- `.specify/sync/backups/20260722T232111Z/{spec.md,plan.md}` (July pass). The
  2026-08-07 spec edits build on those; branch history on
  `sync/spec-drift-sweep-20260807` is the recovery path for this pass.

## Next Steps

1. **Review the drafted FR-016 wire-up** (`src/connection.rs`, `src/rdma.rs`,
   `src/lib.rs`) on the branch — in particular confirm `remote-lookup` handles
   the newly-emitted `ResponderEvent::Error` events appropriately.
2. Implement queued align-tasks 5 (FR-010) and 6 (FR-008) at maintainer discretion.
3. Fix the stale `set_bind_ip` interface doc (July task 4) — shared crate.
4. Commit on the branch only — never to `unstable`.

---

# 2026-08-20 Phase B

**Mode**: Spec-Sync Phase B (shared `PHASE_B_POLICY.md`; no per-component note —
each drift item classified by reading its `location` code).
**Applied**: 2026-08-20
**Source**: `.specify/sync/drift-report.{json,md}` (regenerated 2026-08-20)
**Proposals gate**: `.specify/sync/proposals.{md,json}` (this run)
**Backups**: `.specify/sync/backups/specs/001-rdma-lookup-responder/{spec.md,data-model.md}.bak`
**Scope guard**: only Markdown under `specs/**` and `.specify/sync/**` edited; no
`.rs` source touched; no cargo run.

Drift headline: **23/24 aligned, 1 drifted (FR-014, Low), 0 not_implemented, 2
unspecced, 0 conflicts.** The regenerated report reflects the 2026-08-07 sweep's
FR-016 wire-up and FR-008/FR-010 backfills as resolved/aligned; the only remaining
drift is the FR-014 `eprintln!`-vs-`ILogger` gap.

## Changes Made

### Specs Updated

| Spec | Section | Requirement | Change Type | Reason |
|------|---------|-------------|-------------|--------|
| 001-rdma-lookup-responder/spec.md | Functional Requirements → FR-004 | Unspecced (command-bridge) | BACKFILL-UNSPECCED (added note) | Documents the `rdma-responder-cmd-bridge` SPSC→eventfd bridge thread (`src/rdma.rs:358-373`) that makes the fd-less SPSC command inbox pollable by the accept loop's `epoll`. Realizes FR-004's "command inbox" wait arm; no new externally visible behavior. |
| 001-rdma-lookup-responder/data-model.md | Internal entities (after CM seam) | Unspecced (command-bridge) | BACKFILL-UNSPECCED (new subsection) | Adds a "Command-inbox bridge thread" internal entity (role / lifecycle / real-vs-mock), per the drift-report suggestion to note the FR-004 bridge in plan.md/data-model. |

### Align Tasks Generated

| Task | Requirement | Severity | Status | Files (follow-up code PR) |
|------|-------------|----------|--------|---------------------------|
| Align FR-014 — route async-event diagnostics through `ILogger` | FR-014 | Low | Queued (Markdown-only pass; no source edited) | `src/rdma.rs` (`drain_async_events`, seam logger handle), `src/lib.rs` (`initialize_inner` closure capture) |

### Unspecced Backfilled

| Feature | Location | Resolution |
|---------|----------|------------|
| Device async-event instrumentation (`TAG_ASYNC`, `drain_async_events`, `async_event_name`, FFI shims) | `src/rdma.rs:41,47-70,351-356,440-466`; `src/ffi.rs:297-302`; `src/wrapper.c` | **Already backfilled** (spec.md Known Limitations, 2026-08-07). Verified present and faithful this run; no FR promotion (best-effort, not load-bearing). The `eprintln!` gap it documents is carried as the FR-014 ALIGN task above. |
| Command-bridge thread `rdma-responder-cmd-bridge` (SPSC→eventfd) | `src/rdma.rs:358-373` | **Backfilled this run** — FR-004 implementation note in spec.md + "Command-inbox bridge thread" entity in data-model.md. |

### Resolved

None. (The 2026-08-07 sweep's FR-016/FR-008/FR-010 items are already reflected as
aligned/resolved in the regenerated drift report and are not re-listed as drift.)

### Not Applied / Deferred

| Item | Reason |
|------|--------|
| Routing `drain_async_events` through `ILogger` (the FR-014 code fix) | Source `.rs` change — out of scope for this Markdown-only Phase-B pass; filed as the FR-014 align-task. |
| Promoting async-event instrumentation to an FR | Best-effort operator diagnostic, not load-bearing (drift-report `suggested_spec`); kept in Known Limitations. |

## Backups

- `.specify/sync/backups/specs/001-rdma-lookup-responder/spec.md.bak`
- `.specify/sync/backups/specs/001-rdma-lookup-responder/data-model.md.bak`

(Both taken immediately before the 2026-08-20 edits, per policy
`backups/<same-relative-path>.bak` convention. Earlier July/August backups remain
under `backups/20260722T232111Z/`.)

## Next Steps

1. Review the FR-004 bridge-note (spec.md) and the new data-model entity.
2. Implement the FR-014 align-task in a follow-up code PR (route
   `drain_async_events` through `ILogger`; pairs with July Task 6 / FR-008 `Drop`
   logging — both want the same accept-loop logger handle).
3. Commit spec/sync Markdown on a feature branch only — never to `unstable`.
